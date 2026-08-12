//! Direct (non-cache) Zarr read/write over Apache Arrow `object_store`
//! (feature `object-store`).
//!
//! This is the object-storage path that is **backed by `object_store`'s** mature
//! clients rather than any hand-rolled vendor code. A store URL is dispatched by
//! [`object_store::parse_url_opts`], rooted at its path prefix via
//! [`PrefixStore`], wrapped by the [`zarrs_object_store`] adapter (async), and
//! bridged into the crate's **sync** Zarr reader/writer through zarrs'
//! [`AsyncToSyncStorageAdapter`] driven by a `tokio` runtime. The decode/encode
//! logic itself is shared with the cache-backed path
//! ([`super::zarr::read_arrays`] / [`super::zarr_write::write_all_to_store`]) —
//! only the storage backend differs.
//!
//! **No provider is hard-coded.** A store's home is named by its URL scheme and
//! nothing else; adding a provider is a feature flag on `object_store`, not a new
//! code path here.
//!
//! # Which schemes are active
//!
//! With the `object-store` feature on, `object_store` is built with its `aws`,
//! `gcp`, `azure` and `http` features (plus its default `fs`), so **every** scheme
//! `object_store` recognises resolves:
//!
//! | URL | Backend | Gated on |
//! |---|---|---|
//! | `file://…` | local filesystem | `object_store/fs` (its default) |
//! | `memory://…` | in-process, for tests | always — no feature |
//! | `s3://bucket/…`, `s3a://bucket/…` | S3 **and every S3-compatible endpoint** | `object_store/aws` |
//! | `gs://bucket/…` | Google Cloud Storage | `object_store/gcp` |
//! | `az://`, `adl://`, `azure://`, `abfs://`, `abfss://` | Azure Blob / ADLS Gen2 | `object_store/azure` |
//! | `http://`, `https://` | plain HTTP(S) / WebDAV range reads | `object_store/http` |
//!
//! `https://` URLs are additionally re-dispatched *by host* inside
//! `object_store`: `*.amazonaws.com` and `*.r2.cloudflarestorage.com` resolve to
//! the S3 client, `*.blob.core.windows.net`/`*.dfs.core.windows.net` to Azure, and
//! anything else to the plain HTTP store.
//!
//! Any other scheme (`ftp://`, `opfs://`, …) is rejected by
//! [`resolve_backend`] with a `no object_store backend for …` error — a loud
//! failure, never a silent empty read.
//!
//! # Supplying an endpoint override (R2 / MinIO / Backblaze B2 / Ceph)
//!
//! S3-compatible endpoints matter more than new clouds: Cloudflare R2, MinIO,
//! Backblaze B2 and Ceph are all reached through the **`s3://` path plus a
//! configurable endpoint URL**, not through provider-specific code. The endpoint
//! is a config value:
//!
//! ```no_run
//! use earthsciio::{write_zarr_object_store_with_options, OutputSchema};
//! # fn demo(schema: &OutputSchema) -> Result<(), earthsciio::Error> {
//! // Cloudflare R2 — the vendor surface is one option, nothing else.
//! let opts = [
//!     ("endpoint".to_string(), "https://<account>.r2.cloudflarestorage.com".to_string()),
//!     ("region".to_string(), "auto".to_string()),
//! ];
//! write_zarr_object_store_with_options("s3://my-bucket/datasets/run-1.zarr", schema, &opts)?;
//! # Ok(())
//! # }
//! ```
//!
//! Option keys are `object_store`'s own (`endpoint` / `aws_endpoint_url`,
//! `region`, `aws_access_key_id`, `aws_secret_access_key`, `aws_allow_http`,
//! `aws_virtual_hosted_style_request`, `google_service_account_key`,
//! `azure_storage_account_name`, …); unrecognised keys are ignored, so one option
//! list can be handed to any scheme.
//!
//! The zero-option entry points ([`read_zarr_object_store`] /
//! [`write_zarr_object_store`]) default to [`store_options_from_env`], which
//! harvests `AWS_*`, `GOOGLE_*` and `AZURE_*` from the process environment — the
//! same variables each `object_store` builder's own `from_env` reads. So
//! `AWS_ENDPOINT_URL`, `AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY` and
//! `AWS_REGION` configure an S3-compatible target with no code change at all.
//!
//! (Before this seam existed the module called `parse_url`, which builds an
//! `AmazonS3Builder::new()` — a builder that reads **no** environment at all and
//! has no way to express an endpoint, so `s3://` only ever worked against real
//! AWS from an instance with an IMDS role.)
//!
//! The content-addressed cache remains the path used by the [`crate::Provider`];
//! this module is for callers that want to read/write a store directly.

