//! Rust track's WRITE driver for the cross-language write-conformance harness.
//!
//! Drives the **Rust Zarr v3 sharded writer** ([`earthsciio::write_zarr_v3`], built
//! on the `zarrs` crate) from the shared, language-neutral input spec
//! (`conformance/write_spec.json`) and emits a Zarr v3 sharded store into an output
//! directory. The write mirror of `rust/examples/conformance_dump.rs`
//! (streaming-output-sinks RFC, Wave 5).
//!
//! Unlike the Julia/Python streaming writers (`write_record` per time step), the
//! Rust writer commits the whole dataset in one [`write_zarr_v3`] call, so this
//! driver flattens every record into each variable's row-major (C-order) buffer
//! over `(time, lat, lon)` and sets the `time` dim length to the record count.
//!
//! The store this produces is read back by every available track's reader and
//! cross-checked, and its `zarr.json` metadata is structurally compared against the
//! Python- and Julia-written stores by `conformance/crosscheck_write.py`.
//! Conformance is TOLERANCE-BASED on decoded arrays (RFC §16.6), never byte identity.
//!
//! Usage:  cargo run --example conformance_write -- OUT_DIR [SPEC.json]

use std::collections::BTreeMap;
use std::path::PathBuf;

use earthsciio::{write_zarr_v3, OutputSchema, WriteCoord, WriteVar};
use serde_json::{Map, Value};

fn read_json(path: &std::path::Path) -> Value {
    let bytes = std::fs::read(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    serde_json::from_slice(&bytes).unwrap_or_else(|e| panic!("parse {}: {e}", path.display()))
}

fn spec_path(explicit: Option<&String>) -> PathBuf {
    match explicit {
        Some(p) => PathBuf::from(p),
        None => PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../conformance/write_spec.json"),
    }
}

fn as_map(v: &Value) -> Map<String, Value> {
    v.as_object().cloned().unwrap_or_default()
}

fn f64s(v: &Value) -> Vec<f64> {
    v.as_array()
        .expect("array of numbers")
        .iter()
        .map(|x| x.as_f64().expect("number"))
        .collect()
}

fn build_schema(spec: &Value) -> OutputSchema {
    let time_dim = spec["time_dim"].as_str().unwrap().to_string();
    let records = spec["records"].as_array().unwrap();
    let n_rec = records.len();

    // dims: the time dim's length is the number of records (whole-dataset write).
    let dims: Vec<(String, usize)> = spec["dims"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| {
            let name = d[0].as_str().unwrap().to_string();
            let len = if name == time_dim {
                n_rec
            } else {
                d[1].as_u64().unwrap() as usize
            };
            (name, len)
        })
        .collect();

    let usize_map = |key: &str| -> BTreeMap<String, usize> {
        as_map(&spec[key])
            .iter()
            .map(|(k, v)| (k.clone(), v.as_u64().unwrap() as usize))
            .collect()
    };

    // Coordinates: static values as given; the time coord's values come from the
    // per-record `t` (the spec leaves its `values` empty on purpose).
    let time_vals: Vec<f64> = records.iter().map(|r| r["t"].as_f64().unwrap()).collect();
    let coords: Vec<WriteCoord> = spec["coords"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| {
            let name = c["name"].as_str().unwrap().to_string();
            let values = if name == time_dim { time_vals.clone() } else { f64s(&c["values"]) };
            WriteCoord { name, values, attrs: as_map(&c["attrs"]) }
        })
        .collect();

    // Variables: flatten every record into a row-major (time, lat, lon) buffer.
    let vars: Vec<WriteVar> = spec["vars"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| {
            let name = v["name"].as_str().unwrap().to_string();
            let vdims: Vec<String> =
                v["dims"].as_array().unwrap().iter().map(|d| d.as_str().unwrap().to_string()).collect();
            assert_eq!(
                vdims,
                vec![time_dim.clone(), "lat".to_string(), "lon".to_string()],
                "this driver assembles (time, lat, lon) vars"
            );
            let mut data = Vec::new();
            for r in records {
                let block = r["vars"][&name].as_array().expect("[lat][lon] block");
                for row in block {
                    for x in row.as_array().expect("[lon] row") {
                        data.push(x.as_f64().unwrap());
                    }
                }
            }
            WriteVar { name, dims: vdims, attrs: as_map(&v["attrs"]), data }
        })
        .collect();

    OutputSchema {
        dims,
        time_dim,
        chunk_shape: usize_map("chunk_shape"),
        shard_shape: usize_map("shard_shape"),
        coords,
        vars,
        group_attrs: as_map(&spec["group_attrs"]),
        profile: spec["profile"].as_str().unwrap().to_string(),
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let out_dir = PathBuf::from(args.get(1).expect("usage: conformance_write OUT_DIR [SPEC.json]"));
    let spec = read_json(&spec_path(args.get(2)));

    let schema = build_schema(&spec);
    write_zarr_v3(&out_dir, &schema).expect("write the Zarr v3 sharded store");

    println!(
        "[rust-writer] wrote {} records to {} (profile={}, {} vars)",
        spec["records"].as_array().unwrap().len(),
        out_dir.display(),
        spec["profile"].as_str().unwrap(),
        schema.vars.len(),
    );
}
