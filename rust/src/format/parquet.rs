//! `parquet` format reader — an Apache Parquet file as a **flat table**, one
//! [`NativeField`] per column on a single `index` dimension. Decoded by
//! arrow-rs's own `parquet` crate; no format parsing is hand-rolled here, and
//! what this reader owns is the **mapping onto the native-array contract**.
//!
//! # Why a reader and not a conversion stage
//!
//! Parquet is the columnar interchange format tabular science data actually
//! ships in, and an `.esm` document reaches array data only through a
//! `data_sources` entry. Without this reader every Parquet corpus needs a
//! pre-pass that rewrites it as Zarr or NetCDF — a stage that costs a copy of
//! the data, has to be kept in sync with it, and is exactly the kind of
//! imperative setup step that stops a document being self-sufficient. The
//! motivating corpus is the EPA MOVES/NONROAD oracle snapshots (~200 non-empty
//! `MOVESExecution` input tables plus the expected output, per fixture), which
//! are Parquet throughout.
//!
//! # The shape of the result
//!
//! A Parquet file is a table, not a grid, so — like [`Ff10Reader`](super::Ff10Reader)
//! and [`ShapefileReader`](super::ShapefileReader) — every column becomes a
//! rank-1 field over `index`, keyed by its **on-disk column name**, and the
//! dataset carries no coordinates. `index` has length `num_rows`; a zero-row
//! file still yields every column, empty, with its declared dtype (the schema
//! is in the footer, so an empty table is typed, not absent). Nested columns
//! (list/struct/map/union) have no rank-1 reading and are not supported.
//!
//! # Type mapping (`spec/conformance.md` §3)
//!
//! Parquet carries an explicit logical type per column, so — unlike the CF
//! attribute sniffing the NetCDF reader has to do — the mapping is a total
//! function of the Arrow type. The integer split is deliberately the **same**
//! as [`NetcdfReader`](super::NetcdfReader)'s: narrow integers are `int32`,
//! wide ones `int64`.
//!
//! | Arrow type | `DType` |
//! |---|---|
//! | `Boolean` | `Bool` |
//! | `Int8`/`Int16`/`Int32`/`UInt8`/`UInt16` | `Int32` |
//! | `Int64`/`UInt32`/`UInt64` | `Int64` |
//! | `Float16`/`Float32`/`Float64` | `Float64` |
//! | `Decimal128`/`Decimal256` | `Float64` (unscaled value ÷ 10^scale) |
//! | `Utf8`/`LargeUtf8`/`Utf8View` | `Str` |
//! | `Date32`/`Time32` | `Int32` (raw, **undecoded**) |
//! | `Date64`/`Time64`/`Timestamp`/`Duration` | `Int64` (raw, **undecoded**) |
//! | `Dictionary(_, V)` | as `V` (the categorical is expanded to its values) |
//! | `Null` | `Float64`, all `NaN` |
//!
//! A temporal column is carried **verbatim as its raw integer**, the same rule
//! the NetCDF reader applies to a CF time axis: decoding an epoch offset to a
//! wall-clock instant is ESS's job, not a reader's (Risk R3). The Arrow unit
//! (`s`/`ms`/`us`/`ns`) and any timezone are therefore NOT applied and NOT
//! reported — a document that needs them must state them itself.
//!
//! A `UInt64` value above `i64::MAX` is an error naming the column and row, not
//! a wraparound.
//!
//! # Null policy
//!
//! Nearly every Parquet column is nullable in its schema whether or not it holds
//! a null (a table exported from a relational database usually marks every
//! column nullable), so nullability alone cannot pick the dtype. The policy is
//! about **values**:
//!
//! - a null in a **float** column (including a `Decimal`, a `Null` column, and
//!   any column forced float by [`float_columns`](ParquetReader::float_columns))
//!   becomes `NaN`, the same fold CF `_FillValue` gets, and `fill_value` stays
//!   `None`;
//! - a null in an **integer**, **string** or **boolean** column is an **error**
//!   naming the column and the row. There is no NaN in those types, so any
//!   default would be a real value silently standing in for a missing one —
//!   the failure mode that surfaces much later as wrong numbers.
//!
//! Two reader options open that gate **explicitly**, per
//! [`null_int`](ParquetReader::null_int) / [`null_string`](ParquetReader::null_string):
//! a declared integer sentinel is substituted and reported back in
//! [`NativeField::fill_value`] (an integer sentinel cannot be `NaN`, so it
//! survives, exactly as in the NetCDF reader); a declared string stands in for a
//! null text cell and is distinguishable from `""` only by the document that
//! chose it. Boolean nulls have no such option — declare the column in
//! `float_columns` if a third state is genuinely meant.
//!
//! # Column projection is pushed down
//!
//! The `variables` the loader declares become a `parquet` [`ProjectionMask`], so
//! only those column chunks are read off disk and decoded. This is not an
//! optimization detail for the MOVES corpus: its widest tables run to dozens of
//! columns and a document typically wants three of them. An empty `variables`
//! reads every column. A requested name that is not in the file is an error
//! listing what is present — never a silently missing array.
//!
//! # What this reader does NOT do (Risk R3)
//!
//! Only `reader_options` from a `data_sources` entry reaches a reader. The
//! esm-spec §8.9 pipeline puts `codes` (text column → numbers), `record_filter`
//! (which rows are real), `select` (which rows are delivered) and `extent`
//! **downstream** of the decode, and `select` in particular never reaches a
//! whole-file reader at all: [`crate::Provider`] hands one [`Selection::All`]
//! unconditionally. So this reader reads whole and leaves row selection,
//! filtering and code mapping to ESS, like every other whole-file reader here.
//! No name remap and no `unit_conversion` either — arrays are keyed by the
//! on-disk column name.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fs::File;
use std::path::Path;
use std::sync::Arc;

