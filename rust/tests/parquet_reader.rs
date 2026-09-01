//! The active `parquet` reader (Rust track) — an Apache Parquet file as a flat
//! table on `index`.
//!
//! The fixtures are written **in the test** with arrow-rs's own `ArrowWriter`
//! rather than committed as blobs, because what is under test is a round trip
//! through a real Parquet encoder: schema in the footer, per-column chunks, the
//! null bitmap, dictionary encoding and a compression codec are all things the
//! writer produces and the reader must undo. A committed blob would pin one
//! encoder's output and could not vary any of them.
//!
//! What is checked here: the Arrow → `DType` table (including the narrow/wide
//! integer split shared with the NetCDF reader), the null policy in both its
//! folding and its refusing halves, projection pushdown by `variables`,
//! `float_columns` over both integers and decimal text, the `reader_options`
//! screen, a zero-row table, a compressed file, and registration under the
//! format name without a Provider edit.
//!
//! A real MOVES snapshot table is read too, when one is present — see
//! `moves_snapshot_smoke`.

// Native-only: readers open a cached blob, which does not exist on wasm32.
#![cfg(not(target_arch = "wasm32"))]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use arrow_array::builder::StringDictionaryBuilder;
use arrow_array::types::Int32Type;
use arrow_array::{
    ArrayRef, BooleanArray, Date32Array, Decimal128Array, Float32Array, Float64Array, Int16Array,
    Int32Array, Int64Array, RecordBatch, StringArray, TimestampMillisecondArray, UInt16Array,
    UInt32Array, UInt64Array,
};
use arrow_schema::{DataType, Field, Schema};
use parquet::arrow::ArrowWriter;
use parquet::basic::Compression;
use parquet::file::properties::WriterProperties;

use earthsciio::{
    ArrayData, DType, Error, FormatRegistry, NativeDataset, ParquetReader, Reader, Selection,
};
use serde_json::{json, Map, Value};

// --------------------------------------------------------------------------- //
// Fixture helpers
// --------------------------------------------------------------------------- //

/// Write `columns` as a one-row-group Parquet file under `dir`, returning its
/// path. `compression` exercises the codec features the crate enables.
fn write_parquet(
    dir: &Path,
    name: &str,
    columns: Vec<(&str, ArrayRef)>,
    compression: Compression,
) -> PathBuf {
    let fields: Vec<Field> = columns
        .iter()
        .map(|(n, a)| Field::new(*n, a.data_type().clone(), true))
        .collect();
    let schema = Arc::new(Schema::new(fields));
    let arrays: Vec<ArrayRef> = columns.into_iter().map(|(_, a)| a).collect();
    let path = dir.join(name);
    let file = std::fs::File::create(&path).expect("create fixture");
    let props = WriterProperties::builder()
        .set_compression(compression)
        .build();
    let mut w = ArrowWriter::try_new(file, schema.clone(), Some(props)).expect("open writer");
    // An all-empty column set still has to produce a footer, so only write a
    // batch when there are rows to write.
    let batch = RecordBatch::try_new(schema, arrays).expect("build batch");
    if batch.num_rows() > 0 {
        w.write(&batch).expect("write batch");
    }
    w.close().expect("close writer");
    path
}

fn read(path: &Path, variables: &[&str]) -> Result<NativeDataset, Error> {
    let vars: Vec<String> = variables.iter().map(|s| s.to_string()).collect();
    ParquetReader::new().read_native(path, &vars, &Selection::All)
}

fn configured(options: Value) -> Arc<dyn Reader> {
    let map: Map<String, Value> = options.as_object().expect("an object").clone();
    ParquetReader::new()
        .configured(&map)
        .expect("options accepted")
        .expect("a configured reader")
}

fn f64s<'a>(ds: &'a NativeDataset, name: &str) -> &'a [f64] {
    match &ds.variables[name].data {
        ArrayData::F64(v) => v,
        other => panic!("{name} is {other:?}, not float64"),
    }
}

fn i64s<'a>(ds: &'a NativeDataset, name: &str) -> &'a [i64] {
    match &ds.variables[name].data {
        ArrayData::I64(v) => v,
        other => panic!("{name} is {other:?}, not int64"),
    }
}

fn i32s<'a>(ds: &'a NativeDataset, name: &str) -> &'a [i32] {
    match &ds.variables[name].data {
        ArrayData::I32(v) => v,
        other => panic!("{name} is {other:?}, not int32"),
    }
}

fn strs<'a>(ds: &'a NativeDataset, name: &str) -> &'a [String] {
    match &ds.variables[name].data {
        ArrayData::Str(v) => v,
        other => panic!("{name} is {other:?}, not string"),
    }
}

// --------------------------------------------------------------------------- //
// Type mapping
// --------------------------------------------------------------------------- //

