//! The cadence-aware **Provider** (component (b); plan §4.1–4.6).
//!
//! A `Provider` is bound to one loader and created once per simulation. It holds
//! the parsed loader, a cache handle, the run window, the format reader resolved
//! by the loader's declared format, and a **current native-grid buffer** per
//! variable. The solver reads the buffer as a constant between cadence
//! boundaries; at a boundary the solver's callback calls [`Provider::refresh`].
//!
//! It returns **raw native-grid arrays** keyed by the on-disk `file_variable`
//! name. Variable-name remap and `unit_conversion` stay in ESS; regrid is
//! ESD/C4's job (plan §4.3) — the Provider is the sanctioned impure I/O boundary
//! and nothing more.
//!
//! # CONST vs DISCRETE (plan §4.2)
//!
//! - **CONST** — no `temporal`: [`materialize`](Provider::materialize) reads the
//!   single file once; [`refresh`](Provider::refresh) is a no-op (`None`);
//!   [`refresh_times`](Provider::refresh_times) is empty.
//! - **DISCRETE** — has `temporal`: `refresh(t)` snaps `t` to the loader's
//!   cadence anchor and, when the anchor changed, re-reads the matching record
//!   into the buffer. `refresh_times()` is the cadence schedule over the window —
//!   the solver tstops the callback fires on.
//!
//! # Time type
//!
//! The plan sketches `DateTime<Utc>`, but this crate already standardizes on the
//! `time` crate (see `validate.rs`, the manifest's RFC 3339 stamps); the Provider
//! uses [`time::OffsetDateTime`] for one consistent time type across the core.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;

use time::{Duration, OffsetDateTime};

use crate::cache::{Cache, FetchRequest};
use crate::error::{Error, Result};
use crate::format::{
    ArrayData, AxisSelect, Coord, DType, FormatRegistry, NativeDataset, NativeField, Reader,
    Selection,
};

/// A run window `(start, end)` — half-open `[start, end)`.
pub type Window = (OffsetDateTime, OffsetDateTime);

/// The temporal nature of a loader that refreshes at a cadence (plan §4.2).
///
/// `frequency` is the cadence step (e.g. 1 hour for ERA5) — it drives the
/// refresh tstops and which record within a file is current. `file_period` is
/// the granularity of one file (e.g. 1 day) — it drives URL resolution. A file
/// holds `file_period / frequency` records along `time_dim`.
#[derive(Debug, Clone)]
pub struct SourceTemporal {
    /// The loader's epoch — cadence anchors are aligned to this instant.
    pub start: OffsetDateTime,
    /// Exclusive end of available data, if known (open-ended otherwise).
    pub end: Option<OffsetDateTime>,
    /// Cadence step between successive records.
    pub frequency: Duration,
    /// Time span covered by one file (drives which URL/file is fetched).
    pub file_period: Duration,
    /// Name of the record/time dimension to slice on (default `"time"`).
    pub time_dim: String,
    /// Number of records [`Provider::refresh`] returns per sample. `None` or
    /// `Some(1)` (default) returns the single at-or-before record with `time_dim`
    /// dropped (held piecewise-constant); `Some(2)` returns the two bracketing
    /// records (floor + successor) with `time_dim` retained at length 2 and a
    /// canonical 2-element epoch-seconds `time` coordinate, so a downstream model
    /// interpolates in time. The successor is read across a file boundary when
    /// needed; at the last available record the bracket degenerates to
    /// `[last, last]` so the downstream weight clamps. Only `1` / `2` are
    /// supported (validated in [`Provider::refresh`]); higher-order temporal
    /// stencils are future work.
    pub records_per_sample: Option<u32>,
}

impl SourceTemporal {
    /// A temporal block with `frequency` cadence and `file_period` files,
    /// anchored at `start`, open-ended, slicing on the `time` dimension.
    pub fn new(start: OffsetDateTime, frequency: Duration, file_period: Duration) -> Self {
        Self {
            start,
            end: None,
            frequency,
            file_period,
            time_dim: "time".to_string(),
            records_per_sample: None,
        }
    }

    /// Set the exclusive end of available data.
    pub fn end(mut self, end: OffsetDateTime) -> Self {
        self.end = Some(end);
        self
    }

    /// Override the record/time dimension name (default `"time"`).
    pub fn time_dim(mut self, dim: impl Into<String>) -> Self {
        self.time_dim = dim.into();
        self
    }

    /// Set the number of records returned per sample. `1` (piecewise-constant
    /// single record) or `2` (the 2-record bracket); any other value is
    /// rejected when [`Provider::refresh`] validates the cadence.
    pub fn records_per_sample(mut self, n: u32) -> Self {
        self.records_per_sample = Some(n);
        self
    }
}

/// Environment variable setting the process-wide default for [`StoreAccess`]:
/// `direct` or `cached` (case-insensitive). See [`StoreAccess::from_env`].
pub const STORE_ACCESS_ENV: &str = "EARTHSCIIO_STORE_ACCESS";

/// How a **store-backed** loader (zarr) gets at its objects.
///
/// # Why this is a choice and not a fix
///
/// The content-addressed cache is what makes a *repeated* read cheap: a second
/// run, a second variable out of the same store, a DISCRETE loader stepping back
/// over a file it already has. That is most loaders, which is why
/// [`Cached`](StoreAccess::Cached) is the default and stays the default.
///
/// It earns nothing on a **cold, single-pass, in-region** read, and there it is
/// pure cost: the whole fetched slab is written to disk, read back once, and
/// discarded with the task. The InMAP ISRM source-receptor read is the case in
/// point — ~15–25 GB of gated chunks from a public bucket in the same region as
/// the compute, every chunk touched exactly once. Caching it buys nothing, costs
/// a full write-then-read on the critical path, and on AWS Fargate (20 GiB
/// ephemeral storage by default) may simply not fit.
///
/// # Who decides
///
/// The **caller** does, because the caller is the only one who knows the access
/// pattern. The cache cannot infer it: "will anyone read this object again?" is
/// a fact about the future, and the two cases are indistinguishable at the
/// moment of the first read. Size is not a proxy either — a small store read a
/// thousand times wants the cache more than a huge one read once.
///
/// The alternatives were considered and rejected:
///
/// * **A URL scheme** (`s3+direct://`) — the URL is also the *cache identity*
///   and the document's declared source of truth. A scheme says where bytes
///   live; overloading it to say how we buffer them makes two different URLs
///   name the same object.
/// * **An environment variable alone** — process-global, so a run with one
///   single-pass 20 GB store and one small repeatedly-read store cannot express
///   both. It is still supported ([`from_env`](StoreAccess::from_env)) as a
///   *deployment* override, because a runner that receives documents it did not
///   author has no other way to ask; it is a default, not the decision.
///
/// Precedence is therefore: an explicit [`DataSource::store_access`] wins; then
/// [`STORE_ACCESS_ENV`]; then [`Cached`](StoreAccess::Cached).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StoreAccess {
    /// Every store object is fetched through the content-addressed [`Cache`]:
    /// staged to a temp file, committed to disk, then read back. Integrity
    /// checking, offline replay, mirror failover and cross-process locking all
    /// come with it. **The default** — a warm-cache workflow must never become
    /// mysteriously slow because something changed underneath it.
    #[default]
    Cached,
    /// Store objects are read straight from object storage and decoded in
    /// memory. **Nothing is written to disk.** Selection pushdown is unchanged:
    /// only the chunk objects the selection intersects are fetched.
    ///
    /// The trade is real — no integrity record, no offline replay, no reuse
    /// across runs, and a re-read costs a second download. Choose it when the
    /// read is cold, single-pass, and near the data.
    Direct,
}

