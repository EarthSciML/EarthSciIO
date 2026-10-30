"""The ``zarr`` WRITER — a streaming, sharded **Zarr v3** output backend built on
zarr-python 3.x (the Python mirror of Julia's ``julia/src/zarr_write.jl``).

streaming-output-sinks RFC, Wave 4. Where the Julia writer hand-rolls the Zarr v3
shard container byte-for-byte, this one delegates the whole encode — sharding
codec, Blosc(zstd)+shuffle inner pipeline, ``dimension_names``, growable ``time``
axis via array resize — to the mature ``zarr`` library. Conformance across
languages is **tolerance-based on decoded arrays** (RFC §16.6), never byte
identity, so using zarr-python's own container layout is explicitly allowed; the
codec *parameters* (zstd / level / shuffle / inner-chunk vs shard shape) still
match the Julia profiles so decoded output is reproducible.

Shape of the emitted store (RFC §16.1, normative):

* ``zarr_format: 3`` — one ``zarr.json`` per group and per array.
* The SHARDING codec: the array's chunk grid is the SHARD (outer, write) shape;
  the sharding codec's inner ``chunk_shape`` is the read chunk. One shard object
  packs many inner chunks (few large objects on S3/Lustre, small chunks for
  readers). Inner pipeline = bytes(little) + Blosc(zstd, shuffle).
* The ``time`` axis grows: each :meth:`ZarrWriter.write_record` buffers one
  time slice into its time shard; a full time shard is encoded and written
  whole, a partial one is written padded at :meth:`write_flush` and rewritten
  whole when it fills. ``shape[time]`` in ``zarr.json`` grows only at a commit.

Commit / durability. Local output goes through zarr-python's ``LocalStore``
(atomic per-object writes); ``s3://`` output through ``FsspecStore`` (s3fs). A
per-store **output manifest** (``output_manifest.json``) records the committed
state; writing it is the commit point. A single-writer ``append`` store without
statics writes ``earthsciio/output-manifest/v1`` (unchanged from before the
writer contract); every other store writes ``earthsciio/output-manifest/v2``.

The streaming writer contract (``spec/writer.md``: regions, roles + commit
tokens, indexed records, statics, blobs, read-back, resume) is implemented in
:mod:`earthsciio.backends.zarr_stream`; :class:`ZarrWriter` is its facade.

Public surface (conceptual mirror of the Julia writer):

    w = ZarrWriter()
    h = w.write_open(base_url, schema)      # declare dims/coords/vars/chunk grid ONCE
    w.write_record(h, t, {var: slab, ...})  # persist one time record
    w.write_flush(h)                        # durable barrier (checkpoint boundary)
    w.write_close(h)                        # finalize + end-of-run manifest

``zarr``/``fsspec``/``s3fs`` are imported **lazily**, shipped as optional extras,
so a base install stays lean (matching the reader + the old ``numcodecs`` culture).
"""

from __future__ import annotations

import os
from dataclasses import dataclass, field
from typing import Any, Dict, List, Optional, Tuple

import numpy as np

__all__ = [
    "BloscProfile",
    "ZstdProfile",
    "BLOSC_DIAGNOSTIC",
    "BLOSC_CHECKPOINT",
    "ZSTD_WASM",
    "OutputVar",
    "OutputSchema",
    "IndexedTime",
    "ZarrWriter",
    "ZarrWriteHandle",
    "OUTPUT_MANIFEST_SCHEMA",
]

OUTPUT_MANIFEST_SCHEMA = "earthsciio/output-manifest/v1"


# --- pinned codec profiles (RFC §16.1, matching julia/src/zarr_write.jl) ----- #


@dataclass(frozen=True)
class BloscProfile:
    """A pinned Blosc codec profile: ``cname`` / ``clevel`` / ``shuffle``."""

    cname: str
    clevel: int
    shuffle: bool


#: Diagnostic profile — Blosc **zstd** + byte-shuffle, moderate level (5).
BLOSC_DIAGNOSTIC = BloscProfile("zstd", 5, True)
#: Checkpoint profile — **lossless** Blosc zstd (level 7) + byte-shuffle.
BLOSC_CHECKPOINT = BloscProfile("zstd", 7, True)


