//! **The `wasm32-unknown-unknown` acceptance test** for the streaming-output-sinks
//! RFC's `wasm` codec profile (§16.13), plus an OPFS-backed `zarrs` store.
//!
//! The RFC pins a `wasm` profile — plain Zarr v3 `zstd`, `sharding_indexed`,
//! `crc32c`, no Blosc — specifically so a browser can read these stores, and
//! then says that no wasm32 target had ever been built or executed against
//! them. This crate is that build and that execution.
//!
//! # Verdict: it builds, with two toolchain conditions and no source changes
//!
//! `zarrs` 0.23 with `sharding` + `crc32c` + `zstd` + `ndarray` + `transpose`
//! (no `blosc`, no `filesystem`, no `object_store`/`tokio`) compiles for
//! `wasm32-unknown-unknown` as-is. What is needed is a **wasm-capable clang**
//! (Apple's Xcode clang has no WebAssembly backend; Homebrew LLVM, upstream
//! LLVM or wasi-sdk do) and the **`zstd/no_asm`** feature, because zstd-sys
//! otherwise hands `huf_decompress_amd64.S` to the assembler. Both are recorded
//! against the dependency they belong to in `Cargo.toml`. Nothing was patched,
//! vendored or forked.
//!
//! `zarrs` turns out to be wasm-aware by design: its storage traits are bounded
//! by `MaybeSend`/`MaybeSync`, blanket no-ops on `target_arch = "wasm32"`, so a
//! store holding non-`Send` `JsValue`s implements them with no `unsafe`.
//!
//! # What is proven where
//!
//! * `tests/codec_wasm32.rs` runs **in Node** (no browser needed) and is the
//!   codec proof: a sharded zstd store written and read back inside wasm, and —
//!   the part that actually matters — a store produced by the **native** writer
//!   decoded by the wasm build, element for element. That is the cross-read the
//!   RFC asks for; it is what confirms the two profiles are the same profile.
//! * `tests/opfs_browser.rs` runs **in a browser** and is the storage proof: the
//!   same store written to and read from the Origin Private File System through
//!   [`opfs::OpfsStore`], whose synchronous access handles exist only inside a
//!   Web Worker — which is where the runner already lives.
//!
//! The `#[wasm_bindgen]` entry points below are the same flow exposed to JS, so
//! a plain page/worker harness can drive it without a WebDriver. `harness/`
//! contains that page, the Worker that runs the OPFS half, and a headless
//! driver (`node harness/run.mjs <path-to-playwright>`); all four of its checks
//! pass in Chromium, including the cross-read out of OPFS.
//!
//! # Bundle size
//!
//! Measured on this crate's `release` profile (`opt-level = "s"`, LTO,
//! `panic = "abort"`, `strip`) after `wasm-bindgen --target web`. `wasm-opt` was
//! **not** run — Binaryen typically takes another 10–20% off.
//!
//! | module | raw | gzip -9 | brotli -11 |
//! |---|---|---|---|
//! | this I/O module, standalone | 2.69 MB | 857 kB | 599 kB |
//! | the app's existing `earthsci_ast_bg.wasm` | 6.93 MB | 2.33 MB | 1.55 MB |
//!
//! Read that as the cost of shipping I/O as a **separate lazily-loaded module**,
//! which is the shape that keeps it off the critical path. Folding it into the
//! existing module instead would cost less than 857 kB, because a standalone
//! build re-includes machinery `earthsci-ast` already carries (serde/serde_json,
//! ndarray/num, the formatting and panic runtime) — but it would also put every
//! byte on the eager path.

pub mod opfs;

use std::collections::BTreeMap;
use std::sync::Arc;

use js_sys::{Float64Array, Uint8Array};
use serde_json::{json, Value};
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::JsFuture;
use web_sys::{FileSystemDirectoryHandle, FileSystemGetDirectoryOptions, FileSystemRemoveOptions};
use zarrs::array::{Array, ArrayMetadata};
use zarrs::storage::{
    ReadableStorageTraits, ReadableWritableStorageTraits, StoreKey, WritableStorageTraits,
};

