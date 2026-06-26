//! Pure-Rust **NetCDF-3 classic** reader (CDF-1 / CDF-2) behind the `format`
//! registry. Decode parity is `spec/conformance.md` §3.
//!
//! # Why pure-Rust, not the `netcdf` C-crate
//!
//! The bead asks for "the netcdf crate"; this delivers the same thing — a
//! `netcdf` reader behind the `format` registry — **without** an FFI build,
//! because the environment and the architecture demand it:
//!
//! - **No system libnetcdf and no libclang** here, and the crate deliberately
//!   avoids a C/bindgen toolchain (see `Cargo.toml`: rustls is chosen precisely
//!   so the build needs "a C compiler alone, no clang/bindgen"). The `netcdf`
//!   crate would force either a system lib that is absent or a multi-minute
//!   vendored HDF5-from-source `static` build — fragile on the refinery CI.
//! - The locked epic decision is **"NOT a Rust+FFI core"**: idiomatic per
//!   language, hermetic offline conformance. The conformance corpus is
//!   `NETCDF3_CLASSIC` (a small, fully specified XDR/big-endian format), so a
//!   focused classic reader decodes every committed case hermetically and builds
//!   wherever the core builds.
//!
//! NetCDF-4 / HDF5 (and CDF-5) are **out of scope here on purpose**: they land
//! later as a separate reader registered under the **same** `netcdf` format name
//! — zero [`crate::Provider`] change, which is the whole point of the registry.
//! A NetCDF-4/HDF5 or CDF-5 blob is rejected with a clear [`Error::Format`].
//!
//! Format reference: Unidata "NetCDF Classic Format Specification".

use std::collections::{HashMap, HashSet};
use std::path::Path;

use crate::error::{Error, Result};

use super::{ArrayData, Coord, DType, NativeDataset, NativeField, Reader, Selection};

// Component tags (`spec`: tag values that introduce a non-empty list).
const NC_DIMENSION: u32 = 0x0A;
const NC_VARIABLE: u32 = 0x0B;
const NC_ATTRIBUTE: u32 = 0x0C;

// External data types (`nc_type`).
const NC_BYTE: u32 = 1;
const NC_CHAR: u32 = 2;
const NC_SHORT: u32 = 3;
const NC_INT: u32 = 4;
const NC_FLOAT: u32 = 5;
const NC_DOUBLE: u32 = 6;

// `numrecs` sentinel for a stream-written file: the record count is computed
// from the file size instead of read from the header.
const STREAMING: u32 = 0xFFFF_FFFF;

/// The active `netcdf` reader: pure-Rust NetCDF-3 classic decode.
#[derive(Debug, Default, Clone, Copy)]
pub struct NetcdfReader;

impl NetcdfReader {
    /// Construct the reader.
    pub fn new() -> Self {
        Self
    }
}

impl Reader for NetcdfReader {
    fn formats(&self) -> &'static [&'static str] {
        &["netcdf"]
    }

    fn extensions(&self) -> &'static [&'static str] {
        &["nc", "nc4", "cdf"]
    }

    fn read_native(
        &self,
        blob_path: &Path,
        variables: &[String],
        _select: &Selection,
    ) -> Result<NativeDataset> {
        // Selection::All is the only variant today; the whole (tiny) blob is read.
        let bytes =
            std::fs::read(blob_path).map_err(|e| Error::io(Some(blob_path.to_path_buf()), e))?;
        decode(&bytes, variables)
    }
}