/// The Arrow → `DType` table of the module docs, over one file that carries a
/// column of every supported family. A MOVES table is int64 ID columns, string
/// codes and float values; the rest are here so the contract is pinned.
#[test]
fn every_supported_arrow_type_maps_onto_the_native_contract() {
    let dir = tempdir();
    let mut dict = StringDictionaryBuilder::<Int32Type>::new();
    for v in ["gas", "diesel", "gas"] {
        dict.append_value(v);
    }
    let path = write_parquet(
        dir.path(),
        "types.parquet",
        vec![
            (
                "b",
                Arc::new(BooleanArray::from(vec![true, false, true])) as ArrayRef,
            ),
            ("i16", Arc::new(Int16Array::from(vec![1i16, -2, 3]))),
            ("i32", Arc::new(Int32Array::from(vec![10i32, -20, 30]))),
            ("i64", Arc::new(Int64Array::from(vec![100i64, -200, 300]))),
            ("u16", Arc::new(UInt16Array::from(vec![1u16, 2, 3]))),
            ("u32", Arc::new(UInt32Array::from(vec![1u32, 2, 3]))),
            ("u64", Arc::new(UInt64Array::from(vec![1u64, 2, 3]))),
            ("f32", Arc::new(Float32Array::from(vec![1.5f32, 2.5, 3.5]))),
            (
                "f64",
                Arc::new(Float64Array::from(vec![1.25f64, 2.5, 3.75])),
            ),
            (
                "s",
                Arc::new(StringArray::from(vec!["2260000000", "x", ""])),
            ),
            ("cat", Arc::new(dict.finish())),
            (
                "d32",
                Arc::new(Date32Array::from(vec![19000i32, 19001, 19002])),
            ),
            (
                "ts",
                Arc::new(TimestampMillisecondArray::from(vec![
                    1_700_000_000_000i64,
                    0,
                    -5,
                ])),
            ),
            (
                "dec",
                Arc::new(
                    Decimal128Array::from(vec![261_000_000_000_000i128, -1_500_000_000_000, 0])
                        .with_precision_and_scale(30, 12)
                        .expect("decimal(30,12)"),
                ),
            ),
        ],
        Compression::UNCOMPRESSED,
    );

    let ds = read(&path, &[]).expect("decode");

    // Every field is rank-1 over `index`, length 3, and carries no coordinate.
    assert!(ds.coords.is_empty(), "a table has no native coordinates");
    for (name, f) in &ds.variables {
        assert_eq!(f.dims, vec!["index".to_string()], "{name} dims");
        assert_eq!(f.shape, vec![3], "{name} shape");
        assert_eq!(f.data.len(), 3, "{name} length");
        assert_eq!(f.data.dtype(), f.dtype, "{name} dtype agrees with its data");
    }

    let dt = |n: &str| ds.variables[n].dtype;
    assert_eq!(dt("b"), DType::Bool);
    // The narrow/wide integer split is NetcdfReader's, verbatim.
    assert_eq!(dt("i16"), DType::Int32);
    assert_eq!(dt("i32"), DType::Int32);
    assert_eq!(dt("u16"), DType::Int32);
    assert_eq!(dt("i64"), DType::Int64);
    assert_eq!(dt("u32"), DType::Int64);
    assert_eq!(dt("u64"), DType::Int64);
    assert_eq!(dt("f32"), DType::Float64);
    assert_eq!(dt("f64"), DType::Float64);
    assert_eq!(dt("dec"), DType::Float64);
    assert_eq!(dt("s"), DType::Str);
    // A categorical reads as its VALUE type, expanded — not as its key.
    assert_eq!(dt("cat"), DType::Str);
    // Temporal columns ride as their raw integer, at their stored width.
    assert_eq!(dt("d32"), DType::Int32);
    assert_eq!(dt("ts"), DType::Int64);

    assert_eq!(i32s(&ds, "i16"), [1, -2, 3]);
    assert_eq!(i64s(&ds, "u32"), [1, 2, 3]);
    assert_eq!(f64s(&ds, "f32"), [1.5, 2.5, 3.5]);
    // A leading-zero-safe code column stays text, and an empty cell is "" — an
    // empty string is a VALUE here, not a missing one (that is `null`).
    assert_eq!(strs(&ds, "s"), ["2260000000", "x", ""]);
    assert_eq!(strs(&ds, "cat"), ["gas", "diesel", "gas"]);
    assert_eq!(i32s(&ds, "d32"), [19000, 19001, 19002]);
    // NOT decoded to an instant: the raw stored milliseconds, verbatim.
    assert_eq!(i64s(&ds, "ts"), [1_700_000_000_000, 0, -5]);
    // decimal(30,12): unscaled ÷ 10^12.
    assert_eq!(f64s(&ds, "dec"), [261.0, -1.5, 0.0]);
}

