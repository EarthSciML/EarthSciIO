//! The codec half of the acceptance test, **executed on `wasm32-unknown-unknown`**.
//!
//! Runs under Node (`wasm-bindgen-test-runner`'s default), so it needs no
//! browser and no WebDriver — the codec chain has nothing to do with the DOM:
//!
//! ```text
//! CC_wasm32_unknown_unknown=$(brew --prefix llvm)/bin/clang \
//! AR_wasm32_unknown_unknown=$(brew --prefix llvm)/bin/llvm-ar \
//! cargo test --target wasm32-unknown-unknown
//! ```
//!
//! `native_wasm_profile.store.json` is the cross-read fixture: a `wasm`-profile
//! store written by **`earthsciio`'s native writer** (`write_zarr_v3`, profile
//! `"wasm"`), captured as `{store key: bytes}`. Regenerate it with a native
//! program that calls `write_zarr_v3` on the schema described in
//! `native_wasm_profile.expected.json` and walks the resulting directory.

#![cfg(target_arch = "wasm32")]

use std::sync::Arc;

use earthsciio_wasm_zarr_spike::{
    load_fixture, opfs::OpfsStore, read_all_f64, sample_field, write_wasm_profile_array,
};
use wasm_bindgen_test::wasm_bindgen_test;

const NATIVE_STORE: &str = include_str!("../fixtures/native_wasm_profile.store.json");
const NATIVE_EXPECTED: &str = include_str!("../fixtures/native_wasm_profile.expected.json");

/// Priority 1+3, minus OPFS: the `wasm` profile's whole codec chain —
/// `sharding_indexed` over plain v3 `zstd`, with a `crc32c` shard index — encodes
/// and decodes inside wasm, exactly.
#[wasm_bindgen_test]
fn wasm_profile_encodes_and_decodes_in_wasm() {
    let shape = [4u64, 6];
    let data = sample_field(&shape);
    let store = Arc::new(OpfsStore::staging());
    write_wasm_profile_array(
        store.clone(),
        "conc",
        &["time", "x"],
        &shape,
        &[2, 6],
        &[1, 3],
        &data,
    )
    .expect("write wasm-profile array");

    let got = read_all_f64(store, "conc").expect("decode wasm-profile array");
    assert_eq!(
        got, data,
        "round-trip through zstd + sharding must be exact"
    );
}

/// **Priority 4 — the cross-read, and the point of the whole exercise.** A store
/// the NATIVE writer produced is decoded by the wasm build, element for element.
/// If the two `wasm` profiles were not the same profile, this is what would say so.
#[wasm_bindgen_test]
fn a_native_written_store_decodes_in_wasm() {
    let expected: serde_json::Value =
        serde_json::from_str(NATIVE_EXPECTED).expect("expected fixture");
    let want: Vec<f64> = expected["values"]
        .as_array()
        .expect("values")
        .iter()
        .map(|v| v.as_f64().expect("f64"))
        .collect();

    let store = Arc::new(OpfsStore::staging());
    let n = load_fixture(store.as_ref(), NATIVE_STORE).expect("load native fixture");
    assert!(
        n >= 4,
        "fixture should carry the whole store, got {n} objects"
    );

    let got = read_all_f64(store.clone(), "conc").expect("decode native store in wasm");
    assert_eq!(
        got, want,
        "native-written chunks must decode bit-exactly in wasm"
    );

    // The coordinate arrays are separate v3 arrays with the same codec chain.
    let time = read_all_f64(store, "time").expect("decode native time coord in wasm");
    let want_time: Vec<f64> = expected["coords"]["time"]
        .as_array()
        .expect("time coord")
        .iter()
        .map(|v| v.as_f64().expect("f64"))
        .collect();
    assert_eq!(time, want_time);
}

/// The fixture must actually be the profile it claims — plain v3 `zstd` under
/// `sharding_indexed`, with no Blosc anywhere. A Blosc store would be
/// undecodable in wasm and this test would otherwise not notice.
#[wasm_bindgen_test]
fn the_native_fixture_is_blosc_free_v3_zstd() {
    let map: std::collections::BTreeMap<String, Vec<u8>> =
        serde_json::from_str(NATIVE_STORE).expect("fixture shape");
    let meta = String::from_utf8(map["conc/zarr.json"].clone()).expect("utf8 metadata");
    assert!(meta.contains("sharding_indexed"), "{meta}");
    assert!(meta.contains("\"zstd\""), "{meta}");
    assert!(meta.contains("crc32c"), "{meta}");
    assert!(
        !meta.contains("blosc"),
        "the wasm profile must not use Blosc: {meta}"
    );
}
