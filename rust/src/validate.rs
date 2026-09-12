//! The cache validation ladder (`spec/cache-format.md` §4).
//!
//! A cache **hit** requires the blob to be present **and valid**. Validity is
//! decided in this order (first applicable wins):
//!
//! 0. **Local source recheck** — a `file://` source is compared against the
//!    file it was ingested from ([`file_source_is_current`]). Rules 1-3 are all
//!    about a source that can only be consulted over the network; a local file
//!    has an exact truth sitting on disk.
//! 1. **Content hash** — a loader-declared checksum (none today) beats everything.
//! 2. **Conditional GET** — stored `etag`/`last_modified` ⇒ revalidate over the
//!    network (`If-None-Match` / `If-Modified-Since`).
//! 3. **TTL from `temporal`** — a closed past period is immutable; an incomplete
//!    period has a short TTL; a static loader (no `temporal`) is immutable.
//!
//! Rung 0 lives here as its own function rather than inside [`decide`] because
//! it needs the filesystem, and [`decide`] is pure; `cache::try_hit` applies it
//! to the entries [`decide`] has already called a hit.
//!
//! Offline mode short-circuits all of this to presence + stored hash; that path
//! lives in `cache` and never calls [`decide`].

use std::path::Path;
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

/// Rung 0: is the `file://` source behind a cached entry still the file that
/// was ingested into it?
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
/// * source missing, unreadable, or not a regular file ⇒ **not current** (the
///   caller re-ingests, and the transport then raises the real error instead of
///   the cache serving a ghost);
/// * on-disk length != `manifest.bytes` ⇒ **not current**, no hashing;
/// * `sha256(source)` != `manifest.sha256_content` ⇒ **not current**. Size alone
///   would not do: a float re-encode, a different simulation year, or any edit
///   that preserves the length is exactly the case a size check waves through;
/// * otherwise **current** — serve the warm blob, re-ingest nothing.
///
/// Cost is one read of the source. It is paid only for `file://` entries, and
/// only for entries that were about to be served, i.e. it is bounded by the
/// bytes the caller was already about to read. [`REVALIDATE_FILE_ENV`] turns it
/// off for a caller that would rather have the staleness than the read.
pub fn file_source_is_current(source: &Path, manifest: &Manifest) -> bool {
    let Ok(meta) = std::fs::metadata(source) else {
        // Gone, or unreadable. Either way this entry must not be served: the
        // report's sharpest case is a corpus DELETED outright while the suite
        // kept passing.
        return false;
    };
    if !meta.is_file() || meta.len() != manifest.bytes {
        return false;
    }
    match crate::key::sha256_file(source) {
        Ok(sha) => sha.eq_ignore_ascii_case(&manifest.sha256_content),
        // Unreadable half-way through ⇒ re-ingest and let the transport speak.
        Err(_) => false,
    }
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
