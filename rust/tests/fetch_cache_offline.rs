//! Fetch → cache → offline re-read (acceptance criterion 1), over both the
//! `file` and `http` transports. The HTTP path uses a hermetic localhost mock
//! server (no external network), exercising GET, conditional GET, and `304 Not
//! Modified` reuse.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use earthsciio::auth::StaticHeaderAuth;
use earthsciio::{cache_key, Cache, FetchRequest};

// --- file transport ---------------------------------------------------------

#[test]
fn file_fetch_caches_then_reads_offline() {
    let tmp = tempfile::tempdir().unwrap();
    let cache_root = tmp.path().join("cache");

    // A local "source" file the file:// transport will copy into the cache.
    let src = tmp.path().join("source.nc");
    let body = b"NETCDF-ish bytes for the file transport test".to_vec();
    std::fs::write(&src, &body).unwrap();
    let url = format!("file://{}", src.display());

    // Online fetch: copies the file into the content-addressed cache.
    let online = Cache::builder()
        .data_dir(&cache_root)
        .offline(false)
        .build()
        .unwrap();
    let blob = online
        .fetch(&FetchRequest::new(&url).loader("nei2016"))
        .unwrap();

    assert_eq!(blob.key, cache_key(&url));
    assert_eq!(std::fs::read(&blob.path).unwrap(), body);
    assert_eq!(blob.manifest.bytes, body.len() as u64);
    assert_eq!(blob.manifest.sha256_content, earthsciio::sha256_hex(&body));
    assert_eq!(blob.manifest.url, url);
    assert_eq!(blob.manifest.source_loader.as_deref(), Some("nei2016"));

    // The blob lives under the v1 fan-out, with the URL-derived debug extension.
    let key = blob.key.clone();
    let expected = cache_root
        .join("v1")
        .join("blobs")
        .join(&key[..2])
        .join(format!("{key}.nc"));
    assert_eq!(blob.path, expected);

    // A second online fetch is a hit and returns the identical blob.
    let again = online.fetch(&FetchRequest::new(&url)).unwrap();
    assert_eq!(again.path, blob.path);

    // Offline re-read against the same root resolves purely from disk.
    let offline = Cache::builder()
        .data_dir(&cache_root)
        .offline(true)
        .build()
        .unwrap();
    let read = offline.fetch(&FetchRequest::new(&url)).unwrap();
    assert_eq!(std::fs::read(&read.path).unwrap(), body);

    // Offline miss for an unknown URL raises CacheMiss.
    let miss = offline
        .fetch(&FetchRequest::new("file:///does/not/exist.nc"))
        .unwrap_err();
    assert!(miss.is_cache_miss());
}

// --- http transport (hermetic localhost mock) -------------------------------

/// A tiny HTTP/1.1 server: serves `body` with a fixed `ETag` on a normal GET,
/// and `304 Not Modified` when the request carries a matching `If-None-Match`.
/// Counts full 200s and 304s so the test can prove the conditional-GET path.
struct MockServer {
    port: u16,
    gets: Arc<AtomicUsize>,
    not_modified: Arc<AtomicUsize>,
    /// The header block of every request received (for assertions).
    requests: Arc<Mutex<Vec<String>>>,
}

fn spawn_mock_server(body: Vec<u8>, etag: &'static str) -> MockServer {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let gets = Arc::new(AtomicUsize::new(0));
    let not_modified = Arc::new(AtomicUsize::new(0));
    let requests = Arc::new(Mutex::new(Vec::new()));
    let (g, nm, rq) = (gets.clone(), not_modified.clone(), requests.clone());

    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { continue };
            handle_conn(stream, &body, etag, &g, &nm, &rq);
        }
    });

    MockServer {
        port,
        gets,
        not_modified,
        requests,
    }
}

fn handle_conn(
    mut stream: TcpStream,
    body: &[u8],
    etag: &str,
    gets: &AtomicUsize,
    not_modified: &AtomicUsize,
    requests: &Mutex<Vec<String>>,
) {
    // Read request headers (until the blank line).
    let mut buf = Vec::new();
    let mut chunk = [0u8; 1024];
    loop {
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                buf.extend_from_slice(&chunk[..n]);
                if buf.windows(4).any(|w| w == b"\r\n\r\n") {
                    break;
                }
            }
            Err(_) => return,
        }
    }
    let req = String::from_utf8_lossy(&buf);
    requests.lock().unwrap().push(req.to_string());
    let if_none_match = req.lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        name.trim()
            .eq_ignore_ascii_case("if-none-match")
            .then(|| value.trim().to_string())
    });

    if if_none_match.as_deref() == Some(etag) {
        not_modified.fetch_add(1, Ordering::SeqCst);
        let resp =
            format!("HTTP/1.1 304 Not Modified\r\nETag: {etag}\r\nConnection: close\r\n\r\n");
        let _ = stream.write_all(resp.as_bytes());
    } else {
        gets.fetch_add(1, Ordering::SeqCst);
        let mut resp = format!(
            "HTTP/1.1 200 OK\r\nETag: {etag}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        )
        .into_bytes();
        resp.extend_from_slice(body);
        let _ = stream.write_all(&resp);
    }
    let _ = stream.flush();
}

