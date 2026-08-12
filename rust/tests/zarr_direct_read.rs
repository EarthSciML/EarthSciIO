//! `StoreAccess::Direct` — a store-backed read that writes NOTHING to disk.
//!
//! The claim under test is a resource claim, so it is measured rather than
//! asserted structurally: the same `Provider` read is run both ways against the
//! same store and the bytes that land in the cache directory are counted. The
//! two reads must produce identical values and differ only in disk footprint.
//!
//! The three properties that make the bypass safe to offer are covered here:
//!
//! 1. **Nothing is written** on the direct path (`0` bytes), while the cached
//!    path writes the fetched objects.
//! 2. **Selection pushdown survives.** A direct read that fetched whole arrays
//!    would be far worse than the cache it replaces, so the non-selected chunk
//!    objects are poisoned *in the store itself*: an over-fetching reader
//!    decodes garbage and fails, it cannot quietly succeed.
//! 3. **The default did not move.** A loader that says nothing still caches.
//!
//! `file://` keeps this hermetic. It is the same `object_store` dispatch S3 uses
//! — only the backend behind `parse_url_opts` differs — so the code path under
//! test is the production one. The online twin against the real InMAP ISRM
//! bucket is `isrm_direct_read_writes_nothing_to_disk`, `#[ignore]`d.

#![cfg(all(feature = "object-store", not(target_arch = "wasm32")))]

use std::fs;
use std::path::Path;
use std::sync::Arc;

use earthsciio::{
    ArrayData, AxisSelect, Cache, DataLoader, Provider, Selection, StoreAccess, STORE_ACCESS_ENV,
};

/// Total bytes of every regular file under `dir` (absent dir ⇒ 0). This is the
/// headline number: what the read cost the local disk.
fn bytes_on_disk(dir: &Path) -> u64 {
    if !dir.exists() {
        return 0;
    }
    let mut total = 0;
    for entry in fs::read_dir(dir).expect("read_dir") {
        let entry = entry.expect("dir entry");
        let ft = entry.file_type().expect("file type");
        if ft.is_dir() {
            total += bytes_on_disk(&entry.path());
        } else if ft.is_file() {
            total += entry.metadata().expect("metadata").len();
        }
    }
    total
}

/// Write an UNCOMPRESSED Zarr **v2** array — `.zarray` + `.zattrs` + raw LE f64
/// chunks, deliberately no `zarr.json`. Uncompressed so a poisoned chunk is
/// still a *decode* failure and the byte counts are exactly predictable.
///
/// `value(l, y, x)` fills the array so the two reads can be compared elementwise.
fn write_v2_array(
    root: &Path,
    name: &str,
    shape: [usize; 3],
    chunks: [usize; 3],
    value: impl Fn(usize, usize, usize) -> f64,
) {
    let dir = root.join(name);
    fs::create_dir_all(&dir).expect("create array dir");
    fs::write(
        dir.join(".zarray"),
        format!(
            r#"{{"zarr_format":2,"shape":[{},{},{}],"chunks":[{},{},{}],"dtype":"<f8",
                "compressor":null,"fill_value":0.0,"order":"C","filters":null}}"#,
            shape[0], shape[1], shape[2], chunks[0], chunks[1], chunks[2]
        ),
    )
    .expect("write .zarray");
    fs::write(
        dir.join(".zattrs"),
        r#"{"_ARRAY_DIMENSIONS":["layer","source","receptor"]}"#,
    )
    .expect("write .zattrs");

    let nchunks: Vec<usize> = (0..3).map(|d| shape[d].div_ceil(chunks[d])).collect();
    for c0 in 0..nchunks[0] {
        for c1 in 0..nchunks[1] {
            for c2 in 0..nchunks[2] {
                let mut buf = Vec::with_capacity(chunks.iter().product::<usize>() * 8);
                for i in 0..chunks[0] {
                    for j in 0..chunks[1] {
                        for k in 0..chunks[2] {
                            let (l, y, x) =
                                (c0 * chunks[0] + i, c1 * chunks[1] + j, c2 * chunks[2] + k);
                            let v = if l < shape[0] && y < shape[1] && x < shape[2] {
                                value(l, y, x)
                            } else {
                                0.0
                            };
                            buf.extend_from_slice(&v.to_le_bytes());
                        }
                    }
                }
                fs::write(dir.join(format!("{c0}.{c1}.{c2}")), &buf).expect("write chunk");
            }
        }
    }
}

