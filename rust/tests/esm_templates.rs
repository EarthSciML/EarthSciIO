//! esm URL templates (`spec/esm-sources.md` §1): the shared
//! `esm-url-templates` scenario, and esm mirror templates through a Provider.

#![cfg(not(target_arch = "wasm32"))]

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use earthsciio::{
    esm_template_names, expand_esm_template, ArrayData, Cache, DataSource, Provider, SourceTemporal,
};
use time::macros::datetime;
use time::{Duration, OffsetDateTime};

fn corpus() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../conformance/corpus")
}

fn anchor(v: &serde_json::Value) -> Option<OffsetDateTime> {
    let s = v.as_f64()?;
    let whole = s.floor();
    let nanos = ((s - whole) * 1e9).round() as i64;
    Some(
        OffsetDateTime::from_unix_timestamp(whole as i64).expect("anchor in range")
            + Duration::nanoseconds(nanos),
    )
}

#[test]
fn the_shared_scenario() {
    let text = std::fs::read_to_string(corpus().join("scenarios/esm-url-templates.json")).unwrap();
    let sc: serde_json::Value = serde_json::from_str(&text).unwrap();
    for case in sc["cases"].as_array().unwrap() {
        let id = case["id"].as_str().unwrap();
        let template = case["template"].as_str().unwrap();
        let subs: BTreeMap<String, String> = case["substitutions"]
            .as_object()
            .unwrap()
            .iter()
            .map(|(k, v)| (k.clone(), v.as_str().unwrap().to_string()))
            .collect();
        let got = expand_esm_template(template, anchor(&case["anchor_s"]), &subs);
        if let Some(expect) = case.get("expect") {
            assert_eq!(got.as_deref().ok(), expect.as_str(), "case {id}: {got:?}");
        } else {
            let code = case["error"].as_str().unwrap();
            let e = got.expect_err(id);
            assert_eq!(e.code, code, "case {id}");
            let msg = e.to_string();
            assert!(msg.contains(code), "case {id}: {msg}");
            for m in case
                .get("mentions")
                .and_then(|m| m.as_array())
                .into_iter()
                .flatten()
            {
                assert!(msg.contains(m.as_str().unwrap()), "case {id}: {msg}");
            }
        }
        if let Some(names) = case.get("names") {
            let want: Vec<String> = names
                .as_array()
                .unwrap()
                .iter()
                .map(|n| n.as_str().unwrap().to_string())
                .collect();
            assert_eq!(esm_template_names(template), want, "case {id}");
        }
    }
}

fn f64s(ds: &std::collections::HashMap<String, earthsciio::NativeField>, v: &str) -> Vec<f64> {
    match &ds[v].data {
        ArrayData::F64(x) => x.clone(),
        other => panic!("expected f64, got {:?}", other.dtype()),
    }
}

#[test]
fn mirror_templates_expand_at_the_primary_anchor() {
    let dir = tempfile::tempdir().unwrap();
    let mirror = dir.path().join("mirror");
    std::fs::create_dir_all(&mirror).unwrap();
    let blobs = corpus().join("cache/v1/blobs");
    let jan_src =
        blobs.join("13/13a11223ee45f734cc0898ccd72049a16e3d99f591a0db6497fc4d2d45ee2e7b.nc");
    let feb_src =
        blobs.join("e1/e10c71657146eca3d71e304ce32609ffd719d9f63b7525e744a573f12320d2cf.nc");
    std::fs::copy(&jan_src, mirror.join("emis_agr_01.nc")).unwrap();
    std::fs::copy(&feb_src, mirror.join("emis_agr_02.nc")).unwrap();
    let cache = |name: &str| {
        Arc::new(
            Cache::builder()
                .data_dir(dir.path().join(name))
                .build()
                .unwrap(),
        )
    };

    // An unbound substitution is refused when the provider is built.
    let unbound = DataSource::new("nei", "netcdf", "file:///x/{sector}_{date:%m}.nc")
        .esm_template(BTreeMap::<String, String>::new());
    let e = Provider::new(unbound, cache("c0"), None)
        .err()
        .expect("unbound");
    assert!(e.to_string().contains("esm_template_unbound"), "{e}");

    let primary = format!(
        "file://{}/missing/emis_{{sector}}_{{date:%m}}.nc",
        dir.path().display()
    );
    let loader = DataSource::new("nei", "netcdf", primary)
        .variables(["Q"])
        .mirrors([format!(
            "file://{}/emis_{{sector}}_{{date:%m}}.nc",
            mirror.display()
        )])
        .esm_template([("sector", "agr")])
        .temporal(SourceTemporal::new(
            datetime!(2016-01-01 00:00:00 UTC),
            Duration::days(31),
            Duration::days(31),
        ));
    let p = Provider::new(loader, cache("c1"), None).unwrap();
    let jan = p.sample_at(datetime!(2016-01-15 00:00:00 UTC)).unwrap();
    let feb = p.sample_at(datetime!(2016-02-15 00:00:00 UTC)).unwrap();

    // Each equals the first record of the mirror file it names.
    for (got, src) in [(&jan, &jan_src), (&feb, &feb_src)] {
        let direct = DataSource::new("d", "netcdf", format!("file://{}", src.display()))
            .variables(["Q"])
            .temporal(SourceTemporal::new(
                datetime!(2016-01-01 00:00:00 UTC),
                Duration::days(31),
                Duration::days(31),
            ));
        let d = Provider::new(direct, cache("c2"), None)
            .unwrap()
            .sample_at(datetime!(2016-01-01 00:00:00 UTC))
            .unwrap();
        assert_eq!(f64s(&got.variables, "Q"), f64s(&d.variables, "Q"));
    }
    assert_ne!(f64s(&jan.variables, "Q"), f64s(&feb.variables, "Q"));
}

#[test]
fn the_shared_scenario_construction() {
    let text = std::fs::read_to_string(corpus().join("scenarios/esm-url-templates.json")).unwrap();
    let sc: serde_json::Value = serde_json::from_str(&text).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let cache = Arc::new(Cache::builder().data_dir(dir.path()).build().unwrap());
    for case in sc["construction"].as_array().unwrap() {
        let id = case["id"].as_str().unwrap();
        let subs: BTreeMap<String, String> = case["substitutions"]
            .as_object()
            .unwrap()
            .iter()
            .map(|(k, v)| (k.clone(), v.as_str().unwrap().to_string()))
            .collect();
        let mirrors: Vec<String> = case["mirrors"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m.as_str().unwrap().to_string())
            .collect();
        let loader = DataSource::new("c", "netcdf", case["template"].as_str().unwrap())
            .mirrors(mirrors)
            .esm_template(subs);
        let built = Provider::new(loader, cache.clone(), None);
        match case.get("error").and_then(|e| e.as_str()) {
            None => assert!(built.is_ok(), "case {id}: {:?}", built.err()),
            Some(code) => {
                let msg = built.err().expect(id).to_string();
                assert!(msg.contains(code), "case {id}: {msg}");
                for m in case
                    .get("mentions")
                    .and_then(|m| m.as_array())
                    .into_iter()
                    .flatten()
                {
                    assert!(msg.contains(m.as_str().unwrap()), "case {id}: {msg}");
                }
            }
        }
    }
}