@dataclass(frozen=True)
class ZstdProfile:
    """A pinned **standard Zarr v3 zstd** codec profile: ``level`` / ``checksum``.

    Unlike :class:`BloscProfile` this wraps the plain v3 ``zstd`` codec (NOT the
    Blosc container). It exists for the ``wasm`` profile: a WebAssembly/browser
    Zarr reader (e.g. the pure-Rust ``zarrs`` codecs) can decode the standard
    ``zstd`` codec but NOT Blosc (``blosc-src``'s C sources do not target
    ``wasm32-unknown-unknown``). Lossless; no standalone byte-shuffle filter."""

    level: int
    checksum: bool


#: WASM profile — plain v3 **zstd** (level 5), no Blosc, so a wasm/browser Zarr
#: reader can decode the store. See :class:`ZstdProfile`.
ZSTD_WASM = ZstdProfile(5, False)


def _profile(name: str):
    if name == "diagnostic":
        return BLOSC_DIAGNOSTIC
    if name == "checkpoint":
        return BLOSC_CHECKPOINT
    if name == "wasm":
        return ZSTD_WASM
    raise ValueError(
        f"unknown codec profile {name!r} (expected 'diagnostic', 'checkpoint', or 'wasm')"
    )


# --- the output schema ------------------------------------------------------- #


@dataclass
class OutputVar:
    """One streaming output variable: its on-disk dim names (file order, MUST
    include the schema ``time_dim``), its numpy dtype, and optional CF variable
    attributes ``attrs`` (e.g. ``units``, ``standard_name``, and the CF
    ``coordinates`` attribute naming this variable's auxiliary coordinates). The
    attrs are written verbatim into the array node's ``attributes`` alongside the
    mechanical ``dimension_names``."""

    dims: List[str]
    dtype: Any
    attrs: Dict[str, Any] = field(default_factory=dict)

    def __post_init__(self) -> None:
        self.dims = [str(d) for d in self.dims]
        self.dtype = np.dtype(self.dtype)
        self.attrs = dict(self.attrs)


@dataclass
class IndexedTime:
    """The ``indexed`` time mode (``spec/writer.md`` §6.2): the record axis has
    a fixed length ``len(times)`` from open, and record ``i`` holds time
    ``times[i]`` (float64). Records are written by index in any order."""

    times: Any

    def __post_init__(self) -> None:
        self.times = np.asarray(self.times, dtype=np.float64).reshape(-1)

    @property
    def n_records(self) -> int:
        return int(self.times.shape[0])


@dataclass
class OutputSchema:
    """The input to :meth:`ZarrWriter.write_open`. Mirrors the Julia
    ``OutputSchema`` (a plain struct; no EarthSciAST dependency here).

    Fields:

    * ``dims`` — ORDERED ``[(name, length), ...]``. The ``time_dim`` entry's
      length is a placeholder (0 conventional); the time axis grows.
    * ``time_dim`` — the growable axis name.
    * ``vars`` — ORDERED ``[(name, OutputVar), ...]`` streaming variables.
    * ``chunk_shape`` — ``{dim: inner-chunk length}``.
    * ``shard_shape`` — ``{dim: shard length}`` (a multiple of the inner chunk
      length along every dim). ``shard_shape[time_dim]`` = records per flushed
      shard object.
    * ``coords`` — ORDERED ``[(name, (values, attrs)), ...]`` static coordinate
      arrays, 1-D over their own dim, written once at ``write_open``. An entry
      for ``time_dim`` supplies the time coordinate's attrs (its VALUES are
      ignored — they come from the ``t`` of each record).
    * ``profile`` — ``"diagnostic"``, ``"checkpoint"`` (Blosc-zstd inner codec),
      or ``"wasm"`` (plain v3 zstd inner codec, no Blosc — wasm/browser-loadable).
    * ``attrs`` — group-level attributes.
    * ``time_dtype`` — element type of the time coordinate (default float64).
    * ``statics`` — ORDERED ``[(name, OutputVar), ...]`` time-less variables
      (``spec/writer.md`` §7); their dims MUST NOT include ``time_dim``. A
      schema with statics writes a v2 store.
    * ``time_mode`` — ``"append"`` (the default: records are appended, the time
      axis grows) or :class:`IndexedTime` (``n_records`` fixed at open, records
      written by index in any order; ``spec/writer.md`` §6.2). The time dim's
      length in ``dims`` may be 0 (filled in) or MUST equal ``n_records``.
    """

    dims: List[Tuple[str, int]]
    time_dim: str
    vars: List[Tuple[str, OutputVar]]
    chunk_shape: Dict[str, int]
    shard_shape: Dict[str, int]
    coords: List[Tuple[str, Tuple[Any, Dict[str, Any]]]] = field(default_factory=list)
    profile: str = "diagnostic"
    attrs: Dict[str, Any] = field(default_factory=dict)
    time_dtype: Any = np.float64
    statics: List[Tuple[str, OutputVar]] = field(default_factory=list)
    time_mode: Any = "append"

    def __post_init__(self) -> None:
        self.dims = [(str(k), int(v)) for k, v in self.dims]
        self.time_dim = str(self.time_dim)
        self.vars = [(str(k), v) for k, v in self.vars]
        self.chunk_shape = {str(k): int(v) for k, v in self.chunk_shape.items()}
        self.shard_shape = {str(k): int(v) for k, v in self.shard_shape.items()}
        self.coords = [(str(k), (np.asarray(vals), dict(a))) for k, (vals, a) in self.coords]
        self.attrs = dict(self.attrs)
        self.time_dtype = np.dtype(self.time_dtype)
        self.statics = [(str(k), v) for k, v in self.statics]
        if isinstance(self.time_mode, str):
            if self.time_mode != "append":
                raise ValueError(
                    f"time_mode must be 'append' or IndexedTime(times), got {self.time_mode!r}"
                )
        elif isinstance(self.time_mode, IndexedTime):
            n = self.time_mode.n_records
            self.dims = [(k, n if k == self.time_dim and v == 0 else v) for k, v in self.dims]
            if self.dim_lengths.get(self.time_dim, n) != n:
                raise ValueError(
                    f"indexed time mode: dims[{self.time_dim!r}] = "
                    f"{self.dim_lengths[self.time_dim]} != n_records = {n}"
                )
        else:
            raise TypeError(f"time_mode must be 'append' or IndexedTime, got {self.time_mode!r}")

    @property
    def indexed(self) -> bool:
        return isinstance(self.time_mode, IndexedTime)

    @property
    def dim_lengths(self) -> Dict[str, int]:
        return {k: v for k, v in self.dims}


