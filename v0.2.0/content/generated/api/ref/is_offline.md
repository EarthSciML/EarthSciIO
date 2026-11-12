---
title: "is_offline"
description: "Stable function · Cache"
type: "api"
api_tabs: true
---


**Tier:** stable · **Kind:** function · **Topic:** [Cache](/generated/api/topics/cache)

| Python | Julia | Rust |
|---|---|---|
| `resolve_offline` | `is_offline` | `is_offline` |


<div class="api-tabs">

<div class="api-tab" data-lang="python" data-label="Python">
<p class="api-tab-label">Python</p>

```python
resolve_offline(explicit: bool | None = None) -> bool
```

Resolve effective offline mode.

`explicit` of `True`/`False` wins; `None` means "consult the
environment" (`EARTHSCI_OFFLINE`). This sentinel is how the spec's "the
explicit argument wins over the environment when both are present" is honored
without conflating an unset argument with an explicit `False`.

[Source](https://github.com/EarthSciML/EarthSciIO/blob/a49538bbd6ec318dde369d3496f6e8880586a73f/earthsciio/config.py#L87)

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
is_offline(c::Cache)
```

True if this cache is in offline (cache-only) mode.

[Source](https://github.com/EarthSciML/EarthSciIO/blob/a49538bbd6ec318dde369d3496f6e8880586a73f/julia/src/cache.jl#L354)

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

```rust
pub fn is_offline(explicit: Option<bool>) -> bool
```

Resolve the effective offline flag: the explicit argument wins; otherwise
fall back to `EARTHSCI_OFFLINE`.

[Source](https://github.com/EarthSciML/EarthSciIO/blob/a49538bbd6ec318dde369d3496f6e8880586a73f/rust/src/offline.rs#L12) · [Rust docs](https://docs.rs/earthsciio/latest/earthsciio/fn.is_offline.html)

**Example** (`offline`)

```rust
let offline = Cache::builder()
    .data_dir(cache_root.path())
    .offline(true)
    .build()?;
assert!(offline.is_offline());
assert_eq!(offline.fetch(&FetchRequest::new(&url))?.path, blob.path); // served from disk only

// An offline miss is an error, never a network request.
let miss = offline
    .fetch(&FetchRequest::new("file:///not/cached.csv"))
    .unwrap_err();
assert!(miss.is_cache_miss());
```

<p class="api-src">From <a href="https://github.com/EarthSciML/EarthSciIO/blob/a49538bbd6ec318dde369d3496f6e8880586a73f/docs/api/examples/rust/fetch.rs#L27-L38">docs/api/examples/rust/fetch.rs</a></p>

</div>

</div>
