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
provider_from_esm(name: str, entry: Mapping[str, Any], cache, *, variables: Sequence[str] = (), substitutions: Mapping[str, str] | None = None, url_override: str | None = None, records_per_sample: int | None = None, window=None)
```

A [`Provider`](/generated/api/ref/provider) over an esm `data_sources` entry,
bound to `cache` and the run `window` (UTC datetimes).

[Source](https://github.com/EarthSciML/EarthSciIO/blob/065acc28eb3328a3fa3a580d78e7222f1e92f6bf/earthsciio/esm_source.py#L227)

</div>

<div class="api-tab" data-lang="julia" data-label="Julia">
<p class="api-tab-label">Julia</p>

```julia
provider_from_esm(name::AbstractString, entry, cache::Cache; variables, substitutions, url_override, records_per_sample, window)
```

```julia
provider_from_esm(name, entry, cache; variables, substitutions=Dict(),
                  url_override=nothing, records_per_sample=nothing,
                  window=nothing) -> Provider
```

A [`Provider`](/generated/api/ref/provider) over one esm `data_sources` entry (a parsed JSON object, as
a document carries it after EarthSciAST loaded it), bound to `cache` and the run
`window` (UTC: `DateTime`s or Unix seconds). See spec/esm-sources.md §3. A
time-varying source's ticks are Unix seconds.

[Source](https://github.com/EarthSciML/EarthSciIO/blob/065acc28eb3328a3fa3a580d78e7222f1e92f6bf/julia/src/esm_source.jl#L143)

</div>

<div class="api-tab" data-lang="rust" data-label="Rust">
<p class="api-tab-label">Rust</p>

<p class="api-absent">Rust does not export this function.</p>

</div>

</div>
