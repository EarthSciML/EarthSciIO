//! A `zarrs` store backed by the **Origin Private File System**.
//!
//! # Why the read path is synchronous and the write path stages
//!
//! OPFS is *almost* a synchronous filesystem, and exactly which half is
//! synchronous decides the shape of this type:
//!
//! | Operation | Sync? |
//! |---|---|
//! | `navigator.storage.getDirectory()` | **no** — Promise |
//! | `dir.getDirectoryHandle(name, …)`, `dir.getFileHandle(name, …)` | **no** — Promise |
//! | `dir.entries()` / `keys()` / `values()` | **no** — AsyncIterator |
//! | `file.createSyncAccessHandle()` | **no** — Promise (Worker-only) |
//! | `handle.read(buf, {at})`, `.write(buf, {at})`, `.getSize()`, `.truncate()`, `.flush()` | **YES** |
//!
//! So: *acquiring* a handle is always async, but *I/O on an acquired handle* is
//! synchronous — and the synchronous form exists only inside a Web Worker, which
//! is where the runner already lives. There is no sync directory API anywhere,
//! and blocking on a Promise from wasm is not possible without an
//! `Atomics.wait` shim and a second worker.
//!
//! That yields a store with an async *acquire* phase around a fully synchronous
//! *use* phase, which is all `zarrs`' sync storage traits need:
//!
//! * [`OpfsStore::open`] walks the store directory once (async) and opens a
//!   sync access handle per file. After that, [`ReadableStorageTraits`] is
//!   genuinely lazy: [`supports_get_partial`](ReadableStorageTraits::supports_get_partial)
//!   is **`true`**, so a sharded read pulls the shard index and then only the
//!   inner chunk bytes it needs, straight out of OPFS — never the whole shard.
//! * Writes stage in memory and land in OPFS at [`OpfsStore::commit`] (async),
//!   because creating a file is a Promise and `set` is not. This matches the
//!   whole-dataset-commit model the output plan already assumes; a streaming
//!   writer would pre-create its files during the async phase instead.
//!
//! `zarrs` is wasm-aware here in a way that makes this work at all: its storage
//! traits are bounded by `MaybeSend`/`MaybeSync`, which are blanket-implemented
//! no-ops on `target_arch = "wasm32"`. A store holding `JsValue`s — which are
//! neither `Send` nor `Sync` — therefore implements them without any `unsafe`.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};

use js_sys::{Array, Uint8Array};
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;
use web_sys::{
    FileSystemDirectoryHandle, FileSystemFileHandle, FileSystemGetDirectoryOptions,
    FileSystemGetFileOptions, FileSystemHandle, FileSystemHandleKind, FileSystemReadWriteOptions,
    FileSystemSyncAccessHandle,
};
use zarrs::storage::byte_range::ByteRangeIterator;
use zarrs::storage::{
    Bytes, ListableStorageTraits, MaybeBytesIterator, OffsetBytesIterator, ReadableStorageTraits,
    StorageError, StoreKey, StoreKeys, StoreKeysPrefixes, StorePrefix, StorePrefixes,
    WritableStorageTraits,
};

fn js_err(context: &str, e: &JsValue) -> StorageError {
    StorageError::Other(format!("{context}: {}", format_js(e)))
}

fn format_js(e: &JsValue) -> String {
    e.as_string()
        .or_else(|| {
            js_sys::Reflect::get(e, &JsValue::from_str("message"))
                .ok()?
                .as_string()
        })
        .unwrap_or_else(|| format!("{e:?}"))
}

/// A file already present in OPFS, with its sync access handle held open.
struct OpenFile {
    handle: FileSystemSyncAccessHandle,
}

impl OpenFile {
    fn size(&self) -> Result<u64, StorageError> {
        let n = self
            .handle
            .get_size()
            .map_err(|e| js_err("FileSystemSyncAccessHandle.getSize", &e))?;
        Ok(n as u64)
    }

    /// Read `len` bytes at `offset` — a real OPFS range read, not a slice of a
    /// pre-loaded buffer.
    fn read_at(&self, offset: u64, len: usize) -> Result<Bytes, StorageError> {
        let mut buf = vec![0u8; len];
        let opts = FileSystemReadWriteOptions::new();
        opts.set_at(offset as f64);
        let got = self
            .handle
            .read_with_u8_array_and_options(&mut buf, &opts)
            .map_err(|e| js_err("FileSystemSyncAccessHandle.read", &e))?;
        let got = got as usize;
        if got != len {
            return Err(StorageError::Other(format!(
                "short OPFS read: wanted {len} bytes at {offset}, got {got}"
            )));
        }
        Ok(Bytes::from(buf))
    }
}

