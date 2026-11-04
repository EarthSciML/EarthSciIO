"""Finding a record by the file's own time axis (spec/esm-sources.md §2.5),
driven by the shared ``decoded-time-axis`` scenario."""

from __future__ import annotations

import datetime as dt
import json
import pathlib

import numpy as np
import pytest

from earthsciio import Cache, DataSource, EsmTemplate, Provider, SourceTemporal, decode_time_axis

# Decoding a time axis needs xarray, and reading the corpus files needs netCDF4.
pytest.importorskip("xarray")
pytest.importorskip("netCDF4")

CORPUS = pathlib.Path(__file__).resolve().parents[1] / "conformance" / "corpus"
SC = json.loads((CORPUS / "scenarios" / "decoded-time-axis.json").read_text())
UTC = dt.timezone.utc


def _t(s):
    return dt.datetime.fromtimestamp(s, tz=UTC)


@pytest.mark.parametrize("case", SC["decode"], ids=[c["id"] for c in SC["decode"]])
def test_decode(case):
    values = np.asarray([np.nan if v is None else v for v in case["values"]], dtype=case["dtype"])
    assert decode_time_axis(values, case["units"], case["calendar"]) == case["seconds"]


def _provider(src, rps):
    loader = DataSource(
        name="timeaxis",
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
    cache = Cache(root=str(CORPUS / "cache"), offline=True)
    return Provider(loader, cache, (_t(src["window_s"][0]), _t(src["window_s"][1])))


@pytest.mark.parametrize("name", sorted(SC["providers"]))
@pytest.mark.parametrize("rps", [1, 2])
def test_provider_samples(name, rps):
    src = SC["providers"][name]
    p = _provider(src, rps)
    for smp in src["samples"]:
        if smp["records_per_sample"] != rps:
            continue
        ds = p.sample_at(_t(smp["t_s"]))
        np.testing.assert_array_equal(np.asarray(ds["T"].data), np.asarray(smp["T"]), smp["note"])
        if rps == 2:
            np.testing.assert_array_equal(np.asarray(ds.coords["time"].data), smp["time_s"])
        # refresh (the buffered path) agrees with sample_at
        ds2 = p.refresh(_t(smp["t_s"]))
        np.testing.assert_array_equal(np.asarray(ds2["T"].data), np.asarray(smp["T"]))


@pytest.mark.parametrize("name", sorted(SC["providers"]))
@pytest.mark.parametrize("rps", [1, 2])
def test_provider_errors(name, rps):
    src = SC["providers"][name]
    p = _provider(src, rps)
    for err in src["errors"]:
        if err["records_per_sample"] != rps:
            continue
        with pytest.raises(IndexError):
            p.sample_at(_t(err["t_s"]))
