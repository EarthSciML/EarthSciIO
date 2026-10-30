---
title: "writer_role"
description: "Stable type · Streaming writer"
type: "api"
api_tabs: true
---


**Tier:** stable · **Kind:** type · **Topic:** [Streaming writer](/generated/api/topics/writer)

| Python | Julia | Rust |
|---|---|---|
| not exported | not exported | `Role` |


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
pub enum Role {
    Single,
    Owner { n_writers: usize },
    Contributor { id: usize },
}
```

A writer's role in a store (`spec/writer.md` §5).

[Source](https://github.com/EarthSciML/EarthSciIO/blob/a49538bbd6ec318dde369d3496f6e8880586a73f/rust/src/format/zarr_stream.rs#L294) · [Rust docs](https://docs.rs/earthsciio/latest/earthsciio/format/zarr_stream/enum.Role.html)

</div>

</div>