use arrow_array::cast::AsArray;
use arrow_array::types::{
    Date32Type, Date64Type, Decimal128Type, Decimal256Type, DurationMicrosecondType,
    DurationMillisecondType, DurationNanosecondType, DurationSecondType, Float16Type, Float32Type,
    Float64Type, Int16Type, Int32Type, Int64Type, Int8Type, Time32MillisecondType,
    Time32SecondType, Time64MicrosecondType, Time64NanosecondType, TimestampMicrosecondType,
    TimestampMillisecondType, TimestampNanosecondType, TimestampSecondType, UInt16Type, UInt32Type,
    UInt64Type, UInt8Type,
};
use arrow_array::{Array, ArrowPrimitiveType, PrimitiveArray, RecordBatch, RecordBatchReader};
use arrow_schema::{DataType, TimeUnit};
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use parquet::arrow::ProjectionMask;

use crate::error::{Error, Result};

use super::{ArrayData, DType, NativeDataset, NativeField, Reader, Selection};

/// Rows decoded per Arrow batch. Parquet's own default is 1024, which is a lot
/// of per-batch bookkeeping for a wide table; 8192 is the arrow-rs recommended
/// working size and is purely a throughput knob — it cannot change what decodes.
const BATCH_ROWS: usize = 8192;

/// The active `parquet` reader: an Apache Parquet file as a flat table on
/// `index`. See the module docs for the type and null policies.
#[derive(Debug, Clone, Default)]
pub struct ParquetReader {
    float_columns: BTreeSet<String>,
    null_int: Option<i64>,
    null_string: Option<String>,
}

impl ParquetReader {
    /// The default reader: types map straight off the Arrow schema, and a null
    /// in a non-float column is an error.
    pub fn new() -> Self {
        Self::default()
    }

    /// Force the named columns to `float64` whatever their on-disk type, so
    /// their nulls fold to `NaN` — the Parquet twin of the shapefile reader's
    /// `numeric_columns`.
    ///
    /// Two distinct jobs, one option, because they are the same statement about
    /// the source ("this column is a float64 measurement"):
    ///
    /// - an integer column that is really a measurement, and whose missing cells
    ///   must become `NaN` rather than a sentinel;
    /// - a column of **decimal text**. Corpora that need byte-reproducible
    ///   floats often store them as fixed-decimal strings rather than IEEE
    ///   doubles — the MOVES snapshots write `meanBaseRate` as
    ///   `"261.000000000000"` — and this is how a document says so. The text is
    ///   trimmed and parsed; an empty (or all-whitespace) cell is `NaN`, matching
    ///   the FF10 and shapefile "blank → NaN" rule; anything else unparseable is
    ///   an error naming the column, the row and the offending text.
    pub fn float_columns<I, S>(mut self, columns: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.float_columns = columns.into_iter().map(Into::into).collect();
        self
    }

