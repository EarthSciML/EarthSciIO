//! The `format` registry (`spec/registries.md` §2) — the "reader" registry.
//!
//! Keyed by **format name**. A reader opens a cached blob and returns
//! **CF-decoded native-grid arrays keyed by the on-disk `file_variable` name**
//! plus native coordinates ([`NativeDataset`]). The decode contract — CF
//! `scale_factor`/`add_offset`, `_FillValue` → NaN, raw time axis, integer vs
//! float64 logical types — is `spec/conformance.md` §3 and is what makes the
//! decoded arrays equal across the Python / Julia / Rust tracks.
//!
//! **Hard boundary (Risk R3):** the reader applies read/decode semantics only.
//! It does **not** remap `file_variable` → schema name and does **not** apply
//! the loader's `unit_conversion` — those are ESS's job. Arrays are keyed by the
//! **on-disk** variable name.
//!
//! Component (b) ships the active `netcdf` reader ([`NetcdfReader`]). A second
//! reader (CSV/GeoTIFF/Zarr) registers under a new name **without touching the
//! [`crate::Provider`]** — exactly the extensibility invariant the three
//! registries exist to guarantee.

//! # What survives on wasm32
//!
//! Every **reader** here opens a cached blob, i.e. a path on a real filesystem,
//! so the whole reader half — and the [`Reader`] trait and [`FormatRegistry`]
//! with it, both of which name a [`Cache`](crate::Cache) — is native-only. The
//! **writer** ([`zarr_write`]) is not: it is pure codec work over a `zarrs`
//! store, and on wasm32 that store is [`OpfsStore`]. See `crate`'s module docs.

#[cfg(not(target_arch = "wasm32"))]
mod ff10;
#[cfg(not(target_arch = "wasm32"))]
mod geotiff;
#[cfg(not(target_arch = "wasm32"))]
mod netcdf;
#[cfg(not(target_arch = "wasm32"))]
mod parquet;
#[cfg(not(target_arch = "wasm32"))]
mod shapefile;
#[cfg(not(target_arch = "wasm32"))]
mod zarr;
#[cfg(all(feature = "object-store", not(target_arch = "wasm32")))]
mod zarr_object_store;
#[cfg(all(target_arch = "wasm32", feature = "opfs"))]
mod zarr_opfs;
#[cfg(not(target_arch = "wasm32"))]
mod zarr_store;
mod zarr_write;

#[cfg(not(target_arch = "wasm32"))]
pub use ff10::Ff10Reader;
#[cfg(not(target_arch = "wasm32"))]
pub use geotiff::GeoTiffReader;
#[cfg(not(target_arch = "wasm32"))]
pub use netcdf::NetcdfReader;
#[cfg(not(target_arch = "wasm32"))]
pub use parquet::ParquetReader;
#[cfg(not(target_arch = "wasm32"))]
pub use shapefile::ShapefileReader;
#[cfg(not(target_arch = "wasm32"))]
pub use zarr::ZarrReader;
#[cfg(not(target_arch = "wasm32"))]
pub use zarr_write::write_zarr_v3;
pub use zarr_write::{
    write_all_to_store, BloscProfile, CodecProfile, OutputSchema, WriteCoord, WriteVar,
    ZstdProfile, BLOSC_CHECKPOINT, BLOSC_DIAGNOSTIC, ZSTD_WASM,
};
#[cfg(all(feature = "object-store", not(target_arch = "wasm32")))]
pub(crate) use zarr_object_store::{resolve_backend, runtime};
#[cfg(all(feature = "object-store", not(target_arch = "wasm32")))]
pub use zarr_object_store::{
    array_shape_object_store, read_store_options, read_zarr_object_store,
    read_zarr_object_store_with_options, store_options_from_env, write_zarr_object_store,
    write_zarr_object_store_with_options,
};
#[cfg(all(target_arch = "wasm32", feature = "opfs"))]
pub use zarr_opfs::{read_zarr_opfs_array, write_zarr_opfs, OpfsStore};

use std::collections::{BTreeMap, HashMap};
#[cfg(not(target_arch = "wasm32"))]
use std::path::Path;
#[cfg(not(target_arch = "wasm32"))]
use std::sync::Arc;

#[cfg(not(target_arch = "wasm32"))]
use crate::cache::Cache;
use crate::error::Result;

/// Logical element type of a native array (`spec/schemas/native-field.schema.json`).
///
/// Numeric file variables become [`DType::Float64`] once CF scale/offset are
/// applied; unpacked integer file variables keep an integer logical type
/// ([`DType::Int32`]/[`DType::Int64`]); text columns are [`DType::Str`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DType {
    /// 64-bit float. CF-decoded numerics and any unit-affected read.
    Float64,
    /// 64-bit signed integer (unpacked wide integer file variable).
    Int64,
    /// 32-bit signed integer (unpacked integer file variable, e.g. a CF time axis).
    Int32,
    /// UTF-8 string (text columns from CSV/JSON readers).
    Str,
    /// Boolean.
    Bool,
}

/// A native array's values, flattened **row-major (C order)** per its `shape`.
///
/// For numeric fields, `f64::NAN` encodes a CF `_FillValue`/`missing_value`
/// cell — the corpus represents the same cell as `null` and compares it as NaN.
#[derive(Debug, Clone, PartialEq)]
pub enum ArrayData {
    /// `float64` values; `NaN` marks a masked/fill cell.
    F64(Vec<f64>),
    /// `int64` values.
    I64(Vec<i64>),
    /// `int32` values.
    I32(Vec<i32>),
    /// `string` values.
    Str(Vec<String>),
    /// `bool` values.
    Bool(Vec<bool>),
}

