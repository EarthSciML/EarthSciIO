//! The `cds` transport end-to-end against a hermetic localhost mock of the
//! Copernicus CDS API v1 (no external network) — the offline/CI acceptance path.
//!
//! Drives the full submit → poll → results → download dance through the cache,
//! using the [`era5`] request mapping to build the `cds://` URL. Asserts:
//!   * the asset bytes land in the content-addressed cache under the cds:// key;
//!   * the `PRIVATE-TOKEN` auth header reaches submit + poll;
//!   * the job poll loop iterates (running → successful);
//!   * **skip-if-exists**: a second fetch is a cache hit — no re-submit;
//!   * the blob re-reads offline, purely from disk.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use earthsciio::auth::StaticHeaderAuth;
use earthsciio::era5::Era5PressureLevels;
use earthsciio::transport::{CdsTransport, CDS_REALM};
use earthsciio::{cache_key, Cache, FetchRequest};

/// A tiny CDS API v1 mock. Routes the four endpoints the transport calls and
/// records enough state to prove the protocol: how many submits + job polls it
/// saw, and whether `PRIVATE-TOKEN` arrived on the submit.
struct CdsMock {
    port: u16,
    submits: Arc<AtomicUsize>,
    job_polls: Arc<AtomicUsize>,
    token_on_submit: Arc<Mutex<Option<String>>>,
}

impl CdsMock {
    fn base_url(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }
    fn submits(&self) -> usize {
        self.submits.load(Ordering::SeqCst)
    }
    fn job_polls(&self) -> usize {
        self.job_polls.load(Ordering::SeqCst)
    }
    fn token_on_submit(&self) -> Option<String> {
        self.token_on_submit.lock().unwrap().clone()
    }
}

fn spawn_cds_mock(asset: Vec<u8>) -> CdsMock {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let submits = Arc::new(AtomicUsize::new(0));
    let job_polls = Arc::new(AtomicUsize::new(0));
    let token_on_submit = Arc::new(Mutex::new(None));
    let (s, jp, tok) = (submits.clone(), job_polls.clone(), token_on_submit.clone());

    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { continue };
            handle_conn(stream, port, &asset, &s, &jp, &tok);
        }
    });

    CdsMock {
        port,
        submits,
        job_polls,
        token_on_submit,
    }
}

