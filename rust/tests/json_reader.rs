//! The `json` points reader against the shared cross-language cases in
//! `conformance/corpus/scenarios/json-reader.json`, which the Python
//! (`tests/test_json_reader.py`) and Julia (`julia/test/test_json_reader.jl`)
//! suites read too.

#![cfg(not(target_arch = "wasm32"))]

use std::path::PathBuf;
use std::sync::Arc;

use earthsciio::{
    ArrayData, Cache, DataSource, Error, FormatRegistry, JsonReader, Provider, Reader, Selection,
};
use serde_json::{json, Value};

fn cases() -> Vec<Value> {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../conformance/corpus/scenarios/json-reader.json");
    let doc: Value = serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap();
    doc["cases"].as_array().unwrap().clone()
}

fn decode(dir: &std::path::Path, case: &Value) -> earthsciio::Result<earthsciio::NativeDataset> {
    let id = case["id"].as_str().unwrap();
    let blob = dir.join(format!("{id}.json"));
    std::fs::write(&blob, case["document"].as_str().unwrap()).unwrap();
    let base: Arc<dyn Reader> = Arc::new(JsonReader::new());
    let reader = match case.get("options").and_then(Value::as_object) {
        Some(o) => base.configured(o)?.unwrap_or(base),
        None => base,
    };
    let vars: Vec<String> = case
        .get("variables")
        .and_then(Value::as_array)
        .map(|a| a.iter().map(|v| v.as_str().unwrap().to_string()).collect())
        .unwrap_or_default();
    reader.read_native(&blob, &vars, &Selection::All)
}

#[test]
fn every_shared_case_decodes_as_specified() {
    let dir = tempfile::tempdir().unwrap();
    let mut failures = Vec::new();
    for case in cases() {
        let id = case["id"].as_str().unwrap().to_string();
        let got = decode(dir.path(), &case);
        if let Some(code) = case.get("error").and_then(Value::as_str) {
            match got {
                Err(Error::Format { format, detail })
                    if format == "json" && detail.starts_with(&format!("{code}: ")) => {}
                other => failures.push(format!("{id}: expected error {code}, got {other:?}")),
            }
            continue;
        }
        let ds = match got {
            Ok(ds) => ds,
            Err(e) => {
                failures.push(format!("{id}: unexpected error {e}"));
                continue;
            }
        };
        let want = case["expected"]["fields"].as_object().unwrap();
        let n = case["expected"]["length"].as_u64().unwrap() as usize;
        let mut names: Vec<&String> = ds.variables.keys().collect();
        names.sort();
        let mut want_names: Vec<&String> = want.keys().collect();
        want_names.sort();
        if names != want_names {
            failures.push(format!("{id}: fields {names:?} != {want_names:?}"));
            continue;
        }
        assert!(ds.coords.is_empty(), "{id}: no coordinates");
        for (name, spec) in want {
            let f = &ds.variables[name];
            assert_eq!(f.dims, vec!["index".to_string()], "{id}/{name}");
            assert_eq!(f.shape, vec![n], "{id}/{name}");
            let data = spec["data"].as_array().unwrap();
            let ok = match (spec["dtype"].as_str().unwrap(), &f.data) {
                ("string", ArrayData::Str(v)) => v
                    .iter()
                    .zip(data)
                    .all(|(g, e)| e.as_str() == Some(g.as_str())),
                ("bool", ArrayData::Bool(v)) => {
                    v.iter().zip(data).all(|(g, e)| e.as_bool() == Some(*g))
                }
                ("float64", ArrayData::F64(v)) => v.iter().zip(data).all(|(g, e)| match e {
                    Value::Null => g.is_nan(),
                    e => e.as_f64() == Some(*g),
                }),
                _ => false,
            };
            if !ok {
                failures.push(format!("{id}/{name}: got {:?}, want {spec}", f.data));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn registered_under_json() {
    let reg = FormatRegistry::with_builtins();
    let r = reg.get("json").expect("json is registered");
    assert!(!r.supports_selection());
    let bad = json!({"record_path": ["results"]});
    assert!(r.configured(bad.as_object().unwrap()).is_err());
}

#[test]
fn a_provider_reads_the_declared_options() {
    let case = cases()
        .into_iter()
        .find(|c| c["id"] == "station-discovery")
        .unwrap();
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("stations.json");
    std::fs::write(&p, case["document"].as_str().unwrap()).unwrap();
    let mut options = serde_json::Map::new();
    options.insert("records_path".into(), json!(["results"]));
    let loader = DataSource::new("stations", "json", format!("file://{}", p.display()))
        .variables(["coordinates.latitude", "name"])
        .reader_options(options);
    let cache = Arc::new(
        Cache::builder()
            .data_dir(dir.path().join("cache"))
            .build()
            .unwrap(),
    );
    let mut provider = Provider::new(loader, cache, None).unwrap();
    let fields = provider.materialize().unwrap();
    assert_eq!(fields.len(), 2);
    assert_eq!(
        fields["name"].data,
        ArrayData::Str(vec!["Site A".into(), "Site B".into()])
    );
    assert_eq!(
        fields["coordinates.latitude"].data,
        ArrayData::F64(vec![39.76, 39.5])
    );
}
