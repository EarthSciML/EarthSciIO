---
title: "write_resume"
description: "Stable function · Streaming writer"
type: "api"
api_tabs: true
---


**Tier:** stable · **Kind:** function · **Topic:** [Streaming writer](/generated/api/topics/writer)

| Python | Julia | Rust |
|---|---|---|
| not exported | `write_resume` | not exported |


<div class="api-tabs">

<div class="api-tab" data-lang="python" data-label="Python">
<p class="api-tab-label">Python</p>

<p class="api-absent">Python does not export this function.</p>

</div>

<div class="api-tab" data-lang="julia" data-label="Julia">
<p class="api-tab-label">Julia</p>

```julia
write_resume(w::EarthSciIO.StubWriter, args...; kwargs...)
write_resume(::ZarrWriter, store, base_url::AbstractString; role, n_writers, id, region, at, schema)
```

```julia
write_resume(w, store, base_url; role = :single, n_writers = 1, id = 0,
             region = nothing, at = nothing, schema = nothing) -> handle
```

Reopen a store after preemption (spec/writer.md §7); in append mode committed
records with `t > at` are discarded.

[Source](https://github.com/EarthSciML/EarthSciIO/blob/a49538bbd6ec318dde369d3496f6e8880586a73f/julia/src/writer.jl#L95)

</div>

<div class="api-tab" data-lang="rust" data-label="Rust">
<p class="api-tab-label">Rust</p>

<p class="api-absent">Rust does not export this function.</p>

</div>

</div>