/// An ISRM-shaped fixture: rank-3 `[layer, source, receptor]`, chunked one layer
/// and a block of sources at a time — the real store's `[1, 100, 52411]` shape,
/// scaled down.
const SHAPE: [usize; 3] = [3, 400, 500];
const CHUNKS: [usize; 3] = [1, 100, 500];
const VAR: &str = "SOA";

fn build_store(root: &Path) -> String {
    write_v2_array(root, VAR, SHAPE, CHUNKS, |l, y, x| {
        (l * 1_000_000 + y * 1_000 + x) as f64
    });
    format!("file://{}", root.display())
}

/// The gated selection: one layer, one chunk's worth of sources, every receptor
/// — exactly the ISRM access pattern (a support set of sources against all
/// receptors), and exactly ONE chunk object.
fn gated_selection() -> Selection {
    Selection::Orthogonal(vec![
        AxisSelect::Indices(vec![0]),
        AxisSelect::Range {
            start: 0,
            stop: 100,
            step: 1,
        },
        AxisSelect::All,
    ])
}

fn read_with(url: &str, cache_root: &Path, access: StoreAccess) -> Vec<f64> {
    let cache = Arc::new(
        Cache::builder()
            .data_dir(cache_root)
            .build()
            .expect("build cache"),
    );
    let loader = DataLoader::new("ISRM_SR", "zarr", url)
        .variables([VAR])
        .select(gated_selection())
        .store_access(access);
    let mut provider = Provider::new(loader, cache, None).expect("build provider");
    assert_eq!(provider.store_access(), access);
    let fields = provider.materialize().expect("materialize");
    let field = fields.get(VAR).expect("the selected variable");
    assert_eq!(field.shape, vec![1, 100, 500], "gated output shape");
    match &field.data {
        ArrayData::F64(v) => v.clone(),
        other => panic!("expected F64, got {other:?}"),
    }
}

/// THE MEASUREMENT. Same store, same selection, same values out — and the direct
/// read touches the disk not at all.
#[test]
fn direct_read_writes_nothing_to_disk_and_agrees_with_the_cached_read() {
    let scratch = tempfile::tempdir().unwrap();
    let url = build_store(&scratch.path().join("isrm-mini.zarr"));

    let cached_root = scratch.path().join("cache-cached");
    let cached = read_with(&url, &cached_root, StoreAccess::Cached);
    let cached_bytes = bytes_on_disk(&cached_root);

    let direct_root = scratch.path().join("cache-direct");
    let direct = read_with(&url, &direct_root, StoreAccess::Direct);
    let direct_bytes = bytes_on_disk(&direct_root);

    assert_eq!(cached, direct, "the two paths must decode the same values");

    // One chunk of 100x500 f64 = 400_000 bytes, plus the `.zarray`/`.zattrs`
    // metadata objects the cache also commits. The exact total is not the point;
    // that it is large and that the other is zero, is.
    assert!(
        cached_bytes >= 400_000,
        "the cached read should have committed the fetched chunk to disk, saw {cached_bytes} bytes"
    );
    assert_eq!(
        direct_bytes, 0,
        "the direct read must write NOTHING to the cache directory"
    );
    eprintln!("cached: {cached_bytes} bytes on disk; direct: {direct_bytes} bytes on disk");
}

/// Pushdown is the reason the direct path is worth having: the ISRM fetch is
/// only affordable because it reads 1,520 of 52,411 source cells. Every chunk
/// the selection does NOT intersect is truncated in the store, so a reader that
/// fetched it would hit a short read instead of succeeding quietly.
#[test]
fn direct_read_keeps_selection_pushdown() {
    let scratch = tempfile::tempdir().unwrap();
    let store = scratch.path().join("isrm-mini.zarr");
    let url = build_store(&store);

    // The selection needs ONLY chunk 0.0.0. Poison every other chunk object.
    let mut poisoned = 0;
    for c0 in 0..SHAPE[0] / CHUNKS[0] {
        for c1 in 0..SHAPE[1] / CHUNKS[1] {
            if (c0, c1) == (0, 0) {
                continue;
            }
            fs::write(
                store.join(VAR).join(format!("{c0}.{c1}.0")),
                b"POISON-not-a-chunk",
            )
            .expect("poison chunk");
            poisoned += 1;
        }
    }
    assert_eq!(poisoned, 11, "every chunk but the selected one is poisoned");

    let values = read_with(&url, &scratch.path().join("cache"), StoreAccess::Direct);
    // Spot-check the corners of the gated block against the generator.
    assert_eq!(values[0], 0.0);
    assert_eq!(*values.last().unwrap(), (99 * 1_000 + 499) as f64);
}

