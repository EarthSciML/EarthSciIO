//! The named-dimension form of `select` (`spec/prefetch.md` §2.1) in the two
//! readers that honour a `select`, against the committed corpus, offline.
//!
//! `conformance_decode.rs` already pins the three corpus cases' arrays; this
//! file pins what a single decoded dataset cannot: every refusal, the gate
//! (windowed read == full read sliced by name) beyond the corpus windows, and
//! that the zarr reader fetches ONLY the chunk objects a window intersects —
//! proved by the poisoned store, which fails any reader that fetches one more.

#![cfg(not(target_arch = "wasm32"))]

mod common;

use std::sync::Arc;

use common::{assert_ds_bitwise, corpus_cache, corpus_dir, read_json, slice_by_name};
use earthsciio::{
    AxisSelect, DataSource, DimWindow, Error, FormatRegistry, NativeDataset, Provider, Reader,
    Selection,
};
use serde_json::json;

const MIXED: &str =
    "cache/v1/blobs/15/15b37e1c2dae8dea65eedaab68c5df8f15f007a396dd7b07992ec989588c8aef.nc";

fn netcdf() -> Arc<dyn Reader> {
    FormatRegistry::with_builtins().get("netcdf").unwrap()
}

fn read_mixed(vars: &[&str], sel: &Selection) -> earthsciio::Result<NativeDataset> {
    let vars: Vec<String> = vars.iter().map(|v| v.to_string()).collect();
    netcdf().read_native(&corpus_dir().join(MIXED), &vars, sel)
}

fn sel(v: serde_json::Value) -> Selection {
    Selection::from_json(&v).expect("a well-formed select")
}

fn format_detail(e: &Error) -> String {
    match e {
        Error::Format { detail, .. } => detail.clone(),
        other => panic!("expected a format error, got {other:?}"),
    }
}

/// One window, every rank, a staggered dimension beside its centre: equal to the
/// full read sliced by name, coordinates included — for the corpus windows and
/// for repeats, permutations and steps the corpus does not pin.
#[test]
fn a_named_window_is_the_full_read_sliced_by_name() {
    let full = read_mixed(&[], &Selection::All).unwrap();
    let windows = [
        json!({"dims": {"lat": {"slice": [1, 3]}, "lon": {"indices": [4, 0, 2]},
                        "lev": {"slice": [0, 3, 2]}, "lon_edge": {"slice": [2, 5]}}}),
        json!({"dims": {"lon": {"indices": [3, 4, 0, 1]}}}),
        json!({"dims": {"lat": {"indices": [2, 2, 0]}, "lon_edge": {"indices": [5, 0]}}}),
        json!({"dims": {"lev": {"indices": [1]}, "time": "all"}}),
        json!({"dims": {"lat": "all", "lon": {"slice": [0, 5, 4]}}}),
    ];
    for w in windows {
        let s = sel(w.clone());
        let got = read_mixed(&[], &s).unwrap();
        assert_ds_bitwise(&format!("{w}"), &got, &slice_by_name(&full, &s));
    }
    // `{"dims": {}}` selects nothing away.
    assert_ds_bitwise(
        "dims {}",
        &read_mixed(&[], &sel(json!({"dims": {}}))).unwrap(),
        &full,
    );
}

#[test]
fn a_dimension_no_field_carries_is_an_error_naming_those_present() {
    let err = read_mixed(
        &[],
        &sel(json!({"dims": {"lat": "all", "level": {"indices": [0]}}})),
    )
    .unwrap_err();
    let msg = format_detail(&err);
    assert!(msg.contains("level"), "{msg}");
    for present in ["lat", "lev", "lon", "lon_edge", "time"] {
        assert!(msg.contains(present), "must list {present}: {msg}");
    }
    // Even an `all` must mean something: a window never silently means nothing.
    assert!(read_mixed(&[], &sel(json!({"dims": {"bogus": "all"}}))).is_err());
}

