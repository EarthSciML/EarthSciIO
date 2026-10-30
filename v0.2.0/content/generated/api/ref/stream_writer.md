---
title: "stream_writer"
description: "Stable type · Streaming writer"
type: "api"
api_tabs: true
---


**Tier:** stable · **Kind:** type · **Topic:** [Streaming writer](/generated/api/topics/writer)

| Python | Julia | Rust |
|---|---|---|
| not exported | `ZarrWriter` | `ZarrStreamWriter` |


<div class="api-tabs">

<div class="api-tab" data-lang="python" data-label="Python">
<p class="api-tab-label">Python</p>

<p class="api-absent">Python does not export this type.</p>

</div>

<div class="api-tab" data-lang="julia" data-label="Julia">
<p class="api-tab-label">Julia</p>

```julia
struct ZarrWriter <: Writer
ZarrWriter()
```

The `zarr` streaming writer (sharded Zarr v3). Registered `:active` in
[`WRITER_REGISTRY`]; the write mirror of [`ZarrReader`]. Its handle and the
streaming contract live in zarr_stream.jl.

[Source](https://github.com/EarthSciML/EarthSciIO/blob/a49538bbd6ec318dde369d3496f6e8880586a73f/julia/src/zarr_write.jl#L187)

</div>

<div class="api-tab" data-lang="rust" data-label="Rust">
<p class="api-tab-label">Rust</p>

```rust
pub struct ZarrStreamWriter<S> {
    /* private fields */
}

impl<S> ZarrStreamWriter<S> { pub fn create(store: Arc<S>, base_url: &str, schema: StreamSchema, role: Role, region: Region) -> Result<Self> }

impl<S> ZarrStreamWriter<S> { pub fn join(store: Arc<S>, base_url: &str, role: Role, region: Region) -> Result<Self> }

impl<S> ZarrStreamWriter<S> { pub fn resume(store: Arc<S>, base_url: &str, role: Role, region: Region, at: Option<f64>) -> Result<Self> }

impl<S> ZarrStreamWriter<S> { pub fn write_record(&mut self, t: f64, arrays: &RecordArrays<'_>) -> Result<u64> }

impl<S> ZarrStreamWriter<S> { pub fn write_record_at(&mut self, index: u64, t: f64, arrays: &RecordArrays<'_>) -> Result<()> }

impl<S> ZarrStreamWriter<S> { pub fn write_static(&mut self, name: &str, array: TypedArray<'_>) -> Result<()> }

impl<S> ZarrStreamWriter<S> { pub fn write_blob(&mut self, scope: BlobScope, key: &str, bytes: &[u8]) -> Result<()> }

impl<S> ZarrStreamWriter<S> { pub fn flush(&mut self) -> Result<CommitToken> }

impl<S> ZarrStreamWriter<S> { pub fn commit(&mut self, tokens: &[CommitToken]) -> Result<CommitReceipt> }

impl<S> ZarrStreamWriter<S> { pub fn invalidate(&mut self, indices: &[u64]) -> Result<()> }

impl<S> ZarrStreamWriter<S> { pub fn invalidate_statics(&mut self, names: &[&str]) -> Result<()> }

impl<S> ZarrStreamWriter<S> { pub fn close(self, tokens: &[CommitToken]) -> Result<Option<Value>> }

impl<S> ZarrStreamWriter<S> { pub fn region(&self) -> Region }

impl<S> ZarrStreamWriter<S> { pub fn fingerprint(&self) -> &str }
```

The streaming writer handle (`spec/writer.md`). See the module docs.

[Source](https://github.com/EarthSciML/EarthSciIO/blob/a49538bbd6ec318dde369d3496f6e8880586a73f/rust/src/format/zarr_stream.rs#L1384) · [Rust docs](https://docs.rs/earthsciio/latest/earthsciio/format/zarr_stream/struct.ZarrStreamWriter.html)

</div>

</div>
