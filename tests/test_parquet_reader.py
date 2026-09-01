"""The active ``parquet`` reader — an Apache Parquet file as a flat table.

Mirrors ``rust/tests/parquet_reader.rs`` case for case, because the decode
CONTRACT (``spec/conformance.md`` §3, "Parquet decode notes") is language-neutral
and pyarrow is the same Apache Arrow project as the Rust track's ``parquet``
crate — the Arrow type mapping is literally the same enum on both sides, so any
divergence here is a bug rather than a dialect.

Fixtures are written in the test with ``pq.write_table`` (no committed blobs):
the point is the *mapping*, and a blob would only hide which Arrow type produced
which native array. The one exception is the MOVES-snapshot smoke test, which
reads a real EPA table when the sibling checkout is present and skips otherwise.
"""

from __future__ import annotations

import datetime as dt
import decimal
import pathlib

import numpy as np
import pytest

# pyarrow authors the fixtures (and is the reader's backend); a base install
# without the `parquet` extra simply skips this module.
pa = pytest.importorskip("pyarrow")
pq = pytest.importorskip("pyarrow.parquet")

from earthsciio import ParquetReader  # noqa: E402
from earthsciio.native import NativeDataset  # noqa: E402
from earthsciio.provider import DataSource, check_reader_options  # noqa: E402
from earthsciio.registry import format_registry  # noqa: E402


def write(tmp_path, table, name="t.parquet", **kw):
    """Write ``table`` as a Parquet blob and return its path."""
    path = tmp_path / name
    pq.write_table(table, path, **kw)
    return path


def read(path, variables=None, **kw) -> NativeDataset:
    """Decode ``path`` through the reader the registry serves."""
    reader = ParquetReader()
    return reader.read_native(reader.open(path), variables, **kw)


# --------------------------------------------------------------------------- #
# The type mapping (spec/conformance.md §3)
# --------------------------------------------------------------------------- #


