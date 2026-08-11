//! The writer runs on `wasm32-unknown-unknown`, and its stores cross-read.
//!
//! This is the acceptance test the streaming-output-sinks RFC named as never
//! attempted: build the `wasm` codec profile — `sharding_indexed` over plain v3
//! `zstd`, with a `crc32c` shard index and no Blosc — for wasm32 and *execute*
//! it. It runs under Node (`wasm-bindgen-test-runner`'s default), because the
//! codec chain has nothing to do with the DOM:
//!
//! ```text
//! CC_wasm32_unknown_unknown=$(brew --prefix llvm)/bin/clang \
//! AR_wasm32_unknown_unknown=$(brew --prefix llvm)/bin/llvm-ar \
//! cargo test --target wasm32-unknown-unknown --no-default-features --features opfs
//! ```
//!
//! The **storage** half — OPFS proper — cannot run here: `createSyncAccessHandle`
//! is a browser Worker API that does not exist in Node. It is exercised
//! end-to-end from earthscilab's Playwright tier, in three engines, against a
//! model actually solved in the browser. What [`OpfsStore`] contributes here is
//! its staging half, which is a plain in-memory `zarrs` store until `commit`.
//!
//! `fixtures/native_wasm_profile.store.json` is a `wasm`-profile store written
//! by **this crate's native writer**, captured as `{store key: bytes}` and
//! pinned to the current writer by `tests/wasm_profile_fixture.rs`. Decoding it
//! here is the cross-read: it is what says the two targets share one profile
//! rather than each being self-consistent.

#![cfg(all(target_arch = "wasm32", feature = "opfs"))]
// `Arc` is what the `zarrs` API takes and there are no threads on this target;
// see the note at the top of `src/format/zarr_opfs.rs`.
#![allow(clippy::arc_with_non_send_sync)]

use std::collections::BTreeMap;
use std::sync::Arc;

use earthsciio::format::OpfsStore;
use earthsciio::{write_all_to_store, OutputSchema, WriteCoord, WriteVar};
use serde_json::{Map, Value};
use wasm_bindgen_test::wasm_bindgen_test;
use zarrs::array::Array;
use zarrs::storage::{ReadableStorageTraits, StoreKey, WritableStorageTraits};

const STORE: &str = include_str!("fixtures/native_wasm_profile.store.json");
const EXPECTED: &str = include_str!("fixtures/native_wasm_profile.expected.json");

fn sample_field(n: usize) -> Vec<f64> {
    (0..n)
        .map(|i| (i as f64 * 0.05).sin() * 100.0 + i as f64 * 0.25)
        .collect()
}

/// The same schema `tests/wasm_profile_fixture.rs` writes natively.
fn fixture_schema() -> OutputSchema {
    OutputSchema {
        dims: vec![("time".to_string(), 4), ("x".to_string(), 6)],
        time_dim: "time".to_string(),
        chunk_shape: BTreeMap::from([("time".to_string(), 1), ("x".to_string(), 3)]),
        shard_shape: BTreeMap::from([("time".to_string(), 2), ("x".to_string(), 6)]),
        coords: vec![
            WriteCoord {
                name: "time".to_string(),
                values: vec![0.0, 6.0, 12.0, 18.0],
                attrs: Map::new(),
            },
            WriteCoord {
                name: "x".to_string(),
                values: vec![0.0, 1000.0, 2000.0, 3000.0, 4000.0, 5000.0],
                attrs: Map::from_iter([("units".to_string(), Value::from("m"))]),
            },
        ],
        vars: vec![WriteVar {
            name: "conc".to_string(),
            dims: vec!["time".to_string(), "x".to_string()],
            attrs: Map::from_iter([("units".to_string(), Value::from("kg m-3"))]),
            data: sample_field(24),
        }],
        group_attrs: Map::new(),
        profile: "wasm".to_string(),
    }
}

fn read_all_f64<S>(store: Arc<S>, name: &str) -> Vec<f64>
where
    S: ReadableStorageTraits + 'static,
{
    let array = Array::open(store, &format!("/{name}")).expect("open array");
    array
        .retrieve_array_subset::<Vec<f64>>(&array.subset_all())
        .expect("decode array")
}

