---
title: "window_not_covered"
description: "Stable error · Other"
type: "api"
api_tabs: true
---


**Tier:** stable · **Kind:** error · **Topic:** [Other](/generated/api/topics/other)

| Python | Julia | Rust |
|---|---|---|
| `WindowNotCovered` | `WindowNotCovered` | not exported |


<div class="api-tabs">

<div class="api-tab" data-lang="python" data-label="Python">
<p class="api-tab-label">Python</p>

```python
class WindowNotCovered(EarthSciIOError, ValueError)
```

A provider's run window that some stretch of has no record in effect
(spec/esm-sources.md §2.6): it starts before the source's first record,
runs past the end of its data, or is empty. The message starts with the
code `esm_window_uncovered`, the same in every track.

[Source](https://github.com/EarthSciML/EarthSciIO/blob/770dbc22d557e6e2ae87d9bccd7386f45764e9d0/earthsciio/errors.py#L309)

</div>

<div class="api-tab" data-lang="julia" data-label="Julia">
<p class="api-tab-label">Julia</p>

```julia
struct WindowNotCovered <: Exception
WindowNotCovered(msg::String)
WindowNotCovered(msg)
```

```julia
WindowNotCovered(msg)
```

A provider's run window some stretch of which no record serves
(spec/esm-sources.md §2.6): it starts before the source's first record, runs
past the end of its data, or is empty. The message starts with the code
`esm_window_uncovered`, the same in every track.

[Source](https://github.com/EarthSciML/EarthSciIO/blob/770dbc22d557e6e2ae87d9bccd7386f45764e9d0/julia/src/provider.jl#L205)

</div>

<div class="api-tab" data-lang="rust" data-label="Rust">
<p class="api-tab-label">Rust</p>

<p class="api-absent">Rust does not export this error.</p>

</div>

</div>
