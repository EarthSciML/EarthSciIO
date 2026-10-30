---
title: "parquet_reader"
description: "Stable type · Native arrays and readers"
type: "api"
api_tabs: true
---


**Tier:** stable · **Kind:** type · **Topic:** [Native arrays and readers](/generated/api/topics/native-arrays)

| Python | Julia | Rust |
|---|---|---|
| `ParquetReader` | `ParquetReader` | `ParquetReader` |


<div class="api-tabs">

<div class="api-tab" data-lang="python" data-label="Python">
<p class="api-tab-label">Python</p>

```python
class ParquetReader
ParquetReader.formats() -> list[str]
ParquetReader.extensions() -> list[str]
ParquetReader.open(blob_path: Any) -> Any
ParquetReader.read_native(handle: Any, variables: Sequence[str] | None = None, select: Any | None = None, *, float_columns: Sequence[str] | None = None, null_int: int | None = None, null_string: str | None = None, **_: Any) -> NativeDataset
```

The active `parquet` reader — an Apache Parquet file as a flat table.

Decode is delegated to **pyarrow** (`pyarrow.parquet`), imported lazily
so a base install stays lean. pyarrow is the reference Apache Arrow/Parquet
implementation and the peer of the Rust track's `parquet` crate, so the
Arrow type surface below is literally the same enum on both sides — the
reason cross-language parity is cheap here. Nothing about the format is
re-implemented; what this reader owns is the mapping onto the NATIVE-ARRAY
contract (`spec/conformance.md` §3, "Parquet decode notes").

**A table, not a grid.** Like the `csv`/`ff10`/`shapefile` readers,
every column becomes a rank-1 field over `index`, keyed by its **on-disk
column name**, and the dataset carries **no coordinates**. `index` has
length `num_rows`. A zero-row file is **typed, not absent**: the schema
lives in the footer, so every column comes back empty with its declared
dtype — which matters because most of a MOVES fixture's ~770 tables are
empty and a document binding one must still see the array it named.

**Type mapping** (a total function of the Arrow type):

==========================================  =========
Arrow type                                  dtype
==========================================  =========
`Boolean`                                 `bool`
`Int8`/`Int16`/`Int32`/`UInt8`/`UInt16`  `int32`
`Int64`/`UInt32`/`UInt64`             `int64`
`Float16`/`Float32`/`Float64`         `float64`
`Decimal128`/`Decimal256`               `float64` (unscaled ÷ 10^scale)
`Utf8`/`LargeUtf8`/`Utf8View`         `string`
`Date32`/`Time32`                       `int32` (raw, **undecoded**)
`Date64`/`Time64`/`Timestamp`/`Duration`  `int64` (raw, **undecoded**)
`Dictionary(_, V)`                        as `V`, expanded
`Null`                                    `float64`, all `NaN`
==========================================  =========

The narrow/wide integer split is the [`NetCDFReader`](/generated/api/ref/netcdf_reader)'s, **verbatim**,
so a MOVES `int32` ID column and a CF `int32` time axis cannot drift
apart. A `uint64` above `int64` max is an error naming the column and
row, never a wraparound into a negative ID. Nested and binary columns have
no rank-1 reading: naming one in `variables` is an error, unrequested it is
simply not a field (as the NetCDF reader skips its non-numeric variables).

**Null policy.** Nearly every Parquet column is nullable in its schema
whether or not it holds a null — a table exported from a relational database
usually marks every column nullable — so nullability cannot pick the dtype.
A null in a float column is `NaN`; a null in an integer/string/boolean
column is an error, opened only by a declared `null_int` / `null_string`.

**Column projection pushes down.** `variables` becomes a pyarrow column
projection, so only those column chunks are read off disk — not
read-then-discarded. These tables are wide (a MOVES table runs to dozens of
columns) and a document typically wants three. Empty `variables` reads
every column; a requested name absent from the file is an error listing what
is present, never a silently missing array.

