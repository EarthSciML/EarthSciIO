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

mod geotiff;
mod netcdf;

pub use geotiff::GeoTiffReader;
pub use netcdf::NetcdfReader;

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

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

/// Which records/rows of a blob to read. `All` reads the whole blob — the
/// conformance corpus default (`select.all_records` / `select.all_rows`).
///
/// Extension point: a future variant can carry a record range or row predicate
/// without changing the [`Reader`] trait shape.
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub enum Selection {
    /// Read every record/row in the blob.
    #[default]
    All,
}

/// A format reader (`spec/registries.md` §2): opens a cached blob and returns
/// CF-decoded native arrays. Keyed by format name in the [`FormatRegistry`].
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
}

/// Format-name → reader lookup (`spec/registries.md` §2). Adding a reader is a
/// registration, never a [`crate::Provider`] edit — the Provider resolves the
/// reader by the loader's declared format name at runtime.
#[derive(Default, Clone)]
pub struct FormatRegistry {
    by_name: HashMap<String, Arc<dyn Reader>>,
}

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtins_cover_netcdf() {
        let r = FormatRegistry::with_builtins();
        assert!(r.get("netcdf").is_some());
        assert!(r.get("zarr").is_none()); // stub, not active here
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
