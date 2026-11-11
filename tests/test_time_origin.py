"""A provider that counts time from a time origin (spec/esm-sources.md §4),
driven by the shared ``time-origin`` scenario, plus the Python spellings."""

from __future__ import annotations

import datetime as dt
import json
import pathlib

import numpy as np
import pytest

from earthsciio import (
    Cache,
    EarthSciIOError,
    PrefetchGroup,
    TimeOriginInvalid,
    provider_from_esm,
)

CORPUS = pathlib.Path(__file__).resolve().parents[1] / "conformance" / "corpus"
SC = json.loads((CORPUS / "scenarios" / "time-origin.json").read_text())
UTC = dt.timezone.utc


def _build(case, records_per_sample=None, **kw):
    o = case["options"]
    return provider_from_esm(
        case["name"],
        case["entry"],
        Cache(root=str(CORPUS / "cache"), offline=True),
        variables=o["variables"],
        substitutions=o["substitutions"],
        records_per_sample=records_per_sample,
        window=kw.get("window", tuple(o["window"])),
        time_origin=kw.get("time_origin", o["time_origin"]),
    )


def _secs(times):
    """Times as numbers: a provider without an origin returns datetimes."""
    return [t.timestamp() if isinstance(t, dt.datetime) else t for t in times]


CASES = pytest.mark.parametrize("case", SC["cases"], ids=[c["id"] for c in SC["cases"]])
DATA_CASES = pytest.mark.parametrize(
    "case", [c for c in SC["cases"] if "error" not in c], ids=lambda c: c["id"]
)


@CASES
def test_case(case):
    """The schedule: refresh times, window records, group boundaries. Reads no
    data, so it runs without the netcdf stack."""
    if "error" in case:
        with pytest.raises(EarthSciIOError) as ei:
            _build(case)
        assert case["error"] in str(ei.value)
        return
    exp = case["expect"]
    p = _build(case)
    has_origin = case["options"]["time_origin"] is not None
    assert all(isinstance(t, float) == has_origin for t in p.refresh_times())
    assert _secs(p.refresh_times()) == exp["refresh_times"]
    for rps, want in exp["window_records"].items():
        assert _secs(_build(case, int(rps)).window_records()) == want
    with PrefetchGroup({"m": _build(case, 1)}, depth=2) as g:
        assert _secs(g.boundaries()) == [e["boundary"] for e in exp["epochs"]]


@pytest.mark.needs_format("netcdf")
@DATA_CASES
def test_case_data(case):
    """The data: each epoch's anchor, and the samples at model times."""
    exp = case["expect"]
    with PrefetchGroup({"m": _build(case, 1)}, depth=2) as g:
        g.start("forward")
        for k, e in enumerate(exp["epochs"]):
            assert _secs([g.take(k)["m"].anchor]) == [e["anchor"]]

    var = case["options"]["variables"][0]
    for smp in exp["samples"]:
        p = _build(case, smp["records_per_sample"])
        ds = p.sample_at(smp["t"])
        np.testing.assert_array_equal(np.asarray(ds[var].data), np.asarray(smp[var]))
        # refresh takes the same model time and returns the same arrays.
        np.testing.assert_array_equal(
            np.asarray(p.refresh(smp["t"])[var].data), np.asarray(smp[var])
        )
        if "time" in smp:
            tdim = case["entry"]["temporal"]["time_variable"]
            coord = ds.coords[tdim]
            np.testing.assert_array_equal(np.asarray(coord.data), smp["time"])
            assert coord.attrs["units"] == exp["time_units"]


NEI = next(c for c in SC["cases"] if c["id"] == "nei-monthly")
ORIGIN = dt.datetime(2016, 1, 1, tzinfo=UTC)


