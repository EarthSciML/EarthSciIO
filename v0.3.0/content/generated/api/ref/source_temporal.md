---
title: "source_temporal"
description: "Stable type · Providers"
type: "api"
api_tabs: true
---


**Tier:** stable · **Kind:** type · **Topic:** [Providers](/generated/api/topics/providers)

| Python | Julia | Rust |
|---|---|---|
| `SourceTemporal` | `SourceTemporal` | `SourceTemporal` |


<div class="api-tabs">

<div class="api-tab" data-lang="python" data-label="Python">
<p class="api-tab-label">Python</p>

```python
class SourceTemporal
SourceTemporal(start: _dt.datetime, frequency: _dt.timedelta | CalendarPeriod | str, file_period: _dt.timedelta | CalendarPeriod | str, end: _dt.datetime | None = None, time_dim: str = 'time', records_per_sample: int | None = None)
SourceTemporal.record_anchor(t: _dt.datetime) -> _dt.datetime
SourceTemporal.next_record_anchor(t: _dt.datetime) -> _dt.datetime
SourceTemporal.first_record_anchor_from(t: _dt.datetime) -> _dt.datetime
SourceTemporal.file_anchor(t: _dt.datetime) -> _dt.datetime
SourceTemporal.next_file_anchor(t: _dt.datetime) -> _dt.datetime
SourceTemporal.record_in_file(anchor: _dt.datetime, file_anchor: _dt.datetime) -> int
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

**Members**

- `record_anchor` — The frequency anchor at or before `t`.
- `next_record_anchor` — The frequency anchor after the one at or before `t`.
- `first_record_anchor_from` — The first frequency anchor at or after `t`.
- `file_anchor` — The file-period anchor at or before `t` (the URL is expanded here).
- `next_file_anchor` — The file-period anchor after the one at or before `t`.
- `record_in_file` — The 0-based record of frequency anchor `anchor` in the file that starts at `file_anchor`: the frequency steps between them.

[Source](https://github.com/EarthSciML/EarthSciIO/blob/065acc28eb3328a3fa3a580d78e7222f1e92f6bf/earthsciio/provider.py#L79)

</div>

<div class="api-tab" data-lang="julia" data-label="Julia">
<p class="api-tab-label">Julia</p>

```julia
struct SourceTemporal
SourceTemporal(start::Dates.DateTime, frequency::CalendarPeriod, file_period::CalendarPeriod, stop::Union{Nothing, Dates.DateTime}, time_dim::String, records_per_sample::Union{Nothing, Int64})
SourceTemporal(start, frequency, file_period, stop, time_dim, records_per_sample)
SourceTemporal(start, frequency, file_period; stop, time_dim, records_per_sample)
```

```julia
SourceTemporal(start, frequency, file_period; stop=nothing, time_dim="time",
               records_per_sample=nothing)
```

The cadence of a time-varying source, as in the Python and Rust tracks: records
every `frequency`, files every `file_period`, both anchored at `start`
(spec/esm-sources.md §2). `start` and `stop` are UTC (`DateTime`, Unix seconds
or an ISO 8601 string); `stop` is the exclusive end of the data. The periods are
[`CalendarPeriod`](/generated/api/ref/calendar_period)s or anything one is built from (`"P1M"`, `Hour(1)`).
`records_per_sample` is 1 (default) or 2, as for [`Provider`](/generated/api/ref/provider).

[Source](https://github.com/EarthSciML/EarthSciIO/blob/065acc28eb3328a3fa3a580d78e7222f1e92f6bf/julia/src/calendar.jl#L156)

</div>

<div class="api-tab" data-lang="rust" data-label="Rust">
<p class="api-tab-label">Rust</p>

```rust
pub struct SourceTemporal {
    pub start: OffsetDateTime,
    pub end: Option<OffsetDateTime>,
    pub frequency: CalendarPeriod,
    pub file_period: CalendarPeriod,
    pub time_dim: String,
    pub records_per_sample: Option<u32>,
}

impl SourceTemporal { pub fn new(start: OffsetDateTime, frequency: impl Into<CalendarPeriod>, file_period: impl Into<CalendarPeriod>) -> Self }

impl SourceTemporal { pub fn end(self, end: OffsetDateTime) -> Self }

impl SourceTemporal { pub fn time_dim(self, dim: impl Into<String>) -> Self }

impl SourceTemporal { pub fn records_per_sample(self, n: u32) -> Self }

impl SourceTemporal { pub fn record_anchor(&self, t: OffsetDateTime) -> OffsetDateTime }

impl SourceTemporal { pub fn next_record_anchor(&self, t: OffsetDateTime) -> OffsetDateTime }

impl SourceTemporal { pub fn first_record_anchor_from(&self, t: OffsetDateTime) -> OffsetDateTime }

impl SourceTemporal { pub fn file_anchor(&self, t: OffsetDateTime) -> OffsetDateTime }

impl SourceTemporal { pub fn next_file_anchor(&self, t: OffsetDateTime) -> OffsetDateTime }

impl SourceTemporal { pub fn record_in_file(&self, anchor: OffsetDateTime, file_anchor: OffsetDateTime) -> i64 }
```

The temporal nature of a loader that refreshes at a cadence (plan §4.2).

`frequency` is the cadence step (e.g. 1 hour for ERA5) — it drives the
refresh tstops and which record within a file is current. `file_period` is
the granularity of one file (e.g. 1 day) — it drives URL resolution. Both
are [`CalendarPeriod`](/generated/api/ref/calendar_period)s anchored at `start`, so a monthly (`P1M`) or yearly
(`P1Y`) cadence lands on the calendar's months and years exactly
(`spec/esm-sources.md` §2); a record's index in its file is the number of
frequency anchors between the file's anchor and the record's.

[Source](https://github.com/EarthSciML/EarthSciIO/blob/065acc28eb3328a3fa3a580d78e7222f1e92f6bf/rust/src/provider.rs#L57) · [Rust docs](https://docs.rs/earthsciio/latest/earthsciio/struct.SourceTemporal.html)

</div>

</div>
