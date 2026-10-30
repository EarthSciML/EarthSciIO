---
title: "refresh_times"
description: "Stable function · Providers"
type: "api"
api_tabs: true
---


**Tier:** stable · **Kind:** function · **Topic:** [Providers](/generated/api/topics/providers)

| Python | Julia | Rust |
|---|---|---|
| not exported | `refresh_times` | not exported |


<div class="api-tabs">

<div class="api-tab" data-lang="python" data-label="Python">
<p class="api-tab-label">Python</p>

<p class="api-absent">Python does not export this function.</p>

</div>

<div class="api-tab" data-lang="julia" data-label="Julia">
<p class="api-tab-label">Julia</p>

```julia
refresh_times(p::Provider)
```

```julia
refresh_times(p::Provider) -> Vector{Float64}
```

The discrete time points at which the data changes and the solver must
[`refresh`]. Empty for a [`CONST`] provider; the sorted cadence grid for a
[`DISCRETE`] one. The library exposes these; the user wires them into the
solver (e.g. `PresetTimeCallback(refresh_times(p), …)`).

[Source](https://github.com/EarthSciML/EarthSciIO/blob/a49538bbd6ec318dde369d3496f6e8880586a73f/julia/src/provider.jl#L198)

</div>

<div class="api-tab" data-lang="rust" data-label="Rust">
<p class="api-tab-label">Rust</p>

<p class="api-absent">Rust does not export this function.</p>

</div>

</div>
