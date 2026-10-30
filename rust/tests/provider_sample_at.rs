//! `Provider::sample_at`, `with_window` and `sample_windows_at`
//! (`spec/prefetch.md` §1, §2.3, §2.4) over the committed corpus, offline —
//! including the `multi-window` scenario, run from its JSON.
//!
//! The contract under test is an equality with what already exists:
//! `sample_at(t)` IS `refresh(t)`'s variables plus the `coords()` that refresh
//! leaves, and window `i` of `sample_windows_at` IS
//! `with_window(w_i).sample_at(t)` — bit for bit, coordinates included.

#![cfg(not(target_arch = "wasm32"))]

mod common;

use std::sync::Arc;

use common::{
    assert_ds_bitwise, corpus_cache, fresh_refresh, read_json, scenario_provider, selections,
    slice_by_name, ts,
};
use earthsciio::{
    Cache, DataSource, DimWindow, Error, NativeDataset, Provider, Selection, SourceTemporal,
};
use serde_json::json;
use time::macros::datetime;
use time::{Duration, OffsetDateTime};

fn equivalence() -> serde_json::Value {
    read_json("scenarios/prefetch-equivalence.json")
}

fn cold_cache() -> (tempfile::TempDir, Arc<Cache>) {
    let dir = tempfile::tempdir().unwrap();
    let cache = Cache::builder()
        .data_dir(dir.path())
        .offline(true)
        .build()
        .unwrap();
    (dir, Arc::new(cache))
}

/// At every refresh time — file seams, the end-of-data clamp and an off-anchor
/// instant included — `sample_at(t)` equals a fresh `refresh(t)` plus `coords()`,
/// for both record counts and both cadences of the scenario sources.
#[test]
fn sample_at_is_refresh_and_its_coords() {
    let sc = equivalence();
    for (source, rps) in [
        ("hourly", 1),
        ("hourly", 2),
        ("two_hourly", 1),
        ("two_hourly", 2),
    ] {
        let src = &sc["sources"][source];
        let make = || scenario_provider(src, rps, corpus_cache());
        let p = make();
        for t in p.refresh_times() {
            for dt in [0, 1799] {
                let t = ts(t as i64 + dt);
                let ctx = format!("{source} rps={rps} t={t}");
                assert_ds_bitwise(&ctx, &p.sample_at(t).unwrap(), &fresh_refresh(make, t));
            }
        }
    }
}

/// The SAME provider, stepped with `refresh`: `sample_at` equals each step, and
/// calling it in between leaves the buffer, the anchor and the decoded-file LRU
/// exactly where refresh put them.
#[test]
fn sample_at_touches_no_buffer() {
    let sc = equivalence();
    for rps in [1, 2] {
        let mut p = scenario_provider(&sc["sources"]["hourly"], rps, corpus_cache());
        let times = p.refresh_times();
        for (i, &t) in times.iter().enumerate() {
            let t = ts(t as i64);
            let ahead = p.sample_at(t).unwrap();
            // A sample of a far epoch (another file) between two refreshes.
            let _ = p
                .sample_at(ts(times[(i + 7) % times.len()] as i64))
                .unwrap();
            let vars = p.refresh(t).unwrap().expect("a new anchor refreshes");
            let after = NativeDataset {
                variables: vars,
                coords: p.coords().clone(),
            };
            assert_ds_bitwise(&format!("rps={rps} step {i}"), &ahead, &after);

            let _ = p
                .sample_at(ts(times[(i + 5) % times.len()] as i64))
                .unwrap();
            assert!(p.refresh(t).unwrap().is_none(), "the anchor is still {t}");
            assert_ds_bitwise(
                &format!("rps={rps} step {i} coords"),
                &NativeDataset {
                    variables: Default::default(),
                    coords: p.coords().clone(),
                },
                &NativeDataset {
                    variables: Default::default(),
                    coords: after.coords.clone(),
                },
            );
        }
    }
}

