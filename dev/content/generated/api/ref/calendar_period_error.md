---
title: "calendar_period_error"
description: "Stable error · Other"
type: "api"
api_tabs: true
---


**Tier:** stable · **Kind:** error · **Topic:** [Other](/generated/api/topics/other)

| Python | Julia | Rust |
|---|---|---|
| `CalendarPeriodError` | `CalendarPeriodError` | not exported |


<div class="api-tabs">

<div class="api-tab" data-lang="python" data-label="Python">
<p class="api-tab-label">Python</p>

```python
class CalendarPeriodError(EarthSciIOError, ValueError)
```

An ISO 8601 period EarthSciIO cannot read; the message contains
`esm_period_invalid`.

[Source](https://github.com/EarthSciML/EarthSciIO/blob/065acc28eb3328a3fa3a580d78e7222f1e92f6bf/earthsciio/calendar.py#L33)

</div>

<div class="api-tab" data-lang="julia" data-label="Julia">
<p class="api-tab-label">Julia</p>

```julia
struct CalendarPeriodError <: Exception
CalendarPeriodError(msg::String)
CalendarPeriodError(msg)
```

```julia
CalendarPeriodError(msg)
```

An ISO 8601 period EarthSciIO cannot read; the message contains
`esm_period_invalid`.

[Source](https://github.com/EarthSciML/EarthSciIO/blob/065acc28eb3328a3fa3a580d78e7222f1e92f6bf/julia/src/calendar.jl#L9)

</div>

<div class="api-tab" data-lang="rust" data-label="Rust">
<p class="api-tab-label">Rust</p>

<p class="api-absent">Rust does not export this error.</p>

</div>

</div>
