//! The `cds` transport: the Copernicus Climate Data Store (CDS) API v1
//! (`spec/registries.md` §1). Ported from EarthSciData.jl's `cds_api.jl`.
//!
//! CDS is not a plain byte fetch. A `cds://<dataset>?<request-json>` resolved URL
//! is **submitted** (`POST .../processes/<dataset>/execution`), the returned job
//! is **polled** (`GET .../jobs/<id>`) until it succeeds, the job's **results**
//! (`GET .../jobs/<id>/results`) yield an asset `href`, and that href is finally
//! **downloaded** into the cache staging path. The whole submit→poll→download
//! dance is one [`Transport::fetch`], so the content-addressed cache treats CDS
//! exactly like any other source: same key (`sha256(resolved_url)`), same
//! skip-if-exists, same offline replay.
//!
//! Auth is the pluggable `cds` realm: a `PRIVATE-TOKEN` header carrying the key
//! from `~/.cdsapirc` or `$CDSAPI_KEY`, injected as the fetch `auth` — never
//! baked into the transport (`spec/registries.md` §1).

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use reqwest::blocking::Client;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue, CONTENT_TYPE};

use super::{Conditional, FetchResult, FetchStatus, Transport};
use crate::auth::{AuthResolver, StaticHeaderAuth};
use crate::error::{Error, Result};

/// The production CDS API v1 endpoint.
pub const CDS_API_URL: &str = "https://cds.climate.copernicus.eu/api";

/// The auth realm CDS fetches authenticate under (the `PRIVATE-TOKEN` header).
pub const CDS_REALM: &str = "cds";

/// Default delay between job-status polls (mirrors `cds_api.jl`'s 5 s).
const DEFAULT_POLL_INTERVAL: Duration = Duration::from_secs(5);

/// Default budget before a job is abandoned (mirrors `cds_api.jl`'s 600 s).
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(600);

/// The `cds://` transport: submit a CDS request, poll the job, download the
/// asset. Construct via [`CdsTransport::new`] (production endpoint) or
/// [`CdsTransport::with_base_url`] (tests / mirrors), tuning the cadence with
/// [`poll_interval`](Self::poll_interval) / [`timeout`](Self::timeout).
pub struct CdsTransport {
    client: Client,
    base_url: String,
    poll_interval: Duration,
    timeout: Duration,
}

impl CdsTransport {
    /// A transport against the production CDS endpoint with default cadence.
    pub fn new() -> Self {
        Self::with_base_url(CDS_API_URL)
    }

    /// A transport against `base_url` (a trailing slash is trimmed). Used to
    /// point at a mock server in tests, or an alternate CDS-compatible endpoint.
    pub fn with_base_url(base_url: impl Into<String>) -> Self {
        let client = Client::builder()
            .user_agent(concat!("earthsciio/", env!("CARGO_PKG_VERSION")))
            .build()
            .expect("default reqwest blocking client builds");
        Self::with_client(client, base_url)
    }

    /// A transport from a caller-provided client (custom timeouts, proxies, …).
    pub fn with_client(client: Client, base_url: impl Into<String>) -> Self {
        Self {
            client,
            base_url: base_url.into().trim_end_matches('/').to_string(),
            poll_interval: DEFAULT_POLL_INTERVAL,
            timeout: DEFAULT_TIMEOUT,
        }
    }

    /// Set the delay between job-status polls (default 5 s).
    pub fn poll_interval(mut self, d: Duration) -> Self {
        self.poll_interval = d;
        self
    }

    /// Set the budget before a still-running job is abandoned (default 600 s).
    pub fn timeout(mut self, d: Duration) -> Self {
        self.timeout = d;
        self
    }

