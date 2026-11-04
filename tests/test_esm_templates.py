"""esm URL templates (spec/esm-sources.md §1), driven by the shared
``esm-url-templates`` scenario, plus mirror templates in a Provider."""

from __future__ import annotations

import datetime as dt
import json
import pathlib

import numpy as np
import pytest

from earthsciio import (
    Cache,
    DataSource,
    EsmTemplate,
    EsmTemplateError,
    Provider,
    SourceTemporal,
    esm_template_names,
    expand_esm_template,
)

SCENARIO = (
    pathlib.Path(__file__).resolve().parents[1]
    / "conformance"
    / "corpus"
    / "scenarios"
    / "esm-url-templates.json"
)
_SC = json.loads(SCENARIO.read_text())
CASES = _SC["cases"]
CONSTRUCTION = _SC["construction"]


def _anchor(case):
    s = case["anchor_s"]
    if s is None:
        return None
    epoch = dt.datetime(1970, 1, 1, tzinfo=dt.timezone.utc)
    try:
        return epoch + dt.timedelta(seconds=s)
    except OverflowError:
        pytest.skip("Python's datetime cannot hold this anchor")


@pytest.mark.parametrize("case", CASES, ids=[c["id"] for c in CASES])
def test_scenario_case(case):
    anchor = _anchor(case)
    if "expect" in case:
        assert (
            expand_esm_template(case["template"], anchor, case["substitutions"]) == case["expect"]
        )
    else:
        with pytest.raises(EsmTemplateError) as ei:
            expand_esm_template(case["template"], anchor, case["substitutions"])
        msg = str(ei.value)
        assert case["error"] in msg
        assert ei.value.code == case["error"]
        for m in case.get("mentions", []):
            assert m in msg
    if "names" in case:
        assert esm_template_names(case["template"]) == case["names"]


@pytest.mark.parametrize("case", CONSTRUCTION, ids=[c["id"] for c in CONSTRUCTION])
def test_scenario_construction(case):
    def build():
        return DataSource(
            name="c",
            format="netcdf",
            url=EsmTemplate(case["template"], case["substitutions"]),
            mirrors=case["mirrors"],
        )

    if "error" not in case:
        build()
        return
    with pytest.raises(EsmTemplateError) as ei:
        build()
    msg = str(ei.value)
    assert ei.value.code == case["error"]
    assert case["error"] in msg
    for m in case.get("mentions", []):
        assert m in msg


def test_template_refuses_unbound_at_construction():
    with pytest.raises(EsmTemplateError, match="esm_template_unbound"):
        EsmTemplate("https://h/{sector}_{date:%m}.ncf")


def _write_nc(path: pathlib.Path, value: float) -> None:
    import netCDF4

    with netCDF4.Dataset(path, "w") as ds:
        ds.createDimension("x", 2)
        v = ds.createVariable("v", "f8", ("x",))
        v[:] = [value, value + 1]


def test_mirror_templates_expand_at_the_primary_anchor(tmp_path):
    # The primary location does not exist; each month's file is found at its
    # MIRROR, expanded at that month's anchor with the same substitutions.
    mirror_dir = tmp_path / "mirror"
    mirror_dir.mkdir()
    _write_nc(mirror_dir / "emis_agr_01.nc", 1.0)
    _write_nc(mirror_dir / "emis_agr_02.nc", 10.0)
    loader = DataSource(
        name="nei",
        format="netcdf",
        url=EsmTemplate(
            f"file://{tmp_path}/missing/emis_{{sector}}_{{date:%m}}.nc", {"sector": "agr"}
        ),
        mirrors=[f"file://{mirror_dir}/emis_{{sector}}_{{date:%m}}.nc"],
        variables=["v"],
        temporal=SourceTemporal(
            start=dt.datetime(2016, 1, 1),
            frequency=dt.timedelta(days=31),
            file_period=dt.timedelta(days=31),
        ),
    )
    p = Provider(loader, Cache(root=str(tmp_path / "cache")))
    jan = p.sample_at(dt.datetime(2016, 1, 15))
    feb = p.sample_at(dt.datetime(2016, 2, 15))
    np.testing.assert_array_equal(jan["v"].data, [1.0, 2.0])
    np.testing.assert_array_equal(feb["v"].data, [10.0, 11.0])


def _daily_mirror(tmp_path):
    # The primary location does not exist; each day's file is at its mirror.
    mirror_dir = tmp_path / "mirror"
    mirror_dir.mkdir()
    _write_nc(mirror_dir / "d_20160101.nc", 1.0)
    _write_nc(mirror_dir / "d_20160102.nc", 10.0)
    return DataSource(
        name="m",
        format="netcdf",
        url=EsmTemplate(f"file://{tmp_path}/missing/{{date:%Y%m%d}}.nc"),
        mirrors=[f"file://{mirror_dir}/d_{{date:%Y%m%d}}.nc"],
        variables=["v"],
        temporal=SourceTemporal(
            start=dt.datetime(2016, 1, 1),
            frequency=dt.timedelta(days=1),
            file_period=dt.timedelta(days=1),
        ),
    )


def test_a_file_carries_the_mirrors_of_its_own_anchor(tmp_path):
    # The mirrors travel with the URL they were resolved for; nothing is looked
    # up by URL afterwards (a URL can cover several anchors).
    p = Provider(_daily_mirror(tmp_path), Cache(root=str(tmp_path / "cache")))
    for day in (2, 1):
        (ref,) = p._sample_urls(dt.datetime(2016, 1, day))
        assert ref.url.endswith(f"/missing/2016010{day}.nc")
        assert [m.rsplit("/", 1)[1] for m in ref.mirrors] == [f"d_2016010{day}.nc"]


def test_with_window_reads_an_esm_template_with_mirrors(tmp_path):
    p = Provider(_daily_mirror(tmp_path), Cache(root=str(tmp_path / "cache")))
    w = p.with_window({"dims": {"x": {"indices": [1]}}})
    np.testing.assert_array_equal(w.sample_at(dt.datetime(2016, 1, 2))["v"].data, [11.0])
