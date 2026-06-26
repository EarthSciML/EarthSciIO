"""Active format readers (component (b)) — the decode half of the Provider.

A reader opens a cached blob and returns RAW native-grid arrays keyed by the
on-disk ``file_variable`` name. It applies ONLY the format/CF decode pinned by
``spec/conformance.md`` §3; it does NOT remap variable names or convert units
(those stay in ESS — Risk R3). Readers register into the shared
:data:`~earthsciio.registry.format_registry` by name (``netcdf``, ``csv``), so a
new format plugs in with a new :class:`~earthsciio.registry.Reader` + one
``register`` line — never a Provider edit (``spec/registries.md`` §4).

These are the **active** counterparts to the ``zarr`` stub
(:class:`earthsciio.backends.zarr.ZarrReader`). They mirror the Julia
``NetCDFReader``/``CSVReader`` (``julia/src/readers.jl``) and the Rust
``netcdf`` reader, and they decode **byte-identically** to the conformance
oracle (``conformance/verify.py``) so cross-language array equality holds
(``esio-9nb.9``).

The netcdf reader needs xarray + netCDF4 (the optional ``netcdf`` extra); it
imports them lazily so the cache/transport core stays lean.
"""

from __future__ import annotations

import csv as _csv
from typing import Any, Dict, List, Optional, Sequence

import numpy as np

from .native import NativeDataset, NativeField
from .registry import Registry, format_registry

__all__ = ["NetCDFReader", "CSVReader", "register_format_readers"]


# --------------------------------------------------------------------------- #
# Shared decode helper (spec/conformance.md §3, "Numeric dtype").
# --------------------------------------------------------------------------- #


def _finalize_numeric(values: Any) -> np.ndarray:
    """Normalize a decoded numeric array to its §3 logical dtype.

    Floats (incl. CF-unpacked / mask-and-scaled values, which carry ``NaN`` for
    fill cells) become ``float64``; an unpacked pure-integer field (e.g. a raw
    ``hours since`` time axis) keeps its integer dtype. This removes the
    float32-vs-float64 ambiguity between xarray / NCDatasets / netcdf-rs.
    """
    arr = np.asarray(values)
    if np.issubdtype(arr.dtype, np.floating):
        return arr.astype("float64", copy=False)
    if np.issubdtype(arr.dtype, np.integer):
        return arr  # raw integer read kept as-is (int32/int64)
    return arr


# --------------------------------------------------------------------------- #
# NetCDF reader (xarray / netCDF4).
# --------------------------------------------------------------------------- #


def _field_from_dataarray(da: Any) -> NativeField:
    """Build a :class:`NativeField` from an ``xarray.DataArray`` in file order.

    xarray reports ``dims`` and ``values`` in the on-disk dimension order (unlike
    NCDatasets, which the Julia track has to reverse), so no permute is needed.
    Only the decode-relevant ``units``/``calendar`` attributes are carried — the
    CF packing attrs (``scale_factor``/``add_offset``/``_FillValue``) are consumed
    by ``mask_and_scale`` and intentionally dropped.
    """
    data = _finalize_numeric(da.values)
    dims = tuple(str(d) for d in da.dims)
    attrs = {k: da.attrs[k] for k in ("units", "calendar") if k in da.attrs}
    return NativeField(data, dims, attrs)


class NetCDFReader:
    """The active ``netcdf`` reader, backed by xarray (netCDF4 engine).

    CF-decodes per ``spec/conformance.md`` §3, identically to the oracle
    (``conformance/verify.py``): opens with ``decode_times=False`` and
    ``mask_and_scale=True``, so ``scale_factor``/``add_offset`` are applied in
    float64, ``_FillValue``/``missing_value`` cells become ``NaN``, and the time
    axis is returned **raw** (its stored integers) with ``units``+``calendar``
    carried in ``attrs`` for ESS. Data variables land in ``variables``;
    dimension coordinates (latitude/longitude/time) land in ``coords``.
    """

    #: Registry name + format key(s) + extension sniff hints.
    NAME = "netcdf"
    FORMATS = ("netcdf",)
    EXTENSIONS = ("nc", "nc4", "cdf")

    def formats(self) -> List[str]:
        return list(self.FORMATS)

    def extensions(self) -> List[str]:
        return list(self.EXTENSIONS)

    def open(self, blob_path: Any) -> Any:
        """Return the blob path as the handle; the dataset is opened in
        :meth:`read_native` under a ``with`` block so nothing leaks."""
        return blob_path

    def read_native(
        self,
        handle: Any,
        variables: Optional[Sequence[str]] = None,
        select: Optional[Any] = None,
        **_: Any,
    ) -> NativeDataset:
        """Decode ``handle`` into a :class:`NativeDataset`.

        ``variables`` (on-disk ``file_variable`` names) restricts the returned
        **data variables**; coordinates are always kept. ``None``/empty returns
        all data variables. A requested name absent from the blob is a
        :class:`KeyError`. ``select`` is accepted for interface parity but record
        slicing is the Provider's job (it owns the cadence), so the whole file is
        returned here.
        """
        import xarray as xr  # lazy: only the netcdf path needs the heavy stack

        want = {str(v) for v in variables} if variables else None
        out_vars: Dict[str, NativeField] = {}
        out_coords: Dict[str, NativeField] = {}
        with xr.open_dataset(handle, decode_times=False, mask_and_scale=True) as ds:
            if want is not None:
                missing = [v for v in want if v not in ds.data_vars]
                if missing:
                    raise KeyError(
                        f"requested variables not in blob: {sorted(missing)}; "
                        f"present data variables: {sorted(map(str, ds.data_vars))}"
                    )
            for name, da in ds.data_vars.items():
                if want is not None and str(name) not in want:
                    continue
                out_vars[str(name)] = _field_from_dataarray(da)
            for name, da in ds.coords.items():
                out_coords[str(name)] = _field_from_dataarray(da)
        return NativeDataset(out_vars, out_coords)


