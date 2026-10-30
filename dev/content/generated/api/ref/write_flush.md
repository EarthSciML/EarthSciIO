---
title: "write_flush"
description: "Stable function · Streaming writer"
type: "api"
api_tabs: true
---


**Tier:** stable · **Kind:** function · **Topic:** [Streaming writer](/generated/api/topics/writer)

| Python | Julia | Rust |
|---|---|---|
| not exported | `write_flush!` | not exported |


<div class="api-tabs">

<div class="api-tab" data-lang="python" data-label="Python">
<p class="api-tab-label">Python</p>

<p class="api-absent">Python does not export this function.</p>

</div>

<div class="api-tab" data-lang="julia" data-label="Julia">
<p class="api-tab-label">Julia</p>

```julia
write_flush!(w::EarthSciIO.StubWriter, args...; kwargs...)
write_flush!(::ZarrWriter, h::EarthSciIO.ZarrWriteHandle)
```

```julia
write_flush!(w, handle) -> CommitToken
```

Write every dirty shard of this writer's region and return the token naming what
was written since the last token. A `:single` writer also commits (when
something is pending).

[Source](https://github.com/EarthSciML/EarthSciIO/blob/a49538bbd6ec318dde369d3496f6e8880586a73f/julia/src/writer.jl#L67)

</div>

<div class="api-tab" data-lang="rust" data-label="Rust">
<p class="api-tab-label">Rust</p>

<p class="api-absent">Rust does not export this function.</p>

</div>

</div>
