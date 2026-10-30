"""Named-dimension windows: ``select = {"dims": {...}}`` and ``DimWindow``.

``spec/prefetch.md`` §2.1–2.2 (and ``spec/registries.md`` §2.2, "Two spellings,
one vocabulary"): the named form spells the existing selectors by dimension NAME,
applies each to every decoded field and coordinate carrying that dimension
whatever its rank, leaves unnamed dimensions whole, and keeps every other rule of
the positional form — time refused, bounds checked against the file, never
dropping a dimension, an empty selection legal, order and repetition kept.

The corpus pins the decoded arrays (``named-window-mixed-rank``,
``window-wrap``, ``named-window-zarr``, through the conformance runner); these
tests pin the ERRORS a corpus case cannot express, the gate (a windowed read is
the full read sliced by name afterwards) on windows the corpus does not carry,
and the zarr reader fetching nothing outside the window.
"""

from __future__ import annotations

import json
import pathlib
import pickle

import numpy as np
import pytest

from earthsciio import Cache, DimWindow, NetCDFReader
from earthsciio.selection import _WindowUnion, _select_axes, _select_dims

REPO_ROOT = pathlib.Path(__file__).resolve().parent.parent
CORPUS = REPO_ROOT / "conformance" / "corpus"

MIXED_URL = "https://data.earthsci.dev/geosfp/mini/GEOSFP.20240101.A3mixed.nc"
STATIONS_URL = "https://data.earthsci.dev/openaq/stations/monitor-metadata.nc"


@pytest.fixture
def offline_cache():
    return Cache(root=CORPUS / "cache", offline=True, verify=True)


def _case(case_id: str) -> dict:
    return json.loads((CORPUS / "cases" / f"{case_id}.json").read_text())


def _indices(spec, n):
    """A wire selector's index list (test-side, independent of the reader)."""
    if spec == "all":
        return list(range(n))
    if "indices" in spec:
        return list(spec["indices"])
    s = spec["slice"]
    return list(range(s[0], s[1], s[2] if len(s) > 2 else 1))


def _slice_by_name(field, dims_spec):
    data = np.asarray(field.data)
    for axis, dim in enumerate(field.dims):
        if dim in dims_spec:
            data = np.take(data, _indices(dims_spec[dim], data.shape[axis]), axis=axis)
    return data


# --------------------------------------------------------------------------- #
# The vocabulary (no format stack needed).
# --------------------------------------------------------------------------- #


def test_a_select_carrying_both_forms_is_an_error():
    both = {"axes": ["all"], "dims": {"lat": "all"}}
    with pytest.raises(ValueError, match="both `axes` and `dims`"):
        _select_dims(both)
    with pytest.raises(ValueError, match="both `axes` and `dims`"):
        _select_axes(both)


def test_the_named_form_parses_in_order_and_the_empty_window_is_not_none():
    parsed = _select_dims(
        {"dims": {"lon": {"indices": [3, 0]}, "lat": {"slice": [1, 4, 2]}, "lev": "all"}}
    )
    assert list(parsed) == ["lon", "lat", "lev"]
    assert parsed["lon"] == ("indices", [3, 0])
    assert parsed["lat"] == ("slice", 1, 4, 2)
    assert parsed["lev"] == ("all",)
    assert _select_dims({"dims": {}}) == {}  # a window that selects nothing away
    assert _select_dims({"axes": ["all"]}) is None
    assert _select_axes({"dims": {"lat": "all"}}) is None  # not positional


def test_dimwindow_spells_the_wire_form():
    w = DimWindow(
        {
            "lat": (120, 218),
            "lon": [1151, 0, 1],
            "lev": "all",
            "x": (0, 9, 3),
            "y": {"indices": [2, 2]},
            "z": {"slice": [1, 5]},
        }
    )
    assert w.select == {
        "dims": {
            "lat": {"slice": [120, 218]},
            "lon": {"indices": [1151, 0, 1]},
            "lev": "all",
            "x": {"slice": [0, 9, 3]},
            "y": {"indices": [2, 2]},
            "z": {"slice": [1, 5]},
        }
    }
    # the wire form round-trips, and the window is immutable from outside
    assert DimWindow.from_select(w.select) == w
    w.select["dims"]["lat"]["slice"][0] = 0
    assert w.select["dims"]["lat"] == {"slice": [120, 218]}
    assert pickle.loads(pickle.dumps(w)) == w
    assert DimWindow().select == {"dims": {}}


