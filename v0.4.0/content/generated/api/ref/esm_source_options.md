---
title: "esm_source_options"
description: "Stable type · Other"
type: "api"
api_tabs: true
---


**Tier:** stable · **Kind:** type · **Topic:** [Other](/generated/api/topics/other)

| Python | Julia | Rust |
|---|---|---|
| not exported | not exported | `EsmSourceOptions` |


<div class="api-tabs">

<div class="api-tab" data-lang="python" data-label="Python">
<p class="api-tab-label">Python</p>

<p class="api-absent">Python does not export this type.</p>

</div>

<div class="api-tab" data-lang="julia" data-label="Julia">
<p class="api-tab-label">Julia</p>

<p class="api-absent">Julia does not export this type.</p>

</div>

<div class="api-tab" data-lang="rust" data-label="Rust">
<p class="api-tab-label">Rust</p>

```rust
pub struct EsmSourceOptions {
    pub variables: Vec<String>,
    pub substitutions: BTreeMap<String, String>,
    pub url_override: Option<String>,
    pub records_per_sample: Option<u32>,
}
```

What the caller adds to an esm entry when building a loader from it
([`DataSource::from_esm`](/generated/api/ref/source_from_esm)).

[Source](https://github.com/EarthSciML/EarthSciIO/blob/770dbc22d557e6e2ae87d9bccd7386f45764e9d0/rust/src/esm_source.rs#L25) · [Rust docs](https://docs.rs/earthsciio/latest/earthsciio/esm_source/struct.EsmSourceOptions.html)

</div>

</div>
