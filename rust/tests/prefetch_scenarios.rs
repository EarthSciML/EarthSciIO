//! The `prefetch-equivalence` and `prefetch-errors` scenarios
//! (`conformance/corpus/scenarios/`, `spec/prefetch.md` §6), read from their JSON
//! and run over their full parameter grids against the committed corpus,
//! offline. (`multi-window` runs in `provider_sample_at.rs`.)
//!
//! The gate is §3.5: whatever the order, depth, worker count, byte budget and
//! decode timing, the `take` sequence is the synchronous `refresh` sequence of a
//! fresh provider, bit for bit. Decode latency is randomized through the
//! group's test hook so workers finish out of order.

#![cfg(not(target_arch = "wasm32"))]

mod common;

use std::collections::BTreeMap;
use std::sync::atomic::AtomicU64;
use std::sync::Arc;
use std::time::{Duration as StdDuration, Instant};

use common::{
    assert_ds_bitwise, corpus_cache, fresh_refresh, read_json, scenario_provider, selections,
    splitmix, ts,
};
use earthsciio::{Cache, Error, NativeDataset, Order, PrefetchGroup};
use serde_json::Value;

/// One member of a scenario: its key, source block and record count.
struct Member<'a> {
    key: &'a str,
    src: &'a Value,
    rps: u32,
}

fn members(sc: &Value) -> Vec<Member<'_>> {
    sc["members"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| Member {
            key: m["key"].as_str().unwrap(),
            src: &sc["sources"][m["source"].as_str().unwrap()],
            rps: m["records_per_sample"].as_u64().unwrap() as u32,
        })
        .collect()
}

fn orders(sc: &Value) -> Vec<(String, Order)> {
    sc["orders"]
        .as_array()
        .unwrap()
        .iter()
        .map(|o| match o {
            Value::String(s) if s == "forward" => ("forward".to_string(), Order::Forward),
            Value::String(s) if s == "reverse" => ("reverse".to_string(), Order::Reverse),
            Value::Array(a) => (
                "explicit".to_string(),
                Order::Explicit(a.iter().map(|e| e.as_u64().unwrap() as usize).collect()),
            ),
            other => panic!("unknown order {other}"),
        })
        .collect()
}

fn sequence(order: &Order, n: usize) -> Vec<usize> {
    match order {
        Order::Forward => (0..n).collect(),
        Order::Reverse => (0..n).rev().collect(),
        Order::Explicit(v) => v.clone(),
    }
}

fn usizes(v: &Value) -> Vec<usize> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|x| x.as_u64().unwrap() as usize)
        .collect()
}

/// A latency hook drawing 0–3 ms per decode from a seeded stream, so the
/// completion order differs between runs and between workers.
fn jitter(seed: u64) -> impl Fn(&str, f64) -> StdDuration + Send + Sync + 'static {
    let state = AtomicU64::new(seed);
    move |_, _| StdDuration::from_micros(splitmix(&state) % 3000)
}

