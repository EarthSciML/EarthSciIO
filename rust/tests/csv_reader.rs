//! The `csv` reader: the same decode as the Python and Julia `CSVReader`
//! (`spec/registries.md` §2), through the registry and through a Provider whose
//! loader declares the reader options.

#![cfg(not(target_arch = "wasm32"))]

use std::path::Path;
use std::sync::Arc;

use earthsciio::{
    ArrayData, Cache, CsvReader, DType, DataSource, FormatRegistry, NativeDataset, Provider,
    Reader, Selection,
};
use serde_json::json;

fn write(dir: &Path, name: &str, text: &str) -> std::path::PathBuf {
    let p = dir.join(name);
    std::fs::write(&p, text).unwrap();
    p
}

fn read(reader: &dyn Reader, path: &Path, vars: &[&str]) -> earthsciio::Result<NativeDataset> {
    let vars: Vec<String> = vars.iter().map(|s| s.to_string()).collect();
    reader.read_native(path, &vars, &Selection::All)
}

fn configured(options: serde_json::Value) -> Arc<dyn Reader> {
    let reg = FormatRegistry::with_builtins();
    let base = reg.get("csv").expect("csv is registered");
    base.configured(options.as_object().unwrap())
        .unwrap()
        .unwrap_or(base)
}

fn f64s(ds: &NativeDataset, v: &str) -> Vec<f64> {
    match &ds.variables[v].data {
        ArrayData::F64(x) => x.clone(),
        other => panic!("{v}: expected f64, got {:?}", other.dtype()),
    }
}

fn strs(ds: &NativeDataset, v: &str) -> Vec<String> {
    match &ds.variables[v].data {
        ArrayData::Str(x) => x.clone(),
        other => panic!("{v}: expected strings, got {:?}", other.dtype()),
    }
}

const TABLE: &str =
    "id,lat,name,value\n1,39.5,\"Chico, CA\",1.5\n2,40.25,Davis,2\n\n3,41,Reno, 3e1\n";

#[test]
fn registered_as_csv_and_refuses_selection() {
    let reg = FormatRegistry::with_builtins();
    let r = reg.get("csv").expect("csv reader");
    assert!(!r.supports_selection());
    assert!(!r.store_backed());
}

#[test]
fn infers_numeric_columns_when_none_are_named() {
    let dir = tempfile::tempdir().unwrap();
    let p = write(dir.path(), "t.csv", TABLE);
    let ds = read(&CsvReader::new(), &p, &[]).unwrap();
    assert!(ds.coords.is_empty());
    assert_eq!(ds.variables.len(), 4);
    // A digit-only id column is inferred numeric — which is why a loader names
    // its numeric columns explicitly.
    assert_eq!(f64s(&ds, "id"), vec![1.0, 2.0, 3.0]);
    assert_eq!(f64s(&ds, "lat"), vec![39.5, 40.25, 41.0]);
    assert_eq!(
        f64s(&ds, "value"),
        vec![1.5, 2.0, 30.0],
        "whitespace is trimmed"
    );
    // A quoted field keeps its embedded delimiter; the blank line is skipped.
    assert_eq!(strs(&ds, "name"), vec!["Chico, CA", "Davis", "Reno"]);
    for f in ds.variables.values() {
        assert_eq!(f.dims, vec!["index".to_string()]);
        assert_eq!(f.shape, vec![3]);
        assert_eq!(f.fill_value, None);
    }
}

#[test]
fn named_numeric_columns_keep_the_rest_as_text() {
    let dir = tempfile::tempdir().unwrap();
    let p = write(dir.path(), "t.csv", TABLE);
    let r = configured(json!({"numeric_columns": ["lat", "value"]}));
    let ds = read(r.as_ref(), &p, &[]).unwrap();
    assert_eq!(ds.variables["id"].dtype, DType::Str);
    assert_eq!(strs(&ds, "id"), vec!["1", "2", "3"]);
    assert_eq!(f64s(&ds, "value"), vec![1.5, 2.0, 30.0]);
}

#[test]
fn a_blank_numeric_cell_is_an_error_not_nan() {
    let dir = tempfile::tempdir().unwrap();
    let p = write(dir.path(), "t.csv", "a,b\n1,\n2,3\n");
    let r = configured(json!({"numeric_columns": ["a", "b"]}));
    let e = read(r.as_ref(), &p, &[]).unwrap_err().to_string();
    assert!(e.contains("\"b\"") && e.contains("line 2"), "{e}");
    // Inferred, the blank makes the column text.
    let ds = read(&CsvReader::new(), &p, &[]).unwrap();
    assert_eq!(strs(&ds, "b"), vec!["", "3"]);
}