/// A `zarrs` store over one OPFS directory. See the module docs for why the
/// acquire phase is async and the use phase is not.
pub struct OpfsStore {
    /// Files present in OPFS when [`OpfsStore::open`] ran, handles held open.
    files: RefCell<BTreeMap<StoreKey, OpenFile>>,
    /// Writes not yet flushed to OPFS (`set` cannot create a file synchronously).
    staged: RefCell<BTreeMap<StoreKey, Bytes>>,
    /// Keys erased since open, to be removed from OPFS at commit.
    erased: RefCell<BTreeSet<StoreKey>>,
}

impl OpfsStore {
    /// An empty store with nothing backing it yet — the write path. Populate it
    /// through the `zarrs` API, then [`commit`](Self::commit) it into OPFS.
    #[must_use]
    pub fn staging() -> Self {
        Self {
            files: RefCell::new(BTreeMap::new()),
            staged: RefCell::new(BTreeMap::new()),
            erased: RefCell::new(BTreeSet::new()),
        }
    }

    /// Open every file under `dir` (recursively) and hold a **sync access
    /// handle** for each, so all later reads are synchronous and lazy.
    ///
    /// # Errors
    /// Returns the underlying `JsValue` if any OPFS call rejects.
    pub async fn open(dir: &FileSystemDirectoryHandle) -> Result<Self, JsValue> {
        let store = Self::staging();
        let mut found = Vec::new();
        walk(dir, String::new(), &mut found).await?;
        for (path, file) in found {
            let handle: FileSystemSyncAccessHandle =
                JsFuture::from(file.create_sync_access_handle())
                    .await?
                    .unchecked_into();
            let key = StoreKey::new(&path).map_err(|e| JsValue::from_str(&e.to_string()))?;
            store.files.borrow_mut().insert(key, OpenFile { handle });
        }
        Ok(store)
    }

    /// Number of OPFS files this store has handles open on.
    #[must_use]
    pub fn open_file_count(&self) -> usize {
        self.files.borrow().len()
    }

    /// Total bytes of every OPFS file backing this store.
    ///
    /// # Errors
    /// Returns a [`StorageError`] if an OPFS `getSize` fails.
    pub fn stored_bytes(&self) -> Result<u64, StorageError> {
        self.files
            .borrow()
            .values()
            .try_fold(0u64, |acc, f| Ok(acc + f.size()?))
    }

    /// Flush staged writes and erasures into `dir`, creating directories as
    /// needed. Each file is written through its own sync access handle, which is
    /// closed again so a later [`open`](Self::open) can re-acquire it (OPFS sync
    /// handles are exclusive).
    ///
    /// # Errors
    /// Returns the underlying `JsValue` if any OPFS call rejects.
    pub async fn commit(&self, dir: &FileSystemDirectoryHandle) -> Result<(), JsValue> {
        let erased: Vec<StoreKey> = self.erased.borrow_mut().iter().cloned().collect();
        for key in erased {
            let (parents, name) = split_key(key.as_str());
            if let Some(parent) = resolve_dir(dir, &parents, false).await? {
                // A missing file is not an error — erase is idempotent.
                let _ = JsFuture::from(parent.remove_entry(&name)).await;
            }
        }
        self.erased.borrow_mut().clear();

        let staged: Vec<(StoreKey, Bytes)> = self
            .staged
            .borrow_mut()
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        for (key, value) in staged {
            let (parents, name) = split_key(key.as_str());
            let parent = resolve_dir(dir, &parents, true)
                .await?
                .ok_or_else(|| JsValue::from_str("directory creation returned nothing"))?;
            let opts = FileSystemGetFileOptions::new();
            opts.set_create(true);
            let file: FileSystemFileHandle =
                JsFuture::from(parent.get_file_handle_with_options(&name, &opts))
                    .await?
                    .unchecked_into();
            let handle: FileSystemSyncAccessHandle =
                JsFuture::from(file.create_sync_access_handle())
                    .await?
                    .unchecked_into();
            handle.truncate_with_f64(0.0)?;
            let mut bytes = value.to_vec();
            handle.write_with_u8_array(&mut bytes)?;
            handle.flush()?;
            handle.close();
        }
        self.staged.borrow_mut().clear();
        Ok(())
    }