impl StoreAccess {
    /// The process-wide default from [`STORE_ACCESS_ENV`] (`direct` / `cached`),
    /// or [`StoreAccess::Cached`] when unset or unrecognised.
    ///
    /// An unrecognised value falls back to the safe default rather than
    /// erroring: a typo'd deployment variable should not take a run down, and
    /// the consequence of ignoring it is a slower run, never a wrong one.
    #[must_use]
    pub fn from_env() -> Self {
        match std::env::var(STORE_ACCESS_ENV) {
            Ok(v) if v.trim().eq_ignore_ascii_case("direct") => StoreAccess::Direct,
            _ => StoreAccess::Cached,
        }
    }
}

/// The minimal parsed `DataSource` the Provider needs: where the bytes are, how
/// to decode them, which variables to read, and the temporal cadence (absent ⇒
/// CONST). This is the I/O-relevant projection of the ESM `DataSource` contract —
/// the full contract (units, remap, grid family) lives upstream in ESS.
#[derive(Debug, Clone)]
pub struct DataSource {
    /// Loader name (provenance, recorded in the cache manifest).
    pub name: String,
    /// Format-registry key selecting the reader (e.g. `"netcdf"`).
    pub format: String,
    /// On-disk `file_variable` names to read; empty ⇒ all data variables.
    pub variables: Vec<String>,
    /// Cadence (absent ⇒ CONST/static).
    pub temporal: Option<SourceTemporal>,
    /// URL template in `time` format-description syntax (e.g.
    /// `".../era5/[year]/[month]/[year][month][day].nc"`). A template with no
    /// `[` placeholders is a literal URL (the CONST case).
    pub url_template: String,
    /// Failover mirrors sharing the same cache identity.
    pub mirrors: Vec<String>,
    /// Auth realm to fetch under (resolved by the cache's auth registry).
    pub auth_realm: Option<String>,
    /// Spatial/orthogonal selection for any reader that answers
    /// [`Reader::supports_selection`] — the store-backed `zarr` one (which fetches
    /// only the intersecting chunk objects) and the whole-file `netcdf` one (which
    /// fetches the same blob under the same cache key and materialises only the
    /// requested hyperslab). Default [`Selection::All`]; ignored by a reader that
    /// can honour neither. The Provider still owns temporal record slicing, so a
    /// selection's time axis must be `All`.
    pub select: Selection,
    /// Format-specific decode options, resolved against the registered reader
    /// by [`Reader::configured`] when the Provider is built — the Rust spelling
    /// of the Python/Julia `reader_kwargs`.
    ///
    /// This is how a *declaration* (an `.esm` data loader) says how its format
    /// is decoded, instead of a caller hand-injecting a configured reader
    /// through [`Provider::with_formats`]: `{"member_glob": "*egu*",
    /// "skip_header_row": true}` on an `ff10` loader is the FF10 zip case that
    /// motivated it. An option the bound reader does not recognise is an error
    /// at construction, never a silently-ignored key.
    pub reader_options: serde_json::Map<String, serde_json::Value>,
    /// Whether a **store-backed** read goes through the cache. `None` ⇒ not
    /// stated, so [`StoreAccess::from_env`] decides (which defaults to
    /// [`StoreAccess::Cached`]). Ignored by whole-file readers, which have no
    /// direct path. See [`StoreAccess`] for who decides and why.
    pub store_access: Option<StoreAccess>,
    /// Backend config for a [`StoreAccess::Direct`] read — `object_store`'s own
    /// option keys (`endpoint`, `region`, `aws_skip_signature`, credentials, …).
    /// Empty ⇒ environment defaults. These override the environment per loader,
    /// which is what a process-global variable cannot express when two loaders
    /// read two different stores.
    ///
    /// They are also the only place a *read* can say it should be **signed**:
    /// an `s3://` read defaults to anonymous no matter what credentials the
    /// process environment is carrying, because a platform-injected role is not
    /// this document asking for anything. So a public bucket needs nothing here,
    /// and a private one needs credentials or `aws_skip_signature=false`. See
    /// `format::zarr_object_store::read_store_options`.
    pub store_options: Vec<(String, String)>,
}

impl DataSource {
    /// A loader named `name`, decoded by `format`, resolving `url_template`.
    /// CONST by default — add [`temporal`](DataSource::temporal) for cadence.
    pub fn new(
        name: impl Into<String>,
        format: impl Into<String>,
        url_template: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            format: format.into(),
            variables: Vec::new(),
            temporal: None,
            url_template: url_template.into(),
            mirrors: Vec::new(),
            auth_realm: None,
            select: Selection::All,
            reader_options: serde_json::Map::new(),
            store_access: None,
            store_options: Vec::new(),
        }
    }

    /// Set the spatial/orthogonal selection for a store-backed reader (zarr).
    pub fn select(mut self, select: Selection) -> Self {
        self.select = select;
        self
    }

    /// Declare the format-specific decode options (see
    /// [`DataSource::reader_options`]). Resolved against the reader when the
    /// [`Provider`] is built, so an unrecognised option fails there.
    pub fn reader_options(mut self, options: serde_json::Map<String, serde_json::Value>) -> Self {
        self.reader_options = options;
        self
    }

    /// State whether this loader's store objects go through the cache,
    /// overriding [`STORE_ACCESS_ENV`]. See [`StoreAccess`].
    ///
    /// [`StoreAccess::Direct`] on a loader whose reader has no direct path is a
    /// named error at [`Provider::new`], never a silent fall back to the cache.
    pub fn store_access(mut self, access: StoreAccess) -> Self {
        self.store_access = Some(access);
        self
    }

    /// Backend config for a [`StoreAccess::Direct`] read (`object_store` option
    /// keys). Overrides the environment for this loader only — an S3-compatible
    /// endpoint, a region, or credentials / `aws_skip_signature=false` to ask for
    /// a **signed** read. An unsigned public read needs nothing here: see
    /// [`DataSource::store_options`](struct.DataSource.html#structfield.store_options).
    pub fn store_options<I, K, V>(mut self, options: I) -> Self
    where
        I: IntoIterator<Item = (K, V)>,
        K: Into<String>,
        V: Into<String>,
    {
        self.store_options = options
            .into_iter()
            .map(|(k, v)| (k.into(), v.into()))
            .collect();
        self
    }

    /// Restrict to specific on-disk variable names (empty keeps "all").
    pub fn variables<I, S>(mut self, vars: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.variables = vars.into_iter().map(Into::into).collect();
        self
    }

    /// Make the loader DISCRETE with the given cadence.
    pub fn temporal(mut self, temporal: SourceTemporal) -> Self {
        self.temporal = Some(temporal);
        self
    }

    /// Set failover mirror templates.
    pub fn mirrors<I, S>(mut self, mirrors: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.mirrors = mirrors.into_iter().map(Into::into).collect();
        self
    }

    /// Fetch under an auth realm.
    pub fn auth_realm(mut self, realm: impl Into<String>) -> Self {
        self.auth_realm = Some(realm.into());
        self
    }
}

