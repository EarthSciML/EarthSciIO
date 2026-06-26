//! The `local` store backend (`spec/cache-format.md` §2): the
//! `$EARTHSCIDATADIR` filesystem layout, advisory `flock`, and atomic rename.
//!
//! ```text
//! $EARTHSCIDATADIR/
//!   v1/                                  # cache-format version
//!     blobs/<key[:2]>/<key>.<ext>        # the downloaded file
//!     meta/<key>.json                    # the manifest
//!     locks/<key>.lock                   # per-blob advisory lock
//!     tmp/<uuid>.part                    # atomic-rename staging
//! ```

use std::fs::{self, OpenOptions};
use std::path::{Path, PathBuf};

use fs2::FileExt;

use super::{BlobLock, StagingFile, Store};
use crate::error::{Error, Result};
use crate::manifest::Manifest;

/// Cache-format version directory. Bumping it invalidates the whole cache by
/// changing one path segment (`spec/cache-format.md` §2).
pub const CACHE_VERSION: &str = "v1";

/// Filesystem-backed content-addressed store rooted at `$EARTHSCIDATADIR`.
pub struct LocalStore {
    root: PathBuf,
}

impl LocalStore {
    /// A store rooted at `root` (the `$EARTHSCIDATADIR` value).
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// A store rooted at the env-resolved `$EARTHSCIDATADIR`.
    pub fn from_env() -> Self {
        Self::new(crate::datadir::data_dir())
    }

    /// The cache root (`$EARTHSCIDATADIR`).
    pub fn root(&self) -> &Path {
        &self.root
    }

    fn version_dir(&self) -> PathBuf {
        self.root.join(CACHE_VERSION)
    }

    fn fanout_dir(&self, key: &str) -> PathBuf {
        self.version_dir().join("blobs").join(&key[..2])
    }

    fn blob_path(&self, key: &str, ext: &str) -> PathBuf {
        let name = if ext.is_empty() {
            key.to_string()
        } else {
            format!("{key}.{ext}")
        };
        self.fanout_dir(key).join(name)
    }

    fn meta_dir(&self) -> PathBuf {
        self.version_dir().join("meta")
    }

    fn meta_path(&self, key: &str) -> PathBuf {
        self.meta_dir().join(format!("{key}.json"))
    }

    fn lock_path(&self, key: &str) -> PathBuf {
        self.version_dir().join("locks").join(format!("{key}.lock"))
    }

    fn tmp_dir(&self) -> PathBuf {
        self.version_dir().join("tmp")
    }

    /// Find the blob for `key` regardless of its (debug-only) extension. The
    /// filesystem is the index — there is no separate database.
    fn find_blob(&self, key: &str) -> Option<PathBuf> {
        let dir = self.fanout_dir(key);
        for entry in fs::read_dir(&dir).ok()?.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            // Match `<key>` (no ext) or `<key>.<ext>`.
            if name == *key
                || name
                    .strip_prefix(key)
                    .is_some_and(|rest| rest.starts_with('.'))
            {
                return Some(entry.path());
            }
        }
        None
    }
}

fn create_dir_all(path: &Path) -> Result<()> {
    fs::create_dir_all(path).map_err(|e| Error::io(Some(path.to_path_buf()), e))
}

impl Store for LocalStore {
    fn name(&self) -> &str {
        "local"
    }

    fn exists(&self, key: &str) -> bool {
        self.find_blob(key).is_some()
    }

    fn get_blob(&self, key: &str) -> Option<PathBuf> {
        self.find_blob(key)
    }

