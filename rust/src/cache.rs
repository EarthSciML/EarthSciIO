//! The cache fetcher — the seam that ties transport + store + offline + auth +
//! validation together. This is the entry point a Provider (component (b)) or
//! the ESS opener calls: given a resolved URL, it returns a cached blob,
//! fetching + caching it first when necessary.
//!
//! Fetch flow (`spec/cache-format.md` §6):
//! 1. Compute `key = sha256(resolved_url)`.
//! 2. If present **and valid** → return it (a hit takes **no lock**).
//! 3. Otherwise take the per-blob advisory lock, **re-check** (another process
//!    may have just filled it), download to a `tmp/<uuid>.part` staging file,
//!    verify, **atomically rename** into `blobs/`, then write the manifest.
//!
//! Offline (`spec/offline-mode.md`): no transport is constructed; presence +
//! stored `sha256_content` is the only check; a miss raises [`Error::CacheMiss`].

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::auth::{AuthRegistry, AuthResolver};
use crate::clock::now_rfc3339;
use crate::datadir;
use crate::error::{Error, Result};
use crate::key::{cache_key, sha256_file};
use crate::manifest::{Manifest, MANIFEST_SCHEMA};
use crate::offline;
use crate::store::{LocalStore, StagingFile, Store, StoreRegistry};
use crate::transport::{
    CdsTransport, Conditional, FetchResult, FetchStatus, FileTransport, HttpTransport, Transport,
    TransportRegistry,
};
use crate::validate::{self, CacheDecision, Temporal};

/// A resolved cache entry: the blob path plus its manifest.
#[derive(Debug, Clone)]
pub struct CachedBlob {
    /// `sha256(resolved_url)` — the content-addressed cache key.
    pub key: String,
    /// Path to the cached blob on disk.
    pub path: PathBuf,
    /// The blob's manifest (validation + provenance).
    pub manifest: Manifest,
}

/// One fetch request. Build with [`FetchRequest::new`] and the chained setters,
/// or struct-literal with `..Default::default()`.
#[derive(Debug, Clone, Default)]
pub struct FetchRequest<'a> {
    /// The resolved URL (after time-anchor + parameter expansion). Defines the key.
    pub resolved_url: &'a str,
    /// The `.esm` loader that resolved the URL (provenance; optional).
    pub source_loader: Option<&'a str>,
    /// Auth realm to fetch under (looked up in the auth registry; optional).
    pub auth_realm: Option<&'a str>,
    /// The loader's temporal nature, for the TTL rung of validation.
    pub temporal: Option<Temporal>,
    /// A loader-declared content checksum (none today), strongest validity rung.
    pub expected_checksum: Option<&'a str>,
    /// Failover mirror URLs, tried in order after the primary. They share the
    /// **same** cache identity — the key/manifest record the canonical URL.
    pub mirrors: &'a [&'a str],
}

impl<'a> FetchRequest<'a> {
    /// A request for `resolved_url` with no extras.
    pub fn new(resolved_url: &'a str) -> Self {
        Self {
            resolved_url,
            ..Default::default()
        }
    }
    /// Set the provenance loader name.
    pub fn loader(mut self, loader: &'a str) -> Self {
        self.source_loader = Some(loader);
        self
    }
    /// Fetch under an auth realm (must be registered).
    pub fn auth_realm(mut self, realm: &'a str) -> Self {
        self.auth_realm = Some(realm);
        self
    }
    /// Set the loader's temporal nature (TTL validation).
    pub fn temporal(mut self, temporal: Temporal) -> Self {
        self.temporal = Some(temporal);
        self
    }
    /// Set a loader-declared content checksum.
    pub fn expected_checksum(mut self, checksum: &'a str) -> Self {
        self.expected_checksum = Some(checksum);
        self
    }
    /// Set failover mirror URLs.
    pub fn mirrors(mut self, mirrors: &'a [&'a str]) -> Self {
        self.mirrors = mirrors;
        self
    }
}

/// The cache fetcher. Cheap to clone the registries it holds; share one across
/// threads behind an `Arc`.
pub struct Cache {
    store: Arc<dyn Store>,
    transports: TransportRegistry,
    auth: AuthRegistry,
    offline: bool,
    verify_on_read: bool,
}

impl Cache {
    /// Start building a cache.
    pub fn builder() -> CacheBuilder {
        CacheBuilder::new()
    }

    /// A cache with built-in transports + a `local` store at the env-resolved
    /// `$EARTHSCIDATADIR`; offline resolved from `EARTHSCI_OFFLINE`.
    pub fn from_env() -> Result<Cache> {
        CacheBuilder::new().build()
    }

    /// Whether this cache runs offline (cache-only).
    pub fn is_offline(&self) -> bool {
        self.offline
    }

