//! EarthSciIO — Rust core (component (a)): URL download + a shared
//! content-addressed cache.
//!
//! This is the **first data-loader machinery in the Rust track**. It implements
//! the language-neutral spec under [`../spec`](https://github.com/earthsci/earthsciio/tree/main/spec):
//!
//! - the **shared cache key** `sha256(resolved_url)` ([`cache_key`]) and the
//!   on-disk [`cache-format`](https://github.com/earthsci/earthsciio/blob/main/spec/cache-format.md)
//!   (`v1/blobs/<key[:2]>/<key>.<ext>`, `meta/<key>.json`, `locks/`, `tmp/`);
//! - the [`Manifest`] format, byte-identical to the Python writer, so a blob
//!   fetched by one language is reused and re-validated by the others;
//! - the **transport** and **store** registries
//!   ([`registries`](https://github.com/earthsci/earthsciio/blob/main/spec/registries.md)):
//!   the active `http`/`https`, `file`, and `cds` (Copernicus CDS API:
//!   submit→poll→download) transports and the `local` store — with the ERA5
//!   pressure-level request mapping ([`era5`]) building `cds://` URLs;
//! - `$EARTHSCIDATADIR` resolution ([`data_dir`]), **offline mode**
//!   ([`Cache::is_offline`], [`Error::CacheMiss`]), the ETag/checksum/TTL
//!   validation ladder ([`validate`]), mirror failover, and the pluggable
//!   [`auth`] seam;
//! - the concurrency contract — advisory `flock` + atomic rename — so multiple
//!   processes sharing one `/scratch.local` cache download a URL exactly once.
//!
//! Component (b) builds on this core: the [`FormatRegistry`]'s **readers**
//! decode a cached blob into native-grid arrays ([`NetcdfReader`]), and the
//! cadence-aware [`Provider`] drives `materialize`/`refresh`/`refresh_times`/
//! `prefetch` over them — returning **raw** native arrays (remap/regrid stay
//! upstream/downstream).
//!
//! # Example — fetch (or reuse) a blob
//!
//! ```no_run
//! use earthsciio::{Cache, FetchRequest};
//!
//! let cache = Cache::from_env()?;                 // $EARTHSCIDATADIR + EARTHSCI_OFFLINE
//! let blob = cache.fetch(&FetchRequest::new("https://data.earthsci.dev/era5/2018/11/20181108.nc")
//!     .loader("era5"))?;
//! println!("cached at {} ({} bytes)", blob.path.display(), blob.manifest.bytes);
//! # Ok::<(), earthsciio::Error>(())
//! ```
//!
//! # Example — offline, cache-only (hermetic)
//!
//! ```no_run
//! use earthsciio::{Cache, FetchRequest};
//!
//! let cache = Cache::builder().data_dir("conformance/corpus/cache").offline(true).build()?;
//! let blob = cache.fetch(&FetchRequest::new("https://data.earthsci.dev/era5/2018/11/20181108.nc"))?;
//! // A miss raises Error::CacheMiss naming the url + key — never a silent empty.
//! # Ok::<(), earthsciio::Error>(())
//! ```

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod auth;
mod cache;
mod clock;
pub mod datadir;
pub mod era5;
mod error;
pub mod format;
mod key;
pub mod manifest;
mod offline;
mod provider;
pub mod store;
pub mod transport;
pub mod validate;

pub use cache::{Cache, CacheBuilder, CachedBlob, FetchRequest};
pub use datadir::{data_dir, default_data_dir, expand_datadir, DATADIR_ENV};
pub use error::{Error, Result};
pub use format::{
    ArrayData, Coord, DType, FormatRegistry, GeoTiffReader, NativeDataset, NativeField,
    NetcdfReader, Reader, Selection,
};
pub use key::{cache_key, cache_key_range, sha256_file, sha256_hex};
pub use manifest::{Manifest, MANIFEST_SCHEMA};
pub use offline::{is_offline, OFFLINE_ENV};
pub use provider::{DataLoader, LoaderTemporal, Provider, Window};
pub use validate::{CacheDecision, Temporal};
