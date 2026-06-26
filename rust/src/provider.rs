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

use std::collections::HashMap;
use std::sync::Arc;

use time::{Duration, OffsetDateTime};

use crate::cache::{Cache, FetchRequest};
use crate::error::{Error, Result};
use crate::format::{Coord, FormatRegistry, NativeDataset, NativeField, Reader, Selection};

/// A run window `(start, end)` — half-open `[start, end)`.
pub type Window = (OffsetDateTime, OffsetDateTime);

/// The temporal nature of a loader that refreshes at a cadence (plan §4.2).
///
/// `frequency` is the cadence step (e.g. 1 hour for ERA5) — it drives the
/// refresh tstops and which record within a file is current. `file_period` is
/// the granularity of one file (e.g. 1 day) — it drives URL resolution. A file
/// holds `file_period / frequency` records along `time_dim`.
#[derive(Debug, Clone)]
pub struct LoaderTemporal {
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
}

impl LoaderTemporal {
    /// A temporal block with `frequency` cadence and `file_period` files,
    /// anchored at `start`, open-ended, slicing on the `time` dimension.
    pub fn new(start: OffsetDateTime, frequency: Duration, file_period: Duration) -> Self {
        Self {
            start,
            end: None,
            frequency,
            file_period,
            time_dim: "time".to_string(),
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
}

/// The minimal parsed `DataLoader` the Provider needs: where the bytes are, how
/// to decode them, which variables to read, and the temporal cadence (absent ⇒
/// CONST). This is the I/O-relevant projection of the ESM `DataLoader` contract —
/// the full contract (units, remap, grid family) lives upstream in ESS.
#[derive(Debug, Clone)]
pub struct DataLoader {
    /// Loader name (provenance, recorded in the cache manifest).
    pub name: String,
    /// Format-registry key selecting the reader (e.g. `"netcdf"`).
    pub format: String,
    /// On-disk `file_variable` names to read; empty ⇒ all data variables.
    pub variables: Vec<String>,
    /// Cadence (absent ⇒ CONST/static).
    pub temporal: Option<LoaderTemporal>,
    /// URL template in `time` format-description syntax (e.g.
    /// `".../era5/[year]/[month]/[year][month][day].nc"`). A template with no
    /// `[` placeholders is a literal URL (the CONST case).
    pub url_template: String,
    /// Failover mirrors sharing the same cache identity.
    pub mirrors: Vec<String>,
    /// Auth realm to fetch under (resolved by the cache's auth registry).
    pub auth_realm: Option<String>,
}

impl DataLoader {
    /// A loader named `name`, decoded by `format`, resolving `url_template`.
    /// CONST by default — add [`temporal`](DataLoader::temporal) for cadence.
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
        }
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
    pub fn temporal(mut self, temporal: LoaderTemporal) -> Self {
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
    loader: DataLoader,
    cache: Arc<Cache>,
    window: Option<Window>,
    reader: Arc<dyn Reader>,
    /// Current native-grid buffer per variable (the slice the solver reads).
    buffers: HashMap<String, NativeField>,
    /// Native coordinates of the current file.
    coords: HashMap<String, Coord>,
    /// The full decode of the currently-open file (re-sliced as the record moves).
    current_dataset: Option<NativeDataset>,
    /// The cadence anchor currently in the buffer (DISCRETE).
    current_anchor: Option<OffsetDateTime>,
    /// The file anchor currently open (avoids re-reading within a file).
    current_file_anchor: Option<OffsetDateTime>,
}

impl Provider {
    /// Bind a provider to `loader`, resolving its format reader from the built-in
    /// [`FormatRegistry`]. Errors with [`Error::UnknownFormat`] if no reader is
    /// registered for the loader's format.
    pub fn new(loader: DataLoader, cache: Arc<Cache>, window: Option<Window>) -> Result<Self> {
        Self::with_formats(loader, cache, window, &FormatRegistry::with_builtins())
    }

    /// [`Provider::new`] but resolving the reader from a caller-supplied registry
    /// — the seam that lets a new format plug in without any Provider change.
    pub fn with_formats(
        loader: DataLoader,
        cache: Arc<Cache>,
        window: Option<Window>,
        formats: &FormatRegistry,
    ) -> Result<Self> {
        let reader = formats
            .get(&loader.format)
            .ok_or_else(|| Error::UnknownFormat {
                name: loader.format.clone(),
            })?;
        Ok(Self {
            loader,
            cache,
            window,
            reader,
            buffers: HashMap::new(),
            coords: HashMap::new(),
            current_dataset: None,
            current_anchor: None,
            current_file_anchor: None,
        })
    }

    /// The native coordinates of the current grid (latitude/longitude/time, …).
    /// Populated after the first [`materialize`](Self::materialize) /
    /// [`refresh`](Self::refresh).
    pub fn coords(&self) -> &HashMap<String, Coord> {
        &self.coords
    }

    /// Materialize the loader's native arrays into the buffer.
    ///
    /// CONST: reads the single file once. DISCRETE: primes the buffer at the
    /// first cadence anchor of the window (equivalent to `refresh(window.start)`),
    /// so a caller can read an initial state before stepping.
    pub fn materialize(&mut self) -> Result<HashMap<String, NativeField>> {
        match self.loader.temporal.clone() {
            None => {
                let url = self.resolve_url(OffsetDateTime::UNIX_EPOCH)?;
                let ds = self.read_file(url)?;
                self.coords = ds.coords;
                self.buffers = ds.variables;
                Ok(self.buffers.clone())
            }
            Some(t) => {
                let first = self.lower_bound(&t);
                self.refresh(first)?;
                Ok(self.buffers.clone())
            }
        }
    }

    /// Refresh the buffer to the cadence anchor for time `t`.
    ///
    /// Returns `Some(buffer)` when the anchor changed (the solver must re-read),
    /// `None` when `t` falls in the same cadence interval as the last refresh, and
    /// `None` for a CONST loader (nothing refreshes).
    pub fn refresh(&mut self, t: OffsetDateTime) -> Result<Option<HashMap<String, NativeField>>> {
        let Some(temporal) = self.loader.temporal.clone() else {
            return Ok(None); // CONST: refresh is a no-op
        };
        let freq_s = temporal.frequency.whole_seconds();
        let file_s = temporal.file_period.whole_seconds();
        if freq_s <= 0 || file_s <= 0 {
            return Err(Error::Format {
                format: self.loader.format.clone(),
                detail: "loader cadence frequency/file_period must be positive".to_string(),
            });
        }

        let anchor = snap_down(temporal.start, t, freq_s);
        if self.current_anchor == Some(anchor) {
            return Ok(None); // same cadence interval — unchanged
        }
        let file_anchor = snap_down(temporal.start, anchor, file_s);

        // Re-read the file only when the anchor crossed a file boundary.
        if self.current_file_anchor != Some(file_anchor) {
            let url = self.resolve_url(file_anchor)?;
            let ds = self.read_file(url)?;
            self.coords = ds.coords.clone();
            self.current_dataset = Some(ds);
            self.current_file_anchor = Some(file_anchor);
        }

        let rec = ((anchor - file_anchor).whole_seconds() / freq_s) as usize;
        let ds = self
            .current_dataset
            .as_ref()
            .expect("dataset loaded above for this file anchor");

        let mut buffers = HashMap::with_capacity(ds.variables.len());
        for (name, field) in &ds.variables {
            // Slice the current record from variables on the time axis; leave
            // non-temporal variables whole.
            let is_temporal =
                field.dims.first().map(String::as_str) == Some(temporal.time_dim.as_str());
            let slice = if is_temporal {
                field.select_leading(rec)?
            } else {
                field.clone()
            };
            buffers.insert(name.clone(), slice);
        }
        self.buffers = buffers;
        self.current_anchor = Some(anchor);
        Ok(Some(self.buffers.clone()))
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
    fn lower_bound(&self, temporal: &LoaderTemporal) -> OffsetDateTime {
        match self.window {
            Some((a, _)) if a > temporal.start => a,
            _ => temporal.start,
        }
    }

    /// Resolve a file URL for an anchor. A template without `[` is a literal URL.
    fn resolve_url(&self, anchor: OffsetDateTime) -> Result<String> {
        let tmpl = &self.loader.url_template;
        if !tmpl.contains('[') {
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

    /// Fetch + decode a file into a native dataset.
    fn read_file(&self, url: String) -> Result<NativeDataset> {
        let blob = self.fetch_blob(&url)?;
        self.reader
            .read_native(&blob.path, &self.loader.variables, &Selection::All)
    }
}

/// `start + floor((t - start) / step) * step` — snap `t` down to the aligned
/// anchor at or before it. `step_s` must be positive.
fn snap_down(start: OffsetDateTime, t: OffsetDateTime, step_s: i64) -> OffsetDateTime {
    let elapsed = (t - start).whole_seconds();
    let steps = elapsed.div_euclid(step_s); // floors toward -∞
    start + Duration::seconds(steps * step_s)
}
