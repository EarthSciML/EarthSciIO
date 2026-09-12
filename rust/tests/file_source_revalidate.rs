//! A `file://` entry is rechecked against its source before it is served
//! (`spec/cache-format.md` §4 rung 0) — the regression suite for
//! `EarthSciML/EarthSciAST#293`.
//!
//! The bug's whole signature is that it passes GREEN, so every test here
//! reproduces the actual failure rather than the happy path: warm the cache
//! from a local file, replace that file **in place** at the same path (so the
//! resolved URL, and therefore the cache key, is unchanged), read again, and
//! demand the new bytes. The report's corpus was 2.8 GB replaced at the same
//! paths; the entry it caught held 371 bytes while the file on disk was 363.
//!
//! Two of these are the ones a plausible half-fix fails:
//!
//! * a replacement of the SAME byte length (a float re-encode, a corrected
//!   value) — a size-only check waves it through;
//! * an UNCHANGED file, which must still be served from the warm entry and must
//!   NOT be re-ingested. A "fix" that simply stops caching passes every other
//!   test here; this one counts transport fetches to prove it did not.

// Native-only: the cache, the transports and the store do not exist on wasm32
// (see the crate's module docs).
#![cfg(not(target_arch = "wasm32"))]

use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use earthsciio::auth::AuthResolver;
use earthsciio::transport::{Conditional, FetchResult, FetchStatus, FileTransport, Transport};
use earthsciio::{
    sha256_hex, Cache, FetchRequest, Manifest, Result, SourceState, MANIFEST_SCHEMA,
};

/// The manifest a cache would have written when it ingested `body` from `src` —
/// the record rung 0 judges the source against.
fn manifest_for(src: &Path, body: &[u8]) -> Manifest {
    Manifest {
        auth_realm: None,
        bytes: body.len() as u64,
        etag: None,
        fetched_at: "2026-01-01T00:00:00Z".to_string(),
        last_modified: None,
        schema: MANIFEST_SCHEMA.to_string(),
        sha256_content: sha256_hex(body),
        source_loader: None,
        url: format!("file://{}", src.display()),
    }
}

/// The `file` transport, counting the fetches that actually reach it. A cache
/// hit never gets here, so the counter is the difference between "served warm"
/// and "re-ingested" — the thing tests 3 and 5 of the report turn on.
struct CountingFile {
    inner: FileTransport,
    fetches: Arc<AtomicUsize>,
}

impl Transport for CountingFile {
    fn schemes(&self) -> &'static [&'static str] {
        &["file"]
    }

    fn fetch(
        &self,
        url: &str,
        dest: &Path,
        conditional: &Conditional,
        auth: Option<&dyn AuthResolver>,
    ) -> Result<FetchResult> {
        self.fetches.fetch_add(1, Ordering::SeqCst);
        self.inner.fetch(url, dest, conditional, auth)
    }
}

/// A transport for a made-up REMOTE scheme: it serves bytes from memory and has
/// no local file anywhere. Rung 0 must not touch it.
struct CountingMem {
    body: Vec<u8>,
    fetches: Arc<AtomicUsize>,
}

impl Transport for CountingMem {
    fn schemes(&self) -> &'static [&'static str] {
        &["mem"]
    }

    fn fetch(
        &self,
        _url: &str,
        dest: &Path,
        _conditional: &Conditional,
        _auth: Option<&dyn AuthResolver>,
    ) -> Result<FetchResult> {
        self.fetches.fetch_add(1, Ordering::SeqCst);
        std::fs::write(dest, &self.body).unwrap();
        Ok(FetchResult {
            status: FetchStatus::Downloaded,
            etag: None,
            last_modified: None,
            bytes_written: self.body.len() as u64,
        })
    }
}

/// A cache over `root` with a fetch-counting `file` transport.
fn counting_cache(root: &Path, counter: &Arc<AtomicUsize>) -> Cache {
    Cache::builder()
        .data_dir(root)
        .offline(false)
        .register_transport(Arc::new(CountingFile {
            inner: FileTransport::new(),
            fetches: counter.clone(),
        }))
        .build()
        .unwrap()
}

/// The bytes a fetch actually hands back to a reader.
fn served(cache: &Cache, url: &str) -> Vec<u8> {
    let blob = cache.fetch(&FetchRequest::new(url)).unwrap();
    std::fs::read(&blob.path).unwrap()
}

// --- 1. replaced in place, different length ---------------------------------

