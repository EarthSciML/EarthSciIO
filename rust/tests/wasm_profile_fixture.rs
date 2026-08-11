//! The committed cross-read fixture is what **this** writer produces.
//!
//! `tests/fixtures/native_wasm_profile.store.json` is a `wasm`-profile store
//! written by the native writer and captured as `{store key: bytes}`; the wasm32
//! test (`tests/zarr_wasm32.rs`) decodes it inside wasm to prove the two targets
//! share one profile. A fixture is only worth that if it still matches the
//! writer, so this test regenerates the same schema natively and compares.
//!
//! It compares **metadata**, not bytes: compressed shard bytes are not a stable
//! artifact (the RFC's tolerance policy says so, and zstd's output can change
//! with the library version), whereas `zarr.json` is exactly the declaration a
//! reader dispatches on. `attributes._zarrs` — a provenance stamp carrying the
//! zarrs version — is dropped before comparing, so a dependency bump is not a
//! failure but a codec change is.

#![cfg(not(target_arch = "wasm32"))]

use std::collections::BTreeMap;

use earthsciio::{write_zarr_v3, OutputSchema, WriteCoord, WriteVar};
use serde_json::{Map, Value};

const EXPECTED: &str = include_str!("fixtures/native_wasm_profile.expected.json");
const STORE: &str = include_str!("fixtures/native_wasm_profile.store.json");

/// The deterministic smooth field the fixture was generated from.
fn sample_field(n: usize) -> Vec<f64> {
    (0..n)
        .map(|i| (i as f64 * 0.05).sin() * 100.0 + i as f64 * 0.25)
        .collect()
}

/// The exact schema behind the fixture: `conc[time=4, x=6]`, inner chunk
/// `[1, 3]`, shard `[2, 6]`, browser-loadable `wasm` profile.
fn fixture_schema() -> OutputSchema {
    let mut units = Map::new();
    units.insert("units".to_string(), Value::from("kg m-3"));

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
            attrs: units,
            data: sample_field(24),
        }],
        group_attrs: Map::new(),
        profile: "wasm".to_string(),
    }
}

/// `zarr.json` with the zarrs provenance stamp removed.
fn without_provenance(bytes: &[u8]) -> Value {
    let mut v: Value = serde_json::from_slice(bytes).expect("array metadata is JSON");
    if let Some(attrs) = v.get_mut("attributes").and_then(Value::as_object_mut) {
        attrs.remove("_zarrs");
    }
    v
}

#[test]
fn the_committed_fixture_matches_what_the_native_writer_emits_today() {
    let scratch = tempfile::tempdir().unwrap();
    let dir = scratch.path().join("out.zarr");
    write_zarr_v3(&dir, &fixture_schema()).expect("writes");

    let fixture: BTreeMap<String, Vec<u8>> =
        serde_json::from_str(STORE).expect("fixture is {key: bytes}");

    // Every object the fixture claims still exists, with the same metadata.
    for key in [
        "zarr.json",
        "conc/zarr.json",
        "time/zarr.json",
        "x/zarr.json",
    ] {
        let fresh = std::fs::read(dir.join(key)).unwrap_or_else(|e| panic!("{key}: {e}"));
        assert_eq!(
            without_provenance(&fixture[key]),
            without_provenance(&fresh),
            "{key} has drifted from the fixture the wasm32 cross-read decodes"
        );
    }

    // The chunk objects are the same set (their bytes are not pinned — see the
    // module note on the tolerance policy).
    let mut fresh_keys: Vec<String> = Vec::new();
    for entry in walkdir(&dir) {
        fresh_keys.push(
            entry
                .strip_prefix(&dir)
                .expect("under the store root")
                .to_string_lossy()
                .replace('\\', "/"),
        );
    }
    fresh_keys.sort();
    let mut fixture_keys: Vec<String> = fixture.keys().cloned().collect();
    fixture_keys.sort();
    assert_eq!(
        fixture_keys, fresh_keys,
        "the store's object set has changed"
    );
}

#[test]
fn the_fixture_declares_the_values_the_wasm_cross_read_asserts() {
    let expected: Value = serde_json::from_str(EXPECTED).expect("expected fixture");
    let want: Vec<f64> = expected["values"]
        .as_array()
        .expect("values")
        .iter()
        .map(|v| v.as_f64().expect("f64"))
        .collect();
    assert_eq!(
        want,
        sample_field(24),
        "the expected values no longer describe the schema this test writes"
    );
}

/// Every file under `root`, recursively. (A three-line walk beats a dependency.)
fn walkdir(root: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir)
            .expect("readable directory")
            .flatten()
        {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else {
                out.push(path);
            }
        }
    }
    out
}
