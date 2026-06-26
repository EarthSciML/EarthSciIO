//! Error type for the EarthSciIO cache/transport/store machinery.

use std::path::PathBuf;

/// Convenience alias for fallible EarthSciIO operations.
pub type Result<T> = std::result::Result<T, Error>;

/// Errors raised by the cache, transports, and stores.
///
/// `CacheMiss` is load-bearing for the offline contract: it carries both the
/// resolved URL and its cache key so a failure names exactly which blob the
/// corpus/cache is missing (see `spec/offline-mode.md` §2).
#[derive(Debug)]
#[non_exhaustive]
pub enum Error {
    /// A cache-only read (offline mode, or an offline store) found no blob for
    /// the resolved URL's key. Never a silent empty result, never a fallback
    /// fetch (`spec/offline-mode.md` §2).
    CacheMiss {
        /// The resolved URL whose blob is absent.
        url: String,
        /// The cache key (`sha256(url)`) that was looked up.
        key: String,
    },

    /// An on-disk blob failed its stored `sha256_content` or byte-length check.
    Integrity {
        /// The cache key of the failing blob.
        key: String,
        /// What mismatched (size or hash, with both values).
        detail: String,
    },

    /// No transport is registered for a URL scheme — a registration gap, not a
    /// Provider change (`spec/registries.md` §1).
    UnknownScheme {
        /// The unhandled URL scheme.
        scheme: String,
        /// The URL whose scheme had no transport.
        url: String,
    },

    /// No store is registered under the configured name (`spec/registries.md` §3).
    UnknownStore {
        /// The configured store name that was not found.
        name: String,
    },

    /// A resolved URL could not be parsed or had no usable scheme.
    BadUrl {
        /// The offending URL.
        url: String,
        /// Why it could not be used.
        detail: String,
    },

    /// A transport-level failure (network error, non-success HTTP status, a
    /// missing local `file://` source, …).
    Transport {
        /// The URL being fetched.
        url: String,
        /// The underlying transport failure.
        detail: String,
    },

    /// Every source (the primary URL plus any failover mirrors) failed.
    AllMirrorsFailed {
        /// The canonical resolved URL.
        url: String,
        /// The last underlying failure encountered.
        detail: String,
    },

    /// An authenticated realm was requested but no resolver is registered for it.
    MissingAuth {
        /// The realm with no registered resolver.
        realm: String,
    },

    /// An I/O error, tagged with the path that triggered it when known.
    Io {
        /// The path the I/O error concerns, if known.
        path: Option<PathBuf>,
        /// The underlying I/O error.
        source: std::io::Error,
    },

    /// Manifest JSON could not be (de)serialized.
    Manifest {
        /// The (de)serialization failure detail.
        detail: String,
    },
}

impl Error {
    /// Wrap an I/O error together with the path it concerns.
    pub fn io(path: Option<PathBuf>, source: std::io::Error) -> Self {
        Error::Io { path, source }
    }

    /// True when this is a [`Error::CacheMiss`] — the offline "absent" signal.
    pub fn is_cache_miss(&self) -> bool {
        matches!(self, Error::CacheMiss { .. })
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::CacheMiss { url, key } => {
                write!(f, "cache miss: no blob for key {key} (url {url})")
            }
            Error::Integrity { key, detail } => {
                write!(f, "integrity failure for key {key}: {detail}")
            }
            Error::UnknownScheme { scheme, url } => {
                write!(
                    f,
                    "no transport registered for scheme '{scheme}' (url {url})"
                )
            }
            Error::UnknownStore { name } => write!(f, "no store registered as '{name}'"),
            Error::BadUrl { url, detail } => write!(f, "bad url '{url}': {detail}"),
            Error::Transport { url, detail } => write!(f, "transport error for {url}: {detail}"),
            Error::AllMirrorsFailed { url, detail } => {
                write!(f, "all sources failed for {url}: {detail}")
            }
            Error::MissingAuth { realm } => {
                write!(f, "no auth resolver registered for realm '{realm}'")
            }
            Error::Io { path, source } => match path {
                Some(p) => write!(f, "io error at {}: {source}", p.display()),
                None => write!(f, "io error: {source}"),
            },
            Error::Manifest { detail } => write!(f, "manifest error: {detail}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

impl From<std::io::Error> for Error {
    fn from(source: std::io::Error) -> Self {
        Error::Io { path: None, source }
    }
}
