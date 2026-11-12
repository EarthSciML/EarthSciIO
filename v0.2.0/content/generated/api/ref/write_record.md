---
title: "write_record"
description: "Stable function · Streaming writer"
type: "api"
api_tabs: true
---


**Tier:** stable · **Kind:** function · **Topic:** [Streaming writer](/generated/api/topics/writer)

| Python | Julia | Rust |
|---|---|---|
| not exported | `write_record!` | not exported |


<div class="api-tabs">

<div class="api-tab" data-lang="python" data-label="Python">
<p class="api-tab-label">Python</p>

<p class="api-absent">Python does not export this function.</p>

</div>

<div class="api-tab" data-lang="julia" data-label="Julia">
<p class="api-tab-label">Julia</p>

```julia
write_record!(w::EarthSciIO.StubWriter, args...; kwargs...)
write_record!(::ZarrWriter, h::EarthSciIO.ZarrWriteHandle, t, arrays; region)
```

```julia
write_record!(w, handle, t, arrays; region = nothing) -> index
```

Append ONE time record (append mode): `t` is the time coordinate value, `arrays`
maps each variable this writer supplies to its gridded `Array` (the variable's
non-time dims, in order, region-shaped). Returns the record index. A non-`nothing`
`region` must equal the region the handle was opened with.

[Source](https://github.com/EarthSciML/EarthSciIO/blob/a49538bbd6ec318dde369d3496f6e8880586a73f/julia/src/writer.jl#L36)

</div>

<div class="api-tab" data-lang="rust" data-label="Rust">
<p class="api-tab-label">Rust</p>

<p class="api-absent">Rust does not export this function.</p>

</div>

</div>