use std::sync::Arc;

use object_store::path::Path as ObjectPath;
use object_store::prefix::PrefixStore;
use object_store::ObjectStore;
use url::Url;
use zarrs::storage::storage_adapter::async_to_sync::{
    AsyncToSyncBlockOn, AsyncToSyncStorageAdapter,
};
use zarrs_object_store::AsyncObjectStore;

use super::zarr_store::SanitizedV2;
use super::{AxisSelect, NativeDataset, OutputSchema, Selection};
use crate::error::{Error, Result};

/// Environment prefixes harvested by [`store_options_from_env`], one per cloud
/// backend, matching what each `object_store` builder's own `from_env` reads.
const ENV_PREFIXES: [&str; 3] = ["AWS_", "GOOGLE_", "AZURE_"];

/// Option keys that mean "this caller has a way to authenticate to S3". If ANY
/// of these is present, [`apply_s3_defaults`] leaves signing alone.
const S3_CREDENTIAL_KEYS: [&str; 8] = [
    "aws_skip_signature",
    "skip_signature",
    "aws_access_key_id",
    "access_key_id",
    "aws_secret_access_key",
    "secret_access_key",
    "aws_session_token",
    "aws_container_credentials_relative_uri",
];

fn os_err(detail: impl Into<String>) -> Error {
    Error::Format {
        format: "zarr".to_string(),
        detail: detail.into(),
    }
}

/// Backend configuration for a store URL, harvested from the process
/// environment: every `AWS_*`, `GOOGLE_*` and `AZURE_*` variable, lower-cased
/// into `object_store`'s option keys.
///
/// This is what the zero-option entry points use, so `AWS_ENDPOINT_URL` (or
/// `AWS_ENDPOINT`) is all an S3-compatible provider — R2, MinIO, Backblaze B2,
/// Ceph — needs. Keys `object_store` does not recognise for the dispatched
/// scheme are ignored rather than rejected.
///
/// Values are passed straight through to `object_store` and are never logged by
/// this crate.
#[must_use]
pub fn store_options_from_env() -> Vec<(String, String)> {
    std::env::vars()
        .filter(|(k, _)| ENV_PREFIXES.iter().any(|p| k.starts_with(p)))
        .map(|(k, v)| (k.to_ascii_lowercase(), v))
        .collect()
}

/// Give an `s3://` URL the SAME meaning it has on the cache-backed path:
/// **anonymous, regional, no credentials**.
///
/// This crate's own `s3://` transport (`transport::s3`) is documented as "a
/// public bucket needs no AWS SDK, no SigV4, no credentials", and resolves the
/// region as `$EARTHSCI_S3_REGION` → `$AWS_REGION` → `us-east-2`. `object_store`
/// makes the opposite default choice: it signs, and it has no region fallback.
/// Left alone, the same `s3://inmap-model/isrm_v1.2.1.zarr/` URL would read fine
/// through the cache and fail on a credential lookup without it — a silent
/// divergence between two paths that are supposed to differ only in whether
/// bytes touch the disk.
///
/// So: for `s3://`/`s3a://`, when the caller has supplied **no** way to
/// authenticate ([`S3_CREDENTIAL_KEYS`]), signing is switched off; and a missing
/// region is filled from the same chain the cached transport uses. Anything the
/// caller stated is left exactly as given — including `aws_skip_signature=false`,
/// which is how a caller asks for signed access explicitly.
fn apply_s3_defaults(url_str: &str, options: &mut Vec<(String, String)>) {
    if !(url_str.starts_with("s3://") || url_str.starts_with("s3a://")) {
        return;
    }
    fn has(options: &[(String, String)], k: &str) -> bool {
        options.iter().any(|(key, _)| key == k)
    }
    if !S3_CREDENTIAL_KEYS.iter().any(|k| has(options, k)) {
        options.push(("aws_skip_signature".to_string(), "true".to_string()));
    }
    if !has(options, "region") && !has(options, "aws_region") {
        options.push((
            "aws_region".to_string(),
            crate::transport::resolve_region(None),
        ));
    }
}