impl ArrayData {
    /// Number of elements (the flattened length).
    pub fn len(&self) -> usize {
        match self {
            ArrayData::F64(v) => v.len(),
            ArrayData::I64(v) => v.len(),
            ArrayData::I32(v) => v.len(),
            ArrayData::Str(v) => v.len(),
            ArrayData::Bool(v) => v.len(),
        }
    }

    /// True when the array holds no elements.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The logical element type of this array.
    pub fn dtype(&self) -> DType {
        match self {
            ArrayData::F64(_) => DType::Float64,
            ArrayData::I64(_) => DType::Int64,
            ArrayData::I32(_) => DType::Int32,
            ArrayData::Str(_) => DType::Str,
            ArrayData::Bool(_) => DType::Bool,
        }
    }

    /// Take the contiguous sub-block `[start, start + len)` as a new array.
    /// Used to slice off a leading (record/time) axis.
    fn block(&self, start: usize, len: usize) -> ArrayData {
        match self {
            ArrayData::F64(v) => ArrayData::F64(v[start..start + len].to_vec()),
            ArrayData::I64(v) => ArrayData::I64(v[start..start + len].to_vec()),
            ArrayData::I32(v) => ArrayData::I32(v[start..start + len].to_vec()),
            ArrayData::Str(v) => ArrayData::Str(v[start..start + len].to_vec()),
            ArrayData::Bool(v) => ArrayData::Bool(v[start..start + len].to_vec()),
        }
    }

    /// [`gather`] over whichever element type this array holds.
    #[cfg(not(target_arch = "wasm32"))]
    fn gathered(&self, shape: &[usize], take: &[Vec<usize>]) -> ArrayData {
        match self {
            ArrayData::F64(v) => ArrayData::F64(gather(v, shape, take)),
            ArrayData::I64(v) => ArrayData::I64(gather(v, shape, take)),
            ArrayData::I32(v) => ArrayData::I32(gather(v, shape, take)),
            ArrayData::Str(v) => ArrayData::Str(gather(v, shape, take)),
            ArrayData::Bool(v) => ArrayData::Bool(gather(v, shape, take)),
        }
    }

    /// The array's bytes as `spec/prefetch.md` §3.6 counts them for a memory
    /// budget: the element width for numbers and booleans, a string's UTF-8
    /// length for text.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn budget_bytes(&self) -> u64 {
        match self {
            ArrayData::F64(v) => 8 * v.len() as u64,
            ArrayData::I64(v) => 8 * v.len() as u64,
            ArrayData::I32(v) => 4 * v.len() as u64,
            ArrayData::Str(v) => v.iter().map(|s| s.len() as u64).sum(),
            ArrayData::Bool(v) => v.len() as u64,
        }
    }
}

/// Gather `take` out of a row-major array of `shape`, preserving the requested
/// index ORDER and repetition on every axis (a reader that sorted them fails the
/// permuted corpus case).
///
/// One gather serves the `netcdf` reader's bounding-slab reads and text fields
/// and the Provider's slicing of a union decode back into its windows
/// (`spec/prefetch.md` §2.4) — two gathers would be two chances to get the
/// row-major arithmetic subtly different in one of them, and the second one is
/// held to a BITWISE equality with the first.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn gather<T: Clone>(flat: &[T], shape: &[usize], take: &[Vec<usize>]) -> Vec<T> {
    let out_shape: Vec<usize> = take.iter().map(Vec::len).collect();
    let n: usize = out_shape.iter().product();
    let mut src_strides = vec![1usize; shape.len()];
    for i in (0..shape.len().saturating_sub(1)).rev() {
        src_strides[i] = src_strides[i + 1] * shape[i + 1];
    }
    let mut out = Vec::with_capacity(n);
    let mut idx = vec![0usize; out_shape.len()];
    for _ in 0..n {
        let mut off = 0;
        for (ax, &k) in idx.iter().enumerate() {
            off += take[ax][k] * src_strides[ax];
        }
        out.push(flat[off].clone());
        for ax in (0..idx.len()).rev() {
            idx[ax] += 1;
            if idx[ax] < out_shape[ax] {
                break;
            }
            idx[ax] = 0;
        }
    }
    out
}

/// One native-grid array, keyed by its on-disk `file_variable` name and validated
/// against `spec/schemas/native-field.schema.json`. The native grid is the
/// loader's own grid — **regrid is ESD/C4's job, not the reader's.**
#[derive(Debug, Clone)]
pub struct NativeField {
    /// Logical element type.
    pub dtype: DType,
    /// Ordered dimension names (e.g. `[time, latitude, longitude]`).
    pub dims: Vec<String>,
    /// Length of each dimension, in `dims` order.
    pub shape: Vec<usize>,
    /// Flattened values (row-major per `shape`).
    pub data: ArrayData,
    /// The native fill sentinel if one survives into the array, else `None`. CF
    /// `_FillValue` cells are folded into `NaN`, so a decoded field reports `None`.
    pub fill_value: Option<f64>,
}