# --- dtype <-> Zarr v3 data_type (for the manifest fingerprint) -------------- #

_V3_DTYPE = {
    np.dtype("float64"): "float64",
    np.dtype("float32"): "float32",
    np.dtype("int32"): "int32",
    np.dtype("int64"): "int64",
    np.dtype("uint8"): "uint8",
    np.dtype("uint32"): "uint32",
    np.dtype("uint64"): "uint64",
}


def _v3_dtype(dt: np.dtype) -> str:
    dt = np.dtype(dt)
    if dt not in _V3_DTYPE:
        raise ValueError(f"unsupported output dtype {dt} for Zarr v3")
    return _V3_DTYPE[dt]


def _v3_fill(dt: np.dtype) -> Any:
    return 0.0 if np.issubdtype(np.dtype(dt), np.floating) else 0


# --- output base resolution -------------------------------------------------- #


def _open_output_store(base_url: str):
    """Resolve ``base_url`` to a writable zarr store.

    ``s3://…`` → ``FsspecStore`` (s3fs / the ``s3`` extra); ``file://…`` or a
    bare path → ``LocalStore``. Imported lazily so the base install needs neither
    ``zarr`` nor ``s3fs``.
    """
    from zarr.storage import LocalStore

    if base_url.startswith("s3://") or "://" in base_url and not base_url.startswith("file://"):
        # Object-store / any fsspec-addressable target (needs the ``s3`` extra).
        from zarr.storage import FsspecStore

        return FsspecStore.from_url(base_url, read_only=False), base_url
    if base_url.startswith("file://"):
        path = base_url[len("file://") :]
    else:
        path = base_url
    os.makedirs(path, exist_ok=True)
    return LocalStore(path), path


# --- the writer -------------------------------------------------------------- #


