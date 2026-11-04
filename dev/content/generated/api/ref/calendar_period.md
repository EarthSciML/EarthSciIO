---
title: "calendar_period"
description: "Stable type · Other"
type: "api"
api_tabs: true
---


**Tier:** stable · **Kind:** type · **Topic:** [Other](/generated/api/topics/other)

| Python | Julia | Rust |
|---|---|---|
| `CalendarPeriod` | `CalendarPeriod` | `CalendarPeriod` |


<div class="api-tabs">

<div class="api-tab" data-lang="python" data-label="Python">
<p class="api-tab-label">Python</p>

```python
class CalendarPeriod
CalendarPeriod(months: int = 0, seconds: int = 0)
@classmethod CalendarPeriod.parse(iso: str) -> CalendarPeriod
@classmethod CalendarPeriod.of(value: CalendarPeriod | _dt.timedelta | str | int) -> CalendarPeriod
CalendarPeriod.approximate_seconds() -> float
CalendarPeriod.anchor(start: _dt.datetime, k: int) -> _dt.datetime
CalendarPeriod.floor(start: _dt.datetime, t: _dt.datetime) -> int
CalendarPeriod.ceil(start: _dt.datetime, t: _dt.datetime) -> int
```

A cadence step of `months` calendar months plus `seconds` seconds.

**Members**

- `parse` — Read `P[nY][nM][nW][nD][T[nH][nM][nS]]` (spec §2.1).
- `of` — A period from a period, a fixed `timedelta`, an ISO string or whole seconds.
- `approximate_seconds` — A mean length, only for estimating an anchor index (never an anchor).
- `anchor` — `add_months(start, k*months) + k*seconds`.
- `floor` — The largest `k` with `anchor(start, k) <= t`.
- `ceil` — The smallest `k` with `anchor(start, k) >= t`.

[Source](https://github.com/EarthSciML/EarthSciIO/blob/065acc28eb3328a3fa3a580d78e7222f1e92f6bf/earthsciio/calendar.py#L38)

</div>

<div class="api-tab" data-lang="julia" data-label="Julia">
<p class="api-tab-label">Julia</p>

```julia
struct CalendarPeriod
CalendarPeriod(months::Integer, seconds::Integer)
CalendarPeriod(iso::AbstractString)
CalendarPeriod(p::Union{Dates.Month, Dates.Year})
CalendarPeriod(p::Union{Dates.Day, Dates.Hour, Dates.Minute, Dates.Second, Dates.Week})
CalendarPeriod(p::CalendarPeriod)
```

```julia
CalendarPeriod(months, seconds)
CalendarPeriod(iso::AbstractString)
CalendarPeriod(p::Dates.Period)
```

A cadence step of `months` calendar months plus `seconds` seconds (spec/esm-sources.md
§2.1). From an ISO 8601 string (`"P1M"`, `"PT3H"`), or from a `Dates` period
(`Month(1)`, `Hour(3)`).

[Source](https://github.com/EarthSciML/EarthSciIO/blob/065acc28eb3328a3fa3a580d78e7222f1e92f6bf/julia/src/calendar.jl#L19)

</div>

<div class="api-tab" data-lang="rust" data-label="Rust">
<p class="api-tab-label">Rust</p>

```rust
pub struct CalendarPeriod {
    pub months: i64,
    pub seconds: i64,
}

impl CalendarPeriod { pub fn months(months: i64) -> Self }

impl CalendarPeriod { pub fn seconds(seconds: i64) -> Self }

impl CalendarPeriod { pub fn parse(iso: &str) -> Result<Self> }

impl CalendarPeriod { pub fn is_positive(&self) -> bool }

impl CalendarPeriod { pub fn anchor(&self, start: OffsetDateTime, k: i64) -> OffsetDateTime }

impl CalendarPeriod { pub fn floor_index(&self, start: OffsetDateTime, t: OffsetDateTime) -> i64 }

impl CalendarPeriod { pub fn ceil_index(&self, start: OffsetDateTime, t: OffsetDateTime) -> i64 }

impl CalendarPeriod { pub fn floor(&self, start: OffsetDateTime, t: OffsetDateTime) -> OffsetDateTime }
```

A cadence step of whole calendar `months` plus whole `seconds`
(`spec/esm-sources.md` §2.1): `P1M` is one month, `P1Y` twelve, `PT3H`
10800 seconds, `P1D` 86400 seconds (a UTC day).

[Source](https://github.com/EarthSciML/EarthSciIO/blob/065acc28eb3328a3fa3a580d78e7222f1e92f6bf/rust/src/calendar.rs#L22) · [Rust docs](https://docs.rs/earthsciio/latest/earthsciio/calendar/struct.CalendarPeriod.html)

</div>

</div>