/// Decode a NetCDF-3 classic byte buffer into native arrays.
fn decode(data: &[u8], variables: &[String]) -> Result<NativeDataset> {
    let header = Header::parse(data)?;

    let dim_names: HashSet<&str> = header.dims.iter().map(|d| d.name.as_str()).collect();
    let want: HashSet<&str> = variables.iter().map(String::as_str).collect();

    let mut out = NativeDataset::default();
    for var in &header.vars {
        let is_coord = dim_names.contains(var.name.as_str());
        // A coordinate variable (name == a dimension) is always returned — it is
        // the native grid the data lives on. Data variables honor the filter.
        if !is_coord && !want.is_empty() && !want.contains(var.name.as_str()) {
            continue;
        }

        let layout = header.layout(var)?;
        let raw = header.read_raw(data, var, &layout)?;
        let field = decode_field(var, raw, &layout)?;

        if is_coord {
            out.coords.insert(
                var.name.clone(),
                Coord {
                    field,
                    units: var.att_text("units"),
                    calendar: var.att_text("calendar"),
                },
            );
        } else {
            out.variables.insert(var.name.clone(), field);
        }
    }
    Ok(out)
}

/// Apply the CF decode contract (`spec/conformance.md` §3) to a variable's raw
/// values, producing its native field.
fn decode_field(var: &Var, raw: Raw, layout: &Layout) -> Result<NativeField> {
    let scale = var.att_scalar("scale_factor");
    let add = var.att_scalar("add_offset");
    let fill = var.att_scalar("_FillValue");
    let missing = var.att_scalar("missing_value");
    let is_fill = |v: f64| fill.is_some_and(|f| v == f) || missing.is_some_and(|m| v == m);

    // Packed (scale_factor and/or add_offset) → float64. The math is done in
    // double regardless of the on-disk integer width; the fill check compares the
    // RAW value *before* unpacking; a masked cell becomes NaN.
    if scale.is_some() || add.is_some() {
        let s = scale.unwrap_or(1.0);
        let o = add.unwrap_or(0.0);
        let data: Vec<f64> = raw
            .iter_f64()
            .map(|v| if is_fill(v) { f64::NAN } else { v * s + o })
            .collect();
        return Ok(NativeField {
            dtype: DType::Float64,
            dims: layout.dims.clone(),
            shape: layout.shape.clone(),
            data: ArrayData::F64(data),
            fill_value: None, // folded into NaN
        });
    }

    // Unpacked. Floats → float64 (fill → NaN). Integers keep an integer logical
    // type; an integer fill sentinel *survives* into the array (NaN cannot be
    // represented in an int), so it is reported via `fill_value`.
    match raw {
        Raw::F32(_) | Raw::F64(_) => {
            let data: Vec<f64> = raw
                .iter_f64()
                .map(|v| if is_fill(v) { f64::NAN } else { v })
                .collect();
            Ok(NativeField {
                dtype: DType::Float64,
                dims: layout.dims.clone(),
                shape: layout.shape.clone(),
                data: ArrayData::F64(data),
                fill_value: None,
            })
        }
        Raw::I8(v) => Ok(int_field(
            v.into_iter().map(i32::from).collect(),
            layout,
            fill,
        )),
        Raw::I16(v) => Ok(int_field(
            v.into_iter().map(i32::from).collect(),
            layout,
            fill,
        )),
        Raw::I32(v) => Ok(int_field(v, layout, fill)),
    }
}

/// Build an `int32` native field, carrying a surviving integer fill sentinel.
fn int_field(data: Vec<i32>, layout: &Layout, fill: Option<f64>) -> NativeField {
    NativeField {
        dtype: DType::Int32,
        dims: layout.dims.clone(),
        shape: layout.shape.clone(),
        data: ArrayData::I32(data),
        fill_value: fill,
    }
}

// --- header model -----------------------------------------------------------

#[derive(Debug)]
struct Dim {
    name: String,
    /// Length; the record (unlimited) dimension is stored as length 0 on disk.
    len: usize,
    is_record: bool,
}

#[derive(Debug, Clone)]
enum AttVal {
    Text(String),
    I8(Vec<i8>),
    I16(Vec<i16>),
    I32(Vec<i32>),
    F32(Vec<f32>),
    F64(Vec<f64>),
}

