---
title: "write_record_at"
description: "Stable function · Streaming writer"
type: "api"
api_tabs: true
---


**Tier:** stable · **Kind:** function · **Topic:** [Streaming writer](/generated/api/topics/writer)

| Python | Julia | Rust |
|---|---|---|
| not exported | `write_record_at!` | not exported |


<div class="api-tabs">

<div class="api-tab" data-lang="python" data-label="Python">
<p class="api-tab-label">Python</p>

<p class="api-absent">Python does not export this function.</p>

</div>

<div class="api-tab" data-lang="julia" data-label="Julia">
<p class="api-tab-label">Julia</p>

```julia
write_record_at!(w::EarthSciIO.StubWriter, args...; kwargs...)
write_record_at!(::ZarrWriter, h::EarthSciIO.ZarrWriteHandle, index::Integer, t, arrays; region)
```

```julia
write_record_at!(w, handle, index, t, arrays; region = nothing)
```

Write record `index` (0-based) in indexed mode, in any order; `t` must equal the
schema's `times[index]` bitwise. Rewriting a committed record needs
[`write_invalidate!`] first.

---

```julia
write_record_at!(w, h, index, t, arrays; region = nothing)
```

Indexed mode: write record `index` (0-based, any order). `t` must equal
`times[index]` bitwise. A committed record must be invalidated first.

[Source](https://github.com/EarthSciML/EarthSciIO/blob/a49538bbd6ec318dde369d3496f6e8880586a73f/julia/src/writer.jl#L45)

</div>

<div class="api-tab" data-lang="rust" data-label="Rust">
<p class="api-tab-label">Rust</p>

<p class="api-absent">Rust does not export this function.</p>

</div>

</div>
