//! The streaming writer handle (`spec/writer.md`, RFC U12): regions and the
//! commit protocol, indexed records, statics, blobs, dtypes and resume, over an
//! in-memory store. The cross-language corpus (`conformance/write_spec_v2.json`)
//! is run by `examples/conformance_write_v2.rs`; these tests pin the same rules
//! natively.

use std::collections::BTreeMap;
use std::sync::Arc;

use earthsciio::{
    read_blob, read_output_manifest, read_record, read_static, BlobScope, CommitToken, Error,
    RecordArrays, Region, Role, StreamDType, StreamSchema, StreamVar, TimeMode, TypedVec,
    WriteCoord, ZarrStreamWriter,
};
use serde_json::Map;
use zarrs::storage::store::MemoryStore;
use zarrs::storage::{ListableStorageTraits, ReadableStorageTraits};

const NLAT: usize = 6;
const NLON: usize = 5;

fn var(name: &str, dims: &[&str], dtype: StreamDType) -> StreamVar {
    StreamVar {
        name: name.into(),
        dims: dims.iter().map(|d| d.to_string()).collect(),
        dtype,
        attrs: Map::new(),
    }
}

fn schema(time_mode: TimeMode) -> StreamSchema {
    let m = |t, la, lo| {
        BTreeMap::from([
            ("time".to_string(), t),
            ("lat".to_string(), la),
            ("lon".to_string(), lo),
        ])
    };
    StreamSchema {
        dims: vec![
            ("time".into(), 0),
            ("lat".into(), NLAT),
            ("lon".into(), NLON),
        ],
        time_dim: "time".into(),
        chunk_shape: m(1, 1, 2),
        shard_shape: m(2, 2, 4),
        coords: vec![
            WriteCoord {
                name: "lat".into(),
                values: (0..NLAT).map(|i| i as f64).collect(),
                attrs: Map::new(),
            },
            WriteCoord {
                name: "lon".into(),
                values: (0..NLON).map(|i| i as f64).collect(),
                attrs: Map::new(),
            },
            WriteCoord {
                name: "time".into(),
                values: vec![],
                attrs: Map::new(),
            },
        ],
        vars: vec![
            var("temperature", &["time", "lat", "lon"], StreamDType::F64),
            var("count", &["time", "lat", "lon"], StreamDType::U8),
            var("global_mean", &["time"], StreamDType::F32),
        ],
        statics: vec![],
        time_mode,
        group_attrs: Map::new(),
        profile: "diagnostic".into(),
    }
}

/// `value(r, lat, lon)` for a writer's region (C order).
fn field(r: u64, lat: std::ops::Range<usize>, lon: std::ops::Range<usize>) -> Vec<f64> {
    let mut v = Vec::new();
    for i in lat {
        for j in lon.clone() {
            v.push(1000.0 * r as f64 + 10.0 * i as f64 + j as f64 + 0.5);
        }
    }
    v
}

struct Rec {
    temperature: Vec<f64>,
    count: Vec<u8>,
    global_mean: Vec<f32>,
}

fn rec(r: u64, region: &Region) -> Rec {
    let get = |d: &str, n: usize| {
        region
            .0
            .iter()
            .find(|(x, _)| x == d)
            .map(|(_, r)| r.clone())
            .unwrap_or(0..n)
    };
    let t = field(r, get("lat", NLAT), get("lon", NLON));
    Rec {
        count: t.iter().map(|x| (*x as u64 % 251) as u8).collect(),
        temperature: t,
        global_mean: vec![r as f32 + 0.25],
    }
}

fn arrays<'a>(r: &'a Rec, with_replicated: bool) -> RecordArrays<'a> {
    let mut a = RecordArrays::new();
    a.insert("temperature".into(), (&r.temperature).into());
    a.insert("count".into(), (&r.count).into());
    if with_replicated {
        a.insert("global_mean".into(), (&r.global_mean).into());
    }
    a
}