    /// Submit a retrieve request; returns the job ID. Mirrors `cds_submit`.
    fn submit(
        &self,
        url: &str,
        dataset: &str,
        request_json: &str,
        auth: &HeaderMap,
    ) -> Result<String> {
        let exec_url = format!(
            "{}/retrieve/v1/processes/{dataset}/execution",
            self.base_url
        );
        // `request_json` is already a canonical JSON object; wrap it verbatim so
        // the body matches the cache key's request byte-for-byte.
        let body = format!("{{\"inputs\":{request_json}}}");

        let mut headers = auth.clone();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));

        let resp = self
            .client
            .post(&exec_url)
            .headers(headers)
            .body(body)
            .send()
            .map_err(|e| transport_err(url, e.to_string()))?;
        let status = resp.status();
        let text = resp.text().map_err(|e| transport_err(url, e.to_string()))?;
        if !status.is_success() {
            return Err(transport_err(
                url,
                format!("submit HTTP {}: {text}", status.as_u16()),
            ));
        }

        let data: serde_json::Value = serde_json::from_str(&text)
            .map_err(|e| transport_err(url, format!("submit response not JSON: {e}")))?;
        match data.get("status").and_then(|v| v.as_str()).unwrap_or("") {
            "accepted" | "running" | "successful" => {}
            _ => return Err(transport_err(url, format!("CDS submit rejected: {text}"))),
        }
        let job_id = data
            .get("jobID")
            .and_then(|v| v.as_str())
            .ok_or_else(|| transport_err(url, format!("submit response missing jobID: {text}")))?;
        Ok(job_id.to_string())
    }

    /// Poll a job to completion; returns the download href. Mirrors `cds_wait`.
    fn wait(&self, url: &str, job_id: &str, auth: &HeaderMap) -> Result<String> {
        let job_url = format!("{}/retrieve/v1/jobs/{job_id}", self.base_url);
        let start = Instant::now();
        loop {
            let resp = self
                .client
                .get(&job_url)
                .headers(auth.clone())
                .send()
                .map_err(|e| transport_err(url, e.to_string()))?;
            let status = resp.status();
            let text = resp.text().map_err(|e| transport_err(url, e.to_string()))?;
            if !status.is_success() {
                return Err(transport_err(
                    url,
                    format!("poll HTTP {}: {text}", status.as_u16()),
                ));
            }
            let data: serde_json::Value = serde_json::from_str(&text)
                .map_err(|e| transport_err(url, format!("poll response not JSON: {e}")))?;

            match data.get("status").and_then(|v| v.as_str()).unwrap_or("") {
                "successful" => return self.results_href(url, &job_url, auth),
                "failed" => {
                    return Err(transport_err(
                        url,
                        format!("CDS job {job_id} failed: {text}"),
                    ))
                }
                _ => {
                    if start.elapsed() > self.timeout {
                        return Err(transport_err(
                            url,
                            format!("CDS job {job_id} timed out after {:?}", self.timeout),
                        ));
                    }
                    std::thread::sleep(self.poll_interval);
                }
            }
        }
    }

    /// Fetch the results document and extract `asset.value.href`.
    fn results_href(&self, url: &str, job_url: &str, auth: &HeaderMap) -> Result<String> {
        let results_url = format!("{job_url}/results");
        let resp = self
            .client
            .get(&results_url)
            .headers(auth.clone())
            .send()
            .map_err(|e| transport_err(url, e.to_string()))?;
        let status = resp.status();
        let text = resp.text().map_err(|e| transport_err(url, e.to_string()))?;
        if !status.is_success() {
            return Err(transport_err(
                url,
                format!("results HTTP {}: {text}", status.as_u16()),
            ));
        }
        let data: serde_json::Value = serde_json::from_str(&text)
            .map_err(|e| transport_err(url, format!("results response not JSON: {e}")))?;
        let href = data
            .pointer("/asset/value/href")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                transport_err(url, format!("results missing asset.value.href: {text}"))
            })?;
        Ok(href.to_string())
    }

    /// Stream the asset href into the staging file; returns bytes written.
    fn download(&self, url: &str, href: &str, dest: &Path) -> Result<u64> {
        // The href is a CDS-issued (often pre-signed) URL — downloaded without
        // the PRIVATE-TOKEN, matching `cds_api.jl`'s `_download_with_progress`.
        let mut resp = self
            .client
            .get(href)
            .send()
            .map_err(|e| transport_err(url, e.to_string()))?;
        let status = resp.status();
        if !status.is_success() {
            return Err(transport_err(
                url,
                format!("download HTTP {} from {href}", status.as_u16()),
            ));
        }
        let mut file =
            std::fs::File::create(dest).map_err(|e| Error::io(Some(dest.to_path_buf()), e))?;
        let bytes_written = resp
            .copy_to(&mut file)
            .map_err(|e| transport_err(url, e.to_string()))?;
        file.flush()
            .map_err(|e| Error::io(Some(dest.to_path_buf()), e))?;
        Ok(bytes_written)
    }
}

