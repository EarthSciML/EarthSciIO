---
title: "cancel"
description: "Stable function · Prefetch and windows"
type: "api"
api_tabs: true
---


**Tier:** stable · **Kind:** function · **Topic:** [Prefetch and windows](/generated/api/topics/prefetch)

| Python | Julia | Rust |
|---|---|---|
| not exported | `cancel!` | not exported |


<div class="api-tabs">

<div class="api-tab" data-lang="python" data-label="Python">
<p class="api-tab-label">Python</p>

<p class="api-absent">Python does not export this function.</p>

</div>

<div class="api-tab" data-lang="julia" data-label="Julia">
<p class="api-tab-label">Julia</p>

```julia
cancel!(g::PrefetchGroup; wait)
```

```julia
cancel!(g::PrefetchGroup; wait = false) -> g
```

Drop all pending work at once (spec/prefetch.md §3.8). A decode already running —
possibly inside a C library — runs to completion and its result is discarded; with
`wait = true` this returns only after those decodes finish. The group is reusable:
call [`start!`] to declare a new order. `take!` before then is an error.

[Source](https://github.com/EarthSciML/EarthSciIO/blob/a49538bbd6ec318dde369d3496f6e8880586a73f/julia/src/prefetch.jl#L499)

</div>

<div class="api-tab" data-lang="rust" data-label="Rust">
<p class="api-tab-label">Rust</p>

<p class="api-absent">Rust does not export this function.</p>

</div>

</div>
