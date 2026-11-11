//! A provider that counts time from a time origin (`spec/esm-sources.md` §4),
//! driven by the shared `time-origin` scenario, plus the Rust spellings.

#![cfg(not(target_arch = "wasm32"))]

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use earthsciio::{
    ArrayData, Cache, DataSource, EsmSourceOptions, Order, PrefetchGroup, Provider, ProviderTime,
};
use time::format_description::well_known::Rfc3339;
use time::macros::datetime;
use time::OffsetDateTime;

fn corpus() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../conformance/corpus")
}

fn corpus_cache() -> Arc<Cache> {
    Arc::new(
        Cache::builder()
            .data_dir(corpus().join("cache"))
            .offline(true)
            .verify_on_read(true)
            .build()
            .unwrap(),
    )
}

fn scenario() -> serde_json::Value {
    let text = std::fs::read_to_string(corpus().join("scenarios/time-origin.json")).unwrap();
    serde_json::from_str(&text).unwrap()
}

fn case(id: &str) -> serde_json::Value {
    scenario()["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["id"] == id)
        .unwrap()
        .clone()
}

fn strings(v: &serde_json::Value) -> Vec<String> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|x| x.as_str().unwrap().to_string())
        .collect()
}

fn f64s(v: &serde_json::Value) -> Vec<f64> {
    let mut out = Vec::new();
    flatten(v, &mut out);
    out
}

fn flatten(v: &serde_json::Value, out: &mut Vec<f64>) {
    match v {
        serde_json::Value::Array(a) => a.iter().for_each(|x| flatten(x, out)),
        other => out.push(other.as_f64().unwrap()),
    }
}

fn data(d: &ArrayData) -> Vec<f64> {
    match d {
        ArrayData::F64(v) => v.clone(),
        other => panic!("expected f64 data, got {:?}", other.dtype()),
    }
}

/// The loader of a case: `DataSource::from_esm`, then the origin, if any.
fn source(case: &serde_json::Value, rps: Option<u32>) -> earthsciio::Result<DataSource> {
    let o = &case["options"];
    let subs: BTreeMap<String, String> = o["substitutions"]
        .as_object()
        .unwrap()
        .iter()
        .map(|(k, v)| (k.clone(), v.as_str().unwrap().to_string()))
        .collect();
    let opts = EsmSourceOptions {
        variables: strings(&o["variables"]),
        substitutions: subs,
        records_per_sample: rps,
        ..Default::default()
    };
    let src = DataSource::from_esm(case["name"].as_str().unwrap(), &case["entry"], &opts)?;
    Ok(match o["time_origin"].as_str() {
        Some(s) => src.time_origin(OffsetDateTime::parse(s, &Rfc3339).unwrap()),
        None => src,
    })
}

/// The provider of a case over its window, given in seconds from the origin.
fn build(case: &serde_json::Value, rps: Option<u32>) -> earthsciio::Result<Provider> {
    let src = source(case, rps)?;
    let w = &case["options"]["window"];
    let window = src.run_window(w[0].as_f64().unwrap(), w[1].as_f64().unwrap())?;
    Provider::new(src, corpus_cache(), Some(window))
}

