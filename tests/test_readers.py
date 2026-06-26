"""Active format readers decode the corpus to the oracle's native arrays.

The reader is the cross-language parity surface (``spec/conformance.md`` §3): the
``netcdf``/``csv`` readers must decode the committed corpus blobs to arrays
equal to ``expected`` in each ``conformance/corpus/cases/*.json`` — the same
verdict the Python oracle (``conformance/verify.py``) and the Julia/Rust readers
reach (conformance checks 3–4, run OFFLINE). These tests exercise that decode
through the *active registry readers* this bead adds.
"""

from __future__ import annotations

import json
import math
import pathlib

import numpy as np
import pytest

from earthsciio import Cache, CSVReader, NetCDFReader
from earthsciio.native import NativeDataset
from earthsciio.registry import format_registry

REPO_ROOT = pathlib.Path(__file__).resolve().parent.parent
CORPUS = REPO_ROOT / "conformance" / "corpus"

ERA5_URL = "https://data.earthsci.dev/era5/2018/11/20181108.nc"
OPENAQ_URL = (
    "https://openaq-data-archive.s3.amazonaws.com/records/openaq/"
    "locationid=1/2018-11-08.csv"
)

ATOL, RTOL = 1e-6, 1e-9


def _case(case_id: str) -> dict:
    return json.loads((CORPUS / "cases" / f"{case_id}.json").read_text())


def _flat(x):
    if isinstance(x, list):
        for e in x:
            yield from _flat(e)
    else:
        yield x


def _assert_numeric(field, expected_nested, label: str) -> None:
    exp = np.array(
        [math.nan if v is None else float(v) for v in _flat(expected_nested)],
        dtype="f8",
    )
    got = np.asarray(field.data, dtype="f8").reshape(-1)
    assert got.shape == exp.shape, f"{label}: shape {got.shape} != {exp.shape}"
    gn, en = np.isnan(got), np.isnan(exp)
    assert np.array_equal(gn, en), f"{label}: NaN/fill mask mismatch"
    assert np.allclose(got[~gn], exp[~en], atol=ATOL, rtol=RTOL), f"{label}: value mismatch"


def _assert_string(field, expected_nested, label: str) -> None:
    got = [str(v) for v in _flat(field.data)]
    exp = [None if v is None else str(v) for v in _flat(expected_nested)]
    assert got == [e for e in exp], f"{label}: string mismatch {got} != {exp}"


@pytest.fixture
def offline_cache():
    """A read-only offline cache rooted at the conformance corpus (no network)."""
    return Cache(root=CORPUS / "cache", offline=True, verify=True)


# --------------------------------------------------------------------------- #
# Registration: the active readers are wired into the format registry.
# --------------------------------------------------------------------------- #


def test_active_readers_registered():
    assert "netcdf" in format_registry and "csv" in format_registry
    assert format_registry.status("netcdf") == "active"
    assert format_registry.status("csv") == "active"
    # constructed by name through the seam the Provider uses
    assert isinstance(format_registry.create("netcdf"), NetCDFReader)
    assert isinstance(format_registry.create("csv"), CSVReader)
    assert NetCDFReader().formats() == ["netcdf"]
    assert "nc" in NetCDFReader().extensions()
    assert CSVReader().formats() == ["csv"]


# --------------------------------------------------------------------------- #
# NetCDF decode (CF scale/offset + fill->NaN + raw time), vs the corpus oracle.
# --------------------------------------------------------------------------- #


def test_netcdf_decodes_to_oracle_arrays(offline_cache):
    case = _case("era5-grid-sub-tile")
    blob = offline_cache.fetch(ERA5_URL)
    reader = NetCDFReader()
    nds = reader.read_native(reader.open(blob.path))

    assert isinstance(nds, NativeDataset)
    assert nds.variable_names() == ["sp", "t2m"]
    assert nds.coord_names() == ["latitude", "longitude", "time"]
    for name, spec in case["expected"]["variables"].items():
        _assert_numeric(nds[name], spec["data"], name)
        assert list(nds[name].dims) == spec["dims"]
        assert list(nds[name].shape) == spec["shape"]
        assert nds[name].data.dtype == np.float64  # packed + plain both -> float64
    for name, spec in case["expected"]["coords"].items():
        _assert_numeric(nds[name], spec["data"], f"coord {name}")


def test_netcdf_time_axis_is_raw_with_calendar(offline_cache):
    blob = offline_cache.fetch(ERA5_URL)
    reader = NetCDFReader()
    nds = reader.read_native(reader.open(blob.path))
    time = nds["time"]
    # raw integer time (decode_times=False) — int kept, NOT upcast to float
    assert np.issubdtype(time.data.dtype, np.integer)
    assert time.data.dtype == np.int32
    assert list(time.data) == [0, 1]
    # units + calendar carried for ESS; calendar decoding is NOT the reader's job
    assert time.attrs["units"] == "hours since 2018-11-08 00:00:00"
    assert time.attrs["calendar"] == "gregorian"


def test_netcdf_variable_selection_keeps_coords(offline_cache):
    blob = offline_cache.fetch(ERA5_URL)
    reader = NetCDFReader()
    nds = reader.read_native(reader.open(blob.path), ["t2m"])
    assert nds.variable_names() == ["t2m"]  # sp dropped
    assert nds.coord_names() == ["latitude", "longitude", "time"]  # coords always kept


def test_netcdf_absent_variable_raises(offline_cache):
    blob = offline_cache.fetch(ERA5_URL)
    reader = NetCDFReader()
    with pytest.raises(KeyError):
        reader.read_native(reader.open(blob.path), ["nope"])


# --------------------------------------------------------------------------- #
# CSV decode (numeric_columns -> float64, others -> string) — the 2nd format.
# --------------------------------------------------------------------------- #


def test_csv_decodes_to_oracle_arrays(offline_cache):
    case = _case("openaq-points-slice")
    blob = offline_cache.fetch(OPENAQ_URL)
    reader = CSVReader()
    nds = reader.read_native(
        reader.open(blob.path),
        numeric_columns=case["decode"]["numeric_columns"],
    )
    for name, spec in case["expected"]["variables"].items():
        assert list(nds[name].dims) == ["index"]
        if spec["dtype"] == "string":
            _assert_string(nds[name], spec["data"], name)
            assert isinstance(nds[name].data, list)
        else:
            _assert_numeric(nds[name], spec["data"], name)
            assert nds[name].data.dtype == np.float64


def test_csv_digit_text_stays_string_only_when_declared(offline_cache):
    blob = offline_cache.fetch(OPENAQ_URL)
    reader = CSVReader()
    # location_id is digit-only ("1","2") but NOT in numeric_columns => string
    nds = reader.read_native(reader.open(blob.path), numeric_columns=["value"])
    assert nds["location_id"].data == ["1", "1", "2", "2"]
    assert nds["value"].data.dtype == np.float64


def test_csv_variable_selection(offline_cache):
    blob = offline_cache.fetch(OPENAQ_URL)
    reader = CSVReader()
    nds = reader.read_native(
        reader.open(blob.path),
        ["value", "location_id"],
        numeric_columns=["value"],
    )
    assert nds.variable_names() == ["location_id", "value"]
