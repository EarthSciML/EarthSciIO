"""The cadence Provider over the shared corpus — CONST/DISCRETE, OFFLINE.

Acceptance (``esio-9nb.3``): "A Provider over a fixture loader returns correct
native arrays for CONST (materialize once) and DISCRETE (refresh at each anchor);
refresh_times() matches the temporal cadence; unit-tested standalone (no campfire
dependency to pass)." Drives the full (a)+(b) pipeline offline: cache (shared
corpus) → format reader → native arrays, plus the cadence surface the solver
consumes (materialize/refresh/refresh_times/prefetch). Mirrors the peer
``julia/test/test_provider.jl`` and ``rust/tests/provider_cadence.rs``.
"""

from __future__ import annotations

import datetime as dt
import json
import math
import pathlib

import numpy as np
import pytest

from earthsciio import (
    BackendNotRegistered,
    Cache,
    DataLoader,
    LoaderTemporal,
    Provider,
)

REPO_ROOT = pathlib.Path(__file__).resolve().parent.parent
CORPUS = REPO_ROOT / "conformance" / "corpus"

ERA5_URL = "https://data.earthsci.dev/era5/2018/11/20181108.nc"
OPENAQ_URL = (
    "https://openaq-data-archive.s3.amazonaws.com/records/openaq/"
    "locationid=1/2018-11-08.csv"
)

START = dt.datetime(2018, 11, 8, tzinfo=dt.timezone.utc)
HOUR = dt.timedelta(hours=1)
DAY = dt.timedelta(days=1)


def _utc(h: int) -> dt.datetime:
    return START + h * HOUR


@pytest.fixture
def cache():
    """A read-only offline cache rooted at the conformance corpus (no network)."""
    return Cache(root=CORPUS / "cache", offline=True, verify=True)


def _era5_case() -> dict:
    return json.loads((CORPUS / "cases" / "era5-grid-sub-tile.json").read_text())


def _flat(x):
    if isinstance(x, list):
        for e in x:
            yield from _flat(e)
    else:
        yield x


def _match(field, expected_nested) -> bool:
    exp = np.array(
        [math.nan if v is None else float(v) for v in _flat(expected_nested)],
        dtype="f8",
    )
    got = np.asarray(field.data, dtype="f8").reshape(-1)
    if got.shape != exp.shape:
        return False
    gn, en = np.isnan(got), np.isnan(exp)
    return bool(np.array_equal(gn, en) and np.allclose(got[~gn], exp[~en], atol=1e-6, rtol=1e-9))


# --------------------------------------------------------------------------- #
# CONST grid: empty cadence, materialize once, native arrays match the oracle.
# --------------------------------------------------------------------------- #


def test_const_provider_materializes_oracle_arrays(cache):
    case = _era5_case()
    p = Provider(DataLoader("era5", "netcdf", ERA5_URL), cache)
    assert p.is_const
    assert p.refresh_times() == []  # CONST => never refreshes

    nds = p.materialize()
    assert nds.variable_names() == ["sp", "t2m"]
    assert nds.coord_names() == ["latitude", "longitude", "time"]
    # full native-array equality vs the oracle (checks 3–4 through the Provider)
    for group in ("variables", "coords"):
        for name, spec in case["expected"][group].items():
            assert _match(nds[name], spec["data"]), name
    # the raw time axis is undecoded with its calendar carried for ESS
    assert nds["time"].data.dtype == np.int32
    assert nds["time"].attrs["calendar"] == "gregorian"
    # coords property exposes the current grid
    assert set(p.coords) == {"latitude", "longitude", "time"}


def test_const_refresh_returns_constant_data(cache):
    p = Provider(DataLoader("era5", "netcdf", ERA5_URL), cache)
    a = p.refresh(_utc(0))  # CONST: refresh is the constant data (materialize once)
    b = p.refresh(_utc(99))
    assert np.array_equal(a["sp"].data, b["sp"].data)
    assert a["t2m"].shape == (2, 3, 3)


# --------------------------------------------------------------------------- #
# DISCRETE grid: refresh_times match cadence; per-anchor record slice.
# --------------------------------------------------------------------------- #