/// A `tokio` `block_on` bridge so an async `object_store` store can be used from
/// the crate's synchronous Zarr API (via zarrs' [`AsyncToSyncStorageAdapter`]).
struct TokioBlockOn(tokio::runtime::Handle);

impl AsyncToSyncBlockOn for TokioBlockOn {
    fn block_on<F: core::future::Future>(&self, future: F) -> F::Output {
        self.0.block_on(future)
    }
}

/// The sync storage type: an object_store store, prefix-rooted, adapted to sync.
type SyncObjectStore =
    AsyncToSyncStorageAdapter<AsyncObjectStore<PrefixStore<Box<dyn ObjectStore>>>, TokioBlockOn>;

/// Build a multi-thread `tokio` runtime for driving `object_store` I/O.
fn runtime() -> Result<tokio::runtime::Runtime> {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| os_err(format!("build tokio runtime for object_store: {e}")))
}

/// Dispatch a store URL to an `object_store` backend by **scheme alone**,
/// returning the store and the path prefix inside it.
///
/// This is the one place a provider is chosen. `options` are `object_store`'s
/// own config keys (see the module docs); an endpoint override travels here.
fn resolve_backend(
    url_str: &str,
    options: &[(String, String)],
) -> Result<(Box<dyn ObjectStore>, ObjectPath)> {
    let url =
        Url::parse(url_str).map_err(|e| os_err(format!("invalid store URL '{url_str}': {e}")))?;
    object_store::parse_url_opts(&url, options.iter().map(|(k, v)| (k.as_str(), v.as_str())))
        .map_err(|e| os_err(format!("no object_store backend for '{url_str}': {e}")))
}

/// Resolve a store URL to a sync-usable `zarrs` storage rooted at the store.
fn build_sync_store(
    url_str: &str,
    options: &[(String, String)],
    handle: tokio::runtime::Handle,
) -> Result<Arc<SyncObjectStore>> {
    let (store, prefix) = resolve_backend(url_str, options)?;
    let prefixed = PrefixStore::new(store, prefix);
    let async_store = Arc::new(AsyncObjectStore::new(prefixed));
    Ok(Arc::new(AsyncToSyncStorageAdapter::new(
        async_store,
        TokioBlockOn(handle),
    )))
}

/// The read-side store: [`build_sync_store`] plus the two things a *read* of a
/// foreign store needs and a write of our own does not — anonymous-S3 defaults
/// ([`apply_s3_defaults`]) and Zarr v2 `.zarray` normalization
/// ([`SanitizedV2`]). Together these make a direct read of a given URL behave
/// like the cache-backed read of the same URL.
fn build_read_store(
    url_str: &str,
    options: &[(String, String)],
    handle: tokio::runtime::Handle,
) -> Result<Arc<SanitizedV2<SyncObjectStore>>> {
    let mut opts = options.to_vec();
    apply_s3_defaults(url_str, &mut opts);
    let inner = build_sync_store(url_str, &opts, handle)?;
    Ok(Arc::new(SanitizedV2::new(inner)))
}

