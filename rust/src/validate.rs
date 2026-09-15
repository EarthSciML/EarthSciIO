//! The cache validation ladder (`spec/cache-format.md` §4).
//!
//! A cache **hit** requires the blob to be present **and valid**. Validity is
//! decided in this order (first applicable wins):
//!
//! 0. **Local source recheck** — a `file://` source is compared against the
//!    file it was ingested from ([`file_source_state`]). Rules 1-3 are all
//!    about a source that can only be consulted over the network; a local file
//!    has an exact truth sitting on disk. It answers with four states, not a
//!    boolean: the difference between *gone*, *changed* and *cannot tell from
//!    here* is what the caller acts on (see [`SourceState`]).
//! 1. **Content hash** — a loader-declared checksum (none today) beats everything.
//! 2. **Conditional GET** — stored `etag`/`last_modified` ⇒ revalidate over the
//!    network (`If-None-Match` / `If-Modified-Since`).
//! 3. **TTL from `temporal`** — a closed past period is immutable; an incomplete
//!    period has a short TTL; a static loader (no `temporal`) is immutable.
//!
//! Rung 0 lives here as its own function rather than inside [`decide`] because
//! it needs the filesystem, and [`decide`] is pure; `cache::try_hit` applies it
//! to the entries [`decide`] has already called a hit. [`SourceRevalidator`]
//! wraps it with the fingerprint memo that keeps a per-tick read loop from
//! re-hashing the same unchanged file on every call.
//!
//! Offline mode short-circuits all of this to presence + stored hash; that path
//! lives in `cache` and never calls [`decide`].

use std::collections::HashMap;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

use crate::manifest::Manifest;

/// A loader's temporal nature, supplying the TTL rung of the ladder. Lets a
/// present blob be judged valid **without** a network round-trip.
#[derive(Debug, Clone)]
pub enum Temporal {
    /// No `temporal` block: immutable once fetched (static loaders).
    Static,
    /// A closed past period (e.g. `file_period:P1D` for a past date): immutable.
    ClosedPeriod,
    /// A current / incomplete period: refresh after `ttl` elapses.
    Incomplete {
        /// How long a fetched blob stays fresh before revalidation.
        ttl: Duration,
    },
}

impl Temporal {
    /// Is a blob fetched at `fetched_at` (RFC 3339) still fresh by TTL alone?
    ///
    /// Static and closed periods are always fresh. An incomplete period is fresh
    /// until its TTL elapses; a fetch timestamp in the future (clock skew) is
    /// treated as just-fetched. An unparseable timestamp forces revalidation
    /// rather than silently trusting it.
    pub fn is_fresh(&self, fetched_at: &str, now: OffsetDateTime) -> bool {
        match self {
            Temporal::Static | Temporal::ClosedPeriod => true,
            Temporal::Incomplete { ttl } => match OffsetDateTime::parse(fetched_at, &Rfc3339) {
                Ok(fetched) => {
                    let elapsed = now - fetched;
                    elapsed.is_negative() || elapsed.unsigned_abs() < *ttl
                }
                Err(_) => false,
            },
        }
    }
}

/// What to do with a present cache entry, after consulting the ladder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheDecision {
    /// Present and valid — reuse without touching the network.
    Hit,
    /// Present but must revalidate over the network (conditional GET).
    Revalidate,
    /// Absent or known-stale — must (re)download.
    Miss,
}

/// Apply the validation ladder to a present manifest. `expected_checksum` is a
/// loader-declared content hash (none today); `temporal` is the loader's
/// temporal nature (None ⇒ static/immutable).
pub fn decide(
    manifest: &Manifest,
    temporal: Option<&Temporal>,
    expected_checksum: Option<&str>,
) -> CacheDecision {
    decide_at(
        manifest,
        temporal,
        expected_checksum,
        OffsetDateTime::now_utc(),
    )
}