    /// Substitute `sentinel` for a null in any **integer** column and report it
    /// as the field's [`NativeField::fill_value`]. Without this a null integer
    /// is an error (see the module docs' null policy).
    pub fn null_int(mut self, sentinel: i64) -> Self {
        self.null_int = Some(sentinel);
        self
    }

    /// Substitute `text` for a null in any **string** column. Without this a null
    /// text cell is an error. The substitute is indistinguishable from a real
    /// cell holding the same text — which is why the document has to choose it.
    pub fn null_string(mut self, text: impl Into<String>) -> Self {
        self.null_string = Some(text.into());
        self
    }

    /// Build a configured reader from a loader's declared `reader_options` — the
    /// declared form of the builders above, so an `.esm` document says how its
    /// Parquet decodes rather than a caller hand-injecting a reader. Recognised
    /// keys:
    ///
    /// | key | type | meaning |
    /// |---|---|---|
    /// | `float_columns` | array of string | [`ParquetReader::float_columns`] |
    /// | `null_int` | integer | [`ParquetReader::null_int`] |
    /// | `null_string` | string | [`ParquetReader::null_string`] |
    ///
    /// Any other key — or a recognised key of the wrong type — is an error
    /// (esm-spec §8.9.1: an unrecognised reader option MUST NOT be ignored).
    fn from_options(options: &serde_json::Map<String, serde_json::Value>) -> Result<Self> {
        let mut r = ParquetReader::new();
        for (k, v) in options {
            match k.as_str() {
                "float_columns" => {
                    let arr = v.as_array().ok_or_else(|| {
                        fmt_err(format!("reader option {k:?} must be an array of strings"))
                    })?;
                    let names: Result<Vec<String>> = arr
                        .iter()
                        .map(|m| {
                            m.as_str().map(str::to_string).ok_or_else(|| {
                                fmt_err(format!("reader option {k:?} must be an array of strings"))
                            })
                        })
                        .collect();
                    r = r.float_columns(names?);
                }
                "null_int" => {
                    let n = v.as_i64().ok_or_else(|| {
                        fmt_err(format!("reader option {k:?} must be an integer"))
                    })?;
                    r = r.null_int(n);
                }
                "null_string" => {
                    let s = v
                        .as_str()
                        .ok_or_else(|| fmt_err(format!("reader option {k:?} must be a string")))?;
                    r = r.null_string(s);
                }
                other => {
                    return Err(fmt_err(format!(
                        "unknown reader option {other:?}; parquet takes float_columns, \
                         null_int, null_string"
                    )));
                }
            }
        }
        Ok(r)
    }
}