def _discrete_internal_axis() -> DataLoader:
    # one file holds the day's hourly records; cadence slices the internal axis
    temporal = LoaderTemporal(start=START, frequency=HOUR, file_period=2 * HOUR)
    return DataLoader("era5", "netcdf", ERA5_URL, temporal=temporal)


def test_discrete_refresh_times_match_cadence(cache):
    p = Provider(_discrete_internal_axis(), cache, window=(_utc(0), _utc(2)))
    assert not p.is_const
    assert p.refresh_times() == [_utc(0), _utc(1)]  # the hourly cadence


def test_discrete_refresh_slices_record_per_anchor(cache):
    p = Provider(_discrete_internal_axis(), cache, window=(_utc(0), _utc(2)))
    s0 = p.refresh(_utc(0))
    s1 = p.refresh(_utc(1))
    # the time record is sliced out: (time, lat, lon) -> (lat, lon)
    assert list(s0["t2m"].dims) == ["latitude", "longitude"]
    assert s0["t2m"].shape == (3, 3)
    assert s0["t2m"].data[0, 0] == pytest.approx(282.5)
    assert s1["t2m"].data[0, 0] == pytest.approx(282.6)  # a different record per tick
    assert math.isnan(s1["t2m"].data[2, 2])  # the masked cell survives the slice
    assert "time" not in s0  # the sliced dim's coordinate is dropped
    assert set(p.coords) == {"latitude", "longitude"}  # time dropped from coords too


def test_discrete_materialize_primes_first_anchor(cache):
    p = Provider(_discrete_internal_axis(), cache, window=(_utc(0), _utc(2)))
    primed = p.materialize()
    assert np.array_equal(primed["t2m"].data, p.refresh(_utc(0))["t2m"].data)


def test_discrete_between_anchors_uses_active_record(cache):
    p = Provider(_discrete_internal_axis(), cache, window=(_utc(0), _utc(2)))
    at_anchor = p.refresh(_utc(0))["t2m"].data.copy()
    between = p.refresh(START + dt.timedelta(minutes=30))  # snaps down to hour 0
    assert np.array_equal(between["t2m"].data, at_anchor)


def test_discrete_refresh_is_idempotent_within_interval(cache):
    p = Provider(_discrete_internal_axis(), cache, window=(_utc(0), _utc(2)))
    a = p.refresh(_utc(1))["sp"].data
    b = p.refresh(_utc(1))["sp"].data
    assert np.array_equal(a, b)


# --------------------------------------------------------------------------- #
# refresh_times bounds: window end, temporal.end, and the unbounded case.
# --------------------------------------------------------------------------- #


def test_refresh_times_bounded_by_temporal_end_without_window(cache):
    temporal = LoaderTemporal(start=START, frequency=HOUR, file_period=2 * HOUR, end=_utc(3))
    p = Provider(DataLoader("era5", "netcdf", ERA5_URL, temporal=temporal), cache)
    assert p.refresh_times() == [_utc(0), _utc(1), _utc(2)]


def test_refresh_times_empty_when_unbounded(cache):
    temporal = LoaderTemporal(start=START, frequency=HOUR, file_period=2 * HOUR)
    p = Provider(DataLoader("era5", "netcdf", ERA5_URL, temporal=temporal), cache)
    assert p.refresh_times() == []  # no window, no end => no enumerable schedule


def test_refresh_times_window_start_clamped_to_epoch(cache):
    # window starts mid-cadence; the first tstop is the aligned anchor >= start
    temporal = LoaderTemporal(start=START, frequency=HOUR, file_period=DAY)
    p = Provider(
        DataLoader("era5", "netcdf", ERA5_URL, temporal=temporal),
        cache,
        window=(START + dt.timedelta(minutes=30), _utc(3)),
    )
    assert p.refresh_times() == [_utc(1), _utc(2)]


# --------------------------------------------------------------------------- #
# Per-file URL resolution (strftime template + callable) and prefetch.
# --------------------------------------------------------------------------- #


def test_strftime_url_template_resolves_per_anchor():
    loader = DataLoader("era5", "netcdf", "https://data.earthsci.dev/era5/%Y/%m/%Y%m%d.nc")
    assert loader.resolve_url(START) == ERA5_URL