@pytest.mark.parametrize(
    "bad",
    [
        {"lat": (1,)},
        {"lat": (1, 2, 3, 4)},
        {"lat": "some"},
        {"lat": 3},
        {"lat": {"range": [0, 1]}},
        {"lat": {"slice": [1]}},
    ],
)
def test_dimwindow_rejects_a_malformed_selector(bad):
    with pytest.raises(ValueError):
        DimWindow(bad)


def test_dimwindow_from_select_refuses_the_positional_form():
    with pytest.raises(ValueError, match="dims"):
        DimWindow.from_select({"axes": ["all"]})


def test_the_window_union_is_sorted_distinct_or_all():
    u = _WindowUnion(
        [
            DimWindow({"lat": (2, 4), "lon": [4, 0, 4]}),
            {"dims": {"lat": [3, 1], "lev": "all"}},
            DimWindow({"lat": [], "lon": [1]}),
        ]
    )
    assert u.select == {
        "dims": {
            "lat": {"indices": [1, 2, 3]},
            # the middle window leaves lon unnamed: it needs the whole axis
            "lon": "all",
            # named "all" (and unnamed elsewhere) stays in the union, so the read
            # still refuses a name no field carries
            "lev": "all",
        }
    }


# --------------------------------------------------------------------------- #
# NetCDF: the named path, against the mixed-rank corpus blob.
# --------------------------------------------------------------------------- #


def _read(cache, url, select=None, variables=None):
    path = cache.fetch(url).path
    return NetCDFReader().read_native(path, variables, select)


@pytest.mark.needs_format("netcdf")
def test_netcdf_named_window_is_the_full_read_sliced_by_name(offline_cache):
    """The gate, at every rank: 1-D AREA_LAT, 2-D PHIS, 3-D PS and the
    staggered U_EDGE, 4-D T and every coordinate — one window, by name."""
    full = _read(offline_cache, MIXED_URL)
    dims = {
        "lat": {"slice": [0, 4, 3]},
        "lon": {"indices": [4, 4, 0]},
        "lev": {"indices": [2, 0]},
        "lon_edge": {"slice": [5, 6]},
    }
    got = _read(offline_cache, MIXED_URL, {"dims": dims})
    assert got.variable_names() == full.variable_names()
    assert got.coord_names() == full.coord_names()
    for name in full.variable_names() + full.coord_names():
        assert got[name].dims == full[name].dims, name
        assert got[name].attrs == full[name].attrs, name
        assert np.asarray(got[name].data).dtype == np.asarray(full[name].data).dtype
        assert np.array_equal(got[name].data, _slice_by_name(full[name], dims), equal_nan=True), (
            name
        )
    # time is unnamed, so whole and raw
    assert np.array_equal(got["time"].data, full["time"].data)


@pytest.mark.needs_format("netcdf")
def test_netcdf_empty_named_window_is_the_whole_read(offline_cache):
    full = _read(offline_cache, MIXED_URL)
    got = _read(offline_cache, MIXED_URL, {"dims": {}})
    for name in full.variable_names() + full.coord_names():
        assert np.array_equal(got[name].data, full[name].data, equal_nan=True)


@pytest.mark.needs_format("netcdf")
def test_netcdf_named_window_keeps_an_empty_axis_and_a_length_one_axis(offline_cache):
    got = _read(
        offline_cache, MIXED_URL, {"dims": {"lat": {"indices": []}, "lev": {"slice": [1, 2]}}}
    )
    assert got["T"].shape == (2, 1, 0, 5)
    assert list(got["T"].dims) == ["time", "lev", "lat", "lon"]
    assert got["AREA_LAT"].shape == (0,)
    assert got["PHIS"].shape == (0, 5)
    assert got["lat"].shape == (0,)
    assert got["lev"].shape == (1,)
    assert got["U_EDGE"].shape == (2, 0, 6)