fn quad() -> Vec<(Role, Region)> {
    let r = |la: std::ops::Range<usize>, lo: std::ops::Range<usize>| {
        Region::new([("lat", la), ("lon", lo)])
    };
    vec![
        (Role::Owner { n_writers: 4 }, r(0..4, 0..4)),
        (Role::Contributor { id: 1 }, r(0..4, 4..5)),
        (Role::Contributor { id: 2 }, r(4..6, 0..4)),
        (Role::Contributor { id: 3 }, r(4..6, 4..5)),
    ]
}

/// Every shared object (`zarr.json`s and the manifest) and its bytes.
fn shared_objects(store: &MemoryStore) -> BTreeMap<String, Vec<u8>> {
    store
        .list()
        .unwrap()
        .into_iter()
        .filter(|k| k.as_str().ends_with("zarr.json") || k.as_str() == "output_manifest.json")
        .map(|k| {
            (
                k.as_str().to_string(),
                store.get(&k).unwrap().unwrap().to_vec(),
            )
        })
        .collect()
}

#[test]
fn region_tiling_equals_a_single_writer_and_contributors_write_no_metadata() {
    let single = Arc::new(MemoryStore::new());
    let mut w = ZarrStreamWriter::create(
        single.clone(),
        "mem://single",
        schema(TimeMode::Append),
        Role::Single,
        Region::whole(),
    )
    .unwrap();
    for r in 0..5 {
        let x = rec(r, &Region::whole());
        w.write_record(r as f64 * 3600.0, &arrays(&x, true))
            .unwrap();
        if r == 2 {
            w.flush().unwrap();
        }
    }
    let m = w.close(&[]).unwrap().unwrap();
    assert_eq!(m["schema"], "earthsciio/output-manifest/v1");

    let tiled = Arc::new(MemoryStore::new());
    let mut ws: Vec<ZarrStreamWriter<MemoryStore>> = Vec::new();
    for (role, region) in quad() {
        ws.push(match role {
            Role::Owner { .. } => ZarrStreamWriter::create(
                tiled.clone(),
                "mem://tiled",
                schema(TimeMode::Append),
                role,
                region,
            )
            .unwrap(),
            _ => ZarrStreamWriter::join(tiled.clone(), "mem://tiled", role, region).unwrap(),
        });
    }
    let before = shared_objects(&tiled);
    let mut toks = Vec::new();
    for r in 0..3 {
        for (k, w) in ws.iter_mut().enumerate() {
            let x = rec(r, &w.region());
            w.write_record(r as f64 * 3600.0, &arrays(&x, k == 0))
                .unwrap();
        }
    }
    for w in &mut ws[1..] {
        toks.push(CommitToken::from_json(&w.flush().unwrap().to_json()).unwrap());
    }
    // Contributors have flushed: no shared object changed.
    assert_eq!(before, shared_objects(&tiled));
    let mut all = vec![ws[0].flush().unwrap()];
    all.extend(toks);
    let receipt = ws[0].commit(&all).unwrap();
    assert_eq!((receipt.commit_seq, receipt.n_committed), (1, 3));
    for r in 3..5 {
        for (k, w) in ws.iter_mut().enumerate() {
            let x = rec(r, &w.region());
            w.write_record(r as f64 * 3600.0, &arrays(&x, k == 0))
                .unwrap();
        }
    }
    let mut owner = None;
    let mut toks = Vec::new();
    for (k, mut w) in ws.into_iter().enumerate() {
        if k == 0 {
            owner = Some(w);
        } else {
            toks.push(w.flush().unwrap());
            assert!(w.close(&[]).unwrap().is_none());
        }
    }
    let m = owner.unwrap().close(&toks).unwrap().unwrap();
    assert_eq!(m["schema"], "earthsciio/output-manifest/v2");
    assert_eq!(m["commit_seq"], 2);
    assert_eq!(m["n_records"], 5);

    for r in 0..5u64 {
        for v in ["temperature", "count", "global_mean"] {
            let a = read_record(single.clone(), v, r, &Region::whole()).unwrap();
            let b = read_record(tiled.clone(), v, r, &Region::whole()).unwrap();
            assert_eq!(a, b, "{v}[{r}]");
        }
    }
    // A region read is the region of the whole.
    let sub = read_record(
        tiled.clone(),
        "temperature",
        4,
        &Region::new([("lat", 2..4)]),
    )
    .unwrap();
    assert_eq!(sub, TypedVec::F64(field(4, 2..4, 0..NLON)));
}

