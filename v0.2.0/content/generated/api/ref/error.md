---
title: "error"
description: "Stable error · Errors"
type: "api"
api_tabs: true
---


**Tier:** stable · **Kind:** error · **Topic:** [Errors](/generated/api/topics/errors)

| Python | Julia | Rust |
|---|---|---|
| `EarthSciIOError` | not exported | `Error` |


<div class="api-tabs">

<div class="api-tab" data-lang="python" data-label="Python">
<p class="api-tab-label">Python</p>

```python
class EarthSciIOError(Exception)
```

Base class for every EarthSciIO error.

Every subclass pickles: the default exception pickle re-calls `cls(*args)`
with the formatted MESSAGE as the only argument, which breaks every
subclass whose `__init__` takes structured fields instead (`CacheMiss(url, key)` cannot be rebuilt from its message). A process-pool prefetch worker
(`earthsciio.prefetch`) must hand the consumer the error `refresh` would
have raised — a `CacheMiss`, not a wrapper — so the error is rebuilt from
its message and attributes directly.

[Source](https://github.com/EarthSciML/EarthSciIO/blob/a49538bbd6ec318dde369d3496f6e8880586a73f/earthsciio/errors.py#L54)

</div>

<div class="api-tab" data-lang="julia" data-label="Julia">
<p class="api-tab-label">Julia</p>

<p class="api-absent">Julia does not export this error.</p>

</div>

<div class="api-tab" data-lang="rust" data-label="Rust">
<p class="api-tab-label">Rust</p>

```rust
pub enum Error {
    CacheMiss { url: String, key: String },
    Integrity { key: String, detail: String },
    UnknownScheme { scheme: String, url: String },
    UnknownStore { name: String },
    BadUrl { url: String, detail: String },
    Transport { url: String, detail: String },
    NotFound { url: String, detail: String },
    AllMirrorsFailed { url: String, detail: String, not_found: bool },
    MissingAuth { realm: String },
    Io { path: Option<PathBuf>, source: Error },
    Manifest { detail: String },
    UnknownFormat { name: String },
    BadConfig { var: String, detail: String },
    Format { format: String, detail: String },
    RegionNotShardAligned { detail: String },
    Commit { detail: String },
    Prefetch { detail: String },
}

impl Error { pub fn io(path: Option<PathBuf>, source: Error) -> Self }

impl Error { pub fn is_cache_miss(&self) -> bool }

impl Error { pub fn is_not_found(&self) -> bool }
```

Errors raised by the cache, transports, and stores.

`CacheMiss` is load-bearing for the offline contract: it carries both the
resolved URL and its cache key so a failure names exactly which blob the
corpus/cache is missing (see `spec/offline-mode.md` §2).

[Source](https://github.com/EarthSciML/EarthSciIO/blob/a49538bbd6ec318dde369d3496f6e8880586a73f/rust/src/error.rs#L15) · [Rust docs](https://docs.rs/earthsciio/latest/earthsciio/enum.Error.html)

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
