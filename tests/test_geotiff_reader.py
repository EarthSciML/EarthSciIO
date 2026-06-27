"""The active ``geotiff`` reader (gap G3) — raster bands → native lon/lat grid.

The reader is the decode half for the ArcGIS ImageServer ``exportImage`` rasters
the ESS loaders fetch (LANDFIRE fuel model, USGS 3DEP elevation). There is no
committed binary GeoTIFF in the conformance corpus yet (``spec/conformance.md``
§ "format-reserved"), so these tests AUTHOR a small georeferenced fixture with
``tifffile`` and read it back through the registry reader — exercising band
decode, the cell-center lon/lat axes derived from the GeoTIFF georef tags, the
``GDAL_NODATA``→``NaN`` mapping, band selection, and positional band renaming.
"""

from __future__ import annotations

import numpy as np
import pytest

# tifffile authors the fixture (and is also the reader's fallback backend); skip
# the whole module if it is absent (a rasterio-only env would commit a binary).
tifffile = pytest.importorskip("tifffile")

from earthsciio import GeoTIFFReader  # noqa: E402
from earthsciio.native import NativeDataset  # noqa: E402
from earthsciio.registry import format_registry  # noqa: E402

# A 4(lat) x 3(lon) raster, EPSG:4326, north-up. Top-left CORNER at (lon0, lat0),
# 0.5deg cells. Cell (1,1) holds the GDAL_NODATA sentinel.
LON0, LAT0, RES = -121.5, 40.0, 0.5
NODATA = -9999.0
EXP_LON = np.array([-121.25, -120.75, -120.25])  # LON0 + (col + 0.5) * RES
EXP_LAT = np.array([39.75, 39.25, 38.75, 38.25])  # LAT0 - (row + 0.5) * RES


def _write_geotiff(path, data, *, geographic=True, nodata=NODATA):
    geo_model_type = 2 if geographic else 1  # GTModelTypeGeoKey: 2=geographic
    epsg_key = 2048 if geographic else 3072  # Geographic vs Projected CS key
    geokeys = (1, 1, 0, 3, 1024, 0, 1, geo_model_type, 1025, 0, 1, 1, epsg_key, 0, 1, 4326)
    extratags = [
        (33550, "d", 3, (RES, RES, 0.0)),                 # ModelPixelScaleTag
        (33922, "d", 6, (0.0, 0.0, 0.0, LON0, LAT0, 0.0)),  # ModelTiepointTag
        (34735, "H", len(geokeys), geokeys),              # GeoKeyDirectoryTag
    ]
    if nodata is not None:
        sentinel = f"{nodata}\x00"
        extratags.append((42113, "s", len(sentinel), sentinel.encode()))  # GDAL_NODATA
    tifffile.imwrite(str(path), np.asarray(data), extratags=extratags)


@pytest.fixture
def landfire_like_tif(tmp_path):
    data = np.arange(12, dtype=np.float32).reshape(4, 3)
    data[1, 1] = NODATA
    path = tmp_path / "fuel_model.tif"
    _write_geotiff(path, data)
    return path


# --------------------------------------------------------------------------- #
# Registration: the active reader is wired into the format registry.
# --------------------------------------------------------------------------- #


def test_geotiff_reader_registered():
    assert "geotiff" in format_registry
    assert format_registry.status("geotiff") == "active"
    assert isinstance(format_registry.create("geotiff"), GeoTIFFReader)
    assert GeoTIFFReader().formats() == ["geotiff"]
    assert set(GeoTIFFReader().extensions()) == {"tif", "tiff"}


# --------------------------------------------------------------------------- #
# Decode: band -> Band1, georef tags -> lon/lat axes, GDAL_NODATA -> NaN.
# --------------------------------------------------------------------------- #


def test_geotiff_decodes_band_grid_and_nodata(landfire_like_tif):
    reader = GeoTIFFReader()
    nds = reader.read_native(reader.open(landfire_like_tif))

    assert isinstance(nds, NativeDataset)
    assert nds.variable_names() == ["Band1"]            # 1-based GDAL convention
    assert nds.coord_names() == ["lat", "lon"]          # geographic CRS -> lon/lat

    band = nds["Band1"]
    assert list(band.dims) == ["lat", "lon"]            # on-disk (row=lat, col=lon)
    assert list(band.shape) == [4, 3]
    assert band.data.dtype == np.float64                # numeric -> float64 (§3)
    assert np.isnan(band.data[1, 1])                    # GDAL_NODATA sentinel -> NaN
    # every other cell is its raw value (row-major arange), untouched
    expected = np.arange(12, dtype="float64").reshape(4, 3)
    finite = ~np.isnan(band.data)
    assert np.array_equal(band.data[finite], expected[finite])

    np.testing.assert_allclose(nds["lon"].data, EXP_LON)
    np.testing.assert_allclose(nds["lat"].data, EXP_LAT)
    assert list(nds["lon"].dims) == ["lon"]
    assert list(nds["lat"].dims) == ["lat"]


def test_geotiff_band_selection_and_missing(landfire_like_tif):
    reader = GeoTIFFReader()
    # selecting the present band keeps it (coords always returned)
    nds = reader.read_native(reader.open(landfire_like_tif), ["Band1"])
    assert nds.variable_names() == ["Band1"]
    assert nds.coord_names() == ["lat", "lon"]
    # an absent band name is a hard KeyError, like the netcdf reader
    with pytest.raises(KeyError):
        reader.read_native(reader.open(landfire_like_tif), ["elevation"])


def test_geotiff_band_names_rename(landfire_like_tif):
    """A single-band elevation raster can be renamed positionally (USGS 3DEP)."""
    reader = GeoTIFFReader()
    nds = reader.read_native(reader.open(landfire_like_tif), band_names=["elevation"])
    assert nds.variable_names() == ["elevation"]
    assert list(nds["elevation"].dims) == ["lat", "lon"]


def test_geotiff_projected_crs_gives_xy_axes(tmp_path):
    data = np.ones((2, 2), dtype=np.float32)
    path = tmp_path / "projected.tif"
    _write_geotiff(path, data, geographic=False, nodata=None)
    nds = GeoTIFFReader().read_native(str(path))
    assert nds.coord_names() == ["x", "y"]
    assert list(nds["Band1"].dims) == ["y", "x"]
