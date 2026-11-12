---
title: "time_origin_invalid"
description: "Stable error · Other"
type: "api"
api_tabs: true
---


**Tier:** stable · **Kind:** error · **Topic:** [Other](/generated/api/topics/other)

| Python | Julia | Rust |
|---|---|---|
| `TimeOriginInvalid` | `TimeOriginInvalid` | not exported |


<div class="api-tabs">

<div class="api-tab" data-lang="python" data-label="Python">
<p class="api-tab-label">Python</p>

```python
class TimeOriginInvalid(EarthSciIOError, ValueError)
```

A provider's `time_origin` that is not a whole-second UTC instant
(spec/esm-sources.md §4). The message starts with the code
`time_origin_invalid`, the same in every track.

[Source](https://github.com/EarthSciML/EarthSciIO/blob/770dbc22d557e6e2ae87d9bccd7386f45764e9d0/earthsciio/errors.py#L318)

</div>

<div class="api-tab" data-lang="julia" data-label="Julia">
<p class="api-tab-label">Julia</p>

```julia
struct TimeOriginInvalid <: Exception
TimeOriginInvalid(msg::String)
TimeOriginInvalid(msg)
```

```julia
TimeOriginInvalid(msg)
```

A provider's `time_origin` that is not a whole-second UTC instant
(spec/esm-sources.md §4). The message starts with the code
`time_origin_invalid`, the same in every track.

[Source](https://github.com/EarthSciML/EarthSciIO/blob/770dbc22d557e6e2ae87d9bccd7386f45764e9d0/julia/src/provider.jl#L157)

</div>

<div class="api-tab" data-lang="rust" data-label="Rust">
<p class="api-tab-label">Rust</p>

<p class="api-absent">Rust does not export this error.</p>

</div>

</div>
