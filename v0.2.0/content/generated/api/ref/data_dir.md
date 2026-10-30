---
title: "data_dir"
description: "Stable function · Cache"
type: "api"
api_tabs: true
---


**Tier:** stable · **Kind:** function · **Topic:** [Cache](/generated/api/topics/cache)

| Python | Julia | Rust |
|---|---|---|
| `resolve_cache_root` | `datadir` | `data_dir` |


<div class="api-tabs">

<div class="api-tab" data-lang="python" data-label="Python">
<p class="api-tab-label">Python</p>

```python
resolve_cache_root(explicit: os.PathLike | None = None) -> pathlib.Path
```

Resolve the cache root directory.

Precedence: an explicit argument, then `$EARTHSCIDATADIR`, then the
`/scratch.local` default. The environment variable **always wins** over the
default — only an explicit in-process argument overrides it (used by tests
and by callers that point at, e.g., the conformance corpus).

[Source](https://github.com/EarthSciML/EarthSciIO/blob/a49538bbd6ec318dde369d3496f6e8880586a73f/earthsciio/config.py#L56)

**Example** (`data_dir`)

```python
root = earthsciio.resolve_cache_root(cache_root)
assert Path(root) == cache_root
```

<p class="api-src">From <a href="https://github.com/EarthSciML/EarthSciIO/blob/a49538bbd6ec318dde369d3496f6e8880586a73f/docs/api/examples/python/fetch.py#L41-L42">docs/api/examples/python/fetch.py</a></p>

</div>

<div class="api-tab" data-lang="julia" data-label="Julia">
<p class="api-tab-label">Julia</p>

```julia
datadir()
```

```julia
datadir() -> String
```

Resolve `$EARTHSCIDATADIR` (spec/cache-format.md §5). The env var always wins;
the fallback default lives on `/scratch.local`, NEVER `/u` (the home inode quota
cannot absorb many small NetCDF slices — a hard rule).

[Source](https://github.com/EarthSciML/EarthSciIO/blob/a49538bbd6ec318dde369d3496f6e8880586a73f/julia/src/cache.jl#L30)

**Example** (`data_dir`)

```julia
@assert datadir() isa String
```

<p class="api-src">From <a href="https://github.com/EarthSciML/EarthSciIO/blob/a49538bbd6ec318dde369d3496f6e8880586a73f/docs/api/examples/julia/fetch.jl#L37">docs/api/examples/julia/fetch.jl</a></p>

</div>

<div class="api-tab" data-lang="rust" data-label="Rust">
<p class="api-tab-label">Rust</p>

```rust
pub fn data_dir() -> PathBuf
```

Resolve the cache root `$EARTHSCIDATADIR`.

`EARTHSCIDATADIR` wins when set and non-empty; otherwise
`/scratch.local/$USER/earthsci-cache`.

[Source](https://github.com/EarthSciML/EarthSciIO/blob/a49538bbd6ec318dde369d3496f6e8880586a73f/rust/src/datadir.rs#L17) · [Rust docs](https://docs.rs/earthsciio/latest/earthsciio/datadir/fn.data_dir.html)

**Example** (`data_dir`)

```rust
assert!(!earthsciio::data_dir().as_os_str().is_empty());
```

<p class="api-src">From <a href="https://github.com/EarthSciML/EarthSciIO/blob/a49538bbd6ec318dde369d3496f6e8880586a73f/docs/api/examples/rust/fetch.rs#L42">docs/api/examples/rust/fetch.rs</a></p>

</div>

</div>