#[test]
fn scenario_prefetch_equivalence() {
    let sc = read_json("scenarios/prefetch-equivalence.json");
    let members = members(&sc);
    let expect = &sc["expect"];
    let b: Vec<f64> = expect["boundaries_s"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_f64().unwrap())
        .collect();

    // The right-hand side, once per (member, epoch): a FRESH provider's
    // refresh(B[k]) with its coords — and sample_at agrees with it.
    let mut want: BTreeMap<&str, Vec<NativeDataset>> = BTreeMap::new();
    for m in &members {
        let make = || scenario_provider(m.src, m.rps, corpus_cache());
        let p = make();
        let seq: Vec<NativeDataset> = b
            .iter()
            .map(|&t| fresh_refresh(make, ts(t as i64)))
            .collect();
        for (k, ds) in seq.iter().enumerate() {
            assert_ds_bitwise(
                &format!("{} sample_at({k})", m.key),
                &p.sample_at(ts(b[k] as i64)).unwrap(),
                ds,
            );
        }
        want.insert(m.key, seq);
    }
    let windows = selections(&sc["windows"]);
    let mut want_windows: BTreeMap<&str, Vec<Vec<NativeDataset>>> = BTreeMap::new();
    for m in &members {
        let p = scenario_provider(m.src, m.rps, corpus_cache());
        let per_window: Vec<_> = windows
            .iter()
            .map(|w| p.with_window(w.clone()).unwrap())
            .collect();
        want_windows.insert(
            m.key,
            b.iter()
                .map(|&t| {
                    per_window
                        .iter()
                        .map(|pw| pw.sample_at(ts(t as i64)).unwrap())
                        .collect()
                })
                .collect(),
        );
    }

    let anchors: BTreeMap<&str, Vec<f64>> = members
        .iter()
        .map(|m| {
            let a = expect["anchors_s"][m.key]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_f64().unwrap());
            (m.key, a.collect())
        })
        .collect();

    let mut seed = 1;
    for (order_name, order) in orders(&sc) {
        for depth in usizes(&sc["depth"]) {
            for workers in usizes(&sc["workers"]) {
                for max_bytes in sc["max_bytes"].as_array().unwrap() {
                    seed += 1;
                    let ctx = format!(
                        "{order_name} depth={depth} workers={workers} max_bytes={max_bytes}"
                    );
                    let mut builder = PrefetchGroup::builder()
                        .depth(depth)
                        .workers(workers)
                        .decode_latency(jitter(seed));
                    if let Some(mb) = max_bytes.as_u64() {
                        builder = builder.max_bytes(mb);
                    }
                    for m in &members {
                        builder = builder.member(
                            m.key,
                            Arc::new(scenario_provider(m.src, m.rps, corpus_cache())),
                        );
                    }
                    let mut g = builder.build().unwrap();
                    assert_eq!(g.boundaries(), b, "{ctx}: boundaries");

                    let seq = sequence(&order, b.len());
                    g.start(order.clone(), None).unwrap();
                    let mut prev: BTreeMap<&str, f64> = BTreeMap::new();
                    for (pos, &k) in seq.iter().enumerate() {
                        let ep = g
                            .take(k)
                            .unwrap_or_else(|e| panic!("{ctx}: take({k}): {e}"));
                        assert_eq!(ep.epoch(), k);
                        assert_eq!(ep.len(), members.len());
                        for m in &members {
                            let s = ep.get(m.key).unwrap();
                            let c = format!("{ctx} pos {pos} take({k}) {}", m.key);
                            assert_ds_bitwise(&c, &s.data, &want[m.key][k]);
                            assert_eq!(s.anchor, anchors[m.key][k], "{c}: anchor");
                            let changed = prev.get(m.key) != Some(&s.anchor);
                            assert_eq!(s.changed, changed, "{c}: changed");
                            if order_name == "forward" {
                                assert_eq!(
                                    Value::Bool(s.changed),
                                    expect["changed_forward"][m.key][k],
                                    "{c}"
                                );
                            }
                            prev.insert(m.key, s.anchor);
                        }
                    }
                    let err = g.take(seq[0]).unwrap_err();
                    assert!(
                        matches!(err, Error::Prefetch { .. }),
                        "{ctx}: exhausted: {err}"
                    );

                    let st = g.stats();
                    let n = seq.len() as u64;
                    assert!(st.overlap, "{ctx}");
                    assert!(st.epochs_decoded >= n, "{ctx}: {st:?}");
                    assert_eq!(
                        st.samples_decoded + st.samples_reused,
                        n * members.len() as u64,
                        "{ctx}: {st:?}"
                    );
                    assert!(st.peak_bytes > 0 && st.decode_s > 0.0, "{ctx}: {st:?}");
                    if max_bytes.as_u64() == Some(1) {
                        // Every slot exceeds one byte: each was admitted alone, as the head.
                        assert_eq!(st.oversize_epochs, n, "{ctx}: {st:?}");
                    } else {
                        assert_eq!(st.oversize_epochs, 0, "{ctx}: {st:?}");
                    }
                    if order_name == "forward" {
                        // PS (two-hourly) repeats every other epoch: decoded once per anchor.
                        assert!(st.samples_reused >= 6, "{ctx}: {st:?}");
                    }
                }
            }
        }

        // ...and once per order with `windows`: the member data is the list of
        // `with_window(w).sample_at(B[k])`.
        for max_bytes in [None, Some(1u64)] {
            seed += 1;
            let ctx = format!("{order_name} windows max_bytes={max_bytes:?}");
            let mut builder = PrefetchGroup::builder()
                .depth(2)
                .workers(4)
                .decode_latency(jitter(seed));
            if let Some(mb) = max_bytes {
                builder = builder.max_bytes(mb);
            }
            for m in &members {
                builder = builder.member(
                    m.key,
                    Arc::new(scenario_provider(m.src, m.rps, corpus_cache())),
                );
            }
            let mut g = builder.windows(windows.clone()).build().unwrap();
            g.start(order.clone(), None).unwrap();
            for k in sequence(&order, b.len()) {
                let ep = g.take(k).unwrap();
                for m in &members {
                    let got = &ep.get(m.key).unwrap().data;
                    assert_eq!(got.len(), windows.len(), "{ctx}");
                    for (i, ds) in got.iter().enumerate() {
                        assert_ds_bitwise(
                            &format!("{ctx} take({k}) {} w{i}", m.key),
                            ds,
                            &want_windows[m.key][k][i],
                        );
                    }
                }
            }
        }
    }
}

