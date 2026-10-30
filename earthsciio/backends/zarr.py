"""The active, store-backed ``zarr`` reader — now built on **zarr-python 3.x**.

Rewritten (streaming-output-sinks RFC, Wave 4) to delegate all chunk math,
codec, endianness, order, sharding (Zarr **v3**) and v2 handling to the mature
``zarr`` Python library (zarr-python >= 3) instead of the previous hand-rolled
``numcodecs`` decoder. The library reads **both** Zarr v2 (the pinned ISRM store)
and Zarr v3 **sharded** stores (what the new :mod:`earthsciio.backends.zarr_write`
writer emits), so one reader round-trips the whole stack.

Why still "store-backed". A Zarr store is not one blob: each array's metadata
(``.zarray``/``.zattrs`` for v2, ``zarr.json`` for v3) and every chunk/shard is
its own object with its own URL. So the reader declares ``store_backed = True``
and is handed ``(cache, base_url, variables, select)`` (not a pre-fetched blob) —
it fetches each object it needs through the existing content-addressed cache
(``earthsciio.provider.Provider._read_file``). The bridge is :class:`_CacheStore`,
a ``zarr.abc.store.Store`` whose ``get(key)`` maps to ``cache.fetch(base/key)`` —
so zarr-python's own lazy chunk access (only the chunks a selection intersects are
fetched) rides directly on the content-addressed cache, offline included.

Decode contract (``spec/conformance.md`` §3, zarr notes) — preserved:

* blosc / zstd / gzip / none decompression, C/F order, endianness, sharding — all
  handled by zarr-python;
* dim names from v3 ``dimension_names`` or v2 ``.zattrs`` ``_ARRAY_DIMENSIONS``
  (synthesized ``dim_0…`` if absent); no coordinate arrays are produced;
* float kinds → **float64**, integer zarr dtypes keep their width;
* **``fill_value`` is NOT mapped to NaN** — a stored ``0.0`` is legitimate ISRM
  data. zarr-python already returns ``fill_value`` (not NaN) for absent chunks, so
  this deviation is the library default, not extra code.
* orthogonal ``select`` (per-axis index lists / slices, ordering preserved) is
  pushed down through zarr's ``oindex`` — fetching only the intersecting chunks.
  Both spellings (``spec/registries.md`` §2.2): positional ``axes`` over the
  arrays of matching rank, or named ``dims`` over every array carrying the
  dimension (``spec/prefetch.md`` §2.1). Either way a selection shrinks the
  DOWNLOAD, not only the decode.

**Memory profile — measured, not assumed.** The Julia and Rust zarr readers
decode *every* chunk a selection intersects into a map and only then assemble the
output, so their peak memory scales with the total decompressed chunk volume
rather than with the answer: on the real ISRM source-receptor array that is 416
chunks x ~21 MB = ~8.7 GB held simultaneously to produce a 0.59 GB result (~15x),
which OOM-killed a production run. **This reader does not have that shape**, and
that was established by measurement (``bench/zarr_peak_memory.py``), not by
reading zarr-python's source. Delegating to ``oindex`` means zarr-python
scatters each decoded chunk straight into a preallocated output and drops it, so
peak *live* memory is::

    peak ~= output + O(zarr async.concurrency x chunk size)

— independent of how many chunks the selection touches. Measured with
``tracemalloc`` (numpy data allocations included), growing the intersected chunk
volume 32x (50 MB -> 1602 MB) at a near-constant answer moved peak live memory
only 1.52x (20.9 MB -> 31.8 MB); with ISRM-sized 21 MB chunks peak live memory
sat at ~205 MB ~= 10 x 21 MB and was flat across 20/40/80 chunks. Extrapolated
to the real ISRM read that is ~1.1 GB (0.59 GB result + the float32->float64
``astype`` copy + ~0.2 GB of in-flight chunks) against Julia/Rust's ~8.7 GB.
**So no scatter rewrite is needed here** — the amplification the sibling tracks
are being fixed for does not exist in Python.

Two caveats worth knowing:

* peak **RSS** does grow with chunk count (5-10x above live memory), but that is
  glibc arena retention of *freed* chunk buffers, not simultaneous liveness:
  pinning ``MALLOC_MMAP_THRESHOLD_=131072`` collapses RSS back onto the live
  figure (313 MB -> 30 MB at 800 chunks). Measure liveness, not RSS, here.
* the ``astype`` below is a float32->float64 copy, so a *dense* read transiently
  costs 1.5x the output. That is a property of the §3 float64 output contract,
  not of chunk handling, and it does not scale with chunk count.

``tests/test_zarr_reader.py`` pins both halves of the invariant
(``test_selection_streams_chunks_and_does_not_buffer_them_all`` and
``test_peak_memory_is_flat_in_the_number_of_chunks``) so a future refactor that
re-collects chunks into a dict cannot silently reintroduce the amplification.

**v2 compatibility deviation.** zarr-python 3.x's stricter v2 metadata parser
rejects ``.zarray`` with ``dimension_separator: null`` (the corpus fixtures write
``null``; the Zarr v2 spec default is ``"."``). :class:`_CacheStore` normalizes a
``null``/absent v2 ``dimension_separator`` to ``"."`` on the way out — a documented,
value-preserving shim that keeps the existing v2 corpus readable (RFC §16.1 keeps
v2 read support).

``zarr``/``fsspec`` are imported **lazily** inside the reader methods, shipped as
the optional ``zarr`` extra, so a base cache/transport install stays lean (the
same optional-extra culture the old ``numcodecs`` dependency followed).
"""

