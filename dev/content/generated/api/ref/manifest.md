---
title: "manifest"
description: "Stable type · Cache"
type: "api"
api_tabs: true
---


**Tier:** stable · **Kind:** type · **Topic:** [Cache](/generated/api/topics/cache)

| Python | Julia | Rust |
|---|---|---|
| `Manifest` | `Manifest` | `Manifest` |


<div class="api-tabs">

<div class="api-tab" data-lang="python" data-label="Python">
<p class="api-tab-label">Python</p>

```python
class Manifest
Manifest(url: str, sha256_content: str, bytes: int, fetched_at: str, etag: str | None = None, last_modified: str | None = None, source_loader: str | None = None, auth_realm: str | None = None, schema: str = MANIFEST_SCHEMA_TAG)
Manifest.to_dict() -> dict
Manifest.to_json() -> str
@classmethod Manifest.from_dict(obj: dict) -> Manifest
@classmethod Manifest.from_json(text: str) -> Manifest
```

In-memory view of `meta/<key>.json` (see the schema for field meanings).

**Members**

- `to_dict` — All nine fields, always present (the key carries `null` when N/A).
- `to_json` — Serialize exactly as the corpus does: sorted keys, indent 2, newline.

[Source](https://github.com/EarthSciML/EarthSciIO/blob/a49538bbd6ec318dde369d3496f6e8880586a73f/earthsciio/manifest.py#L22)

</div>

<div class="api-tab" data-lang="julia" data-label="Julia">
<p class="api-tab-label">Julia</p>

```julia
struct Manifest
Manifest(url::String, etag::Union{Nothing, String}, last_modified::Union{Nothing, String}, sha256_content::String, bytes::Int64, fetched_at::String, source_loader::Union{Nothing, String}, auth_realm::Union{Nothing, String})
Manifest(url, etag, last_modified, sha256_content, bytes, fetched_at, source_loader, auth_realm)
```

```julia
Manifest
```

Validation + provenance record for one cached blob. Field mapping (bead → here):
source-url → `url`, etag → `etag`, checksum → `sha256_content`,
fetched-at → `fetched_at`, byte-size → `bytes`.

[Source](https://github.com/EarthSciML/EarthSciIO/blob/a49538bbd6ec318dde369d3496f6e8880586a73f/julia/src/manifest.jl#L10)

</div>

<div class="api-tab" data-lang="rust" data-label="Rust">
<p class="api-tab-label">Rust</p>

```rust
pub struct Manifest {
    pub auth_realm: Option<String>,
    pub bytes: u64,
    pub etag: Option<String>,
    pub fetched_at: String,
    pub last_modified: Option<String>,
    pub schema: String,
    pub sha256_content: String,
    pub source_loader: Option<String>,
    pub url: String,
}

impl Manifest { pub fn to_json_bytes(&self) -> Result<Vec<u8>> }

impl Manifest { pub fn from_json_bytes(data: &[u8]) -> Result<Manifest> }

impl Manifest { pub fn read(path: &Path) -> Result<Manifest> }
```

Validation + provenance record stored alongside every cached blob.

`url`, `sha256_content`, `bytes`, and `fetched_at` are required; the
remaining fields are always present but may be `null`. Credentials are
**never** stored — only the `auth_realm` name.

[Source](https://github.com/EarthSciML/EarthSciIO/blob/a49538bbd6ec318dde369d3496f6e8880586a73f/rust/src/manifest.rs#L28) · [Rust docs](https://docs.rs/earthsciio/latest/earthsciio/manifest/struct.Manifest.html)

</div>

</div>
