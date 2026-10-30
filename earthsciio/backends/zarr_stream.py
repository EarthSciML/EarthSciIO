"""The streaming **Zarr v3** writer contract (``spec/writer.md``, RFC U12) —
regions, roles and the commit protocol, indexed records, statics, blobs,
dtypes, read-back and resume — on top of zarr-python 3.x.

:class:`earthsciio.backends.zarr_write.ZarrWriter` is the public facade; every
``ZarrWriter.write_*`` method delegates to a :class:`ZarrStreamHandle` built
here by :func:`open_writer` / :func:`resume_writer`. The read-back functions
(:func:`read_output_manifest`, :func:`read_record`, :func:`read_static`,
:func:`read_blob`) are module level and read committed data straight from the
store (never through the content-addressed cache).

How the rules map onto zarr-python:

* **Whole shards only.** Every writer buffers its region of each open time
  shard and writes it as ONE whole-shard selection (padded with fill where
  records are missing; rewritten whole when they arrive). The write goes
  through an *in-memory* array — ``AsyncArray(metadata=meta.update_shape(...))``
  — whose shape is the persisted one rounded UP to whole shards on every dim
  (time included), so even an edge shard (``stop == len``) and a shard beyond
  the committed ``shape[time]`` is a complete selection. That array object is
  never saved: no writer writes ``zarr.json`` except the owner, at create and
  at commit.
* **Verified (zarr-python 3.4.2, ``BatchedCodecPipeline``):** a selection that
  covers a whole shard is encoded from the new values alone — the store is NOT
  read (the codec pipeline's ``is_complete_chunk`` skips the GET). A selection
  that covers only part of a shard, including the in-bounds part of an edge
  shard at the true array shape, IS read-modify-written; the rounded-up
  in-memory shape is what avoids that read. ``tests/test_zarr_stream.py``
  pins this with a store that records every GET. An all-fill shard is deleted
  instead of written (``write_empty_chunks=False``), i.e. "erased".
* **Commit point.** ``output_manifest.json`` is written last; readers trust only
  the manifest and the ``zarr.json`` shapes, so shard objects beyond the
  committed state are invisible and are overwritten later.

The v1 path. A ``single`` writer in ``append`` mode with no statics writes the
``earthsciio/output-manifest/v1`` manifest byte-for-byte as before this
contract (``written_at`` aside), keeps its legacy auto-commit (the manifest is
refreshed whenever a shard's worth of records is pending, exactly when the old
writer flushed), and keeps its ``time_shards`` list of flushed batches.
"""

from __future__ import annotations

import bisect
import contextlib
import copy
import hashlib
import itertools
import json
import math
import re
import struct
from collections.abc import Iterable, Sequence
from dataclasses import dataclass, field
from datetime import datetime, timezone
from typing import Any, Union

import numpy as np

from ..errors import CommitError, EarthSciIOError, RegionNotShardAligned
from .zarr_write import (
    IndexedTime,
    OutputSchema,
    OutputVar,
    ZarrWriter,
    ZstdProfile,
    _open_output_store,
    _profile,
    _v3_dtype,
    _v3_fill,
)

__all__ = [
    "COMMIT_TOKEN_SCHEMA",
    "OUTPUT_MANIFEST_V1",
    "OUTPUT_MANIFEST_V2",
    "CommitToken",
    "IndexedTime",
    "OutputSchema",
    "OutputVar",
    "ZarrStreamHandle",
    "ZarrWriter",
    "open_writer",
    "read_blob",
    "read_output_manifest",
    "read_record",
    "read_static",
    "resume_writer",
    "schema_fingerprint",
]

COMMIT_TOKEN_SCHEMA = "earthsciio/commit-token/v1"
OUTPUT_MANIFEST_V1 = "earthsciio/output-manifest/v1"
OUTPUT_MANIFEST_V2 = "earthsciio/output-manifest/v2"
MANIFEST_KEY = "output_manifest.json"

_BLOB_KEY = re.compile(r"[A-Za-z0-9._-]+\Z")

#: A region: ``{dim: (start, stop)}`` over non-time dims, 0-based half-open.
Region = dict[str, tuple[int, int]]
#: A blob scope: ``"static"`` or ``("record", i)``.
BlobScope = Union[str, tuple[str, int]]


# --- small helpers ----------------------------------------------------------- #


