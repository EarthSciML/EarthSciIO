"""The active ``zarr`` reader — a **store-backed** chunked-array reader.

A Zarr v2 store is not one blob: each array's ``.zarray``/``.zattrs`` metadata and
every chunk is its **own object with its own URL**, so "lazy partial read" is just
"fetch only the chunk objects the selection intersects, each through the existing
content-addressed cache" (``spec/cloud-future.md`` §3; the zarr impl spec). No new
cache-key scheme and no byte-range machinery are needed for the pinned v2 target.

This reader therefore declares itself **store-backed** (``store_backed = True``):
the Provider hands it ``(cache, base_url, variables, select)`` instead of a single
pre-fetched blob path (``earthsciio.provider.Provider._read_file``). It fetches
each object it needs — ``<base_url>/<array>/.zarray``, ``…/.zattrs`` (optional),
and only the intersecting ``…/<chunk_key>`` chunk objects — through
``cache.fetch(obj_url)``.

Decode contract (``spec/conformance.md`` §3, zarr notes):

* blosc (``cname`` lz4/lz4hc/zlib/zstd/blosclz) / zlib / zstd / gzip / none
  decompression via ``numcodecs`` (c-blosc undoes the shuffle filter and the
  multi-block container internally);
* C-order (or F-order) chunk unpack;
* endianness taken from the ``dtype`` typestr (``<f4``/``<f8`` → float64), integer
  zarr dtypes keep int32/int64;
* dim names from ``.zattrs`` ``_ARRAY_DIMENSIONS`` (synthesized ``dim_0…`` if
  absent); no coordinate arrays are produced (like the CSV reader);
* **``fill_value`` is NOT mapped to NaN** — a deliberate deviation from the NetCDF
  ``_FillValue → NaN`` rule, because in the pinned ISRM store ``fill_value == 0.0``
  is a legitimate data value. ``fill_value`` fills only the region of a chunk
  object that is **absent** (a cache/transport miss).

``numcodecs`` is imported lazily inside :meth:`read_store`, shipped as the
optional ``zarr`` extra so the cache/transport core stays lean.
"""

from __future__ import annotations

import itertools
from typing import Any, Dict, List, Optional, Sequence, Tuple

import numpy as np

from ..errors import CacheMiss, Unsupported
from ..native import NativeDataset, NativeField

__all__ = ["ZarrReader"]


# --------------------------------------------------------------------------- #
# Per-axis selectors (orthogonal selection).  Each is a small tagged tuple.
# --------------------------------------------------------------------------- #

_ALL = ("all",)


def _parse_axis(spec: Any) -> Tuple:
    """Normalize one axis selector to a tagged tuple.

    Accepts ``"all"``/``None`` (whole axis), ``{"indices": [...]}`` (an explicit,
    possibly non-contiguous, ordered index list), ``{"slice": [start, stop,
    step?]}`` (a strided range, ``step`` defaults to 1), or a bare list of ints
    (shorthand for ``indices``).
    """
    if spec is None or spec == "all":
        return _ALL
    if isinstance(spec, dict):
        if "indices" in spec:
            return ("indices", [int(i) for i in spec["indices"]])
        if "slice" in spec:
            s = list(spec["slice"])
            start = int(s[0])
            stop = int(s[1])
            step = int(s[2]) if len(s) > 2 else 1
            return ("slice", start, stop, step)
        raise ValueError(f"unrecognized axis selector: {spec!r}")
    if isinstance(spec, (list, tuple)):
        return ("indices", [int(i) for i in spec])
    raise ValueError(f"unrecognized axis selector: {spec!r}")


def _select_axes(select: Any) -> Optional[List[Any]]:
    """Extract the ordered per-axis selector list from a ``select`` argument.

    ``None`` ⇒ ``None`` (all). A ``{"axes": [...]}`` mapping or a bare list both
    yield the axis list. Anything else ⇒ ``None`` (all).
    """
    if select is None:
        return None
    if isinstance(select, dict) and "axes" in select:
        return list(select["axes"])
    if isinstance(select, (list, tuple)):
        return list(select)
    return None


