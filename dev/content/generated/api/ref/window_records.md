---
title: "window_records"
description: "Stable function · Other"
type: "api"
api_tabs: true
---


**Tier:** stable · **Kind:** function · **Topic:** [Other](/generated/api/topics/other)

| Python | Julia | Rust |
|---|---|---|
| not exported | `window_records` | not exported |


<div class="api-tabs">

<div class="api-tab" data-lang="python" data-label="Python">
<p class="api-tab-label">Python</p>

<p class="api-absent">Python does not export this function.</p>

</div>

<div class="api-tab" data-lang="julia" data-label="Julia">
<p class="api-tab-label">Julia</p>

```julia
window_records(p::Provider)
```

```julia
window_records(p::Provider) -> Vector{Float64}
```

The anchors, in seconds from the provider's `time_origin` when it has one
(Unix seconds otherwise), of the records that serve a `temporal` provider's run
window, in order (spec/esm-sources.md §2.6): the record in effect at the window
start, every record whose anchor is inside the window, the record at the window
end itself when there is one ([`refresh_times_closed`](/generated/api/ref/refresh_times_closed) adds it) and, with
`records_per_sample = 2`, the record after the last of these that its bracket
reads (unless that is at or past the source's `stop`).

Unlike [`refresh_times`](/generated/api/ref/refresh_times) it is never empty for a `temporal` provider: a
window inside one record's interval has no refresh times and is served by that
record. Empty for a provider not built from a [`SourceTemporal`](/generated/api/ref/source_temporal).

[Source](https://github.com/EarthSciML/EarthSciIO/blob/770dbc22d557e6e2ae87d9bccd7386f45764e9d0/julia/src/provider.jl#L543)

</div>

<div class="api-tab" data-lang="rust" data-label="Rust">
<p class="api-tab-label">Rust</p>

<p class="api-absent">Rust does not export this function.</p>

</div>

</div>
