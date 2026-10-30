---
title: "Rust extension seams"
description: "Rust's extension-tier names: documented, reachable, not portable."
type: "api"
---


Extension-tier names are part of this binding's documented surface but are not harmonized across bindings, and may change in a minor release.

1 names.

### `Reader` {#x-reader}

*type* · canonical name `reader`

```rust
pub trait Reader: Send + Sync {
    fn formats(&self) -> &'static [&'static str];
    fn extensions(&self) -> &'static [&'static str];
    fn read_native(&self, blob_path: &Path, variables: &[String], select: &Selection) -> Result<NativeDataset>;
    fn read_native_records(&self, blob_path: &Path, variables: &[String], select: &Selection, records: Option<&Records>) -> Result<NativeDataset>;
    fn supports_records(&self) -> bool;
    fn dim_length(&self, _blob_path: &Path, _dim: &str) -> Result<Option<usize>>;
    fn store_backed(&self) -> bool;
    fn supports_selection(&self) -> bool;
    fn read_store(&self, _cache: Arc<Cache>, _base_url: &str, _variables: &[String], _select: &Selection) -> Result<NativeDataset>;
    fn array_shape(&self, _cache: Arc<Cache>, _base_url: &str, _var: &str) -> Result<Option<Vec<usize>>>;
    fn configured(&self, options: &Map<String, Value>) -> Result<Option<Arc<dyn Reader>>>;
    fn supports_direct_read(&self) -> bool;
    fn read_store_direct(&self, _base_url: &str, _variables: &[String], _select: &Selection, _options: &[(String, String)]) -> Result<NativeDataset>;
    fn array_shape_direct(&self, _base_url: &str, _var: &str, _options: &[(String, String)]) -> Result<Option<Vec<usize>>>;
}
```

A format reader (`spec/registries.md` §2): opens a cached blob and returns
CF-decoded native arrays. Keyed by format name in the `FormatRegistry`.

Native-only: a reader's input is a path into the content-addressed cache.

[Source](https://github.com/EarthSciML/EarthSciIO/blob/a49538bbd6ec318dde369d3496f6e8880586a73f/rust/src/format/mod.rs#L746) · [Rust docs](https://docs.rs/earthsciio/latest/earthsciio/format/trait.Reader.html)
