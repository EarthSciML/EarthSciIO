"""Building a provider from an esm data_sources entry (spec/esm-sources.md §3),
driven by the shared ``esm-sources`` scenario."""

from __future__ import annotations

import datetime as dt
import json
import pathlib

import numpy as np
import pytest

from earthsciio import Cache, EarthSciIOError, provider_from_esm

CORPUS = pathlib.Path(__file__).resolve().parents[1] / "conformance" / "corpus"
SC = json.loads((CORPUS / "scenarios" / "esm-sources.json").read_text())
UTC = dt.timezone.utc


def _t(s):
    return dt.datetime.fromtimestamp(s, tz=UTC)


def _build(case, records_per_sample=None):
    o = case["options"]
    window = (_t(o["window_s"][0]), _t(o["window_s"][1])) if "window_s" in o else None
    return provider_from_esm(
        case["name"],
        case["entry"],
        Cache(root=str(CORPUS / "cache"), offline=True),
        variables=o.get("variables", []),
        substitutions=o.get("substitutions"),
        url_override=o.get("url_override"),
        records_per_sample=records_per_sample,
        window=window,
    )


@pytest.mark.parametrize("case", SC["cases"], ids=[c["id"] for c in SC["cases"]])
def test_case(case):
    if "error" in case:
        with pytest.raises(EarthSciIOError) as ei:
            _build(case)
        msg = str(ei.value)
        assert case["error"] in msg
        for m in case["mentions"]:
            assert m in msg
        return
    exp = case["expect"]
    p = _build(case)
    loader = p.loader
    assert loader.format == exp["format"]
    assert loader.is_const == exp["const"]
    assert [t.timestamp() for t in p.refresh_times()] == exp["refresh_times_s"]
    temporal = loader.temporal
    if temporal is not None:
        assert temporal.time_dim == exp["time_dim"]
    for f in exp["files"]:
        if temporal is None:
            fa = dt.datetime(1970, 1, 1, tzinfo=UTC)
        else:
            a = temporal.record_anchor(_t(f["t_s"]))
            fa = temporal.file_anchor(a)
            assert fa.timestamp() == f["file_anchor_s"]
            assert temporal.record_in_file(a, fa) == f["record"]
        assert loader.resolve_url(fa) == f["url"]
        assert loader.resolve_mirrors(fa) == f["mirrors"]
    var = case["options"]["variables"][0]
    if exp.get("samples"):
        pytest.importorskip("xarray")
        pytest.importorskip("netCDF4")
    for smp in exp.get("samples", []):
        ds = _build(case, smp["records_per_sample"]).sample_at(_t(smp["t_s"]))
        np.testing.assert_array_equal(np.asarray(ds[var].data), np.asarray(smp[var]))
        if "time_s" in smp:
            np.testing.assert_array_equal(
                np.asarray(ds.coords[exp["time_dim"]].data), smp["time_s"]
            )
