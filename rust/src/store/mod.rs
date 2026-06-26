//! The `store` registry (`spec/registries.md` §3): where the content-addressed
//! cache physically lives. The cache **key** is store-independent; a store just
//! realizes the on-disk (or object-store) layout.
//!
//! Component (a) ships the active `local` backend (`spec/cache-format.md` §2).
//! An `s3` backend is a registered stub elsewhere; the trait below is what it
//! plugs into without any Provider change.

mod local;

pub use local::{LocalStore, CACHE_VERSION};

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::error::Result;
use crate::manifest::Manifest;

/// A staging handle for an in-progress download: a `tmp/<uuid>.part` file on the
/// **same filesystem** as the blobs, so committing it is an atomic rename rather
/// than a cross-device copy. Dropping it without committing removes the partial
/// file (`spec/cache-format.md` §6 — "never commit partial downloads").
pub struct StagingFile {
    temp: tempfile::NamedTempFile,
}

impl StagingFile {
    pub(crate) fn new(temp: tempfile::NamedTempFile) -> Self {
        Self { temp }
    }

    /// The staging path a transport writes its bytes to.
    pub fn path(&self) -> &Path {
        self.temp.path()
    }

    /// Consume the handle, yielding the underlying temp file (for committing).
    pub(crate) fn into_temp(self) -> tempfile::NamedTempFile {
        self.temp
    }
}

/// An acquired per-blob advisory lock. The OS lock is released when this guard
/// drops (the underlying file descriptor closes) — RAII, scope = one blob fetch.
pub struct BlobLock {
    // Held only for its Drop: closing the fd releases the flock.
    _file: std::fs::File,
}

impl BlobLock {
    pub(crate) fn new(file: std::fs::File) -> Self {
        Self { _file: file }
    }
}

/// Physical home of the content-addressed cache. Keyed by store name in the
/// store registry.
pub trait Store: Send + Sync {
    /// The store's registry name (e.g. `"local"`).
    fn name(&self) -> &str;

    /// Is a blob present for `key`? (No validity check — that's the cache's job.)
    fn exists(&self, key: &str) -> bool;

    /// Path to the blob for `key`, or `None` on a miss. Lookups are by key,
    /// never by the debug-only extension.
    fn get_blob(&self, key: &str) -> Option<PathBuf>;

    /// Read the manifest for `key`, or `None` if absent.
    fn get_meta(&self, key: &str) -> Result<Option<Manifest>>;

    /// Write the manifest for `key` (atomically).
    fn put_meta(&self, key: &str, manifest: &Manifest) -> Result<()>;

    /// Open a fresh staging file on the cache filesystem.
    fn new_staging(&self) -> Result<StagingFile>;

    /// Atomically commit a staged download into the blob slot for `key`. `ext`
    /// is a debug-only suffix taken from the URL/content-type.
    fn commit_blob(&self, key: &str, staged: StagingFile, ext: &str) -> Result<PathBuf>;

    /// Acquire the per-blob advisory lock (blocks until held).
    fn lock(&self, key: &str) -> Result<BlobLock>;
}

/// Store-name → store instance lookup (`spec/registries.md` §3). Swapping the
/// configured name (e.g. `local`→`s3`) changes where blobs live without
/// touching the Provider, the key scheme, or any reader.
#[derive(Default, Clone)]
pub struct StoreRegistry {
    by_name: HashMap<String, Arc<dyn Store>>,
}

impl StoreRegistry {
    /// An empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Registry with the active `local` store rooted at `root` (`$EARTHSCIDATADIR`).
    pub fn with_local(root: impl Into<PathBuf>) -> Self {
        let mut r = Self::new();
        r.register(Arc::new(LocalStore::new(root)));
        r
    }

    /// Register a store under its name.
    pub fn register(&mut self, store: Arc<dyn Store>) -> &mut Self {
        self.by_name.insert(store.name().to_string(), store);
        self
    }

    /// Look up the store for a configured name.
    pub fn get(&self, name: &str) -> Option<Arc<dyn Store>> {
        self.by_name.get(name).cloned()
    }

    /// All registered store names.
    pub fn registered(&self) -> Vec<String> {
        self.by_name.keys().cloned().collect()
    }
}
