---
title: "file_source_state"
description: "Stable function · Cache"
type: "api"
api_tabs: true
---


**Tier:** stable · **Kind:** function · **Topic:** [Cache](/generated/api/topics/cache)

| Python | Julia | Rust |
|---|---|---|
| `file_source_state` | `file_source_state` | `file_source_state` |


<div class="api-tabs">

<div class="api-tab" data-lang="python" data-label="Python">
<p class="api-tab-label">Python</p>

```python
file_source_state(source: os.PathLike, manifest: Manifest) -> str
```

Rung 0 without a memo: what is the `file://` source behind this entry?

The cache is keyed by the resolved URL, so a local file replaced **in
place** keeps the same key and every later read is served the bytes of the
file that used to be there. Nothing in `decide` catches it: a
`file://` source has no ETag and no `Last-Modified`, and a source with
no `temporal` is *declared* immutable by rule 2, so `decide` answers
`HIT` forever. That is `EarthSciML/EarthSciAST#293
<https://github.com/EarthSciML/EarthSciAST/issues/293>`_, where a snapshot
corpus was replaced at the same paths and a test suite stayed green against
a corpus that no longer existed — the manifest recorded 371 bytes while the
file on disk was 363.

Note what this is NOT. `Cache(verify=True)` hashes the **cached blob**
against the manifest: it compares the copy with the record of the copy, so
it passes with flying colours while the file the copy was made from has been
replaced. This compares the **source** with that record.

The manifest already carries everything needed, so this is not a
cache-format change:

* nothing at the path, or not a regular file → `MISSING`;
* the path cannot be read at all → `UNKNOWN`, and rung 0 abstains
  rather than claim a change it did not observe;
* on-disk length != `manifest.bytes` → `REPLACED`, **without
  hashing**;
* `sha256(source)` != `manifest.sha256_content` → `REPLACED`. Size
  alone would not do: a float re-encode, a different scenario year, or any
  edit that preserves the length is exactly what a size check waves through;
* otherwise `CURRENT` — serve the cached blob, re-ingest nothing.

A [`Cache`](/generated/api/ref/cache) calls `SourceRevalidator.state`
instead, which is this with a `(size, mtime)` memo in front of the read;
use this when there is nothing to amortise over.

[Source](https://github.com/EarthSciML/EarthSciIO/blob/a49538bbd6ec318dde369d3496f6e8880586a73f/earthsciio/validate.py#L287)

</div>

<div class="api-tab" data-lang="julia" data-label="Julia">
<p class="api-tab-label">Julia</p>

```julia
file_source_state(source::AbstractString, m::Manifest)
```

```julia
file_source_state(source, manifest) -> Symbol
```

Rung 0 without a memo (spec/cache-format.md §4.1): is the `file://` source behind
a cached entry still the file that was ingested into it?

The cache is keyed by the resolved URL, so a local file replaced IN PLACE keeps
the same key and every later read is served the bytes of the file that used to be
there. Nothing else catches it: a `file://` source has no ETag and no
`Last-Modified`, and a present blob is immutable by default, so the entry
outlives the file it was copied from. That is EarthSciML/EarthSciAST#293, where a
snapshot corpus was replaced at the same paths and a test suite stayed green
against a corpus that no longer existed — the manifest recorded 371 bytes while
the file on disk was 363.

Note what this is NOT: `verify=true` hashes the CACHED BLOB against the manifest,
comparing the copy with the record of the copy, so it passes with flying colours
while the file the copy was made from has been replaced. This compares the
SOURCE with that record.

The manifest already carries everything needed, so this is not a cache-format
change:

  * nothing at the path, or not a regular file ⇒ [`SOURCE_MISSING`];
  * the path cannot be read at all ⇒ [`SOURCE_UNKNOWN`], and rung 0 abstains
    rather than claim a change it did not observe;
  * on-disk size != `manifest.bytes` ⇒ [`SOURCE_REPLACED`], WITHOUT hashing;
  * `sha256(source)` != `manifest.sha256_content` ⇒ [`SOURCE_REPLACED`]. Size
    alone would not do: a float re-encode, a different scenario year, or any edit
    that preserves the size is exactly what a size check waves through;
  * otherwise [`SOURCE_CURRENT`] — serve the cached blob, re-ingest nothing.

A [`Cache`] calls [`source_state`] instead, which is this with a `(size, mtime)`
memo in front of the read.

[Source](https://github.com/EarthSciML/EarthSciIO/blob/a49538bbd6ec318dde369d3496f6e8880586a73f/julia/src/cache.jl#L259)

</div>

<div class="api-tab" data-lang="rust" data-label="Rust">
<p class="api-tab-label">Rust</p>

```rust
pub fn file_source_state(source: &Path, manifest: &Manifest) -> SourceState
```

Rung 0 without a memo: is the `file://` source behind a cached entry still
the file that was ingested into it?

The cache is keyed by the resolved URL, so a local file replaced **in place**
keeps the same key and every later read is served the bytes of the file that
used to be there. Nothing above catches it: a `file://` source has no ETag
and no `Last-Modified`, and a source with no `temporal` is declared
immutable (rule 2), so `decide` answers `Hit` forever. That is
`EarthSciML/EarthSciAST#293`, where a snapshot corpus was replaced at the
same paths and a test suite stayed green against the corpus that no longer
existed — the manifest recorded 371 bytes while the file on disk was 363.

Note what this is NOT. `Cache::verify_on_read` hashes the **cached blob**
against the manifest; it compares the copy with the record of the copy, so
it passes with flying colours while the file the copy was made from has been
replaced. This compares the **source** with that record — the check whose
absence the issue reports.

The manifest already carries everything needed, so this is not a cache-format
change:

* nothing at the path, or not a regular file ⇒ `SourceState::Missing`;
* the path cannot be read at all ⇒ `SourceState::Unknown`, and rung 0
  abstains rather than claim a change it did not observe;
* on-disk length != `manifest.bytes` ⇒ `SourceState::Replaced`, no hashing;
* `sha256(source)` != `manifest.sha256_content` ⇒ `SourceState::Replaced`.
  Size alone would not do: a float re-encode, a different simulation year, or
  any edit that preserves the length is exactly the case a size check waves
  through;
* otherwise `SourceState::Current` — serve the warm blob, re-ingest nothing.

A [`Cache`](/generated/api/ref/cache) calls `SourceRevalidator::state` instead, which is this with a
`(length, mtime)` memo in front of the hash; use this when there is nothing
to amortise over.

[Source](https://github.com/EarthSciML/EarthSciIO/blob/a49538bbd6ec318dde369d3496f6e8880586a73f/rust/src/validate.rs#L428) · [Rust docs](https://docs.rs/earthsciio/latest/earthsciio/validate/fn.file_source_state.html)

</div>

</div>
