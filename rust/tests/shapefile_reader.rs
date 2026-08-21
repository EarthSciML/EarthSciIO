//! The active `shapefile` reader (Rust track) — an ESRI shapefile as a feature
//! table. Sibling of `tests/test_shapefile_reader.py` and
//! `julia/test/test_shapefile_reader.jl`.
//!
//! The fixture is the COMMITTED conformance blob `shapefile-polygon-zip` — the
//! same bytes the Python and Julia tracks read — so this file checks the decode
//! contract (one row per part, the esm-spec §8.6.1 padding, the `*`-only
//! deletion rule, the dtype rules, the stored bbox, the `meta` fields) and the
//! Rust-side seams the corpus case cannot reach with a single blob: member
//! selection, a bare `.shp`, variable filtering, the reserved-name collision and
//! the `reader_options` screen.

// Native-only: readers open a cached blob, which does not exist on wasm32.
#![cfg(not(target_arch = "wasm32"))]

use std::fs;
use std::io::{Cursor, Read, Write};
use std::path::{Path, PathBuf};

use earthsciio::{ArrayData, DType, NativeDataset, Reader, Selection, ShapefileReader};
use serde_json::{json, Map, Value};

fn corpus_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../conformance/corpus")
}

fn case() -> Value {
    serde_json::from_slice(
        &fs::read(corpus_dir().join("cases/shapefile-polygon-zip.json")).unwrap(),
    )
    .unwrap()
}

fn blob_path() -> PathBuf {
    corpus_dir().join(case()["blob_path"].as_str().unwrap())
}

/// The corpus blob's zip members, so a test can rezip a variant of them.
fn members() -> Vec<(String, Vec<u8>)> {
    let mut archive = zip::ZipArchive::new(Cursor::new(fs::read(blob_path()).unwrap())).unwrap();
    let names: Vec<String> = (0..archive.len())
        .map(|i| archive.by_index(i).unwrap().name().to_string())
        .collect();
    names
        .into_iter()
        .map(|n| {
            let mut buf = Vec::new();
            archive.by_name(&n).unwrap().read_to_end(&mut buf).unwrap();
            (n, buf)
        })
        .collect()
}

fn rezip(path: &Path, members: &[(String, Vec<u8>)]) {
    let mut w = zip::ZipWriter::new(fs::File::create(path).unwrap());
    let opts: zip::write::FileOptions<()> =
        zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (name, bytes) in members {
        w.start_file(name.as_str(), opts).unwrap();
        w.write_all(bytes).unwrap();
    }
    w.finish().unwrap();
}

fn configured(options: Value) -> std::sync::Arc<dyn Reader> {
    let base = ShapefileReader::new();
    let map: Map<String, Value> = options.as_object().unwrap().clone();
    base.configured(&map).unwrap().expect("a configured reader")
}

fn read(path: &Path, options: Value) -> NativeDataset {
    configured(options)
        .read_native(path, &[], &Selection::All)
        .expect("decode")
}

fn f64s(ds: &NativeDataset, name: &str) -> Vec<f64> {
    match &ds.variables[name].data {
        ArrayData::F64(v) => v.clone(),
        other => panic!("{name} is not float64: {other:?}"),
    }
}

fn i64s(ds: &NativeDataset, name: &str) -> Vec<i64> {
    match &ds.variables[name].data {
        ArrayData::I64(v) => v.clone(),
        other => panic!("{name} is not int64: {other:?}"),
    }
}

fn strs(ds: &NativeDataset, name: &str) -> Vec<String> {
    match &ds.variables[name].data {
        ArrayData::Str(v) => v.clone(),
        other => panic!("{name} is not string: {other:?}"),
    }
}

fn corpus_options() -> Value {
    let c = case();
    json!({
        "member": c["decode"]["member"].clone(),
        "numeric_columns": c["decode"]["numeric_columns"].clone(),
    })
}

#[test]
fn explodes_records_to_parts_and_replicates_their_attributes() {
    let ds = read(&blob_path(), corpus_options());
    // 5 records, one `*`-deleted; record 1 has a mainland + an island.
    assert_eq!(i64s(&ds, "shape_index"), vec![0, 1, 1, 2, 4]);
    assert_eq!(i64s(&ds, "part_index"), vec![0, 0, 1, 0, 0]);
    assert_eq!(i64s(&ds, "n_parts"), vec![1, 2, 2, 1, 1]);
    assert_eq!(
        strs(&ds, "NAME"),
        ["Alpha", "Bravo", "Bravo", "Charlie", "Echo"]
    );
    // The `*` row is gone; the NUL-flagged one ("Echo") is NOT a deletion.
    assert!(!strs(&ds, "NAME").iter().any(|n| n == "Deleted"));
}

