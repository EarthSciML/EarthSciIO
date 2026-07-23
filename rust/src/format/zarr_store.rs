//! `zarrs` storage adapters used by the Zarr reader/writer.
//!
//! Two backends bridge the crate's data plumbing to the `zarrs` storage traits:
//!
//! * [`CacheStorage`] — a **read-only** [`zarrs::storage::ReadableStorageTraits`]
//!   backed by the crate's content-addressed [`Cache`]. Each Zarr `StoreKey`
//!   (`field3d/.zarray`, `field3d/0.0.0`, `field3d/c/0/0/0`, …) maps to the object
//!   URL `<base_url>/<key>` and is fetched through `cache.fetch`, so a Zarr store
//!   read reuses the exact same offline/HTTP/S3 transport + integrity + locking
//!   path as every other blob. This is what keeps the pinned ISRM corpus store
//!   (Zarr **v2**, blosc-lz4) readable fully offline: zarrs converts the v2 blosc
//!   compressor metadata to a v3 codec chain on open and decodes the chunks.
//!
//! * an `object_store`-backed opener (`object_store` module, feature-gated) — a
//!   **direct** (non-cache) read/write path over Apache Arrow `object_store` via the
//!   `zarrs_object_store` adapter, covering `s3://`, `http(s)://`, and local
//!   `file://` roots. This is where S3 access is backed by `object_store`'s
//!   mature S3 client rather than any hand-rolled S3 code on the Rust side.
//!   (Feature-gated behind `object-store` to keep the default build's C/TLS
//!   footprint small; see `Cargo.toml`.)

use std::sync::Arc;

use zarrs::storage::byte_range::ByteRangeIterator;
use zarrs::storage::{
    Bytes, MaybeBytesIterator, ReadableStorageTraits, StorageError, StoreKey,
};

use crate::cache::{Cache, FetchRequest};

/// Drop a `"dimension_separator": null` field from a Zarr **v2** `.zarray` JSON
/// document (a zarr-python/numcodecs quirk `zarrs` rejects). Returns the input
/// unchanged if it is not the JSON object with that null field.
fn sanitize_v2_array_meta(data: Vec<u8>) -> Vec<u8> {
    let Ok(serde_json::Value::Object(mut map)) =
        serde_json::from_slice::<serde_json::Value>(&data)
    else {
        return data;
    };
    if map.get("dimension_separator") != Some(&serde_json::Value::Null) {
        return data;
    }
    map.remove("dimension_separator");
    serde_json::to_vec(&serde_json::Value::Object(map)).unwrap_or(data)
}

/// A read-only `zarrs` storage backed by the content-addressed [`Cache`].
///
/// The store root is `base_url`; a Zarr `StoreKey` `k` is the object at
/// `<base_url>/<k>`, fetched (and cached / integrity-checked / lock-coordinated)
/// through `cache.fetch`. A cache **miss** maps to "key not present" (`Ok(None)`),
/// exactly as a Zarr store reports an absent chunk/metadata object.
///
/// The [`Cache`] is held as an `Arc` (not a borrow) because `zarrs`' `Array`
/// requires `TStorage: 'static`.
pub(crate) struct CacheStorage {
    cache: Arc<Cache>,
    base: String,
}

impl CacheStorage {
    /// A cache-backed store rooted at `base_url` (trailing `/` trimmed).
    pub(crate) fn new(cache: Arc<Cache>, base_url: &str) -> Self {
        Self {
            cache,
            base: base_url.trim_end_matches('/').to_string(),
        }
    }

    /// Fetch the whole object for `key` (or `None` on a cache miss). The cache is
    /// object-granular and content-addressed — there is no server-side byte-range
    /// read — so partial reads slice this whole buffer in memory (see
    /// [`supports_get_partial`](ReadableStorageTraits::supports_get_partial)).
    fn fetch_whole(&self, key: &StoreKey) -> Result<Option<Bytes>, StorageError> {
        let url = format!("{}/{}", self.base, key.as_str());
        match self.cache.fetch(&FetchRequest::new(&url)) {
            Ok(blob) => {
                let data = std::fs::read(&blob.path).map_err(|e| {
                    StorageError::Other(format!(
                        "reading cached zarr object {}: {e}",
                        blob.path.display()
                    ))
                })?;
                // Zarr v2 `.zarray` written by zarr-python/numcodecs commonly carries
                // `"dimension_separator": null`, which `zarrs` refuses to parse (it
                // wants ".", "/", or the field absent). Normalize it away — an absent
                // separator defaults to "." exactly as the corpus store intends.
                let data = if key.as_str().ends_with(".zarray") {
                    sanitize_v2_array_meta(data)
                } else {
                    data
                };
                Ok(Some(Bytes::from(data)))
            }
            Err(e) if e.is_cache_miss() => Ok(None),
            Err(e) => Err(StorageError::Other(format!(
                "cache fetch of zarr object {url}: {e}"
            ))),
        }
    }
}

impl ReadableStorageTraits for CacheStorage {
    fn get_partial_many<'a>(
        &'a self,
        key: &StoreKey,
        byte_ranges: ByteRangeIterator<'a>,
    ) -> Result<MaybeBytesIterator<'a>, StorageError> {
        let Some(whole) = self.fetch_whole(key)? else {
            return Ok(None);
        };
        let size = whole.len() as u64;
        // Resolve every requested range against the whole object up front (the
        // object is already in memory), so the returned iterator is infallible on
        // I/O and just yields slices.
        let mut out: Vec<Result<Bytes, StorageError>> = Vec::new();
        for br in byte_ranges {
            let start = br.start(size) as usize;
            let len = br.length(size) as usize;
            let end = start + len;
            if end > whole.len() {
                out.push(Err(StorageError::Other(format!(
                    "zarr byte range {start}..{end} out of bounds for object of {} bytes",
                    whole.len()
                ))));
            } else {
                out.push(Ok(whole.slice(start..end)));
            }
        }
        Ok(Some(Box::new(out.into_iter())))
    }

    fn size_key(&self, key: &StoreKey) -> Result<Option<u64>, StorageError> {
        Ok(self.fetch_whole(key)?.map(|b| b.len() as u64))
    }

    fn supports_get_partial(&self) -> bool {
        // The cache stores whole objects; zarrs will full-read then slice locally.
        false
    }
}