/// The default must not have moved: a loader that states nothing, in an
/// environment that states nothing, still goes through the cache. A silent
/// switch would make somebody's warm-cache workflow mysteriously slow.
#[test]
fn the_default_is_still_cached() {
    let scratch = tempfile::tempdir().unwrap();
    let url = build_store(&scratch.path().join("isrm-mini.zarr"));
    let cache_root = scratch.path().join("cache");
    let cache = Arc::new(
        Cache::builder()
            .data_dir(&cache_root)
            .build()
            .expect("build cache"),
    );
    // No `.store_access(..)` call at all — the pre-existing construction.
    let loader = DataLoader::new("ISRM_SR", "zarr", &url)
        .variables([VAR])
        .select(gated_selection());
    let mut provider = Provider::new(loader, cache, None).expect("build provider");
    assert_eq!(
        provider.store_access(),
        StoreAccess::Cached,
        "an unstated loader must still cache"
    );
    provider.materialize().expect("materialize");
    assert!(
        bytes_on_disk(&cache_root) > 0,
        "the default path must still populate the cache"
    );
}

/// The environment override is a DEFAULT, not the decision: it applies when the
/// loader is silent, and an explicit loader setting beats it.
///
/// Serialized into one test because `set_var` is process-global and Rust's test
/// harness is threaded.
#[test]
fn env_sets_the_default_but_the_loader_wins() {
    let base = DataLoader::new("L", "zarr", "file:///nonexistent.zarr");

    std::env::set_var(STORE_ACCESS_ENV, "direct");
    assert_eq!(StoreAccess::from_env(), StoreAccess::Direct);

    let scratch = tempfile::tempdir().unwrap();
    let cache = || {
        Arc::new(
            Cache::builder()
                .data_dir(scratch.path().join("cache"))
                .build()
                .expect("build cache"),
        )
    };
    // Silent loader: the environment decides.
    let p = Provider::new(base.clone(), cache(), None).expect("provider");
    assert_eq!(p.store_access(), StoreAccess::Direct);

    // Explicit loader: it wins over the environment, in the direction that
    // matters most — asking for the cache back while the process default is off.
    let p = Provider::new(
        base.clone().store_access(StoreAccess::Cached),
        cache(),
        None,
    )
    .expect("provider");
    assert_eq!(p.store_access(), StoreAccess::Cached);

    // An unrecognised value is not an error; it falls back to the safe default.
    std::env::set_var(STORE_ACCESS_ENV, "yes-please");
    assert_eq!(StoreAccess::from_env(), StoreAccess::Cached);

    std::env::remove_var(STORE_ACCESS_ENV);
    assert_eq!(StoreAccess::from_env(), StoreAccess::Cached);
}

/// A Zarr v2 `.zarray` written by zarr-python commonly carries
/// `"dimension_separator": null`, which `zarrs` refuses to parse. The cache path
/// has normalized that away since it was the only backend; the direct path must
/// too, or the same store is readable through one path and unopenable through
/// the other — the exact silent divergence a *choice* between two paths must not
/// have.
#[test]
fn direct_read_tolerates_the_zarr_python_v2_metadata_quirk() {
    let scratch = tempfile::tempdir().unwrap();
    let store = scratch.path().join("quirky.zarr");
    write_v2_array(&store, VAR, [1, 2, 2], [1, 2, 2], |_, y, x| {
        (y * 10 + x) as f64
    });

    // Re-write the metadata with the null field zarr-python emits.
    let meta = store.join(VAR).join(".zarray");
    let text = fs::read_to_string(&meta).unwrap();
    let mut doc: serde_json::Value = serde_json::from_str(&text).unwrap();
    doc["dimension_separator"] = serde_json::Value::Null;
    fs::write(&meta, serde_json::to_vec(&doc).unwrap()).unwrap();

    let cache = Arc::new(
        Cache::builder()
            .data_dir(scratch.path().join("cache"))
            .build()
            .expect("build cache"),
    );
    let loader = DataLoader::new("L", "zarr", format!("file://{}", store.display()))
        .variables([VAR])
        .store_access(StoreAccess::Direct);
    let mut provider = Provider::new(loader, cache, None).expect("provider");
    let fields = provider
        .materialize()
        .expect("a null dimension_separator must not make the store unopenable");
    match &fields[VAR].data {
        ArrayData::F64(v) => assert_eq!(v, &vec![0.0, 1.0, 10.0, 11.0]),
        other => panic!("expected F64, got {other:?}"),
    }
}