#[test]
fn delimiter_and_header_row() {
    let dir = tempfile::tempdir().unwrap();
    let p = write(dir.path(), "t.txt", "a title line\nx;y\n1;a\n2;b\n");
    let r = configured(json!({"delimiter": ";", "header_row": 1}));
    let ds = read(r.as_ref(), &p, &[]).unwrap();
    assert_eq!(f64s(&ds, "x"), vec![1.0, 2.0]);
    assert_eq!(strs(&ds, "y"), vec!["a", "b"]);
}

#[test]
fn variables_project_and_a_missing_one_is_named() {
    let dir = tempfile::tempdir().unwrap();
    let p = write(dir.path(), "t.csv", TABLE);
    let ds = read(&CsvReader::new(), &p, &["lat"]).unwrap();
    assert_eq!(ds.variables.keys().collect::<Vec<_>>(), vec!["lat"]);
    let e = read(&CsvReader::new(), &p, &["lat", "nope"])
        .unwrap_err()
        .to_string();
    assert!(e.contains("nope") && e.contains("present columns"), "{e}");
}

#[test]
fn an_unknown_option_is_an_error() {
    let reg = FormatRegistry::with_builtins();
    let base = reg.get("csv").unwrap();
    let opts = json!({"numeric_colums": ["a"]});
    let e = base
        .configured(opts.as_object().unwrap())
        .err()
        .expect("unknown option refused")
        .to_string();
    assert!(e.contains("numeric_colums"), "{e}");
}

#[test]
fn an_empty_file_is_an_empty_dataset() {
    let dir = tempfile::tempdir().unwrap();
    let p = write(dir.path(), "t.csv", "\n\n");
    let ds = read(&CsvReader::new(), &p, &[]).unwrap();
    assert!(ds.variables.is_empty() && ds.coords.is_empty());
}

#[test]
fn a_provider_reads_the_declared_options() {
    let dir = tempfile::tempdir().unwrap();
    let p = write(dir.path(), "t.csv", TABLE);
    let mut options = serde_json::Map::new();
    options.insert("numeric_columns".into(), json!(["lat"]));
    let loader = DataSource::new("pts", "csv", format!("file://{}", p.display()))
        .variables(["id", "lat"])
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
    assert_eq!(fields["id"].dtype, DType::Str);
    assert_eq!(fields["lat"].data, ArrayData::F64(vec![39.5, 40.25, 41.0]));
}

#[test]
fn errors_name_the_file_line() {
    let dir = tempfile::tempdir().unwrap();
    // The bad cell is on file line 4: a blank line does not shift the count.
    let p = write(dir.path(), "t.csv", "a,b\n1,2\n\n3,x\n");
    let r = configured(json!({"numeric_columns": ["a", "b"]}));
    let e = read(r.as_ref(), &p, &[]).unwrap_err().to_string();
    assert!(e.contains("line 4"), "{e}");
    // A short row names its line and the 1-based field it lacks.
    let p = write(dir.path(), "s.csv", "a,b\n\n1\n");
    let e = read(&CsvReader::new(), &p, &[]).unwrap_err().to_string();
    assert!(e.contains("line 3") && e.contains("field 2"), "{e}");
}

#[test]
fn a_repeated_column_name_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let p = write(dir.path(), "t.csv", "id,value,value\n1,2,3\n");
    let e = read(&CsvReader::new(), &p, &[]).unwrap_err().to_string();
    assert!(e.contains("value") && e.contains("repeats"), "{e}");
}

#[test]
fn a_numeric_column_absent_from_the_header_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let p = write(dir.path(), "t.csv", TABLE);
    let r = configured(json!({"numeric_columns": ["lat", "longtitude"]}));
    let e = read(r.as_ref(), &p, &[]).unwrap_err().to_string();
    assert!(
        e.contains("longtitude") && e.contains("numeric_columns"),
        "{e}"
    );
}

#[test]
fn a_null_option_is_unset() {
    let dir = tempfile::tempdir().unwrap();
    let p = write(dir.path(), "t.csv", TABLE);
    let r = configured(json!({"numeric_columns": null, "delimiter": null, "header_row": null}));
    let ds = read(r.as_ref(), &p, &[]).unwrap();
    assert_eq!(f64s(&ds, "lat"), vec![39.5, 40.25, 41.0]);
    // An unknown key is refused even when null.
    let reg = FormatRegistry::with_builtins();
    let opts = json!({"numeric_colums": null});
    assert!(reg
        .get("csv")
        .unwrap()
        .configured(opts.as_object().unwrap())
        .is_err());
}

#[test]
fn a_byte_order_mark_is_dropped() {
    let dir = tempfile::tempdir().unwrap();
    let p = write(dir.path(), "t.csv", "\u{feff}id,v\n1,2\n");
    let ds = read(&CsvReader::new(), &p, &[]).unwrap();
    assert_eq!(f64s(&ds, "id"), vec![1.0]);
}
