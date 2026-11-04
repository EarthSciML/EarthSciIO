---
title: "region_not_shard_aligned"
description: "Stable type · Errors"
type: "api"
api_tabs: true
---


**Tier:** stable · **Kind:** type · **Topic:** [Errors](/generated/api/topics/errors)

| Python | Julia | Rust |
|---|---|---|
| `RegionNotShardAligned` | `RegionNotShardAligned` | not exported |


<div class="api-tabs">

<div class="api-tab" data-lang="python" data-label="Python">
<p class="api-tab-label">Python</p>

```python
class RegionNotShardAligned(EarthSciIOError, ValueError)
```

A writer region violates the alignment rule of `spec/writer.md` §4.

For every region dim `d`: `0 <= start < stop <= len(d)`,
`start % shard[d] == 0` and `stop % shard[d] == 0 or stop == len(d)`.
Raised at open / join / resume, before any write, so every shard object of
a partitioned array is owned by exactly one writer.

[Source](https://github.com/EarthSciML/EarthSciIO/blob/a49538bbd6ec318dde369d3496f6e8880586a73f/earthsciio/errors.py#L305)

</div>

<div class="api-tab" data-lang="julia" data-label="Julia">
<p class="api-tab-label">Julia</p>

```julia
struct RegionNotShardAligned <: Exception
RegionNotShardAligned(msg::String)
RegionNotShardAligned(msg)
```

```julia
RegionNotShardAligned(msg)
```

A writer's region (spec/writer.md §4) does not start on a shard boundary, does
not stop on one (or at the array end), or is out of bounds. Raised by
[`write_open!`]/[`write_resume`] before any I/O.

[Source](https://github.com/EarthSciML/EarthSciIO/blob/a49538bbd6ec318dde369d3496f6e8880586a73f/julia/src/zarr_stream.jl#L22)

</div>

<div class="api-tab" data-lang="rust" data-label="Rust">
<p class="api-tab-label">Rust</p>

<p class="api-absent">Rust does not export this type.</p>

</div>

</div>
