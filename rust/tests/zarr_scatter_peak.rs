//! The Zarr reader scatters each decoded chunk into the output **as it is
//! fetched** rather than collecting every decoded chunk first (see the peak-memory
//! note in `src/format/zarr.rs`). This test pins the two properties that inversion
//! must not disturb, end to end over a real (synthetic, offline) MULTI-CHUNK
//! sharded Zarr v3 store:
//!
//!   1. **Values.** The decoded array equals the exact `f64` values the previous
//!      collect-then-scatter reader produced — asserted BIT for bit, not within a
//!      tolerance. Every fixture value is exactly representable, so the expected
//!      array can be written down independently of either implementation.
//!   2. **Laziness.** Only the chunk objects the selection intersects are fetched.
//!      Every OTHER chunk object in the store is overwritten with undecodable
//!      "poison" bytes before the read, so any over-fetch fails loudly instead of
//!      succeeding quietly. (`zarr_read_store.rs` proves the same for the committed
//!      corpus store; this proves it for the sharded v3 writer's layout, where one
//!      fetched object is a whole SHARD of inner chunks.)
//!
//! Everything here is offline: the store is written to a temp dir and read back
//! over `file://`.

// Native-only: this tier drives the cache, the transports and the format
// readers, none of which exist on wasm32 (see the crate's module docs).
#![cfg(not(target_arch = "wasm32"))]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use earthsciio::{
    write_zarr_v3, ArrayData, AxisSelect, Cache, DataLoader, OutputSchema, Provider, Selection,
    WriteCoord, WriteVar,
};
use serde_json::{Map, Value};

// dims 4 x 6 x 5; inner (read) chunk 1 x 3 x 5; shard (outer, fetched) 2 x 3 x 5
// => an outer chunk grid of 2 x 2 x 1 = FOUR fetchable chunk objects, each holding
// two inner chunks. Multi-chunk on two axes is what makes the scatter interesting.
const NT: usize = 4;
const NY: usize = 6;
const NX: usize = 5;

/// `conc[t, y, x] = t*100 + y*10 + x` — small integers, exactly representable in
/// both `f32` and `f64`, so "bit-identical" is a statement about the scatter and
/// not about float rounding.
fn value(t: usize, y: usize, x: usize) -> f64 {
    (t * 100 + y * 10 + x) as f64
}

fn attrs(pairs: &[(&str, Value)]) -> Map<String, Value> {
    pairs.iter().map(|(k, v)| (k.to_string(), v.clone())).collect()
}

/// Write the multi-shard fixture store and return its path.
fn write_fixture(store_dir: &Path) {
    let mut conc = Vec::with_capacity(NT * NY * NX);
    for t in 0..NT {
        for y in 0..NY {
            for x in 0..NX {
                conc.push(value(t, y, x));
            }
        }
    }
    let chunk_shape: BTreeMap<String, usize> =
        [("time", 1), ("y", 3), ("x", NX)].iter().map(|(k, v)| (k.to_string(), *v)).collect();
    let shard_shape: BTreeMap<String, usize> =
        [("time", 2), ("y", 3), ("x", NX)].iter().map(|(k, v)| (k.to_string(), *v)).collect();

    let schema = OutputSchema {
        dims: vec![("time".into(), NT), ("y".into(), NY), ("x".into(), NX)],
        time_dim: "time".into(),
        chunk_shape,
        shard_shape,
        coords: vec![
            WriteCoord {
                name: "time".into(),
                values: (0..NT).map(|t| t as f64).collect(),
                attrs: attrs(&[("units", Value::from("hours since 2020-01-01 00:00:00"))]),
            },
            WriteCoord {
                name: "y".into(),
                values: (0..NY).map(|y| y as f64).collect(),
                attrs: attrs(&[("units", Value::from("degrees_north"))]),
            },
            WriteCoord {
                name: "x".into(),
                values: (0..NX).map(|x| x as f64).collect(),
                attrs: attrs(&[("units", Value::from("degrees_east"))]),
            },
        ],
        vars: vec![WriteVar {
            name: "conc".into(),
            dims: vec!["time".into(), "y".into(), "x".into()],
            attrs: attrs(&[("units", Value::from("ug/m3"))]),
            data: conc,
        }],
        group_attrs: attrs(&[("title", Value::from("scatter-peak fixture"))]),
        profile: "diagnostic".into(),
    };
    write_zarr_v3(store_dir, &schema).expect("write multi-shard zarr v3 store");
}

/// Every chunk OBJECT of array `var`, as `(outer chunk id, path)`.
///
/// The chunk key encoding is the writer's business (`c/1/0/0` for v3 default),
/// so the id is recovered from the path rather than assumed: strip the array
/// directory and any `c` prefix component, then read the remaining components as
/// the chunk coordinate (splitting on `.` too, for a v2-style `1.0.0` key).
fn chunk_objects(store_dir: &Path, var: &str) -> Vec<(Vec<usize>, PathBuf)> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let entry = entry.unwrap();
            if entry.file_type().unwrap().is_dir() {
                walk(&entry.path(), out);
            } else {
                out.push(entry.path());
            }
        }
    }
    let root = store_dir.join(var);
    let mut files = Vec::new();
    walk(&root, &mut files);

    let mut out = Vec::new();
    for path in files {
        let rel = path.strip_prefix(&root).unwrap();
        let comps: Vec<String> =
            rel.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
        // Metadata objects are not chunks.
        if comps.iter().any(|c| c.as_str() == "zarr.json" || c.starts_with('.')) {
            continue;
        }
        let id: Vec<usize> = comps
            .iter()
            .filter(|c| c.as_str() != "c")
            .flat_map(|c| c.split('.').map(str::to_string).collect::<Vec<_>>())
            .map(|c| c.parse::<usize>().expect("chunk key component is a number"))
            .collect();
        out.push((id, path));
    }
    out.sort();
    out
}

