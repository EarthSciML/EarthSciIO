---
title: "refresh"
description: "Stable function · Providers"
type: "api"
api_tabs: true
---


**Tier:** stable · **Kind:** function · **Topic:** [Providers](/generated/api/topics/providers)

| Python | Julia | Rust |
|---|---|---|
| not exported | `refresh` | not exported |


<div class="api-tabs">

<div class="api-tab" data-lang="python" data-label="Python">
<p class="api-tab-label">Python</p>

<p class="api-absent">Python does not export this function.</p>

</div>

<div class="api-tab" data-lang="julia" data-label="Julia">
<p class="api-tab-label">Julia</p>

```julia
refresh(p::Provider, t::Real; select)
```

```julia
refresh(p::Provider, t::Real; select=nothing) -> NativeDataset
```

Re-materialize at the cadence tick `t`. This is the call the solver makes from
its `PresetTimeCallback` at each of [`refresh_times`]`(p)`. Identical to
[`materialize`]`(p, t)`; named for the solver-side update site. `select` is the
per-call projection pushdown override (see [`materialize`]).

[Source](https://github.com/EarthSciML/EarthSciIO/blob/a49538bbd6ec318dde369d3496f6e8880586a73f/julia/src/provider.jl#L652)

</div>

<div class="api-tab" data-lang="rust" data-label="Rust">
<p class="api-tab-label">Rust</p>

<p class="api-absent">Rust does not export this function.</p>

</div>

</div>