impl Reader for ParquetReader {
    fn formats(&self) -> &'static [&'static str] {
        &["parquet"]
    }

    fn extensions(&self) -> &'static [&'static str] {
        &["parquet", "parq", "pq"]
    }

    fn configured(
        &self,
        options: &serde_json::Map<String, serde_json::Value>,
    ) -> Result<Option<Arc<dyn Reader>>> {
        if options.is_empty() {
            return Ok(None);
        }
        Ok(Some(Arc::new(ParquetReader::from_options(options)?)))
    }

    fn read_native(
        &self,
        blob_path: &Path,
        variables: &[String],
        _select: &Selection,
    ) -> Result<NativeDataset> {
        // `Selection` never reaches a whole-file reader: the Provider hands one
        // `Selection::All` unconditionally, and row selection is esm-spec §8.9.2
        // work that happens downstream of the decode. The whole table is read.
        let file =
            File::open(blob_path).map_err(|e| Error::io(Some(blob_path.to_path_buf()), e))?;
        let builder = ParquetRecordBatchReaderBuilder::try_new(file).map_err(pq_err)?;

        // --- projection pushdown -------------------------------------------
        // Resolve the requested names against the PARQUET schema's root fields,
        // so only those column chunks are read off disk. An unknown name is an
        // error naming what is present, never a silently absent array.
        let roots: Vec<String> = builder
            .parquet_schema()
            .root_schema()
            .get_fields()
            .iter()
            .map(|f| f.name().to_string())
            .collect();
        let builder = if variables.is_empty() {
            builder
        } else {
            let mut missing: Vec<&str> = Vec::new();
            let mut want: BTreeSet<usize> = BTreeSet::new();
            for v in variables {
                match roots.iter().position(|r| r == v) {
                    Some(i) => {
                        want.insert(i);
                    }
                    None => missing.push(v.as_str()),
                }
            }
            if !missing.is_empty() {
                missing.sort_unstable();
                missing.dedup();
                return Err(fmt_err(format!(
                    "requested variables not in the parquet file: {missing:?}; present: {roots:?}"
                )));
            }
            let mask = ProjectionMask::roots(builder.parquet_schema(), want);
            builder.with_projection(mask)
        };

        let reader = builder
            .with_batch_size(BATCH_ROWS)
            .build()
            .map_err(pq_err)?;
        let schema = reader.schema();

        // Accumulators are created from the SCHEMA, not from the first batch, so
        // a zero-row file still yields every column — empty and correctly typed.
        let forced: HashSet<&str> = self.float_columns.iter().map(String::as_str).collect();
        let mut accs: BTreeMap<String, (Acc, DType)> = BTreeMap::new();
        let mut order: Vec<String> = Vec::new();
        for f in schema.fields() {
            let name = f.name();
            let Some(dt) = target_dtype(f.data_type(), forced.contains(name.as_str())) else {
                // A column with no rank-1 reading. Silently skipping one that was
                // explicitly requested would hand back a dataset missing an array
                // the document named, so that case is an error; in read-everything
                // mode the column is simply not a native field (the NetCDF reader
                // skips its non-numeric variables the same way).
                if variables.iter().any(|v| v == name) {
                    return Err(fmt_err(format!(
                        "column {name:?} has arrow type {} , which has no rank-1 native \
                         reading (nested and binary columns are not supported)",
                        f.data_type()
                    )));
                }
                continue;
            };
            accs.insert(name.clone(), (Acc::new(dt), dt));
            order.push(name.clone());
        }

        let mut nrows = 0usize;
        for batch in reader {
            let batch: RecordBatch = batch.map_err(arrow_err)?;
            let bschema = batch.schema();
            for (i, f) in bschema.fields().iter().enumerate() {
                let Some((acc, _)) = accs.get_mut(f.name()) else {
                    continue; // an unsupported column in read-everything mode
                };
                let cells = cells(batch.column(i).as_ref(), f.name(), nrows)?;
                acc.extend(
                    cells,
                    f.name(),
                    nrows,
                    self.null_int,
                    self.null_string.as_deref(),
                )?;
            }
            nrows += batch.num_rows();
        }

        let mut out = NativeDataset::default();
        for name in order {
            let (acc, dtype) = accs
                .remove(&name)
                .expect("accumulator was registered above");
            let data = acc.finish();
            debug_assert_eq!(data.len(), nrows);
            out.variables.insert(
                name,
                NativeField {
                    dtype,
                    dims: vec!["index".to_string()],
                    shape: vec![nrows],
                    // A NaN-folded float carries no surviving sentinel; a declared
                    // integer sentinel does, and is reported like a CF integer fill.
                    fill_value: match dtype {
                        DType::Int32 | DType::Int64 => self.null_int.map(|n| n as f64),
                        _ => None,
                    },
                    data,
                },
            );
        }
        Ok(out)
    }
}

// --------------------------------------------------------------------------- //
// Type mapping
// --------------------------------------------------------------------------- //

