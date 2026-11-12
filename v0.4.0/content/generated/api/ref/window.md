---
title: "window"
description: "Stable type · Providers"
type: "api"
api_tabs: true
---


**Tier:** stable · **Kind:** type · **Topic:** [Providers](/generated/api/topics/providers)

| Python | Julia | Rust |
|---|---|---|
| `Window` | not exported | `Window` |


<div class="api-tabs">

<div class="api-tab" data-lang="python" data-label="Python">
<p class="api-tab-label">Python</p>

```python
Window = tuple[TimeLike, TimeLike]
```

A run window `(start, end)` — half-open `[start, end)`.

[Source](https://github.com/EarthSciML/EarthSciIO/blob/770dbc22d557e6e2ae87d9bccd7386f45764e9d0/earthsciio/provider.py#L84)

</div>

<div class="api-tab" data-lang="julia" data-label="Julia">
<p class="api-tab-label">Julia</p>

<p class="api-absent">Julia does not export this type.</p>

</div>

<div class="api-tab" data-lang="rust" data-label="Rust">
<p class="api-tab-label">Rust</p>

```rust
pub type Window = (OffsetDateTime, OffsetDateTime);
```

A run window `(start, end)` — half-open `[start, end)`.

[Source](https://github.com/EarthSciML/EarthSciIO/blob/770dbc22d557e6e2ae87d9bccd7386f45764e9d0/rust/src/provider.rs#L53) · [Rust docs](https://docs.rs/earthsciio/latest/earthsciio/type.Window.html)

</div>

</div>