/// Overwrite `path` with bytes no codec pipeline can decode.
fn poison(path: &Path) {
    std::fs::write(path, b"\x00POISON-not-a-shard\xff").unwrap();
}

fn read_conc(store_dir: &Path, cache_dir: &Path, sel: Selection) -> earthsciio::Result<Vec<f64>> {
    let base_url = format!("file://{}", store_dir.display());
    let cache = Arc::new(Cache::builder().data_dir(cache_dir).build().expect("cache"));
    let loader = DataLoader::new("scatter-peak", "zarr", &base_url)
        .variables(["conc"])
        .select(sel);
    let mut provider = Provider::new(loader, cache, None).expect("provider");
    let buffers = provider.materialize()?;
    let ArrayData::F64(v) = &buffers["conc"].data else {
        panic!("expected F64 data");
    };
    Ok(v.clone())
}

#[test]
fn scatter_as_you_go_decodes_exactly_and_stays_lazy() {
    let scratch = tempfile::tempdir().unwrap();
    let store_dir = scratch.path().join("multi.zarr");
    write_fixture(&store_dir);

    // The writer must actually have produced a MULTI-chunk store, or the test
    // proves nothing about scattering across chunks.
    let objects = chunk_objects(&store_dir, "conc");
    let ids: Vec<Vec<usize>> = objects.iter().map(|(id, _)| id.clone()).collect();
    assert_eq!(
        ids,
        vec![vec![0, 0, 0], vec![0, 1, 0], vec![1, 0, 0], vec![1, 1, 0]],
        "expected a 2x2x1 outer chunk grid, got {ids:?}"
    );

    // select time=[3], y=[4,1] (PERMUTED, so a sorted read would fail), x=all.
    // time 3 -> outer chunk 1; y 4 -> chunk 1, y 1 -> chunk 0; x -> chunk 0.
    // Needed: (1,0,0) and (1,1,0). Poison the other two.
    let needed = [vec![1usize, 0, 0], vec![1usize, 1, 0]];
    let mut poisoned = 0;
    for (id, path) in &objects {
        if !needed.contains(id) {
            poison(path);
            poisoned += 1;
        }
    }
    assert_eq!(poisoned, 2, "the laziness check needs unselected chunks to poison");

    let sel = Selection::Orthogonal(vec![
        AxisSelect::Indices(vec![3]),
        AxisSelect::Indices(vec![4, 1]),
        AxisSelect::All,
    ]);
    let got = read_conc(&store_dir, &scratch.path().join("cache"), sel)
        .expect("a lazy read must not touch the poisoned chunks");

    // Values written down independently of the reader, in the SELECTION's order.
    let mut want = Vec::new();
    for &y in &[4usize, 1] {
        for x in 0..NX {
            want.push(value(3, y, x));
        }
    }
    assert_eq!(got.len(), want.len());
    for (i, (g, w)) in got.iter().zip(&want).enumerate() {
        // Bit equality, not a tolerance: the scatter moves values, never computes.
        assert_eq!(g.to_bits(), w.to_bits(), "element {i}: {g} vs {w}");
    }
}

#[test]
fn poisoned_chunk_is_genuinely_undecodable() {
    // Control for the test above: a selection that DOES intersect a poisoned chunk
    // must fail. Without this, "no error" could just mean the poison was harmless.
    let scratch = tempfile::tempdir().unwrap();
    let store_dir = scratch.path().join("multi.zarr");
    write_fixture(&store_dir);

    for (id, path) in chunk_objects(&store_dir, "conc") {
        if id == vec![1, 1, 0] {
            poison(&path);
        }
    }
    let sel = Selection::Orthogonal(vec![
        AxisSelect::Indices(vec![3]),
        AxisSelect::Indices(vec![4]), // y 4 -> the poisoned chunk
        AxisSelect::All,
    ]);
    assert!(
        read_conc(&store_dir, &scratch.path().join("cache"), sel).is_err(),
        "reading a poisoned chunk must error"
    );
}

#[test]
fn whole_array_read_reassembles_every_chunk() {
    // The other end of the range: no selection at all, so EVERY chunk object is
    // fetched and scattered. Each of the 4 shards contributes a disjoint block of
    // the output; a scatter that mis-placed one would show up immediately.
    let scratch = tempfile::tempdir().unwrap();
    let store_dir = scratch.path().join("multi.zarr");
    write_fixture(&store_dir);

    let got = read_conc(&store_dir, &scratch.path().join("cache"), Selection::All)
        .expect("whole-array read");

    let mut want = Vec::with_capacity(NT * NY * NX);
    for t in 0..NT {
        for y in 0..NY {
            for x in 0..NX {
                want.push(value(t, y, x));
            }
        }
    }
    assert_eq!(got.len(), want.len());
    for (i, (g, w)) in got.iter().zip(&want).enumerate() {
        assert_eq!(g.to_bits(), w.to_bits(), "element {i}: {g} vs {w}");
    }
}
