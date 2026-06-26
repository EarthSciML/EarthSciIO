//! Offline-mode detection (`spec/offline-mode.md` §1).
//!
//! Offline is on when **either** the provider is constructed with
//! `offline = true`, **or** `EARTHSCI_OFFLINE` is truthy (`1`/`true`/`yes`,
//! case-insensitive). The explicit argument wins over the environment.

/// Environment variable that forces cache-only (offline) mode.
pub const OFFLINE_ENV: &str = "EARTHSCI_OFFLINE";

/// Resolve the effective offline flag: the explicit argument wins; otherwise
/// fall back to `EARTHSCI_OFFLINE`.
pub fn is_offline(explicit: Option<bool>) -> bool {
    match explicit {
        Some(v) => v,
        None => std::env::var(OFFLINE_ENV)
            .map(|s| is_truthy(&s))
            .unwrap_or(false),
    }
}

/// Truthy values for `EARTHSCI_OFFLINE` (case-insensitive, trimmed).
fn is_truthy(s: &str) -> bool {
    matches!(s.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_true_wins_over_unset_env() {
        assert!(is_offline(Some(true)));
    }

    #[test]
    fn explicit_false_wins_over_truthy_env() {
        let prev = std::env::var_os(OFFLINE_ENV);
        std::env::set_var(OFFLINE_ENV, "1");
        // Explicit false overrides a truthy environment.
        assert!(!is_offline(Some(false)));
        match prev {
            Some(v) => std::env::set_var(OFFLINE_ENV, v),
            None => std::env::remove_var(OFFLINE_ENV),
        }
    }

    #[test]
    fn truthy_table() {
        for v in ["1", "true", "TRUE", "Yes", " yes "] {
            assert!(is_truthy(v), "{v:?} should be truthy");
        }
        for v in ["0", "false", "no", "", "off"] {
            assert!(!is_truthy(v), "{v:?} should be falsey");
        }
    }
}
