//! Zarr v3 writer <-> reader round-trip (streaming-output-sinks, Wave 4).
//!
//! Writes a small sharded Zarr **v3** store with the new `zarrs`-backed writer
//! ([`write_zarr_v3`]), then reads the data + coordinate arrays back through the
//! new `zarrs`-backed reader (the store-backed [`earthsciio::ZarrReader`] driven
//! by a [`Provider`] over a `file://` URL), and asserts decoded-value agreement
//! within tolerance. The written metadata + output manifest are checked directly
//! from the JSON the writer emits (codec pipeline, `dimension_names`, CF attrs).
//!
//! Per the RFC tolerance policy this is a **decoded-value** round-trip, not a
//! byte-identity check.

use std::collections::BTreeMap;
use std::sync::Arc;

use earthsciio::{
    write_zarr_v3, ArrayData, Cache, DataLoader, NativeField, OutputSchema, Provider, WriteCoord,
    WriteVar,
};
use serde_json::{Map, Value};

const RTOL: f64 = 1e-6;

fn attrs(pairs: &[(&str, Value)]) -> Map<String, Value> {
    pairs.iter().map(|(k, v)| (k.to_string(), v.clone())).collect()
}

fn assert_close(got: &NativeField, want: &[f64], ctx: &str) {
    let ArrayData::F64(v) = &got.data else {
        panic!("{ctx}: expected F64 data, got {:?}", got.dtype);
    };
    assert_eq!(v.len(), want.len(), "{ctx}: length mismatch");
    for (i, (&g, &w)) in v.iter().zip(want).enumerate() {
        let tol = RTOL * w.abs().max(1.0);
        assert!(
            (g - w).abs() <= tol,
            "{ctx}: element {i} = {g}, expected {w} (tol {tol})"
        );
    }
}

#[test]
fn write_then_read_roundtrip_within_tolerance() {
    let scratch = tempfile::tempdir().unwrap();
    let store_dir = scratch.path().join("out.zarr");
    let cache_dir = scratch.path().join("cache");

    // --- schema: time (2 records) x y (4) x x (3), one variable `conc` ------- //
    let time_vals = vec![0.0, 3.0];
    let x_vals = vec![-100.0, -99.5, -99.0];
    let y_vals = vec![40.0, 40.5, 41.0, 41.5];
    // conc[t, y, x] = t*1000 + y_idx*10 + x_idx  (C-order over [time, y, x])
    let mut conc = Vec::new();
    for t in 0..2 {
        for yi in 0..4 {
            for xi in 0..3 {
                conc.push((t * 1000 + yi * 10 + xi) as f64);
            }
        }
    }

    let chunk_shape: BTreeMap<String, usize> =
        [("time", 1), ("y", 2), ("x", 3)].iter().map(|(k, v)| (k.to_string(), *v)).collect();
    let shard_shape: BTreeMap<String, usize> =
        [("time", 2), ("y", 4), ("x", 3)].iter().map(|(k, v)| (k.to_string(), *v)).collect();

    let schema = OutputSchema {
        dims: vec![("time".into(), 2), ("y".into(), 4), ("x".into(), 3)],
        time_dim: "time".into(),
        chunk_shape,
        shard_shape,
        coords: vec![
            WriteCoord {
                name: "time".into(),
                values: time_vals.clone(),
                attrs: attrs(&[
                    ("units", Value::from("hours since 2018-01-01 00:00:00")),
                    ("calendar", Value::from("gregorian")),
                ]),
            },
            WriteCoord {
                name: "x".into(),
                values: x_vals.clone(),
                attrs: attrs(&[("units", Value::from("degrees_east"))]),
            },
            WriteCoord {
                name: "y".into(),
                values: y_vals.clone(),
                attrs: attrs(&[("units", Value::from("degrees_north"))]),
            },
        ],
        vars: vec![WriteVar {
            name: "conc".into(),
            dims: vec!["time".into(), "y".into(), "x".into()],
            attrs: attrs(&[
                ("units", Value::from("ug/m3")),
                ("standard_name", Value::from("mass_concentration")),
                ("coordinates", Value::from("x y")),
            ]),
            data: conc.clone(),
        }],
        group_attrs: attrs(&[("title", Value::from("roundtrip fixture"))]),
        profile: "diagnostic".into(),
    };

    write_zarr_v3(&store_dir, &schema).expect("write zarr v3 store");

    // --- read back through the new ZarrReader (Provider over file://) -------- //
    let base_url = format!("file://{}", store_dir.display());
    let cache = Arc::new(Cache::builder().data_dir(&cache_dir).build().expect("cache"));
    let loader = DataLoader::new("roundtrip", "zarr", &base_url)
        .variables(["conc", "time", "x", "y"]);
    let mut provider = Provider::new(loader, cache, None).expect("provider");
    let buffers = provider.materialize().expect("materialize written store");

    // Variable `conc`: dims + shape + values (C-order) agree within tolerance.
    let got = &buffers["conc"];
    assert_eq!(got.dims, vec!["time", "y", "x"]);
    assert_eq!(got.shape, vec![2, 4, 3]);
    assert_close(got, &conc, "conc");

    // Coordinate arrays read back as ordinary arrays.
    assert_close(&buffers["time"], &time_vals, "time coord");
    assert_close(&buffers["x"], &x_vals, "x coord");
    assert_close(&buffers["y"], &y_vals, "y coord");

    // --- metadata + manifest checks straight from the written JSON ----------- //
    let conc_meta: Value = serde_json::from_slice(
        &std::fs::read(store_dir.join("conc/zarr.json")).expect("conc/zarr.json"),
    )
    .unwrap();
    assert_eq!(conc_meta["zarr_format"], 3);
    assert_eq!(conc_meta["data_type"], "float64");
    assert_eq!(conc_meta["dimension_names"], serde_json::json!(["time", "y", "x"]));
    assert_eq!(conc_meta["attributes"]["units"], "ug/m3");
    assert_eq!(conc_meta["attributes"]["coordinates"], "x y");
    assert_eq!(conc_meta["attributes"]["_ARRAY_DIMENSIONS"], serde_json::json!(["time", "y", "x"]));
    // Sharding codec pipeline: sharding_indexed(inner=[bytes,blosc], index=[bytes,crc32c]).
    let codec = &conc_meta["codecs"][0];
    assert_eq!(codec["name"], "sharding_indexed");
    assert_eq!(codec["configuration"]["chunk_shape"], serde_json::json!([1, 2, 3]));
    assert_eq!(codec["configuration"]["codecs"][1]["name"], "blosc");
    assert_eq!(codec["configuration"]["codecs"][1]["configuration"]["cname"], "zstd");
    assert_eq!(codec["configuration"]["index_codecs"][1]["name"], "crc32c");
    // The outer chunk grid is the shard shape.
    assert_eq!(
        conc_meta["chunk_grid"]["configuration"]["chunk_shape"],
        serde_json::json!([2, 4, 3])
    );

    let manifest: Value = serde_json::from_slice(
        &std::fs::read(store_dir.join("output_manifest.json")).expect("output_manifest.json"),
    )
    .unwrap();
    assert_eq!(manifest["schema"], "earthsciio/output-manifest/v1");
    assert_eq!(manifest["zarr_format"], 3);
    assert_eq!(manifest["time_dim"], "time");
    assert_eq!(manifest["n_records"], 2);
    assert_eq!(manifest["last_t"], 3.0);
    assert_eq!(manifest["codec"]["cname"], "zstd");
    assert_eq!(manifest["vars"][0]["name"], "conc");
}
