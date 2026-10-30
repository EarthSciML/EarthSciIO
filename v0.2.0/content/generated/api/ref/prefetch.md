---
title: "prefetch"
description: "Stable function · Providers"
type: "api"
api_tabs: true
---


**Tier:** stable · **Kind:** function · **Topic:** [Providers](/generated/api/topics/providers)

| Python | Julia | Rust |
|---|---|---|
| not exported | `prefetch` | not exported |


<div class="api-tabs">

<div class="api-tab" data-lang="python" data-label="Python">
<p class="api-tab-label">Python</p>

<p class="api-absent">Python does not export this function.</p>

</div>

<div class="api-tab" data-lang="julia" data-label="Julia">
<p class="api-tab-label">Julia</p>

```julia
prefetch(p::Provider)
```

```julia
prefetch(p::Provider) -> Vector{CacheEntry}
```

Warm the cache for every URL the provider will need — the single URL for a
[`CONST`] provider, the unique per-tick URLs for a [`DISCRETE`] one — WITHOUT
decoding. Lets a caller pull all blobs up front (e.g. before a solve, or while
online) so later [`materialize`]/[`refresh`] calls hit a warm, offline-readable
cache. Returns the [`CacheEntry`] for each.

[Source](https://github.com/EarthSciML/EarthSciIO/blob/a49538bbd6ec318dde369d3496f6e8880586a73f/julia/src/provider.jl#L683)

</div>

<div class="api-tab" data-lang="rust" data-label="Rust">
<p class="api-tab-label">Rust</p>

<p class="api-absent">Rust does not export this function.</p>

</div>

</div>
