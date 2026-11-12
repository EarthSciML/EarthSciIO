---
title: "temporal"
description: "Stable type · Cache"
type: "api"
api_tabs: true
---


**Tier:** stable · **Kind:** type · **Topic:** [Cache](/generated/api/topics/cache)

| Python | Julia | Rust |
|---|---|---|
| `Temporal` | not exported | `Temporal` |


<div class="api-tabs">

<div class="api-tab" data-lang="python" data-label="Python">
<p class="api-tab-label">Python</p>

```python
class Temporal
Temporal(kind: _Kind, ttl: _dt.timedelta | None = None)
@classmethod Temporal.static() -> Temporal
@classmethod Temporal.closed_period() -> Temporal
@classmethod Temporal.incomplete(ttl: _dt.timedelta | float | int) -> Temporal
Temporal.immutable: bool  # property
```

The freshness policy derived from a loader's `temporal` block.

Build via the classmethods rather than the raw constructor:

* `static` — no `temporal` block; immutable once fetched.
* `closed_period` — a closed past period (e.g. `file_period:P1D` for a
  past date); immutable, infinite TTL.
* `incomplete` — a current/incomplete period; fresh only until `ttl`.

**Members**

- `immutable` — True for static + closed-period loaders (infinite TTL).

[Source](https://github.com/EarthSciML/EarthSciIO/blob/a49538bbd6ec318dde369d3496f6e8880586a73f/earthsciio/validate.py#L66)

</div>

<div class="api-tab" data-lang="julia" data-label="Julia">
<p class="api-tab-label">Julia</p>

<p class="api-absent">Julia does not export this type.</p>

</div>

<div class="api-tab" data-lang="rust" data-label="Rust">
<p class="api-tab-label">Rust</p>

```rust
pub enum Temporal {
    Static,
    ClosedPeriod,
    Incomplete { ttl: Duration },
}

impl Temporal { pub fn is_fresh(&self, fetched_at: &str, now: OffsetDateTime) -> bool }
```

A loader's temporal nature, supplying the TTL rung of the ladder. Lets a
present blob be judged valid **without** a network round-trip.

[Source](https://github.com/EarthSciML/EarthSciIO/blob/a49538bbd6ec318dde369d3496f6e8880586a73f/rust/src/validate.rs#L42) · [Rust docs](https://docs.rs/earthsciio/latest/earthsciio/validate/enum.Temporal.html)

</div>

</div>