impl Default for CdsTransport {
    fn default() -> Self {
        Self::new()
    }
}

impl Transport for CdsTransport {
    fn schemes(&self) -> &'static [&'static str] {
        &["cds"]
    }

    fn fetch(
        &self,
        url: &str,
        dest: &Path,
        _conditional: &Conditional,
        auth: Option<&dyn AuthResolver>,
    ) -> Result<FetchResult> {
        // CDS regenerates the asset on every request, so conditional GET does
        // not apply — there is nothing to revalidate against.
        let (dataset, request_json) = parse_cds_url(url)?;
        let headers = auth_headers(url, auth)?;

        let job_id = self.submit(url, &dataset, &request_json, &headers)?;
        let href = self.wait(url, &job_id, &headers)?;
        let bytes_written = self.download(url, &href, dest)?;

        Ok(FetchResult {
            status: FetchStatus::Downloaded,
            etag: None,
            last_modified: None,
            bytes_written,
        })
    }
}

/// Build a `cds://<dataset>?<request-json>` resolved URL. `request_json` must be
/// a **canonical** (deterministic) JSON encoding of the CDS request object so the
/// same logical request always yields the same cache key (`spec/cache-format.md`).
pub fn build_cds_url(dataset: &str, request_json: &str) -> String {
    format!("cds://{dataset}?{request_json}")
}

/// Split a `cds://<dataset>?<request-json>` URL into `(dataset, request_json)`.
///
/// The request is validated as a JSON object so a malformed request fails at the
/// cache boundary with a clear [`Error::BadUrl`], not as an opaque CDS 400 later.
pub fn parse_cds_url(url: &str) -> Result<(String, String)> {
    let bad = |detail: String| Error::BadUrl {
        url: url.to_string(),
        detail,
    };
    let rest = url
        .strip_prefix("cds://")
        .ok_or_else(|| bad("not a cds:// URL".to_string()))?;
    let (dataset, request_json) = rest
        .split_once('?')
        .ok_or_else(|| bad("cds:// URL missing '?<request-json>'".to_string()))?;
    if dataset.is_empty() {
        return Err(bad("cds:// URL has an empty dataset".to_string()));
    }
    if request_json.is_empty() {
        return Err(bad("cds:// URL has an empty request".to_string()));
    }
    let parsed: serde_json::Value = serde_json::from_str(request_json)
        .map_err(|e| bad(format!("cds:// request is not JSON: {e}")))?;
    if !parsed.is_object() {
        return Err(bad("cds:// request must be a JSON object".to_string()));
    }
    Ok((dataset.to_string(), request_json.to_string()))
}

/// Read the CDS API key from `$CDSAPI_KEY` or `~/.cdsapirc` (the `key:` line),
/// mirroring `cds_api.jl`'s `cds_api_key`. Errors when neither is present.
pub fn cds_api_key() -> Result<String> {
    if let Some(k) = std::env::var_os("CDSAPI_KEY") {
        let k = k.to_string_lossy().trim().to_string();
        if !k.is_empty() {
            return Ok(k);
        }
    }
    if let Some(home) = std::env::var_os("HOME") {
        let rc = PathBuf::from(home).join(".cdsapirc");
        if let Some(k) = read_cdsapirc_key(&rc)? {
            return Ok(k);
        }
    }
    Err(Error::Transport {
        url: "cds://".to_string(),
        detail:
            "CDS API key not found: set CDSAPI_KEY or create ~/.cdsapirc with 'key: <your-key>'"
                .to_string(),
    })
}

/// A ready-to-register `cds`-realm [`AuthResolver`] carrying the `PRIVATE-TOKEN`
/// header, with the key resolved from `$CDSAPI_KEY` / `~/.cdsapirc`. Register it
/// with `Cache::builder().register_auth(Arc::new(cds_auth()?))`.
pub fn cds_auth() -> Result<StaticHeaderAuth> {
    Ok(StaticHeaderAuth::header(
        CDS_REALM,
        "PRIVATE-TOKEN",
        cds_api_key()?,
    ))
}

