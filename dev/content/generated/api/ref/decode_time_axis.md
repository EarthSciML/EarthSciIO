---
title: "decode_time_axis"
description: "Stable function · Other"
type: "api"
api_tabs: true
---


**Tier:** stable · **Kind:** function · **Topic:** [Other](/generated/api/topics/other)

| Python | Julia | Rust |
|---|---|---|
| `decode_time_axis` | `decode_time_axis` | `decode_time_axis` |


<div class="api-tabs">

<div class="api-tab" data-lang="python" data-label="Python">
<p class="api-tab-label">Python</p>

```python
decode_time_axis(values: Any, units: str | None, calendar: str | None = None) -> list[int] | None
```

The Unix seconds (UTC, floored to the second) of a raw CF time axis, or
`None` when it cannot be decoded (`spec/esm-sources.md` §2.5).

This is the decode [`Provider`](/generated/api/ref/provider) uses to find a record in a file by the
file's own time axis. `None` means the provider falls back to the
cadence offset: no `units`, units without `since`, a calendar other
than `standard`/`gregorian`/`proleptic_gregorian`, a missing entry,
or a date that is not representable — outside 1677-09-21 to 2262-04-11
when pandas decodes it, outside the years 1678 to 2261 when cftime does
(spec/esm-sources.md §2.5 says which decodes which).

[Source](https://github.com/EarthSciML/EarthSciIO/blob/065acc28eb3328a3fa3a580d78e7222f1e92f6bf/earthsciio/provider.py#L1046)

</div>

<div class="api-tab" data-lang="julia" data-label="Julia">
<p class="api-tab-label">Julia</p>

```julia
decode_time_axis(values, units; ...)
decode_time_axis(values, units, calendar; fill)
decode_time_axis(coord::NativeField)
```

```julia
decode_time_axis(values, units, calendar = nothing; fill = nothing)
    -> Union{Nothing, Vector{Int64}}
decode_time_axis(coord::NativeField) -> Union{Nothing, Vector{Int64}}
```

The Unix seconds (UTC, floored to the second) of a raw CF time axis, or
`nothing` when it is not decoded (spec/esm-sources.md §2.5). A `temporal`
[`Provider`] uses this to find a record in a file by the file's own time axis,
and falls back to the cadence offset when it gets `nothing`: no `units`, units
without `since`, a calendar other than `standard`, `gregorian` or
`proleptic_gregorian`, a NaN or `fill` value, or a date that is not
representable (outside 1677-09-21 to 2262-04-11 on the first reading, outside
the years 1678 to 2261 on the second). The decode is the Python track's
(xarray's), rule by rule. The `NativeField` form reads `units`, `calendar`,
`_FillValue` and `missing_value` from the field's attributes, and applies a
`scale_factor`/`add_offset` after the fill check, as the Python reader does.

[Source](https://github.com/EarthSciML/EarthSciIO/blob/065acc28eb3328a3fa3a580d78e7222f1e92f6bf/julia/src/time_axis.jl#L239)

</div>

<div class="api-tab" data-lang="rust" data-label="Rust">
<p class="api-tab-label">Rust</p>

```rust
pub fn decode_time_axis(values: &ArrayData, units: Option<&str>, calendar: Option<&str>, fill: Option<f64>) -> Option<Vec<i64>>
```

The Unix seconds (UTC, floored to the second) of a raw CF time axis, or
`None` when it is not decoded (`spec/esm-sources.md` §2.5).

A DISCRETE [`Provider`](/generated/api/ref/provider) uses this to find a record in a file
by the file's own time axis, and falls back to the cadence offset on `None`:
no `units`, units without `since`, a calendar other than `standard`,
`gregorian` or `proleptic_gregorian`, a NaN or `fill` value, or a date that
is not representable (outside 1677-09-21 to 2262-04-11 on the first reading,
outside the years 1678 to 2261 on the second). The decode is the Python
track's (xarray's), rule by rule.

[Source](https://github.com/EarthSciML/EarthSciIO/blob/065acc28eb3328a3fa3a580d78e7222f1e92f6bf/rust/src/time_axis.rs#L404) · [Rust docs](https://docs.rs/earthsciio/latest/earthsciio/time_axis/fn.decode_time_axis.html)

</div>

</div>