    /// The backing store.
    pub fn store(&self) -> &Arc<dyn Store> {
        &self.store
    }

    /// Fetch (or reuse) the blob for a request. Offline ⇒ cache-only.
    pub fn fetch(&self, req: &FetchRequest) -> Result<CachedBlob> {
        let key = cache_key(req.resolved_url);
        if self.offline {
            self.read_offline(&key, req.resolved_url)
        } else {
            self.fetch_online(&key, req)
        }
    }

    /// Convenience: a plain offline read of a resolved URL.
    pub fn get_offline(&self, resolved_url: &str) -> Result<CachedBlob> {
        let key = cache_key(resolved_url);
        self.read_offline(&key, resolved_url)
    }

    // --- offline ------------------------------------------------------------

    fn read_offline(&self, key: &str, url: &str) -> Result<CachedBlob> {
        let path = self.store.get_blob(key).ok_or_else(|| Error::CacheMiss {
            url: url.to_string(),
            key: key.to_string(),
        })?;
        // A valid entry has a manifest too; treat an entry missing its manifest
        // as a miss rather than returning a blob with no provenance.
        let manifest = self.store.get_meta(key)?.ok_or_else(|| Error::CacheMiss {
            url: url.to_string(),
            key: key.to_string(),
        })?;
        if self.verify_on_read {
            self.verify_integrity(key, &path, &manifest)?;
        }
        Ok(CachedBlob {
            key: key.to_string(),
            path,
            manifest,
        })
    }

    // --- online -------------------------------------------------------------

    fn fetch_online(&self, key: &str, req: &FetchRequest) -> Result<CachedBlob> {
        // Fast path: present + valid → return without taking a lock.
        if let Some(hit) = self.try_hit(key, req)? {
            return Ok(hit);
        }
        // Slow path: serialize redundant downloads behind the per-blob lock,
        // then re-check (a racer may have just filled it).
        let _lock = self.store.lock(key)?;
        if let Some(hit) = self.try_hit(key, req)? {
            return Ok(hit);
        }
        self.download_locked(key, req)
    }

    /// Return the cached blob only if the validation ladder says `Hit`.
    /// `Revalidate`/`Miss` ⇒ `None` (caller proceeds to download).
    fn try_hit(&self, key: &str, req: &FetchRequest) -> Result<Option<CachedBlob>> {
        let Some(path) = self.store.get_blob(key) else {
            return Ok(None);
        };
        let Some(manifest) = self.store.get_meta(key)? else {
            return Ok(None);
        };
        match validate::decide(&manifest, req.temporal.as_ref(), req.expected_checksum) {
            CacheDecision::Hit => {
                if self.verify_on_read {
                    self.verify_integrity(key, &path, &manifest)?;
                }
                Ok(Some(CachedBlob {
                    key: key.to_string(),
                    path,
                    manifest,
                }))
            }
            CacheDecision::Revalidate | CacheDecision::Miss => Ok(None),
        }
    }

    fn download_locked(&self, key: &str, req: &FetchRequest) -> Result<CachedBlob> {
        let prior = self.store.get_meta(key)?;
        let conditional = match &prior {
            Some(m) => Conditional {
                etag: m.etag.clone(),
                last_modified: m.last_modified.clone(),
            },
            None => Conditional::default(),
        };

        // Resolve the auth resolver up front — an unknown realm is a clean error.
        let auth: Option<Arc<dyn AuthResolver>> = match req.auth_realm {
            Some(realm) => Some(self.auth.get(realm).ok_or_else(|| Error::MissingAuth {
                realm: realm.to_string(),
            })?),
            None => None,
        };

        // Ordered sources: the canonical URL first, then failover mirrors.
        let mut sources: Vec<&str> = Vec::with_capacity(1 + req.mirrors.len());
        sources.push(req.resolved_url);
        sources.extend_from_slice(req.mirrors);

        let mut last_err: Option<Error> = None;
        for src in sources {
            let scheme = scheme_of(src)?.to_ascii_lowercase();
            let Some(transport) = self.transports.get(&scheme) else {
                last_err = Some(Error::UnknownScheme {
                    scheme,
                    url: src.to_string(),
                });
                continue;
            };
            let staging = self.store.new_staging()?;
            match transport.fetch(src, staging.path(), &conditional, auth.as_deref()) {
                Ok(result) => {
                    return self.commit_result(key, req, src, result, prior.as_ref(), staging);
                }
                Err(e) => {
                    last_err = Some(e);
                    // staging drops here → the partial file is removed; try the
                    // next mirror.
                }
            }
        }
        Err(Error::AllMirrorsFailed {
            url: req.resolved_url.to_string(),
            detail: last_err
                .map(|e| e.to_string())
                .unwrap_or_else(|| "no sources".to_string()),
        })
    }

