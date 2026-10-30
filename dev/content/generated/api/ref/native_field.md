---
title: "native_field"
description: "Stable type · Native arrays and readers"
type: "api"
api_tabs: true
---


**Tier:** stable · **Kind:** type · **Topic:** [Native arrays and readers](/generated/api/topics/native-arrays)

| Python | Julia | Rust |
|---|---|---|
| `NativeField` | `NativeField` | `NativeField` |


<div class="api-tabs">

<div class="api-tab" data-lang="python" data-label="Python">
<p class="api-tab-label">Python</p>

```python
class NativeField
NativeField(data: Any, dims: Sequence[str], attrs: Mapping[str, Any] | None = None)
NativeField.shape: tuple[int, ...]  # property
NativeField.dtype: Any  # property
```

One native-grid array as a reader decodes it.

**Parameters**

- `data` (`Any`) — The values. Numeric fields are a `numpy.ndarray` whose axes
  correspond, **in order**, to `dims` (on-disk dimension names, file
  order — e.g. `("time", "latitude", "longitude")`); text columns are a
  plain `list` of `str`.
- `dims` (`Sequence[str]`) — Ordered dimension names. Row-major (C order) flattening of `data`
  follows this order, matching the corpus encoding.
- `attrs` (`Mapping[str, Any] | None`) — Decode-relevant metadata the reader must NOT act on but ESS needs —
  notably a CF time axis's `units`/`calendar` (calendar decoding to
  wall-clock instants is ESS's job, never the reader's).
- `Per`, `for`, `integer`

**Members**

- `shape` — Length of each dimension, in `dims` order.
- `dtype` — The numpy dtype for a numeric field, or `None` for a string field.

[Source](https://github.com/EarthSciML/EarthSciIO/blob/a49538bbd6ec318dde369d3496f6e8880586a73f/earthsciio/native.py#L25)

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

<p class="api-src">From <a href="https://github.com/EarthSciML/EarthSciIO/blob/a49538bbd6ec318dde369d3496f6e8880586a73f/docs/api/examples/python/provider.py#L15-L28">docs/api/examples/python/provider.py</a></p>

</div>

<div class="api-tab" data-lang="julia" data-label="Julia">
<p class="api-tab-label">Julia</p>

```julia
struct NativeField
NativeField(data::AbstractArray, dims::Vector{String}, attrs::Dict{String, Any})
NativeField(data, dims, attrs)
NativeField(data::AbstractArray, dims::AbstractVector)
```

```julia
NativeField(data, dims, attrs)
```

One native-grid array exactly as a reader decodes it: `data` is a Julia array
whose axes correspond, in order, to `dims` (the on-disk dimension names, file
order — e.g. `["time","latitude","longitude"]`). Numeric fields are `Float64`
with `NaN` for masked/`_FillValue` cells; raw integer reads (e.g. an undecoded
time axis) keep their stored integer type; text columns are `String`. `attrs`
carries decode-relevant metadata the reader must NOT act on but ESS needs —
notably a time axis's `units`/`calendar` (calendar decoding is ESS's job).

[Source](https://github.com/EarthSciML/EarthSciIO/blob/a49538bbd6ec318dde369d3496f6e8880586a73f/julia/src/readers.jl#L13)

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

<p class="api-src">From <a href="https://github.com/EarthSciML/EarthSciIO/blob/a49538bbd6ec318dde369d3496f6e8880586a73f/docs/api/examples/julia/provider.jl#L8-L20">docs/api/examples/julia/provider.jl</a></p>

</div>

<div class="api-tab" data-lang="rust" data-label="Rust">
<p class="api-tab-label">Rust</p>

```rust
pub struct NativeField {
    pub dtype: DType,
    pub dims: Vec<String>,
    pub shape: Vec<usize>,
    pub data: ArrayData,
    pub fill_value: Option<f64>,
}

impl NativeField { pub fn select_leading(&self, i: usize) -> Result<NativeField> }
```

One native-grid array, keyed by its on-disk `file_variable` name and validated
against `spec/schemas/native-field.schema.json`. The native grid is the
loader's own grid — **regrid is ESD/C4's job, not the reader's.**

[Source](https://github.com/EarthSciML/EarthSciIO/blob/a49538bbd6ec318dde369d3496f6e8880586a73f/rust/src/format/mod.rs#L237) · [Rust docs](https://docs.rs/earthsciio/latest/earthsciio/format/struct.NativeField.html)

</div>

</div>
