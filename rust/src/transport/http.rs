//! The `http` transport: GET + conditional GET over HTTP(S) via reqwest
//! (`spec/registries.md` §1). Mirror failover is the caller's job (the cache),
//! so this transport handles exactly one URL.

use std::io::Write;
use std::path::Path;

use reqwest::blocking::Client;
use reqwest::header::{
    HeaderMap, HeaderName, HeaderValue, ETAG, IF_MODIFIED_SINCE, IF_NONE_MATCH, LAST_MODIFIED,
};
use reqwest::StatusCode;

use super::{Conditional, FetchResult, FetchStatus, Transport};
use crate::auth::AuthResolver;
use crate::error::{Error, Result};

/// HTTP(S) transport. The `rustls-tls` / ring backend means `https` works
/// without a system OpenSSL.
pub struct HttpTransport {
    client: Client,
}

impl HttpTransport {
    /// Construct with a default blocking client.
    pub fn new() -> Self {
        let client = Client::builder()
            .user_agent(concat!("earthsciio/", env!("CARGO_PKG_VERSION")))
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
            return Ok(FetchResult {
                status: FetchStatus::NotModified,
                etag: conditional.etag.clone(),
                last_modified: conditional.last_modified.clone(),
                bytes_written: 0,
            });
        }
        if !status.is_success() {
            return Err(Error::Transport {
                url: url.to_string(),
                detail: format!("HTTP {}", status.as_u16()),
            });
        }

        // Capture validators before consuming the body.
        let etag = header_string(resp.headers(), &ETAG);
        let last_modified = header_string(resp.headers(), &LAST_MODIFIED);

        // Stream the body straight to the staging file (no full-body buffering).
        let mut file =
            std::fs::File::create(dest).map_err(|e| Error::io(Some(dest.to_path_buf()), e))?;
        let bytes_written = resp.copy_to(&mut file).map_err(|e| Error::Transport {
            url: url.to_string(),
            detail: e.to_string(),
        })?;
        file.flush()
            .map_err(|e| Error::io(Some(dest.to_path_buf()), e))?;

        Ok(FetchResult {
            status: FetchStatus::Downloaded,
            etag,
            last_modified,
            bytes_written,
        })
    }
}

fn header_string(headers: &HeaderMap, name: &HeaderName) -> Option<String> {
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .map(String::from)
}