impl AttVal {
    /// The first value widened to f64 (for scale/offset/fill scalars).
    fn scalar_f64(&self) -> Option<f64> {
        match self {
            AttVal::Text(_) => None,
            AttVal::I8(v) => v.first().map(|&x| x as f64),
            AttVal::I16(v) => v.first().map(|&x| x as f64),
            AttVal::I32(v) => v.first().map(|&x| x as f64),
            AttVal::F32(v) => v.first().map(|&x| x as f64),
            AttVal::F64(v) => v.first().copied(),
        }
    }
}

#[derive(Debug)]
struct Var {
    name: String,
    dimids: Vec<usize>,
    atts: HashMap<String, AttVal>,
    nc_type: u32,
    vsize: usize,
    begin: usize,
    is_record: bool,
}

impl Var {
    fn att_scalar(&self, name: &str) -> Option<f64> {
        self.atts.get(name).and_then(AttVal::scalar_f64)
    }
    fn att_text(&self, name: &str) -> Option<String> {
        match self.atts.get(name) {
            Some(AttVal::Text(s)) => Some(s.clone()),
            _ => None,
        }
    }
}

#[derive(Debug)]
struct Header {
    numrecs: usize,
    dims: Vec<Dim>,
    vars: Vec<Var>,
    recsize: usize,
}

/// The on-disk shape of one variable.
struct Layout {
    dims: Vec<String>,
    shape: Vec<usize>,
    is_record: bool,
    /// Element count in a single record (product of the non-record dims); equals
    /// the full count for a non-record variable.
    per_record_elems: usize,
}

impl Header {
    fn parse(data: &[u8]) -> Result<Header> {
        // Magic + version. Distinguish the common confusions with clear errors.
        if data.len() < 4 {
            return Err(fmt_err("file shorter than the 4-byte magic"));
        }
        if &data[0..4] == b"\x89HDF" {
            return Err(fmt_err(
                "NetCDF-4/HDF5 file: the pure-Rust reader handles NetCDF-3 \
                 classic only; register a netcdf4 reader for HDF5-backed files",
            ));
        }
        if &data[0..3] != b"CDF" {
            return Err(fmt_err(
                "not a NetCDF classic file (bad magic, expected 'CDF')",
            ));
        }
        let version = data[3];
        let offset_size = match version {
            1 => 4usize,
            2 => 8usize,
            5 => {
                return Err(fmt_err(
                    "CDF-5 (64-bit data) not supported; NetCDF-3 classic (CDF-1/CDF-2) only",
                ))
            }
            v => return Err(fmt_err(&format!("unknown NetCDF classic version byte {v}"))),
        };

        let mut cur = Cursor {
            buf: data,
            pos: 4,
            offset_size,
        };

        let numrecs_raw = cur.u32()?;
        let dims = parse_dim_list(&mut cur)?;
        let _gatts = parse_att_list(&mut cur)?; // global attributes: parsed to advance, not used
        let vars = parse_var_list(&mut cur, &dims)?;

        let recsize: usize = vars.iter().filter(|v| v.is_record).map(|v| v.vsize).sum();

        // Resolve the record count: explicit, or computed from the file size for
        // a stream-written file.
        let numrecs = if numrecs_raw == STREAMING {
            match (
                recsize,
                vars.iter().filter(|v| v.is_record).map(|v| v.begin).min(),
            ) {
                (rs, Some(first)) if rs > 0 && data.len() > first => (data.len() - first) / rs,
                _ => 0,
            }
        } else {
            numrecs_raw as usize
        };

        Ok(Header {
            numrecs,
            dims,
            vars,
            recsize,
        })
    }

