---
title: "native_dataset"
description: "Stable type · Native arrays and readers"
type: "api"
api_tabs: true
---


**Tier:** stable · **Kind:** type · **Topic:** [Native arrays and readers](/generated/api/topics/native-arrays)

| Python | Julia | Rust |
|---|---|---|
| `NativeDataset` | `NativeDataset` | `NativeDataset` |


<div class="api-tabs">

<div class="api-tab" data-lang="python" data-label="Python">
<p class="api-tab-label">Python</p>

```python
class NativeDataset
NativeDataset(variables: Mapping[str, NativeField] | None = None, coords: Mapping[str, NativeField] | None = None)
NativeDataset.__getitem__(name: str) -> NativeField
NativeDataset.__contains__(name: object) -> bool
NativeDataset.variable_names() -> list[str]
NativeDataset.coord_names() -> list[str]
```

The native arrays from one blob: data `variables` + grid `coords`.

Both map a `file_variable` name → [`NativeField`](/generated/api/ref/native_field). `__getitem__`
looks in `variables` then `coords`, so `nds["t2m"]` and `nds["time"]`
both resolve (mirrors the Julia/Rust `NativeDataset`). Coordinates are the
dimension-coordinate fields (e.g. `latitude`/`longitude`/`time`); the
data variables are everything else.

**Members**

- `variable_names` — Names of the data variables (not coordinates), sorted.
- `coord_names` — Names of the coordinate fields, sorted.

[Source](https://github.com/EarthSciML/EarthSciIO/blob/a49538bbd6ec318dde369d3496f6e8880586a73f/earthsciio/native.py#L76)

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
struct NativeDataset
NativeDataset(variables::Dict{String, NativeField}, coords::Dict{String, NativeField})
NativeDataset(variables, coords)
NativeDataset()
```

```julia
NativeDataset(variables, coords)
```

The native arrays a reader returns from one blob: `variables` (data fields keyed
by `file_variable`) and `coords` (the dimension-coordinate fields — e.g. the
grid's `latitude`/`longitude`/`time`). Both map `String` name → [`NativeField`].
`getindex` looks in `variables` then `coords`, so `nds["t2m"]` and `nds["time"]`
both resolve.

[Source](https://github.com/EarthSciML/EarthSciIO/blob/a49538bbd6ec318dde369d3496f6e8880586a73f/julia/src/readers.jl#L41)

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
pub struct NativeDataset {
    pub variables: HashMap<String, NativeField>,
    pub coords: HashMap<String, Coord>,
}
```

A decoded native dataset: data variables keyed by on-disk name, plus the
native coordinates of the loader's grid.

[Source](https://github.com/EarthSciML/EarthSciIO/blob/a49538bbd6ec318dde369d3496f6e8880586a73f/rust/src/format/mod.rs#L323) · [Rust docs](https://docs.rs/earthsciio/latest/earthsciio/format/struct.NativeDataset.html)

</div>

</div>
