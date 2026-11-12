---
title: "provider_time"
description: "Stable type · Other"
type: "api"
api_tabs: true
---


**Tier:** stable · **Kind:** type · **Topic:** [Other](/generated/api/topics/other)

| Python | Julia | Rust |
|---|---|---|
| `TimeLike` | not exported | `ProviderTime` |


<div class="api-tabs">

<div class="api-tab" data-lang="python" data-label="Python">
<p class="api-tab-label">Python</p>

```python
TimeLike = Union[_dt.datetime, float, int]
```

A time a provider takes: a `datetime.datetime` (an instant; naive is
UTC), or a number of seconds from the provider's `time_origin` (the Unix
epoch when it has none), spec/esm-sources.md §4.

[Source](https://github.com/EarthSciML/EarthSciIO/blob/770dbc22d557e6e2ae87d9bccd7386f45764e9d0/earthsciio/provider.py#L81)

</div>

<div class="api-tab" data-lang="julia" data-label="Julia">
<p class="api-tab-label">Julia</p>

<p class="api-absent">Julia does not export this type.</p>

</div>

<div class="api-tab" data-lang="rust" data-label="Rust">
<p class="api-tab-label">Rust</p>

```rust
pub enum ProviderTime {
    Instant(OffsetDateTime),
    Seconds(f64),
}
```

A time a [`Provider`](/generated/api/ref/provider) takes (`spec/esm-sources.md` §4): an instant, or a
number of seconds from the loader's `time_origin`
(the Unix epoch when it has none). `OffsetDateTime` and `f64` convert into
it, so `p.sample_at(t)` takes either.

[Source](https://github.com/EarthSciML/EarthSciIO/blob/770dbc22d557e6e2ae87d9bccd7386f45764e9d0/rust/src/provider.rs#L60) · [Rust docs](https://docs.rs/earthsciio/latest/earthsciio/enum.ProviderTime.html)

</div>

</div>
