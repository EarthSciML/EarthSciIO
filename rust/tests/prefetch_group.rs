//! The `PrefetchGroup` contract beyond the scenarios (`spec/prefetch.md` §3):
//! what construction refuses, every misuse of `take`/`start`, `ready`, a
//! `transform`, a panicking transform, `close`, and `waited_s` falling to about
//! zero once synthetic compute exceeds decode time.

#![cfg(not(target_arch = "wasm32"))]

mod common;

use std::sync::Arc;
use std::time::Duration as StdDuration;

use common::{corpus_cache, read_json, scenario_provider, ts};
use earthsciio::{
    DataSource, DimWindow, Error, NativeDataset, Order, PrefetchGroup, Provider, SourceTemporal,
};
use time::Duration;

fn hourly() -> Arc<Provider> {
    let sc = read_json("scenarios/prefetch-equivalence.json");
    Arc::new(scenario_provider(&sc["sources"]["hourly"], 1, corpus_cache()))
}

fn detail(r: earthsciio::Result<impl std::fmt::Debug>) -> String {
    match r {
        Err(Error::Prefetch { detail }) => detail,
        other => panic!("expected a prefetch error, got {other:?}"),
    }
}

fn group() -> PrefetchGroup {
    PrefetchGroup::builder().member("Q", hourly()).build().unwrap()
}

#[test]
fn construction_refuses_what_it_cannot_schedule() {
    assert!(detail(PrefetchGroup::builder().build()).contains("at least one member"));
    assert!(detail(PrefetchGroup::builder().member("Q", hourly()).depth(0).build()).contains("depth"));
    assert!(detail(PrefetchGroup::builder().member("Q", hourly()).workers(0).build()).contains("workers"));
    let twice = PrefetchGroup::builder().member("Q", hourly()).member("Q", hourly()).build();
    assert!(detail(twice).contains("twice"));

    let url = "https://data.earthsci.dev/geosfp/mini/GEOSFP.20240101.A3mixed.nc";
    let constant = Provider::new(DataSource::new("geosfp", "netcdf", url), corpus_cache(), None).unwrap();
    let msg = detail(PrefetchGroup::builder().member("C", Arc::new(constant)).build());
    assert!(msg.contains("CONST"), "{msg}");

    let open_ended = SourceTemporal::new(ts(1_704_067_200), Duration::hours(1), Duration::hours(4));
    let loader = DataSource::new("h", "netcdf", "https://x/[year].nc").temporal(open_ended);
    let unbounded = Provider::new(loader, corpus_cache(), None).unwrap();
    let msg = detail(PrefetchGroup::builder().member("U", Arc::new(unbounded)).build());
    assert!(msg.contains("schedule"), "{msg}");

    // A window a member cannot take is refused at construction, with
    // with_window's own error.
    let windowed = Arc::new(hourly().with_window(DimWindow::new().range("lat", 0, 1)).unwrap());
    let r = PrefetchGroup::builder()
        .member("W", windowed)
        .windows([DimWindow::new().range("lon", 0, 1)])
        .build();
    assert!(matches!(r, Err(Error::Format { .. })), "a baked select does not compose");
}

#[test]
fn take_refuses_every_epoch_but_the_next_and_consumes_nothing() {
    let mut g = group();
    let n = g.boundaries().len();
    assert!(detail(g.take(0)).contains("before any start"));
    assert!(!g.ready(0));

    g.start(Order::Forward, None).unwrap();
    let msg = detail(g.take(3));
    assert!(msg.contains("next epoch in the current order is 0"), "{msg}");
    assert!(!g.ready(3), "ready reports only the next entry");
    assert_eq!(g.take(0).unwrap().epoch(), 0, "the refused take consumed nothing");
    for k in 1..n {
        g.take(k).unwrap();
    }
    assert!(detail(g.take(0)).contains("exhausted"));

    g.start(Order::Reverse, None).unwrap();
    assert!(g.take(n - 1).unwrap().get("Q").unwrap().changed);
    g.cancel(false);
    assert!(detail(g.take(n - 2)).contains("after cancel"));
    assert!(!g.ready(n - 2));
    // Reusable after start.
    g.start(Order::Explicit(vec![4, 4, 2]), None).unwrap();
    let first = g.take(4).unwrap();
    let again = g.take(4).unwrap();
    assert!(first.get("Q").unwrap().changed && !again.get("Q").unwrap().changed);
    assert!(Arc::ptr_eq(&first.get("Q").unwrap().data, &again.get("Q").unwrap().data), "one sample, shared");
    assert_eq!(g.take(2).unwrap().epoch(), 2);
}

