---
title: "cache_miss"
description: "Stable error · Errors"
type: "api"
api_tabs: true
---


**Tier:** stable · **Kind:** error · **Topic:** [Errors](/generated/api/topics/errors)

| Python | Julia | Rust |
|---|---|---|
| `CacheMiss` | `CacheMiss` | not exported |


<div class="api-tabs">

<div class="api-tab" data-lang="python" data-label="Python">
<p class="api-tab-label">Python</p>

```python
class CacheMiss(EarthSciIOError)
CacheMiss(resolved_url: str, key: str)
```

Raised in **offline mode** when the blob for a resolved URL is absent.

Per `spec/offline-mode.md` §2 this is *never* a silent empty result and
*never* a fallback fetch — offline is cache-only. The error carries the
`resolved_url` and the derived `key` (`sha256(resolved_url)`) so the
caller can point at the exact blob the corpus/cache is missing. Carrying both
is a cross-language requirement (the Julia/Rust `CacheMiss` do the same).

[Source](https://github.com/EarthSciML/EarthSciIO/blob/a49538bbd6ec318dde369d3496f6e8880586a73f/earthsciio/errors.py#L178)

**Example** (`offline`)

```python
offline = Cache(root=cache_root, offline=True)
assert offline.fetch(URL).path == entry.path  # served from disk only

try:
    offline.fetch((FIXTURES / "not-cached.csv").as_uri())
    raise AssertionError("an offline miss must raise")
except CacheMiss:
    pass  # offline mode never touches the network
```

<p class="api-src">From <a href="https://github.com/EarthSciML/EarthSciIO/blob/a49538bbd6ec318dde369d3496f6e8880586a73f/docs/api/examples/python/fetch.py#L29-L36">docs/api/examples/python/fetch.py</a></p>

</div>

<div class="api-tab" data-lang="julia" data-label="Julia">
<p class="api-tab-label">Julia</p>

```julia
struct CacheMiss <: Exception
CacheMiss(url::String, key::String)
CacheMiss(url, key)
```

Raised in offline mode when the blob for a resolved URL is absent. Carries
both the `url` and its `key` so a failure names exactly which blob is missing.

[Source](https://github.com/EarthSciML/EarthSciIO/blob/a49538bbd6ec318dde369d3496f6e8880586a73f/julia/src/cache.jl#L6)

**Example** (`offline`)

```julia
offline = Cache(; root = CACHE_ROOT, offline = true)
@assert is_offline(offline)
@assert fetch_blob(offline, URL).path == entry.path  # served from disk only

missed = try
    fetch_blob(offline, "file://" * abspath(joinpath(FIXTURES, "not-cached.csv")))
    false
catch err
    err isa CacheMiss  # offline mode never touches the network
end
@assert missed
```

<p class="api-src">From <a href="https://github.com/EarthSciML/EarthSciIO/blob/a49538bbd6ec318dde369d3496f6e8880586a73f/docs/api/examples/julia/fetch.jl#L23-L33">docs/api/examples/julia/fetch.jl</a></p>

</div>

<div class="api-tab" data-lang="rust" data-label="Rust">
<p class="api-tab-label">Rust</p>

<p class="api-absent">Rust does not export this error.</p>

</div>

</div>
