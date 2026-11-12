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

For a `temporal` provider these are the record anchors in the half-open window
`[lo, hi)`: a refresh at exactly the window end is not among them;
[`refresh_times_closed`](/generated/api/ref/refresh_times_closed) includes it.

[Source](https://github.com/EarthSciML/EarthSciIO/blob/770dbc22d557e6e2ae87d9bccd7386f45764e9d0/julia/src/provider.jl#L513)

</div>

<div class="api-tab" data-lang="rust" data-label="Rust">
<p class="api-tab-label">Rust</p>

<p class="api-absent">Rust does not export this function.</p>

</div>

</div>