use opfs::OpfsStore;

/// The RFC's `wasm` codec profile, byte-for-byte as `earthsciio`'s native writer
/// emits it (`format/zarr_write.rs`: `ZSTD_WASM`, `sharding_codec`,
/// `array_meta`). Kept here as data, not as a dependency, because this crate
/// must not pull in the native-only parent.
///
/// `shape` is the array shape, `shard` the outer (write) chunk shape, `inner`
/// the sharded inner chunk shape.
#[must_use]
pub fn wasm_profile_array_metadata(
    dims: &[&str],
    shape: &[u64],
    shard: &[u64],
    inner: &[u64],
) -> Value {
    json!({
        "zarr_format": 3,
        "node_type": "array",
        "shape": shape,
        "data_type": "float64",
        "chunk_grid": {"name": "regular", "configuration": {"chunk_shape": shard}},
        "chunk_key_encoding": {"name": "default", "configuration": {"separator": "/"}},
        "fill_value": 0.0,
        "codecs": [{
            "name": "sharding_indexed",
            "configuration": {
                "chunk_shape": inner,
                "codecs": [
                    {"name": "bytes", "configuration": {"endian": "little"}},
                    // The whole point of the profile: plain v3 zstd, no Blosc.
                    {"name": "zstd", "configuration": {"level": 5, "checksum": false}},
                ],
                "index_codecs": [
                    {"name": "bytes", "configuration": {"endian": "little"}},
                    {"name": "crc32c"},
                ],
                "index_location": "end",
            },
        }],
        "attributes": {"_ARRAY_DIMENSIONS": dims},
        "dimension_names": dims,
    })
}

/// Write a `wasm`-profile array into `store` at `/{name}` and return the data
/// that was written.
///
/// # Errors
/// Returns a message if the metadata is not valid Zarr v3 or a store write fails.
pub fn write_wasm_profile_array<S>(
    store: Arc<S>,
    name: &str,
    dims: &[&str],
    shape: &[u64],
    shard: &[u64],
    inner: &[u64],
    data: &[f64],
) -> Result<(), String>
where
    S: ReadableWritableStorageTraits + 'static,
{
    let meta = wasm_profile_array_metadata(dims, shape, shard, inner);
    let metadata: ArrayMetadata =
        serde_json::from_value(meta).map_err(|e| format!("invalid v3 array metadata: {e}"))?;
    let array = Array::new_with_metadata(store, &format!("/{name}"), metadata)
        .map_err(|e| format!("build array: {e}"))?;
    array
        .store_metadata()
        .map_err(|e| format!("store metadata: {e}"))?;
    array
        .store_array_subset(&array.subset_all(), data.to_vec())
        .map_err(|e| format!("store data: {e}"))?;
    Ok(())
}

/// Decode every element of the array at `/{name}` in `store`.
///
/// # Errors
/// Returns a message if the array cannot be opened or a chunk cannot be decoded.
pub fn read_all_f64<S>(store: Arc<S>, name: &str) -> Result<Vec<f64>, String>
where
    S: ReadableStorageTraits + ?Sized + 'static,
{
    let array =
        Array::open(store, &format!("/{name}")).map_err(|e| format!("open array '{name}': {e}"))?;
    array
        .retrieve_array_subset::<Vec<f64>>(&array.subset_all())
        .map_err(|e| format!("decode array '{name}': {e}"))
}

/// A deterministic smooth field, the kind of data the profile was measured on.
#[must_use]
pub fn sample_field(shape: &[u64]) -> Vec<f64> {
    let n: u64 = shape.iter().product();
    (0..n)
        .map(|i| {
            #[allow(clippy::cast_precision_loss)]
            let x = i as f64;
            (x * 0.05).sin() * 100.0 + x * 0.25
        })
        .collect()
}

// ---------------------------------------------------------------------------
// OPFS entry points, for a page/worker harness (no WebDriver required).
// ---------------------------------------------------------------------------

