---
title: "esm_template_names"
description: "Stable function · Other"
type: "api"
api_tabs: true
---


**Tier:** stable · **Kind:** function · **Topic:** [Other](/generated/api/topics/other)

| Python | Julia | Rust |
|---|---|---|
| `esm_template_names` | `esm_template_names` | `esm_template_names` |


<div class="api-tabs">

<div class="api-tab" data-lang="python" data-label="Python">
<p class="api-tab-label">Python</p>

```python
esm_template_names(template: str) -> list[str]
```

The substitution names `template` needs, in order of first appearance,
without `date`.

[Source](https://github.com/EarthSciML/EarthSciIO/blob/770dbc22d557e6e2ae87d9bccd7386f45764e9d0/earthsciio/esm.py#L143)

</div>

<div class="api-tab" data-lang="julia" data-label="Julia">
<p class="api-tab-label">Julia</p>

```julia
esm_template_names(template::AbstractString)
```

```julia
esm_template_names(template) -> Vector{String}
```

The substitution names `template` needs, in order of first appearance, without
`date`.

[Source](https://github.com/EarthSciML/EarthSciIO/blob/770dbc22d557e6e2ae87d9bccd7386f45764e9d0/julia/src/esm.jl#L146)

</div>

<div class="api-tab" data-lang="rust" data-label="Rust">
<p class="api-tab-label">Rust</p>

```rust
pub fn esm_template_names(template: &str) -> Vec<String>
```

The substitution names `template` needs, in order of first appearance,
without `date`.

[Source](https://github.com/EarthSciML/EarthSciIO/blob/770dbc22d557e6e2ae87d9bccd7386f45764e9d0/rust/src/esm.rs#L230) · [Rust docs](https://docs.rs/earthsciio/latest/earthsciio/esm/fn.esm_template_names.html)

</div>

</div>