# --------------------------------------------------------------------------- #
# CSV reader — a second format proving the registry seam (spec/conformance.md).
# --------------------------------------------------------------------------- #


def _parses_float(s: str) -> bool:
    try:
        float(s.strip())
        return True
    except ValueError:
        return False


class CSVReader:
    """The active ``csv`` reader — a non-NetCDF format behind the same registry.

    Columns named in ``numeric_columns`` parse to ``float64`` 1-D arrays keyed by
    the column (``file_variable``) name; every other column is returned as a
    ``list`` of ``str``. All fields carry the single dimension ``index``; there
    are no coordinates.

    ``numeric_columns`` is REQUIRED by the loader spec and is not inferred: the
    corpus ``location_id`` column is digit-only text (``"1"``/``"2"``) yet must
    stay a string, so "parses as a number" is not a safe signal. When it is
    ``None`` the reader falls back to best-effort inference (every value parses as
    a float), which the loader/``.esm`` node should override. Quoted fields with
    embedded delimiters are handled by :mod:`csv`; matches the Julia ``CSVReader``.
    """

    #: Registry name + format key(s) + extension sniff hints.
    NAME = "csv"
    FORMATS = ("csv",)
    EXTENSIONS = ("csv", "txt")

    def formats(self) -> List[str]:
        return list(self.FORMATS)

    def extensions(self) -> List[str]:
        return list(self.EXTENSIONS)

    def open(self, blob_path: Any) -> Any:
        return blob_path

    def read_native(
        self,
        handle: Any,
        variables: Optional[Sequence[str]] = None,
        select: Optional[Any] = None,
        *,
        numeric_columns: Optional[Sequence[str]] = None,
        delimiter: str = ",",
        header_row: int = 0,
        **_: Any,
    ) -> NativeDataset:
        """Decode a delimited-text blob into a :class:`NativeDataset` of points."""
        with open(handle, newline="") as fh:
            rows = [r for r in _csv.reader(fh, delimiter=delimiter) if r]
        if not rows:
            return NativeDataset()
        header = rows[header_row]
        body = rows[header_row + 1 :]
        want = {str(v) for v in variables} if variables else None
        if want is not None:
            missing = [v for v in want if v not in header]
            if missing:
                raise KeyError(
                    f"requested variables not in CSV: {sorted(missing)}; "
                    f"present columns: {header}"
                )
        numset = {str(c) for c in numeric_columns} if numeric_columns is not None else None

        out_vars: Dict[str, NativeField] = {}
        for j, col in enumerate(header):
            name = str(col)
            if want is not None and name not in want:
                continue
            vals = [r[j] for r in body]
            is_numeric = (name in numset) if numset is not None else all(map(_parses_float, vals))
            if is_numeric:
                data: Any = np.array([float(v) for v in vals], dtype="float64")
            else:
                data = [str(v) for v in vals]
            out_vars[name] = NativeField(data, ("index",), {})
        return NativeDataset(out_vars, {})


# --------------------------------------------------------------------------- #
# Registration (idempotent) — called from earthsciio/__init__.py on import.
# --------------------------------------------------------------------------- #


def register_format_readers(registry: Optional[Registry] = None) -> None:
    """Register the active ``netcdf`` + ``csv`` readers into the format registry.

    Idempotent: the underlying :meth:`Registry.register` is a no-op when the same
    factory is re-registered, so importing the package twice is safe. Orthogonal
    to the ``zarr`` stub — distinct names/keys never collide.
    """
    reg = registry if registry is not None else format_registry
    reg.register(
        NetCDFReader.NAME,
        NetCDFReader,
        keys=list(NetCDFReader.FORMATS),
        status="active",
        extensions=list(NetCDFReader.EXTENSIONS),
        notes="CF-decode via xarray; scale/offset + _FillValue->NaN; time axis raw.",
    )
    reg.register(
        CSVReader.NAME,
        CSVReader,
        keys=list(CSVReader.FORMATS),
        status="active",
        extensions=list(CSVReader.EXTENSIONS),
        notes="Delimited text; numeric_columns->float64, other columns->string.",
    )
