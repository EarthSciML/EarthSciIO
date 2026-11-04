---
title: "with_window"
description: "Stable function · Prefetch and windows"
type: "api"
api_tabs: true
---


**Tier:** stable · **Kind:** function · **Topic:** [Prefetch and windows](/generated/api/topics/prefetch)

| Python | Julia | Rust |
|---|---|---|
| not exported | `with_window` | not exported |


<div class="api-tabs">

<div class="api-tab" data-lang="python" data-label="Python">
<p class="api-tab-label">Python</p>

<p class="api-absent">Python does not export this function.</p>

</div>

<div class="api-tab" data-lang="julia" data-label="Julia">
<p class="api-tab-label">Julia</p>

```julia
with_window(p::Provider, w)
```

```julia
with_window(p::Provider, w) -> Provider
```

A NEW provider that shares `p`'s cache, reader, URL resolver and cadence and has
the window `w` (a [`DimWindow`] or its wire `Dict`) baked in as
`reader_kwargs[:select]`. `p` is unchanged. Raised at once, before any I/O, when
`p` already bakes a `select` (two windows do not compose), when its reader does not
answer [`supports_selection`], or when `w` names `p`'s `time_dim` — records are the
Provider's (spec/prefetch.md §2.3).

A window is bound at construction, never per call: the per-call
`materialize(p, t; select = …)` stays a peek. Everything else about the window —
a named dimension the read lacks, a selector out of range, a non-`"all"` time
dimension — is the reader's to refuse, at the first sample.

[Source](https://github.com/EarthSciML/EarthSciIO/blob/a49538bbd6ec318dde369d3496f6e8880586a73f/julia/src/windows.jl#L201)

</div>

<div class="api-tab" data-lang="rust" data-label="Rust">
<p class="api-tab-label">Rust</p>

<p class="api-absent">Rust does not export this function.</p>

</div>

</div>