#[test]
fn pads_a_short_ring_by_repeating_its_final_vertex() {
    let ds = read(&blob_path(), corpus_options());
    let geom = &ds.variables["geometry"];
    assert_eq!(geom.dims, ["index", "vertex", "xy"]);
    assert_eq!(geom.shape, vec![5, 5, 2]);
    assert_eq!(i64s(&ds, "n_vertices"), vec![5, 5, 4, 4, 5]);
    let ArrayData::F64(v) = &geom.data else {
        panic!("geometry is not float64")
    };
    // Row 3 ("Charlie") is a 4-vertex ring in a 5-vertex stack: the last slot
    // repeats the final vertex (esm-spec §8.6.1), and no slot is NaN.
    let row = 3 * 5 * 2;
    assert_eq!(v[row + 4 * 2], v[row + 3 * 2]);
    assert_eq!(v[row + 4 * 2 + 1], v[row + 3 * 2 + 1]);
    assert!(!v.iter().any(|x| x.is_nan()));
}

#[test]
fn nvert_max_lets_the_document_declare_the_vertex_axis() {
    let c = case();
    let opts = json!({"member": c["decode"]["member"].clone(), "nvert_max": 8});
    let ds = read(&blob_path(), opts);
    assert_eq!(ds.variables["geometry"].shape, vec![5, 8, 2]);
    // The extra slots still repeat the final vertex, so the ring is unchanged.
    let ArrayData::F64(v) = &ds.variables["geometry"].data else {
        panic!("not f64")
    };
    let row = 2 * 8 * 2; // the island: 4 real vertices in 8 slots
    for slot in 4..8 {
        assert_eq!(v[row + slot * 2], v[row + 3 * 2]);
        assert_eq!(v[row + slot * 2 + 1], v[row + 3 * 2 + 1]);
    }
    let err = configured(json!({"member": c["decode"]["member"].clone(), "nvert_max": 4}))
        .read_native(&blob_path(), &[], &Selection::All)
        .unwrap_err();
    assert!(format!("{err}").contains("declared nvert_max=4"), "{err}");
}

#[test]
fn replicates_the_records_stored_bbox_to_its_parts() {
    let ds = read(&blob_path(), corpus_options());
    assert_eq!(f64s(&ds, "xmin"), vec![0.0, 4.0, 4.0, 0.0, 0.0]);
    assert_eq!(f64s(&ds, "xmax"), vec![2.0, 8.0, 8.0, 2.0, 1.0]);
    assert_eq!(f64s(&ds, "ymax"), vec![2.0, 2.0, 2.0, 6.0, 9.0]);
}

#[test]
fn maps_dbf_types_and_honours_numeric_columns() {
    let ds = read(&blob_path(), corpus_options());
    assert_eq!(ds.variables["NAME"].dtype, DType::Str);
    assert_eq!(ds.variables["FLAG"].dtype, DType::Bool); // `L`, not float64
    assert!(matches!(&ds.variables["FLAG"].data,
                     ArrayData::Bool(v) if *v == vec![true, false, false, true, true]));
    assert!(f64s(&ds, "EMIS")[3].is_nan()); // a blank `N` cell
    assert_eq!(
        f64s(&ds, "CODE"),
        vec![1001.0, 17031.0, 17031.0, 6037.0, 36061.0]
    );

    let c = case();
    let plain = read(
        &blob_path(),
        json!({ "member": c["decode"]["member"].clone() }),
    );
    assert_eq!(plain.variables["CODE"].dtype, DType::Str); // a `C` column by default

    let bad = configured(json!({
        "member": c["decode"]["member"].clone(), "numeric_columns": ["NOPE"],
    }))
    .read_native(&blob_path(), &[], &Selection::All);
    assert!(format!("{}", bad.unwrap_err()).contains("no such .dbf column"));
}

#[test]
fn carries_the_shape_type_and_prj_as_meta_fields() {
    let ds = read(&blob_path(), corpus_options());
    assert_eq!(ds.variables["shape_type"].dims, ["meta"]);
    assert_eq!(strs(&ds, "shape_type"), ["Polygon"]);
    assert!(strs(&ds, "crs_wkt")[0].starts_with("GEOGCS["));
}