/// `navigator.storage.getDirectory()` from either a Worker or a Window scope.
async fn opfs_root() -> Result<FileSystemDirectoryHandle, JsValue> {
    let global = js_sys::global();
    let navigator = js_sys::Reflect::get(&global, &JsValue::from_str("navigator"))?;
    if navigator.is_undefined() {
        return Err(JsValue::from_str("no navigator in this scope"));
    }
    let storage = js_sys::Reflect::get(&navigator, &JsValue::from_str("storage"))?;
    if storage.is_undefined() {
        return Err(JsValue::from_str(
            "navigator.storage is undefined — OPFS needs a secure context",
        ));
    }
    let get_directory: js_sys::Function =
        js_sys::Reflect::get(&storage, &JsValue::from_str("getDirectory"))?.unchecked_into();
    let promise: js_sys::Promise = get_directory.call0(&storage)?.unchecked_into();
    Ok(JsFuture::from(promise).await?.unchecked_into())
}

/// A fresh, empty OPFS directory named `name` (any previous one is removed).
async fn fresh_dir(name: &str) -> Result<FileSystemDirectoryHandle, JsValue> {
    let root = opfs_root().await?;
    let rm = FileSystemRemoveOptions::new();
    rm.set_recursive(true);
    let _ = JsFuture::from(root.remove_entry_with_options(name, &rm)).await;
    let opts = FileSystemGetDirectoryOptions::new();
    opts.set_create(true);
    Ok(
        JsFuture::from(root.get_directory_handle_with_options(name, &opts))
            .await?
            .unchecked_into(),
    )
}

/// An existing OPFS directory named `name`.
async fn existing_dir(name: &str) -> Result<FileSystemDirectoryHandle, JsValue> {
    let root = opfs_root().await?;
    Ok(JsFuture::from(root.get_directory_handle(name))
        .await?
        .unchecked_into())
}

/// An OPFS directory named `name`, created if absent and **left alone** if it
/// already has contents (unlike [`fresh_dir`], which empties it).
async fn ensure_dir(name: &str) -> Result<FileSystemDirectoryHandle, JsValue> {
    let root = opfs_root().await?;
    let opts = FileSystemGetDirectoryOptions::new();
    opts.set_create(true);
    Ok(
        JsFuture::from(root.get_directory_handle_with_options(name, &opts))
            .await?
            .unchecked_into(),
    )
}

/// **Priority 3 — the OPFS round-trip.** Write a `wasm`-profile sharded store to
/// OPFS from wasm, reopen it from OPFS, decode it, and check every element.
///
/// Returns a JSON summary string.
///
/// # Errors
/// Returns a `JsValue` if any OPFS call rejects or a decode disagrees.
#[wasm_bindgen]
pub async fn opfs_roundtrip(dir_name: String) -> Result<JsValue, JsValue> {
    let dims = ["time", "x"];
    let shape = [4u64, 6];
    let shard = [2u64, 6];
    let inner = [1u64, 3];
    let data = sample_field(&shape);

    let dir = fresh_dir(&dir_name).await?;

    // Write into a staging store, then commit the whole thing to OPFS — OPFS
    // cannot create a file synchronously, so `set` stages and `commit` lands.
    let staging = Arc::new(OpfsStore::staging());
    staging
        .set(
            &StoreKey::new("zarr.json").map_err(|e| JsValue::from_str(&e.to_string()))?,
            zarrs::storage::Bytes::from(
                serde_json::to_vec(&json!({"zarr_format": 3, "node_type": "group"}))
                    .map_err(|e| JsValue::from_str(&e.to_string()))?,
            ),
        )
        .map_err(|e| JsValue::from_str(&e.to_string()))?;
    write_wasm_profile_array(
        staging.clone(),
        "conc",
        &dims,
        &shape,
        &shard,
        &inner,
        &data,
    )
    .map_err(|e| JsValue::from_str(&e))?;
    staging.commit(&dir).await?;

    // Reopen from OPFS — a different store instance, backed only by the files.
    let reopened = Arc::new(OpfsStore::open(&dir).await?);
    let files = reopened.open_file_count();
    let bytes = reopened
        .stored_bytes()
        .map_err(|e| JsValue::from_str(&e.to_string()))?;
    let got = read_all_f64(reopened.clone(), "conc").map_err(|e| JsValue::from_str(&e))?;

    let max_err = max_abs_diff(&data, &got)?;
    // Confirm no Blosc snuck in and the store really is sharded.
    let meta = reopened
        .get_whole("conc/zarr.json")
        .map_err(|e| JsValue::from_str(&e.to_string()))?
        .ok_or_else(|| JsValue::from_str("conc/zarr.json missing from OPFS"))?;
    let meta = String::from_utf8_lossy(&meta).to_string();

    Ok(JsValue::from_str(
        &json!({
            "ok": max_err == 0.0,
            "elements": got.len(),
            "opfs_files": files,
            "opfs_bytes": bytes,
            "max_abs_error": max_err,
            "sharded": meta.contains("sharding_indexed"),
            "zstd": meta.contains("\"zstd\""),
            "blosc_free": !meta.contains("blosc"),
        })
        .to_string(),
    ))
}

