# earthsciio (Rust)

The Rust core of [EarthSciIO](../README.md) — **component (a)**: URL download +
a **shared content-addressed cache**. This is the first data-loader machinery in
the Rust track (the project had no I/O dependencies before).

It implements the language-neutral contract under [`../spec`](../spec):

| Spec | Here |
|---|---|
| [cache key](../spec/cache-format.md#1-cache-key) `sha256(resolved_url)` | [`cache_key`](src/key.rs) |
| [on-disk layout](../spec/cache-format.md#2-on-disk-layout) + [manifest](../spec/cache-format.md#3-manifest--metakeyjson) | [`store::LocalStore`](src/store/local.rs), [`Manifest`](src/manifest.rs) |
| [transport / store registries](../spec/registries.md) | [`transport`](src/transport), [`store`](src/store) |
| [`$EARTHSCIDATADIR`](../spec/cache-format.md#5-earthscidatadir-resolution) | [`datadir`](src/datadir.rs) |
| [offline mode](../spec/offline-mode.md) + `CacheMiss` | [`Cache`](src/cache.rs), [`offline`](src/offline.rs) |
| [validation ladder](../spec/cache-format.md#4-validation-and-integrity) (ETag / checksum / TTL) | [`validate`](src/validate.rs) |
| concurrency: advisory `flock` + atomic rename | [`store::LocalStore`](src/store/local.rs) |

The manifest serializer is **byte-identical** to the Python writer, so a blob
cached by one language is reused — and re-validated — by the others.

The three registries keep the fetch path extensible by construction: a new
transport (`s3`), store, or reader registers under a new name without touching
the fetch flow. The active set here is the `http`/`https` + `file` transports
and the `local` store. **Format readers** (decoding a blob into native-grid
arrays) are component (b).

## Use

```rust
use earthsciio::{Cache, FetchRequest};

// $EARTHSCIDATADIR + EARTHSCI_OFFLINE from the environment.
let cache = Cache::from_env()?;
let blob = cache.fetch(&FetchRequest::new("https://data.earthsci.dev/era5/2018/11/20181108.nc")
    .loader("era5"))?;
// blob.path is the cached file; blob.manifest carries its validation/provenance.
# Ok::<(), earthsciio::Error>(())
```

Point a provider at the conformance corpus with `offline = true` and every
golden case resolves from disk with no network:

```rust
let cache = earthsciio::Cache::builder()
    .data_dir("../conformance/corpus/cache")
    .offline(true)
    .build()?;
# Ok::<(), earthsciio::Error>(())
```

## Dependencies

`reqwest` (blocking, `rustls-tls`/ring — no system OpenSSL), `sha2`, `serde` +
`serde_json`, `fs2` (advisory lock), `tempfile` (atomic-rename staging), `time`
(RFC 3339 `fetched_at` + TTL). HTTPS works out of the box; offline/CI builds
need no network.

## Develop

```bash
cargo test                       # unit + integration (hermetic; no network)
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```

The integration tests cover the three acceptance criteria: fetch + cache +
offline re-read ([`fetch_cache_offline`](tests/fetch_cache_offline.rs)),
cross-language reuse of the Python-cached corpus
([`conformance_reuse`](tests/conformance_reuse.rs)), and concurrent fetchers
downloading exactly once ([`concurrency_lock`](tests/concurrency_lock.rs)).
