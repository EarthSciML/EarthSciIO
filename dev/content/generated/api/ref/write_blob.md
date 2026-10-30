---
title: "write_blob"
description: "Stable function · Streaming writer"
type: "api"
api_tabs: true
---


**Tier:** stable · **Kind:** function · **Topic:** [Streaming writer](/generated/api/topics/writer)

| Python | Julia | Rust |
|---|---|---|
| not exported | `write_blob!` | not exported |


<div class="api-tabs">

<div class="api-tab" data-lang="python" data-label="Python">
<p class="api-tab-label">Python</p>

<p class="api-absent">Python does not export this function.</p>

</div>

<div class="api-tab" data-lang="julia" data-label="Julia">
<p class="api-tab-label">Julia</p>

```julia
write_blob!(w::EarthSciIO.StubWriter, args...; kwargs...)
write_blob!(::ZarrWriter, h::EarthSciIO.ZarrWriteHandle, scope, key::AbstractString, bytes::AbstractVector{UInt8})
```

```julia
write_blob!(w, handle, scope, key, bytes)
```

Store a small rank-local payload: `scope` is `:static` or `(:record, i)`. Needs a
v2 store. Committed by the next commit.

[Source](https://github.com/EarthSciML/EarthSciIO/blob/a49538bbd6ec318dde369d3496f6e8880586a73f/julia/src/writer.jl#L60)

</div>

<div class="api-tab" data-lang="rust" data-label="Rust">
<p class="api-tab-label">Rust</p>

<p class="api-absent">Rust does not export this function.</p>

</div>

</div>