#[test]
fn every_case_of_the_shared_scenario() {
    for case in scenario()["cases"].as_array().unwrap() {
        let id = case["id"].as_str().unwrap();
        if let Some(code) = case.get("error").and_then(|e| e.as_str()) {
            let e = build(case, None)
                .err()
                .unwrap_or_else(|| panic!("{id}: no error"));
            assert!(e.to_string().contains(code), "{id}: {e}");
            continue;
        }
        let exp = &case["expect"];
        let p = build(case, None).unwrap_or_else(|e| panic!("{id}: {e}"));
        assert_eq!(p.refresh_times(), f64s(&exp["refresh_times"]), "{id}");
        for (rps, want) in exp["window_records"].as_object().unwrap() {
            let p = build(case, Some(rps.parse().unwrap())).unwrap();
            assert_eq!(p.window_records(), f64s(want), "{id} rps {rps}");
        }

        let epochs = exp["epochs"].as_array().unwrap();
        let mut g = PrefetchGroup::builder()
            .member("m", Arc::new(build(case, Some(1)).unwrap()))
            .build()
            .unwrap();
        let want: Vec<f64> = epochs
            .iter()
            .map(|e| e["boundary"].as_f64().unwrap())
            .collect();
        assert_eq!(g.boundaries(), want, "{id}: boundaries");
        g.start(Order::Forward, None).unwrap();
        for (k, e) in epochs.iter().enumerate() {
            let ep = g.take(k).unwrap();
            assert_eq!(
                ep.get("m").unwrap().anchor,
                e["anchor"].as_f64().unwrap(),
                "{id}: anchor of epoch {k}"
            );
        }

        let var = strings(&case["options"]["variables"])[0].clone();
        let tdim = case["entry"]["temporal"]["time_variable"].as_str().unwrap();
        for s in exp["samples"].as_array().unwrap() {
            let rps = s["records_per_sample"].as_u64().unwrap() as u32;
            let mut p = build(case, Some(rps)).unwrap();
            let t = s["t"].as_f64().unwrap();
            let ds = p
                .sample_at(t)
                .unwrap_or_else(|e| panic!("{id} at {t}: {e}"));
            assert_eq!(
                data(&ds.variables[&var].data),
                f64s(&s[var.as_str()]),
                "{id} at {t}"
            );
            // refresh takes the same model time and returns the same arrays.
            let fresh = p.refresh(t).unwrap().expect("a first refresh changes");
            assert_eq!(
                data(&fresh[&var].data),
                f64s(&s[var.as_str()]),
                "{id} refresh at {t}"
            );
            if let Some(times) = s.get("time") {
                let coord = &ds.coords[tdim];
                assert_eq!(data(&coord.field.data), f64s(times), "{id} time at {t}");
                assert_eq!(
                    coord.units.as_deref(),
                    exp["time_units"].as_str(),
                    "{id} units at {t}"
                );
            }
        }
    }
}

#[test]
fn an_instant_is_still_an_instant() {
    let c = case("nei-monthly");
    let src = source(&c, Some(1)).unwrap();
    let p = Provider::new(
        src,
        corpus_cache(),
        Some((
            datetime!(2016-01-01 0:00 UTC),
            datetime!(2016-04-01 0:00 UTC),
        )),
    )
    .unwrap();
    assert_eq!(p.refresh_times(), vec![0.0, 2_678_400.0, 5_184_000.0]);
    let a = p.sample_at(datetime!(2016-02-29 0:00 UTC)).unwrap();
    let b = p.sample_at(ProviderTime::Seconds(59.0 * 86400.0)).unwrap();
    assert_eq!(
        data(&a.variables["NOX"].data),
        data(&b.variables["NOX"].data)
    );
    assert_eq!(p.instant(0.0).unwrap(), datetime!(2016-01-01 0:00 UTC));
}

#[test]
fn without_an_origin_seconds_are_unix_seconds() {
    let mut c = case("nei-monthly");
    c["options"]["time_origin"] = serde_json::Value::Null;
    let src = source(&c, Some(1)).unwrap();
    assert!(src.time_origin.is_none());
    let t = datetime!(2016-02-29 0:00 UTC);
    assert_eq!(src.instant(t.unix_timestamp() as f64).unwrap(), t);
}

#[test]
fn prefetch_takes_a_window_in_model_seconds() {
    let mut p = build(&case("nei-monthly"), None).unwrap();
    p.prefetch(p.run_window(0.0, 2_678_400.0).unwrap()).unwrap();
    let e = p
        .prefetch(p.run_window(-1.0, 2_678_400.0).unwrap())
        .unwrap_err();
    assert!(e.to_string().contains("esm_window_uncovered"), "{e}");
}

#[test]
fn a_window_keeps_the_origin() {
    let p = build(&case("nei-monthly"), None).unwrap();
    let w = p.with_window(earthsciio::DimWindow::new()).unwrap();
    assert_eq!(w.time_origin(), Some(datetime!(2016-01-01 0:00 UTC)));
    assert_eq!(w.refresh_times(), p.refresh_times());
}

#[test]
fn a_group_refuses_members_on_two_clocks() {
    let c = case("nei-monthly");
    let a = build(&c, None).unwrap();
    let src = source(&c, None)
        .unwrap()
        .time_origin(datetime!(2016-01-02 0:00 UTC));
    let b = Provider::new(src, corpus_cache(), None).unwrap();
    let Err(e) = PrefetchGroup::builder()
        .member("a", Arc::new(a))
        .member("b", Arc::new(b))
        .build()
    else {
        panic!("a group of two clocks was built");
    };
    assert!(e.to_string().contains("time origin"), "{e}");
}

#[test]
fn an_unrepresentable_time_is_an_error() {
    let src = source(&case("nei-monthly"), None).unwrap();
    let e = src.instant(f64::NAN).unwrap_err();
    assert!(e.to_string().starts_with("time_invalid"), "{e}");
}