#[test]
fn misaligned_regions_are_refused_at_open() {
    let store = Arc::new(MemoryStore::new());
    let e = ZarrStreamWriter::create(
        store.clone(),
        "m",
        schema(TimeMode::Append),
        Role::Owner { n_writers: 2 },
        Region::new([("lat", 0..3)]),
    )
    .unwrap_err();
    assert!(matches!(e, Error::RegionNotShardAligned { .. }), "{e}");
    assert!(
        store.list().unwrap().is_empty(),
        "no I/O before the alignment check"
    );
    ZarrStreamWriter::create(
        store.clone(),
        "m",
        schema(TimeMode::Append),
        Role::Owner { n_writers: 2 },
        Region::new([("lat", 0..2)]),
    )
    .unwrap();
    // lon [4, 5) ends at the array end, which is allowed; [2, 5) starts mid-shard.
    ZarrStreamWriter::join(
        store.clone(),
        "m",
        Role::Contributor { id: 1 },
        Region::new([("lon", 4..5)]),
    )
    .unwrap();
    let e = ZarrStreamWriter::join(
        store,
        "m",
        Role::Contributor { id: 1 },
        Region::new([("lon", 2..5)]),
    )
    .unwrap_err();
    assert!(matches!(e, Error::RegionNotShardAligned { .. }), "{e}");
}

#[test]
fn gaps_and_overlaps_are_commit_errors_that_commit_nothing() {
    for (r1, what) in [(4..6, "gap"), (0..6, "overlap")] {
        let store = Arc::new(MemoryStore::new());
        let mut o = ZarrStreamWriter::create(
            store.clone(),
            "m",
            schema(TimeMode::Append),
            Role::Owner { n_writers: 2 },
            Region::new([("lat", 0..2)]),
        )
        .unwrap();
        let mut c = ZarrStreamWriter::join(
            store.clone(),
            "m",
            Role::Contributor { id: 1 },
            Region::new([("lat", r1)]),
        )
        .unwrap();
        let (x0, x1) = (rec(0, &o.region()), rec(0, &c.region()));
        o.write_record(0.0, &arrays(&x0, true)).unwrap();
        c.write_record(0.0, &arrays(&x1, false)).unwrap();
        let toks = [o.flush().unwrap(), c.flush().unwrap()];
        let before = shared_objects(&store);
        let e = o.commit(&toks).unwrap_err();
        assert!(
            matches!(&e, Error::Commit { detail } if detail.contains(what)),
            "{e}"
        );
        assert_eq!(before, shared_objects(&store));
        assert_eq!(
            read_output_manifest(store.as_ref()).unwrap()["commit_seq"],
            0
        );
        // A missing token is refused too.
        assert!(matches!(
            o.commit(&toks[..1]).unwrap_err(),
            Error::Commit { .. }
        ));
    }
}

#[test]
fn contributors_may_not_write_replicated_variables() {
    let store = Arc::new(MemoryStore::new());
    let _o = ZarrStreamWriter::create(
        store.clone(),
        "m",
        schema(TimeMode::Append),
        Role::Owner { n_writers: 2 },
        Region::new([("lat", 0..2)]),
    )
    .unwrap();
    let mut c = ZarrStreamWriter::join(
        store,
        "m",
        Role::Contributor { id: 1 },
        Region::new([("lat", 2..6)]),
    )
    .unwrap();
    let x = rec(0, &c.region());
    let e = c.write_record(0.0, &arrays(&x, true)).unwrap_err();
    assert!(e.to_string().contains("replicated"), "{e}");
}

