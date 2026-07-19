//! Cross-language decode parity (conformance checks 3 & 4): point the `format`
//! registry's reader at each committed corpus blob — a `$EARTHSCIDATADIR`
//! populated by the **Python** generator — decode it fully offline, and assert
//! the native arrays equal the case's `expected` arrays.
//!
//! This is the half of conformance that component (a)'s `conformance_reuse.rs`
//! explicitly defers to (b): "Decoding the blob into native arrays (checks 3–4)
//! is component (b)." The corpus `expected` arrays are the cross-language oracle
//! — equality here is what "matching the Python and Julia tracks" means.
//!
//! Cases whose format has no Rust reader yet (e.g. `csv`) are skipped with a
//! note, so the test runs every decodable case today and picks up new readers
//! (csv/geotiff/zarr) automatically as they register — no edit here.

use std::fs;
use std::path::PathBuf;

use earthsciio::{ArrayData, AxisSelect, Coord, DType};
use earthsciio::{Cache, FetchRequest, FormatRegistry, NativeField, Selection};
use serde_json::Value;

/// Parse a case's `select.axes` into a `Selection::Orthogonal` (store-backed
/// zarr cases); absent ⇒ `Selection::All`.
fn parse_selection(case: &Value) -> Selection {
    match case.get("select").and_then(|s| s.get("axes")).and_then(Value::as_array) {
        Some(arr) => Selection::Orthogonal(arr.iter().map(parse_axis).collect()),
        None => Selection::All,
    }
}

fn parse_axis(v: &Value) -> AxisSelect {
    if v.as_str() == Some("all") {
        return AxisSelect::All;
    }
    if let Some(idx) = v.get("indices").and_then(Value::as_array) {
        return AxisSelect::Indices(idx.iter().map(|x| x.as_u64().unwrap() as usize).collect());
    }
    if let Some(s) = v.get("slice").and_then(Value::as_array) {
        let g = |i: usize, d: u64| s.get(i).and_then(Value::as_u64).unwrap_or(d) as usize;
        return AxisSelect::Range { start: g(0, 0), stop: g(1, 0), step: g(2, 1) };
    }
    panic!("unrecognized axis selector: {v}")
}

fn corpus_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../conformance/corpus")
}

/// Compared exactly for raw/unpacked reads; CF-decoded (packed) values differ at
/// the ULP level across libraries, so within `atol` (conformance.md §4).
const ATOL: f64 = 1e-6;

#[test]
fn decodes_every_corpus_case_to_match_expected() {
    let corpus = corpus_dir();
    let cache = Cache::builder()
        .data_dir(corpus.join("cache"))
        .offline(true)
        .verify_on_read(true)
        .build()
        .expect("offline cache over the corpus");

    let formats = FormatRegistry::with_builtins();

    let index: Value =
        serde_json::from_slice(&fs::read(corpus.join("cases.json")).unwrap()).unwrap();
    let cases = index["cases"].as_array().expect("cases array");
    assert!(!cases.is_empty(), "corpus must ship at least one case");

    let mut decoded_any = false;
    for entry in cases {
        let case: Value = serde_json::from_slice(
            &fs::read(corpus.join(entry["file"].as_str().unwrap())).unwrap(),
        )
        .unwrap();
        let id = case["id"].as_str().unwrap();
        let format = case["format"].as_str().unwrap();

        let Some(reader) = formats.get(format) else {
            eprintln!("skip case {id}: no Rust reader for format '{format}' yet");
            continue;
        };
        decoded_any = true;

        // Store-backed (zarr): a Zarr store is many objects, not one blob — the
        // reader is handed (cache, base_url, variables, select) and fetches only
        // the intersecting chunk objects itself. Whole-file readers take the
        // single-blob path.
        let ds = if reader.store_backed() {
            let vars: Vec<String> = case["variables"]
                .as_array()
                .expect("store-backed case has a variables array")
                .iter()
                .map(|v| v.as_str().unwrap().to_string())
                .collect();
            let sel = parse_selection(&case);
            reader
                .read_store(&cache, case["resolved_url"].as_str().unwrap(), &vars, &sel)
                .unwrap_or_else(|e| panic!("store decode failed for {id}: {e}"))
        } else {
            // Resolve the blob offline (reuses the Python-cached bytes), then decode.
            let blob = cache
                .fetch(&FetchRequest::new(case["resolved_url"].as_str().unwrap()))
                .unwrap_or_else(|e| panic!("offline resolve failed for {id}: {e}"));
            reader
                .read_native(&blob.path, &[], &Selection::All)
                .unwrap_or_else(|e| panic!("decode failed for {id}: {e}"))
        };

        // Check 4a: data variables.
        let exp_vars = case["expected"]["variables"].as_object().unwrap();
        assert_eq!(
            ds.variables.len(),
            exp_vars.len(),
            "{id}: variable count (got {:?})",
            ds.variables.keys().collect::<Vec<_>>()
        );
        for (name, exp) in exp_vars {
            let got = ds
                .variables
                .get(name)
                .unwrap_or_else(|| panic!("{id}: missing variable {name}"));
            compare_field(id, name, got, exp);
        }

        // Check 4b: coordinates (the corpus pins dtype + values, not dims/shape).
        let exp_coords = case["expected"]["coords"].as_object().unwrap();
        for (name, exp) in exp_coords {
            let got = ds
                .coords
                .get(name)
                .unwrap_or_else(|| panic!("{id}: missing coord {name}"));
            compare_coord(id, name, got, exp);
        }
    }
    assert!(
        decoded_any,
        "no corpus case was decodable — expected ≥1 (netcdf)"
    );
}

