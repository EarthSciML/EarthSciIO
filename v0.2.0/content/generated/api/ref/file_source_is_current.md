---
title: "file_source_is_current"
description: "Stable function · Cache"
type: "api"
api_tabs: true
---


**Tier:** stable · **Kind:** function · **Topic:** [Cache](/generated/api/topics/cache)

| Python | Julia | Rust |
|---|---|---|
| `file_source_is_current` | `file_source_is_current` | `file_source_is_current` |


<div class="api-tabs">

<div class="api-tab" data-lang="python" data-label="Python">
<p class="api-tab-label">Python</p>

```python
file_source_is_current(source: os.PathLike, manifest: Manifest) -> bool
```

Rung 0 as a yes/no: is the source **provably** the file that was ingested?

Only `CURRENT` is `True`. The cache does not use this — the
difference between `MISSING`, `REPLACED` and `UNKNOWN` is
exactly what it acts on — but it is the honest predicate for a caller that
just wants the question answered.

[Source](https://github.com/EarthSciML/EarthSciIO/blob/a49538bbd6ec318dde369d3496f6e8880586a73f/earthsciio/validate.py#L326)

</div>

<div class="api-tab" data-lang="julia" data-label="Julia">
<p class="api-tab-label">Julia</p>

```julia
file_source_is_current(source::AbstractString, m::Manifest)
```

```julia
file_source_is_current(source, manifest) -> Bool
```

Rung 0 as a yes/no: is the source PROVABLY the file that was ingested? Only
[`SOURCE_CURRENT`] is `true`. The cache does not use this — the difference
between [`SOURCE_MISSING`], [`SOURCE_REPLACED`] and [`SOURCE_UNKNOWN`] is exactly
what it acts on — but it is the honest predicate for a caller that just wants the
question answered.

[Source](https://github.com/EarthSciML/EarthSciIO/blob/a49538bbd6ec318dde369d3496f6e8880586a73f/julia/src/cache.jl#L296)

</div>

<div class="api-tab" data-lang="rust" data-label="Rust">
<p class="api-tab-label">Rust</p>

```rust
pub fn file_source_is_current(source: &Path, manifest: &Manifest) -> bool
```

Rung 0 as a yes/no: is the source **provably** the file that was ingested?

Only `SourceState::Current` is `true`. The cache does not use this — the
difference between `Missing`, `Replaced` and `Unknown` is exactly what it
acts on — but it is the honest predicate for a caller that just wants the
question answered.

[Source](https://github.com/EarthSciML/EarthSciIO/blob/a49538bbd6ec318dde369d3496f6e8880586a73f/rust/src/validate.rs#L438) · [Rust docs](https://docs.rs/earthsciio/latest/earthsciio/validate/fn.file_source_is_current.html)

</div>

</div>
