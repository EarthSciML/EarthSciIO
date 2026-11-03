"""The ``json`` points reader against the shared cross-language cases.

``conformance/corpus/scenarios/json-reader.json`` is read by the Julia
(``julia/test/test_json_reader.jl``) and Rust (``rust/tests/json_reader.rs``)
suites too, so passing here means decoding exactly as they do.
"""

from __future__ import annotations

import json
import math
from pathlib import Path

import numpy as np
import pytest

from earthsciio import DataSource, JSONReader, JSONReaderError, Provider
from earthsciio.errors import UnknownReaderOption
from earthsciio.registry import format_registry

ROOT = Path(__file__).resolve().parents[1]
CASES = json.loads(
    (ROOT / "conformance" / "corpus" / "scenarios" / "json-reader.json").read_text("utf-8")
)["cases"]


def _decode(tmp_path: Path, case: dict):
    blob = tmp_path / f"{case['id']}.json"
    blob.write_bytes(case["document"].encode("utf-8"))
    return JSONReader().read_native(
        str(blob), variables=case.get("variables"), **case.get("options", {})
    )


@pytest.mark.parametrize("case", CASES, ids=[c["id"] for c in CASES])
def test_shared_case(tmp_path: Path, case: dict) -> None:
    if "error" in case:
        with pytest.raises(JSONReaderError) as info:
            _decode(tmp_path, case)
        assert info.value.code == case["error"], str(info.value)
        assert str(info.value).startswith(case["error"] + ":")
        return
    nds = _decode(tmp_path, case)
    want = case["expected"]["fields"]
    assert set(nds.variables) == set(want)
    assert nds.coords == {}
    for name, spec in want.items():
        f = nds.variables[name]
        assert tuple(f.dims) == ("index",)
        if spec["dtype"] == "string":
            assert isinstance(f.data, list)
            assert f.data == spec["data"]
            continue
        arr = np.asarray(f.data)
        assert arr.dtype == np.dtype("float64" if spec["dtype"] == "float64" else "bool")
        assert arr.shape == (case["expected"]["length"],)
        for got, exp in zip(arr.tolist(), spec["data"]):
            if exp is None:
                assert math.isnan(got)
            else:
                # Bitwise, so a -0.0 against an expected 0.0 fails (the sign of a
                # zero is part of the contract: it is always +0.0).
                assert got == exp and math.copysign(1.0, got) == math.copysign(1.0, exp)


def test_registered_under_json() -> None:
    assert format_registry.status("json") == "active"
    assert isinstance(format_registry.create("json"), JSONReader)


def test_provider_reads_json_with_options(tmp_path: Path) -> None:
    from earthsciio import Cache

    case = next(c for c in CASES if c["id"] == "station-discovery")
    blob = tmp_path / "stations.json"
    blob.write_text(case["document"], "utf-8")
    loader = DataSource(
        name="stations",
        format="json",
        url=blob.as_uri(),
        variables=["coordinates.latitude", "name"],
        reader_kwargs={"records_path": ["results"]},
    )
    nds = Provider(loader, Cache(root=tmp_path / "cache")).materialize()
    assert set(nds.variables) == {"coordinates.latitude", "name"}
    assert nds.variables["name"].data == ["Site A", "Site B"]


def test_a_direct_select_or_unknown_option_is_json_option(tmp_path: Path) -> None:
    """``select`` is not a json option (supports_selection is false): called
    directly, the reader refuses it, and any other unknown keyword, with the
    json_option code Rust's JsonReader raises, rather than swallowing it."""
    blob = tmp_path / "x.json"
    blob.write_text('[{"a": 1}]', "utf-8")
    for kw in ({"select": {"axes": [{"indices": [0]}]}}, {"record_path": ["results"]}):
        with pytest.raises(JSONReaderError) as info:
            JSONReader().read_native(str(blob), None, **kw)
        assert info.value.code == "json_option"


def test_unknown_option_refused_at_construction(tmp_path: Path) -> None:
    from earthsciio import Cache

    loader = DataSource(
        name="stations",
        format="json",
        url=(tmp_path / "x.json").as_uri(),
        reader_kwargs={"record_path": ["results"]},
    )
    with pytest.raises(UnknownReaderOption):
        Provider(loader, Cache(root=tmp_path / "cache"))