/// A loader-bound provider of native-grid arrays, refreshed at the loader's
/// cadence. See the module-level documentation for the CONST/DISCRETE contract.
pub struct Provider {
    loader: DataSource,
    cache: Arc<Cache>,
    window: Option<Window>,
    reader: Arc<dyn Reader>,
    /// Current native-grid buffer per variable (the slice the solver reads).
    buffers: HashMap<String, NativeField>,
    /// Native coordinates of the current file.
    coords: HashMap<String, Coord>,
    /// The cadence anchor currently in the buffer (DISCRETE).
    current_anchor: Option<OffsetDateTime>,
    /// Tiny LRU of whole decoded files keyed by file anchor, most-recently-used
    /// at the tail: the buffer [`refresh`](Self::refresh) re-slices as the record
    /// moves, so one whole-file decode is amortised over every tick in the file.
    /// Capacity 1 for a single-record cadence (the open file) and 2 for
    /// `records_per_sample = 2`, whose successor of a file's last record lives in
    /// the next file, so the bracket may hold two adjacent files at a seam.
    /// [`sample_at`](Self::sample_at) never touches it.
    files: Vec<(OffsetDateTime, Arc<NativeDataset>)>,
    /// The store-access decision, resolved ONCE at construction (loader → env →
    /// [`StoreAccess::Cached`]) so it cannot change mid-run and so the
    /// environment is read once rather than per object.
    store_access: StoreAccess,
}

impl Provider {
    /// Bind a provider to `loader`, resolving its format reader from the built-in
    /// [`FormatRegistry`]. Errors with [`Error::UnknownFormat`] if no reader is
    /// registered for the loader's format.
    pub fn new(loader: DataSource, cache: Arc<Cache>, window: Option<Window>) -> Result<Self> {
        Self::with_formats(loader, cache, window, &FormatRegistry::with_builtins())
    }

    /// [`Provider::new`] but resolving the reader from a caller-supplied registry
    /// — the seam that lets a new format plug in without any Provider change.
    pub fn with_formats(
        loader: DataSource,
        cache: Arc<Cache>,
        window: Option<Window>,
        formats: &FormatRegistry,
    ) -> Result<Self> {
        let reader = formats
            .get(&loader.format)
            .ok_or_else(|| Error::UnknownFormat {
                name: loader.format.clone(),
            })?;
        // The loader's declared decode options configure the registered reader
        // (an unrecognised option errors here, not mid-decode).
        let reader = match reader.configured(&loader.reader_options)? {
            Some(configured) => configured,
            None => reader,
        };
        // Resolve the cached/direct decision once, and refuse an impossible one
        // HERE rather than at the first read. Falling back to the cache would
        // silently restore the disk cost the caller explicitly asked to avoid —
        // which on a 20 GiB ephemeral disk is a failure, not a slow success.
        let store_access = loader.store_access.unwrap_or_else(StoreAccess::from_env);
        if store_access == StoreAccess::Direct
            && reader.store_backed()
            && !reader.supports_direct_read()
        {
            return Err(Error::Format {
                format: loader.format.clone(),
                detail: format!(
                    "loader '{}' asked for uncached (direct) store reads, but the '{}' \
                     reader has no direct path in this build — rebuild earthsciio with \
                     the `object-store` feature, or use StoreAccess::Cached",
                    loader.name, loader.format
                ),
            });
        }
        Ok(Self {
            loader,
            cache,
            window,
            reader,
            store_access,
            buffers: HashMap::new(),
            coords: HashMap::new(),
            current_anchor: None,
            files: Vec::new(),
        })
    }

    /// The native coordinates of the current grid (latitude/longitude/time, …).
    /// Populated after the first [`materialize`](Self::materialize) /
    /// [`refresh`](Self::refresh).
    pub fn coords(&self) -> &HashMap<String, Coord> {
        &self.coords
    }

    /// True when the bound reader can honour an orthogonal `select` **without
    /// materialising the whole array**. A caller uses this to decide whether to
    /// push a projection down (via
    /// [`materialize_with_select`](Self::materialize_with_select)) or read whole
    /// and slice on its own side. Mirrors the Julia/Python `supports_selection`.
    ///
    /// It says nothing about what is FETCHED — pair it with
    /// [`store_backed`](Self::store_backed): `zarr` fetches only the intersecting
    /// chunk objects, so a selection shrinks the download too; `netcdf` fetches
    /// the same blob under the same cache key and only materialises the requested
    /// hyperslab.
    pub fn supports_selection(&self) -> bool {
        self.reader.supports_selection()
    }

    /// True when the bound reader reads a directory-like STORE (a Zarr v2 store,
    /// each object its own fetch) rather than one blob. Read with
    /// [`supports_selection`](Self::supports_selection) it tells a caller whether
    /// a pushed-down `select` shrinks the DOWNLOAD or only the decode.
    #[must_use]
    pub fn store_backed(&self) -> bool {
        self.reader.store_backed()
    }

    /// The full native (dims-order) shape of on-disk array `var`, for a
    /// honour/refuse pushdown decision, or `None`.
    ///
    /// For a store-backed zarr provider this reads ONLY the array's `.zarray`
    /// metadata (never a chunk); `None` for a whole-file reader (whose shape is not
    /// knowable without reading the blob). Mirrors the Julia/Python `array_shape`.
    pub fn array_shape(&self, var: &str) -> Result<Option<Vec<usize>>> {
        if !self.reader.store_backed() {
            return Ok(None);
        }
        let anchor = match &self.loader.temporal {
            None => OffsetDateTime::UNIX_EPOCH,
            Some(t) => t.start,
        };
        let url = self.resolve_url(anchor)?;
        match self.store_access {
            StoreAccess::Direct => {
                self.reader
                    .array_shape_direct(&url, var, &self.loader.store_options)
            }
            StoreAccess::Cached => self.reader.array_shape(self.cache.clone(), &url, var),
        }
    }

    /// How this provider reaches a store-backed loader's objects, after the
    /// loader → environment → default resolution. See [`StoreAccess`].
    #[must_use]
    pub fn store_access(&self) -> StoreAccess {
        self.store_access
    }

    /// Materialize the loader's native arrays into the buffer.
    ///
    /// CONST: reads the single file once. DISCRETE: primes the buffer at the
    /// first cadence anchor of the window (equivalent to `refresh(window.start)`),
    /// so a caller can read an initial state before stepping.
    pub fn materialize(&mut self) -> Result<HashMap<String, NativeField>> {
        self.materialize_with_select(None)
    }

    /// [`materialize`](Self::materialize) with an optional PER-CALL projection
    /// pushdown that OVERRIDES the loader's baked [`DataSource::select`] for this
    /// call only — the seam a caller (EarthSciAST) uses to push a projection down
    /// at sample time without rebuilding the provider. Mirrors the Julia/Python
    /// per-call `select` override.
    ///
    /// Only a store-backed reader that [`supports_selection`](Self::supports_selection)
    /// can honour a per-call `select`; passing one to a whole-file reader is an
    /// error. A per-call `select` is a projection **peek**: it reads fresh and
    /// returns the selected arrays WITHOUT disturbing the cadence buffer the solver
    /// reads (so a subsequent plain `materialize()` restores the baked projection).
    pub fn materialize_with_select(
        &mut self,
        sel: Option<&Selection>,
    ) -> Result<HashMap<String, NativeField>> {
        if sel.is_some() && !self.reader.supports_selection() {
            return Err(Error::Format {
                format: self.loader.format.clone(),
                detail: format!(
                    "reader for format '{}' does not support select/pushdown",
                    self.loader.format
                ),
            });
        }
        match self.loader.temporal.clone() {
            None => {
                let effective = sel.unwrap_or(&self.loader.select);
                let url = self.resolve_url(OffsetDateTime::UNIX_EPOCH)?;
                let ds = self.read_file(url, effective)?;
                if sel.is_none() {
                    self.coords = ds.coords;
                    self.buffers = ds.variables;
                    Ok(self.buffers.clone())
                } else {
                    // Per-call override: a projection peek; leave the buffer intact.
                    Ok(ds.variables)
                }
            }
            Some(t) => {
                let first = self.lower_bound(&t);
                match self.refresh_with_select(first, sel)? {
                    Some(buffers) => Ok(buffers),
                    None => Ok(self.buffers.clone()),
                }
            }
        }
    }