def test_every_supported_arrow_type_maps_onto_the_native_contract(tmp_path):
    """The dtype is a total function of the Arrow type — the §3 table, in full.

    The narrow/wide integer split is the NetCDF reader's verbatim, so a MOVES
    ``int32`` ID column and a CF ``int32`` time axis cannot drift apart; temporal
    columns ride as their RAW stored integer with the unit and timezone
    unapplied; a ``Dictionary`` is expanded to one value per row.
    """
    table = pa.table(
        {
            "b": pa.array([True, False], type=pa.bool_()),
            "i8": pa.array([-1, 2], type=pa.int8()),
            "i16": pa.array([-3, 4], type=pa.int16()),
            "i32": pa.array([-5, 6], type=pa.int32()),
            "u8": pa.array([7, 8], type=pa.uint8()),
            "u16": pa.array([9, 10], type=pa.uint16()),
            "i64": pa.array([-11, 12], type=pa.int64()),
            "u32": pa.array([13, 14], type=pa.uint32()),
            "u64": pa.array([15, 16], type=pa.uint64()),
            "f16": pa.array([np.float16(1.5), np.float16(-2.25)], type=pa.float16()),
            "f32": pa.array([1.25, -2.5], type=pa.float32()),
            "f64": pa.array([1e300, -0.125], type=pa.float64()),
            "dec": pa.array(
                [decimal.Decimal("261.000000000000"), decimal.Decimal("-0.500000000000")],
                type=pa.decimal128(20, 12),
            ),
            "s": pa.array(["01234", "x"], type=pa.string()),
            "ls": pa.array(["a", "b"], type=pa.large_string()),
            "sv": pa.array(["c", "d"], type=pa.string_view()),
            "d32": pa.array([dt.date(1970, 1, 3), dt.date(1970, 1, 1)], type=pa.date32()),
            "t32": pa.array([1_000, 2_000], type=pa.time32("ms")),
            "t64": pa.array([1_234_567, 0], type=pa.time64("us")),
            "ts": pa.array([1_600_000_000, 0], type=pa.timestamp("s", tz="America/Chicago")),
            "dur": pa.array([5, -5], type=pa.duration("ms")),
            "cat": pa.array(["red", "blue"]).dictionary_encode(),
            "nul": pa.nulls(2),
        }
    )
    ds = read(write(tmp_path, table))

    # Every field is rank-1 over `index`, and a table has no coordinates.
    assert ds.coords == {}
    for name, f in ds.variables.items():
        assert f.dims == ("index",), name
        assert f.shape == (2,), name

    def dtype(name):
        d = ds[name].dtype
        return "string" if d is None else str(d)

    assert dtype("b") == "bool"
    for name in ("i8", "i16", "i32", "u8", "u16", "d32", "t32"):
        assert dtype(name) == "int32", name
    for name in ("i64", "u32", "u64", "t64", "ts", "dur"):
        assert dtype(name) == "int64", name
    for name in ("f16", "f32", "f64", "dec", "nul"):
        assert dtype(name) == "float64", name
    for name in ("s", "ls", "sv", "cat"):
        assert dtype(name) == "string", name

    assert list(ds["b"].data) == [True, False]
    assert list(ds["i8"].data) == [-1, 2]
    assert list(ds["u64"].data) == [15, 16]
    assert list(ds["f16"].data) == [1.5, -2.25]
    assert list(ds["f64"].data) == [1e300, -0.125]
    # Decimal: the unscaled integer ÷ 10^scale, in double.
    assert list(ds["dec"].data) == [261.0, -0.5]
    # Utf8 → str and NOTHING else: a leading-zero code stays text.
    assert ds["s"].data == ["01234", "x"]
    assert ds["sv"].data == ["c", "d"]
    # A categorical is EXPANDED: one value per row, key encoding gone.
    assert ds["cat"].data == ["red", "blue"]
    # Temporal: the raw stored integer, undecoded. `d32` is days, `t32` is ms,
    # `ts` is seconds in UTC — the timezone is neither applied nor reported.
    assert list(ds["d32"].data) == [2, 0]
    assert list(ds["t32"].data) == [1_000, 2_000]
    assert list(ds["t64"].data) == [1_234_567, 0]
    # `ts` was authored in seconds and Parquet stores it as milliseconds; the
    # reader hands back the RAW STORED integer, so the coercion is visible rather
    # than normalized away. A document needing the unit must state it itself.
    assert list(ds["ts"].data) == [1_600_000_000_000, 0]
    assert list(ds["dur"].data) == [5, -5]
    assert ds["ts"].attrs == {}, "no unit or timezone is reported"
    # A Null column is float64, every cell NaN.
    assert np.isnan(ds["nul"].data).all()


def test_the_mapping_is_a_total_function_of_the_arrow_type():
    """The dtype table without a file in the way — the peer of the Rust module's
    own unit tests. It also reaches the types Parquet cannot store verbatim
    (``Date64``, which a write folds to ``Date32``, Parquet's DATE being days).
    """
    from earthsciio.readers import _pq_cells, _pq_target_dtype

    for typ in (pa.int8(), pa.int16(), pa.int32(), pa.uint8(), pa.uint16()):
        assert _pq_target_dtype(typ, False) == "int32", typ
    for typ in (pa.int64(), pa.uint32(), pa.uint64()):
        assert _pq_target_dtype(typ, False) == "int64", typ
    assert _pq_target_dtype(pa.date64(), False) == "int64"
    assert _pq_target_dtype(pa.timestamp("ns", tz="UTC"), False) == "int64"
    assert _pq_target_dtype(pa.null(), False) == "float64"
    # A categorical reads as its VALUE type.
    assert _pq_target_dtype(pa.dictionary(pa.int32(), pa.string()), False) == "string"
    assert _pq_target_dtype(pa.dictionary(pa.int8(), pa.int64()), False) == "int64"
    # `float_columns` is a statement about the source, so it wins over any type
    # that has a numeric reading — and over nothing that does not.
    assert _pq_target_dtype(pa.string(), True) == "float64"
    assert _pq_target_dtype(pa.int64(), True) == "float64"
    assert _pq_target_dtype(pa.string(), False) == "string"
    for typ in (pa.list_(pa.int32()), pa.binary(), pa.large_binary(),
                pa.struct([("a", pa.int32())]), pa.month_day_nano_interval()):
        assert _pq_target_dtype(typ, False) is None, typ
        assert _pq_target_dtype(typ, True) is None, typ

    # And the raw-integer decode of the temporal types, undecoded.
    d64 = pa.chunked_array([pa.array([86_400_000, None], type=pa.date64())])
    assert _pq_cells(d64)[0] == [86_400_000, None]
    tsz = pa.chunked_array(
        [pa.array([1_600_000_000_000], type=pa.timestamp("ms", tz="America/Chicago"))]
    )
    assert _pq_cells(tsz)[0] == [1_600_000_000_000], "the timezone is not applied"


