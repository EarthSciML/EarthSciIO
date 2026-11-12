---
title: "refresh_times_closed"
description: "Stable function · Providers"
type: "api"
api_tabs: true
---


**Tier:** stable · **Kind:** function · **Topic:** [Providers](/generated/api/topics/providers)

| Python | Julia | Rust |
|---|---|---|
| not exported | `refresh_times_closed` | not exported |


<div class="api-tabs">

<div class="api-tab" data-lang="python" data-label="Python">
<p class="api-tab-label">Python</p>

<p class="api-absent">Python does not export this function.</p>

</div>

<div class="api-tab" data-lang="julia" data-label="Julia">
<p class="api-tab-label">Julia</p>

```julia
refresh_times_closed(p::Provider)
```

```julia
refresh_times_closed(p::Provider) -> Vector{Float64}
```

[`refresh_times`](/generated/api/ref/refresh_times) over the CLOSED window `[lo, hi]` (spec/esm-sources.md
§2.3): the same anchors plus the one at the window end itself, when the window
end is a cadence anchor before the source's `stop`. The tstops of a solver that
integrates over `[t0, tf]` and must refresh at `tf` when `tf` lands on a record.
With no window (`hi` is `stop`, where no record is) it equals `refresh_times`,
as it does for a provider over a caller-made `times` grid, which has no window.

[Source](https://github.com/EarthSciML/EarthSciIO/blob/770dbc22d557e6e2ae87d9bccd7386f45764e9d0/julia/src/provider.jl#L526)

</div>

<div class="api-tab" data-lang="rust" data-label="Rust">
<p class="api-tab-label">Rust</p>

<p class="api-absent">Rust does not export this function.</p>

</div>

</div>