    fn layout(&self, var: &Var) -> Result<Layout> {
        let mut dims = Vec::with_capacity(var.dimids.len());
        let mut shape = Vec::with_capacity(var.dimids.len());
        for &id in &var.dimids {
            let d = self.dims.get(id).ok_or_else(|| {
                fmt_err(&format!("variable '{}' references dimid {id}", var.name))
            })?;
            dims.push(d.name.clone());
            shape.push(if d.is_record { self.numrecs } else { d.len });
        }
        let per_record_elems: usize = if var.is_record {
            shape.iter().skip(1).product()
        } else {
            shape.iter().product()
        };
        Ok(Layout {
            dims,
            shape,
            is_record: var.is_record,
            per_record_elems,
        })
    }

    /// Read a variable's raw on-disk values, assembling interleaved records for a
    /// record variable. Big-endian throughout (NetCDF classic is XDR-encoded).
    fn read_raw(&self, data: &[u8], var: &Var, layout: &Layout) -> Result<Raw> {
        let elem = type_size(var.nc_type)
            .ok_or_else(|| fmt_err(&format!("variable '{}' has unsupported type", var.name)))?;
        let per_record_bytes = layout.per_record_elems * elem;

        let mut bytes: Vec<u8> = Vec::new();
        if layout.is_record {
            for r in 0..self.numrecs {
                let off = var
                    .begin
                    .checked_add(r.checked_mul(self.recsize).ok_or_else(overflow)?)
                    .ok_or_else(overflow)?;
                bytes.extend_from_slice(slice_at(data, off, per_record_bytes)?);
            }
        } else {
            bytes.extend_from_slice(slice_at(data, var.begin, per_record_bytes)?);
        }
        decode_be(var.nc_type, &bytes)
    }
}

// --- raw numeric values -----------------------------------------------------

/// Raw on-disk values, typed by `nc_type`, before CF decode.
enum Raw {
    I8(Vec<i8>),
    I16(Vec<i16>),
    I32(Vec<i32>),
    F32(Vec<f32>),
    F64(Vec<f64>),
}

impl Raw {
    /// Iterate the values widened to f64 (for packing math and float fields).
    fn iter_f64(&self) -> Box<dyn Iterator<Item = f64> + '_> {
        match self {
            Raw::I8(v) => Box::new(v.iter().map(|&x| x as f64)),
            Raw::I16(v) => Box::new(v.iter().map(|&x| x as f64)),
            Raw::I32(v) => Box::new(v.iter().map(|&x| x as f64)),
            Raw::F32(v) => Box::new(v.iter().map(|&x| x as f64)),
            Raw::F64(v) => Box::new(v.iter().copied()),
        }
    }
}

fn type_size(nc_type: u32) -> Option<usize> {
    match nc_type {
        NC_BYTE | NC_CHAR => Some(1),
        NC_SHORT => Some(2),
        NC_INT | NC_FLOAT => Some(4),
        NC_DOUBLE => Some(8),
        _ => None,
    }
}

/// Decode a big-endian byte run into a typed vector. `NC_CHAR` data variables are
/// rejected (text data variables do not occur in the contract; text lives in
/// attributes and CSV/JSON readers).
fn decode_be(nc_type: u32, bytes: &[u8]) -> Result<Raw> {
    match nc_type {
        NC_BYTE => Ok(Raw::I8(bytes.iter().map(|&b| b as i8).collect())),
        NC_SHORT => Ok(Raw::I16(
            bytes
                .chunks_exact(2)
                .map(|c| i16::from_be_bytes([c[0], c[1]]))
                .collect(),
        )),
        NC_INT => Ok(Raw::I32(
            bytes
                .chunks_exact(4)
                .map(|c| i32::from_be_bytes([c[0], c[1], c[2], c[3]]))
                .collect(),
        )),
        NC_FLOAT => Ok(Raw::F32(
            bytes
                .chunks_exact(4)
                .map(|c| f32::from_be_bytes([c[0], c[1], c[2], c[3]]))
                .collect(),
        )),
        NC_DOUBLE => Ok(Raw::F64(
            bytes
                .chunks_exact(8)
                .map(|c| f64::from_be_bytes([c[0], c[1], c[2], c[3], c[4], c[5], c[6], c[7]]))
                .collect(),
        )),
        NC_CHAR => Err(fmt_err("NC_CHAR data variables are not supported")),
        other => Err(fmt_err(&format!("unsupported nc_type {other}"))),
    }
}

