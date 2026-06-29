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
from typing import Any, Dict, List, Optional, Sequence, Tuple

import numpy as np

from .native import NativeDataset, NativeField
from .registry import Registry, format_registry

__all__ = ["NetCDFReader", "CSVReader", "GeoTIFFReader", "register_format_readers"]


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


def _netcdf_engine() -> Optional[str]:
    """The xarray engine used to decode a NetCDF blob.

    The shared cache stores content-addressed blobs *without* a file extension,
    so xarray's extension-based engine auto-detection fails with "cannot guess
    the engine". Pick the first installed engine explicitly: ``netcdf4`` and
    ``h5netcdf`` read NetCDF4/HDF5 (what the CDS/ERA5 transport downloads) as well
    as classic NetCDF3; ``scipy`` reads NetCDF3 only. ``None`` ⇒ fall back to
    xarray's guess (which raises a clear error when no engine is installed)."""
    import importlib.util

    for engine, module in (("netcdf4", "netCDF4"),
                           ("h5netcdf", "h5netcdf"),
                           ("scipy", "scipy")):
        if importlib.util.find_spec(module) is not None:
            return engine
    return None


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
        with xr.open_dataset(handle, decode_times=False, mask_and_scale=True,
                             engine=_netcdf_engine()) as ds:
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
# GeoTIFF reader — raster bands + a domain-derived lon/lat (or x/y) grid.
#
# The decode half for the ArcGIS ImageServer ``exportImage`` rasters the ESS
# loaders fetch (LANDFIRE fuel model, USGS 3DEP elevation) and any other GeoTIFF.
# Prefers GDAL via ``rasterio`` (``spec/registries.md`` §registries: "raster
# bands via GDAL"); falls back to pure-Python ``tifffile`` so a lean install
# without the GDAL stack still reads the geo-referencing tags directly. Both
# yield the SAME :class:`NativeDataset`, so the Provider/ESS see one shape.
# --------------------------------------------------------------------------- #


class _Raster:
    """A decoded raster: band arrays + cell-center axes + georef flags."""

    __slots__ = ("bands", "x_centers", "y_centers", "geographic", "nodata")

    def __init__(
        self,
        bands: List[np.ndarray],
        x_centers: np.ndarray,
        y_centers: np.ndarray,
        geographic: bool,
        nodata: Optional[float],
    ) -> None:
        self.bands = bands
        self.x_centers = x_centers
        self.y_centers = y_centers
        self.geographic = geographic
        self.nodata = nodata


def _geokey_value(geokeys: Optional[Sequence[int]], key_id: int) -> Optional[int]:
    """Read an *inline* GeoKey from a flat ``GeoKeyDirectoryTag`` (or ``None``).

    The directory is ``[version, keyRev, minorRev, nKeys, (KeyID, loc, count,
    value) * nKeys]``; only inline keys (``loc == 0``) carry their value in the
    4th slot. Used to detect ``GTModelTypeGeoKey`` (1024): 1=projected,
    2=geographic.
    """
    if not geokeys or len(geokeys) < 4:
        return None
    g = [int(v) for v in geokeys]
    n = g[3]
    for k in range(n):
        off = 4 + 4 * k
        if off + 3 >= len(g):
            break
        if g[off] == key_id and g[off + 1] == 0:
            return g[off + 3]
    return None


def _parse_nodata(tags: Dict[str, Any]) -> Optional[float]:
    """The GDAL_NODATA sentinel (an ASCII tag), parsed to ``float`` or ``None``."""
    raw = tags.get("GDAL_NoData", tags.get("GDAL_NODATA"))
    if raw is None:
        return None
    text = raw.decode() if isinstance(raw, (bytes, bytearray)) else str(raw)
    text = text.strip().strip("\x00").strip()
    if not text:
        return None
    try:
        return float(text)
    except ValueError:
        return None


def _read_with_rasterio(path: Any) -> _Raster:
    """Decode via GDAL (``rasterio``): bands, cell-center xy, CRS kind, nodata."""
    import rasterio  # lazy: only this path needs the GDAL stack

    with rasterio.open(path) as ds:
        bands = [np.asarray(ds.read(i + 1)) for i in range(ds.count)]
        height, width = ds.height, ds.width
        # ds.xy(row, col) is the CELL CENTER in the dataset CRS (handles any
        # north-up/affine transform); take the first row/col to get the axes.
        xs = np.array([ds.xy(0, c)[0] for c in range(width)], dtype="float64")
        ys = np.array([ds.xy(r, 0)[1] for r in range(height)], dtype="float64")
        nodata = None if ds.nodata is None else float(ds.nodata)
        crs = ds.crs
        geographic = bool(crs.is_geographic) if crs is not None else True
    return _Raster(bands, xs, ys, geographic, nodata)


