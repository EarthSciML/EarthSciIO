---
title: "stream_schema"
description: "Stable type · Streaming writer"
type: "api"
api_tabs: true
---


**Tier:** stable · **Kind:** type · **Topic:** [Streaming writer](/generated/api/topics/writer)

| Python | Julia | Rust |
|---|---|---|
| not exported | `OutputSchema` | `StreamSchema` |


<div class="api-tabs">

<div class="api-tab" data-lang="python" data-label="Python">
<p class="api-tab-label">Python</p>

<p class="api-absent">Python does not export this type.</p>

</div>

<div class="api-tab" data-lang="julia" data-label="Julia">
<p class="api-tab-label">Julia</p>

```julia
struct OutputSchema
OutputSchema(dims::Vector{Pair{String, Int64}}, time_dim::String, coords::Vector{Pair{String, Tuple{Vector, Dict{String, Any}}}}, vars::Vector{Pair{String, OutputVar}}, chunk_shape::Dict{String, Int64}, shard_shape::Dict{String, Int64}, profile::Symbol, attrs::Dict{String, Any}, time_dtype::DataType, statics::Vector{Pair{String, OutputVar}}, time_mode::Union{Symbol, IndexedTime})
OutputSchema(dims, time_dim, coords, vars, chunk_shape, shard_shape, profile, attrs, time_dtype, statics, time_mode)
OutputSchema(; dims, time_dim, vars, chunk_shape, shard_shape, coords, profile, attrs, time_dtype, statics, time_mode)
```

```julia
OutputSchema(; dims, time_dim, vars, chunk_shape, shard_shape,
               coords=[], statics=[], time_mode=:append,
               profile=:diagnostic, attrs=Dict(), time_dtype=Float64)
```

The input to [`write_open!`] — a plain struct in this repo (the EarthSciAST
`OutputSchema` maps onto it at the binding layer; no EarthSciAST dependency here).
It is the spec/writer.md §3 *stream schema*.

Fields (all load-bearing for the binding to construct):
  * `dims::Vector{Pair{String,Int}}` — ORDERED dim name => length. In `:append`
    mode the `time_dim` entry's length is a placeholder (0 is conventional); the
    time axis grows. In indexed mode it is `n_records` (filled in when 0).
  * `time_dim::String` — the record axis name.
  * `coords::Vector{Pair{String,Tuple{Vector,Dict{String,Any}}}}` — ORDERED static
    coordinate arrays: name => (values, attrs). 1-D over their own dim; written
    once at `write_open!`. An entry for `time_dim` supplies the time coordinate's
    attrs (its VALUES are ignored — they come from the `t` of each record).
  * `vars::Vector{Pair{String,OutputVar}}` — ORDERED streaming variables.
  * `chunk_shape::Dict{String,Int}` — dim name => INNER chunk length.
  * `shard_shape::Dict{String,Int}` — dim name => SHARD length (must be a multiple
    of the inner chunk length along every dim). `shard_shape[time_dim]` is the
    number of records packed per shard object.
  * `profile::Symbol` — `:diagnostic`/`:checkpoint` (Blosc-zstd inner codec) or
    `:wasm` (plain v3 zstd inner codec, no Blosc — wasm/browser-loadable).
  * `attrs::Dict{String,Any}` — group-level attributes.
  * `time_dtype::DataType` — element type of the time coordinate (default `Float64`).
  * `statics::Vector{Pair{String,OutputVar}}` — ORDERED time-less variables
    (gradient fields, …) written once each with [`write_static!`].
  * `time_mode` — `:append` (default) or an [`IndexedTime`].

[Source](https://github.com/EarthSciML/EarthSciIO/blob/a49538bbd6ec318dde369d3496f6e8880586a73f/julia/src/zarr_write.jl#L105)

</div>

<div class="api-tab" data-lang="rust" data-label="Rust">
<p class="api-tab-label">Rust</p>

```rust
pub struct StreamSchema {
    pub dims: Vec<(String, usize)>,
    pub time_dim: String,
    pub chunk_shape: BTreeMap<String, usize>,
    pub shard_shape: BTreeMap<String, usize>,
    pub coords: Vec<WriteCoord>,
    pub vars: Vec<StreamVar>,
    pub statics: Vec<StreamVar>,
    pub time_mode: TimeMode,
    pub group_attrs: Map<String, Value>,
    pub profile: String,
}
```

The stream schema: the v1 `OutputSchema` without
data, plus a time mode, statics and per-variable dtypes.

[Source](https://github.com/EarthSciML/EarthSciIO/blob/a49538bbd6ec318dde369d3496f6e8880586a73f/rust/src/format/zarr_stream.rs#L398) · [Rust docs](https://docs.rs/earthsciio/latest/earthsciio/format/zarr_stream/struct.StreamSchema.html)

</div>

</div>