def test_a_uint64_too_large_for_int64_is_an_error(tmp_path):
    """Refuse rather than wrap: a wraparound would be a negative ID."""
    path = write(tmp_path, pa.table({"u": pa.array([1, 2 ** 63], type=pa.uint64())}))
    with pytest.raises(ValueError) as err:
        read(path)
    assert "'u'" in str(err.value) and "row 1" in str(err.value)


def test_an_int32_column_that_cannot_hold_its_sentinel_is_an_error(tmp_path):
    """A declared ``null_int`` still has to fit the column's native width."""
    path = write(tmp_path, pa.table({"i": pa.array([1, None], type=pa.int32())}))
    with pytest.raises(ValueError) as err:
        read(path, null_int=2 ** 40)
    assert "int32" in str(err.value)


def test_a_nested_column_is_skipped_when_unrequested_and_refused_when_named(tmp_path):
    """Nested and binary columns have no rank-1 reading.

    Naming one in ``variables`` is an error (the document named an array it would
    not get); unrequested, it is simply not a native field — the way the NetCDF
    reader skips its non-numeric variables.
    """
    table = pa.table(
        {
            "id": pa.array([1, 2], type=pa.int64()),
            "lst": pa.array([[1], [2, 3]], type=pa.list_(pa.int32())),
            "st": pa.array([{"a": 1}, {"a": 2}], type=pa.struct([("a", pa.int32())])),
            "blob": pa.array([b"x", b"y"], type=pa.binary()),
        }
    )
    path = write(tmp_path, table)

    ds = read(path)
    assert sorted(ds.variables) == ["id"], "only the flat column is a field"

    for name in ("lst", "st", "blob"):
        with pytest.raises(ValueError) as err:
            read(path, [name])
        assert "rank-1" in str(err.value) and repr(name) in str(err.value)


# --------------------------------------------------------------------------- #
# The null policy
# --------------------------------------------------------------------------- #


def test_a_null_float_is_nan_and_no_sentinel_survives(tmp_path):
    """A null float folds to NaN, exactly as a CF ``_FillValue`` cell does."""
    table = pa.table(
        {
            "f": pa.array([1.0, None, 3.0], type=pa.float64()),
            "dec": pa.array(
                [decimal.Decimal("1.50"), None, decimal.Decimal("-2.25")],
                type=pa.decimal128(9, 2),
            ),
        }
    )
    ds = read(write(tmp_path, table))
    assert np.isnan(ds["f"].data[1]) and list(ds["f"].data[[0, 2]]) == [1.0, 3.0]
    assert np.isnan(ds["dec"].data[1]) and list(ds["dec"].data[[0, 2]]) == [1.5, -2.25]
    # No sentinel survives a NaN fold.
    assert ds["f"].attrs == {} and ds["dec"].attrs == {}


@pytest.mark.parametrize(
    "column, arrow_type, kind",
    [
        ("i", pa.int64(), "integer"),
        ("s", pa.string(), "string"),
        ("b", pa.bool_(), "boolean"),
    ],
)
def test_a_null_in_a_type_with_no_missing_value_is_refused_by_default(
    tmp_path, column, arrow_type, kind
):
    """Those types have no NaN, so a default would be a real value standing in
    for a missing one — the failure mode that surfaces later as wrong numbers.
    """
    path = write(
        tmp_path,
        pa.table({column: pa.array([None, None], type=arrow_type)}),
        name=f"{column}.parquet",
    )
    with pytest.raises(ValueError) as err:
        read(path)
    msg = str(err.value)
    assert repr(column) in msg and "row 0" in msg
    if kind == "boolean":
        assert "float_columns" in msg, "the way out is named"
    else:
        assert ("null_int" if kind == "integer" else "null_string") in msg


