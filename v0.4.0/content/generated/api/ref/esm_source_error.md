---
title: "esm_source_error"
description: "Stable error · Other"
type: "api"
api_tabs: true
---


**Tier:** stable · **Kind:** error · **Topic:** [Other](/generated/api/topics/other)

| Python | Julia | Rust |
|---|---|---|
| `EsmSourceError` | `EsmSourceError` | not exported |


<div class="api-tabs">

<div class="api-tab" data-lang="python" data-label="Python">
<p class="api-tab-label">Python</p>

```python
class EsmSourceError(EarthSciIOError, ValueError)
EsmSourceError(code: str, name: str, detail: str)
```

An esm data source EarthSciIO cannot read. `code` is one of the
`esm_source_*` codes of spec/esm-sources.md §3.3, `esm_period_invalid`,
or an `esm_template_*` code for a template of the entry (the
[`EsmTemplateError`](/generated/api/ref/esm_template_error) is its `__cause__`). The message
contains the code and the source's name.

[Source](https://github.com/EarthSciML/EarthSciIO/blob/770dbc22d557e6e2ae87d9bccd7386f45764e9d0/earthsciio/esm_source.py#L34)

</div>

<div class="api-tab" data-lang="julia" data-label="Julia">
<p class="api-tab-label">Julia</p>

```julia
struct EsmSourceError <: Exception
EsmSourceError(code::String, source::String, detail::String)
EsmSourceError(code, source, detail)
```

```julia
EsmSourceError(code, source, detail)
```

An esm data source EarthSciIO cannot read. `code` is one of the `esm_source_*`
codes of spec/esm-sources.md §3.3, `esm_period_invalid`, or an `esm_template_*`
code for a template of the entry; the message contains the code and the
source's name.

[Source](https://github.com/EarthSciML/EarthSciIO/blob/770dbc22d557e6e2ae87d9bccd7386f45764e9d0/julia/src/esm_source.jl#L9)

</div>

<div class="api-tab" data-lang="rust" data-label="Rust">
<p class="api-tab-label">Rust</p>

<p class="api-absent">Rust does not export this error.</p>

</div>

</div>