/// A `uint64` past `i64::MAX` has no int64 reading, so it is a named error
/// rather than a wraparound into a negative ID.
#[test]
fn a_uint64_too_large_for_int64_is_an_error() {
    let dir = tempdir();
    let path = write_parquet(
        dir.path(),
        "big.parquet",
        vec![("u", Arc::new(UInt64Array::from(vec![u64::MAX])) as ArrayRef)],
        Compression::UNCOMPRESSED,
    );
    let err = read(&path, &[]).expect_err("must refuse");
    let msg = format!("{err}");
    assert!(msg.contains("\"u\""), "names the column: {msg}");
    assert!(msg.contains("row 0"), "names the row: {msg}");
}

/// ...but under `float_columns` there IS a reading, and it must be given.
///
/// The `uint64` refusal exists to stop a value wrapping into a NEGATIVE ID —
/// `spec/conformance.md` §3, "never a wraparound into a negative ID". Under
/// `float_columns` the document has said the column is a float64 measurement:
/// the target has no int64 to wrap into, `f64` represents the magnitude fine,
/// and the Python and Julia tracks both decode it. Rust refused it anyway,
/// because the range check lived in `cells()` — the decode — rather than in the
/// integer coercion, so it fired whatever the column was being read AS. Same
/// shape of bug as `float_columns` promoting a `Binary` column into a field
/// that then hard-errored (fixed in a30c9e4): an option combination one backend
/// handles and another errors on.
#[test]
fn float_columns_reads_a_uint64_beyond_int64_max() {
    let dir = tempdir();
    let path = write_parquet(
        dir.path(),
        "bigu.parquet",
        vec![(
            "u",
            Arc::new(UInt64Array::from(vec![Some(1u64), Some(u64::MAX), None])) as ArrayRef,
        )],
        Compression::UNCOMPRESSED,
    );
    let ds = configured(json!({"float_columns": ["u"]}))
        .read_native(&path, &[], &Selection::All)
        .expect("a uint64 declared float64 decodes");
    assert_eq!(ds.variables["u"].dtype, DType::Float64);
    let v = f64s(&ds, "u");
    assert_eq!(v[0], 1.0);
    assert_eq!(v[1], u64::MAX as f64);
    assert!(v[2].is_nan(), "a null in a float column is NaN");
    // The int64 reading of the SAME column is still refused — the range check
    // moved, it did not disappear.
    let err = read(&path, &[]).expect_err("still refused as an integer");
    assert!(format!("{err}").contains("\"u\""));
}

/// Nested and binary columns have no rank-1 reading. Requesting one by name is
/// an error (the document named an array it will not get); in read-everything
/// mode the column is simply not a native field, as the NetCDF reader skips its
/// non-numeric variables.
#[test]
fn a_nested_column_is_skipped_when_unrequested_and_refused_when_named() {
    use arrow_array::{BinaryArray, StructArray};
    let dir = tempdir();
    let inner = Arc::new(Int32Array::from(vec![1, 2])) as ArrayRef;
    let st = StructArray::from(vec![(
        Arc::new(Field::new("a", DataType::Int32, true)),
        inner,
    )]);
    let path = write_parquet(
        dir.path(),
        "nested.parquet",
        vec![
            ("id", Arc::new(Int64Array::from(vec![7i64, 8])) as ArrayRef),
            (
                "blob",
                Arc::new(BinaryArray::from(vec![&b"x"[..], &b"y"[..]])),
            ),
            ("st", Arc::new(st)),
        ],
        Compression::UNCOMPRESSED,
    );

    let ds = read(&path, &[]).expect("decode the readable columns");
    assert_eq!(i64s(&ds, "id"), [7, 8]);
    assert!(!ds.variables.contains_key("blob"), "binary is not a field");
    assert!(!ds.variables.contains_key("st"), "struct is not a field");

    let err = read(&path, &["st"]).expect_err("naming it must refuse");
    assert!(format!("{err}").contains("\"st\""), "{err}");
}

/// `float_columns` must not promote a binary column into a field. It is a
/// statement about how to READ a column, not a claim that an opaque blob is a
/// number — so an unrequested binary column stays a non-field whether or not it
/// is named, exactly as read-everything mode leaves it.
///
/// Regression: the `forced_float` arm listed only the nested types as
/// unsupported, so a named binary column registered an accumulator and then
/// hard-errored in `cells()` — an error where the spec says "simply not a
/// field", and a divergence from the Python track, which refuses binary
/// regardless of `float_columns`.
#[test]
fn float_columns_does_not_promote_a_binary_column_into_a_field() {
    use arrow_array::BinaryArray;
    let dir = tempdir();
    let path = write_parquet(
        dir.path(),
        "forced_blob.parquet",
        vec![
            ("id", Arc::new(Int64Array::from(vec![7i64, 8])) as ArrayRef),
            (
                "blob",
                Arc::new(BinaryArray::from(vec![&b"x"[..], &b"y"[..]])),
            ),
        ],
        Compression::UNCOMPRESSED,
    );

    let ds = configured(json!({"float_columns": ["blob"]}))
        .read_native(&path, &[], &Selection::All)
        .expect("an unrequested binary column is skipped, not an error");
    assert_eq!(i64s(&ds, "id"), [7, 8]);
    assert!(
        !ds.variables.contains_key("blob"),
        "binary stays a non-field even when named in float_columns"
    );
}