/// Read + parse the `key:` line from a `.cdsapirc` file. Absent file ⇒ `None`.
fn read_cdsapirc_key(path: &Path) -> Result<Option<String>> {
    match std::fs::read_to_string(path) {
        Ok(contents) => Ok(parse_cdsapirc_key(&contents)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(Error::io(Some(path.to_path_buf()), e)),
    }
}

/// Pull the first `key:` value out of a `.cdsapirc` body. Pure + testable.
fn parse_cdsapirc_key(contents: &str) -> Option<String> {
    for line in contents.lines() {
        if let Some(rest) = line.trim_start().strip_prefix("key:") {
            let key = rest.trim();
            if !key.is_empty() {
                return Some(key.to_string());
            }
        }
    }
    None
}

/// Turn an optional resolver into the header map applied to submit + poll. An
/// invalid header name/value is a clean error rather than a silently dropped
/// credential. No resolver ⇒ empty headers (the CDS server then returns 401).
fn auth_headers(url: &str, auth: Option<&dyn AuthResolver>) -> Result<HeaderMap> {
    let mut headers = HeaderMap::new();
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
                    return Err(transport_err(url, format!("invalid auth header '{name}'")));
                }
            }
        }
    }
    Ok(headers)
}

/// A [`Error::Transport`] for `url` with `detail`.
fn transport_err(url: &str, detail: String) -> Error {
    Error::Transport {
        url: url.to_string(),
        detail,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scheme_is_cds() {
        assert_eq!(CdsTransport::new().schemes(), &["cds"]);
    }

    #[test]
    fn base_url_trailing_slash_trimmed() {
        let t = CdsTransport::with_base_url("https://example.test/api/");
        assert_eq!(t.base_url, "https://example.test/api");
    }

    #[test]
    fn url_round_trips() {
        let req = r#"{"area":[50,-130,20,-60],"variable":["temperature"]}"#;
        let url = build_cds_url("reanalysis-era5-pressure-levels", req);
        assert_eq!(url, format!("cds://reanalysis-era5-pressure-levels?{req}"));
        let (dataset, request_json) = parse_cds_url(&url).unwrap();
        assert_eq!(dataset, "reanalysis-era5-pressure-levels");
        assert_eq!(request_json, req);
    }

    #[test]
    fn parse_rejects_malformed_urls() {
        assert!(parse_cds_url("https://x/y").is_err()); // wrong scheme
        assert!(parse_cds_url("cds://dataset-only").is_err()); // no '?'
        assert!(parse_cds_url("cds://?{}").is_err()); // empty dataset
        assert!(parse_cds_url("cds://ds?").is_err()); // empty request
        assert!(parse_cds_url("cds://ds?not-json").is_err()); // not JSON
        assert!(parse_cds_url("cds://ds?[1,2]").is_err()); // JSON but not an object
        assert!(parse_cds_url("cds://ds?{}").is_ok()); // empty object is valid
    }

    #[test]
    fn parses_key_from_cdsapirc_body() {
        let body = "url: https://cds.climate.copernicus.eu/api\nkey: abc-123-def\n";
        assert_eq!(parse_cdsapirc_key(body).as_deref(), Some("abc-123-def"));
        // No surrounding space, first match wins, trailing whitespace trimmed.
        assert_eq!(parse_cdsapirc_key("key:xyz  ").as_deref(), Some("xyz"));
        assert_eq!(
            parse_cdsapirc_key("key: first\nkey: second").as_deref(),
            Some("first")
        );
        // No key line, or an empty value.
        assert_eq!(parse_cdsapirc_key("url: only\n"), None);
        assert_eq!(parse_cdsapirc_key("key:   \n"), None);
    }

    #[test]
    fn reads_key_from_cdsapirc_file_or_absent() {
        let dir = tempfile::tempdir().unwrap();
        let rc = dir.path().join(".cdsapirc");
        std::fs::write(&rc, "url: https://cds/api\nkey: file-key\n").unwrap();
        assert_eq!(read_cdsapirc_key(&rc).unwrap().as_deref(), Some("file-key"));
        // A missing file is `None`, not an error.
        assert_eq!(read_cdsapirc_key(&dir.path().join("nope")).unwrap(), None);
    }

    #[test]
    fn auth_headers_from_resolver_and_empty_without() {
        let resolver = StaticHeaderAuth::header(CDS_REALM, "PRIVATE-TOKEN", "tok");
        let h = auth_headers("cds://ds?{}", Some(&resolver)).unwrap();
        assert_eq!(h.get("PRIVATE-TOKEN").unwrap(), "tok");
        let none = auth_headers("cds://ds?{}", None).unwrap();
        assert!(none.is_empty());
    }
}