    /// Refresh the buffer to the cadence anchor for time `t`.
    ///
    /// Returns `Some(buffer)` when the anchor changed (the solver must re-read),
    /// `None` when `t` falls in the same cadence interval as the last refresh, and
    /// `None` for a CONST loader (nothing refreshes).
    pub fn refresh(&mut self, t: OffsetDateTime) -> Result<Option<HashMap<String, NativeField>>> {
        self.refresh_with_select(t, None)
    }

    /// [`refresh`](Self::refresh) with an optional PER-CALL projection pushdown
    /// override (see [`materialize_with_select`](Self::materialize_with_select)).
    ///
    /// With `sel = None` this is exactly [`refresh`](Self::refresh): it consults
    /// the file cache, slices the current record, and updates the cadence buffer.
    /// With `sel = Some(_)` it is a cache-bypassing projection **peek**: it reads
    /// the covering file(s) fresh under `sel`, returns `Some(selected_arrays)` for
    /// `t`'s anchor, and does NOT disturb the buffer / file caches. A per-call
    /// `select` on a reader that cannot honour it is an error.
    pub fn refresh_with_select(
        &mut self,
        t: OffsetDateTime,
        sel: Option<&Selection>,
    ) -> Result<Option<HashMap<String, NativeField>>> {
        if sel.is_some() && !self.reader.supports_selection() {
            return Err(Error::Format {
                format: self.loader.format.clone(),
                detail: format!(
                    "reader for format '{}' does not support select/pushdown",
                    self.loader.format
                ),
            });
        }
        let Some(cadence) = self.cadence()? else {
            return Ok(None); // CONST: refresh is a no-op
        };
        let anchor = cadence.anchor(t);

        // PER-CALL override: a fresh, cache-bypassing projection peek. It returns
        // the selected arrays for `anchor` WITHOUT touching the buffer / caches, so
        // it never short-circuits on `current_anchor` and never evicts a cached file.
        if let Some(select) = sel {
            let ds = self.sample_anchor(&cadence, anchor, select, &mut FreshFiles)?;
            return Ok(Some(ds.variables));
        }

        if self.current_anchor == Some(anchor) {
            return Ok(None); // same cadence interval — unchanged (bracket too)
        }
        // `sample_at` and the buffer update, sharing the one sampling path
        // (`spec/prefetch.md` §1) — fed from this provider's own decoded-file LRU
        // rather than a fresh decode. The LRU is lent out for the call so the
        // sampler can stay `&self`, which is what lets `sample_at` run on a worker.
        let mut files = std::mem::take(&mut self.files);
        let lru = &mut LruFiles {
            files: &mut files,
            cap: if cadence.bracket() { 2 } else { 1 },
        };
        let sampled = self.sample_anchor(&cadence, anchor, &self.loader.select, lru);
        self.files = files;
        let ds = sampled?;
        self.coords = ds.coords;
        self.buffers = ds.variables;
        self.current_anchor = Some(anchor);
        Ok(Some(self.buffers.clone()))
    }

    /// The dataset [`refresh`](Self::refresh) would produce at `t`, computed
    /// without touching any provider buffer (`spec/prefetch.md` §1).
    ///
    /// `variables` are what `refresh(t)` returns when it returns `Some`, and
    /// `coords` are what [`coords`](Self::coords) holds after that `refresh(t)`:
    /// the covering file's whole coordinates for a single-record cadence (its
    /// time coordinate included), and for `records_per_sample = 2` the floor
    /// file's with `time_dim` replaced by the 2-element epoch-seconds bracket. The
    /// same snap-down, record resolution, bracket and end-of-data clamp apply,
    /// and it fails with what `refresh(t)` would. A CONST provider's sample is its
    /// [`materialize`](Self::materialize) result, coordinates included.
    ///
    /// It reads and writes no buffer — no current record, no current anchor, no
    /// decoded-file LRU — so it takes `&self`, may run concurrently on several
    /// threads (`Provider` is `Send + Sync`), and depends on `t` only through the
    /// cadence anchor `t` snaps to. The price is a fresh decode of the covering
    /// file(s) per call; a caller sampling many ticks of one file wants
    /// [`refresh`](Self::refresh) or a [`PrefetchGroup`](crate::PrefetchGroup),
    /// which share decoded files.
    pub fn sample_at(&self, t: OffsetDateTime) -> Result<NativeDataset> {
        self.sample_from(t, &self.loader.select, &mut FreshFiles)
    }

    /// A NEW provider sharing this one's cache and reader, with `w` baked in as
    /// its [`DataSource::select`] (`spec/prefetch.md` §2.3). This provider is
    /// unchanged.
    ///
    /// A window is bound at construction, never per call: the windowed provider
    /// keeps the decoded-file reuse of [`refresh`](Self::refresh) — each windowed
    /// file decoded once, not once per tick — where a per-call `select`
    /// ([`refresh_with_select`](Self::refresh_with_select)) stays a peek that
    /// reads fresh.
    ///
    /// Refused at once, before any I/O, when this provider already bakes a
    /// `select` (two windows do not compose), when its reader does not answer
    /// [`supports_selection`](Self::supports_selection), when `w` is not a
    /// [`Selection::Named`] window (a positional one cannot be applied by name),
    /// or when it names the loader's `time_dim` at all (records are the
    /// Provider's). `w` is a [`DimWindow`](crate::DimWindow) or the
    /// [`Selection::Named`] it converts to.
    pub fn with_window(&self, w: impl Into<Selection>) -> Result<Provider> {
        let w = w.into();
        self.check_window(&w)?;
        let mut loader = self.loader.clone();
        loader.select = w;
        Ok(Provider {
            loader,
            cache: self.cache.clone(),
            window: self.window,
            reader: self.reader.clone(),
            store_access: self.store_access,
            buffers: HashMap::new(),
            coords: HashMap::new(),
            current_anchor: None,
            files: Vec::new(),
        })
    }

    /// `K` windows at `t` from ONE fetch and decode (`spec/prefetch.md` §2.4):
    /// dataset `i` is bitwise identical — coordinates, dims, dtypes and NaN cells
    /// included — to `self.with_window(windows[i])?.sample_at(t)`.
    ///
    /// The read is the windows' bounding union by name: per dimension the sorted
    /// distinct union of their indices, or the whole dimension when any window
    /// leaves it unnamed or names it [`AxisSelect::All`]. Each window is then
    /// gathered back out of that one decode by dimension name, in its own order
    /// with its own repeats. This is for chunk layouts a window cannot shrink —
    /// GEOS-FP 0.25° A3dyn is one global level per chunk — where one rank per node
    /// decodes its neighbours' windows in a single pass.
    ///
    /// [`with_window`](Self::with_window)'s preconditions apply to every window,
    /// raised before any I/O; an empty list returns an empty list without
    /// fetching anything.
    pub fn sample_windows_at(
        &self,
        t: OffsetDateTime,
        windows: &[Selection],
    ) -> Result<Vec<NativeDataset>> {
        if windows.is_empty() {
            return Ok(Vec::new());
        }
        let plan = self.plan_windows(windows)?;
        self.sample_windows_from(t, &plan, &mut FreshFiles)
    }