/// `Provider::array_shape` — the honour/refuse probe a caller makes BEFORE
/// pushing a projection down — must work on the direct path too, and must still
/// read only the metadata object.
#[test]
fn array_shape_probe_works_directly_and_writes_nothing() {
    let scratch = tempfile::tempdir().unwrap();
    let url = build_store(&scratch.path().join("isrm-mini.zarr"));
    let cache_root = scratch.path().join("cache");
    let cache = Arc::new(
        Cache::builder()
            .data_dir(&cache_root)
            .build()
            .expect("build cache"),
    );
    let loader = DataLoader::new("L", "zarr", &url)
        .variables([VAR])
        .store_access(StoreAccess::Direct);
    let provider = Provider::new(loader, cache, None).expect("provider");
    assert_eq!(
        provider.array_shape(VAR).expect("probe"),
        Some(SHAPE.to_vec())
    );
    assert_eq!(bytes_on_disk(&cache_root), 0, "the probe must not cache");
}

// ---------------------------------------------------------------------------
// The online twin: the REAL InMAP ISRM store.
// ---------------------------------------------------------------------------

/// One chunk of the real `s3://inmap-model/isrm_v1.2.1.zarr` store, read both
/// ways: the actual chunk shape (`[1, 100, 52411]` f4, blosc-lz4, Zarr v2,
/// unconsolidated) and the actual public unsigned-S3 access this must support.
///
/// `#[ignore]`d — it needs the network. Deliberately ONE chunk (~3.5 MB on the
/// wire): the full gated fetch is 15–25 GB and is measured on AWS, not here.
///
/// ```text
/// cargo test --features object-store --test zarr_direct_read -- --ignored --nocapture
/// ```
#[test]
#[ignore = "reads the public inmap-model S3 bucket over the network"]
fn isrm_direct_read_writes_nothing_to_disk() {
    const URL: &str = "s3://inmap-model/isrm_v1.2.1.zarr/";
    const SR: &str = "SOA";

    // Layer 0, the first chunk of source cells, every receptor: one chunk object.
    let select = Selection::Orthogonal(vec![
        AxisSelect::Indices(vec![0]),
        AxisSelect::Range {
            start: 0,
            stop: 100,
            step: 1,
        },
        AxisSelect::All,
    ]);

    let scratch = tempfile::tempdir().unwrap();
    let run = |access: StoreAccess, dir: &str| {
        let root = scratch.path().join(dir);
        let cache = Arc::new(Cache::builder().data_dir(&root).build().expect("cache"));
        // NOTE: no credentials anywhere. The runner has no s3:GetObject and the
        // bucket is public, so the direct path must read unsigned.
        let loader = DataLoader::new("ISRM_SR", "zarr", URL)
            .variables([SR])
            .select(select.clone())
            .store_access(access);
        let mut provider = Provider::new(loader, cache, None).expect("provider");
        let t0 = std::time::Instant::now();
        let fields = provider
            .materialize()
            .expect("materialize the real ISRM SR");
        let elapsed = t0.elapsed();
        let sum = match &fields[SR].data {
            ArrayData::F64(v) => v.iter().sum::<f64>(),
            other => panic!("expected F64, got {other:?}"),
        };
        (sum, bytes_on_disk(&root), elapsed)
    };

    let (cached_sum, cached_bytes, cached_time) = run(StoreAccess::Cached, "cache-cached");
    let (direct_sum, direct_bytes, direct_time) = run(StoreAccess::Direct, "cache-direct");

    eprintln!("ISRM SOA[0, 0:100, :]  ({} f64 out)", 100 * 52_411);
    eprintln!("  cached: {cached_bytes:>10} bytes to disk, {cached_time:?}");
    eprintln!("  direct: {direct_bytes:>10} bytes to disk, {direct_time:?}");

    assert_eq!(
        cached_sum, direct_sum,
        "the two paths must decode the same values"
    );
    assert!(
        cached_bytes > 3_000_000,
        "the cached read commits the chunk"
    );
    assert_eq!(direct_bytes, 0, "the direct read writes nothing");
}
