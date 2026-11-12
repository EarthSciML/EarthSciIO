---
title: "esm_template"
description: "Stable type · Other"
type: "api"
api_tabs: true
---


**Tier:** stable · **Kind:** type · **Topic:** [Other](/generated/api/topics/other)

| Python | Julia | Rust |
|---|---|---|
| `EsmTemplate` | `EsmTemplate` | not exported |


<div class="api-tabs">

<div class="api-tab" data-lang="python" data-label="Python">
<p class="api-tab-label">Python</p>

```python
class EsmTemplate
EsmTemplate(template: str, substitutions: Mapping[str, str] = dict())
EsmTemplate.__call__(anchor: _dt.datetime) -> str
EsmTemplate.check_mirrors(mirrors: Sequence[str]) -> None
EsmTemplate.expand_mirrors(mirrors: Sequence[str], anchor: _dt.datetime) -> list[str]
```

A loader URL that is an esm template (spec/esm-sources.md §1.5).

Use it as [`DataSource`](/generated/api/ref/data_source) `url`; give `mirrors` as esm
templates too and they are expanded at the same anchor with the same
`substitutions`. Calling it expands the template at an anchor.

**Members**

- `check_mirrors` — Refuse a mirror template with an unbound or malformed name, as the primary is refused at construction.
- `expand_mirrors` — Each mirror template expanded at `anchor`.

[Source](https://github.com/EarthSciML/EarthSciIO/blob/770dbc22d557e6e2ae87d9bccd7386f45764e9d0/earthsciio/esm.py#L185)

</div>

<div class="api-tab" data-lang="julia" data-label="Julia">
<p class="api-tab-label">Julia</p>

```julia
struct EsmTemplate <: Function
EsmTemplate(template::AbstractString; substitutions, mirrors)
```

```julia
EsmTemplate(template; substitutions=Dict(), mirrors=String[])
```

A provider URL that is an esm template (spec/esm-sources.md §1.5). Pass it as
the `url` of a [`Provider`](/generated/api/ref/provider): it is called with a Unix-seconds file anchor
and returns the expanded URL. Its `mirrors` are esm templates expanded at the
same anchor with the same substitutions and tried in order after the primary
URL. An unbound substitution is refused here, not at the first fetch.

[Source](https://github.com/EarthSciML/EarthSciIO/blob/770dbc22d557e6e2ae87d9bccd7386f45764e9d0/julia/src/esm.jl#L192)

</div>

<div class="api-tab" data-lang="rust" data-label="Rust">
<p class="api-tab-label">Rust</p>

<p class="api-absent">Rust does not export this type.</p>

</div>

</div>