impl NativeField {
    /// Slice off the leading dimension at index `i`, returning the sub-array with
    /// one fewer dimension. This is how a [`crate::Provider`] selects the current
    /// record (time slice) from a multi-record file at a cadence boundary.
    ///
    /// Errors if the field is scalar (no leading dim) or `i` is out of range.
    pub fn select_leading(&self, i: usize) -> Result<NativeField> {
        let Some((&lead, rest)) = self.shape.split_first() else {
            return Err(crate::Error::Format {
                format: "native".to_string(),
                detail: "cannot select a record from a scalar field".to_string(),
            });
        };
        if i >= lead {
            return Err(crate::Error::Format {
                format: "native".to_string(),
                detail: format!("record index {i} out of range (leading dim = {lead})"),
            });
        }
        let block: usize = rest.iter().product();
        Ok(NativeField {
            dtype: self.dtype,
            dims: self.dims[1..].to_vec(),
            shape: rest.to_vec(),
            data: self.data.block(i * block, block),
            fill_value: self.fill_value,
        })
    }

    /// This field with every axis named in `picks` narrowed to the given
    /// POSITIONS along it (in order, repeats kept); other axes whole. A field
    /// carrying none of the named dimensions is returned unchanged.
    ///
    /// The positions are trusted to be in range — the caller resolved them
    /// against this field's own lengths.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn take_by_name(&self, picks: &HashMap<String, Vec<usize>>) -> NativeField {
        if !self.dims.iter().any(|d| picks.contains_key(d)) {
            return self.clone();
        }
        let take: Vec<Vec<usize>> = self
            .dims
            .iter()
            .zip(&self.shape)
            .map(|(d, &len)| picks.get(d).cloned().unwrap_or_else(|| (0..len).collect()))
            .collect();
        NativeField {
            dtype: self.dtype,
            dims: self.dims.clone(),
            shape: take.iter().map(Vec::len).collect(),
            data: self.data.gathered(&self.shape, &take),
            fill_value: self.fill_value,
        }
    }
}

/// A native coordinate: its array plus the CF metadata that travels with a time
/// axis. `units`/`calendar` are carried **verbatim** — decoding the time axis to
/// wall-clock instants is ESS's job, not the reader's (`spec/conformance.md` §3).
#[derive(Debug, Clone)]
pub struct Coord {
    /// The coordinate's native array.
    pub field: NativeField,
    /// CF `units` attribute (e.g. `"hours since 2018-11-08 00:00:00"`), if present.
    pub units: Option<String>,
    /// CF `calendar` attribute (e.g. `"gregorian"`), if present.
    pub calendar: Option<String>,
}

/// A decoded native dataset: data variables keyed by on-disk name, plus the
/// native coordinates of the loader's grid.
#[derive(Debug, Clone, Default)]
pub struct NativeDataset {
    /// Data variables keyed by on-disk `file_variable` name.
    pub variables: HashMap<String, NativeField>,
    /// Native coordinates keyed by name (a NetCDF coordinate variable is a
    /// variable whose name matches a dimension).
    pub coords: HashMap<String, Coord>,
}

#[cfg(not(target_arch = "wasm32"))]
impl NativeDataset {
    /// The dataset's array bytes, variables and coordinates together, as
    /// `spec/prefetch.md` §3.6 counts a slot against `max_bytes`.
    pub(crate) fn budget_bytes(&self) -> u64 {
        self.variables
            .values()
            .map(|f| f.data.budget_bytes())
            .chain(self.coords.values().map(|c| c.field.data.budget_bytes()))
            .sum()
    }

    /// Every variable and coordinate narrowed by [`NativeField::take_by_name`].
    pub(crate) fn take_by_name(&self, picks: &HashMap<String, Vec<usize>>) -> NativeDataset {
        NativeDataset {
            variables: self
                .variables
                .iter()
                .map(|(k, f)| (k.clone(), f.take_by_name(picks)))
                .collect(),
            coords: self
                .coords
                .iter()
                .map(|(k, c)| {
                    let field = c.field.take_by_name(picks);
                    (k.clone(), Coord { field, units: c.units.clone(), calendar: c.calendar.clone() })
                })
                .collect(),
        }
    }
}

/// Which records/rows/cells of a blob to read: the decode-time `select` of
/// `spec/registries.md` §2.2. `All` reads the whole blob — the conformance
/// corpus default (`select.all_records` / `select.all_rows`).
///
/// A window has **two spellings over one selector vocabulary** ([`AxisSelect`]:
/// 0-based, half-open, bounds-checked, order and repetition kept, a dimension
/// never dropped, empty legal, a non-`All` time axis refused):
///
/// - [`Orthogonal`](Selection::Orthogonal) — the wire `{"axes": [...]}`: one
///   selector per POSITION, over the file-order dims of every array whose rank
///   equals the axis count;
/// - [`Named`](Selection::Named) — the wire `{"dims": {...}}`
///   (`spec/prefetch.md` §2.1): one selector per dimension NAME, applied to every
///   decoded field and coordinate carrying that dimension whatever its rank, an
///   unnamed dimension `All`. It exists because no positional window can cover a
///   provider whose fields differ in rank or carry a staggered dimension — three
///   axes bind position 2 to `lon` for `ps(time, lat, lon)` and to `lon_edge` for
///   `u(time, lat, lon_edge)`. [`DimWindow`] builds one.
///
/// "Exactly one of `axes` and `dims`" is a property of the TYPE here: a value is
/// one variant. [`Selection::from_json`] is where the wire form carrying both is
/// refused.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum Selection {
    /// Read every record/row in the blob.
    #[default]
    All,
    /// A per-axis orthogonal selection. One [`AxisSelect`] per array dimension;
    /// applied to arrays whose rank matches the axis count.
    Orthogonal(Vec<AxisSelect>),
    /// A named-dimension selection (`spec/prefetch.md` §2.1): each entry applies
    /// to every field and coordinate of the read that carries the dimension.
    /// Naming a dimension no field of the read carries is an error, never a
    /// window that silently means nothing.
    Named(BTreeMap<String, AxisSelect>),
}