/// The [`DType`] an Arrow column maps to, or `None` when it has no rank-1
/// native reading. See the table in the module docs.
fn target_dtype(dt: &DataType, forced_float: bool) -> Option<DType> {
    if forced_float {
        // `float_columns` is a statement about the SOURCE, so it applies to any
        // column that can produce a number — including decimal text.
        return match dt {
            DataType::List(_)
            | DataType::LargeList(_)
            | DataType::ListView(_)
            | DataType::LargeListView(_)
            | DataType::FixedSizeList(_, _)
            | DataType::Struct(_)
            | DataType::Map(_, _)
            | DataType::Union(_, _)
            | DataType::RunEndEncoded(_, _) => None,
            _ => Some(DType::Float64),
        };
    }
    match dt {
        DataType::Boolean => Some(DType::Bool),
        // The narrow/wide integer split is the NetCDF reader's, verbatim.
        DataType::Int8 | DataType::Int16 | DataType::Int32 | DataType::UInt8 | DataType::UInt16 => {
            Some(DType::Int32)
        }
        DataType::Int64 | DataType::UInt32 | DataType::UInt64 => Some(DType::Int64),
        DataType::Float16 | DataType::Float32 | DataType::Float64 => Some(DType::Float64),
        DataType::Decimal128(_, _) | DataType::Decimal256(_, _) => Some(DType::Float64),
        DataType::Utf8 | DataType::LargeUtf8 | DataType::Utf8View => Some(DType::Str),
        // Temporal columns ride as their RAW integer — see the module docs.
        DataType::Date32 | DataType::Time32(_) => Some(DType::Int32),
        DataType::Date64
        | DataType::Time64(_)
        | DataType::Timestamp(_, _)
        | DataType::Duration(_) => Some(DType::Int64),
        // A categorical reads as its value type, expanded.
        DataType::Dictionary(_, value) => target_dtype(value, false),
        // An all-null column has no type of its own; float64 is the one logical
        // type that can represent every cell of it (as NaN).
        DataType::Null => Some(DType::Float64),
        _ => None,
    }
}

/// One decoded cell, before it is coerced into the column's target [`DType`].
#[derive(Debug, Clone)]
enum Cell {
    F(f64),
    I(i64),
    S(String),
    B(bool),
}