def test_prefetch_const_warms_single_file(cache):
    p = Provider(DataLoader("era5", "netcdf", ERA5_URL), cache)
    entries = p.prefetch()
    assert len(entries) == 1
    assert entries[0].status == "hit"  # offline corpus hit, no decode


def test_prefetch_strftime_window_hits_corpus(cache):
    temporal = LoaderTemporal(start=START, frequency=HOUR, file_period=DAY)
    loader = DataLoader("era5", "netcdf", "https://data.earthsci.dev/era5/%Y/%m/%Y%m%d.nc",
                        temporal=temporal)
    p = Provider(loader, cache, window=(START, START + DAY))
    entries = p.prefetch()
    assert len(entries) == 1 and entries[0].status == "hit"


def test_prefetch_enumerates_file_anchors_and_dedups(cache):
    seen = []

    def url_for(anchor):
        seen.append(anchor)
        return ERA5_URL  # every file anchor collapses to the one corpus blob

    temporal = LoaderTemporal(start=START, frequency=HOUR, file_period=HOUR)
    p = Provider(DataLoader("era5", "netcdf", url_for, temporal=temporal), cache,
                 window=(_utc(0), _utc(2)))
    entries = p.prefetch()
    assert seen == [_utc(0), _utc(1)]  # one anchor per file period across the window
    assert len(entries) == 1  # collapsed to a single unique fetch
    assert entries[0].status == "hit"


def test_prefetch_unbounded_raises(cache):
    temporal = LoaderTemporal(start=START, frequency=HOUR, file_period=HOUR)
    p = Provider(DataLoader("era5", "netcdf", ERA5_URL, temporal=temporal), cache)
    with pytest.raises(ValueError):
        p.prefetch()  # no window, no temporal.end


# --------------------------------------------------------------------------- #
# CSV points provider + variable selection (the 2nd format, end to end).
# --------------------------------------------------------------------------- #


def test_csv_provider_with_variable_selection(cache):
    loader = DataLoader(
        "openaq",
        "csv",
        OPENAQ_URL,
        variables=["value", "location_id"],
        reader_kwargs={"numeric_columns": ["latitude", "longitude", "value"]},
    )
    nds = Provider(loader, cache).materialize()
    assert nds.variable_names() == ["location_id", "value"]  # restricted
    assert list(nds["value"].data) == [152.3, 168.7, 98.1, 110.4]
    assert nds["value"].data.dtype == np.float64
    assert nds["location_id"].data == ["1", "1", "2", "2"]  # digit text stays string


# --------------------------------------------------------------------------- #
# Construction + use guards.
# --------------------------------------------------------------------------- #


def test_unknown_format_raises_at_construction(cache):
    with pytest.raises(BackendNotRegistered):
        Provider(DataLoader("x", "nonesuch", ERA5_URL), cache)


def test_loader_temporal_rejects_nonpositive_cadence():
    with pytest.raises(ValueError):
        LoaderTemporal(start=START, frequency=dt.timedelta(0), file_period=HOUR)
    with pytest.raises(ValueError):
        LoaderTemporal(start=START, frequency=HOUR, file_period=dt.timedelta(0))


def test_refresh_before_start_raises(cache):
    p = Provider(_discrete_internal_axis(), cache, window=(_utc(0), _utc(2)))
    with pytest.raises(ValueError):
        p.refresh(START - HOUR)


def test_refresh_record_out_of_range_raises(cache):
    # file_period=DAY claims 24 hourly records, but the fixture file holds 2
    temporal = LoaderTemporal(start=START, frequency=HOUR, file_period=DAY)
    p = Provider(DataLoader("era5", "netcdf", ERA5_URL, temporal=temporal), cache)
    with pytest.raises(IndexError):
        p.refresh(_utc(5))  # record 5 absent from the 2-record file


def test_absent_variable_raises(cache):
    p = Provider(DataLoader("era5", "netcdf", ERA5_URL, variables=["nope"]), cache)
    with pytest.raises(KeyError):
        p.materialize()