def test_a_declared_int_sentinel_fills_and_is_reported(tmp_path):
    """``null_int`` substitutes AND is reported back, like a CF integer fill.

    An integer sentinel cannot be NaN, so it survives into the array as a real
    value; the document that chose it needs it named back.
    """
    table = pa.table(
        {
            "i": pa.array([1, None, 3], type=pa.int64()),
            "n": pa.array([4, None, 4], type=pa.int32()),
            "f": pa.array([1.0, None, 3.0], type=pa.float64()),
        }
    )
    ds = read(write(tmp_path, table), null_int=-999)
    assert list(ds["i"].data) == [1, -999, 3]
    assert ds["i"].attrs["fill_value"] == -999
    assert list(ds["n"].data) == [4, -999, 4]
    assert ds["n"].attrs["fill_value"] == -999
    # A float column is untouched by the integer gate: NaN, no sentinel.
    assert np.isnan(ds["f"].data[1])
    assert "fill_value" not in ds["f"].attrs


def test_a_declared_string_stands_in_for_a_null_text_cell(tmp_path):
    """``null_string`` is indistinguishable from a real cell holding that text —
    which is exactly why the document has to choose it."""
    path = write(tmp_path, pa.table({"s": pa.array(["a", None, ""], type=pa.string())}))
    ds = read(path, null_string="MISSING")
    assert ds["s"].data == ["a", "MISSING", ""]


def test_a_null_boolean_has_no_sentinel_option(tmp_path):
    """A boolean has no sentinel option: neither gate opens for it, and the
    error names the only honest way out."""
    path = write(tmp_path, pa.table({"b": pa.array([True, None], type=pa.bool_())}))
    for kwargs in ({"null_int": 0}, {"null_string": "?"}):
        with pytest.raises(ValueError) as err:
            read(path, **kwargs)
        assert "float_columns" in str(err.value)
    # And `float_columns` on a column that really is boolean is itself refused:
    # a bool is not a number, so the option cannot quietly invent one.
    with pytest.raises(ValueError) as err:
        read(path, float_columns=["b"])
    assert "boolean cell cannot be read as float64" in str(err.value)


# --------------------------------------------------------------------------- #
# float_columns
# --------------------------------------------------------------------------- #


def test_float_columns_turns_a_nullable_integer_into_a_nan_carrying_float(tmp_path):
    """An integer column that is really a measurement: missing → NaN, not a
    sentinel. And no pandas round-trip: the untouched column keeps int64."""
    table = pa.table(
        {
            "meas": pa.array([1, None, 3], type=pa.int64()),
            "id": pa.array([10, 20, 30], type=pa.int64()),
        }
    )
    ds = read(write(tmp_path, table), float_columns=["meas"])
    assert str(ds["meas"].dtype) == "float64"
    assert np.isnan(ds["meas"].data[1]) and list(ds["meas"].data[[0, 2]]) == [1.0, 3.0]
    assert "fill_value" not in ds["meas"].attrs
    # THE pandas TRAP: a nullable int64 must NOT be promoted to float64 by the
    # decode of a *neighbouring* column.
    assert str(ds["id"].dtype) == "int64"
    assert list(ds["id"].data) == [10, 20, 30]