    /// The cadence schedule over the window: the solver tstops the refresh
    /// callback fires on, as **Unix epoch seconds** (`f64`). Empty for a CONST
    /// loader, or when the loader is open-ended and no window bounds it.
    ///
    /// Each value is `anchor.unix_timestamp()`; a solver integrating in
    /// "seconds since window start" subtracts the window's start epoch.
    pub fn refresh_times(&self) -> Vec<f64> {
        let Some(temporal) = &self.loader.temporal else {
            return Vec::new(); // CONST: no refreshes
        };
        let freq_s = temporal.frequency.whole_seconds();
        if freq_s <= 0 {
            return Vec::new();
        }
        let lower = self.lower_bound(temporal);
        let Some(upper) = self.window.map(|(_, b)| b).or(temporal.end) else {
            return Vec::new(); // unbounded — no enumerable schedule
        };

        // First aligned anchor >= lower (lower is already >= start, so ceil ≥ 0).
        let elapsed = (lower - temporal.start).whole_seconds();
        let steps = (elapsed + freq_s - 1) / freq_s;
        let mut anchor = temporal.start + Duration::seconds(steps * freq_s);

        let mut out = Vec::new();
        while anchor < upper {
            out.push(anchor.unix_timestamp() as f64);
            anchor += Duration::seconds(freq_s);
        }
        out
    }

    /// Warm the cache for every file covering `window`, so an offline/HPC run
    /// never blocks on the network mid-integration (plan §4.5). Offline, this
    /// asserts each file is already present (a miss is [`Error::CacheMiss`]).
    pub fn prefetch(&mut self, window: Window) -> Result<()> {
        match self.loader.temporal.clone() {
            None => {
                self.fetch_blob(&self.resolve_url(OffsetDateTime::UNIX_EPOCH)?)?;
                Ok(())
            }
            Some(t) => {
                let file_s = t.file_period.whole_seconds();
                if file_s <= 0 {
                    return Err(Error::Format {
                        format: self.loader.format.clone(),
                        detail: "loader file_period must be positive".to_string(),
                    });
                }
                let start = if window.0 > t.start {
                    window.0
                } else {
                    t.start
                };
                let mut fa = snap_down(t.start, start, file_s);
                while fa < window.1 {
                    let url = self.resolve_url(fa)?;
                    self.fetch_blob(&url)?;
                    fa += Duration::seconds(file_s);
                }
                Ok(())
            }
        }
    }

    // --- internals ----------------------------------------------------------

    /// The effective lower bound: the window start clamped to the loader epoch.
    fn lower_bound(&self, temporal: &SourceTemporal) -> OffsetDateTime {
        match self.window {
            Some((a, _)) if a > temporal.start => a,
            _ => temporal.start,
        }
    }

    /// Resolve a file URL for an anchor. A template without `[` is a literal URL,
    /// as is a `cds://` URL — that scheme carries a fully-resolved CDS request
    /// whose canonical JSON legitimately contains `[...]` arrays (e.g. `area`,
    /// `variable`) that must NOT be read as `time` format components.
    fn resolve_url(&self, anchor: OffsetDateTime) -> Result<String> {
        let tmpl = &self.loader.url_template;
        if !tmpl.contains('[') || tmpl.starts_with("cds://") {
            return Ok(tmpl.clone());
        }
        let desc =
            time::format_description::parse_borrowed::<2>(tmpl).map_err(|e| Error::BadUrl {
                url: tmpl.clone(),
                detail: format!("invalid url_template: {e}"),
            })?;
        anchor.format(&desc).map_err(|e| Error::BadUrl {
            url: tmpl.clone(),
            detail: format!("formatting url_template at {anchor}: {e}"),
        })
    }

    /// Fetch (or reuse) the blob for `url` through the cache, with the loader's
    /// provenance, mirrors, and auth realm.
    fn fetch_blob(&self, url: &str) -> Result<crate::CachedBlob> {
        let mirrors: Vec<&str> = self.loader.mirrors.iter().map(String::as_str).collect();
        let mut req = FetchRequest::new(url)
            .loader(&self.loader.name)
            .mirrors(&mirrors);
        if let Some(realm) = &self.loader.auth_realm {
            req = req.auth_realm(realm);
        }
        self.cache.fetch(&req)
    }

    /// Fetch + decode a file into a native dataset under the effective `select`
    /// (the per-call override, else the loader's baked [`DataSource::select`]).
    fn read_file(&self, url: String, select: &Selection) -> Result<NativeDataset> {
        // Store-backed readers (e.g. zarr) are handed (cache, base_url, variables,
        // select): a Zarr `url` is a directory-like prefix, not a fetchable blob,
        // so the reader fetches individual objects on demand. Additive + default-
        // off: whole-file readers inherit `store_backed() == false` and read whole.
        if self.reader.store_backed() {
            return match self.store_access {
                // Straight from object storage, nothing written to disk. The
                // `select` travels unchanged, so pushdown is identical.
                StoreAccess::Direct => self.reader.read_store_direct(
                    &url,
                    &self.loader.variables,
                    select,
                    &self.loader.store_options,
                ),
                StoreAccess::Cached => {
                    self.reader
                        .read_store(self.cache.clone(), &url, &self.loader.variables, select)
                }
            };
        }
        // Whole-file reader. The `select` reaches it too: a reader that declares
        // `supports_selection` without being store-backed (the `netcdf` one)
        // honours it at DECODE time — the same blob under the same cache key,
        // with only the requested hyperslab materialised. A reader that declares
        // neither is a clear error, raised BEFORE the fetch — the per-call
        // override is already refused at the call site, and a BAKED
        // `DataSource::select` must be refused here rather than silently ignored
        // (that is what the Julia and Python tracks do, and a selection quietly
        // dropped is a caller reading the whole array believing it is a window).
        if !matches!(select, Selection::All) && !self.reader.supports_selection() {
            return Err(Error::Format {
                format: self.loader.format.clone(),
                detail: format!(
                    "reader for format '{}' does not support select/pushdown",
                    self.loader.format
                ),
            });
        }
        let blob = self.fetch_blob(&url)?;
        self.reader
            .read_native(&blob.path, &self.loader.variables, select)
    }

    /// Fetch + decode the file covering `file_anchor` under `select`: the one
    /// read every [`FileSource`] bottoms out in.
    pub(crate) fn read_file_at(
        &self,
        file_anchor: OffsetDateTime,
        select: &Selection,
    ) -> Result<NativeDataset> {
        self.read_file(self.resolve_url(file_anchor)?, select)
    }

    /// The validated cadence of a DISCRETE loader, or `None` for a CONST one.
    /// The checks [`refresh`](Self::refresh) has always made before touching a
    /// file, so [`sample_at`](Self::sample_at) fails exactly where it does.
    fn cadence(&self) -> Result<Option<Cadence>> {
        let Some(temporal) = self.loader.temporal.clone() else {
            return Ok(None);
        };
        let freq_s = temporal.frequency.whole_seconds();
        let file_s = temporal.file_period.whole_seconds();
        if freq_s <= 0 || file_s <= 0 {
            return Err(Error::Format {
                format: self.loader.format.clone(),
                detail: "loader cadence frequency/file_period must be positive".to_string(),
            });
        }
        if !matches!(temporal.records_per_sample, None | Some(1) | Some(2)) {
            return Err(Error::Format {
                format: self.loader.format.clone(),
                detail: format!(
                    "records_per_sample must be 1 or 2, got {}",
                    temporal.records_per_sample.unwrap()
                ),
            });
        }
        Ok(Some(Cadence {
            temporal,
            freq_s,
            file_s,
        }))
    }

