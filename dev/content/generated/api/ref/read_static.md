---
title: "read_static"
description: "Stable function · Streaming writer"
type: "api"
api_tabs: true
---


**Tier:** stable · **Kind:** function · **Topic:** [Streaming writer](/generated/api/topics/writer)

| Python | Julia | Rust |
|---|---|---|
| not exported | `read_static` | `read_static` |


<div class="api-tabs">

<div class="api-tab" data-lang="python" data-label="Python">
<p class="api-tab-label">Python</p>

<p class="api-absent">Python does not export this function.</p>

</div>

<div class="api-tab" data-lang="julia" data-label="Julia">
<p class="api-tab-label">Julia</p>

```julia
read_static(store, name::AbstractString; region)
```

```julia
read_static(store, name; region = nothing) -> Array
```

Read the committed static `name` (dims order, region-sized, stored dtype).
Errors if it is not committed.

[Source](https://github.com/EarthSciML/EarthSciIO/blob/a49538bbd6ec318dde369d3496f6e8880586a73f/julia/src/zarr_stream.jl#L1642)

</div>

<div class="api-tab" data-lang="rust" data-label="Rust">
<p class="api-tab-label">Rust</p>

```rust
pub fn read_static<S>(store: Arc<S>, name: &str, region: &Region) -> Result<TypedVec> where S: ReadableStorageTraits + 'static
```

Read the `region` of committed static `name`.

**Errors**
`Error::Format` for an uncommitted or unknown static, or a store failure.

[Source](https://github.com/EarthSciML/EarthSciIO/blob/a49538bbd6ec318dde369d3496f6e8880586a73f/rust/src/format/zarr_stream.rs#L2822) · [Rust docs](https://docs.rs/earthsciio/latest/earthsciio/format/zarr_stream/fn.read_static.html)

</div>

</div>