// --- header list parsers ----------------------------------------------------

fn parse_dim_list(cur: &mut Cursor) -> Result<Vec<Dim>> {
    let (tag, count) = cur.tag_count()?;
    if tag == 0 {
        return Ok(Vec::new()); // ABSENT
    }
    if tag != NC_DIMENSION {
        return Err(fmt_err(&format!("expected NC_DIMENSION tag, got {tag:#x}")));
    }
    let mut dims = Vec::with_capacity(count);
    for _ in 0..count {
        let name = cur.name()?;
        let len = cur.nonneg("dim length")?;
        dims.push(Dim {
            name,
            len,
            is_record: len == 0,
        });
    }
    Ok(dims)
}

fn parse_att_list(cur: &mut Cursor) -> Result<HashMap<String, AttVal>> {
    let (tag, count) = cur.tag_count()?;
    if tag == 0 {
        return Ok(HashMap::new()); // ABSENT
    }
    if tag != NC_ATTRIBUTE {
        return Err(fmt_err(&format!("expected NC_ATTRIBUTE tag, got {tag:#x}")));
    }
    let mut atts = HashMap::with_capacity(count);
    for _ in 0..count {
        let name = cur.name()?;
        let nc_type = cur.u32()?;
        let nelems = cur.nonneg("attribute nelems")?;
        let val = cur.att_values(nc_type, nelems)?;
        atts.insert(name, val);
    }
    Ok(atts)
}

fn parse_var_list(cur: &mut Cursor, dims: &[Dim]) -> Result<Vec<Var>> {
    let (tag, count) = cur.tag_count()?;
    if tag == 0 {
        return Ok(Vec::new()); // ABSENT
    }
    if tag != NC_VARIABLE {
        return Err(fmt_err(&format!("expected NC_VARIABLE tag, got {tag:#x}")));
    }
    let record_dim = dims.iter().position(|d| d.is_record);
    let mut vars = Vec::with_capacity(count);
    for _ in 0..count {
        let name = cur.name()?;
        let ndims = cur.nonneg("variable ndims")?;
        let mut dimids = Vec::with_capacity(ndims);
        for _ in 0..ndims {
            dimids.push(cur.nonneg("dimid")?);
        }
        let atts = parse_att_list(cur)?;
        let nc_type = cur.u32()?;
        let vsize = cur.nonneg("vsize")?;
        let begin = cur.offset()?;
        let is_record = dimids.first().copied() == record_dim;
        vars.push(Var {
            name,
            dimids,
            atts,
            nc_type,
            vsize,
            begin,
            is_record,
        });
    }
    Ok(vars)
}

// --- byte cursor ------------------------------------------------------------

/// A big-endian cursor over the header region, tracking the offset width
/// (4 bytes for CDF-1, 8 for CDF-2) used by `begin` fields.
struct Cursor<'a> {
    buf: &'a [u8],
    pos: usize,
    offset_size: usize,
}