/// The report's own shape: same path, new contents, a different byte length.
/// Before the fix this returned the OLD corpus for the life of the cache dir.
#[test]
fn source_replaced_in_place_is_re_ingested() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("characterization.csv");
    let url = format!("file://{}", src.display());
    let counter = Arc::new(AtomicUsize::new(0));
    let cache = counting_cache(&tmp.path().join("cache"), &counter);

    let old = vec![b'o'; 371]; // the report's stale entry
    std::fs::write(&src, &old).unwrap();
    assert_eq!(served(&cache, &url), old);
    assert_eq!(counter.load(Ordering::SeqCst), 1);

    // Replace the corpus in place — same path, so the same resolved URL and the
    // same cache key.
    let new = vec![b'n'; 363]; // the report's file-on-disk
    std::fs::write(&src, &new).unwrap();

    assert_eq!(
        served(&cache, &url),
        new,
        "a file replaced in place must be re-ingested, not served from the warm entry"
    );
    assert_eq!(
        counter.load(Ordering::SeqCst),
        2,
        "it must have re-ingested"
    );

    // The manifest is the record the report found lying: it must now describe
    // the file that is actually there.
    let blob = cache.fetch(&FetchRequest::new(&url)).unwrap();
    assert_eq!(blob.manifest.bytes, 363);
    assert_eq!(blob.manifest.sha256_content, earthsciio::sha256_hex(&new));
}

// --- 2. replaced in place, SAME length ---------------------------------------

/// The sibling case a size-only check misses. The report's corpus happened to
/// change length; a float re-encode or a corrected value easily would not.
#[test]
fn source_replaced_with_same_length_is_re_ingested() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("emissions.csv");
    let url = format!("file://{}", src.display());
    let counter = Arc::new(AtomicUsize::new(0));
    let cache = counting_cache(&tmp.path().join("cache"), &counter);

    let old = b"year,pm25\n2016,12.500\n".to_vec();
    let new = b"year,pm25\n2016,12.499\n".to_vec();
    assert_eq!(
        old.len(),
        new.len(),
        "this test is about an equal-length edit"
    );
    assert_ne!(old, new);

    std::fs::write(&src, &old).unwrap();
    assert_eq!(served(&cache, &url), old);

    std::fs::write(&src, &new).unwrap();
    assert_eq!(
        served(&cache, &url),
        new,
        "an equal-length replacement must still be detected — size alone is not enough"
    );
    assert_eq!(counter.load(Ordering::SeqCst), 2);
}

// --- 3. an unchanged file is still CACHED ------------------------------------

/// The proof that this is still a cache. A fix that revalidated by always
/// re-copying the file would pass every other test in this file and quietly
/// turn the cache into a no-op.
#[test]
fn unchanged_source_is_served_from_the_warm_entry() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("static.csv");
    let url = format!("file://{}", src.display());
    let counter = Arc::new(AtomicUsize::new(0));
    let cache = counting_cache(&tmp.path().join("cache"), &counter);

    let body = b"year,value\n2016,1.0\n".to_vec();
    std::fs::write(&src, &body).unwrap();

    let first = cache.fetch(&FetchRequest::new(&url)).unwrap();
    assert_eq!(counter.load(Ordering::SeqCst), 1);

    for _ in 0..3 {
        let again = cache.fetch(&FetchRequest::new(&url)).unwrap();
        assert_eq!(again.path, first.path);
        assert_eq!(std::fs::read(&again.path).unwrap(), body);
        assert_eq!(again.manifest.fetched_at, first.manifest.fetched_at);
    }
    assert_eq!(
        counter.load(Ordering::SeqCst),
        1,
        "an unchanged file must NOT be re-ingested — the entry is still a cache hit"
    );

    // A fresh cache object over the same root (a new process, the case the
    // report's "warmed at different times" checkouts hit) is still a hit.
    let reopened = counting_cache(&tmp.path().join("cache"), &counter);
    assert_eq!(
        std::fs::read(&reopened.fetch(&FetchRequest::new(&url)).unwrap().path).unwrap(),
        body
    );
    assert_eq!(counter.load(Ordering::SeqCst), 1);
}

// --- 4. a deleted source is an ERROR -----------------------------------------

