"""Active format readers decode the corpus to the oracle's native arrays.

The reader is the cross-language parity surface (``spec/conformance.md`` §3): the
``netcdf``/``csv`` readers must decode the committed corpus blobs to arrays
equal to ``expected`` in each ``conformance/corpus/cases/*.json`` — the same
verdict the Python oracle (``conformance/verify.py``) and the Julia/Rust readers
reach (conformance checks 3–4, run OFFLINE). These tests exercise that decode
through the *active registry readers* this bead adds.
"""

from __future__ import annotations

import csv as _csv
import io
import json
import math
import pathlib
import zipfile

import numpy as np
import pytest

from earthsciio import Cache, CSVReader, FF10Reader, NetCDFReader
from earthsciio.native import NativeDataset
from earthsciio.readers import FF10_POINT_COLUMNS, FF10_POINT_NUMERIC
from earthsciio.registry import format_registry

REPO_ROOT = pathlib.Path(__file__).resolve().parent.parent
CORPUS = REPO_ROOT / "conformance" / "corpus"

ERA5_URL = "https://data.earthsci.dev/era5/2018/11/20181108.nc"
OPENAQ_URL = (
    "https://openaq-data-archive.s3.amazonaws.com/records/openaq/"
    "locationid=1/2018-11-08.csv"
)
FF10_URL = "https://gaftp.epa.gov/air/emismod/2016/v1/2016fd/point/ff10_point.csv"

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


# --------------------------------------------------------------------------- #
# FF10 decode — the RAW long-format point table; a NEW reader (77-col schema,
# '#' header skip, RFC-4180 quotes, zip member) — reader-only (no pivot/convert).
# --------------------------------------------------------------------------- #


def _ff10_fixture_text() -> str:
    """A tiny FF10 point blob: a '#' header block + 3 data rows. Rows 1 & 2
    (NOX/SO2) share ONE stack (F001/U1/R1/P1 + stack params + lon/lat), differing
    only in POLID/ANN_VALUE. Row 1 has a quoted-comma FACILITY_NAME and a blank
    DESIGN_CAPACITY (numeric -> NaN)."""
    idx = {n: j for j, n in enumerate(FF10_POINT_COLUMNS)}

    def mkrow(**over):
        r = [""] * len(FF10_POINT_COLUMNS)
        for k, v in over.items():
            r[idx[k]] = v
        return r

    stack = dict(
        COUNTRY_CD="US", REGION_CD="01001", FACILITY_ID="F001", UNIT_ID="U1",
        REL_POINT_ID="R1", PROCESS_ID="P1", SCC="0030700101",
        FACILITY_NAME="Autauga Plant, Unit 1", STKHGT="100.0", STKTEMP="500.0",
        LONGITUDE="-86.51045", LATITUDE="32.43878", ZIPCODE="00000",
    )
    rows = [
        mkrow(**stack, POLID="NOX", ANN_VALUE="123.45"),
        mkrow(**stack, POLID="SO2", ANN_VALUE="67.89"),
        mkrow(COUNTRY_CD="US", REGION_CD="01001", FACILITY_ID="F002",
              POLID="PM25", ANN_VALUE="4.2", FACILITY_NAME="Plain Name"),
    ]
    sio = io.StringIO()
    sio.write("#FORMAT=FF10_POINT\n#COUNTRY US\n\n")  # header block + blank line
    w = _csv.writer(sio, lineterminator="\n")
    for r in rows:
        w.writerow(r)
    return sio.getvalue()


def test_ff10_registered():
    assert "ff10" in format_registry
    assert format_registry.status("ff10") == "active"
    assert isinstance(format_registry.create("ff10"), FF10Reader)
    assert FF10Reader().formats() == ["ff10"]
    assert "csv" in FF10Reader().extensions()
    assert len(FF10_POINT_COLUMNS) == 77
    assert len(FF10_POINT_NUMERIC) == 42


