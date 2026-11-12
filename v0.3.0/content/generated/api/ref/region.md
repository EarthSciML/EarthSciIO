---
title: "region"
description: "Stable type · Streaming writer"
type: "api"
api_tabs: true
---


**Tier:** stable · **Kind:** type · **Topic:** [Streaming writer](/generated/api/topics/writer)

| Python | Julia | Rust |
|---|---|---|
| not exported | not exported | `Region` |


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
pub struct Region(pub Vec<(String, Range<usize>)>);
impl Region { pub fn whole() -> Self }
impl Region { pub fn new<I, D>(dims: I) -> Self where I: IntoIterator<Item = (D, Range<usize>)>, D: Into<String> }
```

A block of the non-time dims, `dim → start..stop` (0-based, half-open). An
omitted dim means its full extent; the empty region is the whole array.

[Source](https://github.com/EarthSciML/EarthSciIO/blob/065acc28eb3328a3fa3a580d78e7222f1e92f6bf/rust/src/format/zarr_stream.rs#L321) · [Rust docs](https://docs.rs/earthsciio/latest/earthsciio/format/zarr_stream/struct.Region.html)

</div>

</div>
