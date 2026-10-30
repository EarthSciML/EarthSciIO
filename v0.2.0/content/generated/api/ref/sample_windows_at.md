---
title: "sample_windows_at"
description: "Stable function · Prefetch and windows"
type: "api"
api_tabs: true
---


**Tier:** stable · **Kind:** function · **Topic:** [Prefetch and windows](/generated/api/topics/prefetch)

| Python | Julia | Rust |
|---|---|---|
| not exported | `sample_windows_at` | not exported |


<div class="api-tabs">

<div class="api-tab" data-lang="python" data-label="Python">
<p class="api-tab-label">Python</p>

<p class="api-absent">Python does not export this function.</p>

</div>

<div class="api-tab" data-lang="julia" data-label="Julia">
<p class="api-tab-label">Julia</p>

```julia
sample_windows_at(p::Provider, t::Real, windows)
sample_windows_at(p::Provider, windows)
```

```julia
sample_windows_at(p::Provider, t::Real, windows) -> Vector{NativeDataset}
sample_windows_at(p::Provider, windows) -> Vector{NativeDataset}
```

K windows at once: dataset `i` is identical — bitwise, coordinates included — to
`sample_at(with_window(p, windows[i]), t)`, but the source is fetched and decoded
ONCE, over the windows' bounding union (spec/prefetch.md §2.4). Per dimension the
union is the sorted distinct union of the windows' indices, or the whole dimension
when any window leaves it unnamed or names it `"all"`; each window is then sliced
back out of the union by name, in its own order and with its own repetitions.

This is for chunk layouts a window cannot shrink (GEOS-FP 0.25° A3dyn is chunked
one global level per chunk): one rank per node decodes its neighbours' windows in
one pass and the integrator scatters them. The `with_window` preconditions apply
to every window, before any I/O; an empty `windows` returns `NativeDataset[]`
without fetching anything. The two-argument form is for a [`CONST`] provider.

[Source](https://github.com/EarthSciML/EarthSciIO/blob/a49538bbd6ec318dde369d3496f6e8880586a73f/julia/src/windows.jl#L221)

</div>

<div class="api-tab" data-lang="rust" data-label="Rust">
<p class="api-tab-label">Rust</p>

<p class="api-absent">Rust does not export this function.</p>

</div>

</div>