fn compare_field(id: &str, name: &str, got: &NativeField, exp: &Value) {
    assert_eq!(
        dtype_str(got.dtype),
        exp["dtype"].as_str().unwrap(),
        "{id}/{name}: dtype"
    );
    let exp_dims: Vec<String> = exp["dims"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect();
    assert_eq!(got.dims, exp_dims, "{id}/{name}: dims");
    let exp_shape: Vec<usize> = exp["shape"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_u64().unwrap() as usize)
        .collect();
    assert_eq!(got.shape, exp_shape, "{id}/{name}: shape");
    compare_values(id, name, &got.data, &exp["data"]);
}

fn compare_coord(id: &str, name: &str, got: &Coord, exp: &Value) {
    assert_eq!(
        dtype_str(got.field.dtype),
        exp["dtype"].as_str().unwrap(),
        "{id}/{name}: coord dtype"
    );
    if let Some(units) = exp.get("units").and_then(Value::as_str) {
        assert_eq!(
            got.units.as_deref(),
            Some(units),
            "{id}/{name}: coord units"
        );
    }
    if let Some(cal) = exp.get("calendar").and_then(Value::as_str) {
        assert_eq!(
            got.calendar.as_deref(),
            Some(cal),
            "{id}/{name}: coord calendar"
        );
    }
    compare_values(id, name, &got.field.data, &exp["data"]);
}

/// Compare a decoded array against the corpus's nested `data`: element count and
/// value-by-value (null ↔ NaN; numbers within `ATOL`; strings exact).
fn compare_values(id: &str, name: &str, got: &ArrayData, exp: &Value) {
    match got {
        ArrayData::Str(v) => {
            let expected = flatten_str(exp);
            assert_eq!(v.len(), expected.len(), "{id}/{name}: string len");
            assert_eq!(v, &expected, "{id}/{name}: string values");
        }
        _ => {
            let got_f = to_opt_f64(got);
            let expected = flatten_f64(exp);
            assert_eq!(got_f.len(), expected.len(), "{id}/{name}: element count");
            for (i, (g, e)) in got_f.iter().zip(expected.iter()).enumerate() {
                match (g, e) {
                    (None, None) => {}
                    (Some(a), Some(b)) => assert!(
                        (a - b).abs() <= ATOL,
                        "{id}/{name}[{i}]: {a} != {b} (atol {ATOL})"
                    ),
                    _ => panic!("{id}/{name}[{i}]: fill mask mismatch (got {g:?}, expected {e:?})"),
                }
            }
        }
    }
}

fn dtype_str(d: DType) -> &'static str {
    match d {
        DType::Float64 => "float64",
        DType::Int64 => "int64",
        DType::Int32 => "int32",
        DType::Str => "string",
        DType::Bool => "bool",
    }
}

fn to_opt_f64(data: &ArrayData) -> Vec<Option<f64>> {
    match data {
        ArrayData::F64(v) => v
            .iter()
            .map(|&x| if x.is_nan() { None } else { Some(x) })
            .collect(),
        ArrayData::I64(v) => v.iter().map(|&x| Some(x as f64)).collect(),
        ArrayData::I32(v) => v.iter().map(|&x| Some(x as f64)).collect(),
        ArrayData::Bool(v) => v.iter().map(|&x| Some(x as i64 as f64)).collect(),
        ArrayData::Str(_) => panic!("string array compared as numeric"),
    }
}

/// Flatten a nested JSON array of numbers/null into row-major `Option<f64>`.
fn flatten_f64(v: &Value) -> Vec<Option<f64>> {
    let mut out = Vec::new();
    fn rec(v: &Value, out: &mut Vec<Option<f64>>) {
        match v {
            Value::Array(a) => a.iter().for_each(|x| rec(x, out)),
            Value::Null => out.push(None),
            Value::Number(n) => out.push(Some(n.as_f64().unwrap())),
            other => panic!("unexpected value in numeric data: {other}"),
        }
    }
    rec(v, &mut out);
    out
}

/// Flatten a nested JSON array of strings into row-major order.
fn flatten_str(v: &Value) -> Vec<String> {
    let mut out = Vec::new();
    fn rec(v: &Value, out: &mut Vec<String>) {
        match v {
            Value::Array(a) => a.iter().for_each(|x| rec(x, out)),
            Value::String(s) => out.push(s.clone()),
            other => panic!("unexpected value in string data: {other}"),
        }
    }
    rec(v, &mut out);
    out
}