/// On the ERA5 corpus fixture too (a daily file of hourly records), and two
/// instants that snap to one anchor give one sample.
#[test]
fn sample_at_depends_only_on_the_anchor() {
    const TEMPLATE: &str = "https://data.earthsci.dev/era5/[year]/[month]/[year][month][day].nc";
    for rps in [1, 2] {
        let make = || {
            let temporal = SourceTemporal::new(
                datetime!(2018-11-08 00:00:00 UTC),
                Duration::hours(1),
                Duration::days(1),
            )
            .end(datetime!(2018-11-08 02:00:00 UTC))
            .records_per_sample(rps);
            let loader = DataSource::new("era5", "netcdf", TEMPLATE).temporal(temporal);
            Provider::new(loader, corpus_cache(), None).unwrap()
        };
        let p = make();
        for t in [
            datetime!(2018-11-08 00:00:00 UTC),
            datetime!(2018-11-08 01:00:00 UTC),
        ] {
            let at = p.sample_at(t).unwrap();
            assert_ds_bitwise(&format!("era5 rps={rps} {t}"), &at, &fresh_refresh(make, t));
            let later = p.sample_at(t + Duration::minutes(59)).unwrap();
            assert_ds_bitwise(&format!("era5 rps={rps} {t}+59m"), &later, &at);
        }
    }
}

/// `Provider` is `Send + Sync` (asserted at compile time in the crate); here the
/// point is that concurrent samples are the sequential ones.
#[test]
fn sample_at_runs_concurrently() {
    let sc = equivalence();
    let p = Arc::new(scenario_provider(
        &sc["sources"]["hourly"],
        2,
        corpus_cache(),
    ));
    let times = p.refresh_times();
    let handles: Vec<_> = times
        .iter()
        .map(|&t| {
            let p = p.clone();
            std::thread::spawn(move || p.sample_at(ts(t as i64)).unwrap())
        })
        .collect();
    for (h, &t) in handles.into_iter().zip(&times) {
        let got = h.join().unwrap();
        assert_ds_bitwise(
            &format!("thread t={t}"),
            &got,
            &p.sample_at(ts(t as i64)).unwrap(),
        );
    }
}

#[test]
fn a_const_sample_is_its_materialize() {
    let url = "https://data.earthsci.dev/geosfp/mini/GEOSFP.20240101.A3mixed.nc";
    let mut p = Provider::new(
        DataSource::new("geosfp", "netcdf", url),
        corpus_cache(),
        None,
    )
    .unwrap();
    let sample = p.sample_at(OffsetDateTime::UNIX_EPOCH).unwrap();
    let variables = p.materialize().unwrap();
    let materialized = NativeDataset {
        variables,
        coords: p.coords().clone(),
    };
    assert_ds_bitwise("const", &sample, &materialized);
}

