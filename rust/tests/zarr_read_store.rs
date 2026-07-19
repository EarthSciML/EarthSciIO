//! Store-backed `zarr` read_store — the load-bearing LAZINESS capability proven
//! against the committed corpus store: a runtime index list fetches ONLY the
//! intersecting chunk objects. Non-selected chunks are overwritten with
//! undecodable "poison" bytes in a scratch copy of the corpus, so any over-fetch
//! blosc-errors instead of silently succeeding.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use earthsciio::{
    cache_key, ArrayData, AxisSelect, Cache, DataLoader, Provider, Selection,
};

const BASE: &str = "s3://earthsci-fixtures/isrm-mini.zarr";

fn corpus_cache() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../conformance/corpus/cache")
}

/// Recursively copy `src` into `dst`.
fn copy_dir(src: &Path, dst: &Path) {
    std::fs::create_dir_all(dst).unwrap();
    for entry in std::fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let to = dst.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &to);
        } else {
            std::fs::copy(entry.path(), &to).unwrap();
        }
    }
}

/// Overwrite the cached blob for `url` (bare-key layout) with `bytes`.
fn poison(root: &Path, url: &str, bytes: &[u8]) {
    let key = cache_key(url);
    let blob = root.join("v1/blobs").join(&key[..2]).join(&key);
    assert!(blob.exists(), "expected corpus blob for {url} at {}", blob.display());
    std::fs::write(&blob, bytes).unwrap();
}

#[test]
fn read_store_is_lazy_never_touching_unselected_chunks() {
    let scratch = tempfile::tempdir().unwrap();
    let cache_root = scratch.path().join("cache");
    copy_dir(&corpus_cache(), &cache_root);

    // The selection layer=[1], y=[1,4], x=all needs ONLY field3d/1.0.0 and
    // field3d/1.2.0 (+ pop1d/0). Poison every other field3d chunk: a non-lazy
    // reader that fetched them would blosc-error.
    for key in ["0.0.0", "0.1.0", "0.2.0", "1.1.0"] {
        poison(&cache_root, &format!("{BASE}/field3d/{key}"), b"\x00POISON-not-blosc\xff");
    }

    // verify_on_read=false so poison is only "seen" if actually decoded (a
    // verify=true read would fail on the poison's integrity before decode — also
    // proving non-fetch, but this isolates the laziness of the reader itself).
    let cache = Cache::builder()
        .data_dir(&cache_root)
        .offline(true)
        .verify_on_read(false)
        .build()
        .unwrap();

    let loader = DataLoader::new("isrm", "zarr", BASE)
        .variables(["field3d", "pop1d"])
        .select(Selection::Orthogonal(vec![
            AxisSelect::Indices(vec![1]),
            AxisSelect::Indices(vec![1, 4]),
            AxisSelect::All,
        ]));
    let mut provider = Provider::new(loader, Arc::new(cache), None).unwrap();
    let buffers = provider.materialize().unwrap();

    let f3 = &buffers["field3d"];
    assert_eq!(f3.dims, vec!["layer", "y", "x"]);
    assert_eq!(f3.shape, vec![1, 2, 4]);
    assert_eq!(
        f3.data,
        ArrayData::F64(vec![110.0, 111.0, 112.0, 113.0, 140.0, 141.0, 142.0, 143.0])
    );
    // pop1d (rank 1 != 3 axes) reads whole.
    assert_eq!(
        buffers["pop1d"].data,
        ArrayData::F64(vec![1.0, 3.0, 5.0, 7.0, 9.0, 11.0, 13.0, 15.0])
    );
}

#[test]
fn over_selection_hits_poison_and_errors() {
    // Control: a selection that DOES touch a poisoned chunk decode-errors,
    // proving the poison is genuinely undecodable (so the lazy test is meaningful).
    let scratch = tempfile::tempdir().unwrap();
    let cache_root = scratch.path().join("cache");
    copy_dir(&corpus_cache(), &cache_root);
    // Poison the middle y-chunk (rows 2,3) of layer 1.
    poison(&cache_root, &format!("{BASE}/field3d/1.1.0"), b"\x00POISON\xff");

    let cache = Cache::builder()
        .data_dir(&cache_root)
        .offline(true)
        .verify_on_read(false)
        .build()
        .unwrap();
    let loader = DataLoader::new("isrm", "zarr", BASE)
        .variables(["field3d"])
        .select(Selection::Orthogonal(vec![
            AxisSelect::Indices(vec![1]),
            AxisSelect::Indices(vec![2]), // row 2 -> the poisoned middle chunk
            AxisSelect::All,
        ]));
    let mut provider = Provider::new(loader, Arc::new(cache), None).unwrap();
    assert!(provider.materialize().is_err());
}
