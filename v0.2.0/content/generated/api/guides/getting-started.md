---
title: "Getting started"
description: "Fetch a file into the shared cache, read it back offline, and decode it into native-grid arrays."
weight: 10
api_tabs: true
---

EarthSciIO resolves a URL, downloads it once into a **content-addressed cache**
shared by the Python, Julia and Rust implementations, and decodes the cached file
into **native-grid arrays**. This guide follows one small file through both
steps. Each snippet is cut from a program that runs, and checks its own result,
in CI.

The examples read `stations.csv`, a four-row table of air-quality readings from
two stations, through a `file://` URL so they need no network.

## Fetch into the cache

The cache key is the SHA-256 of the resolved URL, so the same URL always lands
on the same blob, whichever language fetched it. The first fetch downloads; the
second is a hit.


<div class="api-tabs">

<div class="api-tab" data-lang="python" data-label="Python">
<p class="api-tab-label">Python</p>

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


## Work offline

Offline mode serves only what is already cached. A request that is not cached
fails with a cache miss rather than opening a connection, which makes test runs
hermetic.


<div class="api-tabs">

<div class="api-tab" data-lang="python" data-label="Python">
<p class="api-tab-label">Python</p>

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


## Choose the cache location

The cache root defaults to `$EARTHSCIDATADIR`; the constructors take an explicit
root.


<div class="api-tabs">

<div class="api-tab" data-lang="python" data-label="Python">
<p class="api-tab-label">Python</p>

```python
root = earthsciio.resolve_cache_root(cache_root)
assert Path(root) == cache_root
```

<p class="api-src">From <a href="https://github.com/EarthSciML/EarthSciIO/blob/a49538bbd6ec318dde369d3496f6e8880586a73f/docs/api/examples/python/fetch.py#L41-L42">docs/api/examples/python/fetch.py</a></p>

</div>

<div class="api-tab" data-lang="julia" data-label="Julia">
<p class="api-tab-label">Julia</p>

```julia
@assert datadir() isa String
```

<p class="api-src">From <a href="https://github.com/EarthSciML/EarthSciIO/blob/a49538bbd6ec318dde369d3496f6e8880586a73f/docs/api/examples/julia/fetch.jl#L37">docs/api/examples/julia/fetch.jl</a></p>

</div>

<div class="api-tab" data-lang="rust" data-label="Rust">
<p class="api-tab-label">Rust</p>

```rust
assert!(!earthsciio::data_dir().as_os_str().is_empty());
```

<p class="api-src">From <a href="https://github.com/EarthSciML/EarthSciIO/blob/a49538bbd6ec318dde369d3496f6e8880586a73f/docs/api/examples/rust/fetch.rs#L42">docs/api/examples/rust/fetch.rs</a></p>

</div>

</div>


## Read native-grid arrays

A provider ties a data source to the cache and to a format reader, and returns
the file's variables as raw native-grid arrays. Variable renaming, unit
conversion and regridding are left to the rest of the stack.


<div class="api-tabs">

<div class="api-tab" data-lang="python" data-label="Python">
<p class="api-tab-label">Python</p>

```python
source = DataSource(
    "stations",
    "csv",  # the format-registry key that selects the reader
    (FIXTURES / "stations.csv").as_uri(),
    variables=["value", "location_id"],
    reader_kwargs={"numeric_columns": ["latitude", "longitude", "value"]},
)
data = Provider(source, cache).materialize()
assert isinstance(data, NativeDataset)
assert data.variable_names() == ["location_id", "value"]
assert data["value"].data.dtype == np.float64
assert list(data["value"].data) == [152.3, 168.7, 98.1, 110.4]
# Digit-only text columns stay strings: nothing is inferred.
assert data["location_id"].data == ["1", "1", "2", "2"]
```

<p class="api-src">From <a href="https://github.com/EarthSciML/EarthSciIO/blob/a49538bbd6ec318dde369d3496f6e8880586a73f/docs/api/examples/python/provider.py#L15-L28">docs/api/examples/python/provider.py</a></p>

</div>

<div class="api-tab" data-lang="julia" data-label="Julia">
<p class="api-tab-label">Julia</p>

```julia
url = "file://" * abspath(joinpath(FIXTURES, "stations.csv"))
p = const_provider(
    cache, url; format = "csv", source_loader = "stations",
    variables = ["value", "location_id"],
    reader_kwargs = (numeric_columns = ["latitude", "longitude", "value"],)
)
@assert is_const(p) && refresh_times(p) == Float64[]  # a CONST provider never refreshes
data = materialize(p)
@assert data isa NativeDataset
@assert Set(variable_names(data)) == Set(["location_id", "value"])
@assert data["value"].data == [152.3, 168.7, 98.1, 110.4]
# Digit-only text columns stay strings: nothing is inferred.
@assert data["location_id"].data == ["1", "1", "2", "2"]
```

<p class="api-src">From <a href="https://github.com/EarthSciML/EarthSciIO/blob/a49538bbd6ec318dde369d3496f6e8880586a73f/docs/api/examples/julia/provider.jl#L8-L20">docs/api/examples/julia/provider.jl</a></p>

</div>

</div>


The full operation list is in the [API reference](/generated/api/), and the
contract all three implementations follow is in the
[provider spec](/generated/spec/).