    /// The sample at `t` under `select`, its files drawn from `files`: the ONE
    /// sampling path behind [`sample_at`](Self::sample_at) (fresh files),
    /// [`refresh`](Self::refresh) (this provider's LRU) and a
    /// [`PrefetchGroup`](crate::PrefetchGroup) worker (a decoded-file cache shared
    /// across workers). Decoding a file is deterministic, so which source fed it
    /// cannot change the result.
    pub(crate) fn sample_from(
        &self,
        t: OffsetDateTime,
        select: &Selection,
        files: &mut dyn FileSource,
    ) -> Result<NativeDataset> {
        match self.cadence()? {
            None => {
                let ds = files.file(self, OffsetDateTime::UNIX_EPOCH, select)?;
                Ok(Arc::try_unwrap(ds).unwrap_or_else(|shared| (*shared).clone()))
            }
            Some(cadence) => self.sample_anchor(&cadence, cadence.anchor(t), select, files),
        }
    }

    /// The DISCRETE sample at cadence anchor `anchor`: the at-or-before record
    /// with `time_dim` dropped and the covering file's coordinates, or for
    /// `records_per_sample = 2` the bracket ([`sample_bracket`](Self::sample_bracket)).
    fn sample_anchor(
        &self,
        cadence: &Cadence,
        anchor: OffsetDateTime,
        select: &Selection,
        files: &mut dyn FileSource,
    ) -> Result<NativeDataset> {
        if cadence.bracket() {
            return self.sample_bracket(cadence, anchor, select, files);
        }
        let temporal = &cadence.temporal;
        let file_anchor = snap_down(temporal.start, anchor, cadence.file_s);
        let file = files.file(self, file_anchor, select)?;
        let rec = ((anchor - file_anchor).whole_seconds() / cadence.freq_s) as usize;
        Ok(NativeDataset {
            variables: slice_record_buffers(&file, temporal.time_dim.as_str(), rec)?,
            coords: file.coords.clone(),
        })
    }

    /// The `records_per_sample = 2` sample: the two records bracketing `anchor`
    /// (floor + successor), retaining `time_dim` at length 2, plus a canonical
    /// 2-element epoch-seconds `time` coordinate. Handles the cross-file
    /// successor (record 0 of the next file when the floor is the last record)
    /// and the end-of-data clamp (a degenerate `[last, last]` bracket with equal
    /// timestamps — never an error).
    fn sample_bracket(
        &self,
        cadence: &Cadence,
        anchor: OffsetDateTime,
        select: &Selection,
        files: &mut dyn FileSource,
    ) -> Result<NativeDataset> {
        let temporal = &cadence.temporal;
        let time_dim = temporal.time_dim.as_str();
        let (freq_s, file_s) = (cadence.freq_s, cadence.file_s);

        // rec0 = the floor (at-or-before) record in file0, as the single path does.
        let file0_anchor = snap_down(temporal.start, anchor, file_s);
        let file0 = files.file(self, file0_anchor, select)?;
        let rec0 = ((anchor - file0_anchor).whole_seconds() / freq_s) as usize;
        let file0_len = time_len(&file0, time_dim);

        // Successor rec1: rec0+1 in the same file, else record 0 of the next file.
        // None => no reachable successor (end clamp): degenerate [rec0, rec0].
        let next_anchor = anchor + Duration::seconds(freq_s);
        let has_succ = temporal.end.is_none_or(|end| next_anchor < end);
        let mut succ: Option<(Arc<NativeDataset>, usize)> = None;
        if has_succ {
            if rec0 + 1 < file0_len {
                succ = Some((file0.clone(), rec0 + 1)); // successor in the same file
            } else {
                // Successor is record 0 of the next file; read/decode it (a read
                // failure at the seam is treated as end-of-data, not an error).
                let next_file_anchor = snap_down(temporal.start, next_anchor, file_s);
                if let Ok(file1) = files.file(self, next_file_anchor, select) {
                    let r1 = ((next_anchor - next_file_anchor).whole_seconds() / freq_s) as usize;
                    if r1 < time_len(&file1, time_dim) {
                        succ = Some((file1, r1));
                    }
                }
            }
        }
        let (file1, rec1, t1_epoch) = match succ {
            Some((f, r)) => (f, r, next_anchor.unix_timestamp() as f64),
            // Degenerate bracket: hold the last record, equal timestamps.
            None => (file0.clone(), rec0, anchor.unix_timestamp() as f64),
        };
        let t0_epoch = anchor.unix_timestamp() as f64;

        // Stack rec0 (file0) and rec1 (successor file) along the leading time axis.
        let mut variables = HashMap::with_capacity(file0.variables.len());
        for (name, field) in &file0.variables {
            let is_temporal = field.dims.first().map(String::as_str) == Some(time_dim);
            let stacked = if is_temporal {
                let f1 = file1.variables.get(name).unwrap_or(field);
                stack_two_records(field, rec0, f1, rec1)?
            } else {
                field.clone() // non-temporal variables pass through whole
            };
            variables.insert(name.clone(), stacked);
        }
        // Carry file0's coords, but replace the time coord with the 2-element
        // epoch-seconds bracket timestamps (the seam the ESS adapter reads).
        let mut coords = file0.coords.clone();
        coords.insert(
            time_dim.to_string(),
            Coord {
                field: NativeField {
                    dtype: DType::Float64,
                    dims: vec![time_dim.to_string()],
                    shape: vec![2],
                    data: ArrayData::F64(vec![t0_epoch, t1_epoch]),
                    fill_value: None,
                },
                units: Some("seconds since 1970-01-01T00:00:00Z".to_string()),
                calendar: Some("standard".to_string()),
            },
        );
        Ok(NativeDataset { variables, coords })
    }

    /// [`with_window`](Self::with_window)'s preconditions for one window, all
    /// decidable before any I/O.
    fn check_window(&self, w: &Selection) -> Result<()> {
        let refuse = |detail: String| Error::Format {
            format: self.loader.format.clone(),
            detail,
        };
        if !matches!(self.loader.select, Selection::All) {
            return Err(refuse(format!(
                "loader '{}' already bakes a select, and two windows do not compose; \
                 window the provider that has none",
                self.loader.name
            )));
        }
        if !self.reader.supports_selection() {
            return Err(refuse(format!(
                "reader for format '{}' does not support select/pushdown, so a \
                 window cannot be bound to loader '{}'",
                self.loader.format, self.loader.name
            )));
        }
        let Selection::Named(named) = w else {
            return Err(refuse(format!(
                "a window is a NAMED-dimension select (a DimWindow / \"dims\"), got \
                 {w:?}: a positional select cannot be applied by name, nor checked \
                 against loader '{}''s time_dim without opening the file",
                self.loader.name
            )));
        };
        if let Some(temporal) = &self.loader.temporal {
            if named.contains_key(&temporal.time_dim) {
                return Err(refuse(format!(
                    "the window names '{}', loader '{}''s time_dim: records are the \
                     Provider's (it owns the cadence), so a window never names the \
                     record axis",
                    temporal.time_dim, self.loader.name
                )));
            }
        }
        Ok(())
    }