fn handle_conn(
    mut stream: TcpStream,
    port: u16,
    asset: &[u8],
    submits: &AtomicUsize,
    job_polls: &AtomicUsize,
    token_on_submit: &Mutex<Option<String>>,
) {
    // Read the full request: headers until the blank line, then the
    // Content-Length body (so the client's POST is consumed, not RST).
    let mut buf = Vec::new();
    let mut chunk = [0u8; 1024];
    let head_end = loop {
        match stream.read(&mut chunk) {
            Ok(0) => break buf.len(),
            Ok(n) => {
                buf.extend_from_slice(&chunk[..n]);
                if let Some(pos) = find_subslice(&buf, b"\r\n\r\n") {
                    break pos + 4;
                }
            }
            Err(_) => return,
        }
    };
    let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
    let content_length = header_value(&head, "content-length")
        .and_then(|v| v.trim().parse::<usize>().ok())
        .unwrap_or(0);
    while buf.len() < head_end + content_length {
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
            Err(_) => return,
        }
    }

    let request_line = head.lines().next().unwrap_or("");
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or("");
    let path = parts.next().unwrap_or("");

    let (status, content_type, body): (&str, &str, Vec<u8>) =
        if method == "POST" && path.ends_with("/execution") {
            submits.fetch_add(1, Ordering::SeqCst);
            *token_on_submit.lock().unwrap() = header_value(&head, "private-token");
            (
                "200 OK",
                "application/json",
                br#"{"jobID":"job-1","status":"accepted"}"#.to_vec(),
            )
        } else if method == "GET" && path.ends_with("/results") {
            let href = format!("http://127.0.0.1:{port}/download/era5.nc");
            (
                "200 OK",
                "application/json",
                format!(r#"{{"asset":{{"value":{{"href":"{href}"}}}}}}"#).into_bytes(),
            )
        } else if method == "GET" && path == "/download/era5.nc" {
            ("200 OK", "application/x-netcdf", asset.to_vec())
        } else if method == "GET" && path.contains("/jobs/") {
            // First poll reports "running", later polls "successful" — proves
            // the transport's poll loop actually iterates.
            let n = job_polls.fetch_add(1, Ordering::SeqCst) + 1;
            let job_status = if n >= 2 { "successful" } else { "running" };
            (
                "200 OK",
                "application/json",
                format!(r#"{{"jobID":"job-1","status":"{job_status}"}}"#).into_bytes(),
            )
        } else {
            ("404 Not Found", "text/plain", b"unrouted".to_vec())
        };

    let mut resp = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )
    .into_bytes();
    resp.extend_from_slice(&body);
    let _ = stream.write_all(&resp);
    let _ = stream.flush();
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

/// Case-insensitive lookup of a single header value from a raw header block.
fn header_value(head: &str, name: &str) -> Option<String> {
    head.lines().find_map(|line| {
        let (k, v) = line.split_once(':')?;
        k.trim()
            .eq_ignore_ascii_case(name)
            .then(|| v.trim().to_string())
    })
}

fn test_era5() -> Era5PressureLevels {
    Era5PressureLevels {
        variables: vec!["temperature".into(), "geopotential".into()],
        pressure_levels: vec![1000, 850],
        area: [50, -120, 20, -70],
    }
}

fn test_cache(cache_root: &std::path::Path, base_url: &str) -> Cache {
    Cache::builder()
        .data_dir(cache_root)
        .offline(false)
        // Override the built-in production `cds` transport with one pointed at
        // the mock, polling fast so the running→successful loop spins quickly.
        .register_transport(Arc::new(
            CdsTransport::with_base_url(base_url)
                .poll_interval(Duration::from_millis(1))
                .timeout(Duration::from_secs(10)),
        ))
        .register_auth(Arc::new(StaticHeaderAuth::header(
            CDS_REALM,
            "PRIVATE-TOKEN",
            "secret-cds-key",
        )))
        .build()
        .unwrap()
}

#[test]
fn cds_submit_poll_download_caches_then_reads_offline() {
    let asset = b"NETCDF-ish ERA5 asset bytes from the CDS mock\n".repeat(4);
    let mock = spawn_cds_mock(asset.clone());

    let tmp = tempfile::tempdir().unwrap();
    let cache_root = tmp.path().join("cache");
    let cache = test_cache(&cache_root, &mock.base_url());

    let era5 = test_era5();
    let url = era5.cds_url(2018, 11, &[8]);

    // 1) First fetch runs the whole submit→poll→download dance and caches it.
    let blob = cache
        .fetch(&FetchRequest::new(&url).loader("era5").auth_realm(CDS_REALM))
        .unwrap();
    assert_eq!(std::fs::read(&blob.path).unwrap(), asset);
    assert_eq!(blob.key, cache_key(&url));
    assert_eq!(blob.manifest.bytes, asset.len() as u64);
    assert_eq!(blob.manifest.sha256_content, earthsciio::sha256_hex(&asset));
    assert_eq!(blob.manifest.url, url);
    assert_eq!(blob.manifest.source_loader.as_deref(), Some("era5"));
    // The realm is recorded; the credential is NOT (only headers carried it).
    assert_eq!(blob.manifest.auth_realm.as_deref(), Some(CDS_REALM));

    // The protocol actually happened: one submit, ≥2 polls (running→successful),
    // PRIVATE-TOKEN delivered on submit.
    assert_eq!(mock.submits(), 1, "exactly one job submitted");
    assert!(
        mock.job_polls() >= 2,
        "poll loop must iterate to successful"
    );
    assert_eq!(mock.token_on_submit().as_deref(), Some("secret-cds-key"));

    // 2) skip-if-exists: a second fetch is a cache hit — no new submit.
    let again = cache
        .fetch(&FetchRequest::new(&url).loader("era5").auth_realm(CDS_REALM))
        .unwrap();
    assert_eq!(again.path, blob.path);
    assert_eq!(mock.submits(), 1, "cache hit must not re-submit to CDS");

    // 3) Offline re-read resolves purely from disk — never touches the socket.
    let offline = Cache::builder()
        .data_dir(&cache_root)
        .offline(true)
        .build()
        .unwrap();
    let read = offline.fetch(&FetchRequest::new(&url)).unwrap();
    assert_eq!(std::fs::read(&read.path).unwrap(), asset);
    assert_eq!(mock.submits(), 1);

    // An offline miss for an unrequested month is a clean CacheMiss.
    let other = era5.cds_url(2019, 3, &[1]);
    let miss = offline.fetch(&FetchRequest::new(&other)).unwrap_err();
    assert!(miss.is_cache_miss());
}

#[test]
fn cds_job_failure_surfaces_as_transport_error() {
    // A mock that fails the job: submit ok, but every poll reports "failed".
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut chunk = [0u8; 1024];
            let mut buf = Vec::new();
            // Drain headers (bodies are short enough to arrive together here).
            while let Ok(n) = stream.read(&mut chunk) {
                if n == 0 {
                    break;
                }
                buf.extend_from_slice(&chunk[..n]);
                if find_subslice(&buf, b"\r\n\r\n").is_some() {
                    break;
                }
            }
            let head = String::from_utf8_lossy(&buf).to_string();
            let line = head.lines().next().unwrap_or("");
            let body: &[u8] = if line.starts_with("POST") {
                br#"{"jobID":"job-x","status":"accepted"}"#
            } else {
                br#"{"jobID":"job-x","status":"failed"}"#
            };
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = stream.write_all(resp.as_bytes());
            let _ = stream.write_all(body);
            let _ = stream.flush();
        }
    });

    let tmp = tempfile::tempdir().unwrap();
    let cache = test_cache(
        &tmp.path().join("cache"),
        &format!("http://127.0.0.1:{port}"),
    );
    let url = test_era5().cds_url(2018, 11, &[8]);
    let err = cache
        .fetch(&FetchRequest::new(&url).loader("era5").auth_realm(CDS_REALM))
        .unwrap_err();
    // A failed CDS job is reported as every-source-failed (the cache wraps the
    // transport error), naming the cds:// URL.
    assert!(
        matches!(err, earthsciio::Error::AllMirrorsFailed { .. }),
        "got {err}"
    );
    assert!(err.to_string().contains("failed"), "got {err}");
}