/// [`decide`] with an injectable clock (for deterministic tests).
pub fn decide_at(
    manifest: &Manifest,
    temporal: Option<&Temporal>,
    expected_checksum: Option<&str>,
    now: OffsetDateTime,
) -> CacheDecision {
    // 1. Loader-declared checksum is the strongest signal.
    if let Some(expected) = expected_checksum {
        return if expected.eq_ignore_ascii_case(&manifest.sha256_content) {
            CacheDecision::Hit
        } else {
            CacheDecision::Miss
        };
    }
    // 2. Declared immutability. A static or closed-period source cannot change,
    //    so a conditional GET can only ever answer "unchanged" — a network
    //    round-trip whose result is known in advance. This MUST stay above rule
    //    3: S3 returns an ETag on EVERY object, so with the validators first
    //    this arm is unreachable for any S3-backed store and every warm cache
    //    hit pays a round-trip to be told nothing. Measured on the ISRM store,
    //    85.9 ms/chunk before against a 0.078 ms raw file read.
    if matches!(
        temporal,
        None | Some(Temporal::Static) | Some(Temporal::ClosedPeriod)
    ) {
        return CacheDecision::Hit;
    }
    // 3. Conditional validators ⇒ revalidate over the network. Validators beat
    //    the TTL HEURISTIC below, but not the DECLARATION above.
    if manifest.etag.is_some() || manifest.last_modified.is_some() {
        return CacheDecision::Revalidate;
    }
    // 4. TTL from temporal (incomplete period, no validators present).
    match temporal {
        None | Some(Temporal::Static) | Some(Temporal::ClosedPeriod) => CacheDecision::Hit,
        Some(t @ Temporal::Incomplete { .. }) => {
            if t.is_fresh(&manifest.fetched_at, now) {
                CacheDecision::Hit
            } else {
                CacheDecision::Miss
            }
        }
    }
}

/// Environment variable that turns the `file://` source recheck
/// ([`file_source_is_current`]) **off**. Set it to `0`/`false`/`no`/`off`.
///
/// The recheck is ON by default and that default is the fix for
/// `EarthSciML/EarthSciAST#293`; this exists only for a caller who knows its
/// local corpus is immutable and does not want to pay one extra read of the
/// bytes it is about to read anyway. Any other value (including an unparseable
/// one) leaves the recheck ON — a typo in a knob must not silently restore a
/// silent-staleness bug.
pub const REVALIDATE_FILE_ENV: &str = "EARTHSCI_REVALIDATE_FILE";

/// Resolve whether `file://` sources are rechecked: the explicit argument wins,
/// otherwise [`REVALIDATE_FILE_ENV`], otherwise `true`.
pub fn revalidate_file_sources(explicit: Option<bool>) -> bool {
    match explicit {
        Some(v) => v,
        None => std::env::var(REVALIDATE_FILE_ENV)
            .map(|s| !is_falsey(&s))
            .unwrap_or(true),
    }
}

/// The only values that switch the recheck off (case-insensitive, trimmed).
fn is_falsey(s: &str) -> bool {
    matches!(
        s.trim().to_ascii_lowercase().as_str(),
        "0" | "false" | "no" | "off"
    )
}

/// What rung 0 learned about the `file://` source behind a cached entry.
///
/// A boolean cannot carry this. "The corpus was deleted", "the corpus was
/// replaced" and "this host cannot read the corpus" are three different facts,
/// and `cache::try_hit` acts differently on each — in particular it must not
/// turn the third into a re-ingest, because a path that is merely unreachable
/// from this node (no permission, an unmounted filesystem) tells us **nothing**
/// about whether the bytes changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceState {
    /// Byte-for-byte the file the manifest records. Serve the warm blob.
    Current,
    /// Present, and a different file — a different length, or the same length
    /// and a different hash. Re-ingest.
    Replaced,
    /// Nothing is at that path, or something that is not a regular file is.
    /// Re-ingest, which lets the transport raise the real absence — unless a
    /// mirror may have been what served the blob, which is the caller's call.
    Missing,
    /// The source could not be consulted at all: no permission, an I/O error, a
    /// filesystem that is not mounted here. Nothing was learned, so rung 0
    /// **abstains** and the entry is served as the ladder above decided.
    Unknown,
}