// --------------------------------------------------------------------------- //
// Null policy
// --------------------------------------------------------------------------- //

/// A null in a float column is `NaN` — the same fold a CF `_FillValue` gets —
/// and `fill_value` stays `None` because no sentinel survives into the array.
#[test]
fn a_null_float_is_nan_and_no_sentinel_survives() {
    let dir = tempdir();
    let path = write_parquet(
        dir.path(),
        "nullf.parquet",
        vec![(
            "v",
            Arc::new(Float64Array::from(vec![Some(1.0), None, Some(3.0)])) as ArrayRef,
        )],
        Compression::UNCOMPRESSED,
    );
    let ds = read(&path, &[]).expect("decode");
    let v = f64s(&ds, "v");
    assert_eq!(v[0], 1.0);
    assert!(v[1].is_nan(), "a null float folds to NaN");
    assert_eq!(v[2], 3.0);
    assert_eq!(ds.variables["v"].fill_value, None);
}

/// An integer, string or boolean column has no NaN, so a null in one is a named
/// error rather than a silently substituted real value. The message has to name
/// the way out, because only the document can choose a sentinel.
#[test]
fn a_null_in_a_type_with_no_missing_value_is_refused_by_default() {
    let dir = tempdir();
    for (name, col) in [
        (
            "i",
            Arc::new(Int64Array::from(vec![Some(1i64), None])) as ArrayRef,
        ),
        ("s", Arc::new(StringArray::from(vec![Some("a"), None]))),
        ("b", Arc::new(BooleanArray::from(vec![Some(true), None]))),
    ] {
        let path = write_parquet(
            dir.path(),
            &format!("null_{name}.parquet"),
            vec![(name, col)],
            Compression::UNCOMPRESSED,
        );
        let err = read(&path, &[]).expect_err("a null must be refused");
        let msg = format!("{err}");
        assert!(
            msg.contains(&format!("{name:?}")),
            "names the column: {msg}"
        );
        assert!(msg.contains("row 1"), "names the row: {msg}");
        assert!(msg.contains("float_columns"), "names a way out: {msg}");
    }
}

/// `null_int` opens the gate explicitly, and the declared sentinel is reported
/// back in `fill_value` — an integer sentinel cannot be NaN, so it survives into
/// the array exactly as a CF integer fill does.
#[test]
fn a_declared_int_sentinel_fills_and_is_reported() {
    let dir = tempdir();
    let path = write_parquet(
        dir.path(),
        "nulli.parquet",
        vec![(
            "id",
            Arc::new(Int64Array::from(vec![Some(5i64), None, Some(7)])) as ArrayRef,
        )],
        Compression::UNCOMPRESSED,
    );
    let ds = configured(json!({"null_int": -1}))
        .read_native(&path, &[], &Selection::All)
        .expect("decode with a sentinel");
    assert_eq!(i64s(&ds, "id"), [5, -1, 7]);
    assert_eq!(ds.variables["id"].fill_value, Some(-1.0));
}

/// `null_string` likewise, and it is a plain value in the array — only the
/// document that chose it can tell it from a real cell.
#[test]
fn a_declared_string_stands_in_for_a_null_text_cell() {
    let dir = tempdir();
    let path = write_parquet(
        dir.path(),
        "nulls.parquet",
        vec![(
            "scc",
            Arc::new(StringArray::from(vec![Some("2260"), None])) as ArrayRef,
        )],
        Compression::UNCOMPRESSED,
    );
    let ds = configured(json!({"null_string": "UNKNOWN"}))
        .read_native(&path, &[], &Selection::All)
        .expect("decode with a substitute");
    assert_eq!(strs(&ds, "scc"), ["2260", "UNKNOWN"]);
    assert_eq!(ds.variables["scc"].fill_value, None);
}

/// Routing an integer column through `float_columns` is the other way out: its
/// nulls become NaN instead of needing a sentinel.
#[test]
fn float_columns_turns_a_nullable_integer_into_a_nan_carrying_float() {
    let dir = tempdir();
    let path = write_parquet(
        dir.path(),
        "forced.parquet",
        vec![(
            "pop",
            Arc::new(Int64Array::from(vec![Some(3i64), None])) as ArrayRef,
        )],
        Compression::UNCOMPRESSED,
    );
    let ds = configured(json!({"float_columns": ["pop"]}))
        .read_native(&path, &[], &Selection::All)
        .expect("decode as float");
    assert_eq!(ds.variables["pop"].dtype, DType::Float64);
    let v = f64s(&ds, "pop");
    assert_eq!(v[0], 3.0);
    assert!(v[1].is_nan());
}