def _roundup(n: int, s: int) -> int:
    return -(-n // s) * s


def _f64_bits(x: float) -> bytes:
    return struct.pack("<d", float(x))


def _same_t(a: float, b: float) -> bool:
    """Bitwise float64 equality (distinguishes ``0.0`` from ``-0.0``)."""
    return _f64_bits(a) == _f64_bits(b)


def _now() -> str:
    return datetime.now(timezone.utc).replace(microsecond=0).strftime("%Y-%m-%dT%H:%M:%SZ")


def _norm_region(region: Any | None) -> Region:
    """Normalize a region to ``{dim: (start, stop)}`` sorted by dim name."""
    if region is None:
        return {}
    items = region.items() if hasattr(region, "items") else region
    out = {}
    for d, rng in items:
        a, b = rng
        out[str(d)] = (int(a), int(b))
    return dict(sorted(out.items()))


def _region_wire(region: Region) -> dict[str, list[int]]:
    """The wire form ``{"lat": [0, 48]}``, canonical order by dim name."""
    return {d: [a, b] for d, (a, b) in sorted(region.items())}


def _check_region(schema: OutputSchema, region: Region) -> None:
    """The alignment rule of ``spec/writer.md`` §4 (raises before any I/O)."""
    dimlen = schema.dim_lengths
    for d, (a, b) in region.items():
        if d == schema.time_dim:
            raise ValueError(f"a region may not name the time dim {d!r}")
        if d not in dimlen:
            raise ValueError(f"region names unknown dim {d!r}")
        n, s = dimlen[d], schema.shard_shape[d]
        if not (0 <= a < b <= n):
            raise RegionNotShardAligned(f"region {d}=[{a}, {b}) is out of range for length {n}")
        if a % s != 0:
            raise RegionNotShardAligned(
                f"region {d}=[{a}, {b}): start {a} is not a multiple of the shard length {s}"
            )
        if b % s != 0 and b != n:
            raise RegionNotShardAligned(
                f"region {d}=[{a}, {b}): stop {b} is neither a multiple of the "
                f"shard length {s} nor the dim length {n}"
            )


def _parse_scope(scope: BlobScope) -> tuple[str, int | None]:
    if scope == "static":
        return "static", None
    if (
        isinstance(scope, (tuple, list))
        and len(scope) == 2
        and scope[0] == "record"
        and int(scope[1]) >= 0
    ):
        return "record", int(scope[1])
    raise ValueError(f"blob scope must be 'static' or ('record', i), got {scope!r}")


def _blob_key(scope: str, record: int | None, writer: int, key: str) -> str:
    if scope == "static":
        return f"blobs/static/{writer}/{key}"
    return f"blobs/record/{record}/{writer}/{key}"


def _blob_sort(b: tuple[str, int | None, Any, Any]):
    return (b[0], -1 if b[1] is None else b[1], *b[2:])


def _ranges(indices: Iterable[int]) -> list[list[int]]:
    """Sorted indices -> disjoint half-open ``[[start, stop), ...]`` ranges."""
    out: list[list[int]] = []
    for i in sorted(indices):
        if out and out[-1][1] == i:
            out[-1][1] = i + 1
        else:
            out.append([i, i + 1])
    return out


def _codec_dict(profile: str) -> dict[str, Any]:
    c = _profile(profile)
    if isinstance(c, ZstdProfile):
        return {"id": "zstd", "level": c.level, "checksum": c.checksum}
    return {
        "id": "blosc",
        "cname": c.cname,
        "clevel": c.clevel,
        "shuffle": "shuffle" if c.shuffle else "noshuffle",
    }


def _compressor(profile: str):
    from zarr.codecs import BloscCodec, ZstdCodec

    c = _profile(profile)
    # Blosc(zstd) for diagnostic/checkpoint; the PLAIN v3 zstd codec for wasm.
    if isinstance(c, ZstdProfile):
        return ZstdCodec(level=c.level)
    return BloscCodec(
        cname=c.cname, clevel=c.clevel, shuffle="shuffle" if c.shuffle else "noshuffle"
    )


# --- store access ------------------------------------------------------------ #


def _open_store(base_url: str, *, create: bool):
    """``(store, base)`` for ``base_url``. ``create`` makes a local directory
    (the owner's create); otherwise nothing is created."""
    if create:
        return _open_output_store(base_url)
    from zarr.storage import FsspecStore, LocalStore

    if "://" in base_url and not base_url.startswith("file://"):
        return FsspecStore.from_url(base_url, read_only=False), base_url
    path = base_url[len("file://") :] if base_url.startswith("file://") else base_url
    return LocalStore(path), path


def _open_read_store(base_url: str):
    from zarr.storage import FsspecStore, LocalStore

    if "://" in base_url and not base_url.startswith("file://"):
        return FsspecStore.from_url(base_url, read_only=True)
    path = base_url[len("file://") :] if base_url.startswith("file://") else base_url
    return LocalStore(path, read_only=True)


def _get_bytes(store, key: str) -> bytes | None:
    from zarr.core.buffer import default_buffer_prototype
    from zarr.core.sync import sync

    buf = sync(store.get(key, prototype=default_buffer_prototype()))
    return None if buf is None else buf.to_bytes()


def _put_bytes(store, key: str, data: bytes) -> None:
    from zarr.core.buffer import default_buffer_prototype
    from zarr.core.sync import sync

    sync(store.set(key, default_buffer_prototype().buffer.from_bytes(data)))


def _read_manifest(store) -> dict[str, Any] | None:
    raw = _get_bytes(store, MANIFEST_KEY)
    return None if raw is None else json.loads(raw)


def _load_meta(store, name: str):
    from zarr.core.metadata import ArrayV3Metadata

    raw = _get_bytes(store, f"{name}/zarr.json")
    if raw is None:
        raise EarthSciIOError(f"array {name!r} has no zarr.json in the store")
    return ArrayV3Metadata.from_dict(json.loads(raw))


# --- the schema fingerprint (spec/writer.md §3.1) ---------------------------- #


def schema_fingerprint(schema: OutputSchema) -> str:
    """``"sha256:" + hex(sha256(canonical_json(F)))`` — identical in Rust and
    Julia (``spec/writer.md`` §3.1). ``F`` holds no floats."""
    indexed = schema.indexed
    n = schema.time_mode.n_records if indexed else 0
    dims = [d for d, _ in schema.dims]
    F = {
        "chunk_shape": {d: schema.chunk_shape[d] for d in dims},
        "dims": [[d, n if d == schema.time_dim else ln] for d, ln in schema.dims],
        "profile": schema.profile,
        "shard_shape": {d: schema.shard_shape[d] for d in dims},
        "statics": [[nm, list(v.dims), _v3_dtype(v.dtype)] for nm, v in schema.statics],
        "time_dim": schema.time_dim,
        "time_mode": "indexed" if indexed else "append",
        "vars": [[nm, list(v.dims), _v3_dtype(v.dtype)] for nm, v in schema.vars],
    }
    canon = json.dumps(F, sort_keys=True, separators=(",", ":"), ensure_ascii=True)
    return "sha256:" + hashlib.sha256(canon.encode("ascii")).hexdigest()


def _validate_schema(schema: OutputSchema) -> None:
    _profile(schema.profile)
    dimlen = schema.dim_lengths
    if schema.time_dim not in dimlen:
        raise ValueError(f"time_dim {schema.time_dim!r} is not in dims")
    for d, _ in schema.dims:
        if d not in schema.chunk_shape:
            raise ValueError(f"dim {d!r} missing from chunk_shape")
        if d not in schema.shard_shape:
            raise ValueError(f"dim {d!r} missing from shard_shape")
        c, s = schema.chunk_shape[d], schema.shard_shape[d]
        if c <= 0 or s <= 0 or s % c != 0:
            raise ValueError(
                f"shard_shape[{d}]={s} is not a nonzero multiple of chunk_shape[{d}]={c}"
            )
    names: set[str] = set()
    for kind, entries in (("streaming var", schema.vars), ("static", schema.statics)):
        for nm, ov in entries:
            if nm in names or nm == schema.time_dim:
                raise ValueError(f"duplicate variable name {nm!r}")
            names.add(nm)
            _v3_dtype(ov.dtype)
            for d in ov.dims:
                if d not in dimlen:
                    raise ValueError(f"{kind} {nm!r} uses unknown dim {d!r}")
            has_t = schema.time_dim in ov.dims
            if kind == "streaming var" and not has_t:
                raise ValueError(
                    f"streaming var {nm!r} must include the time dim {schema.time_dim!r}"
                )
            if kind == "static" and has_t:
                raise ValueError(f"static {nm!r} must not include the time dim {schema.time_dim!r}")
    if schema.indexed and np.dtype(schema.time_dtype) != np.float64:
        raise ValueError("the time coordinate of an indexed store is float64")


def _schema_from_manifest(m: dict[str, Any]) -> OutputSchema:
    """Rebuild the stream schema from a v1 or v2 manifest (``spec/writer.md``
    §8.2). Coordinates and attrs other than the variables' are not needed by a
    joining / resuming writer and are left empty."""
    time_dim = m["time_dim"]
    dims = []
    for e in m["dims"]:
        d, n = (e["name"], e["length"]) if isinstance(e, dict) else (e[0], e[1])
        dims.append((d, int(n)))

    def _vars(key):
        return [
            (v["name"], OutputVar(v["dims"], v["dtype"], v.get("attrs") or {}))
            for v in m.get(key, [])
        ]

    tm = m.get("time_mode") or {"kind": "append"}
    if tm["kind"] == "indexed":
        entries = sorted(m["records"]["entries"], key=lambda e: e["index"])
        time_mode: Any = IndexedTime([float(e["t"]) for e in entries])
    else:
        time_mode = "append"
        dims = [(d, 0 if d == time_dim else n) for d, n in dims]
    return OutputSchema(
        dims=dims,
        time_dim=time_dim,
        vars=_vars("vars"),
        chunk_shape=dict(m["chunk_shape"]),
        shard_shape=dict(m["shard_shape"]),
        profile=m["profile"],
        statics=_vars("statics") if m["schema"] == OUTPUT_MANIFEST_V2 else [],
        time_mode=time_mode,
    )


# --- the commit token (spec/writer.md §5.2) ---------------------------------- #


@dataclass
class CommitToken:
    """What one writer hands the owner after :meth:`ZarrStreamHandle.flush`.

    A plain value (``to_json`` / ``from_json`` give the documented wire form
    ``earthsciio/commit-token/v1``); the caller moves it (MPI gather, in
    memory, …). ``records`` are ``(index, t)`` pairs, ``blobs`` are
    ``(scope, record, key)`` triples (``record`` is ``None`` for ``static``),
    ``objects`` the store keys PUT or erased (advisory)."""

    writer: int
    region: Region = field(default_factory=dict)
    fingerprint: str = ""
    records: list[tuple[int, float]] = field(default_factory=list)
    statics: list[str] = field(default_factory=list)
    blobs: list[tuple[str, int | None, str]] = field(default_factory=list)
    objects: list[str] = field(default_factory=list)
    schema: str = COMMIT_TOKEN_SCHEMA

    @property
    def empty(self) -> bool:
        """No records, statics or blobs (``objects`` are advisory)."""
        return not (self.records or self.statics or self.blobs)

    def to_dict(self) -> dict[str, Any]:
        return {
            "schema": self.schema,
            "writer": int(self.writer),
            "region": _region_wire(_norm_region(self.region)),
            "fingerprint": self.fingerprint,
            "records": [{"index": int(i), "t": float(t)} for i, t in self.records],
            "statics": list(self.statics),
            "blobs": [{"scope": s, "record": r, "key": k} for s, r, k in self.blobs],
            "objects": list(self.objects),
        }

    def to_json(self) -> str:
        """The wire form. Floats use Python's shortest round-trip repr, so
        ``t`` survives the trip bitwise."""
        return json.dumps(self.to_dict())

    @classmethod
    def from_dict(cls, d: dict[str, Any]) -> CommitToken:
        if d.get("schema") != COMMIT_TOKEN_SCHEMA:
            raise ValueError(f"not a commit token: schema {d.get('schema')!r}")
        return cls(
            writer=int(d["writer"]),
            region=_norm_region(d.get("region") or {}),
            fingerprint=str(d["fingerprint"]),
            records=[(int(e["index"]), float(e["t"])) for e in d.get("records", [])],
            statics=[str(s) for s in d.get("statics", [])],
            blobs=[
                (
                    str(b["scope"]),
                    None if b.get("record") is None else int(b["record"]),
                    str(b["key"]),
                )
                for b in d.get("blobs", [])
            ],
            objects=[str(o) for o in d.get("objects", [])],
        )

    @classmethod
    def from_json(cls, text: str | bytes | dict[str, Any]) -> CommitToken:
        return cls.from_dict(text if isinstance(text, dict) else json.loads(text))


# --- whole-shard writes ------------------------------------------------------ #

_WHOLE_SHARD_PIPELINES: dict[type, type] = {}


def _whole_shard_pipeline(p):
    """``p`` with partial encoding turned off. A sharded array's pipeline
    supports partial encoding, and older zarr-python (3.1.x) then reads the stored
    shard back even for a write that covers it whole; without partial encoding
    a complete shard goes through the plain encode, which never reads. Every
    write here covers whole shards, so nothing is lost. A pipeline type that is
    not a dataclass is returned as is."""
    import dataclasses

    cls = type(p)
    if not dataclasses.is_dataclass(p) or not hasattr(cls, "supports_partial_encode"):
        return p
    sub = _WHOLE_SHARD_PIPELINES.get(cls)
    if sub is None:
        sub = type(
            f"_WholeShard{cls.__name__}",
            (cls,),
            {"supports_partial_encode": property(lambda self: False)},
        )
        sub = dataclasses.dataclass(frozen=True)(sub)
        _WHOLE_SHARD_PIPELINES[cls] = sub
    return sub(**{f.name: getattr(p, f.name) for f in dataclasses.fields(p) if f.init})


# --- per-array geometry ------------------------------------------------------ #


class _Geo:
    """Where one array (streaming var, static, or the time coordinate) sits
    relative to a writer's region."""

    def __init__(
        self, schema: OutputSchema, name: str, ov: OutputVar, region: Region, role: str
    ) -> None:
        self.name = name
        self.dims = list(ov.dims)
        self.dtype = np.dtype(ov.dtype)
        self.ti = self.dims.index(schema.time_dim) if schema.time_dim in self.dims else None
        dimlen, shard = schema.dim_lengths, schema.shard_shape
        self.sdims = [d for d in self.dims if d != schema.time_dim]
        self.lo = [region[d][0] if d in region else 0 for d in self.sdims]
        self.hi = [region[d][1] if d in region else dimlen[d] for d in self.sdims]
        self.full = [dimlen[d] for d in self.sdims]
        self.shard = [shard[d] for d in self.sdims]
        #: the region's stop rounded up to a whole shard (never past the
        #: rounded-up array extent)
        self.hi_pad = [_roundup(h, s) for h, s in zip(self.hi, self.shard)]
        self.full_pad = [_roundup(n, s) for n, s in zip(self.full, self.shard)]
        self.shape = tuple(hi - lo for lo, hi in zip(self.lo, self.hi))
        self.pad_shape = tuple(hi - lo for lo, hi in zip(self.lo, self.hi_pad))
        self.partitioned = any(d in region for d in self.sdims)
        #: this writer passes the array (contributors: partitioned ones only)
        self.supplies = role != "contributor" or self.partitioned
        # A replicated array is written by the owner (or Single writer),
        # wherever its region sits. A partitioned array that lacks some
        # region dim is identical across the writers that differ only along
        # that dim; the one at start 0 of every such dim writes it, so each
        # shard object keeps exactly one writer (spec/writer.md §4).
        if not self.partitioned:
            self.writes = role != "contributor"
        else:
            self.writes = all(region[d][0] == 0 for d in region if d not in self.sdims)

    def place(self, buf: np.ndarray, slot: int | None, arr: np.ndarray) -> None:
        idx: list[Any] = [slice(0, n) for n in self.shape]
        if self.ti is not None:
            idx.insert(self.ti, slot)
        buf[tuple(idx)] = arr

    def selection(self, t0: int | None = None, t1: int | None = None):
        sel: list[Any] = [slice(lo, hi) for lo, hi in zip(self.lo, self.hi_pad)]
        if self.ti is not None:
            sel.insert(self.ti, slice(t0, t1))
        return tuple(sel)

    def buffer(self, S: int) -> np.ndarray:
        shape = list(self.pad_shape)
        if self.ti is not None:
            shape.insert(self.ti, S)
        return np.zeros(shape, dtype=self.dtype)

    def objects(self, ts: int | None = None) -> list[str]:
        axes = [range(lo // s, hi // s) for lo, hi, s in zip(self.lo, self.hi_pad, self.shard)]
        if self.ti is not None:
            axes.insert(self.ti, [ts])
        return [f"{self.name}/c/" + "/".join(str(c) for c in cc) for cc in itertools.product(*axes)]


class _Shard:
    """This writer's buffered region of one time shard: every var it writes,
    plus (owner, ``append`` mode) the time coordinate."""

    def __init__(self, ts: int) -> None:
        self.ts = ts
        self.data: dict[str, np.ndarray] = {}
        self.times: np.ndarray | None = None
        self.filled: np.ndarray | None = None
        self.dirty = False


# --- the handle -------------------------------------------------------------- #


class ZarrStreamHandle:
    """One writer's handle on a streaming store (``spec/writer.md``).

    Built by :func:`open_writer` / :func:`resume_writer`; driven through the
    :class:`~earthsciio.backends.zarr_write.ZarrWriter` facade or directly.
    The owner / ``single`` writer holds the authoritative committed state;
    a contributor holds its view (the manifest at join plus what it has
    tokened since)."""

    def __init__(
        self,
        *,
        store,
        base: str,
        schema: OutputSchema,
        role: str,
        n_writers: int,
        wid: int,
        region: Region,
        version: int,
    ) -> None:
        self.store = store
        self.base = base
        self.schema = schema
        self.role = role
        self.n_writers = n_writers
        self.writer_id = wid
        self.region = region
        self.version = version
        self.fingerprint = schema_fingerprint(schema)
        self.S = schema.shard_shape[schema.time_dim]
        self.indexed = schema.indexed
        self.N = schema.time_mode.n_records if self.indexed else None
        self.times = schema.time_mode.times if self.indexed else None

        self.vars = [_Geo(schema, nm, ov, region, role) for nm, ov in schema.vars]
        self.statics = {nm: _Geo(schema, nm, ov, region, role) for nm, ov in schema.statics}
        self._tgeo = _Geo(
            schema, schema.time_dim, OutputVar([schema.time_dim], schema.time_dtype), {}, role
        )
        self._meta: dict[str, Any] = {}

        # committed state (authoritative for owner / single; a view otherwise)
        self.commit_seq = 0
        self.n_committed = 0  # append
        self.committed_t: list[float] = []  # append (owner / single)
        self.last_committed_t: float | None = None
        self.entries: list[dict[str, Any]] = (  # indexed
            [{"t": float(t), "committed": False, "generation": 0} for t in self.times]
            if self.indexed
            else []
        )
        self.static_state = {nm: {"committed": False, "generation": 0} for nm, _ in schema.statics}
        self.blobs: set[tuple[str, int | None, int, str]] = set()
        self.writers: list[dict[str, Any]] = [{"id": 0, "region": _region_wire(region)}]
        self._base_manifest: dict[str, Any] = {}
        self._v1_time_shards: list[dict[str, Any]] = []
        #: the store has a time-coordinate array (a Rust store may not)
        self.time_coord = True
        #: append, no time coordinate: the committed records' t cannot be read
        #: back, so a resumed handle keeps the manifest's time shards for the
        #: first ``_t_base`` records and ``committed_t`` holds the rest
        self._t_base = 0
        self._prior_time_shards: list[dict[str, Any]] = []

        # writing state
        self.next_index = 0
        self.last_t: float | None = None
        self._touched: set[int] = set()  # indexed: records written here
        self._tokened_records: set[int] = set()  # contributor view
        self._tokened_statics: set[str] = set()
        self._pending_records: dict[int, float] = {}
        self._pending_statics: set[str] = set()
        self._pending_blobs: set[tuple[str, int | None, str]] = set()
        self._pending_objects: set[str] = set()
        self._shards: dict[int, _Shard] = {}
        self._closed = False

    # -- introspection ---------------------------------------------------- #

    @property
    def manifest_schema(self) -> str:
        return OUTPUT_MANIFEST_V1 if self.version == 1 else OUTPUT_MANIFEST_V2

    @property
    def is_owner(self) -> bool:
        return self.role in ("single", "owner")

    def __repr__(self) -> str:
        return (
            f"ZarrStreamHandle({self.base!r}, role={self.role!r}, "
            f"id={self.writer_id}, region={_region_wire(self.region)})"
        )

    # -- internals: arrays ------------------------------------------------ #

    def _mem_array(self, name: str, time_len: int | None = None):
        """An UNSAVED array over ``name`` whose shape is the persisted one
        rounded up to whole shards (time: ``time_len``), so every shard this
        writer owns is a complete selection. Writing through it never touches
        ``zarr.json``."""
        import zarr
        from zarr.core.array import AsyncArray
        from zarr.storage import StorePath

        meta = self._meta[name]
        shard = meta.chunk_grid.chunk_shape
        shape = [_roundup(n, s) for n, s in zip(meta.shape, shard)]
        dims = list(meta.dimension_names or [])
        if time_len is not None and self.schema.time_dim in dims:
            shape[dims.index(self.schema.time_dim)] = time_len
        am = AsyncArray(
            metadata=meta.update_shape(tuple(shape)), store_path=StorePath(self.store, name)
        )
        object.__setattr__(am, "codec_pipeline", _whole_shard_pipeline(am.codec_pipeline))
        return zarr.Array(am)

    def _set_time_shape(self, n: int) -> None:
        """Owner: rewrite ``zarr.json`` of the time coordinate and every
        streaming var with ``shape[time] = n`` (metadata only: no shard object
        is touched — zarr's ``resize`` would delete other writers' shards)."""
        from zarr.core.buffer import default_buffer_prototype

        tname = self.schema.time_dim
        for name in [tname] * self.time_coord + [g.name for g in self.vars]:
            meta = self._meta[name]
            dims = list(meta.dimension_names)
            shape = list(meta.shape)
            shape[dims.index(tname)] = n
            meta = meta.update_shape(tuple(shape))
            buf = meta.to_buffer_dict(default_buffer_prototype())["zarr.json"]
            _put_bytes(self.store, f"{name}/zarr.json", buf.to_bytes())
            self._meta[name] = meta

    # -- internals: committed views --------------------------------------- #

    def _have(self, i: int) -> bool:
        """Does slot ``i`` of a reloaded shard hold data to keep?"""
        if not self.indexed:
            return i < self.next_index
        return self.entries[i]["committed"] or i in self._touched

    def _refresh_from_manifest(self) -> None:
        """Contributor: re-read the committed state from the manifest."""
        m = _read_manifest(self.store)
        if m is None:
            return
        _load_state(self, m)
        self._tokened_records.clear()
        self._tokened_statics.clear()

    def _record_committed(self, i: int) -> bool:
        if self.is_owner:  # the owner's state is authoritative
            return self.entries[i]["committed"]
        view = self.entries[i]["committed"] or i in self._tokened_records
        if view:
            self._refresh_from_manifest()
            view = self.entries[i]["committed"]
        return view

    def _static_committed(self, name: str) -> bool:
        if self.is_owner:
            return self.static_state[name]["committed"]
        view = self.static_state[name]["committed"] or name in self._tokened_statics
        if view:
            self._refresh_from_manifest()
            view = self.static_state[name]["committed"]
        return view

    # -- internals: shards ------------------------------------------------ #

    def _inbounds(self, ts: int) -> int:
        return self.S if not self.indexed else min(self.S, self.N - ts * self.S)

    def _owner_time(self) -> bool:
        return self.is_owner and not self.indexed and self.time_coord

    def _load_shard(self, ts: int) -> _Shard:
        """A buffer for time shard ``ts``. When some of its records are
        committed (or were written by this handle) and the buffer is gone, this
        writer's region of the shard is reloaded from the store first, and the
        slots it does not own are reset to fill."""
        S = self.S
        sh = _Shard(ts)
        inb = self._inbounds(ts)
        have = np.array([k < inb and self._have(ts * S + k) for k in range(S)])
        sh.filled = have.copy()
        t0, t1 = ts * S, (ts + 1) * S
        for g in self.vars:
            if not g.writes:
                continue
            buf = g.buffer(S)
            if have.any():
                buf[...] = self._mem_array(g.name, t1)[g.selection(t0, t1)]
                drop = [slice(None)] * buf.ndim
                drop[g.ti] = ~have
                buf[tuple(drop)] = 0
            sh.data[g.name] = buf
        if self._owner_time():
            sh.times = np.zeros(S, dtype=self.schema.time_dtype)
            if have.any():
                sh.times[:] = self._mem_array(self.schema.time_dim, t1)[t0:t1]
                sh.times[~have] = 0
        self._shards[ts] = sh
        return sh

    def _write_shard(self, sh: _Shard) -> None:
        """Encode and PUT this writer's region of a time shard, whole."""
        t0, t1 = sh.ts * self.S, (sh.ts + 1) * self.S
        for g in self.vars:
            if g.writes:
                self._mem_array(g.name, t1)[g.selection(t0, t1)] = sh.data[g.name]
                self._pending_objects.update(g.objects(sh.ts))
        if sh.times is not None:
            self._mem_array(self.schema.time_dim, t1)[t0:t1] = sh.times
            self._pending_objects.update(self._tgeo.objects(sh.ts))
        sh.dirty = False

    def _check_arrays(self, arrays: dict[str, Any], geos: Sequence[_Geo]) -> dict[str, np.ndarray]:
        out = {}
        by_name = {g.name: g for g in geos}
        for nm in arrays:
            g = by_name.get(nm)
            if g is None:
                raise ValueError(f"unknown variable {nm!r}")
            if not g.supplies:
                raise EarthSciIOError(
                    f"variable {nm!r} is replicated (none of its dims is a region "
                    f"dim): only the owner supplies it, not contributor "
                    f"{self.writer_id}"
                )
        for g in geos:
            if not g.supplies:
                continue
            if g.name not in arrays:
                raise ValueError(f"missing array for variable {g.name!r}")
            a = np.asarray(arrays[g.name], dtype=g.dtype)
            if a.shape != g.shape:
                raise ValueError(
                    f"variable {g.name!r}: array shape {a.shape} != expected {g.shape} "
                    f"(dims minus time, region-sized)"
                )
            out[g.name] = a
        return out

    def _check_open(self) -> None:
        if self._closed:
            raise EarthSciIOError("the write handle is closed")

    def _check_region_arg(self, region) -> None:
        if region is not None and _norm_region(region) != self.region:
            raise ValueError(
                f"region {_region_wire(_norm_region(region))} != the handle's bound "
                f"region {_region_wire(self.region)}"
            )

    def _put_record(self, i: int, t: float, arrays: dict[str, Any]) -> None:
        data = self._check_arrays(arrays, self.vars)
        ts, k = divmod(i, self.S)
        sh = self._shards.get(ts) or self._load_shard(ts)
        for g in self.vars:
            if g.writes:
                g.place(sh.data[g.name], k, data[g.name])
        if sh.times is not None:
            sh.times[k] = t
        sh.filled[k] = True
        sh.dirty = True
        self._pending_records[i] = t
        if sh.filled[: self._inbounds(ts)].all():
            # a fully-covered time shard is written as soon as it fills
            self._write_shard(sh)
            del self._shards[ts]

    # -- writes ------------------------------------------------------------ #

    def write_record(self, t: float, arrays: dict[str, Any], region=None) -> int:
        """Append one record (``append`` mode); returns its index. ``t`` must
        be strictly greater than the previous record's."""
        self._check_open()
        self._check_region_arg(region)
        if self.indexed:
            raise EarthSciIOError("write_record is an error in indexed mode: use write_record_at")
        t = float(t)
        if not math.isfinite(t):
            raise ValueError(f"record time {t!r} is not finite")
        if self.last_t is not None and not t > self.last_t:
            raise ValueError(
                f"append-mode times must be strictly increasing: {t!r} <= {self.last_t!r}"
            )
        i = self.next_index
        self._put_record(i, t, arrays)
        self.next_index = i + 1
        self.last_t = t
        if self.version == 1 and len(self._pending_records) >= self.S:
            # v1 behaviour: the manifest is refreshed whenever a shard's worth
            # of records is pending (what the pre-contract writer did).
            self.flush()
        return i

    def write_record_at(self, index: int, t: float, arrays: dict[str, Any], region=None) -> None:
        """Write record ``index`` (``indexed`` mode, any order). ``t`` must equal
        ``times[index]`` bitwise; a committed record must be invalidated first."""
        self._check_open()
        self._check_region_arg(region)
        if not self.indexed:
            raise EarthSciIOError("write_record_at is an error in append mode: use write_record")
        i = int(index)
        if not 0 <= i < self.N:
            raise ValueError(f"record index {i} out of range [0, {self.N})")
        if not _same_t(t, self.times[i]):
            raise ValueError(f"record {i}: t = {t!r} != times[{i}] = {float(self.times[i])!r}")
        if self._record_committed(i):
            raise EarthSciIOError(
                f"record {i} is committed: the owner must invalidate it before it is rewritten"
            )
        self._put_record(i, float(self.times[i]), arrays)
        self._touched.add(i)

    def write_static(self, name: str, array: Any) -> None:
        """Write a time-less variable: region-shaped when partitioned,
        full-shaped when replicated (owner only). Encoded and written now."""
        self._check_open()
        g = self.statics.get(name)
        if g is None:
            raise ValueError(f"unknown static {name!r}")
        a = self._check_arrays({name: array}, [g])[name]
        if self._static_committed(name):
            raise EarthSciIOError(
                f"static {name!r} is committed: the owner must invalidate_statics "
                f"it before it is rewritten"
            )
        if g.writes:
            buf = g.buffer(0)
            g.place(buf, None, a)
            self._mem_array(name)[g.selection()] = buf
            self._pending_objects.update(g.objects())
        self._pending_statics.add(name)

    def write_blob(self, scope: BlobScope, key: str, data: bytes) -> None:
        """Store a small rank-local payload at ``blobs/static/<writer>/<key>``
        (``scope="static"``) or ``blobs/record/<i>/<writer>/<key>``
        (``scope=("record", i)``). Committed like records; rewriting a
        committed blob replaces it."""
        self._check_open()
        if self.version == 1:
            raise EarthSciIOError(
                "blobs need a v2 store (a single-writer append store without "
                "statics is v1): open with role='owner', n_writers=1"
            )
        sc, rec = _parse_scope(scope)
        if not isinstance(key, str) or not _BLOB_KEY.match(key) or key in (".", ".."):
            raise ValueError(f"blob key {key!r} must match [A-Za-z0-9._-]+ and not be '.' or '..'")
        if self.indexed and rec is not None and rec >= self.N:
            raise ValueError(f"blob record {rec} out of range [0, {self.N})")
        path = _blob_key(sc, rec, self.writer_id, key)
        _put_bytes(self.store, path, bytes(data))
        self._pending_blobs.add((sc, rec, key))
        self._pending_objects.add(path)

    # -- flush / commit ----------------------------------------------------- #

    def flush(self) -> CommitToken:
        """PUT every dirty shard of this writer's region (partial time shards
        padded with fill) and return the token for everything written since the
        last one. A ``single`` writer then commits it, if it is not empty."""
        self._check_open()
        for sh in list(self._shards.values()):
            if sh.dirty:
                self._write_shard(sh)
        tok = CommitToken(
            writer=self.writer_id,
            region=dict(self.region),
            fingerprint=self.fingerprint,
            records=sorted(self._pending_records.items()),
            statics=sorted(self._pending_statics),
            blobs=sorted(self._pending_blobs, key=_blob_sort),
            objects=sorted(self._pending_objects),
        )
        if self.role == "single" and not tok.empty:
            # commit before clearing, so a failed commit leaves the writes
            # pending and the flush can be retried
            self.commit([tok])
        self._tokened_records.update(self._pending_records)
        self._tokened_statics.update(self._pending_statics)
        self._pending_records.clear()
        self._pending_statics.clear()
        self._pending_blobs.clear()
        self._pending_objects.clear()
        return tok

    def _verify(self, tokens: list[CommitToken]):
        """``spec/writer.md`` §5.3, before anything is written."""
        ids = sorted(t.writer for t in tokens)
        if ids != list(range(self.n_writers)):
            dup = sorted({i for i in ids if ids.count(i) > 1})
            missing = sorted(set(range(self.n_writers)) - set(ids))
            extra = sorted(set(ids) - set(range(self.n_writers)))
            raise CommitError(
                f"need exactly one token per writer 0..{self.n_writers - 1}: "
                f"missing {missing}, duplicate {dup}, unknown {extra}"
            )
        tokens = sorted(tokens, key=lambda t: t.writer)
        for t in tokens:
            if t.fingerprint != self.fingerprint:
                raise CommitError(
                    f"writer {t.writer}: schema fingerprint {t.fingerprint} != "
                    f"the owner's {self.fingerprint}"
                )
        # regions: same dims, aligned, disjoint, tiling the full extent
        rdims = set(tokens[0].region)
        dimlen = self.schema.dim_lengths
        for t in tokens:
            if set(t.region) != rdims:
                raise CommitError(
                    f"writer {t.writer} names region dims {sorted(t.region)}, writer "
                    f"{tokens[0].writer} names {sorted(rdims)}"
                )
            try:
                _check_region(self.schema, t.region)
            except (RegionNotShardAligned, ValueError) as e:
                raise CommitError(f"writer {t.writer}: {e}") from None
        for a, b in itertools.combinations(tokens, 2):
            if all(
                a.region[d][0] < b.region[d][1] and b.region[d][0] < a.region[d][1] for d in rdims
            ):
                raise CommitError(
                    f"overlap between writers {a.writer} {_region_wire(a.region)} and "
                    f"{b.writer} {_region_wire(b.region)}"
                )
        full = math.prod(dimlen[d] for d in rdims)
        got = sum(math.prod(t.region[d][1] - t.region[d][0] for d in rdims) for t in tokens)
        if got != full:
            raise CommitError(
                f"gap: the writers' regions cover {got} of {full} cells over region "
                f"dims {sorted(rdims)}"
            )
        # records: same indices, bitwise-equal t
        ref = tokens[0].records
        for t in tokens[1:]:
            if [i for i, _ in t.records] != [i for i, _ in ref] or not all(
                _same_t(x, y) for (_, x), (_, y) in zip(t.records, ref)
            ):
                raise CommitError(
                    f"writer {t.writer} carries records {t.records}, writer "
                    f"{tokens[0].writer} carries {ref}"
                )
        idx = [i for i, _ in ref]
        if not self.indexed:
            want = list(range(self.n_committed, self.n_committed + len(idx)))
            if idx != want:
                raise CommitError(
                    f"append mode: records {idx} are not the continuation {want} of "
                    f"the {self.n_committed} committed records"
                )
        else:
            for i, tv in ref:
                if not 0 <= i < self.N:
                    raise CommitError(f"record index {i} out of range [0, {self.N})")
                if not _same_t(tv, self.times[i]):
                    raise CommitError(f"record {i}: t = {tv!r} != times[{i}]")
                if self.entries[i]["committed"]:
                    raise CommitError(f"record {i} is already committed (invalidate it first)")
        # statics
        names = set().union(*(t.statics for t in tokens))
        for nm in sorted(names):
            if nm not in self.static_state:
                raise CommitError(f"unknown static {nm!r}")
            if self.static_state[nm]["committed"]:
                raise CommitError(f"static {nm!r} is already committed")
            dims = dict(self.schema.statics)[nm].dims
            have = [t.writer for t in tokens if nm in t.statics]
            if any(d in rdims for d in dims):
                if len(have) != len(tokens):
                    raise CommitError(
                        f"partitioned static {nm!r} is in the tokens of writers {have} "
                        f"only: every writer must write its region"
                    )
            elif have != [0]:
                raise CommitError(
                    f"replicated static {nm!r} must appear only in the owner's token "
                    f"(found in writers {have})"
                )
        return tokens, ref, names

    _COMMITTED_STATE = (
        "commit_seq",
        "n_committed",
        "committed_t",
        "last_committed_t",
        "entries",
        "static_state",
        "blobs",
        "writers",
        "_v1_time_shards",
    )

    @contextlib.contextmanager
    def _transaction(self):
        """Restore the committed state if the body fails (a store error on the
        shapes or the manifest), so the handle never claims a commit the
        manifest does not record and the call can be retried."""
        saved = {a: copy.deepcopy(getattr(self, a)) for a in self._COMMITTED_STATE}
        try:
            yield
        except BaseException:
            for a, v in saved.items():
                setattr(self, a, v)
            raise

    def commit(self, tokens: Sequence[CommitToken | str | dict[str, Any]]) -> dict[str, Any]:
        """Owner: verify ``tokens`` (one per writer, the owner's included) and
        commit: grow ``shape[time]`` (append mode), update the record / static /
        blob state, bump ``commit_seq`` and write the manifest LAST. Raises
        :class:`CommitError` — writing nothing — on any failed check. Returns
        the manifest."""
        self._check_open()
        if not self.is_owner:
            raise EarthSciIOError("only the owner (or a single writer) commits")
        toks = [t if isinstance(t, CommitToken) else CommitToken.from_json(t) for t in tokens]
        toks, recs, statics = self._verify(toks)
        with self._transaction():
            return self._apply_commit(toks, recs, statics)

    def _apply_commit(self, toks, recs, statics) -> dict[str, Any]:
        if recs and not self.indexed:
            new = self.n_committed + len(recs)
            self._set_time_shape(new)
            self.n_committed = new
            self.committed_t.extend(t for _, t in recs)
            self.last_committed_t = recs[-1][1]
            if self.version == 1:
                self._v1_time_shards.append(
                    {
                        "index": len(self._v1_time_shards),
                        "t_start": recs[0][1],
                        "t_end": recs[-1][1],
                        "n_records": len(recs),
                    }
                )
        for i, _ in recs if self.indexed else ():
            self.entries[i]["committed"] = True
            self.entries[i]["generation"] += 1
        for nm in statics:
            self.static_state[nm]["committed"] = True
            self.static_state[nm]["generation"] += 1
        for t in toks:
            for sc, rec, key in t.blobs:
                self.blobs.add((sc, rec, t.writer, key))
        self.writers = [{"id": t.writer, "region": _region_wire(t.region)} for t in toks]
        self.commit_seq += 1
        return self._write_manifest()

    def invalidate(self, indices: Iterable[int]) -> dict[str, Any]:
        """Owner / single, ``indexed`` mode: mark records uncommitted (and
        their ``record(i)`` blobs) so they may be rewritten; writes the manifest
        and bumps ``commit_seq``."""
        self._check_open()
        if not self.is_owner:
            raise EarthSciIOError("only the owner (or a single writer) invalidates")
        if not self.indexed:
            raise EarthSciIOError(
                "invalidate needs indexed mode (append mode truncates with resume(at=...))"
            )
        idx = sorted({int(i) for i in indices})
        for i in idx:
            if not 0 <= i < self.N:
                raise ValueError(f"record index {i} out of range [0, {self.N})")
        with self._transaction():
            for i in idx:
                self.entries[i]["committed"] = False
            self.blobs = {b for b in self.blobs if not (b[0] == "record" and b[1] in idx)}
            self.commit_seq += 1
            return self._write_manifest()

    def invalidate_statics(self, names: Iterable[str]) -> dict[str, Any]:
        """Owner / single: mark statics uncommitted so they may be rewritten."""
        self._check_open()
        if not self.is_owner:
            raise EarthSciIOError("only the owner (or a single writer) invalidates")
        names = list(names)
        for nm in names:
            if nm not in self.static_state:
                raise ValueError(f"unknown static {nm!r}")
        with self._transaction():
            for nm in names:
                self.static_state[nm]["committed"] = False
            self.commit_seq += 1
            return self._write_manifest()

    def close(self, tokens: Sequence[Any] = ()) -> dict[str, Any] | None:
        """Single: flush, then rewrite the manifest (same ``commit_seq``).
        Owner: flush, then commit its own token plus the OTHER writers'
        ``tokens``. Both return the manifest. Contributor: errors if anything
        written is unflushed (deliver the token first); returns ``None``."""
        self._check_open()
        if self.role == "contributor":
            if tokens:
                raise ValueError("a contributor's close takes no tokens")
            if (
                self._pending_records
                or self._pending_statics
                or self._pending_blobs
                or any(sh.dirty for sh in self._shards.values())
            ):
                raise EarthSciIOError(
                    "contributor close with unflushed writes: flush and deliver the token first"
                )
            self._closed = True
            return None
        if self.role == "single":
            if tokens:
                raise ValueError("a single writer's close takes no tokens")
            self.flush()
            m = self._write_manifest()
        else:
            own = self.flush()
            m = self.commit([own, *tokens])
        self._closed = True
        return m

    # -- the manifest ------------------------------------------------------ #

    def _time_shards_v2(self) -> list[dict[str, Any]]:
        if self.indexed:
            pairs = [(i, e["t"]) for i, e in enumerate(self.entries) if e["committed"]]
        else:
            pairs = list(enumerate(self.committed_t, start=self._t_base))
        out = [dict(e) for e in self._prior_time_shards]
        for ts, grp in itertools.groupby(pairs, key=lambda p: p[0] // self.S):
            tv = [t for _, t in grp]
            if out and out[-1]["index"] == ts:  # extends a kept, partial shard
                e = out[-1]
                e.update(
                    t_start=min(e["t_start"], *tv), t_end=max(e["t_end"], *tv), n=e["n"] + len(tv)
                )
            else:
                out.append({"index": ts, "t_start": min(tv), "t_end": max(tv), "n": len(tv)})
        return out

    def _manifest_dict(self) -> dict[str, Any]:
        s = self.schema
        if self.version == 1:
            m = dict(self._base_manifest)
            m.update(
                {
                    "time_shards": [dict(e) for e in self._v1_time_shards],
                    "last_t": self.last_committed_t,
                    "total_records": self.n_committed,
                    "written_at": _now(),
                }
            )
            return m
        if self.indexed:
            committed = [i for i, e in enumerate(self.entries) if e["committed"]]
            shape_t = self.N
            last_t = max((self.entries[i]["t"] for i in committed), default=None)
            records: dict[str, Any] = {
                "committed": _ranges(committed),
                "entries": [
                    {
                        "index": i,
                        "t": e["t"],
                        "committed": e["committed"],
                        "generation": e["generation"],
                    }
                    for i, e in enumerate(self.entries)
                ],
            }
            time_mode = {"kind": "indexed", "n_records": self.N}
        else:
            shape_t = self.n_committed
            last_t = self.last_committed_t
            records = {"committed": _ranges(range(self.n_committed))}
            time_mode = {"kind": "append"}

        def _vmeta(nm, ov, extra=None):
            d = {"name": nm, "dims": list(ov.dims), "dtype": _v3_dtype(ov.dtype)}
            d.update(extra or {})
            if ov.attrs:
                d["attrs"] = dict(ov.attrs)
            return d

        m = dict(self._base_manifest)
        m.update(
            {
                "schema": OUTPUT_MANIFEST_V2,
                "dims": [
                    {"name": d, "length": shape_t if d == s.time_dim else n} for d, n in s.dims
                ],
                "vars": [_vmeta(nm, ov) for nm, ov in s.vars],
                "time_shards": self._time_shards_v2(),
                "last_t": last_t,
                "n_records": shape_t,
                "schema_fingerprint": self.fingerprint,
                "time_mode": time_mode,
                "writers": [dict(w) for w in self.writers],
                "records": records,
                "statics": [
                    _vmeta(
                        nm,
                        ov,
                        {
                            "committed": self.static_state[nm]["committed"],
                            "generation": self.static_state[nm]["generation"],
                        },
                    )
                    for nm, ov in s.statics
                ],
                "blobs": [
                    {"scope": sc, "record": r, "writer": w, "key": k}
                    for sc, r, w, k in sorted(self.blobs, key=_blob_sort)
                ],
                "commit_seq": self.commit_seq,
            }
        )
        return m

    def _write_manifest(self) -> dict[str, Any]:
        """Write ``output_manifest.json`` — the commit point — through the same
        store the arrays use. Never called by a contributor."""
        assert self.is_owner
        m = self._manifest_dict()
        _put_bytes(
            self.store,
            MANIFEST_KEY,
            (json.dumps(m, indent=2, sort_keys=True) + "\n").encode("utf-8"),
        )
        return m


# --- committed state from a manifest ----------------------------------------- #


def _load_state(h: ZarrStreamHandle, m: dict[str, Any]) -> None:
    """Load the committed state of manifest ``m`` into handle ``h``."""
    if m["schema"] == OUTPUT_MANIFEST_V1:
        h.n_committed = int(m.get("total_records", m.get("n_records", 0)))
        h.last_committed_t = m.get("last_t")
        h._v1_time_shards = [dict(e) for e in m.get("time_shards", [])]
        return
    h.commit_seq = int(m["commit_seq"])
    h.writers = [dict(w) for w in m.get("writers", [])]
    if h.indexed:
        for e in m["records"]["entries"]:
            h.entries[e["index"]].update(
                committed=bool(e["committed"]), generation=int(e["generation"])
            )
    else:
        h.n_committed = int(m["n_records"])
        h.last_committed_t = m.get("last_t")
    for st in m.get("statics", []):
        if st["name"] in h.static_state:
            h.static_state[st["name"]] = {
                "committed": bool(st["committed"]),
                "generation": int(st["generation"]),
            }
    h.blobs = {(b["scope"], b["record"], int(b["writer"]), b["key"]) for b in m.get("blobs", [])}


def _base_manifest(schema: OutputSchema, base: str, version: int) -> dict[str, Any]:
    """The fields of a manifest fixed at create."""
    common = {
        "base_url": base,
        "format": "zarr",
        "zarr_format": 3,
        "profile": schema.profile,
        "codec": _codec_dict(schema.profile),
        "time_dim": schema.time_dim,
    }
    if version == 1:
        # exactly the pre-contract Python v1 manifest
        common.update(
            {
                "schema": OUTPUT_MANIFEST_V1,
                "dims": [[k, v] for k, v in schema.dims],
                "vars": [
                    {"name": nm, "dims": list(ov.dims), "dtype": _v3_dtype(ov.dtype)}
                    for nm, ov in schema.vars
                ],
                "chunk_shape": dict(schema.chunk_shape),
                "shard_shape": dict(schema.shard_shape),
            }
        )
        return common
    dims = [d for d, _ in schema.dims]
    common.update(
        {
            "chunk_shape": {d: schema.chunk_shape[d] for d in dims},
            "shard_shape": {d: schema.shard_shape[d] for d in dims},
            "created_at": _now(),
        }
    )
    return common


def _norm_role(role: Any, n_writers: int, id: int) -> tuple[str, int, int]:
    role = str(role).lower()
    if role == "single":
        return "single", 1, 0
    if role == "owner":
        if int(n_writers) < 1:
            raise ValueError(f"owner n_writers must be >= 1, got {n_writers}")
        return "owner", int(n_writers), 0
    if role == "contributor":
        wid = int(id)
        if wid < 1 or (int(n_writers) > 1 and wid >= int(n_writers)):
            raise ValueError(
                f"contributor id must satisfy 1 <= id < n_writers, got id={wid}, "
                f"n_writers={n_writers}"
            )
        return "contributor", int(n_writers), wid
    raise ValueError(f"role must be 'single', 'owner' or 'contributor', got {role!r}")


def _check_single_region(schema: OutputSchema, role: str, region: Region) -> None:
    if role == "single":
        dimlen = schema.dim_lengths
        for d, (a, b) in region.items():
            if (a, b) != (0, dimlen[d]):
                raise ValueError(
                    "a single writer writes the whole extent: use role='owner' "
                    "with n_writers for a partial region"
                )


# --- open / join / resume ---------------------------------------------------- #


def open_writer(
    base_url: str,
    schema: OutputSchema,
    role: str = "single",
    n_writers: int = 1,
    id: int = 0,
    region: Any | None = None,
) -> ZarrStreamHandle:
    """Create (``single`` / ``owner``) or join (``contributor``) a store.
    See :meth:`earthsciio.backends.zarr_write.ZarrWriter.write_open`."""
    role, n_writers, wid = _norm_role(role, n_writers, id)
    _validate_schema(schema)
    region = _norm_region(region)
    _check_region(schema, region)  # before any I/O
    _check_single_region(schema, role, region)
    if role == "contributor":
        return _join(base_url, schema, n_writers, wid, region)

    import zarr

    store, base = _open_store(base_url, create=True)
    version = 1 if (role == "single" and not schema.indexed and not schema.statics) else 2
    h = ZarrStreamHandle(
        store=store,
        base=base,
        schema=schema,
        role=role,
        n_writers=n_writers,
        wid=wid,
        region=region,
        version=version,
    )

    group = zarr.open_group(store=store, mode="w", zarr_format=3)
    if schema.attrs:
        group.attrs.update(schema.attrs)
    compressor = _compressor(schema.profile)

    def _mk(name, dims, dtype, shape, attrs, values=None):
        a = group.create_array(
            name=name,
            shape=tuple(shape),
            chunks=tuple(schema.chunk_shape[d] for d in dims),
            shards=tuple(schema.shard_shape[d] for d in dims),
            dtype=np.dtype(dtype),
            compressors=[compressor],
            dimension_names=list(dims),
            fill_value=_v3_fill(dtype),
            attributes=dict(attrs) if attrs else None,
            overwrite=True,
        )
        if values is not None and len(values):
            a[...] = values
        h._meta[name] = a.metadata
        return a

    dimlen = schema.dim_lengths
    time_attrs: dict[str, Any] = {}
    for nm, (vals, attrs) in schema.coords:
        if nm == schema.time_dim:
            time_attrs = attrs
            continue
        varr = np.asarray(vals)
        _mk(nm, [nm], varr.dtype, varr.shape, attrs, varr)
    if schema.indexed:
        _mk(
            schema.time_dim,
            [schema.time_dim],
            np.float64,
            (h.N,),
            time_attrs,
            np.asarray(h.times, dtype=np.float64),
        )
    else:
        _mk(schema.time_dim, [schema.time_dim], schema.time_dtype, (0,), time_attrs)
    tlen = h.N if schema.indexed else 0
    for nm, ov in schema.vars:
        _mk(
            nm,
            ov.dims,
            ov.dtype,
            [tlen if d == schema.time_dim else dimlen[d] for d in ov.dims],
            ov.attrs,
        )
    for nm, ov in schema.statics:
        _mk(nm, ov.dims, ov.dtype, [dimlen[d] for d in ov.dims], ov.attrs)

    h._base_manifest = _base_manifest(schema, base, version)
    if version == 2:
        h._write_manifest()  # commit_seq 0
    return h


def _open_existing(
    base_url: str,
    role: str,
    n_writers: int,
    wid: int,
    region: Region,
    schema: OutputSchema | None,
) -> ZarrStreamHandle:
    store, base = _open_store(base_url, create=False)
    m = _read_manifest(store)
    if m is None:
        raise EarthSciIOError(f"no {MANIFEST_KEY} at {base_url}")
    if m.get("schema") not in (OUTPUT_MANIFEST_V1, OUTPUT_MANIFEST_V2):
        raise EarthSciIOError(f"unknown output manifest schema {m.get('schema')!r}")
    v2 = m["schema"] == OUTPUT_MANIFEST_V2
    if role == "contributor" and not v2:
        raise EarthSciIOError("a contributor needs a v2 store (the manifest is v1)")
    if not v2 and role != "single":
        raise EarthSciIOError("a v1 store is reopened only by a single writer")
    rebuilt = _schema_from_manifest(m)
    fp = schema_fingerprint(rebuilt)
    if v2 and m.get("schema_fingerprint") != fp:
        raise EarthSciIOError(
            f"manifest schema_fingerprint {m.get('schema_fingerprint')} != {fp} "
            f"recomputed from the manifest"
        )
    if schema is not None and schema_fingerprint(schema) != fp:
        raise EarthSciIOError(
            f"the caller's schema fingerprint {schema_fingerprint(schema)} != the store's {fp}"
        )
    _check_region(rebuilt, region)
    _check_single_region(rebuilt, role, region)
    h = ZarrStreamHandle(
        store=store,
        base=m.get("base_url", base),
        schema=rebuilt,
        role=role,
        n_writers=n_writers,
        wid=wid,
        region=region,
        version=2 if v2 else 1,
    )
    h._base_manifest = {k: v for k, v in m.items()}
    h.time_coord = _get_bytes(store, f"{rebuilt.time_dim}/zarr.json") is not None
    names = [g.name for g in h.vars if g.writes] + [g.name for g in h.statics.values() if g.writes]
    if h.is_owner:
        names = [rebuilt.time_dim] * h.time_coord + [g.name for g in h.vars] + list(h.statics)
    for nm in names:
        h._meta[nm] = _load_meta(store, nm)
    _load_state(h, m)
    return h


def _join(
    base_url: str, schema: OutputSchema | None, n_writers: int, wid: int, region: Region
) -> ZarrStreamHandle:
    h = _open_existing(base_url, "contributor", n_writers, wid, region, schema)
    h.next_index = h.n_committed
    h.last_t = h.last_committed_t
    return h


def resume_writer(
    base_url: str,
    role: str = "single",
    n_writers: int = 1,
    id: int = 0,
    region: Any | None = None,
    at: float | None = None,
) -> ZarrStreamHandle:
    """Reopen a store after preemption (``spec/writer.md`` §7). ``append``:
    the committed records are truncated to those with ``t <= at`` (when that
    drops any, the owner / single writer rewrites the shapes and the manifest);
    every writer computes the same count and continues from it. ``indexed``:
    nothing is truncated and ``at`` must be ``None``."""
    role, n_writers, wid = _norm_role(role, n_writers, id)
    region = _norm_region(region)
    h = _open_existing(base_url, role, n_writers, wid, region, None)
    if h.indexed:
        if at is not None:
            raise ValueError("resume of an indexed store takes at=None")
        return h
    import zarr

    n = h.n_committed
    times: list[float] = []
    if not h.time_coord:
        if at is not None:
            raise EarthSciIOError(
                f"resume at t = {at} needs the time coordinate {h.schema.time_dim!r}, "
                f"which the store does not have"
            )
        h._t_base = n
        h._prior_time_shards = [dict(e) for e in h._base_manifest.get("time_shards", [])]
        h.last_t = h.last_committed_t
        h.next_index = n
        return h
    if n:
        tarr = zarr.open_array(store=h.store, path=h.schema.time_dim, mode="r")
        times = [float(x) for x in np.asarray(tarr[:n], dtype=np.float64)]
    keep = n if at is None else bisect.bisect_right(times, float(at))
    h.committed_t = times[:keep]
    h.n_committed = keep
    h.last_committed_t = times[keep - 1] if keep else None
    if keep < n and h.is_owner:
        h._set_time_shape(keep)
        h.blobs = {b for b in h.blobs if not (b[0] == "record" and b[1] >= keep)}
        if h.version == 1:
            out, left = [], keep
            for e in h._v1_time_shards:
                ck = "n_records" if "n_records" in e else "n"
                if left <= 0:
                    break
                e = dict(e)
                if e[ck] > left:
                    e[ck] = left
                    e["t_end"] = times[keep - 1]
                left -= e[ck]
                out.append(e)
            h._v1_time_shards = out
        else:
            h.commit_seq += 1
        h._write_manifest()
    h.next_index = keep
    h.last_t = h.last_committed_t
    return h


# --- read-back (spec/writer.md §7) ------------------------------------------- #


def read_output_manifest(base_url: str) -> dict[str, Any]:
    """The store's output manifest (v1 or v2), parsed."""
    m = _read_manifest(_open_read_store(base_url))
    if m is None:
        raise EarthSciIOError(f"no {MANIFEST_KEY} at {base_url}")
    return m


def committed_records(m: dict[str, Any]) -> list[int]:
    """The committed record indices of manifest ``m``, ascending."""
    if m["schema"] == OUTPUT_MANIFEST_V2:
        return [i for a, b in m["records"]["committed"] for i in range(a, b)]
    return list(range(int(m.get("n_records", m.get("total_records", 0)))))


def _record_committed(m: dict[str, Any], i: int) -> bool:
    """Is record ``i`` committed in manifest ``m``? A range check, without
    expanding the committed ranges."""
    if m["schema"] == OUTPUT_MANIFEST_V2:
        return any(a <= i < b for a, b in m["records"]["committed"])
    return 0 <= i < int(m.get("n_records", m.get("total_records", 0)))


def _read_region(
    base_url: str, name: str, dims: list[str], time_dim: str | None, i: int | None, region
) -> np.ndarray:
    import zarr

    arr = zarr.open_array(store=_open_read_store(base_url), path=name, mode="r")
    reg = _norm_region(region)
    sel: list[Any] = []
    for d, n in zip(dims, arr.shape):
        if d == time_dim:
            sel.append(i)
        elif d in reg:
            a, b = reg[d]
            if not 0 <= a < b <= n:
                raise ValueError(f"region {d}=[{a}, {b}) out of range for length {n}")
            sel.append(slice(a, b))
        else:
            sel.append(slice(0, n))
    return np.asarray(arr[tuple(sel)])


def read_record(base_url: str, var: str, i: int, region=None) -> np.ndarray:
    """The ``region`` (``{dim: (start, stop)}``; ``None`` = everything) of
    committed record ``i`` of streaming variable ``var``: shaped like the
    var's dims minus time, region-sized. Errors if ``i`` is not committed."""
    m = read_output_manifest(base_url)
    v = next((v for v in m["vars"] if v["name"] == var), None)
    if v is None:
        raise ValueError(f"unknown streaming variable {var!r}")
    if not _record_committed(m, int(i)):
        raise EarthSciIOError(f"record {i} is not committed")
    return _read_region(base_url, var, v["dims"], m["time_dim"], int(i), region)


def read_static(base_url: str, name: str, region=None) -> np.ndarray:
    """The ``region`` of committed static ``name``. Errors if not committed."""
    m = read_output_manifest(base_url)
    st = next((s for s in m.get("statics", []) if s["name"] == name), None)
    if st is None:
        raise ValueError(f"unknown static {name!r}")
    if not st["committed"]:
        raise EarthSciIOError(f"static {name!r} is not committed")
    return _read_region(base_url, name, st["dims"], None, None, region)


def read_blob(base_url: str, scope: BlobScope, writer: int, key: str) -> bytes:
    """The bytes of a committed blob. ``scope`` is ``"static"`` or
    ``("record", i)``. Errors if not committed."""
    sc, rec = _parse_scope(scope)
    m = read_output_manifest(base_url)
    committed = {(b["scope"], b["record"], int(b["writer"]), b["key"]) for b in m.get("blobs", [])}
    if (sc, rec, int(writer), key) not in committed:
        raise EarthSciIOError(f"blob {(sc, rec, writer, key)} is not committed")
    data = _get_bytes(_open_read_store(base_url), _blob_key(sc, rec, int(writer), key))
    if data is None:
        raise EarthSciIOError(f"committed blob {(sc, rec, writer, key)} is missing")
    return data
