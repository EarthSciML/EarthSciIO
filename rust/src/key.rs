//! The shared content-addressed cache key and content hashing.
//!
//! `key = lowercase_hex(sha256(utf8(resolved_url)))` (`spec/cache-format.md` §1).
//! The URL is hashed **exactly as resolved** — no normalization, no trailing
//! newline. All three language tracks MUST hash the identical byte string so a
//! blob fetched by one is reused, byte-for-byte, by the others.

use std::path::Path;

use sha2::{Digest, Sha256};

/// Compute the cache key for a resolved URL: `sha256(utf8(url))`, lowercase hex.
///
/// ```
/// assert_eq!(
///     earthsciio::cache_key("https://data.earthsci.dev/era5/2018/11/20181108.nc"),
///     "11cdcec111409f586e6afc432e1a6da47e6f97ccf3715e5db8554632b00671c1",
/// );
/// ```
pub fn cache_key(resolved_url: &str) -> String {
    hex_lower(&Sha256::digest(resolved_url.as_bytes()))
}

/// Cache key for a byte sub-range request. The spec appends `#bytes=<a>-<b>` to
/// the URL **before** hashing, so a sub-slice is its own cache entry
/// (`spec/cache-format.md` §1).
pub fn cache_key_range(resolved_url: &str, start: u64, end: u64) -> String {
    cache_key(&format!("{resolved_url}#bytes={start}-{end}"))
}

/// sha256 of a byte slice as lowercase hex (the manifest's `sha256_content`).
pub fn sha256_hex(data: &[u8]) -> String {
    hex_lower(&Sha256::digest(data))
}

/// Stream a file through sha256 without loading it fully into memory. Used for
/// integrity re-verification (cheap; off by default, on for CI/conformance).
pub fn sha256_file(path: &Path) -> std::io::Result<String> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    std::io::copy(&mut file, &mut hasher)?;
    Ok(hex_lower(&hasher.finalize()))
}

fn hex_lower(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(s, "{b:02x}");
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn era5_key_matches_corpus() {
        // Worked example pinned in spec/cache-format.md §1.
        assert_eq!(
            cache_key("https://data.earthsci.dev/era5/2018/11/20181108.nc"),
            "11cdcec111409f586e6afc432e1a6da47e6f97ccf3715e5db8554632b00671c1"
        );
    }

    #[test]
    fn openaq_key_matches_corpus() {
        assert_eq!(
            cache_key(
                "https://openaq-data-archive.s3.amazonaws.com/records/openaq/locationid=1/2018-11-08.csv"
            ),
            "69dd26b950e43cb2182e3b4d02e89e09bfb798b13469183ca2dad15c5794379a"
        );
    }

    #[test]
    fn key_is_exact_bytes_no_newline() {
        // A trailing newline must change the key — proves "no normalization".
        assert_ne!(cache_key("https://x/y"), cache_key("https://x/y\n"));
    }

    #[test]
    fn range_key_differs_from_whole() {
        let url = "https://x/y.nc";
        assert_ne!(cache_key(url), cache_key_range(url, 0, 1023));
        assert_eq!(
            cache_key_range(url, 0, 1023),
            cache_key("https://x/y.nc#bytes=0-1023")
        );
    }

    #[test]
    fn sha256_hex_known_vector() {
        // sha256("") well-known digest.
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }
}
