---
title: "expand_esm_template"
description: "Stable function · Other"
type: "api"
api_tabs: true
---


**Tier:** stable · **Kind:** function · **Topic:** [Other](/generated/api/topics/other)

| Python | Julia | Rust |
|---|---|---|
| `expand_esm_template` | `expand_esm_template` | `expand_esm_template` |


<div class="api-tabs">

<div class="api-tab" data-lang="python" data-label="Python">
<p class="api-tab-label">Python</p>

```python
expand_esm_template(template: str, anchor: _dt.datetime | None = None, substitutions: Mapping[str, str] | None = None) -> str
```

Expand an esm URL template at `anchor` (a UTC instant; a naive datetime
is read as UTC) with `substitutions`. See spec/esm-sources.md §1.

[Source](https://github.com/EarthSciML/EarthSciIO/blob/065acc28eb3328a3fa3a580d78e7222f1e92f6bf/earthsciio/esm.py#L94)

</div>

<div class="api-tab" data-lang="julia" data-label="Julia">
<p class="api-tab-label">Julia</p>

```julia
expand_esm_template(template::AbstractString; ...)
expand_esm_template(template::AbstractString, anchor; substitutions)
```

```julia
expand_esm_template(template, anchor=nothing; substitutions=Dict()) -> String
```

Expand an esm URL template (spec/esm-sources.md §1) at `anchor` — a UTC
`DateTime`, Unix seconds, or `nothing` — with `substitutions` (name => string).
Errors are [`EsmTemplateError`](/generated/api/ref/esm_template_error)s carrying a §1.3 code.

[Source](https://github.com/EarthSciML/EarthSciIO/blob/065acc28eb3328a3fa3a580d78e7222f1e92f6bf/julia/src/esm.jl#L86)

</div>

<div class="api-tab" data-lang="rust" data-label="Rust">
<p class="api-tab-label">Rust</p>

```rust
pub fn expand_esm_template(template: &str, anchor: Option<OffsetDateTime>, substitutions: &BTreeMap<String, String>) -> Result<String, EsmTemplateError>
```

Expand an esm URL template (`spec/esm-sources.md` §1) at `anchor` (a UTC
instant; an offset is converted to UTC) with `substitutions`.

[Source](https://github.com/EarthSciML/EarthSciIO/blob/065acc28eb3328a3fa3a580d78e7222f1e92f6bf/rust/src/esm.rs#L169) · [Rust docs](https://docs.rs/earthsciio/latest/earthsciio/esm/fn.expand_esm_template.html)

</div>

</div>