// --------------------------------------------------------------------------- //
// float_columns over decimal TEXT — the MOVES snapshot convention
// --------------------------------------------------------------------------- //

/// A corpus that needs byte-reproducible floats stores them as fixed-decimal
/// STRINGS rather than IEEE doubles; the MOVES snapshots write `meanBaseRate`
/// as `"261.000000000000"`. `float_columns` is how a document says so. A blank
/// cell is NaN (the FF10/shapefile rule); anything else unparseable is an error
/// naming the column, the row and the text.
#[test]
fn float_columns_parses_decimal_text_and_refuses_the_unparseable() {
    let dir = tempdir();
    let path = write_parquet(
        dir.path(),
        "text.parquet",
        vec![(
            "meanBaseRate",
            Arc::new(StringArray::from(vec![
                "261.000000000000",
                "219.990000000000",
                "  ",
                "-1.5e3",
            ])) as ArrayRef,
        )],
        Compression::UNCOMPRESSED,
    );
    let ds = configured(json!({"float_columns": ["meanBaseRate"]}))
        .read_native(&path, &[], &Selection::All)
        .expect("decode decimal text");
    let v = f64s(&ds, "meanBaseRate");
    assert_eq!(v[0], 261.0);
    assert_eq!(v[1], 219.99);
    assert!(v[2].is_nan(), "a blank cell is NaN");
    assert_eq!(v[3], -1500.0);

    // Without the option the same column stays TEXT — the reader never guesses
    // that a string is really a number.
    let plain = read(&path, &[]).expect("decode as text");
    assert_eq!(plain.variables["meanBaseRate"].dtype, DType::Str);

    let bad = write_parquet(
        dir.path(),
        "bad.parquet",
        vec![(
            "x",
            Arc::new(StringArray::from(vec!["1.0", "not a number"])) as ArrayRef,
        )],
        Compression::UNCOMPRESSED,
    );
    let err = configured(json!({"float_columns": ["x"]}))
        .read_native(&bad, &[], &Selection::All)
        .expect_err("garbage must not become NaN silently");
    let msg = format!("{err}");
    assert!(msg.contains("not a number"), "quotes the text: {msg}");
    assert!(msg.contains("row 1"), "names the row: {msg}");
}

// --------------------------------------------------------------------------- //
// Projection pushdown
// --------------------------------------------------------------------------- //

/// `variables` selects columns, and does it by pushing a `ProjectionMask` into
/// the Parquet reader. The observable contract is that only the named columns
/// come back; that the others were never decoded is why the option exists.
#[test]
fn variables_select_columns_and_an_unknown_name_is_refused() {
    let dir = tempdir();
    let path = write_parquet(
        dir.path(),
        "wide.parquet",
        vec![
            ("a", Arc::new(Int64Array::from(vec![1i64, 2])) as ArrayRef),
            ("b", Arc::new(Int64Array::from(vec![3i64, 4]))),
            ("c", Arc::new(StringArray::from(vec!["x", "y"]))),
        ],
        Compression::UNCOMPRESSED,
    );

    let ds = read(&path, &["c", "a"]).expect("decode a projection");
    let mut got: Vec<&str> = ds.variables.keys().map(String::as_str).collect();
    got.sort_unstable();
    assert_eq!(got, ["a", "c"]);
    assert_eq!(i64s(&ds, "a"), [1, 2]);
    assert_eq!(strs(&ds, "c"), ["x", "y"]);

    let err = read(&path, &["a", "nope"]).expect_err("an unknown column must refuse");
    let msg = format!("{err}");
    assert!(msg.contains("nope"), "names what is missing: {msg}");
    assert!(msg.contains("\"a\""), "lists what is present: {msg}");
}

