---
title: "cache"
description: "Stable type · Cache"
type: "api"
api_tabs: true
---


**Tier:** stable · **Kind:** type · **Topic:** [Cache](/generated/api/topics/cache)

| Python | Julia | Rust |
|---|---|---|
| `Cache` | `Cache` | `Cache` |


<div class="api-tabs">

<div class="api-tab" data-lang="python" data-label="Python">
<p class="api-tab-label">Python</p>

```python
class Cache
Cache(store=None, *, root: os.PathLike | None = None, store_name: str = 'local', offline: bool | None = None, auth=None, verify: bool = False, revalidate_file: bool | None = None)
Cache.fetch(resolved_url: str, *, source_loader: str | None = None, auth_realm: str | None = None, temporal: validate.Temporal | None = None, expected_checksum: str | None = None, mirrors: Sequence[str] = ()) -> CacheEntry
```

A content-addressed fetch cache over a pluggable [`Store`](/generated/api/extension/python#x-store).

**Parameters**

- `store` — An explicit store instance. When omitted, the `store_name` backend is
  built through the `store` registry (default `local`) rooted at
  `root` (else `$EARTHSCIDATADIR` / the `/scratch.local` default).
- `offline` (`bool | None`) — `True`/`False` forces offline/online; `None` (default) consults
  `$EARTHSCI_OFFLINE`. The explicit argument wins over the environment.
- `auth` — An `AuthRegistry`, a single resolver, an iterable
  of resolvers, or a `{realm: resolver}` dict; `None` means no auth.
- `verify` (`bool`) — Re-verify `sha256` + byte-length on every read (off by default, on for
  CI/conformance per `spec/cache-format.md` §4). This checks the **cached
  blob** against its own manifest — cache-internal consistency. It is NOT
  the `file://` source recheck; see `revalidate_file`.
- `revalidate_file` (`bool | None`) — Recheck a `file://` entry against the file it was ingested from before
  serving it (`spec/cache-format.md` §4.1). `None` (default) consults
  `$EARTHSCI_REVALIDATE_FILE`, which leaves it **on** unless explicitly
  falsey. Turning it off restores the behaviour of
  EarthSciML/EarthSciAST#293 — a local file replaced in place is served
  from the warm entry forever, silently and greenly — so do it only for a
  corpus known to be immutable, and only to save the one extra read. The
  `(size, mtime)` memo on
  `SourceRevalidator` already means that read
  is paid once per file per process, not once per call.

**Members**

- `fetch` — Return a [`CacheEntry`](/generated/api/ref/cache_entry) for `resolved_url`, fetching if needed.

[Source](https://github.com/EarthSciML/EarthSciIO/blob/a49538bbd6ec318dde369d3496f6e8880586a73f/earthsciio/cache.py#L84)

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
struct Cache
Cache(store::Store, offline::Bool, auth, verify::Bool, revalidate_file::Bool, revalidator::EarthSciIO.SourceRevalidator)
Cache(store, offline, auth, verify, revalidate_file, revalidator)
Cache(store::Store; offline, auth, verify, revalidate_file)
Cache(; store, root, kwargs...)
```

```julia
Cache(store::Store; offline=nothing, auth=nothing, verify=false, revalidate_file=nothing)
Cache(; store="local", root=datadir(), offline=nothing, auth=nothing, verify=false,
      revalidate_file=nothing)
```

The content-addressed cache. `offline=nothing` reads `EARTHSCI_OFFLINE` from the
environment; an explicit `offline` argument wins. `auth` is a resolver or a
realm→resolver map. `verify=true` re-checks `sha256_content` on every read (off
by default, on for CI/conformance) — that is the CACHED BLOB against its own
manifest, cache-internal consistency, NOT the `file://` source recheck.

`revalidate_file=nothing` reads `EARTHSCI_REVALIDATE_FILE` and leaves the
`file://` source recheck ON unless the knob is an explicit denial (spec §4.1).
Turning it off restores the behaviour of EarthSciML/EarthSciAST#293: a local file
replaced in place is served from the warm entry forever, silently and greenly. Do
it only for a corpus known to be immutable, and only to save the one extra
read.

[Source](https://github.com/EarthSciML/EarthSciIO/blob/a49538bbd6ec318dde369d3496f6e8880586a73f/julia/src/cache.jl#L309)

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
pub struct Cache {
    /* private fields */
}

impl Cache { pub fn builder() -> CacheBuilder }

impl Cache { pub fn from_env() -> Result<Cache> }

impl Cache { pub fn is_offline(&self) -> bool }

impl Cache { pub fn store(&self) -> &Arc<dyn Store> }

impl Cache { pub fn fetch(&self, req: &FetchRequest<'_>) -> Result<CachedBlob> }

impl Cache { pub fn get_offline(&self, resolved_url: &str) -> Result<CachedBlob> }
```

The cache fetcher. Cheap to clone the registries it holds; share one across
threads behind an `Arc`.

[Source](https://github.com/EarthSciML/EarthSciIO/blob/a49538bbd6ec318dde369d3496f6e8880586a73f/rust/src/cache.rs#L111) · [Rust docs](https://docs.rs/earthsciio/latest/earthsciio/struct.Cache.html)

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
