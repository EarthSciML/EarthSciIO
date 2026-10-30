---
title: "prefetch_order"
description: "Stable type · Prefetch and windows"
type: "api"
api_tabs: true
---


**Tier:** stable · **Kind:** type · **Topic:** [Prefetch and windows](/generated/api/topics/prefetch)

| Python | Julia | Rust |
|---|---|---|
| not exported | not exported | `Order` |


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
pub enum Order {
    Forward,
    Reverse,
    Explicit(Vec<usize>),
}
```

The order in which epochs will be taken (`spec/prefetch.md` §3.3).

[Source](https://github.com/EarthSciML/EarthSciIO/blob/a49538bbd6ec318dde369d3496f6e8880586a73f/rust/src/prefetch.rs#L62) · [Rust docs](https://docs.rs/earthsciio/latest/earthsciio/prefetch/enum.Order.html)

</div>

</div>
