//! Concurrency (acceptance criterion 3): many fetchers racing the same URL must
//! download it **exactly once** and never observe a torn blob. This exercises
//! the advisory `flock` + re-check + atomic rename of `spec/cache-format.md` §6.
//!
//! A `CountingTransport` registered under a synthetic `count://` scheme records
//! how many times bytes were actually fetched, and sleeps mid-fetch to widen the
//! race window so the lock is genuinely contended.

use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use earthsciio::auth::AuthResolver;
use earthsciio::transport::{Conditional, FetchResult, FetchStatus, Transport};
use earthsciio::{Cache, FetchRequest};

/// A transport that counts real downloads and writes a fixed body, sleeping to
/// force overlap between concurrent fetchers.
struct CountingTransport {
    hits: Arc<AtomicUsize>,
    body: Vec<u8>,
}

impl Transport for CountingTransport {
    fn schemes(&self) -> &'static [&'static str] {
        &["count"]
    }

    fn fetch(
        &self,
        _url: &str,
        dest: &Path,
        _conditional: &Conditional,
        _auth: Option<&dyn AuthResolver>,
    ) -> earthsciio::Result<FetchResult> {
        self.hits.fetch_add(1, Ordering::SeqCst);
        // Hold the lock long enough that all racers are queued behind it.
        std::thread::sleep(Duration::from_millis(100));
        std::fs::write(dest, &self.body)?; // io::Error -> earthsciio::Error via From
        Ok(FetchResult {
            status: FetchStatus::Downloaded,
            etag: None,
            last_modified: None,
            bytes_written: self.body.len() as u64,
        })
    }
}

const URL: &str = "count://shared/blob";
const N: usize = 12;

#[test]
fn shared_cache_downloads_exactly_once() {
    let tmp = tempfile::tempdir().unwrap();
    let body = b"shared-blob-content-0123456789".repeat(4);
    let hits = Arc::new(AtomicUsize::new(0));

    let cache = Cache::builder()
        .data_dir(tmp.path().join("cache"))
        .offline(false)
        .register_transport(Arc::new(CountingTransport {
            hits: hits.clone(),
            body: body.clone(),
        }))
        .build()
        .unwrap();
    let cache = Arc::new(cache);

    let contents = race(N, |_| {
        let c = cache.clone();
        move || {
            let blob = c.fetch(&FetchRequest::new(URL)).expect("fetch ok");
            std::fs::read(&blob.path).expect("blob readable")
        }
    });

    assert_eq!(
        hits.load(Ordering::SeqCst),
        1,
        "exactly one download despite {N} fetchers"
    );
    for c in &contents {
        assert_eq!(
            c, &body,
            "every fetcher saw the complete blob (no torn read)"
        );
    }

    // A post-race fetch is a hit, not another download.
    let blob = cache.fetch(&FetchRequest::new(URL)).unwrap();
    assert_eq!(blob.manifest.bytes, body.len() as u64);
    assert_eq!(blob.manifest.sha256_content, earthsciio::sha256_hex(&body));
    assert_eq!(hits.load(Ordering::SeqCst), 1);
}

#[test]
fn independent_caches_same_root_download_once() {
    // Separate Cache instances (separate registries, separate lock fds) racing
    // the same root — the realistic multi-process shape, modeled with threads.
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("cache");
    let body = b"independent-cache-shared-bytes".repeat(3);
    let hits = Arc::new(AtomicUsize::new(0));
    // One shared transport instance so the download counter is global.
    let transport: Arc<CountingTransport> = Arc::new(CountingTransport {
        hits: hits.clone(),
        body: body.clone(),
    });

    let contents = race(N, |_| {
        let root = root.clone();
        let transport = transport.clone();
        move || {
            let cache = Cache::builder()
                .data_dir(&root)
                .offline(false)
                .register_transport(transport)
                .build()
                .unwrap();
            let blob = cache.fetch(&FetchRequest::new(URL)).expect("fetch ok");
            std::fs::read(&blob.path).expect("blob readable")
        }
    });

    assert_eq!(
        hits.load(Ordering::SeqCst),
        1,
        "independent caches still download once"
    );
    for c in &contents {
        assert_eq!(c, &body);
    }
}

/// Spawn `n` threads built from `make` and join them, returning their results in
/// spawn order.
fn race<T, F, W>(n: usize, mut make: F) -> Vec<T>
where
    T: Send + 'static,
    W: FnOnce() -> T + Send + 'static,
    F: FnMut(usize) -> W,
{
    let handles: Vec<_> = (0..n).map(|i| std::thread::spawn(make(i))).collect();
    handles.into_iter().map(|h| h.join().unwrap()).collect()
}