/// Projection pushdown is REAL, not read-then-discard, and this proves it at the
/// byte level rather than by inspecting the result.
///
/// A two-column SNAPPY file is written, then the bytes of one column's chunk are
/// scribbled over in place. The footer is untouched, so the file still opens and
/// its schema is intact — but any reader that actually fetches and decompresses
/// that chunk must fail. Reading both columns fails; reading only the other
/// column succeeds and returns correct values. The skipped chunk was therefore
/// never read off disk, which is the whole point on tables dozens of columns
/// wide where a document wants three.
#[test]
fn projection_never_reads_the_column_chunks_it_skips() {
    let dir = tempdir();
    let n = 4096i64; // big enough that each column is its own multi-page chunk
    let path = write_parquet(
        dir.path(),
        "corrupt.parquet",
        vec![
            (
                "keep",
                Arc::new(Int64Array::from((0..n).collect::<Vec<i64>>())) as ArrayRef,
            ),
            (
                "poison",
                Arc::new(StringArray::from(
                    (0..n).map(|i| format!("row-{i}")).collect::<Vec<String>>(),
                )),
            ),
        ],
        Compression::SNAPPY,
    );

    // Byte range of the `poison` column chunk, from the file's own metadata.
    let (start, len) = {
        let f = std::fs::File::open(&path).expect("open");
        let b = parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder::try_new(f)
            .expect("open as parquet");
        let rg = b.metadata().row_group(0);
        let idx = (0..rg.num_columns())
            .find(|i| rg.column(*i).column_path().string() == "poison")
            .expect("the poison column chunk");
        rg.column(idx).byte_range()
    };
    assert!(len > 0, "the poison chunk has bytes to corrupt");

    // Scribble over exactly that range; the footer and the `keep` chunk are
    // untouched, so the file still opens and still reports both columns.
    let mut bytes = std::fs::read(&path).expect("read back");
    for b in &mut bytes[start as usize..(start + len) as usize] {
        *b = !*b;
    }
    std::fs::write(&path, &bytes).expect("rewrite");

    // Sanity: the file still parses, so a failure below is about the CHUNK.
    let roots = {
        let f = std::fs::File::open(&path).expect("open");
        let b = parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder::try_new(f)
            .expect("the footer survived");
        b.parquet_schema()
            .root_schema()
            .get_fields()
            .iter()
            .map(|f| f.name().to_string())
            .collect::<Vec<_>>()
    };
    assert_eq!(roots, ["keep", "poison"], "both columns are still declared");

    // Reading everything must touch the poisoned chunk and fail.
    let err = read(&path, &[]).expect_err("a corrupt chunk must not decode silently");
    assert!(matches!(err, Error::Format { .. }), "{err}");

    // Reading only `keep` must never touch it.
    let ds = read(&path, &["keep"]).expect("the skipped chunk is never read");
    assert_eq!(ds.variables.len(), 1);
    let v = i64s(&ds, "keep");
    assert_eq!(v.len(), n as usize);
    assert!(v.iter().enumerate().all(|(i, &x)| x == i as i64));
}

/// A projection that reads a column NOT covered by an unsupported one still
/// works — pushdown means the nested column is never touched.
#[test]
fn a_projection_past_an_unsupported_column_still_decodes() {
    use arrow_array::BinaryArray;
    let dir = tempdir();
    let path = write_parquet(
        dir.path(),
        "mixed.parquet",
        vec![
            (
                "blob",
                Arc::new(BinaryArray::from(vec![&b"x"[..]])) as ArrayRef,
            ),
            ("id", Arc::new(Int64Array::from(vec![42i64]))),
        ],
        Compression::UNCOMPRESSED,
    );
    let ds = read(&path, &["id"]).expect("decode past the binary column");
    assert_eq!(i64s(&ds, "id"), [42]);
    assert_eq!(ds.variables.len(), 1);
}

// --------------------------------------------------------------------------- //
// Edges
// --------------------------------------------------------------------------- //

/// A zero-row table is TYPED, not absent: the schema lives in the footer, so
/// every column comes back empty with its declared dtype. Most of a MOVES
/// fixture's ~770 tables are empty, and a document binding one must still see
/// the array it named.
#[test]
fn a_zero_row_table_yields_typed_empty_columns() {
    let dir = tempdir();
    let path = write_parquet(
        dir.path(),
        "empty.parquet",
        vec![
            (
                "id",
                Arc::new(Int64Array::from(Vec::<i64>::new())) as ArrayRef,
            ),
            ("scc", Arc::new(StringArray::from(Vec::<&str>::new()))),
        ],
        Compression::UNCOMPRESSED,
    );
    let ds = read(&path, &[]).expect("decode an empty table");
    assert_eq!(ds.variables.len(), 2);
    assert_eq!(ds.variables["id"].dtype, DType::Int64);
    assert_eq!(ds.variables["id"].shape, vec![0]);
    assert_eq!(ds.variables["scc"].dtype, DType::Str);
    assert!(ds.variables["scc"].data.is_empty());
}

/// A table longer than one Arrow batch decodes to one contiguous column — the
/// per-batch accumulation must not lose or reorder rows.
#[test]
fn a_table_spanning_many_batches_concatenates_in_row_order() {
    let dir = tempdir();
    let n = 20_000i64; // > the reader's 8192-row batch
    let path = write_parquet(
        dir.path(),
        "long.parquet",
        vec![(
            "i",
            Arc::new(Int64Array::from((0..n).collect::<Vec<i64>>())) as ArrayRef,
        )],
        Compression::UNCOMPRESSED,
    );
    let ds = read(&path, &[]).expect("decode");
    let v = i64s(&ds, "i");
    assert_eq!(v.len(), n as usize);
    assert!(v.iter().enumerate().all(|(i, &x)| x == i as i64));
}