from __future__ import annotations

import json
from collections.abc import Sequence
from typing import Any

import numpy as np

from ..errors import CacheMiss, FetchError, Unsupported
from ..native import NativeDataset, NativeField

__all__ = ["ZarrReader"]


# The per-axis selector vocabulary is SHARED with every other reader that can
# honour a `select` (the whole-file netcdf reader honours the same spelling at
# decode time), so it lives in one module rather than in this backend.
from ..selection import (
    _absent_dims_error,
    _parse_axis,
    _resolve_axis_indices,
    _select_axes,
    _select_dims,
)

# --------------------------------------------------------------------------- #
# The cache-backed zarr Store: bridges zarr-python object access to the
# content-addressed cache. Read-only; no listing (variables are always explicit).
# --------------------------------------------------------------------------- #


def _make_cache_store(cache: Any, base_url: str):
    """Build a ``zarr.abc.store.Store`` that fetches objects through ``cache``.

    Defined as a factory (not a module-level class) so the ``zarr`` import stays
    lazy — the base install has no ``zarr``. Returns a :class:`Store` instance
    whose ``get(key)`` resolves ``<base_url>/<key>`` via ``cache.fetch`` (a cache
    miss ⇒ ``None``, the zarr "object absent" signal — which yields ``fill_value``,
    never NaN).
    """
    from zarr.abc.store import (
        OffsetByteRequest,
        RangeByteRequest,
        Store,
        SuffixByteRequest,
    )

    def _normalize_v2_zarray(raw: bytes) -> bytes:
        """Fill a v2 ``.zarray`` ``dimension_separator: null`` with the spec
        default ``"."`` so zarr-python 3.x's stricter parser accepts the corpus
        fixtures (value-preserving; see module docstring)."""
        try:
            meta = json.loads(raw.decode("utf-8"))
        except Exception:
            return raw
        if int(meta.get("zarr_format", 0)) == 2 and meta.get("dimension_separator") in (None, ""):
            meta["dimension_separator"] = "."
            return json.dumps(meta).encode("utf-8")
        return raw

    class _CacheStore(Store):  # type: ignore[misc]
        """Read-only zarr store over the EarthSciIO content-addressed cache."""

        def __init__(self, cache: Any, base: str) -> None:
            super().__init__(read_only=True)
            self._cache = cache
            self._base = base.rstrip("/")

        def __eq__(self, other: object) -> bool:
            return (
                isinstance(other, _CacheStore)
                and other._base == self._base
                and other._cache is self._cache
            )

        def __hash__(self) -> int:
            return hash((id(self._cache), self._base))

        # -- capability flags (read-only, non-enumerable object store) ------- #
        @property
        def supports_writes(self) -> bool:
            return False

        @property
        def supports_deletes(self) -> bool:
            return False

        @property
        def supports_partial_writes(self) -> bool:
            return False

        @property
        def supports_listing(self) -> bool:
            # Arrays are opened by explicit path; no ListObjects is available on
            # the anonymous object store, so listing is unsupported.
            return False

        # -- the one load-bearing method: fetch an object through the cache -- #
        def _raw(self, key: str) -> bytes | None:
            url = f"{self._base}/{key}"
            try:
                entry = self._cache.fetch(url)
            except CacheMiss:
                return None
            except FetchError as exc:
                # zarr's Store contract: a MISSING key is `None`, not an error.
                # This is not a nicety — zarr-python 3 probes `zarr.json` (v3)
                # before falling back to `.zarray` (v2), so raising on that 404
                # makes every v2 store over http/s3 unreadable. Only a
                # DEFINITIVE absence maps to None; a timeout or 5xx still
                # raises, because answering "absent" there would silently
                # mis-report a live store as empty.
                if exc.not_found:
                    return None
                raise
            with open(entry.path, "rb") as fh:
                data = fh.read()
            if key.endswith(".zarray"):
                data = _normalize_v2_zarray(data)
            return data

        async def get(self, key, prototype, byte_range=None):
            data = self._raw(key)
            if data is None:
                return None
            if byte_range is not None:
                if isinstance(byte_range, RangeByteRequest):
                    data = data[byte_range.start : byte_range.end]
                elif isinstance(byte_range, OffsetByteRequest):
                    data = data[byte_range.offset :]
                elif isinstance(byte_range, SuffixByteRequest):
                    data = data[-byte_range.suffix :]
            return prototype.buffer.from_bytes(data)

        async def get_partial_values(self, prototype, key_ranges):
            return [await self.get(k, prototype, r) for k, r in key_ranges]

        async def exists(self, key) -> bool:
            return self._raw(key) is not None

        async def set(self, key, value):  # pragma: no cover - read-only
            raise NotImplementedError("the cache-backed zarr store is read-only")

        async def delete(self, key):  # pragma: no cover - read-only
            raise NotImplementedError("the cache-backed zarr store is read-only")

        async def list(self):  # pragma: no cover - non-enumerable
            return
            yield  # make this an async generator

        async def list_dir(self, prefix):  # pragma: no cover - non-enumerable
            return
            yield

        async def list_prefix(self, prefix):  # pragma: no cover - non-enumerable
            return
            yield

    return _CacheStore(cache, base_url)


