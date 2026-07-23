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
* The ``time`` axis grows by array resize: each :meth:`ZarrWriter.write_record`
  buffers one time slice; a full time-shard (or an explicit :meth:`write_flush`)
  resizes every growable array and writes the slab.

Commit / durability. Local output goes through zarr-python's ``LocalStore``
(atomic per-object writes); ``s3://`` output through ``FsspecStore`` (s3fs). A
per-store **output manifest** (``output_manifest.json``, schema
``earthsciio/output-manifest/v1``) records committed time-shards, the last durable
``t``, the codec params, a schema fingerprint, and the base URL — the mirror of the
read cache's per-blob manifest and the Julia writer's output manifest. It is
refreshed on every flush, so a restart reads the last committed ``t`` from it.

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

import json
import os
from dataclasses import dataclass, field
from typing import Any, Dict, List, Optional, Tuple

import numpy as np

__all__ = [
    "BloscProfile",
    "BLOSC_DIAGNOSTIC",
    "BLOSC_CHECKPOINT",
    "OutputVar",
    "OutputSchema",
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


def _profile(name: str) -> BloscProfile:
    if name == "diagnostic":
        return BLOSC_DIAGNOSTIC
    if name == "checkpoint":
        return BLOSC_CHECKPOINT
    raise ValueError(
        f"unknown codec profile {name!r} (expected 'diagnostic' or 'checkpoint')"
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
    * ``profile`` — ``"diagnostic"`` or ``"checkpoint"`` (codec params).
    * ``attrs`` — group-level attributes.
    * ``time_dtype`` — element type of the time coordinate (default float64).
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

    def __post_init__(self) -> None:
        self.dims = [(str(k), int(v)) for k, v in self.dims]
        self.time_dim = str(self.time_dim)
        self.vars = [(str(k), v) for k, v in self.vars]
        self.chunk_shape = {str(k): int(v) for k, v in self.chunk_shape.items()}
        self.shard_shape = {str(k): int(v) for k, v in self.shard_shape.items()}
        self.coords = [
            (str(k), (np.asarray(vals), dict(a))) for k, (vals, a) in self.coords
        ]
        self.attrs = dict(self.attrs)
        self.time_dtype = np.dtype(self.time_dtype)

    @property
    def dim_lengths(self) -> Dict[str, int]:
        return {k: v for k, v in self.dims}


# --- the write handle -------------------------------------------------------- #


@dataclass
class ZarrWriteHandle:
    """Opaque handle threaded through :meth:`write_record` / :meth:`write_close`."""

    base: str
    schema: OutputSchema
    group: Any
    store: Any
    arrays: Dict[str, Any]
    codec: BloscProfile
    shard_time: int
    #: buffered (not-yet-flushed) time-coord values for the current shard
    time_buffer: List[float] = field(default_factory=list)
    #: var name -> list of spatial slabs buffered for the current shard
    buffers: Dict[str, List[np.ndarray]] = field(default_factory=dict)
    total_records: int = 0
    shard_time_index: int = 0
    time_shards: List[Dict[str, Any]] = field(default_factory=list)
    shard_t_start: Optional[float] = None
    last_t: Optional[float] = None


# --- dtype <-> Zarr v3 data_type (for the manifest fingerprint) -------------- #

_V3_DTYPE = {
    np.dtype("float64"): "float64",
    np.dtype("float32"): "float32",
    np.dtype("int32"): "int32",
    np.dtype("int64"): "int64",
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

    if base_url.startswith("s3://") or "://" in base_url and not base_url.startswith(
        "file://"
    ):
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
    :class:`earthsciio.backends.zarr.ZarrReader`."""

    NAME = "zarr"

    def name(self) -> str:
        return self.NAME

    # -- write_open --------------------------------------------------------- #

    def write_open(self, base_url: str, schema: OutputSchema) -> ZarrWriteHandle:
        """Create the store: group node, static coords (written once), the
        growable time coordinate, and every streaming var at ``shape[time] = 0``.
        Returns the handle threaded through the rest of the lifecycle."""
        import zarr
        from zarr.codecs import BloscCodec

        codec = _profile(schema.profile)
        store, base = _open_output_store(base_url)

        # validate the chunk/shard grid
        for d, _ in schema.dims:
            if d not in schema.chunk_shape:
                raise ValueError(f"dim {d!r} missing from chunk_shape")
            if d not in schema.shard_shape:
                raise ValueError(f"dim {d!r} missing from shard_shape")
            if schema.shard_shape[d] % schema.chunk_shape[d] != 0:
                raise ValueError(
                    f"shard_shape[{d}]={schema.shard_shape[d]} is not a multiple of "
                    f"chunk_shape[{d}]={schema.chunk_shape[d]}"
                )
        shard_time = schema.shard_shape[schema.time_dim]

        group = zarr.open_group(store=store, mode="w", zarr_format=3)
        for k, v in schema.attrs.items():
            group.attrs[k] = v

        blosc = BloscCodec(
            cname=codec.cname,
            clevel=codec.clevel,
            shuffle="shuffle" if codec.shuffle else "noshuffle",
        )
        dimlen = schema.dim_lengths

        def _mk(name, dims, dtype, shape):
            chunks = tuple(schema.chunk_shape[d] for d in dims)
            shards = tuple(schema.shard_shape[d] for d in dims)
            return group.create_array(
                name=name,
                shape=tuple(shape),
                chunks=chunks,
                shards=shards,
                dtype=np.dtype(dtype),
                compressors=[blosc],
                dimension_names=list(dims),
                fill_value=_v3_fill(dtype),
                overwrite=True,
            )

        arrays: Dict[str, Any] = {}

        # static coords (values known now) — written once; time coord is separate
        time_attrs: Dict[str, Any] = {}
        for nm, (vals, attrs) in schema.coords:
            if nm == schema.time_dim:
                time_attrs = attrs
                continue
            varr = np.asarray(vals)
            a = _mk(nm, [nm], varr.dtype, varr.shape)
            a[...] = varr
            for k, v in attrs.items():
                a.attrs[k] = v
            arrays[nm] = a

        # growable time coordinate (1-D), starts empty
        tarr = _mk(schema.time_dim, [schema.time_dim], schema.time_dtype, (0,))
        for k, v in time_attrs.items():
            tarr.attrs[k] = v
        arrays[schema.time_dim] = tarr

        # streaming vars, shape[time] = 0
        for nm, ov in schema.vars:
            if schema.time_dim not in ov.dims:
                raise ValueError(
                    f"streaming var {nm!r} must include the time dim "
                    f"{schema.time_dim!r}"
                )
            shape = [0 if d == schema.time_dim else dimlen[d] for d in ov.dims]
            a = _mk(nm, ov.dims, ov.dtype, shape)
            for k, v in ov.attrs.items():
                a.attrs[k] = v
            arrays[nm] = a

        h = ZarrWriteHandle(
            base=base,
            schema=schema,
            group=group,
            store=store,
            arrays=arrays,
            codec=codec,
            shard_time=shard_time,
        )
        h.buffers = {nm: [] for nm, _ in schema.vars}
        return h

    # -- write_record ------------------------------------------------------- #

    def write_record(
        self, h: ZarrWriteHandle, t: float, arrays: Dict[str, Any], region=None
    ) -> None:
        """Buffer one time record. When the buffer fills a shard's worth of time
        steps (``shard_shape[time_dim]``) it is flushed durably."""
        if region is not None:
            raise ValueError("write_record `region` is reserved (None in Wave 4)")
        s = h.schema
        if not h.time_buffer:
            h.shard_t_start = float(t)
        h.time_buffer.append(float(t))
        for nm, ov in s.vars:
            if nm not in arrays:
                raise ValueError(f"write_record missing array for var {nm!r}")
            slab = np.asarray(arrays[nm], dtype=ov.dtype)
            expected = tuple(
                s.dim_lengths[d] for d in ov.dims if d != s.time_dim
            )
            if slab.shape != expected:
                raise ValueError(
                    f"var {nm!r}: slab shape {slab.shape} != expected {expected} "
                    f"(dims minus time)"
                )
            h.buffers[nm].append(slab)
        h.last_t = float(t)
        if len(h.time_buffer) >= h.shard_time:
            self._flush(h)

    # -- flush -------------------------------------------------------------- #

    def _flush(self, h: ZarrWriteHandle) -> None:
        n = len(h.time_buffer)
        if n == 0:
            return
        s = h.schema
        old = h.total_records
        new = old + n

        # time coordinate
        tarr = h.arrays[s.time_dim]
        tarr.resize((new,))
        tarr[old:new] = np.asarray(h.time_buffer, dtype=s.time_dtype)

        # streaming vars: resize the time axis and write the buffered slab block
        for nm, ov in s.vars:
            a = h.arrays[nm]
            ti = ov.dims.index(s.time_dim)
            newshape = list(a.shape)
            newshape[ti] = new
            a.resize(tuple(newshape))
            block = np.stack(h.buffers[nm], axis=ti)  # (…, n along time, …)
            sel = tuple(
                slice(old, new) if d == ti else slice(None)
                for d in range(len(ov.dims))
            )
            a[sel] = block

        h.time_shards.append(
            {
                "index": h.shard_time_index,
                "t_start": h.shard_t_start,
                "t_end": h.last_t,
                "n_records": n,
            }
        )
        h.total_records = new
        h.shard_time_index += 1
        h.time_buffer = []
        h.buffers = {nm: [] for nm, _ in s.vars}
        self._write_manifest(h)

    # -- write_flush (durable checkpoint barrier) --------------------------- #

    def write_flush(self, h: ZarrWriteHandle) -> None:
        """Force any buffered-but-unflushed records out as a durable (partial)
        shard and refresh the output manifest — the checkpoint durable barrier
        (RFC §10, §16.7). After this returns, a restart reading the manifest sees
        every record written so far as committed. No-op when the buffer is empty."""
        if h.time_buffer:
            self._flush(h)

    # -- write_close -------------------------------------------------------- #

    def write_close(self, h: ZarrWriteHandle) -> Dict[str, Any]:
        """Flush any trailing partial shard, finalize, and write the end-of-run
        output manifest (the close record). Returns the manifest dict."""
        if h.time_buffer:
            self._flush(h)
        return self._write_manifest(h)

    # -- output manifest ---------------------------------------------------- #

    def _manifest_dict(self, h: ZarrWriteHandle) -> Dict[str, Any]:
        from datetime import datetime, timezone

        s = h.schema
        codec = {
            "id": "blosc",
            "cname": h.codec.cname,
            "clevel": h.codec.clevel,
            "shuffle": "shuffle" if h.codec.shuffle else "noshuffle",
        }
        vars_meta = [
            {"name": nm, "dims": list(ov.dims), "dtype": _v3_dtype(ov.dtype)}
            for nm, ov in s.vars
        ]
        return {
            "schema": OUTPUT_MANIFEST_SCHEMA,
            "base_url": h.base,
            "format": "zarr",
            "zarr_format": 3,
            "profile": s.profile,
            "codec": codec,
            "time_dim": s.time_dim,
            "dims": [[k, v] for k, v in s.dims],
            "vars": vars_meta,
            "chunk_shape": dict(s.chunk_shape),
            "shard_shape": dict(s.shard_shape),
            "time_shards": list(h.time_shards),
            "last_t": h.last_t,
            "total_records": h.total_records,
            "written_at": datetime.now(timezone.utc)
            .replace(microsecond=0)
            .strftime("%Y-%m-%dT%H:%M:%SZ"),
        }

    def _write_manifest(self, h: ZarrWriteHandle) -> Dict[str, Any]:
        """Write ``output_manifest.json`` into the store as a sibling object,
        through the same store the arrays use (so it lands on local FS or S3)."""
        m = self._manifest_dict(h)
        text = json.dumps(m, indent=2, sort_keys=True) + "\n"
        self._put_manifest_bytes(h, text.encode("utf-8"))
        return m

    @staticmethod
    def _put_manifest_bytes(h: ZarrWriteHandle, data: bytes) -> None:
        from zarr.core.buffer import default_buffer_prototype
        from zarr.core.sync import sync

        buf = default_buffer_prototype().buffer.from_bytes(data)
        sync(h.store.set("output_manifest.json", buf))