#[test]
fn a_time_subset_is_refused_and_time_all_is_not() {
    let msg = format_detail(
        &read_mixed(&[], &sel(json!({"dims": {"time": {"indices": [0]}}}))).unwrap_err(),
    );
    assert!(msg.contains("time"), "{msg}");
    let whole = read_mixed(&[], &sel(json!({"dims": {"time": "all"}}))).unwrap();
    assert_eq!(whole.variables["T"].shape, vec![2, 3, 4, 5]);
}

/// Every index is bounds-checked against the FILE dimension length; out of range
/// is an error, never a clamp or a wrap.
#[test]
fn bounds_are_checked_against_the_file_length() {
    for w in [
        json!({"dims": {"lat": {"indices": [0, 4]}}}),
        json!({"dims": {"lon_edge": {"slice": [2, 7]}}}),
    ] {
        let msg = format_detail(&read_mixed(&[], &sel(w.clone())).unwrap_err());
        assert!(msg.contains("out of range"), "{w}: {msg}");
    }
    // lon_edge has 6 entries where lon has 5: each dimension's own length.
    assert!(read_mixed(&[], &sel(json!({"dims": {"lon_edge": {"indices": [5]}}}))).is_ok());
    assert!(read_mixed(&[], &sel(json!({"dims": {"lon": {"indices": [5]}}}))).is_err());
}

#[test]
fn an_empty_selection_is_a_zero_length_axis_kept_in_dims() {
    let got = read_mixed(&[], &sel(json!({"dims": {"lat": {"indices": []}}}))).unwrap();
    assert_eq!(got.variables["T"].dims, ["time", "lev", "lat", "lon"]);
    assert_eq!(got.variables["T"].shape, vec![2, 3, 0, 5]);
    assert!(got.variables["T"].data.is_empty());
    assert_eq!(got.variables["AREA_LAT"].shape, vec![0]);
    assert_eq!(got.coords["lat"].field.shape, vec![0]);
    assert_eq!(got.coords["lon"].field.shape, vec![5]);
}

/// "The fields of the read" are the projected data variables plus the
/// coordinates returned with them; and a consumed string length is not a
/// dimension of any of them, so naming it is the same unknown-dimension error.
#[test]
fn the_read_s_fields_decide_what_a_name_may_mean() {
    let case = read_json("cases/station-labels-text.json");
    let blob = corpus_dir().join(case["blob_path"].as_str().unwrap());
    let r = netcdf();
    let read = |vars: &[&str], w: serde_json::Value| {
        let vars: Vec<String> = vars.iter().map(|v| v.to_string()).collect();
        r.read_native(&blob, &vars, &sel(w))
    };

    // `site_id(site, strlen)` consumes `strlen`; selecting along it would
    // truncate every string.
    let msg =
        format_detail(&read(&[], json!({"dims": {"strlen": {"slice": [0, 2]}}})).unwrap_err());
    assert!(
        msg.contains("strlen") && msg.contains("string LENGTH"),
        "{msg}"
    );
    assert!(
        msg.contains("site"),
        "must list the dimensions present: {msg}"
    );

    // Along `site` a named window takes strings, in order, repeats kept — text
    // and numbers alike.
    let got = read(&[], json!({"dims": {"site": {"indices": [2, 0, 2]}}})).unwrap();
    let full = read(&[], json!({"dims": {}})).unwrap();
    let w = sel(json!({"dims": {"site": {"indices": [2, 0, 2]}}}));
    assert_ds_bitwise("station site window", &got, &slice_by_name(&full, &w));
    assert_eq!(
        got.variables["title"].shape,
        Vec::<usize>::new(),
        "the scalar is untouched"
    );

    // Project to the scalar `title` alone: no field of THAT read carries `site`.
    let msg = format_detail(&read(&["title"], json!({"dims": {"site": "all"}})).unwrap_err());
    assert!(msg.contains("site"), "{msg}");
}