def test_float_columns_parses_decimal_text_and_refuses_the_unparseable(tmp_path):
    """Fixed-decimal TEXT is not hypothetical: the MOVES snapshots store floats
    as strings for byte-reproducibility. Trim, parse, blank → NaN, anything else
    an error naming column, row and text."""
    good = pa.table(
        {
            "rate": pa.array(
                ["261.000000000000", "  -0.500000000000  ", "", "   ", None, "1e-3"],
                type=pa.string(),
            )
        }
    )
    ds = read(write(tmp_path, good, name="good.parquet"), float_columns=["rate"])
    v = ds["rate"].data
    assert str(v.dtype) == "float64"
    assert v[0] == 261.0 and v[1] == -0.5 and v[5] == 1e-3
    assert np.isnan(v[2]) and np.isnan(v[3])
    assert np.isnan(v[4]), "a null in a float column is NaN"

    # Without the option the column stays text: the reader never guesses that
    # text is really a number.
    solid = pa.table({"rate": pa.array(["261.000000000000", "1.5"], type=pa.string())})
    plain = read(write(tmp_path, solid, name="plain.parquet"))
    assert plain["rate"].dtype is None
    assert plain["rate"].data == ["261.000000000000", "1.5"]

    bad = pa.table({"rate": pa.array(["1.0", "twelve"], type=pa.string())})
    with pytest.raises(ValueError) as err:
        read(write(tmp_path, bad, name="bad.parquet"), float_columns=["rate"])
    msg = str(err.value)
    assert "'rate'" in msg and "row 1" in msg and "twelve" in msg


def test_float_columns_reaches_through_a_dictionary_and_a_decimal(tmp_path):
    """``float_columns`` is a statement about the SOURCE, so it applies to any
    column with a numeric reading — a categorical of decimal text included."""
    table = pa.table(
        {
            "cat": pa.array(["1.25", "2.50", "1.25"]).dictionary_encode(),
            "i": pa.array([7, 8, 9], type=pa.int32()),
        }
    )
    ds = read(write(tmp_path, table), float_columns=["cat", "i"])
    assert list(ds["cat"].data) == [1.25, 2.5, 1.25]
    assert list(ds["i"].data) == [7.0, 8.0, 9.0]
    assert str(ds["i"].dtype) == "float64"


# --------------------------------------------------------------------------- #
# Projection pushdown
# --------------------------------------------------------------------------- #


def test_variables_select_columns_and_an_unknown_name_is_refused(tmp_path):
    """Empty ``variables`` reads everything; an absent name lists what is
    present, never a silently missing array."""
    table = pa.table(
        {
            "a": pa.array([1, 2], type=pa.int64()),
            "b": pa.array(["x", "y"], type=pa.string()),
            "c": pa.array([1.5, 2.5], type=pa.float64()),
        }
    )
    path = write(tmp_path, table)
    assert sorted(read(path).variables) == ["a", "b", "c"]
    assert sorted(read(path, []).variables) == ["a", "b", "c"]
    assert sorted(read(path, ["c", "a"]).variables) == ["a", "c"]

    with pytest.raises(KeyError) as err:
        read(path, ["a", "nope"])
    msg = str(err.value)
    assert "nope" in msg and "'a'" in msg and "'b'" in msg


def test_projection_never_reads_the_column_chunks_it_skips(tmp_path):
    """PUSHDOWN IS REAL, proven at byte level.

    Scribble over exactly the ``poison`` column chunk's byte range, from the
    file's own metadata. The footer and the ``keep`` chunk are untouched, so the
    file still opens and still declares both columns — but a read that touches
    the poisoned chunk must fail, and a projection of ``keep`` alone must not.
    A read-then-discard implementation fails this.
    """
    n = 4096  # big enough that each column is its own multi-page chunk
    table = pa.table(
        {
            "keep": pa.array(list(range(n)), type=pa.int64()),
            "poison": pa.array([f"row-{i}" for i in range(n)], type=pa.string()),
        }
    )
    path = write(tmp_path, table, name="corrupt.parquet", compression="snappy")

    rg = pq.ParquetFile(path).metadata.row_group(0)
    chunk = next(
        rg.column(i)
        for i in range(rg.num_columns)
        if rg.column(i).path_in_schema == "poison"
    )
    start = chunk.dictionary_page_offset or chunk.data_page_offset
    length = chunk.total_compressed_size
    assert length > 0, "the poison chunk has bytes to corrupt"

    raw = bytearray(path.read_bytes())
    for i in range(start, start + length):
        raw[i] ^= 0xFF
    path.write_bytes(bytes(raw))

    # Sanity: the footer survived, so a failure below is about the CHUNK.
    assert pq.ParquetFile(path).schema_arrow.names == ["keep", "poison"]

    with pytest.raises(Exception):  # noqa: B017 - pyarrow's own corruption error
        read(path)

    ds = read(path, ["keep"])
    assert sorted(ds.variables) == ["keep"]
    assert list(ds["keep"].data) == list(range(n))


