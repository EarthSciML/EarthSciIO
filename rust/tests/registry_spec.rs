//! Per-track option parity against `spec/registries.json`.
//!
//! That file is the MACHINE-READABLE contract an out-of-process caller reads,
//! and it says which decode options and metadata queries each TRACK provides.
//! This asserts the Rust track really provides the ones it is listed under.
//!
//! The gap it closes: a decode option advertised there but implemented in one
//! binding only is a hard error in the others, and **nothing in the conformance
//! corpus can catch it** — a corpus case pins DECODED ARRAYS, and an option that
//! does not exist produces no array to compare. The peers are
//! `tests/test_registry_dispatch.py` and `julia/test/test_registries.jl`; all
//! three read this same file, which is what makes the three tracks' claims about
//! each other checkable rather than prose.
//!
//! Rust has no introspectable keyword set — a trait method's parameters are
//! positional and a reader declares its capabilities as trait methods — so the
//! check maps each declared option name onto the capability that would make it
//! real here. An option name this mapping does not know is a FAILURE, not a
//! skip: a new option added to the spec must be taught to this test, or it goes
//! unchecked in exactly the track that could not honour it.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use earthsciio::{FormatRegistry, Reader, Selection};
use serde_json::Value;

fn spec() -> Value {
    let path: PathBuf = Path::new(env!("CARGO_MANIFEST_DIR")).join("../spec/registries.json");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("reading {}: {e}", path.display()));
    serde_json::from_str(&text).expect("spec/registries.json is valid JSON")
}

fn format_entries(spec: &Value) -> &Vec<Value> {
    spec["registries"]["format"]["entries"]
        .as_array()
        .expect("format registry entries")
}

fn tracks(opt: &Value) -> Vec<&str> {
    opt["tracks"]
        .as_array()
        .expect("an option declares its tracks")
        .iter()
        .map(|t| t.as_str().expect("a track name is a string"))
        .collect()
}

/// Does the Rust reader for this format really offer the option `name`?
fn rust_offers(reader: &Arc<dyn Reader>, name: &str) -> bool {
    match name {
        // Every `Reader::read_native` takes the projection and the window; the
        // window is only honoured when the reader says so.
        "variables" => true,
        "select" => reader.supports_selection(),
        "records" => reader.supports_records(),
        other => panic!(
            "spec/registries.json declares reader option '{other}' for the rust \
             track, but this test does not know what capability would make it \
             real here. Teach it, or the option goes unchecked in the track that \
             could not honour it."
        ),
    }
}

#[test]
fn declared_reader_options_are_the_rust_readers_own() {
    let spec = spec();
    let registry = FormatRegistry::with_builtins();
    let mut checked: Vec<String> = Vec::new();
    for entry in format_entries(&spec) {
        let Some(options) = entry.get("reader_options").and_then(Value::as_array) else {
            continue;
        };
        let name = entry["name"].as_str().expect("a format name");
        checked.push(name.to_string());
        let reader = registry
            .get(name)
            .unwrap_or_else(|| panic!("{name}: declared in the spec, absent from the registry"));
        for opt in options {
            let opt_name = opt["name"].as_str().expect("an option name");
            for track in tracks(opt) {
                assert!(
                    matches!(track, "python" | "julia" | "rust"),
                    "{name}.{opt_name}: unknown track '{track}'"
                );
            }
            let declared = tracks(opt).contains(&"rust");
            assert_eq!(
                declared,
                rust_offers(&reader, opt_name),
                "{name}: registries.json says the rust track {} '{opt_name}', the \
                 reader says otherwise",
                if declared { "has" } else { "lacks" }
            );
        }
    }
    // ...and the file really does carry a block for the reader this parity check
    // exists for, so silently dropping it cannot pass.
    assert!(checked.iter().any(|n| n == "netcdf"), "checked {checked:?}");
}

#[test]
fn declared_metadata_queries_are_answerable_in_rust() {
    // `dim_length` is what lets an out-of-process caller compute a `records`
    // pushdown's indices without decoding an array. Answered against a real blob
    // rather than by reflection: the point is that it returns a LENGTH, not that
    // a method exists.
    let spec = spec();
    let registry = FormatRegistry::with_builtins();
    let corpus = Path::new(env!("CARGO_MANIFEST_DIR")).join("../conformance/corpus");
    let case: Value = serde_json::from_str(
        &std::fs::read_to_string(corpus.join("cases/era5-grid-sub-tile.json")).unwrap(),
    )
    .unwrap();
    let blob = corpus.join(case["blob_path"].as_str().unwrap());

    for entry in format_entries(&spec) {
        for q in entry
            .get("metadata_queries")
            .and_then(Value::as_array)
            .unwrap_or(&Vec::new())
        {
            if !tracks(q).contains(&"rust") {
                continue;
            }
            let name = entry["name"].as_str().unwrap();
            assert_eq!(q["name"], "dim_length", "{name}: unknown metadata query");
            let reader = registry.get(name).expect("a registered reader");
            assert_eq!(
                reader.dim_length(&blob, "time").expect("a header read"),
                Some(2),
                "{name}: dim_length must answer from the header"
            );
            assert_eq!(reader.dim_length(&blob, "no-such-dim").unwrap(), None);
        }
    }
}