impl Selection {
    /// Parse the wire form of a `select` (`spec/registries.json`, the `select`
    /// reader option): `{"axes": [<axis>, ...]}` or `{"dims": {"<dim>": <axis>,
    /// ...}}`, where `<axis>` is `"all"`, `{"indices": [i, ...]}` or
    /// `{"slice": [start, stop, step?]}`.
    ///
    /// A `select` carrying BOTH `axes` and `dims` is an error (`spec/prefetch.md`
    /// §2.1) — this is the one place that rule can be broken, since a
    /// [`Selection`] value is one variant. So is a negative or non-integer index:
    /// the vocabulary is 0-based and never wraps, and a `usize` cannot carry the
    /// mistake past here. The corpus's "everything" markers (`all_records`,
    /// `all_rows`, `all_cells`) and an empty object read as [`Selection::All`];
    /// any other key is an error rather than a window quietly dropped.
    pub fn from_json(v: &serde_json::Value) -> Result<Selection> {
        let err = |detail: String| crate::Error::Format { format: "select".to_string(), detail };
        let obj = v
            .as_object()
            .ok_or_else(|| err(format!("a select must be a JSON object, got {v}")))?;
        for k in obj.keys() {
            if !matches!(k.as_str(), "axes" | "dims" | "all_records" | "all_rows" | "all_cells") {
                return Err(err(format!(
                    "unknown select key {k:?}; a select is {{\"axes\": [...]}} or \
                     {{\"dims\": {{...}}}}"
                )));
            }
        }
        match (obj.get("axes"), obj.get("dims")) {
            (Some(_), Some(_)) => Err(err(
                "a select carries both `axes` and `dims`; it must name its axes one way, \
                 positionally or by dimension name, not both"
                    .to_string(),
            )),
            (Some(axes), None) => {
                let axes = axes
                    .as_array()
                    .ok_or_else(|| err(format!("select.axes must be an array, got {axes}")))?;
                Ok(Selection::Orthogonal(
                    axes.iter().map(AxisSelect::from_json).collect::<Result<_>>()?,
                ))
            }
            (None, Some(dims)) => {
                let dims = dims
                    .as_object()
                    .ok_or_else(|| err(format!("select.dims must be an object, got {dims}")))?;
                let mut out = BTreeMap::new();
                for (name, axis) in dims {
                    out.insert(name.clone(), AxisSelect::from_json(axis)?);
                }
                Ok(Selection::Named(out))
            }
            (None, None) => Ok(Selection::All),
        }
    }
}

/// A named-dimension window, built one dimension at a time — the Rust spelling
/// of `spec/prefetch.md` §2.2's `DimWindow` (the run window `(start, end)`
/// already owns the name `Window`).
///
/// ```
/// # #[cfg(not(target_arch = "wasm32"))]
/// # {
/// use earthsciio::{DimWindow, Selection};
/// // A halo across the periodic longitude seam, the first level only, every
/// // latitude in [120, 218): one window for 2-D and 3-D fields alike.
/// let w = DimWindow::new()
///     .range("lat", 120, 218)
///     .indices("lon", vec![1151, 0, 1])
///     .step_range("lev", 0, 1, 1);
/// let sel: Selection = w.into();
/// assert!(matches!(sel, Selection::Named(_)));
/// # }
/// ```
///
/// Indices are 0-based, ranges half-open — the wire vocabulary in every binding,
/// Julia's included. Naming a dimension twice keeps the last selector.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DimWindow {
    dims: BTreeMap<String, AxisSelect>,
}

impl DimWindow {
    /// An empty window: selects nothing away (`{"dims": {}}`).
    pub fn new() -> Self {
        Self::default()
    }

    /// `[start, stop)` along `dim` (the wire `{"slice": [start, stop]}`).
    pub fn range(self, dim: impl Into<String>, start: usize, stop: usize) -> Self {
        self.step_range(dim, start, stop, 1)
    }

    /// `[start, stop)` by `step` along `dim` (`{"slice": [start, stop, step]}`).
    /// A `step` of 0 is carried as given and refused by the read, as on the wire.
    pub fn step_range(self, dim: impl Into<String>, start: usize, stop: usize, step: usize) -> Self {
        self.axis(dim, AxisSelect::Range { start, stop, step })
    }

    /// Exactly `indices` along `dim`, in that order, repeats kept — the spelling
    /// of a window across a periodic seam (`[1150, 1151, 0, 1]`).
    pub fn indices(self, dim: impl Into<String>, indices: impl IntoIterator<Item = usize>) -> Self {
        self.axis(dim, AxisSelect::Indices(indices.into_iter().collect()))
    }