/// `with_window` refuses at once — on a COLD cache, so any I/O would have been
/// a CacheMiss instead of the precondition error.
#[test]
fn with_window_refuses_before_any_io() {
    let (_dir, cache) = cold_cache();
    let src = &equivalence()["sources"]["hourly"];
    let template = src["url_template"]["rust"].as_str().unwrap();
    let temporal = SourceTemporal::new(ts(1_704_067_200), Duration::hours(1), Duration::hours(4))
        .end(ts(1_704_110_400));
    let loader = DataSource::new("hourly", "netcdf", template).temporal(temporal.clone());
    let p = Provider::new(loader.clone(), cache.clone(), None).unwrap();
    let w = DimWindow::new().range("lat", 0, 1);

    let windowed = p.with_window(w.clone()).expect("no I/O, so no CacheMiss");
    let detail = |e: Error| match e {
        Error::Format { detail, .. } => detail,
        other => panic!("expected the precondition error, got {other:?}"),
    };
    // Two windows do not compose.
    let msg = detail(windowed.with_window(w.clone()).err().unwrap());
    assert!(msg.contains("already bakes a select"), "{msg}");
    let baked = Provider::new(loader.select(w.clone().into()), cache.clone(), None).unwrap();
    assert!(baked.with_window(DimWindow::new()).is_err());
    // The record axis is the Provider's.
    let msg = detail(
        p.with_window(DimWindow::new().range("time", 0, 1))
            .err()
            .unwrap(),
    );
    assert!(msg.contains("time"), "{msg}");
    assert!(p.with_window(DimWindow::new().all("time")).is_err());
    // A reader that cannot honour a select.
    let ff10 = Provider::new(
        DataSource::new("nei", "ff10", "file:///nowhere.csv"),
        cache.clone(),
        None,
    )
    .unwrap();
    let msg = detail(ff10.with_window(w.clone()).err().unwrap());
    assert!(msg.contains("does not support select"), "{msg}");
    // sample_windows_at shares the preconditions, and an empty list fetches nothing.
    assert!(p
        .sample_windows_at(ts(1_704_067_200), &[])
        .unwrap()
        .is_empty());
    let msg = detail(
        p.sample_windows_at(
            ts(1_704_067_200),
            &[w.into(), DimWindow::new().all("time").into()],
        )
        .unwrap_err(),
    );
    assert!(msg.contains("time"), "{msg}");
}

/// The original is unchanged; the windowed provider keeps refresh's decoded-file
/// reuse and windows exactly as the full read sliced by name.
#[test]
fn with_window_bakes_the_window_into_a_new_provider() {
    let src = &equivalence()["sources"]["hourly"];
    let p = scenario_provider(src, 1, corpus_cache());
    let w: Selection = DimWindow::new()
        .indices("lon", vec![2, 0, 2])
        .range("lat", 1, 2)
        .into();
    let mut windowed = p.with_window(w.clone()).unwrap();
    for t in p.refresh_times() {
        let t = ts(t as i64);
        let full = p.sample_at(t).unwrap();
        let sampled = windowed.sample_at(t).unwrap();
        assert_ds_bitwise(
            &format!("window t={t}"),
            &sampled,
            &slice_by_name(&full, &w),
        );
        let vars = windowed.refresh(t).unwrap().unwrap();
        assert_ds_bitwise(
            &format!("windowed refresh t={t}"),
            &NativeDataset {
                variables: vars,
                coords: windowed.coords().clone(),
            },
            &sampled,
        );
        assert_eq!(
            full.variables["Q"].shape,
            vec![2, 3],
            "the original keeps its full grid"
        );
    }
}

/// A window naming a dimension no field carries fails in `sample_windows_at`
/// exactly as in its own read, even when the other windows do not name it.
#[test]
fn every_window_s_own_errors_survive_the_union() {
    let p = scenario_provider(&equivalence()["sources"]["hourly"], 1, corpus_cache());
    let t = ts(1_704_067_200);
    let good: Selection = DimWindow::new().range("lat", 0, 1).into();
    for bad in [
        DimWindow::new().all("level"),
        DimWindow::new().indices("lon", vec![3]),
        DimWindow::new().step_range("lon", 0, 2, 0),
    ] {
        let bad: Selection = bad.into();
        assert!(
            p.with_window(bad.clone()).unwrap().sample_at(t).is_err(),
            "{bad:?} alone"
        );
        assert!(
            p.sample_windows_at(t, &[good.clone(), bad.clone()])
                .is_err(),
            "{bad:?} in a list"
        );
    }
}

