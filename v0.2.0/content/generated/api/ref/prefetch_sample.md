---
title: "prefetch_sample"
description: "Stable type · Prefetch and windows"
type: "api"
api_tabs: true
---


**Tier:** stable · **Kind:** type · **Topic:** [Prefetch and windows](/generated/api/topics/prefetch)

| Python | Julia | Rust |
|---|---|---|
| `Sample` | `Sample` | `Sample` |


<div class="api-tabs">

<div class="api-tab" data-lang="python" data-label="Python">
<p class="api-tab-label">Python</p>

```python
class Sample(NamedTuple)
```

One member's data for one epoch.

`data` is `members[key].sample_at(B[k])` (with `windows`, the list
`sample_windows_at(B[k], windows)`; with `transform`, `transform(key, <that>)`). It is **read-only** — the same object may be handed to several
epochs when the member's anchor repeats, so its numpy arrays are marked
non-writeable; copy before mutating. `anchor` is the member's cadence
anchor for `B[k]`. `changed` says whether `anchor` differs from this
member's anchor at the previous `take` of the current order (always true
on the first `take` after `start`): copy only the changed samples into
the forcing buffers.

[Source](https://github.com/EarthSciML/EarthSciIO/blob/a49538bbd6ec318dde369d3496f6e8880586a73f/earthsciio/prefetch.py#L88)

</div>

<div class="api-tab" data-lang="julia" data-label="Julia">
<p class="api-tab-label">Julia</p>

```julia
struct Sample
Sample(data::T, anchor::Float64, changed::Bool) where {T}
```

```julia
Sample{T}
```

One member's sample for one epoch, as [`take!`](api:take!(::PrefetchGroup, ::Integer))
returns it inside an [`EpochSample`]:

  * `data` — the member's [`sample_at`]`(B[k])` (a `NativeDataset`); with
    `windows`, the `Vector` [`sample_windows_at`]`(B[k], windows)`; with
    `transform`, `transform(key, <that>)`. **Read-only**: the same object is handed
    to every epoch that shares the anchor, so a caller that mutates copies first.
  * `anchor` — the member's cadence anchor for `B[k]`: the tick value from its
    `refresh_times` that `B[k]` snaps down to.
  * `changed` — whether `anchor` differs from this member's anchor at the
    previous `take!` of the current order; always `true` on the first `take!`
    after `start!`. A caller copies only the changed samples into its forcing
    buffers.

[Source](https://github.com/EarthSciML/EarthSciIO/blob/a49538bbd6ec318dde369d3496f6e8880586a73f/julia/src/prefetch.jl#L22)

</div>

<div class="api-tab" data-lang="rust" data-label="Rust">
<p class="api-tab-label">Rust</p>

```rust
pub struct Sample<T> {
    pub data: Arc<T>,
    pub anchor: f64,
    pub changed: bool,
}
```

One member's part of an epoch (`spec/prefetch.md` §3.4).

[Source](https://github.com/EarthSciML/EarthSciIO/blob/a49538bbd6ec318dde369d3496f6e8880586a73f/rust/src/prefetch.rs#L74) · [Rust docs](https://docs.rs/earthsciio/latest/earthsciio/prefetch/struct.Sample.html)

</div>

</div>