#[test]
fn selects_the_shp_member_and_names_the_ambiguous_cases() {
    let dir = tempdir("members");
    let base = members();

    // A single `.shp` member needs no `member` option.
    let one = dir.join("one.zip");
    rezip(&one, &base);
    let ds = ShapefileReader::new()
        .read_native(&one, &[], &Selection::All)
        .expect("single-member zip");
    assert_eq!(i64s(&ds, "n_parts").len(), 5);

    // Two layers in one archive: ambiguous without `member`.
    let mut both = base.clone();
    both.extend(
        base.iter()
            .map(|(n, b)| (n.replace("layer/", "other/"), b.clone())),
    );
    let two = dir.join("two.zip");
    rezip(&two, &both);
    let err = ShapefileReader::new()
        .read_native(&two, &[], &Selection::All)
        .unwrap_err();
    assert!(format!("{err}").contains("2 .shp members"), "{err}");
    let ds = read(&two, json!({"member": "other/emis_polygons.shp"}));
    assert_eq!(strs(&ds, "NAME").len(), 5);
    let err = configured(json!({"member": "nope.shp"}))
        .read_native(&two, &[], &Selection::All)
        .unwrap_err();
    assert!(format!("{err}").contains("not in the archive"), "{err}");
}

#[test]
fn decodes_a_bare_shp_blob_without_attributes() {
    let dir = tempdir("bare");
    let bare = dir.join("blob");
    let shp = members()
        .into_iter()
        .find(|(n, _)| n.ends_with(".shp"))
        .unwrap()
        .1;
    fs::write(&bare, shp).unwrap();
    let ds = ShapefileReader::new()
        .read_native(&bare, &[], &Selection::All)
        .expect("bare .shp");
    // No `.dbf` => nothing is deleted, so the `*` record's shape is present.
    assert_eq!(ds.variables["geometry"].shape[0], 6);
    assert!(!ds.variables.contains_key("NAME"));
    assert!(!ds.variables.contains_key("crs_wkt"));
}

#[test]
fn filters_requested_variables_and_names_an_unknown_one() {
    let c = case();
    let want = vec!["geometry".to_string(), "EMIS".to_string()];
    let ds = configured(json!({"member": c["decode"]["member"].clone()}))
        .read_native(&blob_path(), &want, &Selection::All)
        .expect("filtered decode");
    let mut got: Vec<&str> = ds.variables.keys().map(String::as_str).collect();
    got.sort_unstable();
    assert_eq!(got, ["EMIS", "geometry"]);

    let err = configured(json!({"member": c["decode"]["member"].clone()}))
        .read_native(&blob_path(), &["nope".to_string()], &Selection::All)
        .unwrap_err();
    assert!(format!("{err}").contains("not in the shapefile"), "{err}");
}

#[test]
fn refuses_a_dbf_column_named_like_a_reader_field() {
    let dir = tempdir("clash");
    let patched: Vec<(String, Vec<u8>)> = members()
        .into_iter()
        .map(|(name, mut bytes)| {
            if name.ends_with(".dbf") {
                // Rename the first field descriptor (11 NUL-padded bytes at 32).
                bytes[32..43].copy_from_slice(b"xmin\0\0\0\0\0\0\0");
            }
            (name, bytes)
        })
        .collect();
    let clash = dir.join("clash.zip");
    rezip(&clash, &patched);
    let err = ShapefileReader::new()
        .read_native(&clash, &[], &Selection::All)
        .unwrap_err();
    assert!(
        format!("{err}").contains("collide with the reader's own fields"),
        "{err}"
    );
}

#[test]
fn screens_unknown_reader_options() {
    let mut options = Map::new();
    options.insert("membre".to_string(), json!("a.shp"));
    let Err(err) = ShapefileReader::new().configured(&options) else {
        panic!("a mis-typed reader option must be refused, not ignored");
    };
    assert!(format!("{err}").contains("unknown reader option"), "{err}");
    // An empty option set means "use me as registered".
    assert!(matches!(
        ShapefileReader::new().configured(&Map::new()),
        Ok(None)
    ));
}

/// A fresh directory under the target dir — no `tempfile` in dev-dependencies.
/// Named per test, since cargo runs the tests concurrently in one process.
fn tempdir(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("shapefile_reader")
        .join(name);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}
