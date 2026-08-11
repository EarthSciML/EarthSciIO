//! The storage half of the acceptance test: the same store written to and read
//! back from the **Origin Private File System**.
//!
//! Browser-only — OPFS does not exist in Node — so this needs a WebDriver:
//!
//! ```text
//! CHROMEDRIVER=$(which chromedriver) \
//! CC_wasm32_unknown_unknown=$(brew --prefix llvm)/bin/clang \
//! AR_wasm32_unknown_unknown=$(brew --prefix llvm)/bin/llvm-ar \
//! cargo test --target wasm32-unknown-unknown --test opfs_browser
//! ```
//!
//! `wasm-bindgen-test`'s browser runner drives the **main thread**, where
//! `createSyncAccessHandle()` is unavailable in Chrome — sync access handles are
//! a Worker-only API. So `harness/` exists as well: it loads the same wasm in a
//! real dedicated Worker and calls the `#[wasm_bindgen]` entry points, which is
//! the configuration the runner actually ships in. Keep both; they check
//! different things (this one that the store type is correct, the harness that
//! the Worker-only sync API works).

#![cfg(target_arch = "wasm32")]

use earthsciio_wasm_zarr_spike::{opfs_read_array, opfs_rm, opfs_roundtrip};
use wasm_bindgen_test::{wasm_bindgen_test, wasm_bindgen_test_configure};

wasm_bindgen_test_configure!(run_in_browser);

/// Priority 3: write a `wasm`-profile sharded store to OPFS from wasm, reopen it
/// from OPFS, and check every decoded element.
#[wasm_bindgen_test]
async fn opfs_write_then_read_roundtrip() {
    let summary = opfs_roundtrip("spike-roundtrip".to_string())
        .await
        .expect("OPFS round-trip");
    let summary: serde_json::Value =
        serde_json::from_str(&summary.as_string().expect("summary string")).expect("summary json");
    assert_eq!(summary["ok"], serde_json::Value::Bool(true), "{summary}");
    assert_eq!(summary["max_abs_error"].as_f64(), Some(0.0), "{summary}");
    assert_eq!(
        summary["sharded"],
        serde_json::Value::Bool(true),
        "{summary}"
    );
    assert_eq!(
        summary["blosc_free"],
        serde_json::Value::Bool(true),
        "{summary}"
    );
    assert!(
        summary["opfs_files"].as_u64().unwrap_or(0) >= 2,
        "{summary}"
    );
    opfs_rm("spike-roundtrip".to_string())
        .await
        .expect("cleanup");
}

/// Reading an array out of an OPFS directory that was never written must fail
/// loudly rather than return an empty result.
#[wasm_bindgen_test]
async fn a_missing_opfs_store_is_an_error() {
    let err = opfs_read_array("spike-does-not-exist".to_string(), "conc".to_string()).await;
    assert!(
        err.is_err(),
        "a missing OPFS store must not decode to nothing"
    );
}