    fn commit_result(
        &self,
        key: &str,
        req: &FetchRequest,
        chosen_url: &str,
        result: FetchResult,
        prior: Option<&Manifest>,
        staging: StagingFile,
    ) -> Result<CachedBlob> {
        match result.status {
            FetchStatus::NotModified => {
                // 304: the existing blob is still valid. Refresh fetched_at +
                // validators; the staging file drops unused.
                let path = self.store.get_blob(key).ok_or_else(|| Error::Integrity {
                    key: key.to_string(),
                    detail: "304 Not Modified but no cached blob present".to_string(),
                })?;
                let mut manifest = prior.cloned().ok_or_else(|| Error::Integrity {
                    key: key.to_string(),
                    detail: "304 Not Modified with no prior manifest".to_string(),
                })?;
                manifest.fetched_at = now_rfc3339();
                if result.etag.is_some() {
                    manifest.etag = result.etag.clone();
                }
                if result.last_modified.is_some() {
                    manifest.last_modified = result.last_modified.clone();
                }
                self.store.put_meta(key, &manifest)?;
                Ok(CachedBlob {
                    key: key.to_string(),
                    path,
                    manifest,
                })
            }
            FetchStatus::Downloaded => {
                let staged_path = staging.path().to_path_buf();
                let bytes = std::fs::metadata(&staged_path)
                    .map_err(|e| Error::io(Some(staged_path.clone()), e))?
                    .len();
                let sha = sha256_file(&staged_path)
                    .map_err(|e| Error::io(Some(staged_path.clone()), e))?;

                // Loader-declared checksum (none today) is verified before commit.
                if let Some(expected) = req.expected_checksum {
                    if !expected.eq_ignore_ascii_case(&sha) {
                        return Err(Error::Integrity {
                            key: key.to_string(),
                            detail: format!("declared checksum {expected} != downloaded {sha}"),
                        });
                    }
                }

                let ext = ext_from_url(chosen_url);
                let path = self.store.commit_blob(key, staging, &ext)?;
                let manifest = Manifest {
                    auth_realm: req.auth_realm.map(str::to_string),
                    bytes,
                    etag: result.etag.clone(),
                    fetched_at: now_rfc3339(),
                    last_modified: result.last_modified.clone(),
                    schema: MANIFEST_SCHEMA.to_string(),
                    sha256_content: sha,
                    source_loader: req.source_loader.map(str::to_string),
                    // The manifest records the *canonical* URL, never a mirror.
                    url: req.resolved_url.to_string(),
                };
                self.store.put_meta(key, &manifest)?;
                Ok(CachedBlob {
                    key: key.to_string(),
                    path,
                    manifest,
                })
            }
        }
    }

    fn verify_integrity(&self, key: &str, blob: &Path, manifest: &Manifest) -> Result<()> {
        let len = std::fs::metadata(blob)
            .map_err(|e| Error::io(Some(blob.to_path_buf()), e))?
            .len();
        if len != manifest.bytes {
            return Err(Error::Integrity {
                key: key.to_string(),
                detail: format!("on-disk size {len} != manifest.bytes {}", manifest.bytes),
            });
        }
        let sha = sha256_file(blob).map_err(|e| Error::io(Some(blob.to_path_buf()), e))?;
        if !sha.eq_ignore_ascii_case(&manifest.sha256_content) {
            return Err(Error::Integrity {
                key: key.to_string(),
                detail: format!(
                    "sha256 {sha} != manifest.sha256_content {}",
                    manifest.sha256_content
                ),
            });
        }
        Ok(())
    }
}

/// Extract the URL scheme (the part before `://`).
fn scheme_of(url: &str) -> Result<&str> {
    match url.split_once("://") {
        Some((scheme, _)) if !scheme.is_empty() => Ok(scheme),
        _ => Err(Error::BadUrl {
            url: url.to_string(),
            detail: "missing scheme".to_string(),
        }),
    }
}

/// Pick a debug-only extension from a URL (query/fragment stripped). Empty when
/// there is no clean alphanumeric suffix — lookups never depend on it.
fn ext_from_url(url: &str) -> String {
    let path = url.split(['?', '#']).next().unwrap_or(url);
    let last = path.rsplit('/').next().unwrap_or("");
    match last.rsplit_once('.') {
        Some((stem, ext))
            if !stem.is_empty()
                && !ext.is_empty()
                && ext.len() <= 8
                && ext.chars().all(|c| c.is_ascii_alphanumeric()) =>
        {
            ext.to_ascii_lowercase()
        }
        _ => String::new(),
    }
}

