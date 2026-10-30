---
title: "boundaries"
description: "Stable function · Prefetch and windows"
type: "api"
api_tabs: true
---


**Tier:** stable · **Kind:** function · **Topic:** [Prefetch and windows](/generated/api/topics/prefetch)

| Python | Julia | Rust |
|---|---|---|
| not exported | `boundaries` | not exported |


<div class="api-tabs">

<div class="api-tab" data-lang="python" data-label="Python">
<p class="api-tab-label">Python</p>

<p class="api-absent">Python does not export this function.</p>

</div>

<div class="api-tab" data-lang="julia" data-label="Julia">
<p class="api-tab-label">Julia</p>

```julia
boundaries(g::PrefetchGroup)
```

```julia
boundaries(g::PrefetchGroup) -> Vector{Float64}
```

The sorted, de-duplicated union `B` of the members' `refresh_times`, in their own
cadence unit. Epoch `k` (1-based) is `[B[k], B[k+1])`, the last one open on the
right. Boundaries are solver tstops, so no step straddles one, and a step's epoch
is `searchsortedlast(B, t0)` in both directions (spec/prefetch.md §4).

[Source](https://github.com/EarthSciML/EarthSciIO/blob/a49538bbd6ec318dde369d3496f6e8880586a73f/julia/src/prefetch.jl#L277)

</div>

<div class="api-tab" data-lang="rust" data-label="Rust">
<p class="api-tab-label">Rust</p>

<p class="api-absent">Rust does not export this function.</p>

</div>

</div>
