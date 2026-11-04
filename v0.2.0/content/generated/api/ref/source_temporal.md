---
title: "source_temporal"
description: "Stable type · Providers"
type: "api"
api_tabs: true
---


**Tier:** stable · **Kind:** type · **Topic:** [Providers](/generated/api/topics/providers)

| Python | Julia | Rust |
|---|---|---|
| `SourceTemporal` | not exported | `SourceTemporal` |


<div class="api-tabs">

<div class="api-tab" data-lang="python" data-label="Python">
<p class="api-tab-label">Python</p>

```python
class SourceTemporal
SourceTemporal(start: _dt.datetime, frequency: _dt.timedelta, file_period: _dt.timedelta, end: _dt.datetime | None = None, time_dim: str = 'time', records_per_sample: int | None = None)
```

The temporal cadence of a DISCRETE loader (absent ⇒ CONST).

`frequency` is the cadence step (e.g. 1 hour for ERA5) — it drives the
refresh tstops and which record within a file is current. `file_period` is
the span of one file (e.g. 1 day) — it drives URL resolution; a file holds
`file_period / frequency` records along `time_dim`. Anchors are aligned
to `start` (the loader epoch). `end` (optional) is the exclusive end of
available data and bounds `Provider.refresh_times` when no run window
is given.

`records_per_sample` is HOW MANY time records `Provider.refresh`
returns per query time — pure I/O; the loader does not interpolate. `None`
or `1` (default) returns the single at-or-before record with `time_dim`
dropped (held piecewise-constant); `2` returns the two bracketing records
(floor + successor) with `time_dim` retained at length 2 and a canonical
2-element `time_dim` coordinate of Unix epoch seconds, so a downstream model
can interpolate in time. The successor is read across a file boundary when
needed; at the last available record the bracket degenerates to `[last, last]`
so the downstream weight clamps. Only 1 and 2 are supported; higher-order
temporal stencils are future work. (Distinct from a per-file record count.)

[Source](https://github.com/EarthSciML/EarthSciIO/blob/a49538bbd6ec318dde369d3496f6e8880586a73f/earthsciio/provider.py#L86)

</div>

<div class="api-tab" data-lang="julia" data-label="Julia">
<p class="api-tab-label">Julia</p>

<p class="api-absent">Julia does not export this type.</p>

</div>

<div class="api-tab" data-lang="rust" data-label="Rust">
<p class="api-tab-label">Rust</p>

```rust
pub struct SourceTemporal {
    pub start: OffsetDateTime,
    pub end: Option<OffsetDateTime>,
    pub frequency: Duration,
    pub file_period: Duration,
    pub time_dim: String,
    pub records_per_sample: Option<u32>,
}

impl SourceTemporal { pub fn new(start: OffsetDateTime, frequency: Duration, file_period: Duration) -> Self }

impl SourceTemporal { pub fn end(self, end: OffsetDateTime) -> Self }

impl SourceTemporal { pub fn time_dim(self, dim: impl Into<String>) -> Self }

impl SourceTemporal { pub fn records_per_sample(self, n: u32) -> Self }
```

The temporal nature of a loader that refreshes at a cadence (plan §4.2).

`frequency` is the cadence step (e.g. 1 hour for ERA5) — it drives the
refresh tstops and which record within a file is current. `file_period` is
the granularity of one file (e.g. 1 day) — it drives URL resolution. A file
holds `file_period / frequency` records along `time_dim`.

[Source](https://github.com/EarthSciML/EarthSciIO/blob/a49538bbd6ec318dde369d3496f6e8880586a73f/rust/src/provider.rs#L52) · [Rust docs](https://docs.rs/earthsciio/latest/earthsciio/struct.SourceTemporal.html)

</div>

</div>