/// `stat` the source and classify what came back.
///
/// The split that matters is `NotFound` (the file is gone — the report's
/// sharpest case, and it must stay a loud error) against every other error
/// (`PermissionDenied`, `NotADirectory`, an I/O failure — this host cannot see
/// the corpus, which is not evidence that it changed). Warming a cache where
/// `/corpus` is mounted and reading it where it is not must not be a hard
/// failure.
fn stat_source(source: &Path) -> Result<std::fs::Metadata, SourceState> {
    match std::fs::metadata(source) {
        Ok(meta) if meta.is_file() => Ok(meta),
        // A directory standing where the corpus used to be is the report's
        // "pointed at a path that is not there any more" shape.
        Ok(_) => Err(SourceState::Missing),
        Err(e) if e.kind() == ErrorKind::NotFound => Err(SourceState::Missing),
        Err(_) => Err(SourceState::Unknown),
    }
}

/// What `stat` said about a source, and the digest computed for that exact
/// `stat`. The digest is of the **source**, not of any manifest, so it stays
/// usable after a re-ingest rewrites the manifest.
#[derive(Debug, Clone)]
struct Fingerprint {
    len: u64,
    mtime: std::time::SystemTime,
    sha256: String,
    /// Wall clock taken just before the digest was computed.
    hashed_at: std::time::SystemTime,
}

/// How many source fingerprints one revalidator remembers before dropping the
/// lot. A read loop touches a handful of files; this only has to stop an
/// unbounded walk from growing the map without limit.
const MEMO_CAP: usize = 512;

/// Git's racy-timestamp rule. A filesystem records mtime only to its
/// granularity: 1 s on Lustre, ext3, HFS+ and many NFS servers, 2 s on FAT. A
/// same-length write in the same tick as an earlier hash keeps the same
/// `(length, mtime)`, so a digest taken within one tick of the mtime cannot
/// vouch for later bytes. Any later write sharing a 2 s FAT bucket lands before
/// `mtime + 2 s`, so 2 s with an inclusive comparison covers it.
const RACY_MARGIN: Duration = Duration::from_secs(2);

/// Whether a digest taken at `hashed_at` is too close to `mtime` to trust.
fn is_racy(mtime: std::time::SystemTime, hashed_at: std::time::SystemTime) -> bool {
    match hashed_at.checked_sub(RACY_MARGIN) {
        Some(safe_before) => mtime >= safe_before,
        None => true,
    }
}

/// Rung 0 plus the fingerprint memo that makes it affordable to run on every
/// read (`spec/cache-format.md` §4.1).
///
/// Hashing the source on every `fetch` is what rung 0 costs, and on the
/// record-selective read paths `fetch` is called **per tick**, not per file
/// (`provider.py::_file_for` bypasses the decoded-file buffer whenever a
/// `select` is passed, and the Julia provider keeps no such buffer at all). A
/// 2.8 GB corpus would be re-hashed for every tick of the run.
///
/// So a digest is remembered against the `(length, mtime)` the source had when
/// it was computed, and reused while both are unchanged — the same bargain
/// `make`, `ninja` and `rsync` strike. The memo lives on the [`Cache`] and dies
/// with it, so a fresh process always pays one real hash per file; what it
/// removes is the *repeat* hash within a run.
///
/// [`Cache`]: crate::cache::Cache
///
/// A digest taken while the source's mtime was within [`RACY_MARGIN`] of the
/// moment of hashing is never reused (git's "racy timestamp" rule): the next
/// check hashes again, and only a check that sees the mtime safely in the past
/// is trusted on later reads.
///
/// **What it can still miss:** a replacement that preserves both the length and
/// the mtime, where that mtime is already more than the margin old, in-process,
/// after the file was already read once. `cp --preserve=timestamps` of an old
/// file over a same-length one can do it. The window is one process lifetime,
/// and `EARTHSCI_REVALIDATE_FILE` is unrelated to it — a caller that cannot
/// accept the window wants a fresh `Cache`.
#[derive(Debug)]
pub struct SourceRevalidator {
    seen: Mutex<HashMap<PathBuf, Fingerprint>>,
    full_reads: AtomicU64,
}

impl Default for SourceRevalidator {
    fn default() -> Self {
        Self::new()
    }
}

