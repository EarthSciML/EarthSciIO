//! The `transport` registry (`spec/registries.md` §1).
//!
//! Keyed by URL scheme. A transport fetches a resolved URL's bytes into a
//! `tmp/<uuid>.part` staging path. Transports are **bypassed entirely in
//! offline mode** — the cache never constructs or calls one when `offline`.

mod file;
mod http;

pub use file::FileTransport;
pub use http::HttpTransport;

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use crate::auth::AuthResolver;
use crate::error::Result;

/// Conditional-GET validators carried from a prior manifest into a fetch. Empty
/// when there is nothing cached to revalidate against.
#[derive(Debug, Default, Clone)]
pub struct Conditional {
    /// Stored ETag, sent as `If-None-Match`.
    pub etag: Option<String>,
    /// Stored Last-Modified, sent as `If-Modified-Since`.
    pub last_modified: Option<String>,
}

impl Conditional {
    /// True when neither validator is present.
    pub fn is_empty(&self) -> bool {
        self.etag.is_none() && self.last_modified.is_none()
    }
}

/// Whether a fetch produced new bytes or revalidated an existing blob.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FetchStatus {
    /// New bytes were written to the staging path.
    Downloaded,
    /// `304 Not Modified` — the cached blob is still valid; staging is untouched.
    NotModified,
}

/// The outcome of a transport fetch.
#[derive(Debug, Clone)]
pub struct FetchResult {
    /// Whether new bytes were downloaded or the cached blob was revalidated.
    pub status: FetchStatus,
    /// ETag from the response, to persist into the manifest.
    pub etag: Option<String>,
    /// Last-Modified from the response, to persist into the manifest.
    pub last_modified: Option<String>,
    /// Bytes written to the staging path (`0` for `NotModified`).
    pub bytes_written: u64,
}

/// Fetches a resolved URL's bytes into the cache. Keyed by URL scheme; never
/// constructed in offline mode.
pub trait Transport: Send + Sync {
    /// URL schemes this transport handles (e.g. `["http", "https"]`).
    fn schemes(&self) -> &'static [&'static str];

    /// Download `resolved_url` into `dest` (a staging path on the cache
    /// filesystem), honoring conditional validators and optional auth.
    fn fetch(
        &self,
        resolved_url: &str,
        dest: &Path,
        conditional: &Conditional,
        auth: Option<&dyn AuthResolver>,
    ) -> Result<FetchResult>;
}

/// Scheme → transport lookup. Adding a scheme is a registration, never a
/// Provider edit (`spec/registries.md` §1).
#[derive(Default, Clone)]
pub struct TransportRegistry {
    by_scheme: HashMap<String, Arc<dyn Transport>>,
}

impl TransportRegistry {
    /// An empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Registry with the built-in **active** transports: `http`/`https` + `file`.
    pub fn with_builtins() -> Self {
        let mut r = Self::new();
        r.register(Arc::new(HttpTransport::new()));
        r.register(Arc::new(FileTransport::new()));
        r
    }

    /// Register a transport under each of its schemes.
    pub fn register(&mut self, transport: Arc<dyn Transport>) -> &mut Self {
        for scheme in transport.schemes() {
            self.by_scheme
                .insert((*scheme).to_string(), transport.clone());
        }
        self
    }

    /// Look up the transport for a URL scheme.
    pub fn get(&self, scheme: &str) -> Option<Arc<dyn Transport>> {
        self.by_scheme.get(scheme).cloned()
    }

    /// All registered schemes.
    pub fn schemes(&self) -> Vec<String> {
        self.by_scheme.keys().cloned().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtins_cover_http_https_file() {
        let r = TransportRegistry::with_builtins();
        assert!(r.get("http").is_some());
        assert!(r.get("https").is_some());
        assert!(r.get("file").is_some());
        assert!(r.get("s3").is_none()); // stub, not active here
    }
}