    fn get_meta(&self, key: &str) -> Result<Option<Manifest>> {
        let path = self.meta_path(key);
        match fs::read(&path) {
            Ok(data) => Ok(Some(Manifest::from_json_bytes(&data)?)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(Error::io(Some(path), e)),
        }
    }

    fn put_meta(&self, key: &str, manifest: &Manifest) -> Result<()> {
        let dir = self.meta_dir();
        create_dir_all(&dir)?;
        let bytes = manifest.to_json_bytes()?;
        // Stage in the same dir, then atomic-rename so a reader never sees a
        // torn manifest.
        let mut tmp = tempfile::Builder::new()
            .prefix(".meta-")
            .suffix(".json.part")
            .tempfile_in(&dir)
            .map_err(|e| Error::io(Some(dir.clone()), e))?;
        use std::io::Write as _;
        tmp.write_all(&bytes)
            .map_err(|e| Error::io(Some(tmp.path().to_path_buf()), e))?;
        tmp.flush()
            .map_err(|e| Error::io(Some(tmp.path().to_path_buf()), e))?;
        let dst = self.meta_path(key);
        tmp.persist(&dst)
            .map_err(|e| Error::io(Some(dst.clone()), e.error))?;
        Ok(())
    }

    fn new_staging(&self) -> Result<StagingFile> {
        let dir = self.tmp_dir();
        create_dir_all(&dir)?;
        let temp = tempfile::Builder::new()
            .prefix("dl-")
            .suffix(".part")
            .tempfile_in(&dir)
            .map_err(|e| Error::io(Some(dir), e))?;
        Ok(StagingFile::new(temp))
    }

    fn commit_blob(&self, key: &str, staged: StagingFile, ext: &str) -> Result<PathBuf> {
        let dir = self.fanout_dir(key);
        create_dir_all(&dir)?;
        let dst = self.blob_path(key, ext);
        // Atomic rename from staging (same filesystem) into the blob slot. If a
        // racing process committed first, rename replaces atomically — the
        // content is identical (same key), so this is safe.
        staged
            .into_temp()
            .persist(&dst)
            .map_err(|e| Error::io(Some(dst.clone()), e.error))?;
        Ok(dst)
    }

    fn lock(&self, key: &str) -> Result<BlobLock> {
        let path = self.lock_path(key);
        if let Some(parent) = path.parent() {
            create_dir_all(parent)?;
        }
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(&path)
            .map_err(|e| Error::io(Some(path.clone()), e))?;
        // Blocking exclusive flock; released when the BlobLock (fd) drops.
        file.lock_exclusive()
            .map_err(|e| Error::io(Some(path), e))?;
        Ok(BlobLock::new(file))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_root() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "esio-store-test-{}-{}",
            std::process::id(),
            // a per-call counter avoids collisions between tests in this binary
            COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        dir
    }
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

    fn manifest(key_url: &str, sha: &str, n: u64) -> Manifest {
        Manifest {
            auth_realm: None,
            bytes: n,
            etag: None,
            fetched_at: "2026-06-26T00:00:00Z".to_string(),
            last_modified: None,
            schema: crate::manifest::MANIFEST_SCHEMA.to_string(),
            sha256_content: sha.to_string(),
            source_loader: None,
            url: key_url.to_string(),
        }
    }

    #[test]
    fn put_then_get_blob_and_meta_roundtrip() {
        let root = tmp_root();
        let store = LocalStore::new(&root);
        let key = crate::key::cache_key("https://x/y.nc");

        // Stage some bytes (write through the staging path), commit, read back.
        let staging = store.new_staging().unwrap();
        fs::write(staging.path(), b"hello-bytes").unwrap();
        let blob = store.commit_blob(&key, staging, "nc").unwrap();
        assert!(blob.exists());
        assert_eq!(
            blob.file_name().unwrap().to_string_lossy(),
            format!("{key}.nc")
        );
        assert_eq!(store.get_blob(&key).unwrap(), blob);
        assert!(store.exists(&key));

        let m = manifest(
            "https://x/y.nc",
            &crate::key::sha256_hex(b"hello-bytes"),
            11,
        );
        store.put_meta(&key, &m).unwrap();
        assert_eq!(store.get_meta(&key).unwrap().unwrap(), m);

        // find_blob works regardless of extension.
        assert!(store.get_blob("deadbeef").is_none());

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn fanout_uses_first_two_hex_chars() {
        let store = LocalStore::new("/cache");
        let key = "abcdef0000000000000000000000000000000000000000000000000000000000";
        assert_eq!(
            store.blob_path(key, "nc"),
            PathBuf::from(format!("/cache/v1/blobs/ab/{key}.nc"))
        );
        assert_eq!(
            store.meta_path(key),
            PathBuf::from(format!("/cache/v1/meta/{key}.json"))
        );
        assert_eq!(
            store.lock_path(key),
            PathBuf::from(format!("/cache/v1/locks/{key}.lock"))
        );
    }

    #[test]
    fn lock_is_acquired_and_released() {
        let root = tmp_root();
        let store = LocalStore::new(&root);
        let key = crate::key::cache_key("lock-test");
        {
            let _g = store.lock(&key).unwrap();
            // A second lock on a *separate* fd would block; we only assert the
            // happy path here (cross-thread contention is covered in tests/).
        }
        // After drop, re-locking succeeds immediately.
        let _g2 = store.lock(&key).unwrap();
        fs::remove_dir_all(&root).ok();
    }
}