impl SourceRevalidator {
    /// An empty memo.
    pub fn new() -> Self {
        Self {
            seen: Mutex::new(HashMap::new()),
            full_reads: AtomicU64::new(0),
        }
    }

    /// How many times a source has actually been read end-to-end. Tests assert
    /// the memo saves the repeat reads; nothing else should depend on it.
    pub fn full_reads(&self) -> u64 {
        self.full_reads.load(Ordering::Relaxed)
    }

    /// Rung 0: what is the state of `source` relative to `manifest`?
    pub fn state(&self, source: &Path, manifest: &Manifest) -> SourceState {
        let meta = match stat_source(source) {
            Ok(meta) => meta,
            Err(state) => return state,
        };
        // A different length is a different file, and no read is needed to say
        // so. This is the cheap half of the check and every track keeps it.
        if meta.len() != manifest.bytes {
            return SourceState::Replaced;
        }
        let mtime = meta.modified().ok();
        if let Some(sha) = self.remembered(source, meta.len(), mtime) {
            return verdict(&sha, manifest);
        }
        let hashed_at = std::time::SystemTime::now();
        let sha = match crate::key::sha256_file(source) {
            Ok(sha) => sha,
            // It survived `stat` but not `open`. Absence is still absence;
            // anything else is still "cannot tell".
            Err(e) if e.kind() == ErrorKind::NotFound => return SourceState::Missing,
            Err(_) => return SourceState::Unknown,
        };
        self.full_reads.fetch_add(1, Ordering::Relaxed);
        self.remember(source, meta.len(), mtime, &sha, hashed_at);
        verdict(&sha, manifest)
    }

    /// The digest remembered for this exact `(len, mtime)`, if any, and only if
    /// it was taken safely after that mtime (see [`RACY_MARGIN`]). A source
    /// whose mtime the filesystem will not report is never remembered — there
    /// would be no way to notice it had changed.
    fn remembered(
        &self,
        source: &Path,
        len: u64,
        mtime: Option<std::time::SystemTime>,
    ) -> Option<String> {
        let mtime = mtime?;
        let seen = self.seen.lock().ok()?;
        let fp = seen.get(source)?;
        (fp.len == len && fp.mtime == mtime && !is_racy(fp.mtime, fp.hashed_at))
            .then(|| fp.sha256.clone())
    }

    fn remember(
        &self,
        source: &Path,
        len: u64,
        mtime: Option<std::time::SystemTime>,
        sha: &str,
        hashed_at: std::time::SystemTime,
    ) {
        let Some(mtime) = mtime else { return };
        let Ok(mut seen) = self.seen.lock() else { return };
        // Crude but sufficient: the memo is an optimisation, so dropping all of
        // it costs one re-hash per live file rather than needing an LRU.
        if seen.len() >= MEMO_CAP {
            seen.clear();
        }
        seen.insert(
            source.to_path_buf(),
            Fingerprint {
                len,
                mtime,
                sha256: sha.to_string(),
                hashed_at,
            },
        );
    }
}

fn verdict(sha: &str, manifest: &Manifest) -> SourceState {
    if sha.eq_ignore_ascii_case(&manifest.sha256_content) {
        SourceState::Current
    } else {
        SourceState::Replaced
    }
}