def test_ff10_decodes_to_oracle_arrays(offline_cache):
    case = json.loads((CORPUS / "cases" / "ff10-point-slice.json").read_text())
    blob = offline_cache.fetch(FF10_URL)
    reader = FF10Reader()
    nds = reader.read_native(
        reader.open(blob.path),
        numeric_columns=case["decode"]["numeric_columns"],
    )
    assert isinstance(nds, NativeDataset)
    assert len(nds.variables) == 77
    assert nds.coord_names() == []  # points table: no gridded axis
    for name, spec in case["expected"]["variables"].items():
        assert list(nds[name].dims) == ["index"]
        if spec["dtype"] == "string":
            _assert_string(nds[name], spec["data"], name)
            assert isinstance(nds[name].data, list)
        else:
            _assert_numeric(nds[name], spec["data"], name)
            assert nds[name].data.dtype == np.float64


def test_ff10_header_quote_empty(tmp_path):
    p = tmp_path / "ff10_point.csv"
    p.write_text(_ff10_fixture_text())
    nds = FF10Reader().read_native(str(p))

    assert len(nds.variables) == 77 and nds.coord_names() == []
    assert len(nds["POLID"].data) == 3  # '#' header + blank line skipped
    # numeric vs string typing
    assert nds["ANN_VALUE"].data.dtype == np.float64
    assert list(nds["ANN_VALUE"].data) == [123.45, 67.89, 4.2]
    assert nds["POLID"].data == ["NOX", "SO2", "PM25"]
    # leading-zero codes stay strings
    assert nds["REGION_CD"].data == ["01001", "01001", "01001"]
    assert nds["SCC"].data[0] == "0030700101"
    assert nds["ZIPCODE"].data[0] == "00000"
    # quoted comma preserved verbatim (quotes stripped)
    assert nds["FACILITY_NAME"].data[0] == "Autauga Plant, Unit 1"
    assert nds["FACILITY_NAME"].data[2] == "Plain Name"
    # blank numeric -> NaN; blank string -> ""
    assert math.isnan(nds["DESIGN_CAPACITY"].data[0])
    assert nds["TRIBAL_CODE"].data[0] == ""


def test_ff10_multi_pollutant_same_stack(tmp_path):
    p = tmp_path / "ff10_point.csv"
    p.write_text(_ff10_fixture_text())
    nds = FF10Reader().read_native(str(p))
    # rows 1 & 2 share the stack, differ only in POLID/ANN_VALUE (no pivot)
    assert nds["FACILITY_ID"].data[0] == nds["FACILITY_ID"].data[1] == "F001"
    assert nds["STKHGT"].data[0] == nds["STKHGT"].data[1] == 100.0
    assert nds["POLID"].data[:2] == ["NOX", "SO2"]
    assert list(nds["ANN_VALUE"].data[:2]) == [123.45, 67.89]
    # native units retained (feet / °F), not converted downstream
    assert nds["STKHGT"].data[0] == 100.0
    assert nds["STKTEMP"].data[0] == 500.0


def test_ff10_zip_member_equals_bare(tmp_path):
    text = _ff10_fixture_text()
    # write the bare CSV and decode it
    bare_path = tmp_path / "point.csv"
    bare_path.write_text(text)
    bare = FF10Reader().read_native(str(bare_path))
    # write a zip holding the CSV as member 'inv/point.csv'
    zpath = tmp_path / "2016fd_inputs_point.zip"
    with zipfile.ZipFile(zpath, "w", zipfile.ZIP_DEFLATED) as zf:
        zf.writestr("inv/point.csv", text)
    zipped = FF10Reader().read_native(str(zpath), member="inv/point.csv")
    assert list(zipped["ANN_VALUE"].data) == list(bare["ANN_VALUE"].data)
    assert zipped["POLID"].data == bare["POLID"].data
    assert zipped["FACILITY_NAME"].data == bare["FACILITY_NAME"].data
    # a missing member is a clear error
    with pytest.raises(KeyError):
        FF10Reader().read_native(str(zpath), member="nope.csv")


def test_ff10_absent_variable_raises(tmp_path):
    p = tmp_path / "ff10_point.csv"
    p.write_text(_ff10_fixture_text())
    with pytest.raises(KeyError):
        FF10Reader().read_native(str(p), ["NOT_A_COLUMN"])