@pytest.mark.needs_format("netcdf")
def test_datetimes_still_read_as_instants():
    """With an origin, a datetime is still an instant: the same sample, and the
    same window, as the model seconds that name it."""
    p = _build(NEI, 1, window=(ORIGIN, dt.datetime(2016, 4, 1, tzinfo=UTC)))
    assert p.refresh_times() == [0.0, 2678400.0, 5184000.0]
    a = p.sample_at(dt.datetime(2016, 2, 29, tzinfo=UTC))
    b = p.sample_at(59 * 86400)
    np.testing.assert_array_equal(np.asarray(a["NOX"].data), np.asarray(b["NOX"].data))


def test_origin_spellings_agree():
    """A datetime (aware or naive UTC) and a string name the same origin."""
    for origin in (ORIGIN, dt.datetime(2016, 1, 1), "2016-01-01T00:00:00Z"):
        assert _build(NEI, time_origin=origin).refresh_times() == [0.0, 2678400.0, 5184000.0]


def test_origin_must_be_a_whole_second():
    with pytest.raises(TimeOriginInvalid, match="time_origin_invalid"):
        _build(NEI, time_origin=dt.datetime(2016, 1, 1, 0, 0, 0, 1000, tzinfo=UTC))
    with pytest.raises(TimeOriginInvalid, match="time_origin_invalid"):
        _build(NEI, time_origin="not a time")


@pytest.mark.needs_format("netcdf")
def test_without_origin_numbers_are_unix_seconds():
    """No origin: refresh_times stay datetimes, and a number is Unix seconds."""
    p = _build(NEI, 1, time_origin=None, window=(ORIGIN, dt.datetime(2016, 4, 1, tzinfo=UTC)))
    assert p.refresh_times()[0] == ORIGIN
    a = p.sample_at(dt.datetime(2016, 2, 29, tzinfo=UTC))
    b = p.sample_at(ORIGIN.timestamp() + 59 * 86400)
    np.testing.assert_array_equal(np.asarray(a["NOX"].data), np.asarray(b["NOX"].data))


def test_prefetch_window_in_model_seconds():
    p = _build(NEI)
    # A window ending on February's record also serves the refresh there, so
    # it warms February's file too (spec/esm-sources.md §2.6); one ending just
    # before it needs January's file only.
    assert len(p.prefetch((0, 2678400))) == 2
    assert len(p.prefetch((0, 2678399))) == 1
    with pytest.raises(EarthSciIOError, match="esm_window_uncovered"):
        p.prefetch((-1, 2678400))


def test_with_window_keeps_the_origin():
    pytest.importorskip("netCDF4")
    p = _build(NEI).with_window({"dims": {}})
    assert p.time_origin == ORIGIN
    assert p.refresh_times() == [0.0, 2678400.0, 5184000.0]


def test_group_refuses_members_with_different_origins():
    a = _build(NEI)
    b = _build(NEI, time_origin=None, window=(ORIGIN, dt.datetime(2016, 4, 1, tzinfo=UTC)))
    with pytest.raises(ValueError, match="time origins differ"):
        PrefetchGroup({"a": a, "b": b})


def test_origin_fraction_is_checked_past_microseconds():
    """Any nonzero fraction is refused, even one a datetime cannot hold: a
    string's digits as written, a pandas Timestamp's nanoseconds."""
    with pytest.raises(TimeOriginInvalid, match="time_origin_invalid"):
        _build(NEI, time_origin="2016-01-01T00:00:00.0000001Z")
    pd = pytest.importorskip("pandas")
    with pytest.raises(TimeOriginInvalid, match="time_origin_invalid"):
        _build(NEI, time_origin=pd.Timestamp("2016-01-01T00:00:00.000000001Z"))
    p = _build(NEI, time_origin=pd.Timestamp("2016-01-01T00:00:00Z"))
    assert p.time_origin == ORIGIN and type(p.time_origin) is dt.datetime


def test_units_pad_the_year():
    from earthsciio.provider import _origin_units

    assert _origin_units(dt.datetime(1, 1, 1, tzinfo=UTC)) == "seconds since 0001-01-01T00:00:00Z"
