//! A MISSING key must read as absence, not as an error — the zarr store contract.
//!
//! `zarrs` opens an array by probing the v3 `zarr.json` FIRST and falling back to
//! the v2 `.zarray` when the store answers "no such key". `CacheStore::fetch_whole`
//! mapped only [`Error::CacheMiss`] to `Ok(None)`, so a LIVE source answering
//! "not found" surfaced as a hard error straight through that probe: every Zarr
//! v2 store was unreadable **online**, because the first key zarr asks for is the
//! one that does not exist.
//!
//! Every pre-existing v2 test in this crate builds its cache with `.offline(true)`,
//! where a missing key raises `CacheMiss` and was already handled — which is
//! exactly why the online path went untested. These tests use the `file`
//! transport with a real (online) cache, so the probe takes the
//! definitive-absence path instead.
//!
//! The distinction that matters is DEFINITIVE ABSENCE (404/410, a missing local
//! file) versus UNKNOWN (timeout, 5xx, 403). Only the former may become `None`;
//! reporting absence for a transient fault would present a live store as empty.

// Native-only: this tier drives the cache, the transports and the format
// readers, none of which exist on wasm32 (see the crate's module docs).
#![cfg(not(target_arch = "wasm32"))]

use std::fs;
use std::path::Path;
use std::sync::Arc;

use earthsciio::format::{AxisSelect, Selection};
use earthsciio::{Cache, DataLoader, Error, Provider};

/// An UNCOMPRESSED Zarr **v2** array: `.zarray` + `.zattrs` + raw LE f64 chunks,
/// and deliberately NO `zarr.json`.
fn write_v2_array(root: &Path, name: &str, shape: [usize; 2], chunks: [usize; 2], data: &[f64]) {
    let dir = root.join(name);
    fs::create_dir_all(&dir).expect("create array dir");
    fs::write(
        dir.join(".zarray"),
        format!(
            r#"{{"zarr_format":2,"shape":[{},{}],"chunks":[{},{}],"dtype":"<f8",
                "compressor":null,"fill_value":0.0,"order":"C","filters":null}}"#,
            shape[0], shape[1], chunks[0], chunks[1]
        ),
    )
    .expect("write .zarray");
    fs::write(dir.join(".zattrs"), r#"{"_ARRAY_DIMENSIONS":["y","x"]}"#).expect("write .zattrs");
    assert!(
        !dir.join("zarr.json").exists(),
        "the fixture must be v2-only — the whole point is the absent v3 metadata"
    );

    let (nc0, nc1) = (shape[0].div_ceil(chunks[0]), shape[1].div_ceil(chunks[1]));
    for c0 in 0..nc0 {
        for c1 in 0..nc1 {
            let mut buf = Vec::with_capacity(chunks[0] * chunks[1] * 8);
            for i in 0..chunks[0] {
                for j in 0..chunks[1] {
                    let (r, c) = (c0 * chunks[0] + i, c1 * chunks[1] + j);
                    let v = if r < shape[0] && c < shape[1] {
                        data[r * shape[1] + c]
                    } else {
                        0.0
                    };
                    buf.extend_from_slice(&v.to_le_bytes());
                }
            }
            fs::write(dir.join(format!("{c0}.{c1}")), &buf).expect("write chunk");
        }
    }
}

fn fixture(tmp: &Path) -> String {
    let store = tmp.join("mini.zarr");
    let data: Vec<f64> = (0..3)
        .flat_map(|r| (0..4).map(move |c| (r * 10 + c) as f64))
        .collect();
    write_v2_array(&store, "field", [3, 4], [1, 4], &data);
    format!("file://{}", store.display())
}

/// ONLINE (not `.offline(true)`) — the configuration the pre-existing v2 tests
/// never exercised.
fn online_cache(tmp: &Path) -> Arc<Cache> {
    Arc::new(
        Cache::builder()
            .data_dir(tmp.join("cache"))
            .offline(false)
            .build()
            .expect("cache builds"),
    )
}

#[test]
fn v2_store_without_zarr_json_is_readable_online() {
    let tmp = tempfile::tempdir().expect("tmpdir");
    let base = fixture(tmp.path());
    let loader = DataLoader::new("mini", "zarr", base).variables(["field"]);
    let mut p = Provider::new(loader, online_cache(tmp.path()), None).expect("provider");

    let fields = p.materialize().expect("a v2 store must open online");
    let f = &fields["field"];
    assert_eq!(f.shape, vec![3, 4]);
}

#[test]
fn v2_selection_still_pushes_down_online() {
    let tmp = tempfile::tempdir().expect("tmpdir");
    let base = fixture(tmp.path());
    let loader = DataLoader::new("mini", "zarr", base).variables(["field"]);
    let mut p = Provider::new(loader, online_cache(tmp.path()), None).expect("provider");

    let sel = Selection::Orthogonal(vec![AxisSelect::Indices(vec![0, 2]), AxisSelect::All]);
    let fields = p
        .materialize_with_select(Some(&sel))
        .expect("selection honoured");
    assert_eq!(fields["field"].shape, vec![2, 4]);
}

#[test]
fn array_shape_reads_v2_metadata_online() {
    let tmp = tempfile::tempdir().expect("tmpdir");
    let base = fixture(tmp.path());
    let loader = DataLoader::new("mini", "zarr", base).variables(["field"]);
    let p = Provider::new(loader, online_cache(tmp.path()), None).expect("provider");

    assert_eq!(p.array_shape("field").expect("array_shape"), Some(vec![3, 4]));
}

// --------------------------------------------------------------------------- //
// The classification contract itself.
// --------------------------------------------------------------------------- //

#[test]
fn a_missing_local_file_classifies_as_not_found() {
    let e = Error::io(
        Some("/nowhere/at/all".into()),
        std::io::Error::new(std::io::ErrorKind::NotFound, "no such file"),
    );
    assert!(e.is_not_found());
}

#[test]
fn a_permission_error_is_not_an_absence() {
    let e = Error::io(
        Some("/root/secret".into()),
        std::io::Error::new(std::io::ErrorKind::PermissionDenied, "denied"),
    );
    assert!(
        !e.is_not_found(),
        "a permission failure leaves existence UNKNOWN and must stay an error"
    );
}

#[test]
fn all_mirrors_absent_is_an_absence_but_one_transient_is_not() {
    let absent = Error::AllMirrorsFailed {
        url: "u".into(),
        detail: "HTTP 404".into(),
        not_found: true,
    };
    assert!(absent.is_not_found());

    let unknown = Error::AllMirrorsFailed {
        url: "u".into(),
        detail: "timed out".into(),
        not_found: false,
    };
    assert!(
        !unknown.is_not_found(),
        "one mirror that merely failed to answer might well have had the object"
    );
}

#[test]
fn a_transport_failure_is_not_an_absence() {
    let e = Error::Transport {
        url: "u".into(),
        detail: "HTTP 503".into(),
    };
    assert!(!e.is_not_found());
}