/// Rung 0 without a memo: is the `file://` source behind a cached entry still
/// the file that was ingested into it?
///
/// The cache is keyed by the resolved URL, so a local file replaced **in place**
/// keeps the same key and every later read is served the bytes of the file that
/// used to be there. Nothing above catches it: a `file://` source has no ETag
/// and no `Last-Modified`, and a source with no `temporal` is declared
/// immutable (rule 2), so [`decide`] answers `Hit` forever. That is
/// `EarthSciML/EarthSciAST#293`, where a snapshot corpus was replaced at the
/// same paths and a test suite stayed green against the corpus that no longer
/// existed — the manifest recorded 371 bytes while the file on disk was 363.
///
/// Note what this is NOT. `Cache::verify_on_read` hashes the **cached blob**
/// against the manifest; it compares the copy with the record of the copy, so
/// it passes with flying colours while the file the copy was made from has been
/// replaced. This compares the **source** with that record — the check whose
/// absence the issue reports.
///
/// The manifest already carries everything needed, so this is not a cache-format
/// change:
///
/// * nothing at the path, or not a regular file ⇒ [`SourceState::Missing`];
/// * the path cannot be read at all ⇒ [`SourceState::Unknown`], and rung 0
///   abstains rather than claim a change it did not observe;
/// * on-disk length != `manifest.bytes` ⇒ [`SourceState::Replaced`], no hashing;
/// * `sha256(source)` != `manifest.sha256_content` ⇒ [`SourceState::Replaced`].
///   Size alone would not do: a float re-encode, a different simulation year, or
///   any edit that preserves the length is exactly the case a size check waves
///   through;
/// * otherwise [`SourceState::Current`] — serve the warm blob, re-ingest nothing.
///
/// A [`Cache`] calls [`SourceRevalidator::state`] instead, which is this with a
/// `(length, mtime)` memo in front of the hash; use this when there is nothing
/// to amortise over.
///
/// [`Cache`]: crate::cache::Cache
pub fn file_source_state(source: &Path, manifest: &Manifest) -> SourceState {
    SourceRevalidator::new().state(source, manifest)
}