    /// Whole-object read: staged bytes win over OPFS bytes.
    fn whole(&self, key: &StoreKey) -> Result<Option<Bytes>, StorageError> {
        if let Some(b) = self.staged.borrow().get(key) {
            return Ok(Some(b.clone()));
        }
        let files = self.files.borrow();
        let Some(file) = files.get(key) else {
            return Ok(None);
        };
        let size = file.size()?;
        Ok(Some(file.read_at(0, size as usize)?))
    }

    fn known_keys(&self) -> StoreKeys {
        let mut keys: BTreeSet<StoreKey> = self.files.borrow().keys().cloned().collect();
        keys.extend(self.staged.borrow().keys().cloned());
        for k in self.erased.borrow().iter() {
            keys.remove(k);
        }
        keys.into_iter().collect()
    }
}

/// Split `a/b/c.json` into (`["a", "b"]`, `"c.json"`).
fn split_key(key: &str) -> (Vec<String>, String) {
    let mut parts: Vec<String> = key.split('/').map(ToString::to_string).collect();
    let name = parts.pop().unwrap_or_default();
    (parts, name)
}

/// Walk `parents` from `dir`, optionally creating each level. `Ok(None)` means a
/// level was absent and `create` was false.
async fn resolve_dir(
    dir: &FileSystemDirectoryHandle,
    parents: &[String],
    create: bool,
) -> Result<Option<FileSystemDirectoryHandle>, JsValue> {
    let mut cur = dir.clone();
    for part in parents {
        let opts = FileSystemGetDirectoryOptions::new();
        opts.set_create(create);
        match JsFuture::from(cur.get_directory_handle_with_options(part, &opts)).await {
            Ok(next) => cur = next.unchecked_into(),
            Err(e) if !create => {
                let _ = e;
                return Ok(None);
            }
            Err(e) => return Err(e),
        }
    }
    Ok(Some(cur))
}

/// Recursively collect every file under `dir` as (`store key`, handle).
/// Boxed because `async fn` cannot recurse directly.
fn walk<'a>(
    dir: &'a FileSystemDirectoryHandle,
    prefix: String,
    out: &'a mut Vec<(String, FileSystemFileHandle)>,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), JsValue>> + 'a>> {
    Box::pin(async move {
        let entries = dir.entries();
        loop {
            let next = JsFuture::from(entries.next()?).await?;
            if js_sys::Reflect::get(&next, &JsValue::from_str("done"))?
                .as_bool()
                .unwrap_or(false)
            {
                break;
            }
            let pair: Array = js_sys::Reflect::get(&next, &JsValue::from_str("value"))?.into();
            let name = pair.get(0).as_string().unwrap_or_default();
            let handle: FileSystemHandle = pair.get(1).unchecked_into();
            let path = if prefix.is_empty() {
                name.clone()
            } else {
                format!("{prefix}/{name}")
            };
            match handle.kind() {
                FileSystemHandleKind::File => {
                    out.push((path, handle.unchecked_into()));
                }
                _ => {
                    let sub: FileSystemDirectoryHandle = handle.unchecked_into();
                    walk(&sub, path, out).await?;
                }
            }
        }
        Ok(())
    })
}

impl ReadableStorageTraits for OpfsStore {
    fn get_partial_many<'a>(
        &'a self,
        key: &StoreKey,
        byte_ranges: ByteRangeIterator<'a>,
    ) -> Result<MaybeBytesIterator<'a>, StorageError> {
        // Staged (not yet in OPFS): slice the in-memory buffer.
        if let Some(whole) = self.staged.borrow().get(key).cloned() {
            let size = whole.len() as u64;
            let mut out: Vec<Result<Bytes, StorageError>> = Vec::new();
            for br in byte_ranges {
                let start = br.start(size) as usize;
                let end = start + br.length(size) as usize;
                out.push(if end > whole.len() {
                    Err(StorageError::Other(format!(
                        "byte range {start}..{end} out of bounds for {} bytes",
                        whole.len()
                    )))
                } else {
                    Ok(whole.slice(start..end))
                });
            }
            return Ok(Some(Box::new(out.into_iter())));
        }

        // In OPFS: one `read(buf, {at})` per range. This is the point of holding
        // sync access handles — a sharded read touches the index and one inner
        // chunk, not the whole shard.
        let files = self.files.borrow();
        let Some(file) = files.get(key) else {
            return Ok(None);
        };
        let size = file.size()?;
        let mut out: Vec<Result<Bytes, StorageError>> = Vec::new();
        for br in byte_ranges {
            let start = br.start(size);
            let len = br.length(size);
            out.push(file.read_at(start, len as usize));
        }
        Ok(Some(Box::new(out.into_iter())))
    }

    fn size_key(&self, key: &StoreKey) -> Result<Option<u64>, StorageError> {
        if let Some(b) = self.staged.borrow().get(key) {
            return Ok(Some(b.len() as u64));
        }
        self.files.borrow().get(key).map(OpenFile::size).transpose()
    }

    fn supports_get_partial(&self) -> bool {
        // OPFS sync access handles do real positional reads.
        true
    }
}