/// Seed one object into an OPFS directory — how the harness uploads a store
/// produced by the **native** writer so [`opfs_read_array`] can cross-read it.
///
/// # Errors
/// Returns a `JsValue` if the OPFS write fails.
#[wasm_bindgen]
pub async fn opfs_put(dir_name: String, key: String, bytes: Uint8Array) -> Result<(), JsValue> {
    let dir = ensure_dir(&dir_name).await?;
    let staging = OpfsStore::staging();
    staging
        .set(
            &StoreKey::new(&key).map_err(|e| JsValue::from_str(&e.to_string()))?,
            opfs::js_bytes(&bytes),
        )
        .map_err(|e| JsValue::from_str(&e.to_string()))?;
    staging.commit(&dir).await
}

/// Remove an OPFS directory and everything under it.
///
/// # Errors
/// Returns a `JsValue` if the OPFS root cannot be reached.
#[wasm_bindgen]
pub async fn opfs_rm(dir_name: String) -> Result<(), JsValue> {
    let root = opfs_root().await?;
    let rm = FileSystemRemoveOptions::new();
    rm.set_recursive(true);
    let _ = JsFuture::from(root.remove_entry_with_options(&dir_name, &rm)).await;
    Ok(())
}

/// **Priority 4 — the cross-read.** Decode an array out of an OPFS directory
/// that was seeded with a store the native writer produced.
///
/// # Errors
/// Returns a `JsValue` if the store cannot be opened or a chunk cannot be decoded.
#[wasm_bindgen]
pub async fn opfs_read_array(
    dir_name: String,
    array_name: String,
) -> Result<Float64Array, JsValue> {
    let dir = existing_dir(&dir_name).await?;
    let store = Arc::new(OpfsStore::open(&dir).await?);
    let values = read_all_f64(store, &array_name).map_err(|e| JsValue::from_str(&e))?;
    Ok(Float64Array::from(values.as_slice()))
}

fn max_abs_diff(a: &[f64], b: &[f64]) -> Result<f64, JsValue> {
    if a.len() != b.len() {
        return Err(JsValue::from_str(&format!(
            "length mismatch: wrote {} elements, read {}",
            a.len(),
            b.len()
        )));
    }
    Ok(a.iter()
        .zip(b)
        .fold(0.0f64, |m, (x, y)| m.max((x - y).abs())))
}

/// Load a `{key: [bytes…]}` fixture (see `fixtures/`) into a store.
///
/// # Errors
/// Returns a message if the fixture is not the expected JSON shape.
pub fn load_fixture<S: zarrs::storage::WritableStorageTraits>(
    store: &S,
    fixture_json: &str,
) -> Result<usize, String> {
    let map: BTreeMap<String, Vec<u8>> = serde_json::from_str(fixture_json)
        .map_err(|e| format!("fixture is not {{key: bytes}}: {e}"))?;
    let n = map.len();
    for (key, bytes) in map {
        let key = StoreKey::new(&key).map_err(|e| e.to_string())?;
        store
            .set(&key, zarrs::storage::Bytes::from(bytes))
            .map_err(|e| e.to_string())?;
    }
    Ok(n)
}