/// The compression codecs the crate enables actually decode. Snappy is
/// Parquet's de-facto default and zstd is what modern writers pick.
#[test]
fn the_enabled_compression_codecs_decode() {
    let dir = tempdir();
    for (label, codec) in [
        ("snappy", Compression::SNAPPY),
        ("gzip", Compression::GZIP(Default::default())),
        ("zstd", Compression::ZSTD(Default::default())),
        ("lz4", Compression::LZ4_RAW),
        ("brotli", Compression::BROTLI(Default::default())),
    ] {
        let path = write_parquet(
            dir.path(),
            &format!("{label}.parquet"),
            vec![
                (
                    "v",
                    Arc::new(Float64Array::from(vec![1.5, 2.5])) as ArrayRef,
                ),
                ("s", Arc::new(StringArray::from(vec!["a", "b"]))),
            ],
            codec,
        );
        let ds = read(&path, &[]).unwrap_or_else(|e| panic!("{label}: {e}"));
        assert_eq!(f64s(&ds, "v"), [1.5, 2.5], "{label}");
        assert_eq!(strs(&ds, "s"), ["a", "b"], "{label}");
    }
}

/// Not a Parquet file at all is a clean `Error::Format`, never a panic.
#[test]
fn a_non_parquet_blob_is_an_error_not_a_panic() {
    let dir = tempdir();
    let path = dir.path().join("nope.parquet");
    std::fs::write(&path, b"PAR1 is how it should start, but this is not it").unwrap();
    let err = read(&path, &[]).expect_err("must refuse");
    assert!(matches!(err, Error::Format { .. }), "{err}");
}

// --------------------------------------------------------------------------- //
// Registration + reader_options screen
// --------------------------------------------------------------------------- //

/// The §2 extensibility invariant, concretely: `parquet` resolves out of the
/// builtin registry and the pre-existing readers are untouched.
#[test]
fn parquet_is_a_builtin_format_name() {
    let r = FormatRegistry::with_builtins();
    let reader = r.get("parquet").expect("parquet is registered");
    assert_eq!(reader.formats(), &["parquet"]);
    assert!(reader.extensions().contains(&"parquet"));
    assert!(!reader.store_backed(), "a parquet blob is one file");
    assert!(
        !reader.supports_selection(),
        "row selection is esm-spec §8.9.2 work downstream of the decode"
    );
    assert!(r.get("netcdf").is_some());
    assert!(r.get("shapefile").is_some());
}

/// esm-spec §8.9.1: a key the reader does not recognise MUST be an error, not an
/// ignored key — a mis-spelled option that silently decodes something else is
/// found much later, as wrong numbers. Same for a recognised key mistyped.
#[test]
fn unknown_or_ill_typed_reader_options_are_rejected() {
    let reader = ParquetReader::new();
    for bad in [
        json!({"numeric_columns": ["x"]}), // the shapefile spelling
        json!({"float_columns": "x"}),     // must be an array
        json!({"float_columns": [1, 2]}),  // of strings
        json!({"null_int": "0"}),          // must be an integer
        json!({"null_string": 0}),         // must be a string
        json!({"select": "all"}),          // not a reader concern
    ] {
        let map: Map<String, Value> = bad.as_object().unwrap().clone();
        let Err(err) = reader.configured(&map) else {
            panic!("{bad} must be rejected");
        };
        assert!(matches!(err, Error::Format { .. }), "{bad}: {err}");
    }

    // And an empty set means "use me as registered".
    let Ok(None) = reader.configured(&Map::new()) else {
        panic!("an empty option set must reuse the registered reader");
    };
}

// --------------------------------------------------------------------------- //
// Realistic smoke test
// --------------------------------------------------------------------------- //