fn load_fixture<S: WritableStorageTraits>(store: &S, json: &str) -> usize {
    let map: BTreeMap<String, Vec<u8>> = serde_json::from_str(json).expect("{key: bytes}");
    let n = map.len();
    for (key, bytes) in map {
        store
            .set(
                &StoreKey::new(&key).expect("store key"),
                zarrs::storage::Bytes::from(bytes),
            )
            .expect("stage fixture object");
    }
    n
}

/// The crate's own writer — `write_all_to_store`, the very function the server
/// tier calls — encodes a whole `wasm`-profile dataset inside wasm, and the
/// result decodes exactly.
#[wasm_bindgen_test]
fn the_writer_round_trips_a_whole_dataset_inside_wasm() {
    let schema = fixture_schema();
    let store = Arc::new(OpfsStore::staging());
    write_all_to_store(store.clone(), "opfs:/test", &schema).expect("write the dataset");

    assert_eq!(
        read_all_f64(store.clone(), "conc"),
        sample_field(24),
        "the round-trip through zstd + sharding must be exact"
    );
    assert_eq!(
        read_all_f64(store.clone(), "time"),
        vec![0.0, 6.0, 12.0, 18.0]
    );
    assert_eq!(
        read_all_f64(store.clone(), "x"),
        vec![0.0, 1000.0, 2000.0, 3000.0, 4000.0, 5000.0]
    );

    // The manifest is part of the artifact, not an afterthought: the loader-only
    // `.esm` a saved dataset becomes is synthesized from it.
    let manifest = store
        .get_whole("output_manifest.json")
        .expect("manifest read")
        .expect("manifest present");
    let manifest: Value = serde_json::from_slice(&manifest).expect("manifest is JSON");
    assert_eq!(manifest["profile"], "wasm");
    assert_eq!(manifest["n_records"], 4);
    assert_eq!(manifest["codec"]["id"], "zstd");
}

/// **The cross-read.** A store the NATIVE writer produced is decoded by the
/// wasm build, element for element. If the two `wasm` profiles were not the same
/// profile, this is what would say so.
#[wasm_bindgen_test]
fn a_natively_written_store_decodes_in_wasm() {
    let expected: Value = serde_json::from_str(EXPECTED).expect("expected fixture");
    let want: Vec<f64> = expected["values"]
        .as_array()
        .expect("values")
        .iter()
        .map(|v| v.as_f64().expect("f64"))
        .collect();

    let store = Arc::new(OpfsStore::staging());
    let n = load_fixture(store.as_ref(), STORE);
    assert!(
        n >= 4,
        "fixture should carry the whole store, got {n} objects"
    );

    assert_eq!(
        read_all_f64(store.clone(), "conc"),
        want,
        "native-written chunks must decode bit-exactly in wasm"
    );
    // The coordinate arrays are separate v3 arrays with the same codec chain.
    let want_time: Vec<f64> = expected["coords"]["time"]
        .as_array()
        .expect("time coord")
        .iter()
        .map(|v| v.as_f64().expect("f64"))
        .collect();
    assert_eq!(read_all_f64(store, "time"), want_time);
}

/// A store written in wasm and one written natively declare the *same* codec
/// chain — the claim the cross-read above would satisfy by luck if the profiles
/// happened to agree on one array but not on the pipeline.
#[wasm_bindgen_test]
fn the_wasm_and_native_writers_declare_the_same_codec_chain() {
    let store = Arc::new(OpfsStore::staging());
    write_all_to_store(store.clone(), "opfs:/test", &fixture_schema()).expect("write");
    let here = store
        .get_whole("conc/zarr.json")
        .expect("read")
        .expect("present");
    let mut here: Value = serde_json::from_slice(&here).expect("JSON");

    let fixture: BTreeMap<String, Vec<u8>> = serde_json::from_str(STORE).expect("fixture");
    let mut there: Value =
        serde_json::from_slice(&fixture["conc/zarr.json"]).expect("fixture metadata");

    // `_zarrs` is a provenance stamp carrying the zarrs version, not part of the
    // profile.
    for v in [&mut here, &mut there] {
        if let Some(attrs) = v.get_mut("attributes").and_then(Value::as_object_mut) {
            attrs.remove("_zarrs");
        }
    }
    assert_eq!(here, there);
    assert!(
        !here.to_string().contains("blosc"),
        "the wasm profile must not use Blosc"
    );
}