/// The scenario's source, declared one file longer than the corpus holds.
fn error_group(sc: &Value, cache: Arc<Cache>, depth: usize, workers: usize) -> PrefetchGroup {
    let rps = sc["records_per_sample"].as_u64().unwrap() as u32;
    PrefetchGroup::builder()
        .member("Q", Arc::new(scenario_provider(&sc["source"], rps, cache)))
        .depth(depth)
        .workers(workers)
        .decode_latency(jitter(depth as u64 * 31 + workers as u64))
        .build()
        .unwrap()
}

fn wait_ready(g: &PrefetchGroup, k: usize) {
    let deadline = Instant::now() + StdDuration::from_secs(20);
    while !g.ready(k) {
        assert!(Instant::now() < deadline, "slot {k} never finished");
        std::thread::sleep(StdDuration::from_millis(2));
    }
}

#[test]
fn scenario_prefetch_errors() {
    let sc = read_json("scenarios/prefetch-errors.json");
    let missing_from = sc["missing_from"].as_u64().unwrap() as usize;
    let cancel_after = sc["cancel_after"].as_u64().unwrap() as usize;
    let rps = sc["records_per_sample"].as_u64().unwrap() as u32;
    let make = || scenario_provider(&sc["source"], rps, corpus_cache());
    let b = make().refresh_times();
    assert!(missing_from < b.len());
    let want: Vec<NativeDataset> = b[..missing_from]
        .iter()
        .map(|&t| fresh_refresh(make, ts(t as i64)))
        .collect();

    for depth in usizes(&sc["depth"]) {
        for workers in usizes(&sc["workers"]) {
            let ctx = format!("depth={depth} workers={workers}");

            // (1) CacheMiss at take(missing_from) and not before, although the
            // workers reach it early; then the order is over.
            let mut g = error_group(&sc, corpus_cache(), depth, workers);
            g.start(Order::Forward, None).unwrap();
            for (k, w) in want.iter().enumerate() {
                if k + depth == missing_from {
                    // The failing slot is admitted now: give the workers time to
                    // reach it (a CacheMiss is immediate). The takes before it
                    // must still succeed.
                    std::thread::sleep(StdDuration::from_millis(100));
                }
                let ep = g
                    .take(k)
                    .unwrap_or_else(|e| panic!("{ctx}: take({k}) must succeed: {e}"));
                assert_ds_bitwise(&format!("{ctx} take({k})"), &ep.get("Q").unwrap().data, w);
            }
            wait_ready(&g, missing_from); // stored, not raised
            let err = g.take(missing_from).unwrap_err();
            assert!(
                err.is_cache_miss(),
                "{ctx}: the cache's own CacheMiss, not a wrapper: {err:?}"
            );
            let again = g.take(missing_from + 1).unwrap_err();
            match again {
                Error::Prefetch { detail } => assert!(detail.contains("start"), "{ctx}: {detail}"),
                other => panic!("{ctx}: expected the start-again error, got {other:?}"),
            }

            // (2) Take `cancel_after` epochs, cancel, start(forward, from) — the
            // rest is the uninterrupted sequence.
            let mut g = error_group(&sc, corpus_cache(), depth, workers);
            g.start(Order::Forward, None).unwrap();
            for (k, w) in want.iter().enumerate().take(cancel_after) {
                assert_ds_bitwise(
                    &format!("{ctx} pre-cancel {k}"),
                    &g.take(k).unwrap().get("Q").unwrap().data,
                    w,
                );
            }
            g.cancel(false);
            assert!(
                matches!(g.take(cancel_after), Err(Error::Prefetch { .. })),
                "{ctx}: after cancel"
            );
            g.start(Order::Forward, Some(cancel_after)).unwrap();
            for (k, w) in want.iter().enumerate().skip(cancel_after) {
                let ep = g.take(k).unwrap();
                let s = ep.get("Q").unwrap();
                assert!(s.changed, "{ctx}: every hourly epoch changes");
                assert_ds_bitwise(&format!("{ctx} resumed {k}"), &s.data, w);
            }
            assert!(
                g.take(missing_from).unwrap_err().is_cache_miss(),
                "{ctx}: resumed order"
            );
        }
    }

    // (3) A COLD cache raises CacheMiss at take(0) — start does not raise it.
    let dir = tempfile::tempdir().unwrap();
    let cold = Arc::new(
        Cache::builder()
            .data_dir(dir.path())
            .offline(true)
            .build()
            .unwrap(),
    );
    let mut g = error_group(&sc, cold, 2, 1);
    g.start(Order::Forward, None)
        .expect("start never raises a decode error");
    wait_ready(&g, 0);
    assert!(g.take(0).unwrap_err().is_cache_miss());
}