/// Read a REAL MOVES snapshot table when one is on this machine, so the reader
/// is known to work against the corpus it exists for and not only against
/// arrow-rs's own writer. The snapshots are gigabytes and live outside this
/// repo, so the test locates them and skips when they are absent — it can never
/// be the only thing covering a behaviour.
///
/// `nremissionrate` is the shape that matters: int64 ID columns, an `SCC` string
/// code that must NOT become a number, and a rate stored as fixed-decimal TEXT.
/// The assertions are driven by the file's own schema rather than a hardcoded
/// column list, so a fixture that grows a column does not fail the test.
///
/// Point `EARTHSCIIO_MOVES_SNAPSHOTS` at a directory of `<fixture>/tables/`
/// to run it elsewhere.
#[test]
fn moves_snapshot_smoke() {
    let Some(tables) = moves_tables_dir() else {
        eprintln!("skipping moves_snapshot_smoke: no MOVES snapshot tables found");
        return;
    };
    let Some(path) = find_table(&tables, "nremissionrate") else {
        eprintln!(
            "skipping moves_snapshot_smoke: no nremissionrate under {}",
            tables.display()
        );
        return;
    };

    // Whole table first, so the projection below is checked against the file's
    // own schema rather than an assumed one.
    let all = read(&path, &[]).expect("decode a real MOVES table");
    let n = all
        .variables
        .values()
        .next()
        .expect("a non-empty schema")
        .shape[0];
    assert!(n > 0, "nremissionrate is a non-empty table");
    for (name, f) in &all.variables {
        assert_eq!(f.dims, vec!["index".to_string()], "{name} dims");
        assert_eq!(f.shape, vec![n], "{name} shares the index axis");
    }
    // The MOVES snapshots write every column as int64 or utf8 — floats included,
    // as fixed-decimal text — so nothing here should decode as a native float
    // until a document says `float_columns`.
    assert_eq!(all.variables["polProcessID"].dtype, DType::Int64);
    assert_eq!(
        all.variables["SCC"].dtype,
        DType::Str,
        "a code column stays text"
    );
    assert_eq!(all.variables["meanBaseRate"].dtype, DType::Str);

    // Now the way a document actually reads it: three columns of nine, with the
    // decimal-text rate declared.
    let want = ["polProcessID", "SCC", "meanBaseRate"];
    let ds = configured(json!({"float_columns": ["meanBaseRate"]}))
        .read_native(
            &path,
            &want.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
            &Selection::All,
        )
        .expect("decode a projection of a real MOVES table");

    let mut got: Vec<&str> = ds.variables.keys().map(String::as_str).collect();
    got.sort_unstable();
    let mut expect = want.to_vec();
    expect.sort_unstable();
    assert_eq!(got, expect, "only the projected columns");
    assert_eq!(ds.variables["polProcessID"].dtype, DType::Int64);
    assert_eq!(ds.variables["SCC"].dtype, DType::Str);
    assert_eq!(ds.variables["meanBaseRate"].dtype, DType::Float64);
    assert_eq!(
        ds.variables["SCC"].shape,
        vec![n],
        "the projection keeps every row"
    );

    // The decimal text really parsed: rates are finite and non-negative, and the
    // projected values match the unprojected text cell for cell.
    let rates = f64s(&ds, "meanBaseRate");
    assert!(
        rates.iter().all(|r| r.is_finite() && *r >= 0.0),
        "meanBaseRate decoded to finite non-negative doubles"
    );
    let text = strs(&all, "meanBaseRate");
    for i in [0usize, n / 2, n - 1] {
        let expect: f64 = text[i]
            .trim()
            .parse()
            .expect("the snapshot writes decimal text");
        assert_eq!(rates[i], expect, "row {i}");
    }
    assert_eq!(
        strs(&ds, "SCC"),
        strs(&all, "SCC"),
        "projection does not reorder rows"
    );
    eprintln!("moves_snapshot_smoke: {n} rows from {}", path.display());
}

/// The MOVES snapshot `tables/` directory, from the env override or the
/// sibling checkout the downstream `.esm` project uses. `None` when absent.
fn moves_tables_dir() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("EARTHSCIIO_MOVES_SNAPSHOTS") {
        let p = PathBuf::from(p);
        return first_tables_dir(&p);
    }
    // Walk up looking for a `moves.rs` sibling rather than counting directory
    // levels: this crate is read both from its canonical checkout and from git
    // worktrees at other depths, and a hardcoded `../../../` silently resolves
    // to nothing in one of them — leaving the test inert instead of failing.
    let mut dir: Option<&Path> = Some(Path::new(env!("CARGO_MANIFEST_DIR")));
    while let Some(d) = dir {
        let guess = d.join("moves.rs/characterization/snapshots");
        if guess.is_dir() {
            return first_tables_dir(&guess);
        }
        dir = d.parent();
    }
    None
}

/// The first `<fixture>/tables/` under `root`, or `root` itself if it is one.
fn first_tables_dir(root: &Path) -> Option<PathBuf> {
    if root.join("tables").is_dir() {
        return Some(root.join("tables"));
    }
    let mut fixtures: Vec<PathBuf> = std::fs::read_dir(root)
        .ok()?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.join("tables").is_dir())
        .collect();
    fixtures.sort();
    fixtures.into_iter().next().map(|p| p.join("tables"))
}

/// The `.parquet` under `dir` whose name ends in `__<suffix>`.
fn find_table(dir: &Path, suffix: &str) -> Option<PathBuf> {
    let want = format!("__{suffix}.parquet");
    let mut hits: Vec<PathBuf> = std::fs::read_dir(dir)
        .ok()?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.to_string_lossy().ends_with(&want))
        .collect();
    hits.sort();
    hits.into_iter().next()
}

/// A scratch directory that cleans itself up.
fn tempdir() -> tempfile::TempDir {
    tempfile::tempdir().expect("tempdir")
}
