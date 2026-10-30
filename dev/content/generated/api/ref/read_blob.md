---
title: "read_blob"
description: "Stable function · Streaming writer"
type: "api"
api_tabs: true
---


**Tier:** stable · **Kind:** function · **Topic:** [Streaming writer](/generated/api/topics/writer)

| Python | Julia | Rust |
|---|---|---|
| not exported | `read_blob` | `read_blob` |


<div class="api-tabs">

<div class="api-tab" data-lang="python" data-label="Python">
<p class="api-tab-label">Python</p>

<p class="api-absent">Python does not export this function.</p>

</div>

<div class="api-tab" data-lang="julia" data-label="Julia">
<p class="api-tab-label">Julia</p>

```julia
read_blob(store, scope, writer::Integer, key::AbstractString)
```

```julia
read_blob(store, scope, writer, key) -> Vector{UInt8}
```

Read the committed blob `key` of `writer` under `scope` (`:static` or
`(:record, i)`). Errors if it is not committed.

[Source](https://github.com/EarthSciML/EarthSciIO/blob/a49538bbd6ec318dde369d3496f6e8880586a73f/julia/src/zarr_stream.jl#L1658)

</div>

<div class="api-tab" data-lang="rust" data-label="Rust">
<p class="api-tab-label">Rust</p>

```rust
pub fn read_blob<S: ReadableStorageTraits + ?Sized>(store: &S, scope: BlobScope, writer: usize, key: &str) -> Result<Vec<u8>>
```

Read a committed blob.

**Errors**
`Error::Format` for an uncommitted blob or a store failure.

[Source](https://github.com/EarthSciML/EarthSciIO/blob/a49538bbd6ec318dde369d3496f6e8880586a73f/rust/src/format/zarr_stream.rs#L2841) · [Rust docs](https://docs.rs/earthsciio/latest/earthsciio/format/zarr_stream/fn.read_blob.html)

</div>

</div>
