//! UTC timestamps for the manifest `fetched_at` field.
//!
//! Emitted as second-resolution RFC 3339 with a `Z` suffix
//! (e.g. `2026-06-26T00:00:00Z`) to match the corpus style. Parsing for the TTL
//! rung (`validate`) accepts any RFC 3339 form, including fractional seconds and
//! `+00:00`, so manifests written by other tracks are still understood.

use time::format_description::FormatItem;
use time::macros::format_description;
use time::OffsetDateTime;

/// Second-resolution UTC stamp: `YYYY-MM-DDThh:mm:ssZ`.
const STAMP: &[FormatItem<'static>] =
    format_description!("[year]-[month]-[day]T[hour]:[minute]:[second]Z");

/// Current UTC time as an RFC 3339 `fetched_at` string.
pub fn now_rfc3339() -> String {
    OffsetDateTime::now_utc()
        .format(STAMP)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::format_description::well_known::Rfc3339;

    #[test]
    fn now_is_parseable_rfc3339() {
        let s = now_rfc3339();
        assert!(s.ends_with('Z'), "expected Z suffix, got {s}");
        assert_eq!(s.len(), 20, "expected second-resolution stamp, got {s}");
        // Round-trips through a strict RFC 3339 parser.
        assert!(OffsetDateTime::parse(&s, &Rfc3339).is_ok());
    }
}