    /// The whole of `dim`, stated explicitly. Equivalent to leaving it unnamed,
    /// except that a dimension no field carries is then an error.
    pub fn all(self, dim: impl Into<String>) -> Self {
        self.axis(dim, AxisSelect::All)
    }

    /// Any [`AxisSelect`] along `dim`.
    pub fn axis(mut self, dim: impl Into<String>, select: AxisSelect) -> Self {
        self.dims.insert(dim.into(), select);
        self
    }

    /// The selectors by dimension name.
    pub fn dims(&self) -> &BTreeMap<String, AxisSelect> {
        &self.dims
    }
}

impl From<DimWindow> for Selection {
    fn from(w: DimWindow) -> Self {
        Selection::Named(w.dims)
    }
}

/// A **record pushdown**: the records of one dimension the cadence owner has
/// already chosen (`spec/registries.md` §2.1, `spec/conformance.md` "NetCDF
/// decode notes").
///
/// This is the other half of the rule that makes a [`Selection`] refuse a time
/// axis. The refusal says the reader may not *choose* records, not that every
/// record must be decoded: `Records` is the separate channel through which
/// whoever owns the cadence states the records it wants, in the file's **own**
/// 0-based numbering. The reader is told the records and never the cadence — it
/// does no modular arithmetic, knows nothing of `records_per_sample`, and
/// materialises exactly `indices` of `dim` (and of `dim`'s own coordinate
/// variable) **in the order given**.
///
/// - **Duplicates are legal.** The end-of-data bracket `[last, last]` is one, and
///   a reader that de-duplicated would hand back one record where two were asked
///   for.
/// - **An index outside `0..len` is an error**, never a wrapped read: locating a
///   cadence tick inside its file is the caller's job. (A *negative* index is
///   unrepresentable here, which is the same rule the `usize` of
///   [`AxisSelect::Indices`] states.)
/// - **An empty `indices` is an error**, not a whole-axis read.
/// - `Records` and a `Selection` may be combined but **must not both narrow
///   `dim`**.
///
/// [`Reader::dim_length`] is the metadata-only read that makes `indices`
/// computable before the decode they are meant to narrow.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Records {
    /// The dimension whose records are being chosen (e.g. `"time"`).
    pub dim: String,
    /// Absolute, file-local, 0-based record indices, honoured in this order.
    pub indices: Vec<usize>,
}

impl Records {
    /// A record pushdown of `indices` along `dim`.
    pub fn new(dim: impl Into<String>, indices: Vec<usize>) -> Self {
        Self {
            dim: dim.into(),
            indices,
        }
    }
}

/// One array dimension's selector in an orthogonal [`Selection::Orthogonal`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AxisSelect {
    /// The whole dimension.
    All,
    /// An explicit, possibly non-contiguous, ordered index list. The output
    /// length along this axis is `indices.len()` in the given order.
    Indices(Vec<usize>),
    /// A strided range `[start, stop)` by `step` (`step >= 1`).
    Range {
        /// Inclusive start.
        start: usize,
        /// Exclusive stop.
        stop: usize,
        /// Stride (>= 1).
        step: usize,
    },
}

impl AxisSelect {
    /// Parse one wire selector: `"all"`, `{"indices": [i, ...]}` or
    /// `{"slice": [start, stop, step?]}` (`step` default 1). A negative or
    /// non-integer index is an error, never a wrap.
    pub fn from_json(v: &serde_json::Value) -> Result<AxisSelect> {
        let err = |detail: String| crate::Error::Format { format: "select".to_string(), detail };
        let index = |x: &serde_json::Value| -> Result<usize> {
            x.as_u64().map(|i| i as usize).ok_or_else(|| {
                err(format!(
                    "selector index {x} is not a non-negative integer; the vocabulary is \
                     0-based and an out-of-range or negative index is an error, never a wrap"
                ))
            })
        };
        if v.as_str() == Some("all") {
            return Ok(AxisSelect::All);
        }
        if let Some(obj) = v.as_object() {
            if obj.len() == 1 {
                if let Some(idx) = obj.get("indices").and_then(serde_json::Value::as_array) {
                    return Ok(AxisSelect::Indices(idx.iter().map(index).collect::<Result<_>>()?));
                }
                if let Some(s) = obj.get("slice").and_then(serde_json::Value::as_array) {
                    if !(2..=3).contains(&s.len()) {
                        return Err(err(format!("slice must be [start, stop, step?], got {v}")));
                    }
                    let step = match s.get(2) {
                        Some(x) => index(x)?,
                        None => 1,
                    };
                    return Ok(AxisSelect::Range { start: index(&s[0])?, stop: index(&s[1])?, step });
                }
            }
        }
        Err(err(format!(
            "unrecognised selector {v}; expected \"all\", {{\"indices\": [...]}} or \
             {{\"slice\": [start, stop, step?]}}"
        )))
    }

