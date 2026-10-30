---
title: "start"
description: "Stable function · Prefetch and windows"
type: "api"
api_tabs: true
---


**Tier:** stable · **Kind:** function · **Topic:** [Prefetch and windows](/generated/api/topics/prefetch)

| Python | Julia | Rust |
|---|---|---|
| not exported | `start!` | not exported |


<div class="api-tabs">

<div class="api-tab" data-lang="python" data-label="Python">
<p class="api-tab-label">Python</p>

<p class="api-absent">Python does not export this function.</p>

</div>

<div class="api-tab" data-lang="julia" data-label="Julia">
<p class="api-tab-label">Julia</p>

```julia
start!(g::PrefetchGroup, order; from)
```

```julia
start!(g::PrefetchGroup, order; from = nothing) -> g
```

Declare the order in which epochs will be taken and begin decoding ahead of it
(spec/prefetch.md §3.3): `:forward` (`1, …, n`), `:reverse` (`n, …, 1` — the
adjoint's backward sweep), or an explicit `Vector{Int}` of 1-based epochs, repeats
legal (a revolve schedule). An empty list or an out-of-range epoch is an error.
`from = k` trims the order to begin at its first entry equal to `k` — the restart
form, `start!(g, :forward; from = r)` — and an epoch absent from the order is an
error. Any previous order is cancelled first (`cancel!(g; wait = false)`), and the
`changed` flags restart: the first `take!` after `start!` reports every member
changed.

[Source](https://github.com/EarthSciML/EarthSciIO/blob/a49538bbd6ec318dde369d3496f6e8880586a73f/julia/src/prefetch.jl#L346)

</div>

<div class="api-tab" data-lang="rust" data-label="Rust">
<p class="api-tab-label">Rust</p>

<p class="api-absent">Rust does not export this function.</p>

</div>

</div>