/// The full (dims-order) shape of array `var` in the store at `url`, read from
/// ONLY that array's metadata object — never a chunk.
///
/// The direct-read twin of [`super::Reader::array_shape`]: the same
/// honour/refuse probe a caller makes before pushing a projection down, with no
/// cache and so no bytes written to disk.
///
/// # Errors
/// Returns [`Error::Format`] if the URL has no `object_store` backend or the
/// array's metadata cannot be opened.
pub fn array_shape_object_store(
    url: &str,
    var: &str,
    options: &[(String, String)],
) -> Result<Vec<usize>> {
    let rt = runtime()?;
    let store = build_read_store(url, options, rt.handle().clone())?;
    let array = super::zarr::open_array(store, var)?;
    Ok(array.shape().iter().map(|&s| s as usize).collect())
}

/// Read `variables` from a Zarr store at `url` directly through `object_store`,
/// applying the orthogonal `select` lazily.
///
/// Backend options default to [`store_options_from_env`]; use
/// [`read_zarr_object_store_with_options`] to pass them explicitly. See the
/// module docs for the active schemes.
///
/// # Errors
/// Returns [`Error::Format`] if the URL has no `object_store` backend, the store
/// cannot be opened, or a decode fails.
pub fn read_zarr_object_store(
    url: &str,
    variables: &[String],
    select: &Selection,
) -> Result<NativeDataset> {
    read_zarr_object_store_with_options(url, variables, select, &store_options_from_env())
}

/// [`read_zarr_object_store`] with explicit backend `options` (`object_store`
/// config keys — `endpoint`, `region`, credentials, …) instead of the
/// environment-derived defaults.
///
/// # Errors
/// Returns [`Error::Format`] if the URL has no `object_store` backend, the store
/// cannot be opened, or a decode fails.
pub fn read_zarr_object_store_with_options(
    url: &str,
    variables: &[String],
    select: &Selection,
    options: &[(String, String)],
) -> Result<NativeDataset> {
    if variables.is_empty() {
        return Err(os_err(
            "object-store zarr read requires an explicit list of variables",
        ));
    }
    // The runtime must outlive the synchronous decode (the adapter blocks on it).
    let rt = runtime()?;
    let store = build_read_store(url, options, rt.handle().clone())?;
    let axes: Option<&[AxisSelect]> = match select {
        Selection::Orthogonal(a) => Some(a.as_slice()),
        _ => None,
    };
    // Selection pushdown is NOT re-implemented here: `read_arrays` is the same
    // function the cache-backed reader calls, so `Selection::Orthogonal` fetches
    // exactly the intersecting chunk objects on this path too.
    super::zarr::read_arrays(store, variables, axes)
}

/// Write a sharded Zarr **v3** store to `url` through `object_store`, following
/// `schema` (same layout as [`super::write_zarr_v3`]).
///
/// Backend options default to [`store_options_from_env`]; use
/// [`write_zarr_object_store_with_options`] to pass them explicitly.
///
/// # Errors
/// Returns [`Error::Format`] on schema inconsistency, a missing backend, or a
/// store write error.
pub fn write_zarr_object_store(url: &str, schema: &OutputSchema) -> Result<()> {
    write_zarr_object_store_with_options(url, schema, &store_options_from_env())
}

/// [`write_zarr_object_store`] with explicit backend `options` (`object_store`
/// config keys — `endpoint`, `region`, credentials, …) instead of the
/// environment-derived defaults.
///
/// # Errors
/// Returns [`Error::Format`] on schema inconsistency, a missing backend, or a
/// store write error.
pub fn write_zarr_object_store_with_options(
    url: &str,
    schema: &OutputSchema,
    options: &[(String, String)],
) -> Result<()> {
    let rt = runtime()?;
    let store = build_sync_store(url, options, rt.handle().clone())?;
    super::zarr_write::write_all_to_store(store, url, schema)
}

#[cfg(test)]
mod tests {
    use super::*;
    use object_store::ObjectStoreExt;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    /// Options that keep a backend build hermetic: no credential lookup, no
    /// metadata-server probe, no ambient config file. These are *not* secrets —
    /// they switch signing off, which is what a dispatch test wants.
    fn hermetic_opts() -> Vec<(String, String)> {
        [
            ("aws_skip_signature", "true"),
            ("aws_region", "auto"),
            ("google_skip_signature", "true"),
            ("azure_skip_signature", "true"),
            ("azure_storage_account_name", "dispatchtest"),
        ]
        .iter()
        .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
        .collect()
    }