    /// Check every window and compute their bounding union by name — what
    /// [`sample_windows_at`](Self::sample_windows_at) reads once and slices back.
    ///
    /// Every dimension any window names appears in the union, as the sorted
    /// distinct union of the windows' indices when ALL of them narrow it and as
    /// [`AxisSelect::All`] otherwise. Keeping the `All` entries (rather than
    /// leaving the dimension unnamed) is what makes the union read refuse a
    /// dimension no field carries even when only one window named it — exactly
    /// the error that window's own read would raise.
    pub(crate) fn plan_windows(&self, windows: &[Selection]) -> Result<WindowPlan> {
        for w in windows {
            self.check_window(w)?;
        }
        let named: Vec<&BTreeMap<String, AxisSelect>> = windows
            .iter()
            .map(|w| match w {
                Selection::Named(m) => m,
                _ => unreachable!("check_window refuses every non-named window"),
            })
            .collect();
        let dims: BTreeSet<&String> = named.iter().flat_map(|m| m.keys()).collect();
        let mut union = BTreeMap::new();
        for dim in dims {
            let mut acc: Option<BTreeSet<usize>> = Some(BTreeSet::new());
            for m in &named {
                let narrowed = match m.get(dim) {
                    Some(axis) => axis.unbounded_indices(&self.loader.format)?,
                    None => None,
                };
                match (narrowed, acc.as_mut()) {
                    (Some(idx), Some(set)) => set.extend(idx),
                    _ => acc = None,
                }
            }
            union.insert(
                dim.clone(),
                match acc {
                    Some(set) => AxisSelect::Indices(set.into_iter().collect()),
                    None => AxisSelect::All,
                },
            );
        }
        Ok(WindowPlan {
            windows: windows.to_vec(),
            union: Selection::Named(union),
        })
    }

    /// The windows of `plan` at `t`, the union's files drawn from `files`.
    pub(crate) fn sample_windows_from(
        &self,
        t: OffsetDateTime,
        plan: &WindowPlan,
        files: &mut dyn FileSource,
    ) -> Result<Vec<NativeDataset>> {
        if plan.windows.is_empty() {
            return Ok(Vec::new());
        }
        let union = &plan.union;
        let ds = self.sample_from(t, union, files)?;
        plan.windows
            .iter()
            .map(|w| slice_window(&ds, w, union, &self.loader.format))
            .collect()
    }

    /// Whether the loader is DISCRETE (has a cadence).
    pub(crate) fn is_discrete(&self) -> bool {
        self.loader.temporal.is_some()
    }

    /// The baked [`DataSource::select`].
    pub(crate) fn baked_select(&self) -> &Selection {
        &self.loader.select
    }

    /// The cadence anchor `t` snaps to, or `None` for a CONST loader or a
    /// non-positive frequency (which [`refresh_times`](Self::refresh_times)
    /// already turns into an empty schedule).
    pub(crate) fn cadence_anchor(&self, t: OffsetDateTime) -> Option<OffsetDateTime> {
        let temporal = self.loader.temporal.as_ref()?;
        let freq_s = temporal.frequency.whole_seconds();
        (freq_s > 0).then(|| snap_down(temporal.start, t, freq_s))
    }

    /// The URLs a sample at `t` reads — the covering file and, for a bracket
    /// whose successor may sit across a seam, the next one — for a disk-cache
    /// warm-up that decodes nothing. Empty for a store-backed loader, whose
    /// objects are known only once a decode resolves the chunk keys. Best-effort:
    /// an unresolvable URL is simply left out (the decode raises the real error).
    pub(crate) fn warm_urls(&self, t: OffsetDateTime) -> Vec<String> {
        if self.reader.store_backed() {
            return Vec::new();
        }
        let Ok(Some(cadence)) = self.cadence() else {
            return self.resolve_url(OffsetDateTime::UNIX_EPOCH).into_iter().collect();
        };
        let temporal = &cadence.temporal;
        let anchor = cadence.anchor(t);
        let mut anchors = vec![snap_down(temporal.start, anchor, cadence.file_s)];
        if cadence.bracket() {
            let next = anchor + Duration::seconds(cadence.freq_s);
            let next_file = snap_down(temporal.start, next, cadence.file_s);
            if temporal.end.is_none_or(|end| next < end) && next_file != anchors[0] {
                anchors.push(next_file);
            }
        }
        anchors.into_iter().filter_map(|a| self.resolve_url(a).ok()).collect()
    }

    /// Fetch (or reuse) `url` into the disk cache, decoding nothing.
    pub(crate) fn warm(&self, url: &str) -> Result<()> {
        self.fetch_blob(url).map(|_| ())
    }
}

/// The validated cadence of a DISCRETE loader (see [`Provider::cadence`]).
struct Cadence {
    temporal: SourceTemporal,
    freq_s: i64,
    file_s: i64,
}

impl Cadence {
    /// The cadence anchor `t` snaps down to.
    fn anchor(&self, t: OffsetDateTime) -> OffsetDateTime {
        snap_down(self.temporal.start, t, self.freq_s)
    }

    /// `records_per_sample = 2`: a sample is a two-record bracket.
    fn bracket(&self) -> bool {
        self.temporal.records_per_sample == Some(2)
    }
}

/// Where the sampler ([`Provider::sample_from`]) gets the decoded file covering a
/// file anchor. The sampler is one function; what varies between
/// [`Provider::sample_at`], [`Provider::refresh`] and a prefetch worker is only
/// whether a decoded file is reused and with whom it is shared.
pub(crate) trait FileSource {
    /// The whole decoded file covering `file_anchor`, read under `select`.
    fn file(
        &mut self,
        provider: &Provider,
        file_anchor: OffsetDateTime,
        select: &Selection,
    ) -> Result<Arc<NativeDataset>>;
}

/// No reuse: every request is a fresh fetch + decode. What
/// [`Provider::sample_at`] uses, so it holds no state at all.
pub(crate) struct FreshFiles;

impl FileSource for FreshFiles {
    fn file(
        &mut self,
        provider: &Provider,
        file_anchor: OffsetDateTime,
        select: &Selection,
    ) -> Result<Arc<NativeDataset>> {
        provider.read_file_at(file_anchor, select).map(Arc::new)
    }
}

/// The Provider's own decoded-file LRU, lent to the sampler by
/// [`Provider::refresh`]; on a hit the entry moves to the tail (most recently
/// used), so a file-period seam decodes each adjacent file at most once.
///
/// **Why this Provider does not push its records into the decode.** The
/// netcdf reader honours a [`Records`](crate::Records) pushdown in every
/// track (`spec/registries.md` §2.1) and the Julia Provider uses it, because
/// that one computes its record from the cadence and keeps no decoded-file
/// buffer: there a whole-file decode per sample is pure waste. Here this LRU
/// IS the optimisation — one whole-file decode amortised over every tick in
/// the file — so pushing down would replace one decode per FILE with one per
/// TICK. Measured on this reader (release build, warm page cache): an
/// A1-shaped file (24 records, 47 2-D variables) decodes whole in 37.7 ms,
/// i.e. 1.57 ms per tick, against 8.3 ms for a narrowed 2-record decode —
/// 5.3x worse per tick; an A3dyn-shaped file (8 records, 6 3-D variables)
/// 104.0 ms whole = 13.0 ms per tick against 29.5 ms narrowed, 2.3x worse.
/// The narrowed decode is genuinely 3.5–4.6x cheaper *per decode*, which is
/// exactly why it is worth having on the reader and not worth wiring in
/// front of a buffer that already avoids most of them. A caller that wants it
/// calls [`Reader::read_native_records`](crate::Reader::read_native_records)
/// directly; a track is not made slower for symmetry.
struct LruFiles<'a> {
    files: &'a mut Vec<(OffsetDateTime, Arc<NativeDataset>)>,
    cap: usize,
}

impl FileSource for LruFiles<'_> {
    fn file(
        &mut self,
        provider: &Provider,
        file_anchor: OffsetDateTime,
        select: &Selection,
    ) -> Result<Arc<NativeDataset>> {
        if let Some(pos) = self.files.iter().position(|(a, _)| *a == file_anchor) {
            let entry = self.files.remove(pos);
            let ds = entry.1.clone();
            self.files.push(entry);
            return Ok(ds);
        }
        let ds = Arc::new(provider.read_file_at(file_anchor, select)?);
        self.files.push((file_anchor, ds.clone()));
        while self.files.len() > self.cap {
            self.files.remove(0); // evict the least-recently-used
        }
        Ok(ds)
    }
}