def test_a_projection_past_an_unsupported_column_still_decodes(tmp_path):
    """Pushdown means an unsupported neighbour is never touched."""
    table = pa.table(
        {
            "blob": pa.array([b"x"], type=pa.binary()),
            "id": pa.array([42], type=pa.int64()),
        }
    )
    ds = read(write(tmp_path, table), ["id"])
    assert list(ds["id"].data) == [42]
    assert sorted(ds.variables) == ["id"]


# --------------------------------------------------------------------------- #
# Edges
# --------------------------------------------------------------------------- #


def test_a_zero_row_table_yields_typed_empty_columns(tmp_path):
    """A zero-row file is TYPED, not absent — the schema lives in the footer.

    Most of a MOVES fixture's ~770 tables are empty, and a document binding one
    must still see the array it named.
    """
    schema = pa.schema(
        [("i", pa.int32()), ("f", pa.float64()), ("s", pa.string()), ("b", pa.bool_())]
    )
    ds = read(write(tmp_path, pa.table({c: [] for c in schema.names}, schema=schema)))
    assert sorted(ds.variables) == ["b", "f", "i", "s"]
    for name in schema.names:
        assert ds[name].shape == (0,), name
        assert ds[name].dims == ("index",), name
    assert str(ds["i"].dtype) == "int32"
    assert str(ds["f"].dtype) == "float64"
    assert str(ds["b"].dtype) == "bool"
    assert ds["s"].data == []


def test_a_table_spanning_many_row_groups_concatenates_in_row_order(tmp_path):
    """Row groups are a storage detail; the index axis is the table's rows."""
    n = 5_000
    table = pa.table(
        {
            "i": pa.array(list(range(n)), type=pa.int64()),
            "s": pa.array([str(i) for i in range(n)], type=pa.string()),
        }
    )
    path = write(tmp_path, table, row_group_size=512)
    assert pq.ParquetFile(path).metadata.num_row_groups > 1
    ds = read(path)
    assert list(ds["i"].data) == list(range(n))
    assert ds["s"].data[:3] == ["0", "1", "2"] and ds["s"].data[-1] == str(n - 1)


@pytest.mark.parametrize("codec", ["none", "snappy", "gzip", "zstd", "lz4"])
def test_the_compression_codecs_decode(tmp_path, codec):
    """Compression is transparent to the contract."""
    table = pa.table(
        {"i": pa.array([1, 2, 3], type=pa.int64()), "s": pa.array(["a", "b", "c"])}
    )
    ds = read(write(tmp_path, table, name=f"{codec}.parquet", compression=codec))
    assert list(ds["i"].data) == [1, 2, 3]
    assert ds["s"].data == ["a", "b", "c"]


def test_a_non_parquet_blob_is_an_error_not_a_crash(tmp_path):
    path = tmp_path / "nope.parquet"
    path.write_bytes(b"definitely not parquet")
    with pytest.raises(Exception):  # noqa: B017 - pyarrow's own ArrowInvalid
        read(path)


# --------------------------------------------------------------------------- #
# The registry seam
# --------------------------------------------------------------------------- #


def test_the_registry_serves_the_reader_and_screens_its_options():
    """``parquet`` is a registered active format, and an unrecognised decode
    option is refused at Provider-build time (``spec/registries.md`` §2.1)."""
    assert "parquet" in format_registry
    assert format_registry.status("parquet") == "active"
    reader = format_registry.create("parquet")
    assert isinstance(reader, ParquetReader)
    assert reader.formats() == ["parquet"]
    assert reader.extensions() == ["parquet", "parq", "pq"]

    ok = DataSource(
        name="moves",
        url="file:///t.parquet",
        format="parquet",
        reader_kwargs={"float_columns": ["meanBaseRate"], "null_int": -1,
                       "null_string": ""},
    )
    check_reader_options(reader, ok)

    bad = DataSource(
        name="moves", url="file:///t.parquet", format="parquet",
        reader_kwargs={"member_glob": "*"},
    )
    with pytest.raises(Exception) as err:
        check_reader_options(reader, bad)
    assert "member_glob" in str(err.value)


