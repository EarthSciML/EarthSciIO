---
title: "integrity_error"
description: "Stable error · Errors"
type: "api"
api_tabs: true
---


**Tier:** stable · **Kind:** error · **Topic:** [Errors](/generated/api/topics/errors)

| Python | Julia | Rust |
|---|---|---|
| `IntegrityError` | `IntegrityError` | not exported |


<div class="api-tabs">

<div class="api-tab" data-lang="python" data-label="Python">
<p class="api-tab-label">Python</p>

```python
class IntegrityError(EarthSciIOError)
IntegrityError(message: str, *, key: str | None = None, expected: str | None = None, actual: str | None = None)
```

A cached or freshly downloaded blob failed its checksum / size check.

Raised when `sha256(blob)` disagrees with the expected/stored
`sha256_content` (or the on-disk length disagrees with `manifest.bytes`).
A self-pinned integrity failure means the blob is corrupt and must not be
served; the cache deletes it and re-fetches (online) or surfaces this error.

[Source](https://github.com/EarthSciML/EarthSciIO/blob/a49538bbd6ec318dde369d3496f6e8880586a73f/earthsciio/errors.py#L194)

</div>

<div class="api-tab" data-lang="julia" data-label="Julia">
<p class="api-tab-label">Julia</p>

```julia
struct IntegrityError <: Exception
IntegrityError(url::String, key::String, expected::String, got::String)
IntegrityError(url, key, expected, got)
```

Raised when a blob's bytes do not match its manifest `sha256_content`.

[Source](https://github.com/EarthSciML/EarthSciIO/blob/a49538bbd6ec318dde369d3496f6e8880586a73f/julia/src/cache.jl#L15)

</div>

<div class="api-tab" data-lang="rust" data-label="Rust">
<p class="api-tab-label">Rust</p>

<p class="api-absent">Rust does not export this error.</p>

</div>

</div>
