---
title: "epoch_sample"
description: "Stable type · Prefetch and windows"
type: "api"
api_tabs: true
---


**Tier:** stable · **Kind:** type · **Topic:** [Prefetch and windows](/generated/api/topics/prefetch)

| Python | Julia | Rust |
|---|---|---|
| `EpochSample` | `EpochSample` | `EpochSample` |


<div class="api-tabs">

<div class="api-tab" data-lang="python" data-label="Python">
<p class="api-tab-label">Python</p>

```python
class EpochSample(Mapping)
EpochSample(epoch: int, time: _dt.datetime, samples: dict[str, Sample])
EpochSample.__getitem__(key: str) -> Sample
EpochSample.__iter__() -> Iterator[str]
EpochSample.__len__() -> int
```

Every member's [`Sample`](/generated/api/ref/prefetch_sample) for one epoch: `ep["U"].data`.

A read-only mapping keyed by member, plus `epoch` (the epoch index
`k`) and `time` (its start boundary `B[k]`).

[Source](https://github.com/EarthSciML/EarthSciIO/blob/a49538bbd6ec318dde369d3496f6e8880586a73f/earthsciio/prefetch.py#L107)

</div>

<div class="api-tab" data-lang="julia" data-label="Julia">
<p class="api-tab-label">Julia</p>

```julia
struct EpochSample
EpochSample(epoch::Int64, time::Float64, samples::Dict{String, Sample})
EpochSample(epoch, time, samples)
```

```julia
EpochSample
```

What [`take!`](api:take!(::PrefetchGroup, ::Integer)) returns for one epoch: a
[`Sample`] per member, indexed by member key (`ep["U"].data`, `ep["U"].anchor`,
`ep["U"].changed`). `epoch` is the (1-based) epoch index and `time` its boundary
`B[epoch]`. Iterates as `key => Sample` pairs.

[Source](https://github.com/EarthSciML/EarthSciIO/blob/a49538bbd6ec318dde369d3496f6e8880586a73f/julia/src/prefetch.jl#L44)

</div>

<div class="api-tab" data-lang="rust" data-label="Rust">
<p class="api-tab-label">Rust</p>

```rust
pub struct EpochSample<T> {
    /* private fields */
}

impl<T> EpochSample<T> { pub fn epoch(&self) -> usize }

impl<T> EpochSample<T> { pub fn get(&self, key: &str) -> Option<&Sample<T>> }

impl<T> EpochSample<T> { pub fn iter(&self) -> impl Iterator<Item = (&str, &Sample<T>)> }

impl<T> EpochSample<T> { pub fn len(&self) -> usize }

impl<T> EpochSample<T> { pub fn is_empty(&self) -> bool }

impl<T> EpochSample<T> { pub fn into_samples(self) -> BTreeMap<String, Sample<T>> }
```

What `PrefetchGroup::take` returns: one [`Sample`](/generated/api/ref/prefetch_sample) per member key.

[Source](https://github.com/EarthSciML/EarthSciIO/blob/a49538bbd6ec318dde369d3496f6e8880586a73f/rust/src/prefetch.rs#L101) · [Rust docs](https://docs.rs/earthsciio/latest/earthsciio/prefetch/struct.EpochSample.html)

</div>

</div>