READER-ONLY (Risk R3): no row selection, no filtering, no code mapping, no
name remap, no unit conversion. esm-spec §8.9 puts `codes`,
`record_filter`, `select` and `extent` downstream of the decode, and
`select` never reaches a whole-file reader at all (the Provider hands one
whole-file selection unconditionally).

`reader_kwargs`: `float_columns=[...]` forces the named columns to
`float64` whatever their on-disk type — the Parquet twin of the
`ShapefileReader`'s `numeric_columns` — and does two jobs, because they
are the same statement about the source: an integer column that is really a
measurement (missing cells → `NaN` rather than a sentinel), **and** a
column of fixed-decimal **text**. A corpus that needs byte-reproducible
floats stores them as decimal strings rather than IEEE doubles — the MOVES
snapshots write `meanBaseRate` as `"261.000000000000"` — and this is how
a document says so. `null_int=n` / `null_string="…"` declare the null
substitutes above. None of the three enters the cache key.

**Members**

- `read_native` — Decode a Parquet blob into a flat table of rank-1 `index` fields.

[Source](https://github.com/EarthSciML/EarthSciIO/blob/a49538bbd6ec318dde369d3496f6e8880586a73f/earthsciio/readers.py#L1795)

</div>

<div class="api-tab" data-lang="julia" data-label="Julia">
<p class="api-tab-label">Julia</p>

```julia
struct ParquetReader <: Reader
ParquetReader()
```

```julia
ParquetReader()
```

The `parquet` format reader — an Apache Parquet file as a **flat table**.

Every column becomes a rank-1 field over `index`, keyed by its **on-disk column
name**, and a table produces **no coordinates** (like [`CSVReader`] and
[`ZarrReader`]). `index` has length `num_rows`; a zero-row file still yields
every column, empty and correctly typed (the schema is in the footer, so an
empty table is TYPED, not absent — most of a MOVES fixture's ~770 tables are
empty, and a document binding one must still see the array it named).

Decode is delegated to **Parquet2.jl**, loaded LAZILY via a weakdep extension
(`EarthSciIOParquet2Ext`, active on `using Parquet2`) — mirroring the shapefile
reader's `Shapefile.jl` and the Python track's lazy `pyarrow` import. Calling
`read_native` without `using Parquet2` throws a clear install hint.

**Type mapping** (spec/conformance.md §3, and [`_parquet_kind`]): Parquet carries
an explicit logical type per column, so the dtype is a total function of it —
`Boolean`→Bool; `Int8`/`Int16`/`Int32`/`UInt8`/`UInt16`→Int32 and
`Int64`/`UInt32`/`UInt64`→Int64 (the SAME narrow/wide split as [`NetCDFReader`]);
floats and `Decimal`→Float64 (unscaled ÷ 10^scale); `Utf8`→String; a
dictionary-encoded column reads as its VALUE type, expanded to one value per row;
an all-null column is Float64/all-NaN. **Temporal columns ride as their RAW
stored integer**, undecoded (`Date32`/millisecond-`Time32`→Int32,
`Date64`/`Time64`/`Timestamp`→Int64): the unit and any timezone are not applied,
because an epoch offset → instant is ESS's job (Risk R3). A `uint64` past
`typemax(Int64)` is an error naming the column and row, never a wraparound.
Nested and binary columns have no rank-1 reading: naming one in `variables` is an
error, and unrequested it is simply not a field.

**Null policy.** Nearly every Parquet column is nullable in its schema whether or
not it holds a null (a table exported from a relational database usually marks
every column nullable), so nullability cannot pick the dtype. A null in a FLOAT
column becomes `NaN`; a null in an INTEGER, STRING or BOOLEAN column is an
ERROR naming the column and the row, because those types have no NaN and any
default would be a real value silently standing in for a missing one.

`reader_kwargs`: `variables=[...]` is the projection and is PUSHED DOWN — only
those column chunks are read off disk (these tables are wide and a document
typically wants three columns); an absent name is an error listing what is
present. `float_columns=[...]` forces the named columns to Float64 whatever
their on-disk type — the Parquet twin of [`ShapefileReader`]'s `numeric_columns`
— and does double duty: an integer column whose missing cells must be `NaN`, AND
a column of fixed-decimal TEXT (a corpus needing byte-reproducible floats stores
them as decimal strings; the MOVES snapshots write `meanBaseRate` as
`"261.000000000000"`), trimmed and parsed, blank→`NaN`, anything else
unparseable an error naming column/row/text. `null_int=n` substitutes an integer
sentinel for a null AND reports it back in the field's `attrs["fill_value"]`
(an integer sentinel cannot be NaN, so it survives exactly as a CF integer fill
does); `null_string=s` substitutes text, which is then indistinguishable from a
real cell holding it — which is why the document has to choose it. A boolean has
no such option.

READER-ONLY (Risk R3): row selection, `record_filter`, `codes` and `extent` are
esm-spec §8.9 work DOWNSTREAM of the decode (and a whole-file reader never even
sees a `select`), and there is no name remap and no unit conversion.

> **Parquet2.jl backend limits**
>
> A file containing a nested (`list`/`struct`/`map`) column cannot be OPENED by
> Parquet2.jl at all, so such a file errors rather than decoding its readable
> columns — the one place this track cannot reach the §3 contract's "unrequested,
> it is simply not a field". Parquet2.jl decodes a `Timestamp` into a
> millisecond-resolution `DateTime` before this reader sees it, so the raw
> integer of a MICROS/NANOS timestamp column is recovered only to millisecond
> granularity. A `Float16` column is not decoded at all, so it is simply not a
> field here where the other two tracks return float64. And it decodes a
> `Decimal` into a `Dec64` (16 significant decimal digits) during page decode,
> so a wider `Decimal128`/`Decimal256` cannot be read here at the full precision
> the Rust and Python tracks get from the unscaled integer; within binary64's
> exact range the three agree bit for bit (see `ext/EarthSciIOParquet2Ext.jl`).
>
> One further Parquet2.jl defect is REPAIRED rather than tolerated: it folds a
> `FIXED_LEN_BYTE_ARRAY` decimal's big-endian two's-complement bytes into an
> `Int64` with no sign extension, so every negative cell in a column narrower
> than 8 bytes decoded as its huge unsigned reinterpretation. The extension
> undoes that exactly. The one width it cannot reach is 7 bytes (pyarrow
> precision 15-16), where the unsigned fold overflows `Dec64` inside the
> backend; a negative there is an error naming the limit, never a wrong number.

[Source](https://github.com/EarthSciML/EarthSciIO/blob/a49538bbd6ec318dde369d3496f6e8880586a73f/julia/src/readers.jl#L1763)

</div>

<div class="api-tab" data-lang="rust" data-label="Rust">
<p class="api-tab-label">Rust</p>

```rust
pub struct ParquetReader {
    /* private fields */
}

impl ParquetReader { pub fn new() -> Self }

impl ParquetReader { pub fn float_columns<I, S>(self, columns: I) -> Self where I: IntoIterator<Item = S>, S: Into<String> }

impl ParquetReader { pub fn null_int(self, sentinel: i64) -> Self }

impl ParquetReader { pub fn null_string(self, text: impl Into<String>) -> Self }
```

The active `parquet` reader: an Apache Parquet file as a flat table on
`index`. See the module docs for the type and null policies.

[Source](https://github.com/EarthSciML/EarthSciIO/blob/a49538bbd6ec318dde369d3496f6e8880586a73f/rust/src/format/parquet.rs#L138) · [Rust docs](https://docs.rs/earthsciio/latest/earthsciio/struct.ParquetReader.html)

</div>

</div>
