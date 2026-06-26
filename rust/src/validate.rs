//! The cache validation ladder (`spec/cache-format.md` §4).
//!
//! A cache **hit** requires the blob to be present **and valid**. Validity is
//! decided in this order (first applicable wins):
//!
//! 1. **Content hash** — a loader-declared checksum (none today) beats everything.
//! 2. **Conditional GET** — stored `etag`/`last_modified` ⇒ revalidate over the
//!    network (`If-None-Match` / `If-Modified-Since`).
//! 3. **TTL from `temporal`** — a closed past period is immutable; an incomplete
//!    period has a short TTL; a static loader (no `temporal`) is immutable.
//!
//! Offline mode short-circuits all of this to presence + stored hash; that path
//! lives in `cache` and never calls [`decide`].

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
    // 2. Conditional validators ⇒ revalidate over the network.
    if manifest.etag.is_some() || manifest.last_modified.is_some() {
        return CacheDecision::Revalidate;
    }
    // 3. TTL from temporal (no validators present).
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

    #[test]
    fn etag_forces_revalidate() {
        let m = manifest(Some("\"v1\""), "2026-01-01T00:00:00Z");
        assert_eq!(
            decide(&m, Some(&Temporal::Static), None),
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