#[test]
fn indexed_reverse_equals_forward_and_rewrites_need_invalidate() {
    let n = 5;
    let times: Vec<f64> = (0..n).map(|i| i as f64 * 60.0).collect();
    let mode = TimeMode::Indexed {
        n_records: n,
        times: times.clone(),
    };
    let fwd = Arc::new(MemoryStore::new());
    let mut w = ZarrStreamWriter::create(
        fwd.clone(),
        "f",
        schema(mode.clone()),
        Role::Single,
        Region::whole(),
    )
    .unwrap();
    for i in 0..n as u64 {
        let x = rec(i, &Region::whole());
        w.write_record_at(i, times[i as usize], &arrays(&x, true))
            .unwrap();
    }
    w.close(&[]).unwrap();
    let rev = Arc::new(MemoryStore::new());
    let mut w = ZarrStreamWriter::create(
        rev.clone(),
        "r",
        schema(mode),
        Role::Single,
        Region::whole(),
    )
    .unwrap();
    for i in (0..n as u64).rev() {
        let x = rec(i, &Region::whole());
        w.write_record_at(i, times[i as usize], &arrays(&x, true))
            .unwrap();
        if i == 3 {
            w.flush().unwrap(); // commits 4 and 3 first
        }
    }
    let x = rec(9, &Region::whole());
    assert!(
        w.write_record_at(0, 1.0, &arrays(&x, true)).is_err(),
        "t must equal times[i]"
    );
    w.flush().unwrap();
    assert!(
        w.write_record_at(0, 0.0, &arrays(&x, true)).is_err(),
        "committed record"
    );
    w.invalidate(&[0]).unwrap();
    assert!(
        read_record(rev.clone(), "temperature", 0, &Region::whole()).is_err(),
        "uncommitted is unreadable"
    );
    w.write_record_at(0, 0.0, &arrays(&x, true)).unwrap();
    let m = w.close(&[]).unwrap().unwrap();
    assert_eq!(m["records"]["entries"][0]["generation"], 2);
    assert_eq!(m["records"]["committed"], serde_json::json!([[0, 5]]));
    for i in 1..n as u64 {
        assert_eq!(
            read_record(fwd.clone(), "temperature", i, &Region::whole()).unwrap(),
            read_record(rev.clone(), "temperature", i, &Region::whole()).unwrap()
        );
    }
    assert_eq!(
        read_record(rev, "temperature", 0, &Region::whole()).unwrap(),
        TypedVec::F64(field(9, 0..NLAT, 0..NLON))
    );
}

#[test]
fn resume_truncates_and_continues_a_partial_shard() {
    let store = Arc::new(MemoryStore::new());
    let mut w = ZarrStreamWriter::create(
        store.clone(),
        "m",
        schema(TimeMode::Append),
        Role::Single,
        Region::whole(),
    )
    .unwrap();
    for r in 0..5 {
        let x = rec(r, &Region::whole());
        w.write_record(r as f64, &arrays(&x, true)).unwrap();
    }
    w.flush().unwrap();
    drop(w); // preempted
    let mut w =
        ZarrStreamWriter::resume(store.clone(), "m", Role::Single, Region::whole(), Some(2.0))
            .unwrap();
    // records 0..=2 survive; record 2 sits in the open time shard [2, 4)
    let x = rec(77, &Region::whole());
    assert_eq!(w.write_record(3.0, &arrays(&x, true)).unwrap(), 3);
    let m = w.close(&[]).unwrap().unwrap();
    assert_eq!(m["schema"], "earthsciio/output-manifest/v1");
    assert_eq!(m["n_records"], 4);
    assert_eq!(
        read_record(store.clone(), "temperature", 2, &Region::whole()).unwrap(),
        TypedVec::F64(field(2, 0..NLAT, 0..NLON))
    );
    assert_eq!(
        read_record(store.clone(), "temperature", 3, &Region::whole()).unwrap(),
        TypedVec::F64(field(77, 0..NLAT, 0..NLON))
    );
    assert!(read_record(store, "temperature", 4, &Region::whole()).is_err());
}

