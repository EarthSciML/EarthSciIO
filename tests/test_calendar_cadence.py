"""Calendar cadences (spec/esm-sources.md §2), driven by the shared
``calendar-cadence`` scenario."""

from __future__ import annotations

import datetime as dt
import json
import pathlib

import numpy as np
import pytest

from earthsciio import (
    Cache,
    CacheMiss,
    CalendarPeriod,
    CalendarPeriodError,
    DataSource,
    EsmTemplate,
    Provider,
    SourceTemporal,
    WindowNotCovered,
)

CORPUS = pathlib.Path(__file__).resolve().parents[1] / "conformance" / "corpus"
SC = json.loads((CORPUS / "scenarios" / "calendar-cadence.json").read_text())
UTC = dt.timezone.utc


def _t(s):
    return dt.datetime.fromtimestamp(s, tz=UTC)


@pytest.mark.parametrize("case", SC["periods"], ids=[c["iso"] for c in SC["periods"]])
def test_periods_parse(case):
    p = CalendarPeriod.parse(case["iso"])
    assert (p.months, p.seconds) == (case["months"], case["seconds"])


@pytest.mark.parametrize("iso", SC["bad_periods"])
def test_bad_periods_are_refused(iso):
    with pytest.raises(CalendarPeriodError, match="esm_period_invalid"):
        CalendarPeriod.parse(iso)


def _temporal(c, **kw):
    return SourceTemporal(
        start=_t(c["start_s"]), frequency=c["frequency"], file_period=c["file_period"], **kw
    )


@pytest.mark.parametrize("case", SC["cadences"], ids=[c["id"] for c in SC["cadences"]])
def test_cadence(case):
    temporal = _temporal(case, end=_t(case["end_s"]) if "end_s" in case else None)
    loader = DataSource(name="x", format="netcdf", url="file:///unused.nc", temporal=temporal)
    window = (_t(case["window_s"][0]), _t(case["window_s"][1]))
    p = Provider(loader, Cache(root="/nonexistent-unused", offline=True), window)
    assert [t.timestamp() for t in p.refresh_times()] == case["refresh_times_s"]
    for at in case["at"]:
        t = _t(at["t_s"])
        a = temporal.record_anchor(t)
        fa = temporal.file_anchor(a)
        assert a.timestamp() == at["record_anchor_s"], at
        assert fa.timestamp() == at["file_anchor_s"], at
        assert temporal.record_in_file(a, fa) == at["record"], at
        assert temporal.next_record_anchor(t).timestamp() == at["next_record_anchor_s"], at


@pytest.mark.parametrize("name", sorted(SC["providers"]))
def test_provider_over_the_corpus(name):
    pytest.importorskip("xarray")
    pytest.importorskip("netCDF4")
    src = SC["providers"][name]
    cache = Cache(root=str(CORPUS / "cache"), offline=True)
    var = src["variables"][0]
    window = (_t(src["window_s"][0]), _t(src["window_s"][1]))
    for rps in (1, 2):
        loader = DataSource(
            name=name,
            format=src["format"],
            url=EsmTemplate(src["url_template"], src["substitutions"]),
            variables=src["variables"],
            temporal=SourceTemporal(
                start=_t(src["start_s"]),
                end=_t(src["end_s"]),
                frequency=src["frequency"],
                file_period=src["file_period"],
                time_dim=src["time_dim"],
                records_per_sample=rps,
            ),
        )
        p = Provider(loader, cache, window)
        assert [t.timestamp() for t in p.refresh_times()] == src["refresh_times_s"]
        for sample in src["samples"]:
            if sample["records_per_sample"] != rps:
                continue
            ds = p.sample_at(_t(sample["t_s"]))
            np.testing.assert_array_equal(np.asarray(ds[var].data), np.asarray(sample[var]))
            if rps == 2:
                np.testing.assert_array_equal(
                    np.asarray(ds.coords[src["time_dim"]].data), sample["time_s"]
                )


@pytest.mark.parametrize("case", SC["windows"], ids=[c["id"] for c in SC["windows"]])
def test_window(case):
    """Which records serve a run window, and which windows are refused
    (spec/esm-sources.md §2.6)."""
    temporal = _temporal(
        case,
        end=_t(case["end_s"]) if "end_s" in case else None,
        records_per_sample=case["records_per_sample"],
    )
    loader = DataSource(name="x", format="netcdf", url="file:///unused.nc", temporal=temporal)
    window = (_t(case["window_s"][0]), _t(case["window_s"][1]))
    cache = Cache(root="/nonexistent-unused", offline=True)
    if "error" in case:
        with pytest.raises(WindowNotCovered, match=case["error"]) as err:
            Provider(loader, cache, window)
        assert err.value.code == case["error"]
        return
    p = Provider(loader, cache, window)
    assert [t.timestamp() for t in p.window_records()] == case["window_records_s"]
    assert [t.timestamp() for t in p.refresh_times()] == case["refresh_times_s"]


@pytest.mark.parametrize(
    "name", sorted(n for n in SC["providers"] if "prefetch" in SC["providers"][n])
)
def test_prefetch_reaches_the_bracket_successor(name):
    """A two-record bracket over the window's last interval reads the record
    after the window, so prefetch warms that record's file too (§2.6)."""
    src = SC["providers"][name]
    cache = Cache(root=str(CORPUS / "cache"), offline=True)
    for case in src["prefetch"]:
        loader = DataSource(
            name=name,
            format=src["format"],
            url=EsmTemplate(src["url_template"], src["substitutions"]),
            variables=src["variables"],
            temporal=SourceTemporal(
                start=_t(src["start_s"]),
                end=None if case["open_ended"] else _t(src["end_s"]),
                frequency=src["frequency"],
                file_period=src["file_period"],
                time_dim=src["time_dim"],
                records_per_sample=case["records_per_sample"],
            ),
        )
        window = (_t(case["window_s"][0]), _t(case["window_s"][1]))
        p = Provider(loader, cache, window)
        if case["missing_url"] is None:
            p.prefetch()
        else:
            with pytest.raises(CacheMiss) as err:
                p.prefetch()
            assert err.value.resolved_url == case["missing_url"], case