def _resolve_axis_indices(axis: Tuple, dim_len: int) -> List[int]:
    """Resolve a tagged axis selector to its ordered list of global indices."""
    if axis[0] == "all":
        return list(range(dim_len))
    if axis[0] == "indices":
        idxs = axis[1]
        for g in idxs:
            if g < 0 or g >= dim_len:
                raise IndexError(f"index {g} out of range for dimension length {dim_len}")
        return list(idxs)
    if axis[0] == "slice":
        _, start, stop, step = axis
        if step < 1:
            raise ValueError(f"slice step must be >= 1, got {step}")
        return list(range(start, stop, step))
    raise ValueError(f"unrecognized axis selector: {axis!r}")


# --------------------------------------------------------------------------- #
# .zarray / .zattrs metadata.
# --------------------------------------------------------------------------- #


class _ZArray:
    """Parsed ``.zarray`` metadata the reader consumes."""

    __slots__ = (
        "shape",
        "chunks",
        "typestr",
        "np_dtype",
        "compressor",
        "order",
        "fill_value",
        "dim_sep",
    )

    def __init__(self, meta: Dict[str, Any]) -> None:
        if int(meta.get("zarr_format", 2)) != 2:
            raise ValueError(
                f"zarr reader supports zarr_format 2 only, got {meta.get('zarr_format')!r} "
                "(v3 is future work)"
            )
        self.shape: Tuple[int, ...] = tuple(int(s) for s in meta["shape"])
        self.chunks: Tuple[int, ...] = tuple(int(c) for c in meta["chunks"])
        if len(self.shape) != len(self.chunks):
            raise ValueError(f"shape {self.shape} and chunks {self.chunks} rank mismatch")
        self.typestr: str = str(meta["dtype"])
        self.np_dtype = np.dtype(self.typestr)
        self.compressor = meta.get("compressor")
        filters = meta.get("filters")
        if filters:
            raise ValueError(
                "zarr reader does not support a filter pipeline yet "
                f"(filters={filters!r}); only compressor codecs are supported"
            )
        self.order: str = str(meta.get("order", "C"))
        if self.order not in ("C", "F"):
            raise ValueError(f"unknown zarr order {self.order!r} (expected 'C' or 'F')")
        self.fill_value = meta.get("fill_value", 0)
        sep = meta.get("dimension_separator")
        self.dim_sep: str = "." if sep in (None, "") else str(sep)

    @property
    def ndim(self) -> int:
        return len(self.shape)


def _parse_zattrs(meta: Optional[Dict[str, Any]], ndim: int) -> List[str]:
    """Ordered dim names from ``_ARRAY_DIMENSIONS``; synthesize ``dim_0…`` if absent."""
    if meta:
        dims = meta.get("_ARRAY_DIMENSIONS")
        if dims is not None:
            names = [str(d) for d in dims]
            if len(names) == ndim:
                return names
    return [f"dim_{i}" for i in range(ndim)]


# --------------------------------------------------------------------------- #
# Chunk math.
# --------------------------------------------------------------------------- #


def _chunk_key(chunk_idx: Sequence[int], sep: str) -> str:
    """The object key for a chunk, its per-dim chunk indices joined by ``sep``."""
    return sep.join(str(int(c)) for c in chunk_idx)