/// "A worktree kept passing after its data path was pointed at a directory that
/// does not exist, because nothing was read at all." Absence must be loud.
#[test]
fn deleted_source_behind_a_warm_entry_is_an_error() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("corpus.csv");
    let url = format!("file://{}", src.display());
    let counter = Arc::new(AtomicUsize::new(0));
    let cache = counting_cache(&tmp.path().join("cache"), &counter);

    std::fs::write(&src, b"present").unwrap();
    assert_eq!(served(&cache, &url), b"present");

    std::fs::remove_file(&src).unwrap();

    let err = cache
        .fetch(&FetchRequest::new(&url))
        .expect_err("a deleted source must not keep serving its warm entry");
    assert!(
        err.is_not_found(),
        "a deleted source is a definitive absence, got {err}"
    );
}

/// The same, with the whole directory removed — the report's "pointed at a
/// directory that does not exist".
#[test]
fn deleted_source_directory_behind_a_warm_entry_is_an_error() {
    let tmp = tempfile::tempdir().unwrap();
    let corpus = tmp.path().join("characterization");
    std::fs::create_dir(&corpus).unwrap();
    let src = corpus.join("rates.csv");
    let url = format!("file://{}", src.display());
    let counter = Arc::new(AtomicUsize::new(0));
    let cache = counting_cache(&tmp.path().join("cache"), &counter);

    std::fs::write(&src, b"rates").unwrap();
    assert_eq!(served(&cache, &url), b"rates");

    std::fs::remove_dir_all(&corpus).unwrap();
    assert!(cache.fetch(&FetchRequest::new(&url)).is_err());
}

// --- 5. remote entries are untouched -----------------------------------------

/// Rung 0 is for `file://` only. A remote source has no local truth to consult,
/// and its warm entry must keep being served without a re-fetch — the immutable
/// declaration (rule 2) is what makes an S3-backed store usable at all.
#[test]
fn a_remote_entry_is_not_rechecked_against_the_filesystem() {
    let tmp = tempfile::tempdir().unwrap();
    let counter = Arc::new(AtomicUsize::new(0));
    let cache = Cache::builder()
        .data_dir(tmp.path().join("cache"))
        .offline(false)
        .register_transport(Arc::new(CountingMem {
            body: b"remote-bytes".to_vec(),
            fetches: counter.clone(),
        }))
        .build()
        .unwrap();

    let url = "mem://store/chunk/0.0.0";
    assert_eq!(served(&cache, url), b"remote-bytes");
    for _ in 0..3 {
        assert_eq!(served(&cache, url), b"remote-bytes");
    }
    assert_eq!(
        counter.load(Ordering::SeqCst),
        1,
        "a remote entry must stay a warm hit; rung 0 is a file:// rung"
    );
}

// --- 6. the documented opt-out ------------------------------------------------

/// `revalidate_file_sources(false)` (env: `EARTHSCI_REVALIDATE_FILE=0`) is the
/// escape hatch for a corpus known to be immutable. It restores the old
/// behaviour exactly — which is why it is off the default path.
#[test]
fn the_opt_out_restores_the_stale_serve() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("immutable.csv");
    let url = format!("file://{}", src.display());
    let counter = Arc::new(AtomicUsize::new(0));
    let cache = Cache::builder()
        .data_dir(tmp.path().join("cache"))
        .offline(false)
        .revalidate_file_sources(false)
        .register_transport(Arc::new(CountingFile {
            inner: FileTransport::new(),
            fetches: counter.clone(),
        }))
        .build()
        .unwrap();

    std::fs::write(&src, b"old").unwrap();
    assert_eq!(served(&cache, &url), b"old");
    std::fs::write(&src, b"new").unwrap();
    assert_eq!(
        served(&cache, &url),
        b"old",
        "opted out: the warm entry wins"
    );
    assert_eq!(counter.load(Ordering::SeqCst), 1);
}

// --- the existing blob-integrity check is a different question -----------------

/// Why `verify_on_read` did not catch any of this: it hashes the CACHED BLOB
/// against the manifest, i.e. it compares the copy with the record of the copy.
/// With the source replaced it is perfectly consistent and perfectly wrong.
#[test]
fn blob_integrity_verification_alone_never_sees_a_replaced_source() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("x.csv");
    let url = format!("file://{}", src.display());
    let root = tmp.path().join("cache");

    // Warm the entry, with blob verification ON and the source recheck OFF.
    let counter = Arc::new(AtomicUsize::new(0));
    let old_only = Cache::builder()
        .data_dir(&root)
        .offline(false)
        .verify_on_read(true)
        .revalidate_file_sources(false)
        .register_transport(Arc::new(CountingFile {
            inner: FileTransport::new(),
            fetches: counter.clone(),
        }))
        .build()
        .unwrap();
    std::fs::write(&src, b"old").unwrap();
    assert_eq!(served(&old_only, &url), b"old");
    std::fs::write(&src, b"new-and-longer").unwrap();

    // verify_on_read passes: blob and manifest agree with each other.
    assert_eq!(served(&old_only, &url), b"old");

    // Rung 0 is the check that asks the source. Same cache root, same key.
    let fixed = counting_cache(&root, &counter);
    assert_eq!(served(&fixed, &url), b"new-and-longer");
}

