//! The per-blob manifest, `meta/<key>.json` (`spec/cache-format.md` §3).
//!
//! Every cached blob has a sibling manifest carrying its validation +
//! provenance state. The schema is identical across Python / Julia / Rust so a
//! blob fetched by one language is reused (and re-validated) by the others.
//!
//! Field order here is **alphabetical**, and serialization uses
//! `serde_json`'s 2-space pretty form plus a trailing newline. That makes the
//! bytes identical to the Python writer (`json.dumps(..., indent=2,
//! sort_keys=True) + "\n"`), keeping cross-language manifests diff-clean.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// Manifest schema tag, bumped with the cache-format version.
pub const MANIFEST_SCHEMA: &str = "earthsciio/manifest/v1";

/// Validation + provenance record stored alongside every cached blob.
///
/// `url`, `sha256_content`, `bytes`, and `fetched_at` are required; the
/// remaining fields are always present but may be `null`. Credentials are
/// **never** stored — only the `auth_realm` name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    /// Auth realm used (e.g. `cds`), or `null`. NEVER credentials.
    pub auth_realm: Option<String>,
    /// Blob size in bytes; MUST equal the on-disk blob length.
    pub bytes: u64,
    /// HTTP ETag from the response, for conditional GET (`If-None-Match`).
    pub etag: Option<String>,
    /// RFC 3339 UTC timestamp the blob was fetched.
    pub fetched_at: String,
    /// HTTP Last-Modified, for `If-Modified-Since`.
    pub last_modified: Option<String>,
    /// Manifest schema tag.
    #[serde(default = "default_schema")]
    pub schema: String,
    /// sha256 of the blob bytes (self-pinned integrity hash), lowercase hex.
    pub sha256_content: String,
    /// `.esm` loader that resolved this URL (debug / provenance).
    pub source_loader: Option<String>,
    /// The resolved source URL whose sha256 is this blob's cache key.
    pub url: String,
}

fn default_schema() -> String {
    MANIFEST_SCHEMA.to_string()
}

impl Manifest {
    /// Serialize to bytes byte-identical to the Python writer: 2-space pretty
    /// JSON with a trailing newline.
    pub fn to_json_bytes(&self) -> Result<Vec<u8>> {
        let mut s = serde_json::to_string_pretty(self).map_err(|e| Error::Manifest {
            detail: e.to_string(),
        })?;
        s.push('\n');
        Ok(s.into_bytes())
    }

    /// Parse a manifest from JSON bytes.
    pub fn from_json_bytes(data: &[u8]) -> Result<Manifest> {
        serde_json::from_slice(data).map_err(|e| Error::Manifest {
            detail: e.to_string(),
        })
    }

    /// Read and parse a manifest file.
    pub fn read(path: &Path) -> Result<Manifest> {
        let data = std::fs::read(path).map_err(|e| Error::io(Some(path.to_path_buf()), e))?;
        Manifest::from_json_bytes(&data)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // The exact bytes of corpus meta for the era5 fixture
    // (conformance/corpus/cache/v1/meta/11cd….json). Reading + re-writing it
    // must round-trip byte-for-byte.
    const ERA5_META: &str = "{\n  \"auth_realm\": null,\n  \"bytes\": 1012,\n  \"etag\": null,\n  \"fetched_at\": \"2026-06-26T00:00:00Z\",\n  \"last_modified\": null,\n  \"schema\": \"earthsciio/manifest/v1\",\n  \"sha256_content\": \"fe1f8040ebd8688a4a16e80ef694a37ae2b1deb21b449eebfa90745cab95d3af\",\n  \"source_loader\": \"era5\",\n  \"url\": \"https://data.earthsci.dev/era5/2018/11/20181108.nc\"\n}\n";

    #[test]
    fn parses_corpus_manifest() {
        let m = Manifest::from_json_bytes(ERA5_META.as_bytes()).unwrap();
        assert_eq!(m.url, "https://data.earthsci.dev/era5/2018/11/20181108.nc");
        assert_eq!(m.bytes, 1012);
        assert_eq!(
            m.sha256_content,
            "fe1f8040ebd8688a4a16e80ef694a37ae2b1deb21b449eebfa90745cab95d3af"
        );
        assert_eq!(m.source_loader.as_deref(), Some("era5"));
        assert_eq!(m.etag, None);
        assert_eq!(m.auth_realm, None);
        assert_eq!(m.schema, MANIFEST_SCHEMA);
    }

    #[test]
    fn serializes_byte_identical_to_python() {
        // The whole point of the alphabetical field order + trailing newline:
        // our writer reproduces the committed corpus bytes exactly.
        let m = Manifest::from_json_bytes(ERA5_META.as_bytes()).unwrap();
        let bytes = m.to_json_bytes().unwrap();
        assert_eq!(String::from_utf8(bytes).unwrap(), ERA5_META);
    }

    #[test]
    fn schema_defaults_when_absent() {
        // schema is optional on read (not in the required set).
        let json = "{\"url\":\"u\",\"sha256_content\":\"ab\",\"bytes\":2,\"fetched_at\":\"t\",\"etag\":null,\"last_modified\":null,\"source_loader\":null,\"auth_realm\":null}";
        let m = Manifest::from_json_bytes(json.as_bytes()).unwrap();
        assert_eq!(m.schema, MANIFEST_SCHEMA);
    }
}