#[test]
fn http_fetch_conditional_get_and_offline_reuse() {
    let body = b"the quick brown fox jumps over the lazy dog\n".repeat(8);
    let etag = "\"era5-v1\"";
    let server = spawn_mock_server(body.clone(), etag);
    let url = format!("http://127.0.0.1:{}/era5/20181108.nc", server.port);

    let tmp = tempfile::tempdir().unwrap();
    let cache_root = tmp.path().join("cache");
    let online = Cache::builder()
        .data_dir(&cache_root)
        .offline(false)
        .build()
        .unwrap();

    // 1) First fetch downloads (one 200) and records the ETag in the manifest.
    let blob = online
        .fetch(&FetchRequest::new(&url).loader("era5"))
        .unwrap();
    assert_eq!(std::fs::read(&blob.path).unwrap(), body);
    assert_eq!(blob.manifest.bytes, body.len() as u64);
    assert_eq!(blob.manifest.etag.as_deref(), Some(etag));
    assert_eq!(blob.manifest.sha256_content, earthsciio::sha256_hex(&body));
    assert_eq!(server.gets.load(Ordering::SeqCst), 1);
    assert_eq!(server.not_modified.load(Ordering::SeqCst), 0);

    // 2) Second fetch revalidates with If-None-Match and gets a 304 — the blob
    //    is reused, NOT re-downloaded (gets stays 1; one 304 recorded).
    let blob2 = online
        .fetch(&FetchRequest::new(&url).loader("era5"))
        .unwrap();
    assert_eq!(blob2.path, blob.path);
    assert_eq!(std::fs::read(&blob2.path).unwrap(), body);
    assert_eq!(
        server.gets.load(Ordering::SeqCst),
        1,
        "must not re-download on 304"
    );
    assert_eq!(
        server.not_modified.load(Ordering::SeqCst),
        1,
        "must take the conditional-GET path"
    );

    // 3) Offline re-read resolves purely from disk — no socket at all.
    let offline = Cache::builder()
        .data_dir(&cache_root)
        .offline(true)
        .build()
        .unwrap();
    let read = offline.fetch(&FetchRequest::new(&url)).unwrap();
    assert_eq!(std::fs::read(&read.path).unwrap(), body);
    assert_eq!(server.gets.load(Ordering::SeqCst), 1);
    assert_eq!(server.not_modified.load(Ordering::SeqCst), 1);
}

// --- mirror failover --------------------------------------------------------

#[test]
fn mirror_failover_falls_through_to_a_working_source() {
    let tmp = tempfile::tempdir().unwrap();
    let cache_root = tmp.path().join("cache");
    let body = b"bytes-served-by-the-mirror".to_vec();
    let good = tmp.path().join("mirror.nc");
    std::fs::write(&good, &body).unwrap();

    let primary = "file:///no/such/primary.nc";
    let mirror = format!("file://{}", good.display());
    let mirrors = [mirror.as_str()];

    let cache = Cache::builder()
        .data_dir(&cache_root)
        .offline(false)
        .build()
        .unwrap();
    let blob = cache
        .fetch(&FetchRequest::new(primary).mirrors(&mirrors))
        .unwrap();

    // Bytes came from the mirror, but the cache identity is the canonical primary.
    assert_eq!(std::fs::read(&blob.path).unwrap(), body);
    assert_eq!(blob.key, cache_key(primary));
    assert_eq!(blob.manifest.url, primary);

    // It is now resolvable offline by the *primary* URL.
    let offline = Cache::builder()
        .data_dir(&cache_root)
        .offline(true)
        .build()
        .unwrap();
    assert!(offline.fetch(&FetchRequest::new(primary)).is_ok());

    // With no working source at all, every-source-failed is surfaced.
    let err = cache
        .fetch(&FetchRequest::new("file:///no/such/x.nc").mirrors(&["file:///also/missing.nc"]))
        .unwrap_err();
    assert!(
        matches!(err, earthsciio::Error::AllMirrorsFailed { .. }),
        "got {err}"
    );
}

// --- auth seam through the http transport -----------------------------------

#[test]
fn auth_seam_attaches_headers_through_the_http_transport() {
    let body = b"authenticated-bytes".to_vec();
    let server = spawn_mock_server(body.clone(), "\"auth-v1\"");
    let url = format!("http://127.0.0.1:{}/cds/file.nc", server.port);

    let tmp = tempfile::tempdir().unwrap();
    let cache = Cache::builder()
        .data_dir(tmp.path().join("cache"))
        .offline(false)
        .register_auth(Arc::new(StaticHeaderAuth::bearer(
            "cds",
            "secret-cds-token",
        )))
        .build()
        .unwrap();

    let blob = cache
        .fetch(&FetchRequest::new(&url).loader("era5").auth_realm("cds"))
        .unwrap();
    assert_eq!(std::fs::read(&blob.path).unwrap(), body);
    // The realm is recorded in the manifest; the credential is NOT — only the
    // request headers carried it.
    assert_eq!(blob.manifest.auth_realm.as_deref(), Some("cds"));

    let seen = server.requests.lock().unwrap();
    let joined = seen.join("\n").to_ascii_lowercase();
    assert!(
        joined.contains("authorization:"),
        "auth header missing: {seen:?}"
    );
    assert!(
        joined.contains("bearer secret-cds-token"),
        "token not sent: {seen:?}"
    );

    // A request under an unregistered realm is a clean error (fresh URL ⇒ a
    // real fetch attempt that resolves auth first).
    let other = format!("http://127.0.0.1:{}/cds/other.nc", server.port);
    let err = cache
        .fetch(&FetchRequest::new(&other).auth_realm("unregistered"))
        .unwrap_err();
    assert!(
        matches!(err, earthsciio::Error::MissingAuth { .. }),
        "got {err}"
    );
}