class ZarrWriter:
    """The ``zarr`` streaming writer (sharded Zarr v3). The write mirror of
    :class:`earthsciio.backends.zarr.ZarrReader`.

    Every method delegates to :mod:`earthsciio.backends.zarr_stream`, which
    implements the streaming writer contract (``spec/writer.md``): regions,
    roles and the commit protocol, indexed records, statics, blobs and resume.
    A ``role="single"``, ``append``-mode store without statics is the v1 writer
    and still writes the ``earthsciio/output-manifest/v1`` manifest."""

    NAME = "zarr"

    def name(self) -> str:
        return self.NAME

    def write_open(
        self,
        base_url: str,
        schema: OutputSchema,
        role: str = "single",
        n_writers: int = 1,
        id: int = 0,
        region: Optional[Dict[str, Tuple[int, int]]] = None,
    ):
        """Open a store for writing. ``role="single"`` / ``"owner"`` CREATE it
        (group, coordinates, every array's ``zarr.json``; a v2 store also gets
        its manifest at ``commit_seq`` 0); ``role="contributor"`` JOINS an
        existing v2 store (reads the manifest, checks ``schema``'s fingerprint,
        writes nothing). ``region`` (``{dim: (start, stop)}``, 0-based half-open,
        shard-aligned) is this writer's block; ``None`` = the whole extent.
        ``n_writers`` is the owner's writer count (advisory for a contributor,
        checked only when > 1); ``id`` the contributor's writer id.
        Returns the handle threaded through the rest of the lifecycle."""
        from . import zarr_stream

        return zarr_stream.open_writer(
            base_url, schema, role=role, n_writers=n_writers, id=id, region=region
        )

    def write_resume(
        self,
        base_url: str,
        role: str = "single",
        n_writers: int = 1,
        id: int = 0,
        region: Optional[Dict[str, Tuple[int, int]]] = None,
        at: Optional[float] = None,
    ):
        """Reopen a store after preemption (``spec/writer.md`` §7): in
        ``append`` mode the committed records are truncated to ``t <= at``
        (``at=None`` keeps all) and this writer's region of the open time shard
        is reloaded, so later records continue the stream bitwise."""
        from . import zarr_stream

        return zarr_stream.resume_writer(
            base_url, role=role, n_writers=n_writers, id=id, region=region, at=at
        )

    def write_record(self, h, t: float, arrays: Dict[str, Any], region=None) -> int:
        """Append one time record (``append`` mode); returns its index. A
        non-``None`` ``region`` must equal the handle's bound region."""
        return h.write_record(t, arrays, region=region)

    def write_record_at(self, h, index: int, t: float, arrays: Dict[str, Any], region=None) -> None:
        """Write record ``index`` (``indexed`` mode, any order); ``t`` must equal
        ``times[index]`` bitwise and the record must not be committed."""
        h.write_record_at(index, t, arrays, region=region)

    def write_static(self, h, name: str, array: Any) -> None:
        """Write a time-less variable (region-shaped when partitioned)."""
        h.write_static(name, array)

    def write_blob(self, h, scope, key: str, data: bytes) -> None:
        """Store a small rank-local payload. ``scope`` is ``"static"`` or
        ``("record", i)``."""
        h.write_blob(scope, key, data)

    def write_flush(self, h):
        """Write every dirty shard of this writer's region and return a
        :class:`~earthsciio.backends.zarr_stream.CommitToken` for the records,
        statics and blobs written since the last token. A ``single`` writer
        also commits (when anything is pending) — the durable barrier."""
        return h.flush()

    def write_commit(self, h, tokens) -> Dict[str, Any]:
        """Owner: verify every writer's token and commit (``spec/writer.md``
        §5.3). Raises :class:`~earthsciio.errors.CommitError`, writing nothing,
        on a gap, overlap or divergent writer. Returns the new manifest."""
        return h.commit(tokens)

    def write_invalidate(self, h, indices) -> Dict[str, Any]:
        """Owner / single, ``indexed`` mode: mark records uncommitted so they
        may be rewritten."""
        return h.invalidate(indices)

    def write_invalidate_statics(self, h, names) -> Dict[str, Any]:
        """Owner / single: mark statics uncommitted so they may be rewritten."""
        return h.invalidate_statics(names)

    def write_close(self, h, tokens=()) -> Optional[Dict[str, Any]]:
        """Finish the handle. Single: flush, then rewrite the manifest. Owner:
        flush, then commit its own token plus the OTHER writers' ``tokens``.
        Both return the manifest dict. Contributor: errors if anything written
        is unflushed; returns ``None``."""
        return h.close(tokens)


def __getattr__(name: str):
    # The handle type lives in zarr_stream (which imports this module); resolve
    # the historical name lazily to avoid an import cycle.
    if name == "ZarrWriteHandle":
        from .zarr_stream import ZarrStreamHandle

        return ZarrStreamHandle
    raise AttributeError(f"module {__name__!r} has no attribute {name!r}")