def _read_with_tifffile(path: Any) -> _Raster:
    """Decode via pure-Python ``tifffile``, parsing the GeoTIFF georef tags.

    Reads ``ModelPixelScaleTag`` (cell size) + ``ModelTiepointTag`` (a raster→
    model anchor) to build north-up cell-center axes, ``GeoKeyDirectoryTag`` for
    the geographic/projected flag, and ``GDAL_NODATA`` for the fill sentinel.
    """
    import tifffile  # lazy

    with tifffile.TiffFile(path) as tif:
        page = tif.pages[0]
        arr = np.asarray(page.asarray())
        spp = int(getattr(page, "samplesperpixel", 1) or 1)
        if arr.ndim == 2:
            bands = [arr]
        elif arr.ndim == 3:
            # contiguous (H, W, S) vs planar (S, H, W); pick the axis of length spp.
            if arr.shape[-1] == spp:
                bands = [arr[..., i] for i in range(arr.shape[-1])]
            elif arr.shape[0] == spp:
                bands = [arr[i] for i in range(arr.shape[0])]
            else:
                bands = [arr[..., i] for i in range(arr.shape[-1])]
        else:
            raise ValueError(f"unsupported GeoTIFF array ndim={arr.ndim}")
        tags = {tg.name: tg.value for tg in page.tags.values()}
        scale = tags.get("ModelPixelScaleTag")
        tie = tags.get("ModelTiepointTag")
        if scale is None or tie is None:
            raise ValueError(
                "GeoTIFF lacks ModelPixelScaleTag/ModelTiepointTag; cannot derive "
                "a grid (install rasterio for non-tiepoint georeferencing)."
            )
        sx, sy = float(scale[0]), float(scale[1])
        i0, j0 = float(tie[0]), float(tie[1])
        x0, y0 = float(tie[3]), float(tie[4])
        height, width = bands[0].shape
        # GeoTIFF model space is y-up; raster rows increase downward (north-up).
        xs = x0 + (np.arange(width, dtype="float64") - i0 + 0.5) * sx
        ys = y0 - (np.arange(height, dtype="float64") - j0 + 0.5) * sy
        geographic = _geokey_value(tags.get("GeoKeyDirectoryTag"), 1024) != 1
        nodata = _parse_nodata(tags)
    return _Raster(bands, xs, ys, geographic, nodata)


def _open_raster(path: Any) -> _Raster:
    """Decode a GeoTIFF, preferring GDAL/``rasterio`` then ``tifffile``."""
    try:
        import rasterio  # noqa: F401
    except Exception:
        rasterio = None  # type: ignore[assignment]
    if rasterio is not None:
        return _read_with_rasterio(path)
    try:
        import tifffile  # noqa: F401
    except Exception as exc:  # pragma: no cover - exercised only with no backend
        raise ImportError(
            "the geotiff reader needs a raster backend: install rasterio "
            "(GDAL) or tifffile — e.g. `pip install earthsciio[geotiff]`."
        ) from exc
    return _read_with_tifffile(path)


class GeoTIFFReader:
    """The active ``geotiff`` reader — raster bands on a native lon/lat grid.

    Decodes a GeoTIFF blob into a :class:`NativeDataset`: one data variable per
    raster band keyed ``Band1``..``BandN`` (1-based, the GDAL convention; the
    LANDFIRE loader's ``file_variable: "Band1"`` matches), plus the cell-center
    coordinate fields. Geographic rasters (the ArcGIS ImageServer ``imageSR=4326``
    responses) get ``lon``/``lat`` axes; projected rasters get ``x``/``y``. Band
    arrays are ``float64`` with the ``GDAL_NODATA`` sentinel mapped to ``NaN``
    (``spec/conformance.md`` §3). Reader-only: no variable-name remap, no unit
    conversion, no reprojection — those stay in ESS/ESD.

    ``reader_kwargs``: pass ``band_names=[...]`` to rename the bands positionally
    (e.g. a single-band elevation raster → ``["elevation"]``).
    """

    #: Registry name + format key(s) + extension sniff hints.
    NAME = "geotiff"
    FORMATS = ("geotiff",)
    EXTENSIONS = ("tif", "tiff")

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
        band_names: Optional[Sequence[str]] = None,
        **_: Any,
    ) -> NativeDataset:
        """Decode ``handle`` into a :class:`NativeDataset` of raster bands + grid.

        ``variables`` (band names) restricts the returned data variables; ``None``
        returns all. A requested-but-absent band name is a :class:`KeyError`.
        ``select`` is accepted for interface parity (the Provider owns slicing).
        """
        raster = _open_raster(handle)
        nbands = len(raster.bands)
        if band_names is not None:
            names = [str(n) for n in band_names]
            if len(names) != nbands:
                raise ValueError(
                    f"band_names has {len(names)} entries but the GeoTIFF has "
                    f"{nbands} band(s)"
                )
        else:
            names = [f"Band{i + 1}" for i in range(nbands)]

        ydim, xdim = ("lat", "lon") if raster.geographic else ("y", "x")
        want = {str(v) for v in variables} if variables else None
        if want is not None:
            missing = [v for v in want if v not in names]
            if missing:
                raise KeyError(
                    f"requested bands not in GeoTIFF: {sorted(missing)}; "
                    f"present bands: {names}"
                )

        out_vars: Dict[str, NativeField] = {}
        for name, band in zip(names, raster.bands):
            if want is not None and name not in want:
                continue
            data = np.asarray(band).astype("float64", copy=True)
            if raster.nodata is not None and not np.isnan(raster.nodata):
                data[data == raster.nodata] = np.nan
            out_vars[name] = NativeField(data, (ydim, xdim), {})
        out_coords: Dict[str, NativeField] = {
            xdim: NativeField(np.asarray(raster.x_centers, dtype="float64"), (xdim,), {}),
            ydim: NativeField(np.asarray(raster.y_centers, dtype="float64"), (ydim,), {}),
        }
        return NativeDataset(out_vars, out_coords)


# --------------------------------------------------------------------------- #
# Registration (idempotent) — called from earthsciio/__init__.py on import.
# --------------------------------------------------------------------------- #


def register_format_readers(registry: Optional[Registry] = None) -> None:
    """Register the active ``netcdf`` + ``csv`` + ``geotiff`` readers.

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
    reg.register(
        GeoTIFFReader.NAME,
        GeoTIFFReader,
        keys=list(GeoTIFFReader.FORMATS),
        status="active",
        extensions=list(GeoTIFFReader.EXTENSIONS),
        notes="Raster bands via GDAL/rasterio (tifffile fallback); GDAL_NODATA->NaN.",
    )