# --------------------------------------------------------------------------- #
# A real MOVES table (skipped when the sibling snapshot checkout is absent)
# --------------------------------------------------------------------------- #


def _moves_tables_dir():
    """The MOVES snapshot ``tables/`` directory, or ``None``.

    The env override first, then the sibling checkout the downstream ``.esm``
    project uses — the same two places ``rust/tests/parquet_reader.rs`` looks.
    """
    import os

    env = os.environ.get("EARTHSCIIO_MOVES_SNAPSHOTS")
    roots = [pathlib.Path(env)] if env else [
        pathlib.Path(__file__).resolve().parents[3]
        / "moves.rs" / "characterization" / "snapshots"
    ]
    for root in roots:
        if (root / "tables").is_dir():
            return root / "tables"
        if root.is_dir():
            for fixture in sorted(p for p in root.iterdir() if (p / "tables").is_dir()):
                return fixture / "tables"
    return None


def _find_table(tables, suffix):
    hits = sorted(tables.glob(f"*__{suffix}.parquet"))
    return hits[0] if hits else None


def test_moves_snapshot_smoke():
    """A real EPA MOVES table decodes the way a document would read it.

    The snapshots write every column as int64 or utf8 — floats included, as
    fixed-decimal text — so nothing decodes as a native float until a document
    says ``float_columns``, and a code column stays text.
    """
    tables = _moves_tables_dir()
    if tables is None:
        pytest.skip("no MOVES snapshot tables found")
    path = _find_table(tables, "nremissionrate")
    if path is None:
        pytest.skip(f"no nremissionrate table under {tables}")

    # The whole table first, so the projection below is checked against the
    # file's own schema rather than an assumed one.
    everything = read(path)
    n = next(iter(everything.variables.values())).shape[0]
    assert n > 0, "nremissionrate is a non-empty table"
    for name, f in everything.variables.items():
        assert f.dims == ("index",), name
        assert f.shape == (n,), name
    assert str(everything["polProcessID"].dtype) == "int64"
    assert everything["SCC"].dtype is None, "a code column stays text"
    assert everything["meanBaseRate"].dtype is None

    want = ["polProcessID", "SCC", "meanBaseRate"]
    ds = read(path, want, float_columns=["meanBaseRate"])
    assert sorted(ds.variables) == sorted(want), "only the projected columns"
    assert str(ds["polProcessID"].dtype) == "int64"
    assert ds["SCC"].dtype is None
    assert str(ds["meanBaseRate"].dtype) == "float64"
    assert ds["SCC"].shape == (n,), "the projection keeps every row"

    rates = ds["meanBaseRate"].data
    assert np.isfinite(rates).all() and (rates >= 0).all()
    text = everything["meanBaseRate"].data
    for i in (0, n // 2, n - 1):
        assert rates[i] == float(text[i].strip()), f"row {i}"
    assert ds["SCC"].data == everything["SCC"].data, "projection does not reorder rows"


def test_moves_output_table_stores_its_emissions_as_decimal_text():
    """The motivating corpus, verified: ``MOVESOutput``'s emission columns are
    STRING on disk, and ``float_columns`` is what turns them into numbers."""
    tables = _moves_tables_dir()
    if tables is None:
        pytest.skip("no MOVES snapshot tables found")
    path = _find_table(tables, "movesoutput")
    if path is None:
        pytest.skip(f"no MOVESOutput table under {tables}")

    cols = ["emissionQuant", "emissionQuantMean", "emissionQuantSigma"]
    # The mean/sigma columns really are NULL in this snapshot, so reading them as
    # text needs the gate opened — which is the null policy doing its job.
    with pytest.raises(ValueError) as err:
        read(path, cols)
    assert "null_string" in str(err.value)
    plain = read(path, cols, null_string="")
    for c in cols:
        assert plain[c].dtype is None, f"{c} is decimal text on disk"

    ds = read(path, cols, float_columns=cols)
    for c in cols:
        assert str(ds[c].dtype) == "float64", c
        assert ds[c].shape == plain[c].shape
