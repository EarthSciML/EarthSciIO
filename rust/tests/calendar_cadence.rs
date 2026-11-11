//! Calendar cadences (`spec/esm-sources.md` §2), driven by the shared
//! `calendar-cadence` scenario: period parsing, the anchors a cadence derives,
//! and providers over committed monthly and yearly files.

#![cfg(not(target_arch = "wasm32"))]

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use earthsciio::{
    ArrayData, Cache, CalendarPeriod, DataSource, Error, NativeField, PrefetchGroup, Provider,
    SourceTemporal,
};
use time::OffsetDateTime;

fn corpus() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../conformance/corpus")
}

fn scenario() -> serde_json::Value {
    let text = std::fs::read_to_string(corpus().join("scenarios/calendar-cadence.json")).unwrap();
    serde_json::from_str(&text).unwrap()
}

fn at(v: &serde_json::Value) -> OffsetDateTime {
    OffsetDateTime::from_unix_timestamp(v.as_i64().expect("whole Unix seconds")).unwrap()
}

fn secs(t: OffsetDateTime) -> i64 {
    t.unix_timestamp()
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

#[test]
fn periods_parse_to_months_and_seconds() {
    let sc = scenario();
    for p in sc["periods"].as_array().unwrap() {
        let iso = p["iso"].as_str().unwrap();
        let got = CalendarPeriod::parse(iso).unwrap_or_else(|e| panic!("{iso}: {e}"));
        assert_eq!(got.months, p["months"].as_i64().unwrap(), "{iso}");
        assert_eq!(got.seconds, p["seconds"].as_i64().unwrap(), "{iso}");
    }
    for bad in sc["bad_periods"].as_array().unwrap() {
        let iso = bad.as_str().unwrap();
        let e = CalendarPeriod::parse(iso).expect_err(iso);
        assert!(e.to_string().contains("esm_period_invalid"), "{iso}: {e}");
    }
}

#[test]
fn cadences_derive_the_calendar_anchors() {
    let sc = scenario();
    for c in sc["cadences"].as_array().unwrap() {
        let id = c["id"].as_str().unwrap();
        let mut temporal = SourceTemporal::new(
            at(&c["start_s"]),
            CalendarPeriod::parse(c["frequency"].as_str().unwrap()).unwrap(),
            CalendarPeriod::parse(c["file_period"].as_str().unwrap()).unwrap(),
        );
        if let Some(end) = c.get("end_s") {
            temporal = temporal.end(at(end));
        }
        let w = c["window_s"].as_array().unwrap();
        let window = (at(&w[0]), at(&w[1]));
        let loader = DataSource::new(id, "netcdf", "file:///unused.nc").temporal(temporal.clone());
        let p = Provider::new(loader, corpus_cache(), Some(window)).unwrap();
        let want: Vec<f64> = c["refresh_times_s"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_f64().unwrap())
            .collect();
        assert_eq!(p.refresh_times(), want, "{id}: refresh times");
        for a in c["at"].as_array().unwrap() {
            let t = at(&a["t_s"]);
            let label = format!("{id} at {}", a["t"]);
            let anchor = temporal.record_anchor(t);
            let file_anchor = temporal.file_anchor(anchor);
            assert_eq!(
                secs(anchor),
                a["record_anchor_s"].as_i64().unwrap(),
                "{label}"
            );
            assert_eq!(
                secs(file_anchor),
                a["file_anchor_s"].as_i64().unwrap(),
                "{label}"
            );
            assert_eq!(
                temporal.record_in_file(anchor, file_anchor),
                a["record"].as_i64().unwrap(),
                "{label}"
            );
            assert_eq!(
                secs(temporal.next_record_anchor(t)),
                a["next_record_anchor_s"].as_i64().unwrap(),
                "{label}"
            );
        }
    }
}

#[test]
fn a_record_before_its_file_is_negative_not_wrapped() {
    let temporal = SourceTemporal::new(
        at(&serde_json::json!(1_451_606_400)), // 2016-01-01
        CalendarPeriod::months(1),
        CalendarPeriod::months(1),
    );
    let jan = at(&serde_json::json!(1_451_606_400));
    let feb = at(&serde_json::json!(1_454_284_800));
    assert_eq!(temporal.record_in_file(jan, feb), -1);
}

#[test]
fn a_fractional_or_zero_duration_step_is_refused_by_the_provider() {
    use time::Duration;
    for (freq, file) in [
        (Duration::milliseconds(1500), Duration::hours(1)),
        (Duration::hours(1), Duration::milliseconds(500)),
        (Duration::ZERO, Duration::hours(1)),
    ] {
        let temporal = SourceTemporal::new(OffsetDateTime::UNIX_EPOCH, freq, file);
        let loader = DataSource::new("x", "netcdf", "file:///unused.nc").temporal(temporal);
        let e = Provider::new(loader, corpus_cache(), None)
            .err()
            .unwrap_or_else(|| panic!("{freq:?}/{file:?} was accepted"));
        assert!(e.to_string().contains("esm_period_invalid"), "{e}");
    }
}

/// Flatten a nested JSON array of numbers in row-major order.
fn flatten(v: &serde_json::Value, out: &mut Vec<f64>) {
    match v {
        serde_json::Value::Array(a) => a.iter().for_each(|x| flatten(x, out)),
        other => out.push(other.as_f64().unwrap()),
    }
}

fn f64s(field: &NativeField) -> Vec<f64> {
    match &field.data {
        ArrayData::F64(v) => v.clone(),
        other => panic!("expected float data, got {:?}", other.dtype()),
    }
}

#[test]
fn providers_read_monthly_and_yearly_files() {
    let sc = scenario();
    for (name, src) in sc["providers"].as_object().unwrap() {
        let subs: BTreeMap<String, String> = src["substitutions"]
            .as_object()
            .unwrap()
            .iter()
            .map(|(k, v)| (k.clone(), v.as_str().unwrap().to_string()))
            .collect();
        let vars: Vec<String> = src["variables"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect();
        let w = src["window_s"].as_array().unwrap();
        let window = (at(&w[0]), at(&w[1]));
        let make = |rps: u32| {
            let temporal = SourceTemporal::new(
                at(&src["start_s"]),
                CalendarPeriod::parse(src["frequency"].as_str().unwrap()).unwrap(),
                CalendarPeriod::parse(src["file_period"].as_str().unwrap()).unwrap(),
            )
            .end(at(&src["end_s"]))
            .time_dim(src["time_dim"].as_str().unwrap())
            .records_per_sample(rps);
            let loader = DataSource::new(
                name.as_str(),
                src["format"].as_str().unwrap(),
                src["url_template"].as_str().unwrap(),
            )
            .esm_template(subs.clone())
            .variables(vars.clone())
            .temporal(temporal);
            Provider::new(loader, corpus_cache(), Some(window)).unwrap()
        };
        let want: Vec<f64> = src["refresh_times_s"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_f64().unwrap())
            .collect();
        assert_eq!(make(1).refresh_times(), want, "{name}: refresh times");
        for s in src["samples"].as_array().unwrap() {
            let rps = s["records_per_sample"].as_u64().unwrap() as u32;
            let t = at(&s["t_s"]);
            let ds = make(rps)
                .sample_at(t)
                .unwrap_or_else(|e| panic!("{name} at {t}: {e}"));
            for v in &vars {
                let mut expect = Vec::new();
                flatten(&s[v.as_str()], &mut expect);
                assert_eq!(
                    f64s(&ds.variables[v]),
                    expect,
                    "{name} {v} at {t} (rps {rps})"
                );
            }
            if let Some(times) = s.get("time_s") {
                let mut expect = Vec::new();
                flatten(times, &mut expect);
                let tdim = src["time_dim"].as_str().unwrap();
                assert_eq!(f64s(&ds.coords[tdim].field), expect, "{name} time at {t}");
            }
        }
    }
}

/// Which records serve a run window, and which windows are refused
/// (`spec/esm-sources.md` §2.6).
#[test]
fn windows_are_served_or_refused() {
    let sc = scenario();
    for c in sc["windows"].as_array().unwrap() {
        let id = c["id"].as_str().unwrap();
        let mut temporal = SourceTemporal::new(
            at(&c["start_s"]),
            CalendarPeriod::parse(c["frequency"].as_str().unwrap()).unwrap(),
            CalendarPeriod::parse(c["file_period"].as_str().unwrap()).unwrap(),
        )
        .records_per_sample(c["records_per_sample"].as_u64().unwrap() as u32);
        if let Some(end) = c.get("end_s") {
            temporal = temporal.end(at(end));
        }
        let w = c["window_s"].as_array().unwrap();
        let window = (at(&w[0]), at(&w[1]));
        let loader = DataSource::new("x", "netcdf", "file:///unused.nc").temporal(temporal);
        let made = Provider::new(loader, corpus_cache(), Some(window));
        if let Some(code) = c.get("error") {
            let e = made
                .err()
                .unwrap_or_else(|| panic!("{id}: the window was accepted"));
            assert!(matches!(e, Error::WindowNotCovered { .. }), "{id}: {e}");
            assert!(
                e.to_string().starts_with(code.as_str().unwrap()),
                "{id}: {e}"
            );
            continue;
        }
        let p = made.unwrap_or_else(|e| panic!("{id}: {e}"));
        let secs = |k: &str| -> Vec<f64> {
            c[k].as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_f64().unwrap())
                .collect()
        };
        assert_eq!(
            p.window_records(),
            secs("window_records_s"),
            "{id}: window records"
        );
        assert_eq!(
            p.refresh_times(),
            secs("refresh_times_s"),
            "{id}: refresh times"
        );
        // A group schedules it even with no refresh times: its first epoch
        // starts at the window start (`spec/prefetch.md` §3.2).
        let mut expected = secs("refresh_times_s");
        let lo = w[0].as_f64().unwrap();
        if expected.first() != Some(&lo) {
            expected.insert(0, lo);
        }
        let g = PrefetchGroup::builder()
            .member("x", Arc::new(p))
            .build()
            .unwrap_or_else(|e| panic!("{id}: {e}"));
        assert_eq!(g.boundaries(), expected, "{id}: group boundaries");
    }
}

/// `prefetch(window)` checks the window it is given as the constructor checks
/// its own: never silently clamped to the data (`spec/esm-sources.md` §2.6).
#[test]
fn prefetch_refuses_a_window_the_data_does_not_cover() {
    let sc = scenario();
    for c in sc["windows"].as_array().unwrap() {
        let Some(code) = c.get("error") else {
            continue;
        };
        let id = c["id"].as_str().unwrap();
        let mut temporal = SourceTemporal::new(
            at(&c["start_s"]),
            CalendarPeriod::parse(c["frequency"].as_str().unwrap()).unwrap(),
            CalendarPeriod::parse(c["file_period"].as_str().unwrap()).unwrap(),
        )
        .records_per_sample(c["records_per_sample"].as_u64().unwrap() as u32);
        if let Some(end) = c.get("end_s") {
            temporal = temporal.end(at(end));
        }
        let w = c["window_s"].as_array().unwrap();
        let window = (at(&w[0]), at(&w[1]));
        let loader = DataSource::new("x", "netcdf", "file:///unused.nc").temporal(temporal);
        let mut p = Provider::new(loader, corpus_cache(), None).unwrap();
        match p.prefetch(window) {
            Err(e @ Error::WindowNotCovered { .. }) => {
                assert!(
                    e.to_string().starts_with(code.as_str().unwrap()),
                    "{id}: {e}"
                )
            }
            other => panic!("{id}: expected WindowNotCovered, got {other:?}"),
        }
    }
}

/// A two-record bracket over the window's last interval reads the record after
/// the window, so prefetch warms that record's file too (§2.6).
#[test]
fn prefetch_reaches_the_bracket_successor() {
    let sc = scenario();
    for (name, src) in sc["providers"].as_object().unwrap() {
        let Some(cases) = src.get("prefetch") else {
            continue;
        };
        let subs: BTreeMap<String, String> = src["substitutions"]
            .as_object()
            .unwrap()
            .iter()
            .map(|(k, v)| (k.clone(), v.as_str().unwrap().to_string()))
            .collect();
        for c in cases.as_array().unwrap() {
            let mut temporal = SourceTemporal::new(
                at(&src["start_s"]),
                CalendarPeriod::parse(src["frequency"].as_str().unwrap()).unwrap(),
                CalendarPeriod::parse(src["file_period"].as_str().unwrap()).unwrap(),
            )
            .time_dim(src["time_dim"].as_str().unwrap())
            .records_per_sample(c["records_per_sample"].as_u64().unwrap() as u32);
            if !c["open_ended"].as_bool().unwrap() {
                temporal = temporal.end(at(&src["end_s"]));
            }
            let loader = DataSource::new(
                name.as_str(),
                src["format"].as_str().unwrap(),
                src["url_template"].as_str().unwrap(),
            )
            .esm_template(subs.clone())
            .temporal(temporal);
            let w = c["window_s"].as_array().unwrap();
            let window = (at(&w[0]), at(&w[1]));
            let mut p = Provider::new(loader, corpus_cache(), Some(window)).unwrap();
            match c["missing_url"].as_str() {
                None => p
                    .prefetch(window)
                    .unwrap_or_else(|e| panic!("{name} {c}: {e}")),
                Some(url) => match p.prefetch(window) {
                    Err(Error::CacheMiss { url: got, .. }) => assert_eq!(got, url, "{name} {c}"),
                    other => panic!("{name} {c}: expected a cache miss, got {other:?}"),
                },
            }
        }
    }
}