// --- 9. a source this host cannot READ is not a source that changed ----------

/// Rung 0 answers `Unknown`, not "stale", when the path cannot be consulted.
///
/// Warming a cache where `/corpus` is mounted and reading it where it is not is
/// an ordinary HPC shape, and it used to work: the entry was served warm. The
/// first cut of this rung turned every `stat` failure into a re-ingest, which
/// turned that shape into a hard `AllMirrorsFailed`. Only a genuine `NotFound`
/// is evidence of a deletion; "permission denied" is evidence of nothing.
#[test]
fn unreadable_source_abstains_and_serves_the_warm_entry() {
    use std::os::unix::fs::PermissionsExt;

    let tmp = tempfile::tempdir().unwrap();
    let corpus = tmp.path().join("corpus");
    std::fs::create_dir(&corpus).unwrap();
    let src = corpus.join("x.nc");
    let url = format!("file://{}", src.display());
    let counter = Arc::new(AtomicUsize::new(0));
    let cache = counting_cache(&tmp.path().join("cache"), &counter);

    let body = b"warm-corpus".to_vec();
    std::fs::write(&src, &body).unwrap();
    assert_eq!(served(&cache, &url), body);
    assert_eq!(counter.load(Ordering::SeqCst), 1);

    // Make the path untraversable — the closest a test can get to "that
    // filesystem is not mounted on this node".
    std::fs::set_permissions(&corpus, std::fs::Permissions::from_mode(0o000)).unwrap();
    let out = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| served(&cache, &url)));
    std::fs::set_permissions(&corpus, std::fs::Permissions::from_mode(0o755)).unwrap();

    assert_eq!(
        out.expect("an unreadable source must not turn a warm entry into an error"),
        body
    );
    assert_eq!(
        counter.load(Ordering::SeqCst),
        1,
        "nothing was re-ingested: rung 0 learned nothing and said so"
    );
}

/// The classification itself, on the three error shapes that matter.
#[test]
fn only_a_genuine_absence_reads_as_missing() {
    use std::os::unix::fs::PermissionsExt;
    use earthsciio::file_source_state;

    let tmp = tempfile::tempdir().unwrap();
    let body = b"corpus".to_vec();
    let present = tmp.path().join("present.nc");
    std::fs::write(&present, &body).unwrap();
    let m = manifest_for(&present, &body);

    assert_eq!(file_source_state(&present, &m), SourceState::Current);
    assert_eq!(
        file_source_state(&tmp.path().join("gone.nc"), &m),
        SourceState::Missing
    );

    // A path *under a file* (ENOTDIR) is a broken path, not a deletion: this
    // host may simply have the wrong mount.
    assert_eq!(
        file_source_state(&present.join("deeper.nc"), &m),
        SourceState::Unknown
    );

    let locked = tmp.path().join("locked");
    std::fs::create_dir(&locked).unwrap();
    let hidden = locked.join("x.nc");
    std::fs::write(&hidden, &body).unwrap();
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
    let state = file_source_state(&hidden, &m);
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(state, SourceState::Unknown);
}

// --- 10. a missing canonical with mirrors is not a per-read re-download ------

