---
title: "sha256_file"
description: "Stable function · Cache"
type: "api"
api_tabs: true
---


**Tier:** stable · **Kind:** function · **Topic:** [Cache](/generated/api/topics/cache)

| Python | Julia | Rust |
|---|---|---|
| `sha256_file` | not exported | `sha256_file` |


<div class="api-tabs">

<div class="api-tab" data-lang="python" data-label="Python">
<p class="api-tab-label">Python</p>

```python
sha256_file(path, chunk_size: int = 1 << 20) -> str
```

sha256 of a file's contents, streamed so large blobs don't load to RAM.

[Source](https://github.com/EarthSciML/EarthSciIO/blob/a49538bbd6ec318dde369d3496f6e8880586a73f/earthsciio/cachekey.py#L47)

</div>

<div class="api-tab" data-lang="julia" data-label="Julia">
<p class="api-tab-label">Julia</p>

<p class="api-absent">Julia does not export this function.</p>

</div>

<div class="api-tab" data-lang="rust" data-label="Rust">
<p class="api-tab-label">Rust</p>

```rust
pub fn sha256_file(path: &Path) -> Result<String>
```

Stream a file through sha256 without loading it fully into memory. Used for
integrity re-verification (cheap; off by default, on for CI/conformance).

[Source](https://github.com/EarthSciML/EarthSciIO/blob/a49538bbd6ec318dde369d3496f6e8880586a73f/rust/src/key.rs#L38) · [Rust docs](https://docs.rs/earthsciio/latest/earthsciio/fn.sha256_file.html)

</div>

</div>