def _needed_chunks(
    sel_indices: Sequence[Sequence[int]], chunks: Sequence[int]
) -> List[Tuple[int, ...]]:
    """The **set** of chunk id tuples the orthogonal selection intersects.

    For each dim, every requested global index ``g`` maps to chunk ``g //
    chunk_len``; the dim's needed chunk ids are the distinct such values. The
    chunk keys to fetch are the Cartesian product of the per-dim id sets — the
    crux of laziness: an unselected chunk is never in this list.
    """
    per_dim: List[List[int]] = []
    for d, idxs in enumerate(sel_indices):
        cl = chunks[d]
        per_dim.append(sorted({g // cl for g in idxs}))
    return [tuple(p) for p in itertools.product(*per_dim)]


# --------------------------------------------------------------------------- #
# Decompression.
# --------------------------------------------------------------------------- #


def _to_bytes(x: Any) -> bytes:
    if isinstance(x, (bytes, bytearray, memoryview)):
        return bytes(x)
    return np.asarray(x).tobytes()


def _decompress(compressor: Optional[Dict[str, Any]], raw: bytes) -> bytes:
    """Decompress one chunk object's bytes per the array's ``compressor``.

    Dispatches on the codec ``id``. For blosc the c-blosc container is
    self-describing (codec + shuffle filter + multi-block layout are in the
    16-byte header), so a single ``numcodecs.Blosc().decode`` undoes lz4/zstd/
    zlib/blosclz + the shuffle. ``None`` ⇒ raw (uncompressed store).
    """
    if compressor is None:
        return _to_bytes(raw)
    cid = str(compressor.get("id", "")).lower()
    if cid == "blosc":
        from numcodecs import Blosc

        return _to_bytes(Blosc().decode(raw))
    if cid in ("zlib",):
        from numcodecs import Zlib

        return _to_bytes(Zlib().decode(raw))
    if cid in ("gzip",):
        from numcodecs import GZip

        return _to_bytes(GZip().decode(raw))
    if cid in ("zstd",):
        from numcodecs import Zstd

        return _to_bytes(Zstd().decode(raw))
    if cid in ("", "none"):
        return _to_bytes(raw)
    raise ValueError(f"unsupported zarr compressor id {cid!r}")


def _finalize_dtype(np_dtype: np.dtype) -> np.dtype:
    """The §3 logical output dtype: float kinds → float64; ints keep width."""
    if np.issubdtype(np_dtype, np.floating):
        return np.dtype("float64")
    return np_dtype


# --------------------------------------------------------------------------- #
# Assembly.
# --------------------------------------------------------------------------- #


def _assemble(
    sel_indices: Sequence[Sequence[int]],
    meta: _ZArray,
    buffers: Dict[Tuple[int, ...], Optional[np.ndarray]],
) -> np.ndarray:
    """Scatter the fetched chunk buffers into the output selection array.

    ``buffers`` maps each needed chunk id tuple to its decompressed
    ``chunks``-shaped array (or ``None`` for an absent chunk object → filled with
    ``fill_value``). Output is C-order in the **selection shape**
    ``[len(sel_d) for d]``, normalized to the §3 logical dtype.
    """
    chunks = meta.chunks
    out_dtype = _finalize_dtype(meta.np_dtype)
    sel_shape = tuple(len(s) for s in sel_indices)
    fill = 0.0 if meta.fill_value is None else meta.fill_value
    out = np.full(sel_shape, fill, dtype=out_dtype)

    # Per dim: group output positions (and within-chunk offsets) by chunk id.
    out_pos: List[Dict[int, List[int]]] = []
    within: List[Dict[int, List[int]]] = []
    for d, idxs in enumerate(sel_indices):
        cl = chunks[d]
        op: Dict[int, List[int]] = {}
        wi: Dict[int, List[int]] = {}
        for o, g in enumerate(idxs):
            c = g // cl
            op.setdefault(c, []).append(o)
            wi.setdefault(c, []).append(g % cl)
        out_pos.append(op)
        within.append(wi)

    for ck, block in buffers.items():
        out_ix = np.ix_(*[np.asarray(out_pos[d][ck[d]], dtype=np.intp) for d in range(len(ck))])
        if block is None:
            out[out_ix] = fill
        else:
            in_ix = np.ix_(*[np.asarray(within[d][ck[d]], dtype=np.intp) for d in range(len(ck))])
            out[out_ix] = block[in_ix].astype(out_dtype, copy=False)
    return out


# --------------------------------------------------------------------------- #
# The reader.
# --------------------------------------------------------------------------- #


class ZarrReader:
    """The active, store-backed ``zarr`` reader (Zarr v2 chunked arrays)."""

    #: Registry name + format key(s) + extension sniff hints.
    NAME = "zarr"
    FORMATS = ("zarr",)
    EXTENSIONS = ("zarr",)

    #: This reader needs ``(cache, base_url)``, not a single pre-fetched blob path.
    store_backed = True

    def formats(self) -> List[str]:
        return list(self.FORMATS)

    def extensions(self) -> List[str]:
        return list(self.EXTENSIONS)

    def open(self, blob_path: Any) -> Any:
        raise Unsupported(
            self.NAME,
            registry="format",
            operation="open",
            tracking="esio-cloud",
        )

    def read_native(
        self,
        handle: Any,
        variables: Any = None,
        select: Optional[Any] = None,
        **_: Any,
    ) -> NativeDataset:
        raise Unsupported(
            self.NAME,
            registry="format",
            operation="read_native",
            tracking="esio-cloud",
        )

    # -- the store-backed entry point --------------------------------------- #

    def read_store(
        self,
        cache: Any,
        base_url: str,
        variables: Optional[Sequence[str]],
        select: Optional[Any] = None,
        **_: Any,
    ) -> NativeDataset:
        """Read ``variables`` from the Zarr store at ``base_url`` under ``select``.

        ``variables`` is **required** (unlike NetCDF): with ``.zmetadata`` absent
        and no anonymous ``ListObjects``, the reader cannot enumerate arrays.
        ``select`` is a single orthogonal selection applied to each requested
        array whose rank matches the number of axes (arrays of other rank read
        whole) — so one ``select`` can sub-slice a 3-D SR array while a 1-D
        geometry array reads fully.
        """
        if not variables:
            raise ValueError(
                "the zarr reader requires an explicit list of variables (arrays); "
                "the store cannot be enumerated without a consolidated .zmetadata"
            )
        base = base_url.rstrip("/")
        axes_spec = _select_axes(select)

        out_vars: Dict[str, NativeField] = {}
        for array in variables:
            meta = _ZArray(self._fetch_json(cache, f"{base}/{array}/.zarray"))
            zattrs = self._fetch_json_optional(cache, f"{base}/{array}/.zattrs")
            dims = _parse_zattrs(zattrs, meta.ndim)

            # Resolve the per-axis global index lists (ndim-match on the selection).
            if axes_spec is not None and len(axes_spec) == meta.ndim:
                axes = [_parse_axis(a) for a in axes_spec]
            else:
                axes = [_ALL] * meta.ndim
            sel_indices = [
                _resolve_axis_indices(axes[d], meta.shape[d]) for d in range(meta.ndim)
            ]

            buffers: Dict[Tuple[int, ...], Optional[np.ndarray]] = {}
            for ck in _needed_chunks(sel_indices, meta.chunks):
                url = f"{base}/{array}/{_chunk_key(ck, meta.dim_sep)}"
                raw = self._fetch_bytes_optional(cache, url)
                if raw is None:
                    buffers[ck] = None  # absent chunk object → fill_value region
                else:
                    dec = _decompress(meta.compressor, raw)
                    arr = np.frombuffer(dec, dtype=meta.np_dtype)
                    buffers[ck] = arr.reshape(meta.chunks, order=meta.order)

            data = _assemble(sel_indices, meta, buffers)
            out_vars[str(array)] = NativeField(data, dims, {})

        return NativeDataset(out_vars, {})

    # -- object fetch helpers ----------------------------------------------- #

    @staticmethod
    def _fetch_bytes(cache: Any, url: str) -> bytes:
        entry = cache.fetch(url)
        with open(entry.path, "rb") as fh:
            return fh.read()

    @classmethod
    def _fetch_bytes_optional(cls, cache: Any, url: str) -> Optional[bytes]:
        try:
            return cls._fetch_bytes(cache, url)
        except CacheMiss:
            return None

    @classmethod
    def _fetch_json(cls, cache: Any, url: str) -> Dict[str, Any]:
        import json

        return json.loads(cls._fetch_bytes(cache, url).decode("utf-8"))

    @classmethod
    def _fetch_json_optional(cls, cache: Any, url: str) -> Optional[Dict[str, Any]]:
        import json

        raw = cls._fetch_bytes_optional(cache, url)
        if raw is None:
            return None
        return json.loads(raw.decode("utf-8"))