/// A `file://` canonical plus a working mirror must still produce cache HITS.
///
/// The manifest records the canonical URL whichever candidate served the bytes,
/// so rung 0 stats a canonical that was never there, finds nothing, and — if it
/// treated that as stale — would re-download from the mirror on every single
/// read, for ever, never once hitting. Measured before the fix: 3 fetches for 3
/// reads. The replaced-in-place protection is NOT given up here; only the
/// "canonical absent" verdict is, and only when mirrors were supplied.
#[test]
fn a_missing_canonical_with_mirrors_still_hits() {
    let tmp = tempfile::tempdir().unwrap();
    let canonical = tmp.path().join("never-mounted.nc");
    let canonical_url = format!("file://{}", canonical.display());
    let mem_fetches = Arc::new(AtomicUsize::new(0));
    let body = b"bytes-from-the-mirror".to_vec();

    let cache = Cache::builder()
        .data_dir(tmp.path().join("cache"))
        .offline(false)
        .register_transport(Arc::new(FileTransport::new()))
        .register_transport(Arc::new(CountingMem {
            body: body.clone(),
            fetches: mem_fetches.clone(),
        }))
        .build()
        .unwrap();

    let mirrors = ["mem://mirror/x.nc"];
    for i in 0..3 {
        let blob = cache
            .fetch(&FetchRequest::new(&canonical_url).mirrors(&mirrors))
            .unwrap();
        assert_eq!(std::fs::read(&blob.path).unwrap(), body, "read {i}");
    }
    assert_eq!(
        mem_fetches.load(Ordering::SeqCst),
        1,
        "the mirror must be downloaded ONCE, not once per read"
    );
}

/// …but a canonical that is really there and really changed is still caught,
/// mirrors or no mirrors. The abstention above is scoped to *absence*.
#[test]
fn mirrors_do_not_excuse_a_replaced_canonical() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("x.nc");
    let url = format!("file://{}", src.display());
    let counter = Arc::new(AtomicUsize::new(0));
    let mem_fetches = Arc::new(AtomicUsize::new(0));

    let cache = Cache::builder()
        .data_dir(tmp.path().join("cache"))
        .offline(false)
        .register_transport(Arc::new(CountingFile {
            inner: FileTransport::new(),
            fetches: counter.clone(),
        }))
        .register_transport(Arc::new(CountingMem {
            body: b"mirror".to_vec(),
            fetches: mem_fetches.clone(),
        }))
        .build()
        .unwrap();

    let mirrors = ["mem://mirror/x.nc"];
    let fetch = |c: &Cache| -> Vec<u8> {
        let blob = c
            .fetch(&FetchRequest::new(&url).mirrors(&mirrors))
            .unwrap();
        std::fs::read(&blob.path).unwrap()
    };

    std::fs::write(&src, b"year,value\n2016,1.0\n").unwrap();
    assert_eq!(fetch(&cache), b"year,value\n2016,1.0\n".to_vec());
    std::fs::write(&src, b"year,value\n2016,9.0\n").unwrap(); // same length
    assert_eq!(fetch(&cache), b"year,value\n2016,9.0\n".to_vec());
    assert_eq!(mem_fetches.load(Ordering::SeqCst), 0, "canonical served both");
}

// --- 11. the fingerprint memo -----------------------------------------------

/// Rung 0 reads each unchanged source ONCE per process, not once per call.
///
/// `fetch` is called per TICK on the record-selective read paths, so hashing on
/// every call made the rung cost the whole corpus per tick. A digest is kept
/// against the `(length, mtime)` it was computed for and reused while both hold.
#[test]
fn an_unchanged_source_is_hashed_once_not_once_per_read() {
    use earthsciio::SourceRevalidator;

    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("x.nc");
    let body = vec![b'a'; 4096];
    std::fs::write(&src, &body).unwrap();
    let m = manifest_for(&src, &body);

    let rev = SourceRevalidator::new();
    for _ in 0..25 {
        assert_eq!(rev.state(&src, &m), SourceState::Current);
    }
    assert_eq!(rev.full_reads(), 1, "24 of the 25 reads came from the memo");
}

/// The memo must not outlive the fingerprint it was taken under: a replacement
/// changes the mtime, and the digest is recomputed.
#[test]
fn the_memo_is_dropped_when_the_source_changes() {
    use earthsciio::SourceRevalidator;

    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("x.nc");
    let body = b"year,value\n2016,1.0\n".to_vec();
    std::fs::write(&src, &body).unwrap();
    let m = manifest_for(&src, &body);

    let rev = SourceRevalidator::new();
    assert_eq!(rev.state(&src, &m), SourceState::Current);

    // Same length, different bytes — only the mtime betrays it, which is
    // exactly the case the memo has to get right.
    std::thread::sleep(std::time::Duration::from_millis(10));
    std::fs::write(&src, b"year,value\n2016,9.0\n").unwrap();
    assert_eq!(rev.state(&src, &m), SourceState::Replaced);
    assert_eq!(rev.full_reads(), 2);
}
