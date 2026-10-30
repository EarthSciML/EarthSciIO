---
title: "read_record"
description: "Stable function · Streaming writer"
type: "api"
api_tabs: true
---


**Tier:** stable · **Kind:** function · **Topic:** [Streaming writer](/generated/api/topics/writer)

| Python | Julia | Rust |
|---|---|---|
| not exported | `read_record` | `read_record` |


<div class="api-tabs">

<div class="api-tab" data-lang="python" data-label="Python">
<p class="api-tab-label">Python</p>

<p class="api-absent">Python does not export this function.</p>

</div>

<div class="api-tab" data-lang="julia" data-label="Julia">
<p class="api-tab-label">Julia</p>

```julia
read_record(store, var::AbstractString, index::Integer; region)
```

```julia
read_record(store, var, index; region = nothing) -> Array
```

Read committed record `index` of streaming variable `var` straight from the store
(a directory or `file://` URL; never through the cache). The result is indexed
in the variable's dim order with the time dim dropped (a time-only variable
gives a 0-dimensional array), region-sized, in the stored dtype. Errors if the
record is not committed.

[Source](https://github.com/EarthSciML/EarthSciIO/blob/a49538bbd6ec318dde369d3496f6e8880586a73f/julia/src/zarr_stream.jl#L1621)

</div>

<div class="api-tab" data-lang="rust" data-label="Rust">
<p class="api-tab-label">Rust</p>

```rust
pub fn read_record<S>(store: Arc<S>, var: &str, index: u64, region: &Region) -> Result<TypedVec> where S: ReadableStorageTraits + 'static
```

Read the `region` of committed record `index` of variable `var` (shape: the
variable's dims minus time, region-sized). Committed data only; read
directly from the store, never through the content-addressed cache.

**Errors**
`Error::Format` for an uncommitted record, an unknown variable, a region
outside the array, or a store failure.

[Source](https://github.com/EarthSciML/EarthSciIO/blob/a49538bbd6ec318dde369d3496f6e8880586a73f/rust/src/format/zarr_stream.rs#L2806) · [Rust docs](https://docs.rs/earthsciio/latest/earthsciio/format/zarr_stream/fn.read_record.html)

</div>

</div>
