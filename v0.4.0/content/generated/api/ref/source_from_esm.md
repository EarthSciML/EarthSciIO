---
title: "source_from_esm"
description: "Stable function · Other"
type: "api"
api_tabs: true
---


**Tier:** stable · **Kind:** function · **Topic:** [Other](/generated/api/topics/other)

| Python | Julia | Rust |
|---|---|---|
| `source_from_esm` | not exported | `DataSource::from_esm` |


<div class="api-tabs">

<div class="api-tab" data-lang="python" data-label="Python">
<p class="api-tab-label">Python</p>

```python
source_from_esm(name: str, entry: Mapping[str, Any], *, variables: Sequence[str] = (), substitutions: Mapping[str, str] | None = None, url_override: str | None = None, records_per_sample: int | None = None)
```

The [`DataSource`](/generated/api/ref/data_source) an esm `data_sources` entry
declares (spec/esm-sources.md §3).

[Source](https://github.com/EarthSciML/EarthSciIO/blob/770dbc22d557e6e2ae87d9bccd7386f45764e9d0/earthsciio/esm_source.py#L160)

</div>

<div class="api-tab" data-lang="julia" data-label="Julia">
<p class="api-tab-label">Julia</p>

<p class="api-absent">Julia does not export this function.</p>

</div>

<div class="api-tab" data-lang="rust" data-label="Rust">
<p class="api-tab-label">Rust</p>

`DataSource::from_esm`

<p class="api-absent">Not found by the documentation extractor.</p>

</div>

</div>