/// Builder for [`Cache`].
pub struct CacheBuilder {
    data_dir: Option<PathBuf>,
    store_name: String,
    stores: StoreRegistry,
    user_transports: Vec<Arc<dyn Transport>>,
    builtin_transports: bool,
    auth: AuthRegistry,
    offline: Option<bool>,
    verify_on_read: bool,
}

impl CacheBuilder {
    /// A builder with the built-in transports (`http`/`https`/`file`, added only
    /// when online), the `local` store selected, and offline + data-dir resolved
    /// from the environment at `build()`.
    pub fn new() -> Self {
        Self {
            data_dir: None,
            store_name: "local".to_string(),
            stores: StoreRegistry::new(),
            user_transports: Vec::new(),
            builtin_transports: true,
            auth: AuthRegistry::new(),
            offline: None,
            verify_on_read: false,
        }
    }

    /// Override the cache root (otherwise `$EARTHSCIDATADIR`). Used to point at
    /// the conformance corpus.
    pub fn data_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.data_dir = Some(dir.into());
        self
    }

    /// Force offline (or online), overriding `EARTHSCI_OFFLINE`.
    pub fn offline(mut self, offline: bool) -> Self {
        self.offline = Some(offline);
        self
    }

    /// Re-verify `sha256_content` on read (off by default, on for CI/conformance).
    pub fn verify_on_read(mut self, verify: bool) -> Self {
        self.verify_on_read = verify;
        self
    }

    /// Select the store backend by registry name (default `local`).
    pub fn store(mut self, name: impl Into<String>) -> Self {
        self.store_name = name.into();
        self
    }

    /// Register an additional store backend.
    pub fn register_store(mut self, store: Arc<dyn Store>) -> Self {
        self.stores.register(store);
        self
    }

    /// Register an additional transport (e.g. a test transport, or future `s3`).
    /// User transports can override a built-in scheme.
    pub fn register_transport(mut self, transport: Arc<dyn Transport>) -> Self {
        self.user_transports.push(transport);
        self
    }

    /// Omit the built-in `http`/`https`/`file` transports (e.g. a fully custom
    /// transport set, or an offline-only deployment that never fetches).
    pub fn without_builtin_transports(mut self) -> Self {
        self.builtin_transports = false;
        self
    }

    /// Register an auth resolver for a realm.
    pub fn register_auth(mut self, resolver: Arc<dyn AuthResolver>) -> Self {
        self.auth.register(resolver);
        self
    }

    /// Build the cache, resolving the data dir + offline flag from the
    /// environment where not set explicitly.
    pub fn build(mut self) -> Result<Cache> {
        let root = self.data_dir.clone().unwrap_or_else(datadir::data_dir);
        // Provide the default local store if the caller didn't register one.
        if self.stores.get("local").is_none() {
            self.stores.register(Arc::new(LocalStore::new(root)));
        }
        let store = self
            .stores
            .get(&self.store_name)
            .ok_or_else(|| Error::UnknownStore {
                name: self.store_name.clone(),
            })?;
        let offline = offline::is_offline(self.offline);

        // Offline never consults a transport (`spec/offline-mode.md` §2): build
        // none — in particular the reqwest client is not constructed offline.
        let mut transports = TransportRegistry::new();
        if !offline {
            if self.builtin_transports {
                transports.register(Arc::new(HttpTransport::new()));
                transports.register(Arc::new(FileTransport::new()));
                transports.register(Arc::new(CdsTransport::new()));
            }
            for transport in self.user_transports {
                transports.register(transport);
            }
        }

        Ok(Cache {
            store,
            transports,
            auth: self.auth,
            offline,
            verify_on_read: self.verify_on_read,
        })
    }
}

impl Default for CacheBuilder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scheme_parsing() {
        assert_eq!(scheme_of("https://x/y").unwrap(), "https");
        assert_eq!(scheme_of("file:///a/b").unwrap(), "file");
        assert_eq!(scheme_of("s3://bucket/key").unwrap(), "s3");
        assert!(scheme_of("no-scheme").is_err());
    }

    #[test]
    fn ext_extraction() {
        assert_eq!(ext_from_url("https://x/y/20181108.nc"), "nc");
        assert_eq!(ext_from_url("https://x/y.csv?token=abc"), "csv");
        assert_eq!(ext_from_url("https://x/y/data.NC4"), "nc4");
        assert_eq!(ext_from_url("https://x/y/no-extension"), "");
        assert_eq!(ext_from_url("https://x/.hidden"), ""); // empty stem ⇒ no ext
    }

    #[test]
    fn unknown_store_is_an_error() {
        let result = Cache::builder().store("nope").offline(true).build();
        assert!(matches!(result, Err(Error::UnknownStore { .. })));
    }
}
