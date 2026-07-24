//! Direct (non-cache) Zarr read/write over Apache Arrow `object_store`
//! (feature `object-store`).
//!
//! This is the S3 (and HTTP / local `file://`) path that is **backed by
//! `object_store`'s** mature clients rather than any hand-rolled S3 code. A store
//! URL is dispatched by [`object_store::parse_url`] (`s3://`, `http(s)://`,
//! `file://`), rooted at its path prefix via [`PrefixStore`], wrapped by the
//! [`zarrs_object_store`] adapter (async), and bridged into the crate's **sync**
//! Zarr reader/writer through zarrs' [`AsyncToSyncStorageAdapter`] driven by a
//! `tokio` runtime. The decode/encode logic itself is shared with the
//! cache-backed path ([`super::zarr::read_arrays`] /
//! [`super::zarr_write::write_all_to_store`]) — only the storage backend differs.
//!
//! `s3://` resolves only when `object_store`'s `aws` feature is compiled in;
//! credentials/region come from the standard AWS environment. `file://` and
//! `http(s)://` are always available. The content-addressed cache remains the
//! path used by the [`crate::Provider`]; this module is for callers that want to
//! read/write a store directly.

use std::sync::Arc;

use object_store::prefix::PrefixStore;
use object_store::ObjectStore;
use url::Url;
use zarrs::storage::storage_adapter::async_to_sync::{
    AsyncToSyncBlockOn, AsyncToSyncStorageAdapter,
};
use zarrs_object_store::AsyncObjectStore;

use super::{AxisSelect, NativeDataset, OutputSchema, Selection};
use crate::error::{Error, Result};

fn os_err(detail: impl Into<String>) -> Error {
    Error::Format {
        format: "zarr".to_string(),
        detail: detail.into(),
    }
}

/// A `tokio` `block_on` bridge so an async `object_store` store can be used from
/// the crate's synchronous Zarr API (via zarrs' [`AsyncToSyncStorageAdapter`]).
struct TokioBlockOn(tokio::runtime::Handle);

impl AsyncToSyncBlockOn for TokioBlockOn {
    fn block_on<F: core::future::Future>(&self, future: F) -> F::Output {
        self.0.block_on(future)
    }
}

/// The sync storage type: an object_store store, prefix-rooted, adapted to sync.
type SyncObjectStore =
    AsyncToSyncStorageAdapter<AsyncObjectStore<PrefixStore<Box<dyn ObjectStore>>>, TokioBlockOn>;

/// Build a multi-thread `tokio` runtime for driving `object_store` I/O.
fn runtime() -> Result<tokio::runtime::Runtime> {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| os_err(format!("build tokio runtime for object_store: {e}")))
}

/// Resolve a store URL to a sync-usable `zarrs` storage rooted at the store.
fn build_sync_store(url_str: &str, handle: tokio::runtime::Handle) -> Result<Arc<SyncObjectStore>> {
    let url =
        Url::parse(url_str).map_err(|e| os_err(format!("invalid store URL '{url_str}': {e}")))?;
    let (store, prefix) = object_store::parse_url(&url)
        .map_err(|e| os_err(format!("no object_store backend for '{url_str}': {e}")))?;
    let prefixed = PrefixStore::new(store, prefix);
    let async_store = Arc::new(AsyncObjectStore::new(prefixed));
    Ok(Arc::new(AsyncToSyncStorageAdapter::new(
        async_store,
        TokioBlockOn(handle),
    )))
}

/// Read `variables` from a Zarr store at `url` (`s3://`, `http(s)://`, `file://`)
/// directly through `object_store`, applying the orthogonal `select` lazily.
///
/// # Errors
/// Returns [`Error::Format`] if the URL has no `object_store` backend, the store
/// cannot be opened, or a decode fails.
pub fn read_zarr_object_store(
    url: &str,
    variables: &[String],
    select: &Selection,
) -> Result<NativeDataset> {
    if variables.is_empty() {
        return Err(os_err(
            "object-store zarr read requires an explicit list of variables",
        ));
    }
    // The runtime must outlive the synchronous decode (the adapter blocks on it).
    let rt = runtime()?;
    let store = build_sync_store(url, rt.handle().clone())?;
    let axes: Option<&[AxisSelect]> = match select {
        Selection::Orthogonal(a) => Some(a.as_slice()),
        _ => None,
    };
    super::zarr::read_arrays(store, variables, axes)
}

/// Write a sharded Zarr **v3** store to `url` (`s3://`, `file://`) through
/// `object_store`, following `schema` (same layout as [`super::write_zarr_v3`]).
///
/// # Errors
/// Returns [`Error::Format`] on schema inconsistency, a missing backend, or a
/// store write error.
pub fn write_zarr_object_store(url: &str, schema: &OutputSchema) -> Result<()> {
    let rt = runtime()?;
    let store = build_sync_store(url, rt.handle().clone())?;
    super::zarr_write::write_all_to_store(store, url, schema)
}
