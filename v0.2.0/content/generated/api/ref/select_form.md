---
title: "select_form"
description: "Stable function · Prefetch and windows"
type: "api"
api_tabs: true
---


**Tier:** stable · **Kind:** function · **Topic:** [Prefetch and windows](/generated/api/topics/prefetch)

| Python | Julia | Rust |
|---|---|---|
| not exported | `select_form` | not exported |


<div class="api-tabs">

<div class="api-tab" data-lang="python" data-label="Python">
<p class="api-tab-label">Python</p>

<p class="api-absent">Python does not export this function.</p>

</div>

<div class="api-tab" data-lang="julia" data-label="Julia">
<p class="api-tab-label">Julia</p>

```julia
select_form(w::DimWindow)
select_form(sel::AbstractDict)
select_form(x)
```

```julia
select_form(w::DimWindow) -> Dict{String,Any}
select_form(sel::AbstractDict) -> Dict{String,Any}
```

The wire form of a window, `Dict("dims" => Dict(dim => selector, ...))` — what a
reader's `select` option and `reader_kwargs[:select]` take (spec/prefetch.md
§2.2). Given a wire `Dict` already, it is validated and normalised: it must be the
named `dims` form, since a positional `axes` list cannot be applied by name and so
is not a window.

[Source](https://github.com/EarthSciML/EarthSciIO/blob/a49538bbd6ec318dde369d3496f6e8880586a73f/julia/src/windows.jl#L119)

</div>

<div class="api-tab" data-lang="rust" data-label="Rust">
<p class="api-tab-label">Rust</p>

<p class="api-absent">Rust does not export this function.</p>

</div>

</div>
