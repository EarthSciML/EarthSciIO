//! Cross-language cache **reuse** (acceptance criterion 2): point the Rust
//! cache at the committed conformance corpus — a `$EARTHSCIDATADIR` populated by
//! the **Python** generator — and, fully offline, resolve each case's URL back
//! to its Python-cached blob byte-for-byte.
//!
//! This is the store-level half of conformance (checks 1, 2, and 5 of the
//! reference runner: cache-key agreement, manifest integrity, offline-only).
//! Decoding the blob into native arrays (checks 3–4) is component (b).

use std::fs;
use std::path::PathBuf;

use earthsciio::{cache_key, sha256_file, Cache, FetchRequest};

fn corpus_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../conformance/corpus")
}

#[test]
fn reuses_python_cached_corpus_offline() {
    let corpus = corpus_dir();
    let cache_root = corpus.join("cache");
    assert!(
        cache_root.join("v1").is_dir(),
        "corpus cache must exist at {cache_root:?}"
    );

    // Offline + integrity re-verification on read = conformance checks 2 & 5.
    let cache = Cache::builder()
        .data_dir(&cache_root)
        .offline(true)
        .verify_on_read(true)
        .build()
        .expect("build offline cache over the corpus");
    assert!(cache.is_offline());

    let index: serde_json::Value =
        serde_json::from_slice(&fs::read(corpus.join("cases.json")).unwrap()).unwrap();
    let cases = index["cases"].as_array().expect("cases array");
    assert!(!cases.is_empty(), "corpus must ship at least one case");

    for entry in cases {
        let case_path = corpus.join(entry["file"].as_str().unwrap());
        let case: serde_json::Value =
            serde_json::from_slice(&fs::read(&case_path).unwrap()).unwrap();

        let url = case["resolved_url"].as_str().unwrap();
        let expected_key = case["cache_key"].as_str().unwrap();
        let expected_sha = case["content_sha256"].as_str().unwrap();
        let expected_bytes = case["bytes"].as_u64().unwrap();

        // Check 1: cache-key agreement — sha256(resolved_url) == case.cache_key.
        assert_eq!(
            cache_key(url),
            expected_key,
            "cache-key mismatch for {}",
            entry["id"]
        );

        // Reuse the *Python-written* blob, offline, with no re-fetch.
        let blob = cache
            .fetch(&FetchRequest::new(url))
            .unwrap_or_else(|e| panic!("offline reuse failed for {url}: {e}"));

        assert_eq!(blob.key, expected_key);
        assert!(blob.path.exists(), "blob path must exist: {:?}", blob.path);

        // Check 2: manifest integrity — manifest + on-disk blob both agree with
        // the case's pinned content hash and size.
        assert_eq!(
            blob.manifest.sha256_content, expected_sha,
            "manifest sha for {}",
            entry["id"]
        );
        assert_eq!(
            blob.manifest.bytes, expected_bytes,
            "manifest bytes for {}",
            entry["id"]
        );
        assert_eq!(blob.manifest.url, url, "manifest url for {}", entry["id"]);
        assert_eq!(
            sha256_file(&blob.path).unwrap(),
            expected_sha,
            "on-disk blob bytes for {}",
            entry["id"]
        );
        assert_eq!(fs::metadata(&blob.path).unwrap().len(), expected_bytes);

        // Byte-identity: parsing the Python-written manifest and re-serializing
        // it reproduces the exact committed bytes — the cross-language guarantee.
        let manifest_bytes =
            fs::read(corpus.join(case["manifest_path"].as_str().unwrap())).unwrap();
        let reserialized = earthsciio::Manifest::from_json_bytes(&manifest_bytes)
            .unwrap()
            .to_json_bytes()
            .unwrap();
        assert_eq!(
            reserialized, manifest_bytes,
            "manifest round-trip not byte-identical for {}",
            entry["id"]
        );
    }
}

#[test]
fn offline_miss_names_url_and_key() {
    let cache = Cache::builder()
        .data_dir(corpus_dir().join("cache"))
        .offline(true)
        .build()
        .unwrap();

    let url = "https://data.earthsci.dev/era5/2099/01/20990101.nc"; // not in the corpus
    let err = cache.fetch(&FetchRequest::new(url)).unwrap_err();
    assert!(err.is_cache_miss(), "expected CacheMiss, got {err}");
    match err {
        earthsciio::Error::CacheMiss { url: u, key: k } => {
            assert_eq!(u, url);
            assert_eq!(k, cache_key(url)); // the miss names exactly which blob is absent
        }
        other => panic!("expected CacheMiss, got {other}"),
    }
}
