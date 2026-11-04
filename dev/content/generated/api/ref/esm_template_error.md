---
title: "esm_template_error"
description: "Stable error · Other"
type: "api"
api_tabs: true
---


**Tier:** stable · **Kind:** error · **Topic:** [Other](/generated/api/topics/other)

| Python | Julia | Rust |
|---|---|---|
| `EsmTemplateError` | `EsmTemplateError` | `EsmTemplateError` |


<div class="api-tabs">

<div class="api-tab" data-lang="python" data-label="Python">
<p class="api-tab-label">Python</p>

```python
class EsmTemplateError(EarthSciIOError, ValueError)
EsmTemplateError(code: str, template: str, detail: str)
```

An esm URL template could not be expanded. `code` is one of the
`esm_template_*` codes of spec/esm-sources.md §1.3, and the message
contains it.

[Source](https://github.com/EarthSciML/EarthSciIO/blob/065acc28eb3328a3fa3a580d78e7222f1e92f6bf/earthsciio/esm.py#L29)

</div>

<div class="api-tab" data-lang="julia" data-label="Julia">
<p class="api-tab-label">Julia</p>

```julia
struct EsmTemplateError <: Exception
EsmTemplateError(code::String, template::String, detail::String)
EsmTemplateError(code, template, detail)
```

```julia
EsmTemplateError(code, template, detail)
```

An esm URL template could not be expanded. `code` is one of the
`esm_template_*` codes of spec/esm-sources.md §1.3; the message contains it.

[Source](https://github.com/EarthSciML/EarthSciIO/blob/065acc28eb3328a3fa3a580d78e7222f1e92f6bf/julia/src/esm.jl#L8)

</div>

<div class="api-tab" data-lang="rust" data-label="Rust">
<p class="api-tab-label">Rust</p>

```rust
pub struct EsmTemplateError {
    pub code: &'static str,
    pub template: String,
    pub detail: String,
}
```

An esm URL template could not be expanded. `code` is one of the
`esm_template_*` codes of `spec/esm-sources.md` §1.3; the message (its
`Display`) contains it.

[Source](https://github.com/EarthSciML/EarthSciIO/blob/065acc28eb3328a3fa3a580d78e7222f1e92f6bf/rust/src/esm.rs#L18) · [Rust docs](https://docs.rs/earthsciio/latest/earthsciio/esm/struct.EsmTemplateError.html)

</div>

</div>
