//! object_store-backed Zarr v3 round-trip (feature `object-store`).
//!
//! Exercises the SAME `object_store` code path used for S3, but over a local
//! `file://` URL (object_store's `LocalFileSystem` — no network, no AWS), so the
//! async->sync bridge, the `zarrs_object_store` adapter, and the shared
//! read/write logic are actually executed end-to-end. S3 uses the identical code
//! with a different `parse_url` backend.

#![cfg(feature = "object-store")]

use std::collections::BTreeMap;

use earthsciio::{
    read_zarr_object_store, write_zarr_object_store, ArrayData, OutputSchema, Selection, WriteCoord,
    WriteVar,
};
use serde_json::{Map, Value};

fn attrs(pairs: &[(&str, Value)]) -> Map<String, Value> {
    pairs.iter().map(|(k, v)| (k.to_string(), v.clone())).collect()
}

fn f64s(data: &ArrayData) -> &Vec<f64> {
    match data {
        ArrayData::F64(v) => v,
        other => panic!("expected F64, got {other:?}"),
    }
}

#[test]
fn object_store_write_then_read_roundtrip() {
    let scratch = tempfile::tempdir().unwrap();
    let store_dir = scratch.path().join("os.zarr");
    std::fs::create_dir_all(&store_dir).unwrap();
    let url = format!("file://{}", store_dir.display());

    let time_vals = vec![0.0, 6.0];
    let x_vals = vec![1.0, 2.0, 3.0];
    // conc[t, x] = t*10 + x_idx  (C-order over [time, x])
    let conc: Vec<f64> = (0..2).flat_map(|t| (0..3).map(move |xi| (t * 10 + xi) as f64)).collect();

    let chunk_shape: BTreeMap<String, usize> =
        [("time", 1), ("x", 3)].iter().map(|(k, v)| (k.to_string(), *v)).collect();
    let shard_shape: BTreeMap<String, usize> =
        [("time", 2), ("x", 3)].iter().map(|(k, v)| (k.to_string(), *v)).collect();

    let schema = OutputSchema {
        dims: vec![("time".into(), 2), ("x".into(), 3)],
        time_dim: "time".into(),
        chunk_shape,
        shard_shape,
        coords: vec![
            WriteCoord { name: "time".into(), values: time_vals.clone(), attrs: Map::new() },
            WriteCoord {
                name: "x".into(),
                values: x_vals.clone(),
                attrs: attrs(&[("units", Value::from("m"))]),
            },
        ],
        vars: vec![WriteVar {
            name: "conc".into(),
            dims: vec!["time".into(), "x".into()],
            attrs: attrs(&[("units", Value::from("kg"))]),
            data: conc.clone(),
        }],
        group_attrs: Map::new(),
        profile: "checkpoint".into(),
    };

    write_zarr_object_store(&url, &schema).expect("object_store write");

    let ds = read_zarr_object_store(
        &url,
        &["conc".to_string(), "x".to_string()],
        &Selection::All,
    )
    .expect("object_store read");

    let conc_got = &ds.variables["conc"];
    assert_eq!(conc_got.dims, vec!["time", "x"]);
    assert_eq!(conc_got.shape, vec![2, 3]);
    assert_eq!(f64s(&conc_got.data), &conc);
    assert_eq!(f64s(&ds.variables["x"].data), &x_vals);

    // The output manifest was written through the object_store as an object.
    assert!(store_dir.join("output_manifest.json").exists());
}