@pytest.mark.needs_format("netcdf")
def test_netcdf_named_window_refuses_both_forms(offline_cache):
    with pytest.raises(ValueError, match="both `axes` and `dims`"):
        _read(offline_cache, MIXED_URL, {"axes": ["all", "all", "all"], "dims": {"lat": "all"}})


@pytest.mark.needs_format("netcdf")
def test_netcdf_named_window_refuses_a_dimension_nothing_carries(offline_cache):
    with pytest.raises(
        ValueError,
        match=r"'latitude'.*present are \['lat', 'lev', "
        r"'lon', 'lon_edge', 'time'\]",
    ):
        _read(offline_cache, MIXED_URL, {"dims": {"latitude": {"slice": [0, 1]}}})
    # ...even when it selects "all": a window never silently means nothing
    with pytest.raises(ValueError, match="no field of this read carries"):
        _read(offline_cache, MIXED_URL, {"dims": {"level": "all"}})


@pytest.mark.needs_format("netcdf")
def test_netcdf_named_window_checks_against_the_projected_fields(offline_cache):
    """ "The fields of the read" are the PROJECTED data variables plus the
    coordinates: `site` has no coordinate variable, so once the projection
    leaves only the scalar `title`, nothing returned carries it."""
    full = _read(offline_cache, STATIONS_URL)
    got = _read(offline_cache, STATIONS_URL, {"dims": {"site": [2, 0]}}, ["value"])
    assert np.array_equal(got["value"].data, full["value"].data[[2, 0]], equal_nan=True)
    with pytest.raises(ValueError, match="'site'.*present are \\[\\]"):
        _read(offline_cache, STATIONS_URL, {"dims": {"site": [0]}}, ["title"])


@pytest.mark.needs_format("netcdf")
def test_netcdf_named_window_refuses_a_consumed_string_length(offline_cache):
    """`site_id(site, strlen)` decodes to one string per site: `strlen` is no
    decoded field's dimension, and applying it would truncate every string."""
    for sel in ({"strlen": {"slice": [0, 2]}}, {"strlen": "all"}, {"titlelen": [0]}):
        with pytest.raises(ValueError, match="string length of a char variable"):
            _read(offline_cache, STATIONS_URL, {"dims": sel})
    # the real axis next to it windows the strings, not their characters
    full = _read(offline_cache, STATIONS_URL)
    got = _read(offline_cache, STATIONS_URL, {"dims": {"site": [3, 1]}})
    assert got["site_id"].data.tolist() == np.asarray(full["site_id"].data)[[3, 1]].tolist()
    assert got["title"].data.tolist() == full["title"].data.tolist()


@pytest.mark.needs_format("netcdf")
def test_netcdf_named_window_refuses_a_time_subset_but_not_all(offline_cache):
    with pytest.raises(ValueError, match="time dimension 'time'"):
        _read(offline_cache, MIXED_URL, {"dims": {"time": {"indices": [0]}}})
    got = _read(offline_cache, MIXED_URL, {"dims": {"time": "all", "lat": [1]}})
    assert got["T"].shape == (2, 3, 1, 5)


@pytest.mark.needs_format("netcdf")
@pytest.mark.parametrize(
    "sel",
    [
        {"lat": {"slice": [2, 5]}},  # past the end: never a clamp
        {"lat": {"slice": [-1, 2]}},  # negative: never a wrap
        {"lon": {"indices": [0, 5]}},
        {"lon_edge": {"indices": [-1]}},
    ],
)
def test_netcdf_named_window_bounds_checks_against_the_file(offline_cache, sel):
    with pytest.raises(IndexError, match="out of range"):
        _read(offline_cache, MIXED_URL, {"dims": sel})


@pytest.mark.needs_format("netcdf")
def test_netcdf_named_window_composes_with_variables_and_records(offline_cache):
    full = _read(offline_cache, MIXED_URL)
    path = offline_cache.fetch(MIXED_URL).path
    got = NetCDFReader().read_native(
        path, ["PS"], {"dims": {"lon": [4, 1]}}, records={"dim": "time", "indices": [1]}
    )
    assert got.variable_names() == ["PS"]
    assert np.array_equal(got["PS"].data, full["PS"].data[[1]][:, :, [4, 1]])