impl<'a> Cursor<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self.pos.checked_add(n).ok_or_else(overflow)?;
        if end > self.buf.len() {
            return Err(fmt_err("unexpected end of header"));
        }
        let s = &self.buf[self.pos..end];
        self.pos = end;
        Ok(s)
    }

    fn u32(&mut self) -> Result<u32> {
        let b = self.take(4)?;
        Ok(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }

    /// A non-negative 32-bit count/size as usize (NetCDF stores these as signed
    /// INT; a negative value is malformed).
    fn nonneg(&mut self, what: &str) -> Result<usize> {
        let v = self.u32()? as i32;
        if v < 0 {
            return Err(fmt_err(&format!("negative {what}: {v}")));
        }
        Ok(v as usize)
    }

    /// A `begin` offset, 4 bytes (CDF-1) or 8 bytes (CDF-2).
    fn offset(&mut self) -> Result<usize> {
        if self.offset_size == 8 {
            let b = self.take(8)?;
            Ok(u64::from_be_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]) as usize)
        } else {
            Ok(self.u32()? as usize)
        }
    }

    /// A list header: `(tag, count)`. ABSENT is encoded as `(0, 0)`.
    fn tag_count(&mut self) -> Result<(u32, usize)> {
        let tag = self.u32()?;
        let count = self.nonneg("list count")?;
        Ok((tag, count))
    }

    /// A name: a 4-byte-padded, length-prefixed string.
    fn name(&mut self) -> Result<String> {
        let n = self.nonneg("name length")?;
        let raw = self.take(n)?;
        let s = String::from_utf8_lossy(raw).into_owned();
        self.skip_pad(n)?;
        Ok(s)
    }

    /// Skip the XDR padding that rounds a `n`-byte field up to a 4-byte boundary.
    fn skip_pad(&mut self, n: usize) -> Result<()> {
        let pad = (4 - (n % 4)) % 4;
        if pad > 0 {
            self.take(pad)?;
        }
        Ok(())
    }

    /// Read `nelems` attribute values of `nc_type`, consuming the 4-byte padding.
    fn att_values(&mut self, nc_type: u32, nelems: usize) -> Result<AttVal> {
        let elem = type_size(nc_type)
            .ok_or_else(|| fmt_err(&format!("attribute has unsupported nc_type {nc_type}")))?;
        let nbytes = nelems.checked_mul(elem).ok_or_else(overflow)?;
        let raw = self.take(nbytes)?.to_vec();
        self.skip_pad(nbytes)?;
        let val = match nc_type {
            NC_CHAR => AttVal::Text(
                String::from_utf8_lossy(&raw)
                    .trim_end_matches('\0')
                    .to_string(),
            ),
            NC_BYTE => AttVal::I8(raw.iter().map(|&b| b as i8).collect()),
            _ => match decode_be(nc_type, &raw)? {
                Raw::I8(v) => AttVal::I8(v),
                Raw::I16(v) => AttVal::I16(v),
                Raw::I32(v) => AttVal::I32(v),
                Raw::F32(v) => AttVal::F32(v),
                Raw::F64(v) => AttVal::F64(v),
            },
        };
        Ok(val)
    }
}

// --- small helpers ----------------------------------------------------------

fn slice_at(data: &[u8], off: usize, n: usize) -> Result<&[u8]> {
    let end = off.checked_add(n).ok_or_else(overflow)?;
    data.get(off..end).ok_or_else(|| {
        fmt_err(&format!(
            "data slice [{off}, {end}) past end of file {}",
            data.len()
        ))
    })
}

fn fmt_err(detail: &str) -> Error {
    Error::Format {
        format: "netcdf".to_string(),
        detail: detail.to_string(),
    }
}

fn overflow() -> Error {
    fmt_err("integer overflow computing a data offset")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_hdf5_magic() {
        let err = decode(b"\x89HDF\r\n\x1a\n", &[]).unwrap_err();
        assert!(matches!(err, Error::Format { .. }));
        assert!(err.to_string().contains("HDF5"));
    }

    #[test]
    fn rejects_bad_magic() {
        let err = decode(b"NOPE", &[]).unwrap_err();
        assert!(err.to_string().contains("bad magic"));
    }

    #[test]
    fn rejects_cdf5() {
        let err = decode(b"CDF\x05", &[]).unwrap_err();
        assert!(err.to_string().contains("CDF-5"));
    }

    #[test]
    fn truncated_file_is_an_error_not_a_panic() {
        // Valid magic + version, then nothing — must error cleanly.
        let err = decode(b"CDF\x01\x00\x00", &[]).unwrap_err();
        assert!(matches!(err, Error::Format { .. }));
    }
}
