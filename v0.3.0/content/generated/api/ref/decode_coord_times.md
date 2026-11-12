---
title: "decode_coord_times"
description: "Stable function · Other"
type: "api"
api_tabs: true
---


**Tier:** stable · **Kind:** function · **Topic:** [Other](/generated/api/topics/other)

| Python | Julia | Rust |
|---|---|---|
| not exported | not exported | `decode_coord_times` |


<div class="api-tabs">

<div class="api-tab" data-lang="python" data-label="Python">
<p class="api-tab-label">Python</p>

<p class="api-absent">Python does not export this function.</p>

</div>

<div class="api-tab" data-lang="julia" data-label="Julia">
<p class="api-tab-label">Julia</p>

<p class="api-absent">Julia does not export this function.</p>

</div>

<div class="api-tab" data-lang="rust" data-label="Rust">
<p class="api-tab-label">Rust</p>

```rust
pub fn decode_coord_times(coord: &Coord) -> Option<Vec<i64>>
```

[`decode_time_axis`](/generated/api/ref/decode_time_axis) of a decoded coordinate: its values, `units`,
`calendar` and the fill value the reader kept.

[Source](https://github.com/EarthSciML/EarthSciIO/blob/065acc28eb3328a3fa3a580d78e7222f1e92f6bf/rust/src/time_axis.rs#L484) · [Rust docs](https://docs.rs/earthsciio/latest/earthsciio/time_axis/fn.decode_coord_times.html)

</div>

</div>