/// Rung 0 as a yes/no: is the source **provably** the file that was ingested?
///
/// Only [`SourceState::Current`] is `true`. The cache does not use this — the
/// difference between `Missing`, `Replaced` and `Unknown` is exactly what it
/// acts on — but it is the honest predicate for a caller that just wants the
/// question answered.
pub fn file_source_is_current(source: &Path, manifest: &Manifest) -> bool {
    file_source_state(source, manifest) == SourceState::Current
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest(etag: Option<&str>, fetched_at: &str) -> Manifest {
        Manifest {
            auth_realm: None,
            bytes: 3,
            etag: etag.map(String::from),
            fetched_at: fetched_at.to_string(),
            last_modified: None,
            schema: crate::manifest::MANIFEST_SCHEMA.to_string(),
            sha256_content: "abc123".to_string(),
            source_loader: None,
            url: "https://x/y".to_string(),
        }
    }

    fn t(s: &str) -> OffsetDateTime {
        OffsetDateTime::parse(s, &Rfc3339).unwrap()
    }

    #[test]
    fn declared_checksum_match_is_hit() {
        let m = manifest(None, "2026-01-01T00:00:00Z");
        assert_eq!(decide(&m, None, Some("ABC123")), CacheDecision::Hit); // case-insensitive
        assert_eq!(decide(&m, None, Some("deadbeef")), CacheDecision::Miss);
    }

    /// Rule 2 beats rule 3 — the regression guard for the ordering fix.
    ///
    /// S3 returns an ETag on EVERY object, so with validators checked first no
    /// S3-backed store could ever take the immutable path, and every warm cache
    /// hit paid a network round-trip to be told "unchanged". Measured on the
    /// ISRM zarr store: 85.9 ms per chunk against a 0.078 ms local read.
    #[test]
    fn an_immutable_source_is_never_revalidated_even_with_validators() {
        let m = manifest(Some("\"v1\""), "2026-01-01T00:00:00Z");
        assert_eq!(decide(&m, None, None), CacheDecision::Hit);
        assert_eq!(
            decide(&m, Some(&Temporal::Static), None),
            CacheDecision::Hit
        );
        assert_eq!(
            decide(&m, Some(&Temporal::ClosedPeriod), None),
            CacheDecision::Hit
        );
    }

    /// Rule 3 beats rule 4: only the DECLARATION outranks a conditional GET.
    #[test]
    fn validators_still_beat_the_ttl_heuristic() {
        let m = manifest(Some("\"v1\""), "2026-01-01T00:00:00Z");
        let temporal = Temporal::Incomplete {
            ttl: Duration::from_secs(3600),
        };
        assert_eq!(
            decide(&m, Some(&temporal), None),
            CacheDecision::Revalidate
        );
    }

    #[test]
    fn static_and_closed_are_immutable_hits() {
        let m = manifest(None, "2000-01-01T00:00:00Z");
        assert_eq!(decide(&m, None, None), CacheDecision::Hit);
        assert_eq!(
            decide(&m, Some(&Temporal::Static), None),
            CacheDecision::Hit
        );
        assert_eq!(
            decide(&m, Some(&Temporal::ClosedPeriod), None),
            CacheDecision::Hit
        );
    }

    #[test]
    fn incomplete_period_respects_ttl() {
        let temporal = Temporal::Incomplete {
            ttl: Duration::from_secs(3600),
        };
        let m = manifest(None, "2026-06-26T00:00:00Z");
        // 30 min later: still fresh.
        assert_eq!(
            decide_at(&m, Some(&temporal), None, t("2026-06-26T00:30:00Z")),
            CacheDecision::Hit
        );
        // 2 h later: stale ⇒ refetch.
        assert_eq!(
            decide_at(&m, Some(&temporal), None, t("2026-06-26T02:00:00Z")),
            CacheDecision::Miss
        );
    }

    // --- rung 0: the file:// source recheck (EarthSciML/EarthSciAST#293) ------

    /// A manifest describing `body` as the ingested bytes.
    fn file_manifest(body: &[u8]) -> Manifest {
        Manifest {
            auth_realm: None,
            bytes: body.len() as u64,
            etag: None,
            fetched_at: "2026-01-01T00:00:00Z".to_string(),
            last_modified: None,
            schema: crate::manifest::MANIFEST_SCHEMA.to_string(),
            sha256_content: crate::key::sha256_hex(body),
            source_loader: None,
            url: "file:///corpus/x.csv".to_string(),
        }
    }

    #[test]
    fn unchanged_source_is_current() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("x.csv");
        std::fs::write(&src, b"year,value\n2016,1.0\n").unwrap();
        let m = file_manifest(b"year,value\n2016,1.0\n");
        assert!(file_source_is_current(&src, &m));
    }

    #[test]
    fn replaced_source_of_different_length_is_not_current() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("x.csv");
        let m = file_manifest(b"371-bytes-worth-of-old-corpus");
        std::fs::write(&src, b"363-bytes-worth").unwrap();
        assert!(!file_source_is_current(&src, &m));
    }

    /// The half a size check waves through: same length, different bytes — a
    /// float re-encode, a different simulation year, a corrected code.
    #[test]
    fn replaced_source_of_equal_length_is_not_current() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("x.csv");
        let m = file_manifest(b"year,value\n2016,1.0\n");
        std::fs::write(&src, b"year,value\n2016,9.0\n").unwrap();
        assert_eq!(
            std::fs::metadata(&src).unwrap().len(),
            m.bytes,
            "the point of this test is that the LENGTH still matches"
        );
        assert!(!file_source_is_current(&src, &m));
    }

    #[test]
    fn deleted_source_is_not_current() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("gone.csv");
        let m = file_manifest(b"whatever");
        assert!(!file_source_is_current(&src, &m));
    }

    /// A path that resolves to a directory (the report's "pointed at a path
    /// that is not there any more" shape) is not a source to serve from.
    #[test]
    fn directory_in_place_of_source_is_not_current() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("subdir");
        std::fs::create_dir(&src).unwrap();
        let m = file_manifest(b"whatever");
        assert!(!file_source_is_current(&src, &m));
    }

    #[test]
    fn revalidation_is_on_by_default_and_only_explicit_falsey_disables_it() {
        assert!(revalidate_file_sources(Some(true)));
        assert!(!revalidate_file_sources(Some(false)));
        for v in ["0", "false", "NO", " off "] {
            assert!(is_falsey(v), "{v:?} should disable the recheck");
        }
        // Anything else — including a typo — keeps the recheck ON.
        for v in ["1", "true", "yes", "", "offf", "maybe"] {
            assert!(!is_falsey(v), "{v:?} must NOT disable the recheck");
        }
    }

    #[test]
    fn unparseable_timestamp_forces_refetch() {
        let temporal = Temporal::Incomplete {
            ttl: Duration::from_secs(3600),
        };
        let m = manifest(None, "not-a-date");
        assert!(!temporal.is_fresh("not-a-date", t("2026-06-26T00:00:00Z")));
        assert_eq!(
            decide_at(&m, Some(&temporal), None, t("2026-06-26T00:00:00Z")),
            CacheDecision::Miss
        );
    }
}
