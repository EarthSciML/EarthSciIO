---
title: "csv_reader"
description: "Stable type · Native arrays and readers"
type: "api"
api_tabs: true
---


**Tier:** stable · **Kind:** type · **Topic:** [Native arrays and readers](/generated/api/topics/native-arrays)

| Python | Julia | Rust |
|---|---|---|
| `CSVReader` | `CSVReader` | `CsvReader` |


<div class="api-tabs">

<div class="api-tab" data-lang="python" data-label="Python">
<p class="api-tab-label">Python</p>

```python
class CSVReader
CSVReader.formats() -> list[str]
CSVReader.extensions() -> list[str]
CSVReader.open(blob_path: Any) -> Any
CSVReader.read_native(handle: Any, variables: Sequence[str] | None = None, select: Any | None = None, *, numeric_columns: Sequence[str] | None = None, delimiter: str | None = ',', header_row: int | None = 0, **_: Any) -> NativeDataset
```

The active `csv` reader — a non-NetCDF format behind the same registry.

Columns named in `numeric_columns` parse to `float64` 1-D arrays keyed by
the column (`file_variable`) name; every other column is returned as a
`list` of `str`. All fields carry the single dimension `index`; there
are no coordinates.

`numeric_columns` is REQUIRED by the loader spec and is not inferred: the
corpus `location_id` column is digit-only text (`"1"`/`"2"`) yet must
stay a string, so "parses as a number" is not a safe signal. When it is
`None` the reader falls back to best-effort inference (every value parses as
a float), which the loader/`.esm` node should override. Quoted fields with
embedded delimiters are handled by `csv`; matches the Julia `CSVReader`.

**Members**

- `read_native` — Decode a delimited-text blob into a [`NativeDataset`](/generated/api/ref/native_dataset) of points.

[Source](https://github.com/EarthSciML/EarthSciIO/blob/065acc28eb3328a3fa3a580d78e7222f1e92f6bf/earthsciio/readers.py#L540)

**Example** (`provider`)

```python
source = DataSource(
    "stations",
    "csv",  # the format-registry key that selects the reader
    (FIXTURES / "stations.csv").as_uri(),
    variables=["value", "location_id"],
    reader_kwargs={"numeric_columns": ["latitude", "longitude", "value"]},
)
data = Provider(source, cache).materialize()
assert isinstance(data, NativeDataset)
assert data.variable_names() == ["location_id", "value"]
assert data["value"].data.dtype == np.float64
assert list(data["value"].data) == [152.3, 168.7, 98.1, 110.4]
# Digit-only text columns stay strings: nothing is inferred.
assert data["location_id"].data == ["1", "1", "2", "2"]
```

<p class="api-src">From <a href="https://github.com/EarthSciML/EarthSciIO/blob/065acc28eb3328a3fa3a580d78e7222f1e92f6bf/docs/api/examples/python/provider.py#L15-L28">docs/api/examples/python/provider.py</a></p>

</div>

<div class="api-tab" data-lang="julia" data-label="Julia">
<p class="api-tab-label">Julia</p>

```julia
struct CSVReader <: Reader
CSVReader()
```

```julia
CSVReader()
```

The `csv` format reader — a second reader proving a non-NetCDF format plugs into
`FORMAT_REGISTRY` unchanged (spec/conformance.md). Columns named in
`numeric_columns` parse to `Float64` 1-D arrays keyed by the column
(`file_variable`) name; every other column is returned as a `String` array. All
fields have a single dimension `index`; there are no coordinates.

`numeric_columns` is REQUIRED by the loader spec and is not inferred: the corpus
`location_id` column is digit-only text (`"1"`,`"2"`) yet must stay a string, so
"parses as a number" is not a safe signal. When `numeric_columns === nothing`
the reader falls back to best-effort inference (every value parses as a float),
which the loader/`.esm` node should override. Quoted fields with embedded
delimiters are not handled (the points corpus has none); add that when a loader
needs it.

`header_row` is the **0-based** non-blank row holding the column names (default
0), the same index in every track; rows before it are dropped. The text is UTF-8
and a leading byte-order mark is dropped. A header that repeats a column name,
or a `numeric_columns` entry the header lacks, is an error. A `nothing` option
is unset.

[Source](https://github.com/EarthSciML/EarthSciIO/blob/065acc28eb3328a3fa3a580d78e7222f1e92f6bf/julia/src/readers.jl#L739)

**Example** (`provider`)

```julia
url = "file://" * abspath(joinpath(FIXTURES, "stations.csv"))
p = const_provider(
    cache, url; format = "csv", source_loader = "stations",
    variables = ["value", "location_id"],
    reader_kwargs = (numeric_columns = ["latitude", "longitude", "value"],)
)
@assert is_const(p) && refresh_times(p) == Float64[]  # a CONST provider never refreshes
data = materialize(p)
@assert data isa NativeDataset
@assert Set(variable_names(data)) == Set(["location_id", "value"])
@assert data["value"].data == [152.3, 168.7, 98.1, 110.4]
# Digit-only text columns stay strings: nothing is inferred.
@assert data["location_id"].data == ["1", "1", "2", "2"]
```

<p class="api-src">From <a href="https://github.com/EarthSciML/EarthSciIO/blob/065acc28eb3328a3fa3a580d78e7222f1e92f6bf/docs/api/examples/julia/provider.jl#L8-L20">docs/api/examples/julia/provider.jl</a></p>

</div>

<div class="api-tab" data-lang="rust" data-label="Rust">
<p class="api-tab-label">Rust</p>

```rust
pub struct CsvReader {
    /* private fields */
}

impl CsvReader { pub fn new() -> Self }

impl CsvReader { pub fn numeric_columns<I, S>(self, names: I) -> Self where I: IntoIterator<Item = S>, S: Into<String> }

impl CsvReader { pub fn delimiter(self, delimiter: u8) -> Self }

impl CsvReader { pub fn header_row(self, row: usize) -> Self }
```

The `csv` format reader. The default instance (no options) infers numeric
columns, splits on `,` and reads the header from row 0.

[Source](https://github.com/EarthSciML/EarthSciIO/blob/065acc28eb3328a3fa3a580d78e7222f1e92f6bf/rust/src/format/csv_reader.rs#L40) · [Rust docs](https://docs.rs/earthsciio/latest/earthsciio/struct.CsvReader.html)

</div>

</div>
