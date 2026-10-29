//! Helpers shared by the `spec/prefetch.md` tests: the offline corpus cache, a
//! Provider built from a scenario's source block, a BITWISE dataset comparison,
//! and an independent by-name slicer for the "windowed read == full read sliced
//! by name" gate.
//!
//! The slicer here is written separately from the crate's own on purpose: a
//! window checked against the code that produced it proves nothing.

#![allow(dead_code)]

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::Arc;

use earthsciio::{
    ArrayData, AxisSelect, Cache, Coord, DataSource, NativeDataset, NativeField, Provider,
    Selection, SourceTemporal,
};
use serde_json::Value;
use time::{Duration, OffsetDateTime};

pub fn corpus_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../conformance/corpus")
}

pub fn read_json(rel: &str) -> Value {
    let path = corpus_dir().join(rel);
    let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    serde_json::from_slice(&bytes).unwrap_or_else(|e| panic!("parse {}: {e}", path.display()))
}

/// An offline, verifying cache over the committed corpus.
pub fn corpus_cache() -> Arc<Cache> {
    Arc::new(
        Cache::builder()
            .data_dir(corpus_dir().join("cache"))
            .offline(true)
            .verify_on_read(true)
            .build()
            .expect("offline cache over the corpus"),
    )
}

pub fn ts(seconds: i64) -> OffsetDateTime {
    OffsetDateTime::from_unix_timestamp(seconds).unwrap()
}

/// A DISCRETE provider over a scenario `source` block (Rust `url_template`).
pub fn scenario_provider(src: &Value, rps: u32, cache: Arc<Cache>) -> Provider {
    let s = |k: &str| src[k].as_i64().unwrap_or_else(|| panic!("source.{k}"));
    let temporal = SourceTemporal::new(
        ts(s("start_s")),
        Duration::seconds(s("frequency_s")),
        Duration::seconds(s("file_period_s")),
    )
    .end(ts(s("end_s")))
    .time_dim(src["time_dim"].as_str().unwrap())
    .records_per_sample(rps);
    let vars: Vec<String> = src["variables"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect();
    let loader = DataSource::new(
        "scenario",
        src["format"].as_str().unwrap(),
        src["url_template"]["rust"].as_str().unwrap(),
    )
    .variables(vars)
    .temporal(temporal);
    let p = Provider::new(loader, cache, None).expect("scenario provider");
    let want: Vec<f64> = src["refresh_times_s"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_f64().unwrap())
        .collect();
    assert_eq!(p.refresh_times(), want, "refresh_times_s of the scenario source");
    p
}

/// What a FRESH provider's `refresh(t)` hands back: its variables plus the
/// `coords()` it leaves — the right-hand side of every equivalence here.
pub fn fresh_refresh(make: impl Fn() -> Provider, t: OffsetDateTime) -> NativeDataset {
    let mut p = make();
    let variables = p
        .refresh(t)
        .expect("refresh")
        .expect("a fresh provider always refreshes");
    NativeDataset {
        variables,
        coords: p.coords().clone(),
    }
}

pub fn selections(v: &Value) -> Vec<Selection> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|w| Selection::from_json(w).expect("a scenario window"))
        .collect()
}

fn data_bits(d: &ArrayData) -> Vec<String> {
    // A canonical, bit-exact rendering: f64 by its bits (NaN for NaN, -0 kept).
    match d {
        ArrayData::F64(v) => v.iter().map(|x| format!("{:016x}", x.to_bits())).collect(),
        ArrayData::I64(v) => v.iter().map(|x| x.to_string()).collect(),
        ArrayData::I32(v) => v.iter().map(|x| x.to_string()).collect(),
        ArrayData::Str(v) => v.iter().map(|x| format!("{x:?}")).collect(),
        ArrayData::Bool(v) => v.iter().map(|x| x.to_string()).collect(),
    }
}

pub fn assert_field_bitwise(ctx: &str, got: &NativeField, want: &NativeField) {
    assert_eq!(got.dtype, want.dtype, "{ctx}: dtype");
    assert_eq!(got.dims, want.dims, "{ctx}: dims");
    assert_eq!(got.shape, want.shape, "{ctx}: shape");
    assert_eq!(
        got.fill_value.map(f64::to_bits),
        want.fill_value.map(f64::to_bits),
        "{ctx}: fill_value"
    );
    assert_eq!(data_bits(&got.data), data_bits(&want.data), "{ctx}: data");
}