    /// Every scheme the module claims is active must actually dispatch to a
    /// backend, with the in-store path prefix parsed off the URL. Pure URL
    /// parsing + client construction: no network, no credentials.
    #[test]
    fn every_claimed_scheme_dispatches_to_a_backend() {
        let opts = hermetic_opts();
        let cases: &[(&str, &str)] = &[
            ("file:///tmp/some/store.zarr", "tmp/some/store.zarr"),
            ("memory:///datasets/run-1.zarr", "datasets/run-1.zarr"),
            ("s3://bucket/datasets/run-1.zarr", "datasets/run-1.zarr"),
            ("s3a://bucket/datasets/run-1.zarr", "datasets/run-1.zarr"),
            ("gs://bucket/datasets/run-1.zarr", "datasets/run-1.zarr"),
            ("az://container/datasets/run-1.zarr", "datasets/run-1.zarr"),
            (
                "abfs://container/datasets/run-1.zarr",
                "datasets/run-1.zarr",
            ),
            (
                "abfss://container/datasets/run-1.zarr",
                "datasets/run-1.zarr",
            ),
            ("adl://container/datasets/run-1.zarr", "datasets/run-1.zarr"),
            (
                "azure://container/datasets/run-1.zarr",
                "datasets/run-1.zarr",
            ),
            (
                "http://example.org/datasets/run-1.zarr",
                "datasets/run-1.zarr",
            ),
            (
                "https://example.org/datasets/run-1.zarr",
                "datasets/run-1.zarr",
            ),
            // https is re-dispatched by host inside object_store: these are the
            // S3 and Azure clients, not the plain HTTP one.
            (
                "https://bucket.s3.us-east-1.amazonaws.com/datasets/run-1.zarr",
                "datasets/run-1.zarr",
            ),
            (
                "https://acct.r2.cloudflarestorage.com/bucket/datasets/run-1.zarr",
                "datasets/run-1.zarr",
            ),
            (
                "https://acct.blob.core.windows.net/container/datasets/run-1.zarr",
                "datasets/run-1.zarr",
            ),
        ];
        for (url, want_prefix) in cases {
            let (_store, prefix) = resolve_backend(url, &opts)
                .unwrap_or_else(|e| panic!("scheme dispatch failed for {url}: {e}"));
            assert_eq!(prefix.as_ref(), *want_prefix, "path prefix for {url}");
        }
    }

    /// A scheme with no backend fails loudly and names the URL — it must never
    /// fall through to some default store.
    #[test]
    fn an_unsupported_scheme_is_a_named_error() {
        for url in ["ftp://host/store.zarr", "opfs:///datasets/run-1.zarr"] {
            let err = resolve_backend(url, &[]).expect_err("should have no backend");
            let msg = err.to_string();
            assert!(msg.contains(url), "error should name the URL: {msg}");
        }
    }