/// Decode an Arrow array into per-row cells, `None` for a null. `row0` is the
/// index of the array's first row within the table, so an error names the row
/// as the document would count it.
fn cells(col: &dyn Array, name: &str, row0: usize) -> Result<Vec<Option<Cell>>> {
    /// Read a primitive array, mapping each value through `f`.
    macro_rules! prim {
        ($t:ty, $f:expr) => {{
            let a: &PrimitiveArray<$t> = col.as_primitive::<$t>();
            #[allow(clippy::redundant_closure_call)]
            Ok(a.iter().map(|v| v.map($f)).collect())
        }};
    }

    match col.data_type() {
        DataType::Null => Ok(vec![None; col.len()]),
        DataType::Boolean => Ok(col.as_boolean().iter().map(|v| v.map(Cell::B)).collect()),

        DataType::Int8 => prim!(Int8Type, |v| Cell::I(v as i64)),
        DataType::Int16 => prim!(Int16Type, |v| Cell::I(v as i64)),
        DataType::Int32 => prim!(Int32Type, |v| Cell::I(v as i64)),
        DataType::Int64 => prim!(Int64Type, Cell::I),
        DataType::UInt8 => prim!(UInt8Type, |v| Cell::I(v as i64)),
        DataType::UInt16 => prim!(UInt16Type, |v| Cell::I(v as i64)),
        DataType::UInt32 => prim!(UInt32Type, |v| Cell::I(v as i64)),
        // The one integer width that does not fit: refuse rather than wrap.
        DataType::UInt64 => {
            let a = col.as_primitive::<UInt64Type>();
            let mut out = Vec::with_capacity(a.len());
            for (i, v) in a.iter().enumerate() {
                out.push(match v {
                    None => None,
                    Some(u) => Some(Cell::I(i64::try_from(u).map_err(|_| {
                        fmt_err(format!(
                            "column {name:?} row {}: uint64 value {u} exceeds the int64 \
                             native dtype",
                            row0 + i
                        ))
                    })?)),
                });
            }
            Ok(out)
        }

        DataType::Float16 => prim!(Float16Type, |v| Cell::F(f64::from(v))),
        DataType::Float32 => prim!(Float32Type, |v| Cell::F(v as f64)),
        DataType::Float64 => prim!(Float64Type, Cell::F),

        // Unscaled integer ÷ 10^scale, in double. A negative scale (legal in
        // Arrow) multiplies, which `powi` on a negated exponent handles.
        DataType::Decimal128(_, scale) => {
            let s = 10f64.powi(i32::from(*scale));
            prim!(Decimal128Type, |v: i128| Cell::F(v as f64 / s))
        }
        DataType::Decimal256(_, scale) => {
            let s = 10f64.powi(i32::from(*scale));
            let a = col.as_primitive::<Decimal256Type>();
            let mut out = Vec::with_capacity(a.len());
            for (i, v) in a.iter().enumerate() {
                out.push(match v {
                    None => None,
                    // i256 has no lossless f64 conversion of its own; its decimal
                    // text does, and a decimal256 that does not parse as a double
                    // is a real error rather than a silent infinity.
                    Some(x) => {
                        let t = x.to_string();
                        let f: f64 = t.parse().map_err(|_| {
                            fmt_err(format!(
                                "column {name:?} row {}: decimal256 value {t:?} is not \
                                 representable as float64",
                                row0 + i
                            ))
                        })?;
                        Some(Cell::F(f / s))
                    }
                });
            }
            Ok(out)
        }

        DataType::Utf8 => Ok(col
            .as_string::<i32>()
            .iter()
            .map(|v| v.map(|s| Cell::S(s.to_string())))
            .collect()),
        DataType::LargeUtf8 => Ok(col
            .as_string::<i64>()
            .iter()
            .map(|v| v.map(|s| Cell::S(s.to_string())))
            .collect()),
        DataType::Utf8View => Ok(col
            .as_string_view()
            .iter()
            .map(|v| v.map(|s| Cell::S(s.to_string())))
            .collect()),

        // Temporal: the raw stored integer, undecoded (Risk R3).
        DataType::Date32 => prim!(Date32Type, |v| Cell::I(v as i64)),
        DataType::Date64 => prim!(Date64Type, Cell::I),
        DataType::Time32(TimeUnit::Second) => prim!(Time32SecondType, |v| Cell::I(v as i64)),
        DataType::Time32(_) => prim!(Time32MillisecondType, |v| Cell::I(v as i64)),
        DataType::Time64(TimeUnit::Microsecond) => prim!(Time64MicrosecondType, Cell::I),
        DataType::Time64(_) => prim!(Time64NanosecondType, Cell::I),
        DataType::Timestamp(TimeUnit::Second, _) => prim!(TimestampSecondType, Cell::I),
        DataType::Timestamp(TimeUnit::Millisecond, _) => prim!(TimestampMillisecondType, Cell::I),
        DataType::Timestamp(TimeUnit::Microsecond, _) => prim!(TimestampMicrosecondType, Cell::I),
        DataType::Timestamp(TimeUnit::Nanosecond, _) => prim!(TimestampNanosecondType, Cell::I),
        DataType::Duration(TimeUnit::Second) => prim!(DurationSecondType, Cell::I),
        DataType::Duration(TimeUnit::Millisecond) => prim!(DurationMillisecondType, Cell::I),
        DataType::Duration(TimeUnit::Microsecond) => prim!(DurationMicrosecondType, Cell::I),
        DataType::Duration(TimeUnit::Nanosecond) => prim!(DurationNanosecondType, Cell::I),

        // A categorical: decode the (small) value array once, then index it by
        // key. A null key and a key pointing at a null value are both nulls.
        DataType::Dictionary(key, _) => {
            let keys: Vec<Option<usize>> = match **key {
                DataType::Int8 => dict_keys::<Int8Type>(col),
                DataType::Int16 => dict_keys::<Int16Type>(col),
                DataType::Int32 => dict_keys::<Int32Type>(col),
                DataType::Int64 => dict_keys::<Int64Type>(col),
                DataType::UInt8 => dict_keys::<UInt8Type>(col),
                DataType::UInt16 => dict_keys::<UInt16Type>(col),
                DataType::UInt32 => dict_keys::<UInt32Type>(col),
                DataType::UInt64 => dict_keys::<UInt64Type>(col),
                _ => {
                    return Err(fmt_err(format!(
                        "column {name:?}: dictionary key type {key} is not an integer"
                    )))
                }
            };
            let values = col.as_any_dictionary().values().clone();
            let vcells = cells(values.as_ref(), name, 0)?;
            Ok(keys
                .into_iter()
                .map(|k| k.and_then(|k| vcells.get(k).cloned().flatten()))
                .collect())
        }

        other => Err(fmt_err(format!(
            "column {name:?}: arrow type {other} has no rank-1 native reading"
        ))),
    }
}