/// A window is a NAMED-dimension select (`spec/prefetch.md` §2.3): a positional
/// one is refused by `with_window` and `sample_windows_at` alike, before any I/O,
/// as the Python and Julia tracks refuse it.
#[test]
fn positional_windows_are_refused() {
    let p = scenario_provider(&equivalence()["sources"]["hourly"], 2, corpus_cache());
    let t = ts(1_704_078_000);
    let ws = selections(&json!([
        {"axes": ["all", {"indices": [1]}, {"slice": [0, 3, 2]}]},
        {"dims": {"lon": {"indices": [2]}}},
    ]));
    match p.with_window(ws[0].clone()).err().unwrap() {
        Error::Format { detail, .. } => assert!(detail.contains("NAMED-dimension"), "{detail}"),
        other => panic!("expected the precondition error, got {other:?}"),
    }
    assert!(p.sample_windows_at(t, &ws).is_err());
    assert!(p.with_window(Selection::All).is_err());
}

/// Scenario `multi-window`: `sample_windows_at(t, W)` equals
/// `[with_window(w).sample_at(t) for w in W]` bitwise and each equals the whole
/// read sliced by name — on the mixed-rank blob as a CONST provider
/// (overlapping, permuted, repeated, wrapping and empty windows), and on the
/// hourly source at every refresh time for both record counts, seams included.
#[test]
fn scenario_multi_window() {
    let sc = read_json("scenarios/multi-window.json");

    let c = &sc["const"];
    let mut loader = DataSource::new(
        "geosfp",
        c["format"].as_str().unwrap(),
        c["url"].as_str().unwrap(),
    );
    let vars: Vec<&str> = c["variables"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    loader = loader.variables(vars);
    let p = Provider::new(loader, corpus_cache(), None).unwrap();
    check_windows(
        "const",
        &p,
        OffsetDateTime::UNIX_EPOCH,
        &selections(&c["windows"]),
    );

    let d = &sc["discrete"];
    let windows = selections(&d["windows"]);
    for rps in d["records_per_sample"].as_array().unwrap() {
        let rps = rps.as_u64().unwrap() as u32;
        let p = scenario_provider(&d["source"], rps, corpus_cache());
        for t in p.refresh_times() {
            check_windows(
                &format!("discrete rps={rps} t={t}"),
                &p,
                ts(t as i64),
                &windows,
            );
        }
    }
}

fn check_windows(ctx: &str, p: &Provider, t: OffsetDateTime, windows: &[Selection]) {
    let full = p.sample_at(t).unwrap();
    let got = p.sample_windows_at(t, windows).unwrap();
    assert_eq!(got.len(), windows.len(), "{ctx}: one dataset per window");
    for (i, w) in windows.iter().enumerate() {
        let alone = p.with_window(w.clone()).unwrap().sample_at(t).unwrap();
        assert_ds_bitwise(&format!("{ctx} window {i} vs with_window"), &got[i], &alone);
        assert_ds_bitwise(
            &format!("{ctx} window {i} vs sliced"),
            &got[i],
            &slice_by_name(&full, w),
        );
    }
}

/// Window B leaves `time` unnamed, so the union reads it whole; window A's
/// subset of it must still be refused, as A's own read refuses it — on a CONST
/// provider, where `time` is just a dimension of the file.
#[test]
fn a_time_subset_the_union_read_whole_is_still_refused() {
    let url = "https://data.earthsci.dev/geosfp/mini/GEOSFP.20240101.A3mixed.nc";
    let p = Provider::new(
        DataSource::new("geosfp", "netcdf", url),
        corpus_cache(),
        None,
    )
    .unwrap();
    let t = OffsetDateTime::UNIX_EPOCH;
    let a = DimWindow::new().range("time", 0, 1);
    let alone = p.with_window(a.clone()).and_then(|w| w.sample_at(t));
    let msg = format!("{}", alone.expect_err("a time subset alone is refused"));
    assert!(msg.contains("subset of the time dimension"), "{msg}");
    let both = p.sample_windows_at(t, &[a.into(), DimWindow::new().range("lat", 0, 1).into()]);
    let msg = format!("{}", both.expect_err("a time subset in a union is refused"));
    assert!(msg.contains("subset of the time dimension"), "{msg}");
}