#[test]
fn statics_blobs_and_dtypes_round_trip_bitwise() {
    let mut s = schema(TimeMode::Append);
    s.statics = vec![
        var("dJ_du0", &["lat", "lon"], StreamDType::F64),
        var("steps", &["lat", "lon"], StreamDType::I64),
        var("dJ_dlon", &["lon"], StreamDType::F32),
    ];
    let store = Arc::new(MemoryStore::new());
    let mut o = ZarrStreamWriter::create(
        store.clone(),
        "m",
        s,
        Role::Owner { n_writers: 2 },
        Region::new([("lat", 0..2)]),
    )
    .unwrap();
    let mut c = ZarrStreamWriter::join(
        store.clone(),
        "m",
        Role::Contributor { id: 1 },
        Region::new([("lat", 2..6)]),
    )
    .unwrap();
    let g = |lat: std::ops::Range<usize>| field(0, lat, 0..NLON);
    let i = |v: &[f64]| {
        v.iter()
            .map(|x| -(*x as i64) * 1_000_000_007)
            .collect::<Vec<i64>>()
    };
    o.write_static("dJ_du0", g(0..2).as_slice().into()).unwrap();
    c.write_static("dJ_du0", g(2..6).as_slice().into()).unwrap();
    o.write_static("steps", i(&g(0..2)).as_slice().into())
        .unwrap();
    c.write_static("steps", i(&g(2..6)).as_slice().into())
        .unwrap();
    let lon: Vec<f32> = vec![0.1, -2.5e-30, f32::MAX, 3.0, f32::MIN_POSITIVE];
    assert!(
        c.write_static("dJ_dlon", lon.as_slice().into()).is_err(),
        "replicated static"
    );
    o.write_static("dJ_dlon", lon.as_slice().into()).unwrap();
    c.write_blob(BlobScope::Static, "rng", &[1, 2, 3]).unwrap();
    assert!(c.write_blob(BlobScope::Static, "bad/key", &[]).is_err());
    let tok = c.flush().unwrap();
    c.close(&[]).unwrap();
    o.close(&[tok]).unwrap();
    assert_eq!(
        read_static(store.clone(), "dJ_du0", &Region::whole()).unwrap(),
        TypedVec::F64(g(0..NLAT))
    );
    assert_eq!(
        read_static(store.clone(), "steps", &Region::whole()).unwrap(),
        TypedVec::I64(i(&g(0..NLAT)))
    );
    assert_eq!(
        read_static(store.clone(), "dJ_dlon", &Region::whole()).unwrap(),
        TypedVec::F32(lon)
    );
    assert_eq!(
        read_blob(store.as_ref(), BlobScope::Static, 1, "rng").unwrap(),
        vec![1, 2, 3]
    );
    assert!(read_blob(store.as_ref(), BlobScope::Static, 0, "rng").is_err());
}

#[test]
fn a_v1_store_refuses_blobs_and_tokens_round_trip_json() {
    let store = Arc::new(MemoryStore::new());
    let mut w = ZarrStreamWriter::create(
        store,
        "m",
        schema(TimeMode::Append),
        Role::Single,
        Region::whole(),
    )
    .unwrap();
    assert!(w.write_blob(BlobScope::Static, "k", b"x").is_err());
    let x = rec(0, &Region::whole());
    w.write_record(0.5, &arrays(&x, true)).unwrap();
    let t = w.flush().unwrap();
    assert_eq!(t.records.len(), 1);
    assert_eq!(CommitToken::from_json(&t.to_json()).unwrap(), t);
    assert!(w.fingerprint().starts_with("sha256:"));
}