    /// The ordered indices this selector names, WITHOUT a dimension length:
    /// `None` for [`All`](AxisSelect::All). What a union of windows is computed
    /// from before the read that bounds-checks it (`spec/prefetch.md` §2.4); the
    /// step rule is [`resolve_in`](AxisSelect::resolve_in)'s.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn unbounded_indices(&self, format: &str) -> Result<Option<Vec<usize>>> {
        match self {
            AxisSelect::All => Ok(None),
            AxisSelect::Indices(v) => Ok(Some(v.clone())),
            AxisSelect::Range { start, stop, step } => {
                if *step < 1 {
                    return Err(crate::Error::Format {
                        format: format.to_string(),
                        detail: format!("slice step must be >= 1, got {step}"),
                    });
                }
                Ok(Some((*start..*stop).step_by(*step).collect()))
            }
        }
    }

    /// Resolve this selector to its ordered list of global indices over a
    /// dimension of length `dim_len`, reporting errors as the `zarr` format's.
    pub fn resolve(&self, dim_len: usize) -> Result<Vec<usize>> {
        self.resolve_in(dim_len, "zarr")
    }

    /// [`resolve`](AxisSelect::resolve), tagging its errors with the reader that
    /// asked. The selector vocabulary is shared by every reader that can honour a
    /// `select` — the store-backed `zarr` one and the whole-file `netcdf` one —
    /// so an out-of-range index must not report the wrong format's name.
    pub fn resolve_in(&self, dim_len: usize, format: &str) -> Result<Vec<usize>> {
        match self {
            AxisSelect::All => Ok((0..dim_len).collect()),
            AxisSelect::Indices(v) => {
                for &g in v {
                    if g >= dim_len {
                        return Err(crate::Error::Format {
                            format: format.to_string(),
                            detail: format!("index {g} out of range for dimension length {dim_len}"),
                        });
                    }
                }
                Ok(v.clone())
            }
            AxisSelect::Range { start, stop, step } => {
                if *step < 1 {
                    return Err(crate::Error::Format {
                        format: format.to_string(),
                        detail: format!("slice step must be >= 1, got {step}"),
                    });
                }
                let out: Vec<usize> = (*start..*stop).step_by(*step).collect();
                // A range is bounds-checked exactly like an explicit index list:
                // a `[start, stop)` reaching past the dimension is an ERROR, never
                // a silent clamp. The three tracks cannot agree on a clamp (numpy
                // clamps; NCDatasets and the `netcdf-reader` slice API do not), and
                // an over-long window that quietly returns fewer cells than asked
                // for is a wrong number. (`usize` already rules out a negative
                // start, which is where Python's clamp becomes a wrap-around.)
                if let Some(&g) = out.last() {
                    if g >= dim_len {
                        return Err(crate::Error::Format {
                            format: format.to_string(),
                            detail: format!(
                                "slice [{start}, {stop}) by {step} reaches index {g}, out of \
                                 range for dimension length {dim_len}"
                            ),
                        });
                    }
                }
                Ok(out)
            }
        }
    }
}

