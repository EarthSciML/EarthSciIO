---
title: "provider_from_esm"
description: "Stable function · Other"
type: "api"
api_tabs: true
---


**Tier:** stable · **Kind:** function · **Topic:** [Other](/generated/api/topics/other)

| Python | Julia | Rust |
|---|---|---|
| `provider_from_esm` | `provider_from_esm` | not exported |


<div class="api-tabs">

<div class="api-tab" data-lang="python" data-label="Python">
<p class="api-tab-label">Python</p>

```python
provider_from_esm(name: str, entry: Mapping[str, Any], cache, *, variables: Sequence[str] = (), substitutions: Mapping[str, str] | None = None, url_override: str | None = None, records_per_sample: int | None = None, window=None, time_origin=None)
```

A [`Provider`](/generated/api/ref/provider) over an esm `data_sources` entry,
bound to `cache` and the run `window` (UTC datetimes, or seconds from
`time_origin`).

`time_origin` (a datetime or ISO 8601 string, a whole second) is the
instant the provider counts time from, usually the model's time origin
(EarthSciAST's `time_origin(document)`); with it, every time in and out
is seconds from it (spec/esm-sources.md §4). An entry does not carry the
model's origin, so there is no default: without one, times are as before.

[Source](https://github.com/EarthSciML/EarthSciIO/blob/770dbc22d557e6e2ae87d9bccd7386f45764e9d0/earthsciio/esm_source.py#L227)

</div>

<div class="api-tab" data-lang="julia" data-label="Julia">
<p class="api-tab-label">Julia</p>

```julia
provider_from_esm(name::AbstractString, entry, cache::Cache; variables, substitutions, url_override, records_per_sample, window, time_origin)
```

```julia
provider_from_esm(name, entry, cache; variables, substitutions=Dict(),
                  url_override=nothing, records_per_sample=nothing,
                  window=nothing, time_origin=nothing) -> Provider
```

A [`Provider`](/generated/api/ref/provider) over one esm `data_sources` entry (a parsed JSON object, as
a document carries it after EarthSciAST loaded it), bound to `cache` and the run
`window` (UTC: `DateTime`s, or seconds from `time_origin`). See
spec/esm-sources.md §3. A time-varying source's ticks are Unix seconds, or,
with a `time_origin` (a `DateTime` or ISO 8601 string, a whole second, usually
the model's time origin from EarthSciAST's `time_origin(document)`), seconds
from it, in and out (§4). An entry does not carry the model's origin, so there
is no default.

[Source](https://github.com/EarthSciML/EarthSciIO/blob/770dbc22d557e6e2ae87d9bccd7386f45764e9d0/julia/src/esm_source.jl#L143)

</div>

<div class="api-tab" data-lang="rust" data-label="Rust">
<p class="api-tab-label">Rust</p>

<p class="api-absent">Rust does not export this function.</p>

</div>

</div>
