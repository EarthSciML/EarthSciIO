//! The `s3` transport — an anonymous `s3://` → regional-HTTPS URL rewriter over
//! the [`HttpTransport`] (`spec/registries.md` §1).
//!
//! The canonical resolved URL stays `s3://<bucket>/<key…>` (kept verbatim in the
//! cache key + `manifest.url`, exactly like `cds://`). [`S3Transport::fetch`]
//! rewrites it to **virtual-hosted HTTPS** —
//! `https://<bucket>.s3.<region>.amazonaws.com/<key>` — and delegates a plain
//! **anonymous** GET to a held `HttpTransport`. A public bucket needs **no AWS
//! SDK, no SigV4, no credentials**; streaming, conditional GET (S3 returns
//! ETags), redirect following, and mirror failover all come from the HTTP
//! delegate. Region defaults to `us-east-2` (the pinned InMAP ISRM bucket),
//! overridable via `$EARTHSCI_S3_REGION` (fallback `$AWS_REGION`) or
//! [`S3Transport::with_region`]. The `auth` resolver threads through unchanged so
//! a future SigV4/requester-pays resolver plugs in with no transport edit.

use std::path::Path;

use super::{Conditional, FetchResult, HttpTransport, Transport};
use crate::auth::AuthResolver;
use crate::error::{Error, Result};

/// Default region — the pinned InMAP ISRM bucket lives in `us-east-2`.
pub const DEFAULT_S3_REGION: &str = "us-east-2";

/// Resolve the S3 region: explicit arg → `$EARTHSCI_S3_REGION` → `$AWS_REGION` →
/// [`DEFAULT_S3_REGION`].
pub fn resolve_region(explicit: Option<&str>) -> String {
    if let Some(r) = explicit {
        return r.to_string();
    }
    for var in ["EARTHSCI_S3_REGION", "AWS_REGION"] {
        if let Ok(v) = std::env::var(var) {
            if !v.is_empty() {
                return v;
            }
        }
    }
    DEFAULT_S3_REGION.to_string()
}

/// Rewrite `s3://<bucket>/<key…>` to regional virtual-hosted HTTPS.
pub fn s3_https_url(s3_url: &str, region: &str) -> Result<String> {
    let bad = |detail: String| Error::BadUrl {
        url: s3_url.to_string(),
        detail,
    };
    let rest = s3_url
        .strip_prefix("s3://")
        .ok_or_else(|| bad("not an s3:// URL".to_string()))?;
    let (bucket, key) = rest
        .split_once('/')
        .ok_or_else(|| bad("s3:// URL has no object key".to_string()))?;
    if bucket.is_empty() {
        return Err(bad("s3:// URL has an empty bucket".to_string()));
    }
    Ok(format!("https://{bucket}.s3.{region}.amazonaws.com/{key}"))
}

/// Anonymous `s3://` transport: rewrite to regional HTTPS, delegate to HTTP.
pub struct S3Transport {
    http: HttpTransport,
    region: Option<String>,
}

impl S3Transport {
    /// A transport resolving the region from the environment (default us-east-2).
    pub fn new() -> Self {
        Self {
            http: HttpTransport::new(),
            region: None,
        }
    }

    /// A transport pinned to `region` (overrides the environment).
    pub fn with_region(region: impl Into<String>) -> Self {
        Self {
            http: HttpTransport::new(),
            region: Some(region.into()),
        }
    }
}

impl Default for S3Transport {
    fn default() -> Self {
        Self::new()
    }
}

impl Transport for S3Transport {
    fn schemes(&self) -> &'static [&'static str] {
        &["s3"]
    }

    fn fetch(
        &self,
        url: &str,
        dest: &Path,
        conditional: &Conditional,
        auth: Option<&dyn AuthResolver>,
    ) -> Result<FetchResult> {
        let region = resolve_region(self.region.as_deref());
        let https = s3_https_url(url, &region)?;
        self.http.fetch(&https, dest, conditional, auth)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scheme_is_s3() {
        assert_eq!(S3Transport::new().schemes(), &["s3"]);
    }

    #[test]
    fn rewrite_default_and_explicit_region() {
        assert_eq!(
            s3_https_url(
                "s3://inmap-model/isrm_v1.2.1.zarr/PrimaryPM25/0.5.0",
                "us-east-2"
            )
            .unwrap(),
            "https://inmap-model.s3.us-east-2.amazonaws.com/isrm_v1.2.1.zarr/PrimaryPM25/0.5.0"
        );
        assert_eq!(
            s3_https_url("s3://b/k/o", "eu-west-1").unwrap(),
            "https://b.s3.eu-west-1.amazonaws.com/k/o"
        );
    }

    #[test]
    fn rewrite_rejects_bad_urls() {
        assert!(s3_https_url("https://x/y", "us-east-2").is_err());
        assert!(s3_https_url("s3://bucket-only", "us-east-2").is_err());
        assert!(s3_https_url("s3:///key", "us-east-2").is_err());
    }
}