# --------------------------------------------------------------------------- #
# Metadata helpers.
# --------------------------------------------------------------------------- #


def _finalize_dtype(np_dtype: np.dtype) -> np.dtype:
    """The §3 logical output dtype: float kinds → float64; ints keep width."""
    if np.issubdtype(np_dtype, np.floating):
        return np.dtype("float64")
    return np_dtype


def _dims_of(arr: Any) -> list[str]:
    """Ordered dim names for a zarr array: v3 ``dimension_names`` or v2
    ``_ARRAY_DIMENSIONS``; synthesize ``dim_0…`` when neither is present."""
    ndim = arr.ndim
    names = getattr(arr.metadata, "dimension_names", None)
    if names is not None and all(n is not None for n in names) and len(names) == ndim:
        return [str(n) for n in names]
    aad = None
    try:
        aad = arr.attrs.get("_ARRAY_DIMENSIONS")
    except Exception:  # pragma: no cover - defensive
        aad = None
    if aad is not None and len(aad) == ndim:
        return [str(d) for d in aad]
    return [f"dim_{i}" for i in range(ndim)]


# --------------------------------------------------------------------------- #
# The reader.
# --------------------------------------------------------------------------- #


class ZarrReader:
    """The active, store-backed ``zarr`` reader (Zarr v2 + v3 sharded), built on
    zarr-python 3.x."""

    #: Registry name + format key(s) + extension sniff hints.
    NAME = "zarr"
    FORMATS = ("zarr",)
    EXTENSIONS = ("zarr",)
    #: zarr-python 3.x; the `zarr` extra (needs Python >=3.11).
    REQUIRES = (("zarr",),)

    #: This reader needs ``(cache, base_url)``, not a single pre-fetched blob path.
    store_backed = True

    #: This reader honours a per-axis orthogonal ``select`` at read time (lazy
    #: projection pushdown — see :func:`earthsciio.registry.supports_selection`).
    supports_selection = True

    def formats(self) -> list[str]:
        return list(self.FORMATS)

    def extensions(self) -> list[str]:
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
        select: Any | None = None,
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
        variables: Sequence[str] | None,
        select: Any | None = None,
        **_: Any,
    ) -> NativeDataset:
        """Read ``variables`` from the Zarr store at ``base_url`` under ``select``.

        ``variables`` is **required** (unlike NetCDF): the anonymous object store
        exposes no ``ListObjects``, so the reader cannot enumerate arrays. Each
        array is opened by explicit path through zarr-python, which auto-detects
        v2 vs v3. ``select`` is one orthogonal selection applied to each requested
        array whose rank matches its axis count (arrays of other rank read whole)
        — so one ``select`` sub-slices a 3-D SR array while a 1-D geometry array
        reads fully.

        The NAMED form ``{"dims": {...}}`` (``spec/prefetch.md`` §2.1) instead
        applies each selector to every requested array carrying that dimension,
        whatever its rank, unnamed dimensions ``"all"``. The names are the
        arrays' own (v3 ``dimension_names``, v2 ``_ARRAY_DIMENSIONS``, else the
        synthesized ``dim_0…``, which are nameable too), and they are resolved
        against EVERY array's metadata before any chunk is fetched: a name no
        requested array carries, or an index past an array's length, fails
        having read only ``.zarray``/``.zattrs``. There are no coordinate arrays
        here, so "the fields of the read" are the requested arrays.

        A named selector other than ``"all"`` on a dimension named ``time``
        (case-insensitively) is REFUSED, as the netcdf reader refuses one:
        choosing records is the Provider's. A store carries no coordinate
        variables, so the name is the whole of the rule. The positional form
        keeps its historical behaviour and does not refuse — the named form is
        new, so it starts from the rule every reader shares
        (``spec/prefetch.md`` §2.1).
        """
        if not variables:
            raise ValueError(
                "the zarr reader requires an explicit list of variables (arrays); "
                "the store cannot be enumerated without consolidated metadata"
            )
        import zarr  # lazy: optional ``zarr`` extra

        store = _make_cache_store(cache, base_url)
        named = _select_dims(select)
        axes_spec = _select_axes(select)

        if named is not None:
            return self._read_named(zarr, store, variables, named)

        out_vars: dict[str, NativeField] = {}
        for array in variables:
            arr = zarr.open_array(store=store, path=str(array), mode="r")
            dims = _dims_of(arr)
            ndim = arr.ndim

            if axes_spec is not None and len(axes_spec) == ndim:
                axes = [_parse_axis(a) for a in axes_spec]
                key = tuple(
                    slice(None)
                    if axes[d][0] == "all"
                    else np.asarray(_resolve_axis_indices(axes[d], arr.shape[d]), dtype=np.intp)
                    for d in range(ndim)
                )
                data = arr.oindex[key]
            else:
                data = arr[...]

            data = np.asarray(data)
            data = data.astype(_finalize_dtype(data.dtype), copy=False)
            out_vars[str(array)] = NativeField(data, dims, {})

        return NativeDataset(out_vars, {})

    def _read_named(
        self,
        zarr: Any,
        store: Any,
        variables: Sequence[str],
        named: dict[str, tuple],
    ) -> NativeDataset:
        """The named-dimension read: plan every array, THEN fetch chunks.

        Planning opens metadata only, so every error the window can raise —
        an absent name, an out-of-range index — is raised before the first
        chunk object is requested. The fetch is then one ``oindex`` per array,
        which asks zarr-python for exactly the chunks the per-axis index arrays
        intersect (``named-window-zarr`` poisons every other chunk object).
        """
        arrays = [(str(a), zarr.open_array(store=store, path=str(a), mode="r")) for a in variables]
        dims_of = {name: _dims_of(arr) for name, arr in arrays}
        present = {d for dims in dims_of.values() for d in dims}
        absent = [d for d in named if d not in present]
        if absent:
            raise _absent_dims_error(absent, present)
        for dim, axis in named.items():
            if axis[0] != "all" and dim.lower() == "time":
                raise ValueError(
                    f"select asks for a subset of the time dimension {dim!r}; record "
                    "selection is the Provider's, not the reader's — a time dimension "
                    'in a named `select` may only be "all"'
                )

        plans = []
        for name, arr in arrays:
            key = tuple(
                slice(None)
                if dim not in named or named[dim][0] == "all"
                else np.asarray(_resolve_axis_indices(named[dim], arr.shape[d]), dtype=np.intp)
                for d, dim in enumerate(dims_of[name])
            )
            plans.append((name, arr, key))

        out_vars: dict[str, NativeField] = {}
        for name, arr, key in plans:
            if all(isinstance(k, slice) for k in key):
                data = arr[...]
            else:
                data = arr.oindex[key]
            data = np.asarray(data)
            data = data.astype(_finalize_dtype(data.dtype), copy=False)
            out_vars[name] = NativeField(data, dims_of[name], {})
        return NativeDataset(out_vars, {})

    # -- shape probe -------------------------------------------------------- #

    def array_shape(self, cache: Any, base_url: str, var: str) -> tuple[int, ...]:
        """The full (dims-order) shape of array ``var`` in the Zarr store at
        ``base_url``, learned by opening ONLY that array's metadata object —
        NEVER a chunk. A lightweight honour/refuse probe for projection-pushdown
        decisions (mirrors the Julia/Rust ``array_shape``)."""
        import zarr  # lazy

        store = _make_cache_store(cache, base_url)
        arr = zarr.open_array(store=store, path=str(var), mode="r")
        return tuple(int(s) for s in arr.shape)
