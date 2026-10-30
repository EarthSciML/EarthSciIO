---
title: "sample_at"
description: "Stable function · Prefetch and windows"
type: "api"
api_tabs: true
---


**Tier:** stable · **Kind:** function · **Topic:** [Prefetch and windows](/generated/api/topics/prefetch)

| Python | Julia | Rust |
|---|---|---|
| not exported | `sample_at` | not exported |


<div class="api-tabs">

<div class="api-tab" data-lang="python" data-label="Python">
<p class="api-tab-label">Python</p>

<p class="api-absent">Python does not export this function.</p>

</div>

<div class="api-tab" data-lang="julia" data-label="Julia">
<p class="api-tab-label">Julia</p>

```julia
sample_at(p::Provider, t::Real)
sample_at(p::Provider)
```

```julia
sample_at(p::Provider, t::Real) -> NativeDataset
sample_at(p::Provider) -> NativeDataset
```

The dataset `refresh(p, t)` returns — data variables AND coordinates, with the same
snap-down to the cadence anchor, the same file-local record, the same
`records_per_sample = 2` bracket and the same end-of-data rules — and it raises
what `refresh(p, t)` raises (spec/prefetch.md §1). It depends on `t` only through
the anchor `t` snaps to, so two times in one cadence interval give equal samples.
The no-argument form is a [`CONST`] provider's [`materialize`]`(p)`.

It is safe to call from several threads at once on one provider, which is what
[`PrefetchGroup`] does. In the Python and Rust tracks `sample_at` is new because
their `refresh` updates a decoded-file buffer; the Julia [`Provider`] holds no
buffer at all (immutable, decoding per sample with its `records` pushdown), so
here it is the same function as [`materialize`], under the name the cross-track
contract gives it. The shared state it does touch is thread-safe: the cache takes
a per-blob lock to fill a miss, and every NetCDF-C access is serialised by one
process-wide lock.

[Source](https://github.com/EarthSciML/EarthSciIO/blob/a49538bbd6ec318dde369d3496f6e8880586a73f/julia/src/provider.jl#L661)

</div>

<div class="api-tab" data-lang="rust" data-label="Rust">
<p class="api-tab-label">Rust</p>

<p class="api-absent">Rust does not export this function.</p>

</div>

</div>
