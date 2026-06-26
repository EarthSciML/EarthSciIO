//! `$EARTHSCIDATADIR` resolution and `file://` template expansion.
//!
//! The cache root is `$EARTHSCIDATADIR` (`spec/cache-format.md` §5): the
//! environment variable **always wins**; the default is only a fallback and
//! lives on `/scratch.local`, **never `/u`** — the home inode quota cannot
//! absorb many small NetCDF slices (Risk R6). This is a hard rule.

use std::path::{Path, PathBuf};

/// Environment variable that overrides the cache root.
pub const DATADIR_ENV: &str = "EARTHSCIDATADIR";

/// Resolve the cache root `$EARTHSCIDATADIR`.
///
/// `EARTHSCIDATADIR` wins when set and non-empty; otherwise
/// `/scratch.local/$USER/earthsci-cache`.
pub fn data_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os(DATADIR_ENV) {
        if !dir.is_empty() {
            return PathBuf::from(dir);
        }
    }
    default_data_dir()
}

/// The fallback cache root on `/scratch.local` (never the home filesystem).
pub fn default_data_dir() -> PathBuf {
    let user = std::env::var("USER")
        .or_else(|_| std::env::var("LOGNAME"))
        .unwrap_or_else(|_| "shared".to_string());
    PathBuf::from(format!("/scratch.local/{user}/earthsci-cache"))
}

/// Expand `${EARTHSCIDATADIR}` / `$EARTHSCIDATADIR` inside a `file://` mirror
/// template against the env-resolved root (the `nei2016` pattern,
/// `spec/cache-format.md` §5). Only this one variable is expanded — there is no
/// general shell expansion.
pub fn expand_datadir(template: &str) -> String {
    expand_datadir_with(template, &data_dir())
}

/// Expand `${EARTHSCIDATADIR}` against an explicit root (so the expansion tracks
/// the active store root rather than only the environment).
pub fn expand_datadir_with(template: &str, root: &Path) -> String {
    if !template.contains("EARTHSCIDATADIR") {
        return template.to_string();
    }
    let root = root.to_string_lossy();
    template
        .replace("${EARTHSCIDATADIR}", &root)
        .replace("$EARTHSCIDATADIR", &root)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_env_wins() {
        // Use a guarded scope; serialized via the env mutex in tests/util is not
        // needed here because we read a unique key we set ourselves.
        let prev = std::env::var_os(DATADIR_ENV);
        std::env::set_var(DATADIR_ENV, "/scratch.local/somebody/cache");
        assert_eq!(data_dir(), PathBuf::from("/scratch.local/somebody/cache"));
        // restore
        match prev {
            Some(v) => std::env::set_var(DATADIR_ENV, v),
            None => std::env::remove_var(DATADIR_ENV),
        }
    }

    #[test]
    fn default_is_on_scratch_never_home() {
        let d = default_data_dir();
        assert!(
            d.starts_with("/scratch.local"),
            "default must live on /scratch.local, got {d:?}"
        );
        assert!(!d.starts_with("/u"), "default must never live under /u");
        assert!(d.ends_with("earthsci-cache"));
    }

    #[test]
    fn expands_template_var() {
        let root = Path::new("/scratch.local/u/earthsci-cache");
        assert_eq!(
            expand_datadir_with("file://${EARTHSCIDATADIR}/nei2016/x.nc", root),
            "file:///scratch.local/u/earthsci-cache/nei2016/x.nc"
        );
        assert_eq!(
            expand_datadir_with("file://$EARTHSCIDATADIR/x.nc", root),
            "file:///scratch.local/u/earthsci-cache/x.nc"
        );
    }

    #[test]
    fn leaves_untemplated_untouched() {
        let root = Path::new("/whatever");
        assert_eq!(
            expand_datadir_with("file:///abs/path.nc", root),
            "file:///abs/path.nc"
        );
    }
}
