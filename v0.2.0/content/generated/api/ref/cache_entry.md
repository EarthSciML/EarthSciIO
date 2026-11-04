---
title: "cache_entry"
description: "Stable type · Cache"
type: "api"
api_tabs: true
---


**Tier:** stable · **Kind:** type · **Topic:** [Cache](/generated/api/topics/cache)

| Python | Julia | Rust |
|---|---|---|
| `CacheEntry` | `CacheEntry` | `CachedBlob` |


<div class="api-tabs">

<div class="api-tab" data-lang="python" data-label="Python">
<p class="api-tab-label">Python</p>

```python
class CacheEntry
CacheEntry(key: str, path: pathlib.Path, manifest: Manifest | None, status: str)
```

The result of a fetch: where the blob is + how it got there.

`status` is `"hit"` (served from cache, no network), `"downloaded"` (a
fresh GET), or `"not_modified"` (a conditional GET returned 304 and the
cached blob was reused). `manifest` may be `None` only for an offline hit
against a blob whose sidecar manifest is absent.

[Source](https://github.com/EarthSciML/EarthSciIO/blob/a49538bbd6ec318dde369d3496f6e8880586a73f/earthsciio/cache.py#L68)

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
struct CacheEntry
CacheEntry(key::String, path::String, manifest::Union{Nothing, Manifest}, status::Symbol)
CacheEntry(key, path, manifest, status)
```

The outcome of [`fetch_blob`]: the resolved blob plus how it was obtained.

[Source](https://github.com/EarthSciML/EarthSciIO/blob/a49538bbd6ec318dde369d3496f6e8880586a73f/julia/src/cache.jl#L357)

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
pub struct CachedBlob {
    pub key: String,
    pub path: PathBuf,
    pub manifest: Manifest,
}
```

A resolved cache entry: the blob path plus its manifest.

[Source](https://github.com/EarthSciML/EarthSciIO/blob/a49538bbd6ec318dde369d3496f6e8880586a73f/rust/src/cache.rs#L46) · [Rust docs](https://docs.rs/earthsciio/latest/earthsciio/struct.CachedBlob.html)

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
