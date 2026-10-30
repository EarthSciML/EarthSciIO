---
title: "commit_token"
description: "Stable type · Streaming writer"
type: "api"
api_tabs: true
---


**Tier:** stable · **Kind:** type · **Topic:** [Streaming writer](/generated/api/topics/writer)

| Python | Julia | Rust |
|---|---|---|
| not exported | `CommitToken` | `CommitToken` |


<div class="api-tabs">

<div class="api-tab" data-lang="python" data-label="Python">
<p class="api-tab-label">Python</p>

<p class="api-absent">Python does not export this type.</p>

</div>

<div class="api-tab" data-lang="julia" data-label="Julia">
<p class="api-tab-label">Julia</p>

```julia
struct CommitToken
CommitToken(writer::Int64, region::Vector{Pair{String, Tuple{Int64, Int64}}}, fingerprint::String, records::Vector{Tuple{Int64, Float64}}, statics::Vector{String}, blobs::Vector{Tuple{String, Union{Nothing, Int64}, String}}, objects::Vector{String})
CommitToken(writer, region, fingerprint, records, statics, blobs, objects)
```

```julia
CommitToken
```

What one writer wrote since its previous token (spec/writer.md §5.2), returned by
[`write_flush!`] and handed (by the caller — EarthSciIO is MPI-free) to the
owner's [`write_commit!`]/[`write_close!`]. Fields: `writer` id, its `region`
(`dim => (start, stop)`), the schema `fingerprint`, `records` (`(index, t)`,
ascending), `statics` (names, sorted), `blobs` (`(scope, record, key)`, sorted;
`record` is `nothing` for scope `"static"`) and the advisory `objects` (store
keys PUT, sorted). [`token_json`]/[`parse_token`] are its documented JSON form.

[Source](https://github.com/EarthSciML/EarthSciIO/blob/a49538bbd6ec318dde369d3496f6e8880586a73f/julia/src/zarr_stream.jl#L195)

</div>

<div class="api-tab" data-lang="rust" data-label="Rust">
<p class="api-tab-label">Rust</p>

```rust
pub struct CommitToken {
    pub schema: String,
    pub writer: usize,
    pub region: BTreeMap<String, [usize; 2]>,
    pub fingerprint: String,
    pub records: Vec<TokenRecord>,
    pub statics: Vec<String>,
    pub blobs: Vec<TokenBlob>,
    pub objects: Vec<String>,
}

impl CommitToken { pub fn to_json(&self) -> String }

impl CommitToken { pub fn from_json(s: &str) -> Result<Self> }

impl CommitToken { pub fn is_empty(&self) -> bool }
```

The value a writer hands the owner after `flush` (`spec/writer.md` §5.2).
Serializes to the documented JSON wire format.

[Source](https://github.com/EarthSciML/EarthSciIO/blob/a49538bbd6ec318dde369d3496f6e8880586a73f/rust/src/format/zarr_stream.rs#L427) · [Rust docs](https://docs.rs/earthsciio/latest/earthsciio/format/zarr_stream/struct.CommitToken.html)

</div>

</div>