/// The hook really does reorder completions: four workers start four epochs
/// at once with latencies that DECREASE with the epoch, so they finish newest
/// first (logged by the transform, which runs on the worker right after the
/// decode, identifying the epoch by its data) — and the delivered sequence is
/// still the declared one.
#[test]
fn completion_order_varies_but_delivery_does_not() {
    let sc = read_json("scenarios/prefetch-equivalence.json");
    let src = &sc["sources"]["hourly"];
    let p = scenario_provider(src, 1, corpus_cache());
    let b = p.refresh_times();
    let by_epoch: Vec<_> = b
        .iter()
        .map(|&t| {
            p.sample_at(ts(t as i64)).unwrap().variables["Q"]
                .data
                .clone()
        })
        .collect();
    let finished = Arc::new(std::sync::Mutex::new(Vec::new()));
    let log = finished.clone();
    let b0 = b[0];
    let mut g = PrefetchGroup::builder()
        .member("Q1", Arc::new(p))
        .depth(4)
        .workers(4)
        .fetch_ahead(0)
        .decode_latency(move |_, anchor| {
            let k = ((anchor - b0) / 3600.0) as u64;
            StdDuration::from_millis(40 * (3 - k % 4))
        })
        .transform(move |_, ds: NativeDataset| {
            let k = by_epoch
                .iter()
                .position(|d| *d == ds.variables["Q"].data)
                .unwrap();
            log.lock().unwrap().push(k);
            ds
        })
        .build()
        .unwrap();
    g.start(Order::Forward, None).unwrap();
    for (k, &t) in b.iter().enumerate() {
        assert_eq!(g.take(k).unwrap().get("Q1").unwrap().anchor, t);
    }
    let order = finished.lock().unwrap().clone();
    assert_eq!(order.len(), b.len());
    assert_ne!(
        order[..4],
        [0, 1, 2, 3],
        "the first four decodes finish out of epoch order: {order:?}"
    );
}
