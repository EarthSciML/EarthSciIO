//! Finding a record by the file's own time axis (`spec/esm-sources.md` §2.5),
//! driven by the shared `decoded-time-axis` scenario: the CF decode, and
//! providers over trimmed, irregular and undecodable files in the corpus.

#![cfg(not(target_arch = "wasm32"))]

use std::path::PathBuf;
use std::sync::Arc;

use earthsciio::{
    decode_time_axis, ArrayData, Cache, CalendarPeriod, DataSource, NativeField, Provider,
    SourceTemporal,
};
use time::OffsetDateTime;

fn corpus() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../conformance/corpus")
}

fn scenario() -> serde_json::Value {
    let text = std::fs::read_to_string(corpus().join("scenarios/decoded-time-axis.json")).unwrap();
    serde_json::from_str(&text).unwrap()
}

fn at(v: &serde_json::Value) -> OffsetDateTime {
    OffsetDateTime::from_unix_timestamp(v.as_i64().expect("whole Unix seconds")).unwrap()
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
fn decode_matches_the_python_track() {
    let sc = scenario();
    for c in sc["decode"].as_array().unwrap() {
        let id = c["id"].as_str().unwrap();
        let vals = c["values"].as_array().unwrap();
        let data = if c["dtype"] == "float64" {
            ArrayData::F64(
                vals.iter()
                    .map(|v| v.as_f64().unwrap_or(f64::NAN))
                    .collect(),
            )
        } else {
            ArrayData::I64(vals.iter().map(|v| v.as_i64().unwrap()).collect())
        };
        let want: Option<Vec<i64>> = c["seconds"]
            .as_array()
            .map(|a| a.iter().map(|v| v.as_i64().unwrap()).collect());
        let got = decode_time_axis(&data, c["units"].as_str(), c["calendar"].as_str(), None);
        assert_eq!(got, want, "decode case {id}");
    }
}

fn provider(name: &str, src: &serde_json::Value, rps: u32) -> Provider {
    let temporal = SourceTemporal::new(
        at(&src["start_s"]),
        CalendarPeriod::parse(src["frequency"].as_str().unwrap()).unwrap(),
        CalendarPeriod::parse(src["file_period"].as_str().unwrap()).unwrap(),
    )
    .end(at(&src["end_s"]))
    .time_dim(src["time_dim"].as_str().unwrap())
    .records_per_sample(rps);
    let loader = DataSource::new(
        name,
        src["format"].as_str().unwrap(),
        src["url_template"].as_str().unwrap(),
    )
    .esm_template(std::collections::BTreeMap::<String, String>::new())
    .variables(vec!["T".to_string()])
    .temporal(temporal);
    let w = src["window_s"].as_array().unwrap();
    Provider::new(loader, corpus_cache(), Some((at(&w[0]), at(&w[1])))).unwrap()
}

#[test]
fn providers_find_records_by_the_files_time_axis() {
    let sc = scenario();
    for (name, src) in sc["providers"].as_object().unwrap() {
        for rps in [1u32, 2] {
            let mut p = provider(name, src, rps);
            for s in src["samples"].as_array().unwrap() {
                if s["records_per_sample"].as_u64().unwrap() as u32 != rps {
                    continue;
                }
                let t = at(&s["t_s"]);
                let note = s["note"].as_str().unwrap();
                let ds = p
                    .sample_at(t)
                    .unwrap_or_else(|e| panic!("{name} at {t} (rps {rps}): {e}"));
                let mut expect = Vec::new();
                flatten(&s["T"], &mut expect);
                assert_eq!(
                    f64s(&ds.variables["T"]),
                    expect,
                    "{name} at {t} (rps {rps}): {note}"
                );
                if let Some(times) = s.get("time_s") {
                    let mut expect = Vec::new();
                    flatten(times, &mut expect);
                    assert_eq!(f64s(&ds.coords["time"].field), expect, "{name} time at {t}");
                }
                // refresh (the buffered path) agrees with sample_at
                if let Some(buf) = p.refresh(t).unwrap() {
                    assert_eq!(
                        f64s(&buf["T"]),
                        expect_flat(&s["T"]),
                        "{name} refresh at {t}"
                    );
                }
            }
            for e in src["errors"].as_array().unwrap() {
                if e["records_per_sample"].as_u64().unwrap() as u32 != rps {
                    continue;
                }
                let t = at(&e["t_s"]);
                assert!(
                    p.sample_at(t).is_err(),
                    "{name} at {t} (rps {rps}) should fail"
                );
            }
        }
    }
}

fn expect_flat(v: &serde_json::Value) -> Vec<f64> {
    let mut out = Vec::new();
    flatten(v, &mut out);
    out
}