fn assert_coord_bitwise(ctx: &str, got: &Coord, want: &Coord) {
    assert_field_bitwise(ctx, &got.field, &want.field);
    assert_eq!(got.units, want.units, "{ctx}: units");
    assert_eq!(got.calendar, want.calendar, "{ctx}: calendar");
}

/// Variables, coordinates, attributes, dims and dtypes, bit for bit.
pub fn assert_ds_bitwise(ctx: &str, got: &NativeDataset, want: &NativeDataset) {
    let keys = |m: &HashMap<String, NativeField>| {
        let mut k: Vec<String> = m.keys().cloned().collect();
        k.sort();
        k
    };
    assert_eq!(keys(&got.variables), keys(&want.variables), "{ctx}: variable names");
    for (name, f) in &want.variables {
        assert_field_bitwise(&format!("{ctx}/{name}"), &got.variables[name], f);
    }
    let mut gc: Vec<&String> = got.coords.keys().collect();
    let mut wc: Vec<&String> = want.coords.keys().collect();
    gc.sort();
    wc.sort();
    assert_eq!(gc, wc, "{ctx}: coordinate names");
    for (name, c) in &want.coords {
        assert_coord_bitwise(&format!("{ctx}/coord {name}"), &got.coords[name], c);
    }
}

/// Gather `take` per axis out of a row-major array of `shape`.
fn gather<T: Clone>(flat: &[T], shape: &[usize], take: &[Vec<usize>]) -> Vec<T> {
    let mut out = Vec::new();
    fn rec<T: Clone>(
        flat: &[T],
        shape: &[usize],
        take: &[Vec<usize>],
        axis: usize,
        offset: usize,
        out: &mut Vec<T>,
    ) {
        if axis == shape.len() {
            out.push(flat[offset].clone());
            return;
        }
        let stride: usize = shape[axis + 1..].iter().product();
        for &i in &take[axis] {
            rec(flat, shape, take, axis + 1, offset + i * stride, out);
        }
    }
    rec(flat, shape, take, 0, 0, &mut out);
    out
}

fn slice_field(f: &NativeField, window: &BTreeMap<String, AxisSelect>) -> NativeField {
    let take: Vec<Vec<usize>> = f
        .dims
        .iter()
        .zip(&f.shape)
        .map(|(d, &len)| match window.get(d) {
            None | Some(AxisSelect::All) => (0..len).collect(),
            Some(AxisSelect::Indices(v)) => v.clone(),
            Some(AxisSelect::Range { start, stop, step }) => (*start..*stop).step_by(*step).collect(),
        })
        .collect();
    let data = match &f.data {
        ArrayData::F64(v) => ArrayData::F64(gather(v, &f.shape, &take)),
        ArrayData::I64(v) => ArrayData::I64(gather(v, &f.shape, &take)),
        ArrayData::I32(v) => ArrayData::I32(gather(v, &f.shape, &take)),
        ArrayData::Str(v) => ArrayData::Str(gather(v, &f.shape, &take)),
        ArrayData::Bool(v) => ArrayData::Bool(gather(v, &f.shape, &take)),
    };
    NativeField {
        dtype: f.dtype,
        dims: f.dims.clone(),
        shape: take.iter().map(Vec::len).collect(),
        data,
        fill_value: f.fill_value,
    }
}

/// The full read sliced afterwards by name — the gate a windowed read must meet.
pub fn slice_by_name(full: &NativeDataset, window: &Selection) -> NativeDataset {
    let empty = BTreeMap::new();
    let w = match window {
        Selection::Named(m) => m,
        Selection::All => &empty,
        other => panic!("slice_by_name takes a named window, got {other:?}"),
    };
    NativeDataset {
        variables: full
            .variables
            .iter()
            .map(|(k, f)| (k.clone(), slice_field(f, w)))
            .collect(),
        coords: full
            .coords
            .iter()
            .map(|(k, c)| {
                (
                    k.clone(),
                    Coord {
                        field: slice_field(&c.field, w),
                        units: c.units.clone(),
                        calendar: c.calendar.clone(),
                    },
                )
            })
            .collect(),
    }
}

/// A seeded splitmix64 stream, for decode latencies that vary run to run in a
/// reproducible way.
pub fn splitmix(state: &std::sync::atomic::AtomicU64) -> u64 {
    let mut z = state.fetch_add(0x9E37_79B9_7F4A_7C15, std::sync::atomic::Ordering::Relaxed);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}
