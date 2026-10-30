---
title: "schema_fingerprint"
description: "Stable function · Streaming writer"
type: "api"
api_tabs: true
---


**Tier:** stable · **Kind:** function · **Topic:** [Streaming writer](/generated/api/topics/writer)

| Python | Julia | Rust |
|---|---|---|
| not exported | `schema_fingerprint` | `schema_fingerprint` |


<div class="api-tabs">

<div class="api-tab" data-lang="python" data-label="Python">
<p class="api-tab-label">Python</p>

<p class="api-absent">Python does not export this function.</p>

</div>

<div class="api-tab" data-lang="julia" data-label="Julia">
<p class="api-tab-label">Julia</p>

```julia
schema_fingerprint(s::OutputSchema)
```

```julia
schema_fingerprint(schema::OutputSchema) -> "sha256:<hex>"
```

The stream-schema fingerprint of spec/writer.md §3.1: the sha256 of the canonical
JSON of dims (time length `0` in append mode, `n_records` in indexed mode),
`time_dim`, `chunk_shape`, `shard_shape`, `profile`, `time_mode`, and the
`[name, dims, dtype]` of every streaming variable and static, in schema order.
Identical across the Julia / Python / Rust bindings.

[Source](https://github.com/EarthSciML/EarthSciIO/blob/a49538bbd6ec318dde369d3496f6e8880586a73f/julia/src/zarr_stream.jl#L169)

</div>

<div class="api-tab" data-lang="rust" data-label="Rust">
<p class="api-tab-label">Rust</p>

```rust
pub fn schema_fingerprint(schema: &StreamSchema) -> String
```

The schema fingerprint (`spec/writer.md` §3.1): `"sha256:" + hex` over the
canonical JSON of the schema's structural fields.

[Source](https://github.com/EarthSciML/EarthSciIO/blob/a49538bbd6ec318dde369d3496f6e8880586a73f/rust/src/format/zarr_stream.rs#L575) · [Rust docs](https://docs.rs/earthsciio/latest/earthsciio/format/zarr_stream/fn.schema_fingerprint.html)

</div>

</div>
