---
title: "cache_key"
description: "Stable function · Cache"
type: "api"
api_tabs: true
---


**Tier:** stable · **Kind:** function · **Topic:** [Cache](/generated/api/topics/cache)

| Python | Julia | Rust |
|---|---|---|
| `cache_key` | `cache_key` | `cache_key` |


<div class="api-tabs">

<div class="api-tab" data-lang="python" data-label="Python">
<p class="api-tab-label">Python</p>

```python
cache_key(resolved_url: str, byte_range: tuple[int, int] | None = None) -> str
```

Return the lowercase-hex sha256 of the resolved URL.

`byte_range=(a, b)` appends `#bytes=a-b` before hashing so a byte-slice
addresses its own blob, exactly as the spec requires. The base URL is hashed
verbatim; callers pass the URL already resolved (time anchor + parameters
expanded). URL resolution itself is pure and lives in the cores — this layer
starts from the resolved URL.

[Source](https://github.com/EarthSciML/EarthSciIO/blob/a49538bbd6ec318dde369d3496f6e8880586a73f/earthsciio/cachekey.py#L21)

**Example** (`fetch`)

```python
cache = Cache(root=cache_root, offline=False)
entry = cache.fetch(URL, source_loader="stations")
assert entry.status == "downloaded"
assert entry.path.read_bytes() == (FIXTURES / "stations.csv").read_bytes()
# The blob is addressed by the SHA-256 of the resolved URL.
assert entry.key == earthsciio.cache_key(URL)

# Asking again is a cache hit: no transfer happens.
again = cache.fetch(URL, source_loader="stations")
assert again.status == "hit" and again.path == entry.path
```

<p class="api-src">From <a href="https://github.com/EarthSciML/EarthSciIO/blob/a49538bbd6ec318dde369d3496f6e8880586a73f/docs/api/examples/python/fetch.py#L15-L24">docs/api/examples/python/fetch.py</a></p>

</div>

<div class="api-tab" data-lang="julia" data-label="Julia">
<p class="api-tab-label">Julia</p>

```julia
cache_key(resolved_url::AbstractString)
cache_key(resolved_url::AbstractString, byterange::Tuple{Integer, Integer})
```

```julia
cache_key(resolved_url) -> String
```

`lowercase_hex(sha256(utf8(resolved_url)))`. The URL is hashed exactly as
resolved — UTF-8, no trailing newline, no normalization — so all three language
tracks hash the identical byte string and share the cache.

---

Sub-range request: `#bytes=<a>-<b>` is appended before hashing, so a
byte-slice is its own cache entry.

[Source](https://github.com/EarthSciML/EarthSciIO/blob/a49538bbd6ec318dde369d3496f6e8880586a73f/julia/src/cache.jl#L65)

**Example** (`fetch`)

```julia
cache = Cache(; root = CACHE_ROOT, offline = false)
entry = fetch_blob(cache, URL; source_loader = "stations")
@assert entry isa CacheEntry
@assert entry.status == :downloaded
@assert read(entry.path) == read(joinpath(FIXTURES, "stations.csv"))
# The blob is addressed by the SHA-256 of the resolved URL.
@assert entry.key == cache_key(URL)

# Asking again is a cache hit: no transfer happens.
again = fetch_blob(cache, URL; source_loader = "stations")
@assert again.status == :hit && again.path == entry.path
```

<p class="api-src">From <a href="https://github.com/EarthSciML/EarthSciIO/blob/a49538bbd6ec318dde369d3496f6e8880586a73f/docs/api/examples/julia/fetch.jl#L9-L19">docs/api/examples/julia/fetch.jl</a></p>

</div>

<div class="api-tab" data-lang="rust" data-label="Rust">
<p class="api-tab-label">Rust</p>

```rust
pub fn cache_key(resolved_url: &str) -> String
```

Compute the cache key for a resolved URL: `sha256(utf8(url))`, lowercase hex.

```rust
assert_eq!(
    earthsciio::cache_key("https://data.earthsci.dev/era5/2018/11/20181108.nc"),
    "11cdcec111409f586e6afc432e1a6da47e6f97ccf3715e5db8554632b00671c1",
);
```

[Source](https://github.com/EarthSciML/EarthSciIO/blob/a49538bbd6ec318dde369d3496f6e8880586a73f/rust/src/key.rs#L20) · [Rust docs](https://docs.rs/earthsciio/latest/earthsciio/fn.cache_key.html)

**Example** (`fetch`)

```rust
let cache = Cache::builder()
    .data_dir(cache_root.path())
    .offline(false)
    .build()?;
let blob = cache.fetch(&FetchRequest::new(&url).loader("stations"))?;
assert_eq!(std::fs::read(&blob.path)?, std::fs::read(&source)?);
// The blob is addressed by the SHA-256 of the resolved URL.
assert_eq!(blob.key, cache_key(&url));

// Asking again returns the same blob without a transfer.
let again = cache.fetch(&FetchRequest::new(&url))?;
assert_eq!(again.path, blob.path);
```

<p class="api-src">From <a href="https://github.com/EarthSciML/EarthSciIO/blob/a49538bbd6ec318dde369d3496f6e8880586a73f/docs/api/examples/rust/fetch.rs#L12-L23">docs/api/examples/rust/fetch.rs</a></p>

</div>

</div>
