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

The sorted, de-duplicated union `B` of the members' [`refresh_times_closed`](/generated/api/ref/refresh_times_closed)
and, for a `temporal` member, its run-window start, where the record in effect
there takes over (spec/esm-sources.md §2.6), in their own cadence unit. Epoch `k`
(1-based) is `[B[k], B[k+1])`, the last one open on the right. When a member's
window ends on one of its records, the last boundary is that window end: its
epoch holds no step of the run, but its sample is the refresh there. Boundaries are solver tstops, so no step straddles one, and a step's epoch
is `searchsortedlast(B, t0)` in both directions (spec/prefetch.md §4).

[Source](https://github.com/EarthSciML/EarthSciIO/blob/770dbc22d557e6e2ae87d9bccd7386f45764e9d0/julia/src/prefetch.jl#L293)

</div>

<div class="api-tab" data-lang="rust" data-label="Rust">
<p class="api-tab-label">Rust</p>

<p class="api-absent">Rust does not export this function.</p>

</div>

</div>