/// A format reader (`spec/registries.md` §2): opens a cached blob and returns
/// CF-decoded native arrays. Keyed by format name in the [`FormatRegistry`].
///
/// Native-only: a reader's input is a path into the content-addressed cache.
#[cfg(not(target_arch = "wasm32"))]
pub trait Reader: Send + Sync {
    /// Format names this reader handles (e.g. `["netcdf"]`).
    fn formats(&self) -> &'static [&'static str];

    /// Extension sniff hints (e.g. `["nc", "nc4", "cdf"]`). Format selection is
    /// by the loader's declared format, **never** by trusting the blob suffix
    /// alone; these are advisory hints only.
    fn extensions(&self) -> &'static [&'static str];

    /// Decode `blob_path` into native arrays.
    ///
    /// `variables` lists the on-disk `file_variable` names to read; an empty
    /// slice reads every data variable. `select` chooses which records/rows.
    /// Native coordinates are always returned (the grid the arrays live on).
    fn read_native(
        &self,
        blob_path: &Path,
        variables: &[String],
        select: &Selection,
    ) -> Result<NativeDataset>;

    /// [`read_native`](Reader::read_native) under a [`Records`] pushdown: the
    /// records of one dimension the cadence owner has already chosen.
    ///
    /// `records = None` is exactly [`read_native`](Reader::read_native).
    /// `Some(_)` on a reader that does not honour it is an **error** rather than
    /// a whole-axis read — the caller asked for two of a file's twenty-four
    /// records and would otherwise be handed all twenty-four with no sign that
    /// its selection was dropped, which the tick→record slicing downstream would
    /// then index wrongly. Callers gate on
    /// [`supports_records`](Reader::supports_records).
    ///
    /// The default implementation is that error, so every existing reader keeps
    /// its current behaviour and none silently ignores a pushdown.
    fn read_native_records(
        &self,
        blob_path: &Path,
        variables: &[String],
        select: &Selection,
        records: Option<&Records>,
    ) -> Result<NativeDataset> {
        match records {
            None => self.read_native(blob_path, variables, select),
            Some(r) => Err(crate::Error::Format {
                format: self.formats().first().copied().unwrap_or("native").to_string(),
                detail: format!(
                    "reader does not honour a `records` pushdown, but one was asked \
                     for along dimension '{}'; read the whole record axis and slice \
                     on the caller's side instead",
                    r.dim
                ),
            }),
        }
    }

    /// Whether this reader honours a [`Records`] pushdown in
    /// [`read_native_records`](Reader::read_native_records). Default `false`;
    /// mirrors the Julia/Python declaration of `records` as a reader option
    /// (`spec/registries.md` §2.1), which is what tells a caller the option is
    /// real in THIS track rather than only in the spec.
    fn supports_records(&self) -> bool {
        false
    }

    /// The length of dimension `dim` in the blob at `blob_path`, read from the
    /// file's METADATA — the header, never an array (`spec/registries.md` §2.3).
    ///
    /// `None` when the reader cannot answer (the default every reader inherits)
    /// or when the blob has no such dimension; a caller must treat that as "read
    /// whole and slice on my own side" rather than as an error. The whole-file
    /// counterpart of [`array_shape`](Reader::array_shape), and what makes a
    /// [`Records`] pushdown expressible out of process at all: the records a
    /// cadence owner wants are a function of the file's length along the record
    /// axis, so that length has to be known before the decode the selection is
    /// meant to narrow.
    fn dim_length(&self, _blob_path: &Path, _dim: &str) -> Result<Option<usize>> {
        Ok(None)
    }

    /// Whether this reader is **store-backed**: its source is not one fetchable
    /// blob but a directory-like store (a Zarr v2 store, whose `.zarray`/
    /// `.zattrs`/chunks are each their own object). Default `false`; whole-file
    /// readers (`netcdf`/`geotiff`) inherit it unchanged.
    fn store_backed(&self) -> bool {
        false
    }

    /// Whether this reader can honour a per-axis orthogonal [`Selection`] at read
    /// time **without materialising the whole array** — projection pushdown. The
    /// [`crate::Provider`] surfaces this so a caller can decide whether to push a
    /// projection down or read whole and slice itself. Mirrors the Julia/Python
    /// `supports_selection` trait. Default `false`.
    ///
    /// It says nothing about what is FETCHED; pair it with
    /// [`store_backed`](Reader::store_backed) for that, because the two cases are
    /// genuinely different:
    ///
    /// * store-backed + `supports_selection` ([`ZarrReader`]): the selection
    ///   decides which chunk OBJECTS are downloaded, so it shrinks the transfer;
    /// * whole-file + `supports_selection` (the `netcdf` reader): the same blob is
    ///   fetched under the same cache key and only the requested hyperslab is
    ///   materialised, so it shrinks the decode and the resident arrays.
    fn supports_selection(&self) -> bool {
        false
    }

    /// Decode a store at `base_url` into native arrays, fetching each object it
    /// needs through `cache`. Only called by the Provider when
    /// [`store_backed`](Reader::store_backed) is `true` (default: an error).
    fn read_store(
        &self,
        _cache: Arc<Cache>,
        _base_url: &str,
        _variables: &[String],
        _select: &Selection,
    ) -> Result<NativeDataset> {
        Err(crate::Error::Format {
            format: "native".to_string(),
            detail: "reader is not store-backed".to_string(),
        })
    }

    /// The full (dims-order) shape of on-disk array `var` in the store at
    /// `base_url`, learned by fetching ONLY that array's metadata (never a chunk)
    /// — a lightweight honour/refuse probe for projection-pushdown decisions.
    /// `None` for a whole-file reader (shape unknowable without reading the blob);
    /// a store-backed reader ([`ZarrReader`]) overrides to read the `.zarray`.
    fn array_shape(
        &self,
        _cache: Arc<Cache>,
        _base_url: &str,
        _var: &str,
    ) -> Result<Option<Vec<usize>>> {
        Ok(None)
    }

    /// A copy of this reader configured by the loader's declared
    /// [`DataSource::reader_options`](crate::DataSource::reader_options) — the
    /// Rust spelling of the Python/Julia `reader_kwargs`, and the seam that
    /// lets a DOCUMENT (rather than a caller with a hand-built registry) say
    /// how a format is decoded: the FF10 zip member glob, the asserted header
    /// row, a GeoTIFF band naming, and so on.
    ///
    /// `Ok(None)` (the default for an empty option set) means "use me as
    /// registered". The default implementation **rejects** a non-empty option
    /// set rather than ignoring it: a mis-typed option that silently does
    /// nothing is the failure mode this seam exists to prevent.
    fn configured(
        &self,
        options: &serde_json::Map<String, serde_json::Value>,
    ) -> Result<Option<Arc<dyn Reader>>> {
        if options.is_empty() {
            return Ok(None);
        }
        let mut keys: Vec<&str> = options.keys().map(String::as_str).collect();
        keys.sort_unstable();
        Err(crate::Error::Format {
            format: self.formats().first().copied().unwrap_or("native").to_string(),
            detail: format!("reader takes no reader_options, but the loader declares {keys:?}"),
        })
    }
    /// Whether this reader can read its store **without the cache** — straight
    /// from object storage, writing nothing to disk. Default `false`.
    ///
    /// A reader that answers `true` must honour
    /// [`read_store_direct`](Reader::read_store_direct); the [`crate::Provider`]
    /// consults this before routing a
    /// [`StoreAccess::Direct`](crate::StoreAccess::Direct) loader, so an
    /// unsupported combination is a named error rather than a silent fallback to
    /// the cache (which would restore exactly the disk cost the caller asked to
    /// avoid, invisibly).
    fn supports_direct_read(&self) -> bool {
        false
    }

    /// [`read_store`](Reader::read_store) with **no cache**: fetch each object
    /// the selection needs straight from the store and decode it in memory.
    ///
    /// `options` are backend config keys (`object_store`'s own — `endpoint`,
    /// `region`, credentials, …); an empty slice means "environment defaults".
    /// Selection pushdown is unchanged — a direct read that lost it would fetch
    /// vastly more than the cached read it replaces.
    ///
    /// Only called by the Provider when
    /// [`supports_direct_read`](Reader::supports_direct_read) is `true`
    /// (default: an error).
    fn read_store_direct(
        &self,
        _base_url: &str,
        _variables: &[String],
        _select: &Selection,
        _options: &[(String, String)],
    ) -> Result<NativeDataset> {
        Err(crate::Error::Format {
            format: "native".to_string(),
            detail: "reader does not support direct (uncached) store reads".to_string(),
        })
    }

    /// [`array_shape`](Reader::array_shape) with no cache — the direct-read twin
    /// of the pushdown probe. `None` when the reader has no direct path.
    fn array_shape_direct(
        &self,
        _base_url: &str,
        _var: &str,
        _options: &[(String, String)],
    ) -> Result<Option<Vec<usize>>> {
        Ok(None)
    }
}

