//! The `http` transport: GET + conditional GET over HTTP(S) via reqwest
//! (`spec/registries.md` §1). Mirror failover is the caller's job (the cache),
//! so this transport handles exactly one URL.

use std::io::Write;
use std::path::Path;
use std::time::Duration;

use reqwest::blocking::Client;
use reqwest::header::{
    HeaderMap, HeaderName, HeaderValue, ETAG, IF_MODIFIED_SINCE, IF_NONE_MATCH, LAST_MODIFIED,
};
use reqwest::StatusCode;

use super::{Conditional, FetchResult, FetchStatus, Transport};
use crate::auth::AuthResolver;
use crate::error::{Error, Result};
use crate::key::Sha256Writer;

/// `$var` as whole seconds, or `default_secs` when unset or unparsable.
fn env_secs(var: &str, default_secs: u64) -> Duration {
    std::env::var(var)
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .map_or(Duration::from_secs(default_secs), Duration::from_secs)
}

/// HTTP(S) transport. The `rustls-tls` / ring backend means `https` works
/// without a system OpenSSL.
pub struct HttpTransport {
    client: Client,
}

impl HttpTransport {
    /// Construct with a default blocking client.
    ///
    /// A stalled connection must fail rather than hang the run, but this
    /// reqwest's blocking builder has no per-read timeout, and a tight total
    /// deadline would kill slow-but-progressing large downloads (record files
    /// run 100+ MB) — reqwest's own blocking default of 30 s total did exactly
    /// that. So: 30 s to connect, a generous 600 s per request, overridable via
    /// `EARTHSCIIO_HTTP_CONNECT_TIMEOUT_SECS` /
    /// `EARTHSCIIO_HTTP_READ_TIMEOUT_SECS` (whole seconds).
    pub fn new() -> Self {
        let client = Client::builder()
            .user_agent(concat!("earthsciio/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(env_secs("EARTHSCIIO_HTTP_CONNECT_TIMEOUT_SECS", 30))
            .timeout(env_secs("EARTHSCIIO_HTTP_READ_TIMEOUT_SECS", 600))
            .build()
            .expect("default reqwest blocking client builds");
        Self { client }
    }

    /// Construct from a caller-provided client (timeouts, proxies, …).
    pub fn with_client(client: Client) -> Self {
        Self { client }
    }
}

impl Default for HttpTransport {
    fn default() -> Self {
        Self::new()
    }
}

impl Transport for HttpTransport {
    fn schemes(&self) -> &'static [&'static str] {
        &["http", "https"]
    }

    fn fetch(
        &self,
        url: &str,
        dest: &Path,
        conditional: &Conditional,
        auth: Option<&dyn AuthResolver>,
    ) -> Result<FetchResult> {
        self.fetch_hashed(url, dest, conditional, auth)
            .map(|(result, _)| result)
    }

    fn fetch_hashed(
        &self,
        url: &str,
        dest: &Path,
        conditional: &Conditional,
        auth: Option<&dyn AuthResolver>,
    ) -> Result<(FetchResult, Option<String>)> {
        let mut headers = HeaderMap::new();
        if let Some(etag) = &conditional.etag {
            if let Ok(v) = HeaderValue::from_str(etag) {
                headers.insert(IF_NONE_MATCH, v);
            }
        }
        if let Some(lm) = &conditional.last_modified {
            if let Ok(v) = HeaderValue::from_str(lm) {
                headers.insert(IF_MODIFIED_SINCE, v);
            }
        }
        if let Some(resolver) = auth {
            for (name, value) in resolver.headers() {
                match (
                    HeaderName::from_bytes(name.as_bytes()),
                    HeaderValue::from_str(&value),
                ) {
                    (Ok(n), Ok(v)) => {
                        headers.insert(n, v);
                    }
                    _ => {
                        return Err(Error::Transport {
                            url: url.to_string(),
                            detail: format!("invalid auth header '{name}'"),
                        })
                    }
                }
            }
        }

        let mut resp =
            self.client
                .get(url)
                .headers(headers)
                .send()
                .map_err(|e| Error::Transport {
                    url: url.to_string(),
                    detail: e.to_string(),
                })?;

        let status = resp.status();
        if status == StatusCode::NOT_MODIFIED {
            // Cached blob is still valid; staging stays empty.
            return Ok((
                FetchResult {
                    status: FetchStatus::NotModified,
                    etag: conditional.etag.clone(),
                    last_modified: conditional.last_modified.clone(),
                    bytes_written: 0,
                },
                None,
            ));
        }
        if !status.is_success() {
            // 404/410 are a DEFINITIVE absence — the store answered, and the
            // answer is "no such object". Everything else (5xx, 403, a timeout)
            // leaves existence UNKNOWN and must stay a hard error.
            if status == StatusCode::NOT_FOUND || status == StatusCode::GONE {
                return Err(Error::NotFound {
                    url: url.to_string(),
                    detail: format!("HTTP {}", status.as_u16()),
                });
            }
            return Err(Error::Transport {
                url: url.to_string(),
                detail: format!("HTTP {}", status.as_u16()),
            });
        }

        // Capture validators before consuming the body.
        let etag = header_string(resp.headers(), &ETAG);
        let last_modified = header_string(resp.headers(), &LAST_MODIFIED);

        // Stream the body straight to the staging file (no full-body buffering),
        // hashing the bytes in transit so the cache commit needs no second full
        // read of the file.
        let file =
            std::fs::File::create(dest).map_err(|e| Error::io(Some(dest.to_path_buf()), e))?;
        let mut writer = Sha256Writer::new(file);
        let bytes_written = resp.copy_to(&mut writer).map_err(|e| Error::Transport {
            url: url.to_string(),
            detail: e.to_string(),
        })?;
        writer
            .flush()
            .map_err(|e| Error::io(Some(dest.to_path_buf()), e))?;
        let sha = writer.finalize();

        Ok((
            FetchResult {
                status: FetchStatus::Downloaded,
                etag,
                last_modified,
                bytes_written,
            },
            Some(sha),
        ))
    }
}

fn header_string(headers: &HeaderMap, name: &HeaderName) -> Option<String> {
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .map(String::from)
}
