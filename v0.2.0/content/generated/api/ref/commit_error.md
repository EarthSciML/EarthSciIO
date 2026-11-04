---
title: "commit_error"
description: "Stable type · Errors"
type: "api"
api_tabs: true
---


**Tier:** stable · **Kind:** type · **Topic:** [Errors](/generated/api/topics/errors)

| Python | Julia | Rust |
|---|---|---|
| `CommitError` | `CommitError` | not exported |


<div class="api-tabs">

<div class="api-tab" data-lang="python" data-label="Python">
<p class="api-tab-label">Python</p>

```python
class CommitError(EarthSciIOError)
```

An owner's `commit` refused the delivered tokens (`spec/writer.md`
§5.3): a missing or duplicate writer, a divergent fingerprint, a gap or an
overlap between regions, divergent records / statics, or data that is
already committed. Nothing was written; the committed state is unchanged.

[Source](https://github.com/EarthSciML/EarthSciIO/blob/a49538bbd6ec318dde369d3496f6e8880586a73f/earthsciio/errors.py#L315)

</div>

<div class="api-tab" data-lang="julia" data-label="Julia">
<p class="api-tab-label">Julia</p>

```julia
struct CommitError <: Exception
CommitError(msg::String)
CommitError(msg)
```

```julia
CommitError(msg)
```

The tokens handed to a commit do not describe a consistent step (spec/writer.md
§5.3): a missing or duplicate writer, a divergent fingerprint, a gap or overlap
between regions, divergent records, or a rewrite of a committed record/static.
Nothing is written; the committed state is unchanged.

[Source](https://github.com/EarthSciML/EarthSciIO/blob/a49538bbd6ec318dde369d3496f6e8880586a73f/julia/src/zarr_stream.jl#L33)

</div>

<div class="api-tab" data-lang="rust" data-label="Rust">
<p class="api-tab-label">Rust</p>

<p class="api-absent">Rust does not export this type.</p>

</div>

</div>