/// The store-backed reader fetches only the chunk objects a named window
/// intersects. Every other chunk object of `named-window-zarr` is poisoned, so
/// the windowed read succeeding IS the proof, and the whole read failing proves
/// the poison is live.
#[test]
fn a_named_zarr_window_fetches_only_the_chunks_it_intersects() {
    let case = read_json("cases/named-window-zarr.json");
    let url = case["resolved_url"].as_str().unwrap();
    let vars: Vec<String> = case["variables"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect();
    let zarr = FormatRegistry::with_builtins().get("zarr").unwrap();
    let window = Selection::from_json(&case["select"]).unwrap();

    let got = zarr
        .read_store(corpus_cache(), url, &vars, &window)
        .expect("only intersected chunks");
    assert_eq!(got.variables["T3"].shape, vec![3, 2, 2]);
    assert_eq!(got.variables["PS2"].shape, vec![2, 2]);

    assert!(
        zarr.read_store(corpus_cache(), url, &vars, &Selection::All)
            .is_err(),
        "an over-fetching read must meet a poisoned chunk"
    );
    // The same window as a DimWindow, through a Provider's baked select.
    let w = DimWindow::new()
        .all("lev")
        .range("lat", 2, 4)
        .indices("lon", vec![5, 3]);
    assert_eq!(Selection::from(w.clone()), window);
    let loader = DataSource::new("geosfp", "zarr", url)
        .variables(vars.clone())
        .select(w.into());
    let mut p = Provider::new(loader, corpus_cache(), None).unwrap();
    let fields = p.materialize().unwrap();
    assert_eq!(fields["PS2"].data, got.variables["PS2"].data);

    // Refusals happen on metadata, before a chunk object is fetched — a poisoned
    // chunk would otherwise be what failed.
    for (w, needle) in [
        (json!({"dims": {"bogus": "all"}}), "bogus"),
        (json!({"dims": {"lat": {"indices": [9]}}}), "out of range"),
    ] {
        let msg = format_detail(
            &zarr
                .read_store(corpus_cache(), url, &vars, &sel(w))
                .unwrap_err(),
        );
        assert!(msg.contains(needle), "{msg}");
        assert!(
            !msg.contains("decode chunk"),
            "refused before any chunk: {msg}"
        );
    }
}

#[test]
fn the_wire_form_takes_exactly_one_spelling() {
    let both = json!({"axes": ["all"], "dims": {"lat": "all"}});
    let msg = format_detail(&Selection::from_json(&both).unwrap_err());
    assert!(msg.contains("both"), "{msg}");

    for bad in [
        json!({"dims": {"lat": {"indices": [-1]}}}),
        json!({"dims": {"lat": {"indices": [1.5]}}}),
        json!({"dims": {"lat": {"slice": [0]}}}),
        json!({"dims": {"lat": "some"}}),
        json!({"dims": ["lat"]}),
        json!({"axis": ["all"]}),
    ] {
        assert!(Selection::from_json(&bad).is_err(), "{bad} must be refused");
    }
    for everything in [
        json!({}),
        json!({"all_records": true}),
        json!({"all_rows": true}),
    ] {
        assert_eq!(Selection::from_json(&everything).unwrap(), Selection::All);
    }
    assert_eq!(
        sel(json!({"axes": ["all", {"slice": [1, 3]}, {"indices": [0, 2]}]})),
        Selection::Orthogonal(vec![
            AxisSelect::All,
            AxisSelect::Range {
                start: 1,
                stop: 3,
                step: 1
            },
            AxisSelect::Indices(vec![0, 2]),
        ])
    );
    assert_eq!(
        sel(
            json!({"dims": {"lat": {"slice": [1, 3]}, "lon": {"indices": [4, 0, 2]},
                            "lev": {"slice": [0, 3, 2]}, "time": "all"}})
        ),
        DimWindow::new()
            .range("lat", 1, 3)
            .indices("lon", vec![4, 0, 2])
            .step_range("lev", 0, 3, 2)
            .all("time")
            .into()
    );
}