# --------------------------------------------------------------------------- #
# Zarr: the named path fetches only the chunk objects the window intersects.
# --------------------------------------------------------------------------- #


class _Recording:
    """A cache wrapper recording every URL fetched through it."""

    def __init__(self, cache):
        self._cache = cache
        self.urls = []

    def fetch(self, url, **kw):
        self.urls.append(url)
        return self._cache.fetch(url, **kw)


@pytest.mark.needs_format("zarr")
def test_zarr_named_window_reads_the_corpus_case_and_no_poisoned_chunk(offline_cache):
    from earthsciio.backends.zarr import ZarrReader

    case = _case("named-window-zarr")
    rec = _Recording(offline_cache)
    got = ZarrReader().read_store(
        rec, case["resolved_url"], case["variables"], select=case["select"]
    )
    for name, spec in case["expected"]["variables"].items():
        assert list(got[name].dims) == spec["dims"]
        assert np.array_equal(got[name].data, np.array(spec["data"], dtype="f8"))
    poisoned = {o["url"] for o in case["objects"] if o.get("poisoned")}
    assert poisoned and not poisoned & set(rec.urls)


@pytest.mark.needs_format("zarr")
def test_zarr_a_window_that_overreaches_hits_the_poison(offline_cache):
    """The poisoned objects are what make the case able to FAIL an over-fetching
    reader: a wider window, or the whole read, decodes one and errors."""
    from earthsciio.backends.zarr import ZarrReader

    case = _case("named-window-zarr")
    reader = ZarrReader()
    with pytest.raises(Exception):
        reader.read_store(
            offline_cache,
            case["resolved_url"],
            case["variables"],
            select={"dims": {"lat": {"slice": [0, 4]}}},
        )
    with pytest.raises(Exception):
        reader.read_store(offline_cache, case["resolved_url"], case["variables"])


@pytest.mark.needs_format("zarr")
def test_zarr_named_window_errors_before_any_chunk_is_fetched(offline_cache):
    from earthsciio.backends.zarr import ZarrReader

    case = _case("named-window-zarr")
    reader = ZarrReader()
    for sel, exc, msg in (
        ({"lat": [2], "time": "all"}, ValueError, r"'time'.*present are \['lat', 'lev', 'lon'\]"),
        ({"lat": [2], "lon": [6]}, IndexError, "out of range"),
        ({"lev": {"slice": [2, 4]}}, IndexError, "out of range"),
    ):
        rec = _Recording(offline_cache)
        with pytest.raises(exc, match=msg):
            reader.read_store(rec, case["resolved_url"], case["variables"], select={"dims": sel})
        chunks = [u for u in rec.urls if not u.rsplit("/", 1)[1].startswith((".z", "zarr.json"))]
        assert chunks == [], sel
    with pytest.raises(ValueError, match="both"):
        reader.read_store(
            offline_cache, case["resolved_url"], case["variables"], select={"dims": {}, "axes": []}
        )


@pytest.mark.needs_format("zarr")
def test_zarr_named_window_applies_by_name_at_both_ranks(offline_cache):
    """One window over T3[lev, lat, lon] and PS2[lat, lon] — the order and the
    repetition of an `indices` list are kept, and an empty axis is legal."""
    from earthsciio.backends.zarr import ZarrReader

    case = _case("named-window-zarr")
    reader = ZarrReader()
    got = reader.read_store(
        offline_cache,
        case["resolved_url"],
        case["variables"],
        select={"dims": {"lat": [3, 2, 3], "lon": [4, 5, 4]}},
    )
    t3 = np.array(case["expected"]["variables"]["T3"]["data"])  # lat 2,3 x lon 5,3
    assert got["T3"].shape == (3, 3, 3)
    assert got["PS2"].shape == (3, 3)
    assert got["T3"].data[:, 1, 1].tolist() == t3[:, 0, 0].tolist()  # (lat 2, lon 5)
    empty = reader.read_store(
        offline_cache,
        case["resolved_url"],
        case["variables"],
        select={"dims": {"lat": [], "lon": [5]}},
    )
    assert empty["T3"].shape == (3, 0, 1)
    assert empty["PS2"].shape == (0, 1)