/// [`Provider::plan_windows`]'s result: the windows, and the by-name union they
/// are read through.
pub(crate) struct WindowPlan {
    windows: Vec<Selection>,
    union: Selection,
}

/// Window `w` gathered back out of `ds`, a decode under the by-name `union`.
///
/// Along a dimension the union narrowed, the window's indices are a subset of the
/// union's sorted list, so each is found by position. Along a dimension the
/// union left whole, the decoded axis IS the file axis and the window's indices
/// are its positions — bounds-checked here against that length, since the union
/// read never saw them (it is exactly the check the window's own read makes, in
/// its words).
fn slice_window(
    ds: &NativeDataset,
    w: &Selection,
    union: &Selection,
    format: &str,
) -> Result<NativeDataset> {
    let (Selection::Named(window), Selection::Named(union)) = (w, union) else {
        return Ok(ds.clone()); // `Selection::All`: the whole union, which is everything
    };
    let mut picks: HashMap<String, Vec<usize>> = HashMap::new();
    for (dim, axis) in window {
        let Some(wanted) = axis.unbounded_indices(format)? else {
            continue;
        };
        let positions = match union.get(dim) {
            Some(AxisSelect::Indices(held)) => wanted
                .iter()
                .map(|g| held.binary_search(g).expect("the union holds every window index"))
                .collect(),
            _ => match axis_len(ds, dim) {
                // The union read this dimension whole for some OTHER window, so
                // the reader never refused this window's subset of a time axis:
                // refuse it here, as the window's own read would.
                Some(_) if is_time_dim(ds, dim) => {
                    return Err(Error::Format {
                        format: format.to_string(),
                        detail: format!(
                            "select asks for a subset of the time dimension '{dim}'; record \
                             selection is the Provider's, not the reader's — a time \
                             dimension may only be \"all\""
                        ),
                    })
                }
                Some(len) => axis.resolve_in(len, format)?,
                None => continue, // carried by no field: the union read refused it
            },
        };
        picks.insert(dim.clone(), positions);
    }
    Ok(ds.take_by_name(&picks))
}

/// Is `dim` the decoded dataset's time axis? A same-named coordinate with CF
/// `"<step> since <ref>"` units, or a dimension literally named `time` — the
/// union of the readers' rules, since only a refusal rests on it.
fn is_time_dim(ds: &NativeDataset, dim: &str) -> bool {
    let cf = ds.coords.get(dim).and_then(|c| c.units.as_deref()).is_some_and(|u| {
        u.split_whitespace().any(|w| w.eq_ignore_ascii_case("since"))
    });
    cf || dim.eq_ignore_ascii_case("time")
}

/// The length of `dim` in the first field (variable or coordinate) carrying it.
fn axis_len(ds: &NativeDataset, dim: &str) -> Option<usize> {
    ds.variables
        .values()
        .chain(ds.coords.values().map(|c| &c.field))
        .find_map(|f| f.dims.iter().position(|d| d == dim).map(|i| f.shape[i]))
}

// `sample_at` takes `&self` so a `PrefetchGroup` can call it from worker threads;
// that is only sound if every piece of a Provider — the cache, the reader, the
// loader — may be shared across threads. Asserted here, at compile time, so a
// field that is not `Sync` breaks the build rather than the prefetcher.
const _: () = {
    fn assert_send_sync<T: Send + Sync>() {}
    let _ = assert_send_sync::<Provider>;
};

/// Slice cadence record `rec` from every variable carrying `time_dim` as its
/// leading axis; non-temporal variables pass through whole. Shared by the plain
/// single-record refresh path and the per-call `select` peek.
fn slice_record_buffers(
    ds: &NativeDataset,
    time_dim: &str,
    rec: usize,
) -> Result<HashMap<String, NativeField>> {
    let mut buffers = HashMap::with_capacity(ds.variables.len());
    for (name, field) in &ds.variables {
        let is_temporal = field.dims.first().map(String::as_str) == Some(time_dim);
        let slice = if is_temporal {
            field.select_leading(rec)?
        } else {
            field.clone()
        };
        buffers.insert(name.clone(), slice);
    }
    Ok(buffers)
}

/// Length of `ds` along `time_dim`: from the time coordinate if it carries it,
/// else the first variable that does, else 0.
fn time_len(ds: &NativeDataset, time_dim: &str) -> usize {
    if let Some(coord) = ds.coords.get(time_dim) {
        if let Some(pos) = coord.field.dims.iter().position(|d| d == time_dim) {
            return coord.field.shape[pos];
        }
    }
    for field in ds.variables.values() {
        if let Some(pos) = field.dims.iter().position(|d| d == time_dim) {
            return field.shape[pos];
        }
    }
    0
}

/// Stack record `rec0` of `f0` and record `rec1` of `f1` along the leading axis,
/// keeping that axis (the time dim) at length 2. `f0`/`f1` are the same variable
/// across (possibly) adjacent files, so they share dtype/trailing shape.
fn stack_two_records(
    f0: &NativeField,
    rec0: usize,
    f1: &NativeField,
    rec1: usize,
) -> Result<NativeField> {
    let a = f0.select_leading(rec0)?; // drops the leading dim -> one record
    let b = f1.select_leading(rec1)?;
    let data = concat_array(&a.data, &b.data)?;
    let mut dims = Vec::with_capacity(a.dims.len() + 1);
    dims.push(f0.dims[0].clone());
    dims.extend(a.dims.iter().cloned());
    let mut shape = Vec::with_capacity(a.shape.len() + 1);
    shape.push(2);
    shape.extend(a.shape.iter().cloned());
    Ok(NativeField {
        dtype: f0.dtype,
        dims,
        shape,
        data,
        fill_value: f0.fill_value,
    })
}

/// Concatenate two same-dtype native arrays end-to-end (used to stack two records
/// into a size-2 leading axis).
fn concat_array(a: &ArrayData, b: &ArrayData) -> Result<ArrayData> {
    Ok(match (a, b) {
        (ArrayData::F64(x), ArrayData::F64(y)) => {
            let mut v = x.clone();
            v.extend_from_slice(y);
            ArrayData::F64(v)
        }
        (ArrayData::I64(x), ArrayData::I64(y)) => {
            let mut v = x.clone();
            v.extend_from_slice(y);
            ArrayData::I64(v)
        }
        (ArrayData::I32(x), ArrayData::I32(y)) => {
            let mut v = x.clone();
            v.extend_from_slice(y);
            ArrayData::I32(v)
        }
        (ArrayData::Str(x), ArrayData::Str(y)) => {
            let mut v = x.clone();
            v.extend_from_slice(y);
            ArrayData::Str(v)
        }
        (ArrayData::Bool(x), ArrayData::Bool(y)) => {
            let mut v = x.clone();
            v.extend_from_slice(y);
            ArrayData::Bool(v)
        }
        _ => {
            return Err(Error::Format {
                format: "native".to_string(),
                detail: "cannot stack bracket records of differing dtypes".to_string(),
            })
        }
    })
}

/// `start + floor((t - start) / step) * step` — snap `t` down to the aligned
/// anchor at or before it. `step_s` must be positive.
fn snap_down(start: OffsetDateTime, t: OffsetDateTime, step_s: i64) -> OffsetDateTime {
    let elapsed = (t - start).whole_seconds();
    let steps = elapsed.div_euclid(step_s); // floors toward -∞
    start + Duration::seconds(steps * step_s)
}