#[test]
fn a_reader_that_does_not_declare_records_refuses_one() {
    // The other direction of the same contract: `used_by_provider` and `tracks`
    // are only meaningful if a reader listed as LACKING the option errors rather
    // than quietly reading the whole record axis.
    let registry = FormatRegistry::with_builtins();
    let corpus = Path::new(env!("CARGO_MANIFEST_DIR")).join("../conformance/corpus");
    let case: Value = serde_json::from_str(
        &std::fs::read_to_string(corpus.join("cases/era5-grid-sub-tile.json")).unwrap(),
    )
    .unwrap();
    let blob = corpus.join(case["blob_path"].as_str().unwrap());
    let csvish = registry.get("ff10").expect("the ff10 reader");
    assert!(!csvish.supports_records());
    let err = csvish
        .read_native_records(
            &blob,
            &[],
            &Selection::All,
            Some(&earthsciio::Records::new("time", vec![0])),
        )
        .expect_err("a pushdown at a reader that cannot honour it");
    assert!(
        format!("{err}").contains("does not honour a `records` pushdown"),
        "{err}"
    );
}

#[test]
fn the_records_pushdown_is_declared_for_every_track() {
    // The specific claim PR #4 could not make honestly: `records` is real in all
    // three tracks. Which Providers USE it is recorded separately, because that
    // is a per-track performance decision — the Rust Provider keeps a 2-entry LRU
    // of decoded files and amortises one whole-file decode over every tick in the
    // file, which a per-tick narrowed decode would throw away.
    let spec = spec();
    let netcdf = format_entries(&spec)
        .iter()
        .find(|e| e["name"] == "netcdf")
        .expect("the netcdf entry");
    let records = netcdf["reader_options"]
        .as_array()
        .unwrap()
        .iter()
        .find(|o| o["name"] == "records")
        .expect("the records option");
    let mut t = tracks(records);
    t.sort_unstable();
    assert_eq!(t, vec!["julia", "python", "rust"]);
    assert_eq!(records["used_by_provider"]["rust"], Value::Bool(false));
    assert_eq!(records["used_by_provider"]["julia"], Value::Bool(true));
}

/// `selection.forms` (`spec/registries.md` §2.2, "Two spellings, one
/// vocabulary"): each selection-capable format says which tracks take the
/// positional `axes` form and which the named `dims` form. Checked by DOING it
/// against the corpus — a form is real in this track only if a read through it
/// returns the window — and a reader with no `selection` block must not claim
/// `supports_selection` either, so the block cannot silently go missing.
#[test]
fn declared_selection_forms_are_the_rust_readers_own() {
    let spec = spec();
    let registry = FormatRegistry::with_builtins();
    let corpus = Path::new(env!("CARGO_MANIFEST_DIR")).join("../conformance/corpus");
    let cache = Arc::new(
        earthsciio::Cache::builder()
            .data_dir(corpus.join("cache"))
            .offline(true)
            .verify_on_read(true)
            .build()
            .unwrap(),
    );
    let case = |id: &str| -> Value {
        serde_json::from_str(&std::fs::read_to_string(corpus.join(format!("cases/{id}.json"))).unwrap())
            .unwrap()
    };
    // One corpus case per (format, form) proves the form end to end.
    let proof = |format: &str, form: &str| -> &'static str {
        match (format, form) {
            ("netcdf", "axes") => "era5-window-slice",
            ("netcdf", "dims") => "named-window-mixed-rank",
            ("zarr", "axes") => "isrm-zarr-tile",
            ("zarr", "dims") => "named-window-zarr",
            other => panic!(
                "spec/registries.json declares selection form {other:?} for the rust \
                 track, and this test knows no corpus case proving it. Teach it."
            ),
        }
    };

    let mut checked = Vec::new();
    for entry in format_entries(&spec) {
        let name = entry["name"].as_str().unwrap();
        let Some(reader) = registry.get(name) else {
            continue; // a format with no Rust reader claims nothing here
        };
        let Some(selection) = entry.get("selection") else {
            assert!(
                !reader.supports_selection(),
                "{name}: the reader honours a select but registries.json has no selection block"
            );
            continue;
        };
        assert_eq!(
            selection["supports_selection"].as_bool(),
            Some(reader.supports_selection()),
            "{name}: supports_selection"
        );
        assert_eq!(selection["store_backed"].as_bool(), Some(reader.store_backed()), "{name}: store_backed");
        for form in selection["forms"].as_array().expect("selection.forms") {
            let form_name = form["form"].as_str().unwrap();
            if !tracks(form).contains(&"rust") {
                continue;
            }
            let c = case(proof(name, form_name));
            assert_eq!(c["format"], name);
            let select = Selection::from_json(&c["select"]).unwrap();
            assert!(
                matches!(
                    (form_name, &select),
                    ("axes", Selection::Orthogonal(_)) | ("dims", Selection::Named(_))
                ),
                "{name}/{form_name}: the proving case spells its select that way"
            );
            let ds = if reader.store_backed() {
                let vars: Vec<String> = c["variables"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|v| v.as_str().unwrap().to_string())
                    .collect();
                reader.read_store(cache.clone(), c["resolved_url"].as_str().unwrap(), &vars, &select)
            } else {
                reader.read_native(&corpus.join(c["blob_path"].as_str().unwrap()), &[], &select)
            }
            .unwrap_or_else(|e| panic!("{name}/{form_name}: registries.json says rust takes it: {e}"));
            for (var, exp) in c["expected"]["variables"].as_object().unwrap() {
                let shape: Vec<usize> =
                    exp["shape"].as_array().unwrap().iter().map(|v| v.as_u64().unwrap() as usize).collect();
                assert_eq!(ds.variables[var].shape, shape, "{name}/{form_name}/{var}: the window was applied");
            }
            checked.push(format!("{name}/{form_name}"));
        }
    }
    checked.sort();
    assert_eq!(checked, ["netcdf/axes", "netcdf/dims", "zarr/axes", "zarr/dims"]);
}
