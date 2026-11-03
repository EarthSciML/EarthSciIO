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
CASES = json.loads(SCENARIO.read_text())["cases"]


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
