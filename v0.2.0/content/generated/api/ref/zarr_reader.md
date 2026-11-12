---
title: "zarr_reader"
description: "Stable type · Native arrays and readers"
type: "api"
api_tabs: true
---


**Tier:** stable · **Kind:** type · **Topic:** [Native arrays and readers](/generated/api/topics/native-arrays)

| Python | Julia | Rust |
|---|---|---|
| not exported | `ZarrReader` | `ZarrReader` |


<div class="api-tabs">

<div class="api-tab" data-lang="python" data-label="Python">
<p class="api-tab-label">Python</p>

<p class="api-absent">Python does not export this type.</p>

</div>

<div class="api-tab" data-lang="julia" data-label="Julia">
<p class="api-tab-label">Julia</p>

```julia
struct ZarrReader <: Reader
ZarrReader()
```

The `zarr` store-backed reader (Zarr v2 and sharded Zarr v3 chunked arrays).

[Source](https://github.com/EarthSciML/EarthSciIO/blob/a49538bbd6ec318dde369d3496f6e8880586a73f/julia/src/zarr.jl#L26)

</div>

<div class="api-tab" data-lang="rust" data-label="Rust">
<p class="api-tab-label">Rust</p>

```rust
pub struct ZarrReader;
impl ZarrReader { pub fn new() -> Self }
```

The store-backed `zarr` reader (Zarr v3 + v2 chunked arrays, via `zarrs`).

[Source](https://github.com/EarthSciML/EarthSciIO/blob/a49538bbd6ec318dde369d3496f6e8880586a73f/rust/src/format/zarr.rs#L54) · [Rust docs](https://docs.rs/earthsciio/latest/earthsciio/struct.ZarrReader.html)

</div>

</div>