impl WritableStorageTraits for OpfsStore {
    fn set(&self, key: &StoreKey, value: Bytes) -> Result<(), StorageError> {
        self.erased.borrow_mut().remove(key);
        self.staged.borrow_mut().insert(key.clone(), value);
        Ok(())
    }

    fn set_partial_many(
        &self,
        key: &StoreKey,
        offset_values: OffsetBytesIterator,
    ) -> Result<(), StorageError> {
        zarrs::storage::store_set_partial_many(self, key, offset_values)
    }

    fn erase(&self, key: &StoreKey) -> Result<(), StorageError> {
        self.staged.borrow_mut().remove(key);
        if self.files.borrow_mut().remove(key).is_some() {
            self.erased.borrow_mut().insert(key.clone());
        }
        Ok(())
    }

    fn erase_prefix(&self, prefix: &StorePrefix) -> Result<(), StorageError> {
        for key in self.known_keys() {
            if key.has_prefix(prefix) {
                self.erase(&key)?;
            }
        }
        Ok(())
    }

    fn supports_set_partial(&self) -> bool {
        // A partial set becomes a staged read-modify-write; OPFS itself could do
        // better, but only for files that already exist, and the writer never
        // needs it (whole shards are written once).
        false
    }
}

// `ReadableWritableStorageTraits` (and the Readable+Listable combinations) come
// from zarrs' blanket impls over the three traits above.

impl ListableStorageTraits for OpfsStore {
    fn list(&self) -> Result<StoreKeys, StorageError> {
        Ok(self.known_keys())
    }

    fn list_prefix(&self, prefix: &StorePrefix) -> Result<StoreKeys, StorageError> {
        Ok(self
            .known_keys()
            .into_iter()
            .filter(|k| k.has_prefix(prefix))
            .collect())
    }

    fn list_dir(&self, prefix: &StorePrefix) -> Result<StoreKeysPrefixes, StorageError> {
        let mut keys: StoreKeys = Vec::new();
        let mut prefixes: BTreeSet<String> = BTreeSet::new();
        for key in self.known_keys() {
            if !key.has_prefix(prefix) {
                continue;
            }
            let rest = &key.as_str()[prefix.as_str().len()..];
            match rest.split_once('/') {
                None => keys.push(key),
                Some((head, _)) => {
                    prefixes.insert(format!("{}{head}/", prefix.as_str()));
                }
            }
        }
        let prefixes: StorePrefixes = prefixes
            .into_iter()
            .map(StorePrefix::new)
            .collect::<Result<_, _>>()
            .map_err(|e| StorageError::Other(e.to_string()))?;
        Ok(StoreKeysPrefixes::new(keys, prefixes))
    }

    fn size_prefix(&self, prefix: &StorePrefix) -> Result<u64, StorageError> {
        let mut total = 0;
        for key in self.list_prefix(prefix)? {
            total += self.size_key(&key)?.unwrap_or(0);
        }
        Ok(total)
    }
}

impl OpfsStore {
    /// Whole-object convenience read used by the verification entry points.
    ///
    /// # Errors
    /// Returns a [`StorageError`] if the underlying OPFS read fails.
    pub fn get_whole(&self, key: &str) -> Result<Option<Bytes>, StorageError> {
        let key = StoreKey::new(key).map_err(|e| StorageError::Other(e.to_string()))?;
        self.whole(&key)
    }
}

/// Bytes of a `Uint8Array` as a `zarrs` value — used by the JS harness to seed
/// OPFS with a store produced by the **native** writer.
#[must_use]
pub fn js_bytes(a: &Uint8Array) -> Bytes {
    Bytes::from(a.to_vec())
}