#[test]
fn start_validates_before_dropping_the_running_order() {
    let mut g = group();
    let n = g.boundaries().len();
    g.start(Order::Forward, None).unwrap();
    g.take(0).unwrap();
    assert!(detail(g.start(Order::Explicit(vec![]), None)).contains("at least one"));
    assert!(detail(g.start(Order::Explicit(vec![1, n]), None)).contains("out of range"));
    assert!(detail(g.start(Order::Explicit(vec![1, 2]), Some(5))).contains("not in the order"));
    // None of those disturbed the forward order.
    assert_eq!(g.take(1).unwrap().epoch(), 1);

    // `from` trims to the first entry equal to it.
    g.start(Order::Explicit(vec![3, 1, 4, 1, 5]), Some(1)).unwrap();
    for k in [1, 4, 1, 5] {
        assert_eq!(g.take(k).unwrap().epoch(), k);
    }
}

#[test]
fn a_transform_runs_on_the_worker_and_its_panic_is_raised_at_take() {
    let mut g = PrefetchGroup::builder()
        .member("Q", hourly())
        .member("A", hourly())
        .transform(|key, ds: NativeDataset| (key.to_string(), ds.variables["Q"].shape.clone()))
        .build()
        .unwrap();
    g.start(Order::Forward, Some(2)).unwrap();
    let ep = g.take(2).unwrap();
    let keys: Vec<&str> = ep.iter().map(|(k, _)| k).collect();
    assert_eq!(keys, ["A", "Q"], "byte-wise key order");
    assert_eq!(*ep.get("Q").unwrap().data, ("Q".to_string(), vec![2, 3]));

    let mut g = PrefetchGroup::builder()
        .member("Q", hourly())
        .transform(|_, ds: NativeDataset| {
            if ds.variables["Q"].data.len() == 6 {
                panic!("transform refused");
            }
            ds
        })
        .build()
        .unwrap();
    g.start(Order::Forward, None).unwrap();
    let msg = detail(g.take(0));
    assert!(msg.contains("panicked") && msg.contains("transform refused"), "{msg}");
    assert!(detail(g.take(1)).contains("start again"));
}

#[test]
fn close_is_final_and_drop_joins() {
    let mut g = group();
    g.start(Order::Forward, None).unwrap();
    g.take(0).unwrap();
    g.close();
    g.close(); // idempotent
    assert!(detail(g.start(Order::Forward, None)).contains("closed"));
    assert!(detail(g.take(1)).contains("closed"));
    g.cancel(true);

    // Dropped mid-order with work queued and in flight: cancels and joins.
    let mut g = PrefetchGroup::builder()
        .member("Q", hourly())
        .depth(3)
        .workers(2)
        .decode_latency(|_, _| StdDuration::from_millis(20))
        .build()
        .unwrap();
    g.start(Order::Forward, None).unwrap();
    drop(g);
}

/// §6: `stats().waited_s` falls to about zero when synthetic compute exceeds
/// decode time. Decode is pinned at 30 ms per epoch by the latency hook; a
/// consumer that takes back to back waits for nearly every one, while one that
/// "integrates" for 90 ms per epoch waits only for the first.
#[test]
fn waited_s_falls_to_about_zero_when_compute_exceeds_decode() {
    let run = |compute: StdDuration| {
        let mut g = PrefetchGroup::builder()
            .member("Q", hourly())
            .depth(2)
            .decode_latency(|_, _| StdDuration::from_millis(30))
            .build()
            .unwrap();
        g.start(Order::Forward, None).unwrap();
        for k in 0..6 {
            g.take(k).unwrap();
            std::thread::sleep(compute);
        }
        g.stats()
    };
    let busy = run(StdDuration::ZERO);
    let overlapped = run(StdDuration::from_millis(90));
    assert!(busy.waited_s > 0.12, "back-to-back takes wait on every decode: {busy:?}");
    assert!(
        overlapped.waited_s < 0.06,
        "only the first epoch should be waited for: {overlapped:?}"
    );
    assert!(overlapped.waited_s < busy.waited_s / 3.0);
    assert!(overlapped.decode_s >= 0.18 && overlapped.overlap);
}