/// The key indices of a dictionary array, `None` for a null key.
fn dict_keys<K>(col: &dyn Array) -> Vec<Option<usize>>
where
    K: ArrowPrimitiveType,
    K::Native: TryInto<usize>,
{
    col.as_any_dictionary()
        .keys()
        .as_primitive::<K>()
        .iter()
        .map(|k| k.and_then(|k| k.try_into().ok()))
        .collect()
}

// --------------------------------------------------------------------------- //
// Column accumulation
// --------------------------------------------------------------------------- //

/// A growing column in its target [`DType`].
#[derive(Debug)]
enum Acc {
    F64(Vec<f64>),
    I64(Vec<i64>),
    I32(Vec<i32>),
    Str(Vec<String>),
    Bool(Vec<bool>),
}

impl Acc {
    fn new(dtype: DType) -> Self {
        match dtype {
            DType::Float64 => Acc::F64(Vec::new()),
            DType::Int64 => Acc::I64(Vec::new()),
            DType::Int32 => Acc::I32(Vec::new()),
            DType::Str => Acc::Str(Vec::new()),
            DType::Bool => Acc::Bool(Vec::new()),
        }
    }

    fn finish(self) -> ArrayData {
        match self {
            Acc::F64(v) => ArrayData::F64(v),
            Acc::I64(v) => ArrayData::I64(v),
            Acc::I32(v) => ArrayData::I32(v),
            Acc::Str(v) => ArrayData::Str(v),
            Acc::Bool(v) => ArrayData::Bool(v),
        }
    }

    /// Coerce `cells` into this accumulator's type and append them, applying the
    /// null policy. `row0` is the table row of `cells[0]`.
    fn extend(
        &mut self,
        cells: Vec<Option<Cell>>,
        name: &str,
        row0: usize,
        null_int: Option<i64>,
        null_string: Option<&str>,
    ) -> Result<()> {
        for (i, cell) in cells.into_iter().enumerate() {
            let row = row0 + i;
            match self {
                // A null float is NaN — the same fold a CF _FillValue gets.
                Acc::F64(out) => out.push(match cell {
                    None => f64::NAN,
                    Some(Cell::F(v)) => v,
                    Some(Cell::I(v)) => v as f64,
                    Some(Cell::B(_)) => {
                        return Err(fmt_err(format!(
                            "column {name:?} row {row}: a boolean cell cannot be read as float64"
                        )))
                    }
                    // `float_columns` over decimal TEXT. Blank → NaN, matching the
                    // FF10/shapefile rule; anything else unparseable is an error.
                    Some(Cell::S(s)) => {
                        let t = s.trim();
                        if t.is_empty() {
                            f64::NAN
                        } else {
                            t.parse::<f64>().map_err(|_| {
                                fmt_err(format!(
                                    "column {name:?} row {row}: {s:?} is not a float64 (the \
                                     column is declared in float_columns)"
                                ))
                            })?
                        }
                    }
                }),
                Acc::I64(out) => out.push(int_cell(cell, name, row, null_int)?),
                Acc::I32(out) => {
                    let v = int_cell(cell, name, row, null_int)?;
                    out.push(i32::try_from(v).map_err(|_| {
                        fmt_err(format!(
                            "column {name:?} row {row}: value {v} does not fit the int32 \
                             native dtype"
                        ))
                    })?);
                }
                Acc::Str(out) => out.push(match cell {
                    Some(Cell::S(s)) => s,
                    Some(_) => {
                        return Err(fmt_err(format!(
                            "column {name:?} row {row}: expected a text cell"
                        )))
                    }
                    None => match null_string {
                        Some(s) => s.to_string(),
                        None => return Err(null_err(name, row, "string", "null_string")),
                    },
                }),
                Acc::Bool(out) => out.push(match cell {
                    Some(Cell::B(b)) => b,
                    Some(_) => {
                        return Err(fmt_err(format!(
                            "column {name:?} row {row}: expected a boolean cell"
                        )))
                    }
                    // No sentinel option: a third boolean state is a float64 column.
                    None => {
                        return Err(fmt_err(format!(
                            "column {name:?} row {row} is null, and a boolean native field has \
                             no missing value; declare the column in the `float_columns` reader \
                             option if a third state is meant"
                        )))
                    }
                }),
            }
        }
        Ok(())
    }
}

