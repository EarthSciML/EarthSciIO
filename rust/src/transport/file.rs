//! The `file` transport: copies a local file into the cache
//! (`spec/registries.md` §1). Expands `${EARTHSCIDATADIR}` in the path so a
//! pre-populated local mirror is found (`spec/cache-format.md` §5). No
//! conditional GET / auth — local files have no HTTP validators.

use std::path::{Path, PathBuf};

use super::{Conditional, FetchResult, FetchStatus, Transport};
use crate::auth::AuthResolver;
use crate::datadir::expand_datadir;
use crate::error::{Error, Result};

/// `file://` transport.
pub struct FileTransport;

impl FileTransport {
    /// Construct the file transport.
    pub fn new() -> Self {
        Self
    }
}

impl Default for FileTransport {
    fn default() -> Self {
        Self::new()
    }
}

impl Transport for FileTransport {
    fn schemes(&self) -> &'static [&'static str] {
        &["file"]
    }

    fn fetch(
        &self,
        url: &str,
        dest: &Path,
        _conditional: &Conditional,
        _auth: Option<&dyn AuthResolver>,
    ) -> Result<FetchResult> {
        let src = file_url_to_path(url)?;
        let bytes_written =
            std::fs::copy(&src, dest).map_err(|e| Error::io(Some(src.clone()), e))?;
        Ok(FetchResult {
            status: FetchStatus::Downloaded,
            etag: None,
            last_modified: None,
            bytes_written,
        })
    }
}

/// Turn a `file://` URL into a local path, expanding `${EARTHSCIDATADIR}` first.
///
/// Handles `file:///abs/path` (empty authority) and `file://host/abs/path`
/// (authority dropped). The path is taken literally after expansion — no
/// percent-decoding (the resolved URLs the cache produces are not encoded).
fn file_url_to_path(url: &str) -> Result<PathBuf> {
    let expanded = expand_datadir(url);
    let rest = expanded
        .strip_prefix("file://")
        .ok_or_else(|| Error::BadUrl {
            url: url.to_string(),
            detail: "not a file:// URL".to_string(),
        })?;

    let path = if let Some(stripped) = rest.strip_prefix('/') {
        // `file:///abs` ⇒ rest = "/abs" ⇒ keep the leading slash.
        format!("/{stripped}")
    } else {
        // `file://host/abs` ⇒ drop the authority, path starts at the first '/'.
        match rest.find('/') {
            Some(i) => rest[i..].to_string(),
            None => {
                return Err(Error::BadUrl {
                    url: url.to_string(),
                    detail: "file:// URL has no path".to_string(),
                })
            }
        }
    };
    Ok(PathBuf::from(path))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_triple_slash_abs_path() {
        assert_eq!(
            file_url_to_path("file:///scratch/x.nc").unwrap(),
            PathBuf::from("/scratch/x.nc")
        );
    }

    #[test]
    fn drops_authority() {
        assert_eq!(
            file_url_to_path("file://localhost/scratch/x.nc").unwrap(),
            PathBuf::from("/scratch/x.nc")
        );
    }

    #[test]
    fn rejects_non_file_url() {
        assert!(file_url_to_path("https://x/y").is_err());
    }
}
