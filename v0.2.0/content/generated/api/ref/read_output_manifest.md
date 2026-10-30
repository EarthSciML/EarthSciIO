---
title: "read_output_manifest"
description: "Stable function · Streaming writer"
type: "api"
api_tabs: true
---


**Tier:** stable · **Kind:** function · **Topic:** [Streaming writer](/generated/api/topics/writer)

| Python | Julia | Rust |
|---|---|---|
| not exported | `read_output_manifest` | `read_output_manifest` |


<div class="api-tabs">

<div class="api-tab" data-lang="python" data-label="Python">
<p class="api-tab-label">Python</p>

<p class="api-absent">Python does not export this function.</p>

</div>

<div class="api-tab" data-lang="julia" data-label="Julia">
<p class="api-tab-label">Julia</p>

```julia
read_output_manifest(path::AbstractString)
```

```julia
read_output_manifest(path) -> OutputManifest | AbstractDict | nothing
```

Read a store's output manifest. `path` is the `output_manifest.json` file, the
store directory, or a `file://` URL of either; `nothing` when absent. A v1
manifest (`earthsciio/output-manifest/v1`) parses to an [`OutputManifest`]; a v2
manifest (spec/writer.md §8.2) is returned as the parsed JSON object (keys as in
the spec: `commit_seq`, `records`, `statics`, `blobs`, `writers`, …). Any other
`schema` label is an error (readers pin the label exactly).

[Source](https://github.com/EarthSciML/EarthSciIO/blob/a49538bbd6ec318dde369d3496f6e8880586a73f/julia/src/manifest.jl#L168)

</div>

<div class="api-tab" data-lang="rust" data-label="Rust">
<p class="api-tab-label">Rust</p>

```rust
pub fn read_output_manifest<S: ReadableStorageTraits + ?Sized>(store: &S) -> Result<Value>
```

Read and parse `output_manifest.json` (v1 or v2).

**Errors**
`Error::Format` when the store has no manifest or it does not parse.

[Source](https://github.com/EarthSciML/EarthSciIO/blob/a49538bbd6ec318dde369d3496f6e8880586a73f/rust/src/format/zarr_stream.rs#L2743) · [Rust docs](https://docs.rs/earthsciio/latest/earthsciio/format/zarr_stream/fn.read_output_manifest.html)

</div>

</div>
