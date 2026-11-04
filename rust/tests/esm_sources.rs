//! Building a loader from an esm `data_sources` entry (`spec/esm-sources.md`
//! §3), driven by the shared `esm-sources` scenario.

#![cfg(not(target_arch = "wasm32"))]

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use earthsciio::{expand_esm_template, ArrayData, Cache, DataSource, EsmSourceOptions, Provider};
use time::OffsetDateTime;

fn corpus() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../conformance/corpus")
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

fn strings(v: Option<&serde_json::Value>) -> Vec<String> {
    v.and_then(|v| v.as_array())
        .map(|a| a.iter().map(|x| x.as_str().unwrap().to_string()).collect())
        .unwrap_or_default()
}

fn options(o: &serde_json::Value, records_per_sample: Option<u32>) -> EsmSourceOptions {
    EsmSourceOptions {
        variables: strings(o.get("variables")),
        substitutions: o
            .get("substitutions")
            .and_then(|s| s.as_object())
            .map(|m| {
                m.iter()
                    .map(|(k, v)| (k.clone(), v.as_str().unwrap().to_string()))
                    .collect::<BTreeMap<_, _>>()
            })
            .unwrap_or_default(),
        url_override: o
            .get("url_override")
            .and_then(|u| u.as_str())
            .map(str::to_string),
        records_per_sample,
    }
}

/// `DataSource::from_esm`, then `Provider::new` over the offline corpus.
fn build(
    case: &serde_json::Value,
    records_per_sample: Option<u32>,
) -> earthsciio::Result<(DataSource, Provider)> {
    let o = &case["options"];
    let src = DataSource::from_esm(
        case["name"].as_str().unwrap(),
        &case["entry"],
        &options(o, records_per_sample),
    )?;
    let window = o.get("window_s").map(|w| (at(&w[0]), at(&w[1])));
    let p = Provider::new(src.clone(), corpus_cache(), window)?;
    Ok((src, p))
}

fn flatten(v: &serde_json::Value, out: &mut Vec<f64>) {
    match v {
        serde_json::Value::Array(a) => a.iter().for_each(|x| flatten(x, out)),
        other => out.push(other.as_f64().unwrap()),
    }
}

fn f64s(data: &ArrayData) -> Vec<f64> {
    match data {
        ArrayData::F64(v) => v.clone(),
        other => panic!("expected f64 data, got {:?}", other.dtype()),
    }
}

#[test]
fn every_case_of_the_shared_scenario() {
    let text = std::fs::read_to_string(corpus().join("scenarios/esm-sources.json")).unwrap();
    let sc: serde_json::Value = serde_json::from_str(&text).unwrap();
    for case in sc["cases"].as_array().unwrap() {
        let id = case["id"].as_str().unwrap();
        if let Some(code) = case.get("error").and_then(|e| e.as_str()) {
            let e = build(case, None)
                .err()
                .unwrap_or_else(|| panic!("{id}: no error"));
            let msg = e.to_string();
            assert!(msg.contains(code), "{id}: {msg}");
            for m in strings(case.get("mentions")) {
                assert!(msg.contains(&m), "{id}: {msg} lacks {m:?}");
            }
            continue;
        }
        let exp = &case["expect"];
        let (src, p) = build(case, None).unwrap_or_else(|e| panic!("{id}: {e}"));
        assert_eq!(src.format, exp["format"].as_str().unwrap(), "{id}");
        assert_eq!(
            src.temporal.is_none(),
            exp["const"].as_bool().unwrap(),
            "{id}: const"
        );
        let want: Vec<f64> = exp["refresh_times_s"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_f64().unwrap())
            .collect();
        assert_eq!(p.refresh_times(), want, "{id}: refresh times");
        if let Some(t) = &src.temporal {
            assert_eq!(t.time_dim, exp["time_dim"].as_str().unwrap(), "{id}");
        }
        let subs = src.esm_substitutions.clone().expect("an esm template");
        for f in exp["files"].as_array().unwrap() {
            let fa = match &src.temporal {
                None => OffsetDateTime::UNIX_EPOCH,
                Some(t) => {
                    let anchor = t.record_anchor(at(&f["t_s"]));
                    let file_anchor = t.file_anchor(anchor);
                    assert_eq!(
                        file_anchor.unix_timestamp(),
                        f["file_anchor_s"].as_i64().unwrap(),
                        "{id}: file anchor"
                    );
                    assert_eq!(
                        t.record_in_file(anchor, file_anchor),
                        f["record"].as_i64().unwrap(),
                        "{id}"
                    );
                    file_anchor
                }
            };
            let url = expand_esm_template(&src.url_template, Some(fa), &subs).unwrap();
            assert_eq!(url, f["url"].as_str().unwrap(), "{id}: url");
            let mirrors: Vec<String> = src
                .mirrors
                .iter()
                .map(|m| expand_esm_template(m, Some(fa), &subs).unwrap())
                .collect();
            assert_eq!(mirrors, strings(f.get("mirrors")), "{id}: mirrors");
        }
        let vars = strings(case["options"].get("variables"));
        for s in exp
            .get("samples")
            .and_then(|s| s.as_array())
            .into_iter()
            .flatten()
        {
            let rps = s["records_per_sample"].as_u64().unwrap() as u32;
            let (_, p) = build(case, Some(rps)).unwrap();
            let t = at(&s["t_s"]);
            let ds = p
                .sample_at(t)
                .unwrap_or_else(|e| panic!("{id} at {t}: {e}"));
            let mut expect = Vec::new();
            flatten(&s[vars[0].as_str()], &mut expect);
            assert_eq!(f64s(&ds.variables[&vars[0]].data), expect, "{id} at {t}");
            if let Some(times) = s.get("time_s") {
                let mut expect = Vec::new();
                flatten(times, &mut expect);
                let tdim = exp["time_dim"].as_str().unwrap();
                assert_eq!(
                    f64s(&ds.coords[tdim].field.data),
                    expect,
                    "{id} time at {t}"
                );
            }
        }
    }
}

#[test]
fn a_url_override_must_still_be_a_url() {
    let entry = serde_json::json!({
        "source": {"url_template": "https://h/{date:%Y}.nc"},
        "temporal": {"start": "2020-01-01T00:00:00Z", "frequency": "P1Y"},
        "metadata": {"esio_format": "netcdf"}
    });
    let ok = EsmSourceOptions {
        url_override: Some("file:///local/{date:%Y}.nc".to_string()),
        ..Default::default()
    };
    let src = DataSource::from_esm("S", &entry, &ok).unwrap();
    assert!(src.mirrors.is_empty(), "an override drops the mirrors");
    let relative = EsmSourceOptions {
        url_override: Some("local/{date:%Y}.nc".to_string()),
        ..Default::default()
    };
    let e = DataSource::from_esm("S", &entry, &relative).unwrap_err();
    assert!(e.to_string().contains("esm_source_url"), "{e}");
}
