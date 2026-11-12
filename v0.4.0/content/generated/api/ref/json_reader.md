---
title: "json_reader"
description: "Stable type · Other"
type: "api"
api_tabs: true
---


**Tier:** stable · **Kind:** type · **Topic:** [Other](/generated/api/topics/other)

| Python | Julia | Rust |
|---|---|---|
| `JSONReader` | `JSONReader` | `JsonReader` |


<div class="api-tabs">

<div class="api-tab" data-lang="python" data-label="Python">
<p class="api-tab-label">Python</p>

```python
class JSONReader
JSONReader.formats() -> list[str]
JSONReader.extensions() -> list[str]
JSONReader.open(blob_path: Any) -> Any
JSONReader.read_native(handle: Any, variables: Sequence[str] | None = None, *, records_path: Sequence[str] | None = None, numeric_columns: Sequence[str] | None = None, **unknown: Any) -> NativeDataset
```

The active `json` reader: a JSON array of records becomes a points table.

The records are the array at `records_path` (a list of object keys from the
document root; default `[]`, the root itself). Each record must be an
object. Its fields are its leaves: a nested object is flattened, its keys
joined with `.` (`{"coordinates": {"latitude": 1}}` is the field
`coordinates.latitude`). The fields are the union over all records; a
field absent from a record is `null` there. Each field is one rank-1
array on the dimension `index` (length = number of records); there are no
coordinates.

A field's type comes from its non-null values: all numbers → `float64`
(`null` → NaN); all strings → string (`null` → `""`); all booleans →
`bool` (a `null` is an error, a boolean has no missing value); all
`null` → `float64` of NaN. A field mixing kinds is an error. A field
named in `numeric_columns` is `float64` whatever its cells: a number as
is, `null` → NaN, a string holding a decimal number (after trimming ASCII
whitespace) parsed, anything else an error. `variables` projects the
fields; a field that is not decoded may hold anything, an array included.

**Members**

- `read_native` — Decode a JSON blob into a [`NativeDataset`](/generated/api/ref/native_dataset) of points.

[Source](https://github.com/EarthSciML/EarthSciIO/blob/770dbc22d557e6e2ae87d9bccd7386f45764e9d0/earthsciio/readers.py#L740)

</div>

<div class="api-tab" data-lang="julia" data-label="Julia">
<p class="api-tab-label">Julia</p>

```julia
struct JSONReader <: Reader
JSONReader()
```

```julia
JSONReader()
```

The `json` format reader: a JSON array of records becomes a points table, as the
Python `JSONReader` and the Rust `JsonReader` decode it (one contract, pinned by
`conformance/corpus/scenarios/json-reader.json`; `spec/registries.md` §2).

The records are the array at `records_path` (object keys from the document root;
default none, the root itself). Each record must be an object; its fields are its
leaves, a nested object flattened with its keys joined by `.`
(`coordinates.latitude`). The fields are the union over the records; a field a
record lacks is `null` there. Each field is one rank-1 array on `index`; there
are no coordinates.

A field's type comes from its non-null values: numbers → `Float64` (`null` →
`NaN`); strings → `String` (`null` → `""`); booleans → `Bool` (a `null` is an
error); all `null` → `Float64` of `NaN`. Mixed kinds are an error. A field in
`numeric_columns` is `Float64` whatever its cells: a number as is, `null` → `NaN`,
a string holding a decimal number (after trimming ASCII whitespace) parsed,
anything else an error. `variables` projects the fields, and a field that is not
decoded may hold anything, an array included.

[Source](https://github.com/EarthSciML/EarthSciIO/blob/770dbc22d557e6e2ae87d9bccd7386f45764e9d0/julia/src/readers.jl#L825)

</div>

<div class="api-tab" data-lang="rust" data-label="Rust">
<p class="api-tab-label">Rust</p>

```rust
pub struct JsonReader {
    /* private fields */
}

impl JsonReader { pub fn new() -> Self }

impl JsonReader { pub fn records_path<I, S>(self, keys: I) -> Self where I: IntoIterator<Item = S>, S: Into<String> }

impl JsonReader { pub fn numeric_columns<I, S>(self, names: I) -> Self where I: IntoIterator<Item = S>, S: Into<String> }
```

The `json` format reader. The default instance reads the records from the
document root and infers every field's type.

[Source](https://github.com/EarthSciML/EarthSciIO/blob/770dbc22d557e6e2ae87d9bccd7386f45764e9d0/rust/src/format/json_reader.rs#L47) · [Rust docs](https://docs.rs/earthsciio/latest/earthsciio/struct.JsonReader.html)

</div>

</div>
