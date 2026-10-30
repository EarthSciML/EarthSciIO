---
title: "indexed_time"
description: "Stable type · Streaming writer"
type: "api"
api_tabs: true
---


**Tier:** stable · **Kind:** type · **Topic:** [Streaming writer](/generated/api/topics/writer)

| Python | Julia | Rust |
|---|---|---|
| not exported | `IndexedTime` | `TimeMode` |


<div class="api-tabs">

<div class="api-tab" data-lang="python" data-label="Python">
<p class="api-tab-label">Python</p>

<p class="api-absent">Python does not export this type.</p>

</div>

<div class="api-tab" data-lang="julia" data-label="Julia">
<p class="api-tab-label">Julia</p>

```julia
struct IndexedTime
IndexedTime(times::Vector{Float64})
IndexedTime(times)
IndexedTime(times::AbstractVector)
```

```julia
IndexedTime(times)
```

The `indexed` time mode (spec/writer.md §6.2): the time axis has a fixed length
`n_records = length(times)` from [`write_open!`], the owner writes `times` as the
time coordinate at open, and records are written by index with
[`write_record_at!`], in any order. The default time mode is `:append`.

[Source](https://github.com/EarthSciML/EarthSciIO/blob/a49538bbd6ec318dde369d3496f6e8880586a73f/julia/src/zarr_write.jl#L93)

</div>

<div class="api-tab" data-lang="rust" data-label="Rust">
<p class="api-tab-label">Rust</p>

```rust
pub enum TimeMode {
    Append,
    Indexed { n_records: usize, times: Vec<f64> },
}
```

How the record axis is addressed (`spec/writer.md` §6).

[Source](https://github.com/EarthSciML/EarthSciIO/blob/a49538bbd6ec318dde369d3496f6e8880586a73f/rust/src/format/zarr_stream.rs#L279) · [Rust docs](https://docs.rs/earthsciio/latest/earthsciio/format/zarr_stream/enum.TimeMode.html)

</div>

</div>