/// Format-name → reader lookup (`spec/registries.md` §2). Adding a reader is a
/// registration, never a [`crate::Provider`] edit — the Provider resolves the
/// reader by the loader's declared format name at runtime.
#[cfg(not(target_arch = "wasm32"))]
#[derive(Default, Clone)]
pub struct FormatRegistry {
    by_name: HashMap<String, Arc<dyn Reader>>,
}

#[cfg(not(target_arch = "wasm32"))]
impl FormatRegistry {
    /// An empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Registry with the built-in **active** readers: `netcdf` (NetCDF-3
    /// classic) and `geotiff` (single-/multi-band raster). Further readers
    /// (csv/zarr) register the same way.
    pub fn with_builtins() -> Self {
        let mut r = Self::new();
        r.register(Arc::new(NetcdfReader::new()));
        r.register(Arc::new(GeoTiffReader::new()));
        // FF10 point (SMOKE/Emissions.jl FF10_POINT): the DEFAULT reader
        // (member=None) decodes a bare `.csv` — the conformance/crosscheck path.
        // A member-configured reader for the zipped tutorial input is injected via
        // `Provider::with_formats` (no Provider/trait edit; see ff10.rs docs).
        r.register(Arc::new(Ff10Reader::new()));
        // shapefile (ESRI feature table): one row per PART, `geometry`
        // [index, vertex, xy] padded by repeating the final vertex, the record's
        // stored bbox and the `.dbf` columns. The DEFAULT reader takes the single
        // `.shp` of a zip (or a bare `.shp`); `member`/`numeric_columns` reach it
        // from a document through `Reader::configured`.
        r.register(Arc::new(ShapefileReader::new()));
        // parquet (columnar table): every column becomes a rank-1 field over
        // `index`, keyed by its on-disk column name, with the requested
        // `variables` pushed down as a projection so only those column chunks
        // are read. `float_columns`/`null_int`/`null_string` reach it from a
        // document through `Reader::configured`.
        r.register(Arc::new(ParquetReader::new()));
        r.register(Arc::new(ZarrReader::new()));
        r
    }

    /// Register a reader under each of its format names.
    pub fn register(&mut self, reader: Arc<dyn Reader>) -> &mut Self {
        for name in reader.formats() {
            self.by_name.insert((*name).to_string(), reader.clone());
        }
        self
    }

    /// Look up the reader for a format name.
    pub fn get(&self, name: &str) -> Option<Arc<dyn Reader>> {
        self.by_name.get(name).cloned()
    }

    /// All registered format names.
    pub fn registered(&self) -> Vec<String> {
        self.by_name.keys().cloned().collect()
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;

    #[test]
    fn builtins_cover_netcdf() {
        let r = FormatRegistry::with_builtins();
        assert!(r.get("netcdf").is_some());
        assert!(r.get("zarr").is_some()); // store-backed zarr reader is active
        assert!(r.get("zarr").unwrap().store_backed());
        assert!(!r.get("netcdf").unwrap().store_backed());
    }

    #[test]
    fn a_second_reader_plugs_in_without_touching_the_provider() {
        // Proves the §2 invariant structurally: a new format is a registration.
        struct DummyReader;
        impl Reader for DummyReader {
            fn formats(&self) -> &'static [&'static str] {
                &["dummy"]
            }
            fn extensions(&self) -> &'static [&'static str] {
                &["dmy"]
            }
            fn read_native(&self, _: &Path, _: &[String], _: &Selection) -> Result<NativeDataset> {
                Ok(NativeDataset::default())
            }
        }
        let mut r = FormatRegistry::with_builtins();
        r.register(Arc::new(DummyReader));
        assert!(r.get("dummy").is_some());
        assert!(r.get("netcdf").is_some()); // existing readers untouched
    }

    #[test]
    fn select_leading_drops_the_record_axis() {
        let f = NativeField {
            dtype: DType::Float64,
            dims: vec!["time".into(), "lat".into(), "lon".into()],
            shape: vec![2, 2, 2],
            data: ArrayData::F64(vec![0.0, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0]),
            fill_value: None,
        };
        let r0 = f.select_leading(0).unwrap();
        assert_eq!(r0.dims, vec!["lat".to_string(), "lon".to_string()]);
        assert_eq!(r0.shape, vec![2, 2]);
        assert_eq!(r0.data, ArrayData::F64(vec![0.0, 1.0, 2.0, 3.0]));
        let r1 = f.select_leading(1).unwrap();
        assert_eq!(r1.data, ArrayData::F64(vec![4.0, 5.0, 6.0, 7.0]));
        assert!(f.select_leading(2).is_err()); // out of range
    }
}