    /// The whole of S3-compatible provider support: a configurable endpoint URL.
    /// Points the `s3://` path at a loopback listener and asserts the request
    /// actually goes there, path-style, with the bucket and key intact — which is
    /// what makes Cloudflare R2 / MinIO / Backblaze B2 / Ceph reachable without a
    /// vendor-specific code path. Loopback only: no network, no credentials
    /// (signing is switched off).
    #[test]
    fn an_s3_endpoint_override_redirects_the_request() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            let (mut sock, _) = listener.accept().expect("accept");
            let mut buf = [0u8; 4096];
            let n = sock.read(&mut buf).expect("read request");
            let req = String::from_utf8_lossy(&buf[..n]).to_string();
            sock.write_all(
                b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nETag: \"x\"\r\n\
                  Last-Modified: Thu, 01 Jan 1970 00:00:00 GMT\r\n\r\nhello",
            )
            .expect("write response");
            let _ = sock.flush();
            req
        });

        let endpoint = format!("http://127.0.0.1:{port}");
        let opts: Vec<(String, String)> = [
            ("endpoint", endpoint.as_str()),
            ("region", "auto"),
            ("aws_allow_http", "true"),
            ("aws_skip_signature", "true"),
        ]
        .iter()
        .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
        .collect();

        let (store, prefix) =
            resolve_backend("s3://my-bucket/datasets/run-1.zarr", &opts).expect("s3 dispatch");
        assert_eq!(prefix.as_ref(), "datasets/run-1.zarr");

        let rt = runtime().expect("runtime");
        let body = rt
            .block_on(async move {
                let key = prefix.join("zarr.json");
                store.get(&key).await?.bytes().await
            })
            .expect("get through the overridden endpoint");
        assert_eq!(&body[..], b"hello");

        let req = server.join().expect("server thread");
        let first_line = req.lines().next().unwrap_or_default().to_string();
        assert!(
            first_line.starts_with("GET /my-bucket/datasets/run-1.zarr/zarr.json "),
            "request should be path-style against the custom endpoint, got: {first_line}"
        );
        assert!(
            req.to_ascii_lowercase()
                .contains(&format!("host: 127.0.0.1:{port}")),
            "request should be addressed to the override host"
        );
    }

    /// An `s3://` URL means the same thing here as it does on the cache-backed
    /// path: anonymous, regional, no credentials. That is what makes
    /// `s3://inmap-model/...` — a public bucket read by a runner that
    /// deliberately holds no `s3:GetObject` — work through both paths with no
    /// configuration at all.
    #[test]
    fn an_s3_read_defaults_to_unsigned_and_regional() {
        let mut opts = Vec::new();
        apply_s3_defaults("s3://inmap-model/isrm_v1.2.1.zarr/", &mut opts);
        assert_eq!(
            opts.iter().find(|(k, _)| k == "aws_skip_signature"),
            Some(&("aws_skip_signature".to_string(), "true".to_string())),
            "an unauthenticated s3:// read must not try to sign"
        );
        assert!(
            opts.iter().any(|(k, v)| k == "aws_region" && !v.is_empty()),
            "object_store has no region fallback; the cached transport's chain supplies one"
        );
    }

    /// Whatever the caller stated is left alone — including asking for signed
    /// access explicitly, and including a non-S3 scheme, which is untouched.
    #[test]
    fn stated_credentials_and_other_schemes_are_left_alone() {
        for stated in [
            ("aws_access_key_id", "AKIAEXAMPLE"),
            ("aws_skip_signature", "false"),
        ] {
            let mut opts = vec![(stated.0.to_string(), stated.1.to_string())];
            apply_s3_defaults("s3://private-bucket/store.zarr", &mut opts);
            assert_eq!(
                opts.iter()
                    .filter(|(k, _)| k == "aws_skip_signature")
                    .count(),
                usize::from(stated.0 == "aws_skip_signature"),
                "stating {} must not add a signing default",
                stated.0
            );
        }

        let mut opts = Vec::new();
        apply_s3_defaults("https://example.org/store.zarr", &mut opts);
        assert!(opts.is_empty(), "non-s3 schemes get no S3 defaults");
    }

    /// `store_options_from_env` picks up exactly the cloud-config prefixes and
    /// lower-cases them into `object_store` option keys.
    #[test]
    fn env_options_are_prefix_scoped_and_lowercased() {
        // A synthetic, non-secret value; asserted on the key, not the value.
        std::env::set_var("AWS_ENDPOINT_URL", "http://127.0.0.1:9000");
        std::env::set_var("EARTHSCIIO_NOT_A_STORE_OPTION", "ignored");
        let opts = store_options_from_env();
        assert!(opts.iter().any(|(k, _)| k == "aws_endpoint_url"));
        assert!(
            !opts.iter().any(|(k, _)| k.contains("earthsciio")),
            "only AWS_/GOOGLE_/AZURE_ variables are harvested"
        );
        std::env::remove_var("AWS_ENDPOINT_URL");
        std::env::remove_var("EARTHSCIIO_NOT_A_STORE_OPTION");
    }
}