/// One integer cell under the null policy.
fn int_cell(cell: Option<Cell>, name: &str, row: usize, null_int: Option<i64>) -> Result<i64> {
    match cell {
        Some(Cell::I(v)) => Ok(v),
        Some(_) => Err(fmt_err(format!(
            "column {name:?} row {row}: expected an integer cell"
        ))),
        None => match null_int {
            Some(s) => Ok(s),
            None => Err(null_err(name, row, "integer", "null_int")),
        },
    }
}

/// The refusal a null in a type with no missing value gets. It names the way
/// out, because "declare a sentinel" is a decision only the document can make.
fn null_err(name: &str, row: usize, kind: &str, option: &str) -> Error {
    fmt_err(format!(
        "column {name:?} row {row} is null, and a {kind} native field has no missing value; \
         declare the `{option}` reader option to substitute one, or list the column in \
         `float_columns` to read it as float64 with NaN"
    ))
}

/// Wrap a `parquet` error as the registry's `parquet` format error.
fn pq_err(e: parquet::errors::ParquetError) -> Error {
    fmt_err(e.to_string())
}

/// Wrap an `arrow` error as the registry's `parquet` format error.
fn arrow_err(e: arrow_schema::ArrowError) -> Error {
    fmt_err(e.to_string())
}

/// A `parquet` format error.
fn fmt_err(detail: impl Into<String>) -> Error {
    Error::Format {
        format: "parquet".to_string(),
        detail: detail.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn integer_widths_follow_the_netcdf_split() {
        assert_eq!(target_dtype(&DataType::Int8, false), Some(DType::Int32));
        assert_eq!(target_dtype(&DataType::Int16, false), Some(DType::Int32));
        assert_eq!(target_dtype(&DataType::Int32, false), Some(DType::Int32));
        assert_eq!(target_dtype(&DataType::UInt8, false), Some(DType::Int32));
        assert_eq!(target_dtype(&DataType::UInt16, false), Some(DType::Int32));
        assert_eq!(target_dtype(&DataType::Int64, false), Some(DType::Int64));
        assert_eq!(target_dtype(&DataType::UInt32, false), Some(DType::Int64));
        assert_eq!(target_dtype(&DataType::UInt64, false), Some(DType::Int64));
    }

    #[test]
    fn a_forced_column_is_float_whatever_it_was() {
        assert_eq!(target_dtype(&DataType::Utf8, true), Some(DType::Float64));
        assert_eq!(target_dtype(&DataType::Int64, true), Some(DType::Float64));
        assert_eq!(target_dtype(&DataType::Utf8, false), Some(DType::Str));
    }

    #[test]
    fn a_categorical_reads_as_its_value_type() {
        let d = DataType::Dictionary(Box::new(DataType::Int32), Box::new(DataType::Utf8));
        assert_eq!(target_dtype(&d, false), Some(DType::Str));
    }

    #[test]
    fn nested_columns_have_no_rank_1_reading() {
        let f = Arc::new(arrow_schema::Field::new("x", DataType::Int32, true));
        assert_eq!(target_dtype(&DataType::List(f), false), None);
        assert_eq!(target_dtype(&DataType::Binary, false), None);
    }

    #[test]
    fn unknown_reader_options_are_rejected() {
        let mut m = serde_json::Map::new();
        m.insert("nope".into(), serde_json::Value::Bool(true));
        let Err(err) = ParquetReader::new().configured(&m) else {
            panic!("an unknown reader option must be rejected");
        };
        assert!(matches!(err, Error::Format { .. }));
        assert!(format!("{err}").contains("nope"));
    }

    #[test]
    fn an_empty_option_set_uses_the_reader_as_registered() {
        let m = serde_json::Map::new();
        let Ok(None) = ParquetReader::new().configured(&m) else {
            panic!("an empty option set must reuse the registered reader");
        };
    }

    #[test]
    fn not_a_parquet_file_is_an_error_not_a_panic() {
        use std::io::Write;
        let mut f = tempfile::NamedTempFile::new().unwrap();
        f.write_all(b"definitely not parquet").unwrap();
        f.flush().unwrap();
        let err = ParquetReader::new()
            .read_native(f.path(), &[], &Selection::All)
            .unwrap_err();
        assert!(matches!(err, Error::Format { .. }));
    }
}
