//! ESRI **shapefile** reader behind the `format` registry — a shapefile as a
//! feature table [`NativeDataset`]. Decode parity with the Python
//! (`earthsciio.readers.ShapefileReader`, pyshp) and Julia (`ShapefileReader`,
//! Shapefile.jl) readers.
//!
//! Decode is delegated to the pure-Rust [`shapefile`](https://docs.rs/shapefile)
//! crate (and, for the `.dbf` attribute table, its `dbase` dependency). Nothing
//! about either format is re-implemented here; what this reader owns is the
//! mapping onto the NATIVE-ARRAY contract.
//!
//! # One row per PART
//!
//! A shapefile record may carry several parts — a polygon's outer ring plus its
//! holes, a county's mainland plus its islands, a multi-part route. The op that
//! consumes this geometry (`polygon_intersection_area`, `intersect_polygon`)
//! takes ONE ring, so a reader that surfaced only the first part would silently
//! drop the islands. Each part therefore becomes one row of the `index` axis,
//! with the record's `.dbf` attributes REPLICATED across its parts and
//! `shape_index`/`part_index`/`n_parts` naming where the row came from. A layer
//! whose records are all single-part decodes 1:1.
//!
//! # Reader-only (Risk R3)
//!
//! No reprojection (the `.prj` WKT is carried verbatim as the geometry field's
//! `crs_wkt` attribute), no unit conversion, no ring-orientation fix, no
//! polygon/hole classification, no name remap. Vertices are the stored ones: an
//! explicitly closed ring keeps its closing vertex (dropping it is the geometry
//! kernel's job, `esm-spec` §8.6.1) and winding is untouched.
//!
//! # Reader options
//!
//! Like the FF10 reader, kwargs live in the reader **instance** and reach it
//! from a document through [`Reader::configured`]: `member` (the `.shp` inside a
//! zip blob; sidecars are the same stem with `.dbf`/`.shx`/`.prj`) and
//! `numeric_columns` (parse the named `C` columns as `float64`) and `nvert_max`
//! (pad `geometry` to exactly that many vertex slots, so the DOCUMENT declares
//! the vertex-axis length; a longer part is an error, never a truncation).

use std::collections::HashMap;
use std::io::{Cursor, Read};
use std::path::Path;
use std::sync::Arc;

use super::{ArrayData, DType, NativeDataset, NativeField, Reader, Selection};
use crate::error::{Error, Result};

/// Field names the reader itself produces. A `.dbf` column of the same name is a
/// collision the reader refuses rather than silently shadowing either side.
const RESERVED: [&str; 11] = [
    "geometry",
    "shape_type",
    "crs_wkt",
    "n_vertices",
    "shape_index",
    "part_index",
    "n_parts",
    "xmin",
    "ymin",
    "xmax",
    "ymax",
];

fn fmt_err(detail: &str) -> Error {
    Error::Format {
        format: "shapefile".to_string(),
        detail: detail.to_string(),
    }
}

/// The `shapefile` format reader. See the module docs for the decode contract.
#[derive(Debug, Clone, Default)]
pub struct ShapefileReader {
    member: Option<String>,
    numeric_columns: Vec<String>,
    nvert_max: Option<usize>,
}

impl ShapefileReader {
    /// The default reader: no zip member (decodes a bare `.shp`, or the single
    /// `.shp` of a zip), no forced-numeric columns.
    pub fn new() -> Self {
        Self::default()
    }

    /// Name the `.shp` inside a `.zip` blob (reader config; NOT part of the
    /// cache key — many layers can share one cached archive).
    pub fn member(mut self, member: impl Into<String>) -> Self {
        self.member = Some(member.into());
        self
    }

    /// Parse these text (`C`) `.dbf` columns as `float64` — for a code column (a
    /// FIPS `GEOID`) a model wants as a number.
    pub fn numeric_columns<I, S>(mut self, columns: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.numeric_columns = columns.into_iter().map(Into::into).collect();
        self
    }

    /// Pad `geometry` to exactly `n` vertex slots instead of to the longest
    /// part, so a DOCUMENT declares the vertex-axis length rather than
    /// inheriting a number the file happens to have. A longer part is an error
    /// naming it — never a silent truncation.
    pub fn nvert_max(mut self, n: usize) -> Self {
        self.nvert_max = Some(n);
        self
    }

    fn from_options(options: &serde_json::Map<String, serde_json::Value>) -> Result<Self> {
        let mut r = ShapefileReader::new();
        for (k, v) in options {
            match k.as_str() {
                "member" => {
                    let s = v
                        .as_str()
                        .ok_or_else(|| fmt_err(&format!("reader option {k:?} must be a string")))?;
                    r = r.member(s);
                }
                "numeric_columns" => {
                    let arr = v.as_array().ok_or_else(|| {
                        fmt_err(&format!("reader option {k:?} must be an array of strings"))
                    })?;
                    let mut names = Vec::with_capacity(arr.len());
                    for it in arr {
                        names.push(
                            it.as_str()
                                .ok_or_else(|| {
                                    fmt_err(&format!(
                                        "reader option {k:?} must be an array of strings"
                                    ))
                                })?
                                .to_string(),
                        );
                    }
                    r = r.numeric_columns(names);
                }
                "nvert_max" => {
                    let n = v.as_u64().ok_or_else(|| {
                        fmt_err(&format!("reader option {k:?} must be a positive integer"))
                    })?;
                    r = r.nvert_max(n as usize);
                }
                other => {
                    return Err(fmt_err(&format!(
                        "unknown reader option {other:?}; shapefile takes member, \
                         numeric_columns, nvert_max"
                    )));
                }
            }
        }
        Ok(r)
    }
}

impl Reader for ShapefileReader {
    fn formats(&self) -> &'static [&'static str] {
        &["shapefile"]
    }

    fn extensions(&self) -> &'static [&'static str] {
        &["shp", "zip"]
    }

    fn configured(
        &self,
        options: &serde_json::Map<String, serde_json::Value>,
    ) -> Result<Option<Arc<dyn Reader>>> {
        if options.is_empty() {
            return Ok(None);
        }
        Ok(Some(Arc::new(ShapefileReader::from_options(options)?)))
    }

    fn read_native(
        &self,
        blob_path: &Path,
        variables: &[String],
        _select: &Selection,
    ) -> Result<NativeDataset> {
        // Selection::All is the only variant today; the whole table is read.
        let blobs = shapefile_members(blob_path, self.member.as_deref())?;
        let shp = blobs
            .get("shp")
            .ok_or_else(|| fmt_err("the blob has no .shp content"))?;

        let mut reader = match blobs.get("shx") {
            Some(shx) => ::shapefile::ShapeReader::with_shx(
                Cursor::new(shp.clone()),
                Cursor::new(shx.clone()),
            ),
            None => ::shapefile::ShapeReader::new(Cursor::new(shp.clone())),
        }
        .map_err(|e| fmt_err(&format!("cannot open the .shp: {e}")))?;
        let shape_type = shape_type_name(reader.header().shape_type);
        let shapes = reader
            .iter_shapes()
            .collect::<std::result::Result<Vec<::shapefile::Shape>, _>>()
            .map_err(|e| fmt_err(&format!("cannot decode the .shp: {e}")))?;

        // `.dbf`: file-order column names + the LIVE rows. A row is deleted iff
        // its flag byte is `*` (spec/conformance.md §3) — `dbase` skips exactly
        // those, so the deletion flags are read here to realign the shapes.
        let mut colnames: Vec<String> = Vec::new();
        let mut rows: Vec<::shapefile::dbase::Record> = Vec::new();
        let mut deleted = vec![false; shapes.len()];
        if let Some(dbf) = blobs.get("dbf") {
            let flags = deletion_flags(dbf);
            if flags.len() != shapes.len() {
                return Err(fmt_err(&format!(
                    "shapefile has {} shapes but {} .dbf rows",
                    shapes.len(),
                    flags.len()
                )));
            }
            deleted = flags;
            let mut dr = ::shapefile::dbase::Reader::new(Cursor::new(dbf.clone()))
                .map_err(|e| fmt_err(&format!("cannot open the .dbf: {e}")))?;
            colnames = dr.fields().iter().map(|f| f.name().to_string()).collect();
            rows = dr
                .read()
                .map_err(|e| fmt_err(&format!("cannot decode the .dbf: {e}")))?;
        }
        let clash: Vec<&str> = RESERVED
            .iter()
            .copied()
            .filter(|r| colnames.iter().any(|c| c == r))
            .collect();
        if !clash.is_empty() {
            return Err(fmt_err(&format!(
                ".dbf column name(s) {clash:?} collide with the reader's own fields {RESERVED:?}"
            )));
        }
        for c in &self.numeric_columns {
            if !colnames.contains(c) {
                return Err(fmt_err(&format!(
                    "numeric_columns names no such .dbf column: {c:?}"
                )));
            }
        }

        // Explode to one row per part, dropping `*`-deleted records whole.
        let mut rings: Vec<Vec<[f64; 2]>> = Vec::new();
        let mut shape_ix: Vec<i64> = Vec::new();
        let mut part_ix: Vec<i64> = Vec::new();
        let mut nparts: Vec<i64> = Vec::new();
        let mut boxes: Vec<[f64; 4]> = Vec::new();
        let mut row_of: Vec<usize> = Vec::new();
        let mut live = 0usize;
        for (si, shape) in shapes.iter().enumerate() {
            if deleted[si] {
                continue;
            }
            let parts = shape_parts(shape);
            let bbox = shape_bbox(shape);
            for (pi, ring) in parts.iter().enumerate() {
                rings.push(ring.clone());
                shape_ix.push(si as i64);
                part_ix.push(pi as i64);
                nparts.push(parts.len() as i64);
                boxes.push(bbox);
                row_of.push(live);
            }
            live += 1;
        }
        if !colnames.is_empty() && rows.len() != live {
            return Err(fmt_err(&format!(
                "shapefile has {live} live shapes but {} live .dbf rows",
                rows.len()
            )));
        }

        let n = rings.len();
        let mut nvert = rings.iter().map(Vec::len).max().unwrap_or(0);
        if let Some(cap) = self.nvert_max {
            if nvert > cap {
                let w = (0..rings.len())
                    .max_by_key(|&i| rings[i].len())
                    .unwrap_or(0);
                return Err(fmt_err(&format!(
                    "declared nvert_max={cap} but row {w} (shape {}, part {}) has {nvert} vertices",
                    shape_ix[w], part_ix[w]
                )));
            }
            nvert = cap;
        }
        let nvert = nvert.max(1);
        let mut geom = vec![f64::NAN; n * nvert * 2];
        for (i, ring) in rings.iter().enumerate() {
            for v in 0..nvert {
                let pt = if v < ring.len() {
                    ring[v]
                } else if let Some(last) = ring.last() {
                    *last // right-pad by repeating the final vertex (esm-spec §8.6.1)
                } else {
                    continue; // a Null shape's row stays NaN
                };
                geom[(i * nvert + v) * 2] = pt[0];
                geom[(i * nvert + v) * 2 + 1] = pt[1];
            }
        }

        let mut out = NativeDataset::default();
        out.variables.insert(
            "geometry".into(),
            NativeField {
                dtype: DType::Float64,
                dims: vec!["index".into(), "vertex".into(), "xy".into()],
                shape: vec![n, nvert, 2],
                data: ArrayData::F64(geom),
                fill_value: None,
            },
        );
        // The layer's shape type and (when the archive carries a `.prj`) its
        // projection WKT are one-element `meta` string fields rather than field
        // attributes, because the Rust `NativeField` has no `attrs` and the
        // cross-language native-array equality check compares FIELDS.
        let mut meta = vec![("shape_type".to_string(), shape_type.to_string())];
        if let Some(prj) = blobs.get("prj") {
            meta.push((
                "crs_wkt".to_string(),
                String::from_utf8_lossy(prj).trim().to_string(),
            ));
        }
        for (name, value) in meta {
            out.variables.insert(
                name,
                NativeField {
                    dtype: DType::Str,
                    dims: vec!["meta".into()],
                    shape: vec![1],
                    data: ArrayData::Str(vec![value]),
                    fill_value: None,
                },
            );
        }
        out.variables.insert(
            "n_vertices".into(),
            i64_field(rings.iter().map(|r| r.len() as i64).collect()),
        );
        out.variables
            .insert("shape_index".into(), i64_field(shape_ix));
        out.variables
            .insert("part_index".into(), i64_field(part_ix));
        out.variables.insert("n_parts".into(), i64_field(nparts));
        for (k, name) in ["xmin", "ymin", "xmax", "ymax"].iter().enumerate() {
            out.variables.insert(
                (*name).into(),
                f64_field(boxes.iter().map(|b| b[k]).collect()),
            );
        }

        for name in &colnames {
            let cells: Vec<Option<&::shapefile::dbase::FieldValue>> =
                row_of.iter().map(|&r| rows[r].get(name)).collect();
            let forced = self.numeric_columns.iter().any(|c| c == name);
            let field = if forced || cells.iter().all(|c| is_numeric(*c)) {
                f64_field(cells.iter().map(|c| cell_f64(*c)).collect())
            } else if cells.iter().all(|c| is_logical(*c)) {
                NativeField {
                    dtype: DType::Bool,
                    dims: vec!["index".into()],
                    shape: vec![cells.len()],
                    data: ArrayData::Bool(cells.iter().map(|c| cell_bool(*c)).collect()),
                    fill_value: None,
                }
            } else {
                NativeField {
                    dtype: DType::Str,
                    dims: vec!["index".into()],
                    shape: vec![cells.len()],
                    data: ArrayData::Str(cells.iter().map(|c| cell_text(*c)).collect()),
                    fill_value: None,
                }
            };
            out.variables.insert(name.clone(), field);
        }

        if !variables.is_empty() {
            let mut missing: Vec<&str> = variables
                .iter()
                .map(String::as_str)
                .filter(|v| !out.variables.contains_key(*v))
                .collect();
            if !missing.is_empty() {
                missing.sort_unstable();
                let mut present: Vec<&str> = out.variables.keys().map(String::as_str).collect();
                present.sort_unstable();
                return Err(fmt_err(&format!(
                    "requested variables not in the shapefile: {missing:?}; present: {present:?}"
                )));
            }
            out.variables
                .retain(|k, _| variables.iter().any(|v| v == k));
        }
        Ok(out)
    }
}

fn i64_field(values: Vec<i64>) -> NativeField {
    NativeField {
        dtype: DType::Int64,
        dims: vec!["index".into()],
        shape: vec![values.len()],
        data: ArrayData::I64(values),
        fill_value: None,
    }
}

fn f64_field(values: Vec<f64>) -> NativeField {
    NativeField {
        dtype: DType::Float64,
        dims: vec!["index".into()],
        shape: vec![values.len()],
        data: ArrayData::F64(values),
        fill_value: None,
    }
}

/// Shape-type code -> name (ESRI Shapefile Technical Description, page 4).
fn shape_type_name(t: ::shapefile::ShapeType) -> &'static str {
    use ::shapefile::ShapeType as S;
    match t {
        S::NullShape => "Null",
        S::Point => "Point",
        S::Polyline => "PolyLine",
        S::Polygon => "Polygon",
        S::Multipoint => "MultiPoint",
        S::PointZ => "PointZ",
        S::PolylineZ => "PolyLineZ",
        S::PolygonZ => "PolygonZ",
        S::MultipointZ => "MultiPointZ",
        S::PointM => "PointM",
        S::PolylineM => "PolyLineM",
        S::PolygonM => "PolygonM",
        S::MultipointM => "MultiPointM",
        S::Multipatch => "MultiPatch",
    }
}

/// One shape's vertex rings/parts, in FILE order, as `[x, y]` pairs. A
/// Polygon/PolyLine/MultiPatch carries explicit parts; a Point or MultiPoint has
/// none and is ONE part (a MultiPoint's points stay together). A Null shape is
/// one EMPTY part, so a null row still occupies its slot in the record axis.
fn shape_parts(shape: &::shapefile::Shape) -> Vec<Vec<[f64; 2]>> {
    use ::shapefile::Shape as S;
    macro_rules! from_parts {
        ($p:expr) => {
            $p.parts()
                .iter()
                .map(|pt| pt.iter().map(|p| [p.x, p.y]).collect())
                .collect()
        };
    }
    macro_rules! from_rings {
        ($p:expr) => {
            $p.rings()
                .iter()
                .map(|r| r.points().iter().map(|p| [p.x, p.y]).collect())
                .collect()
        };
    }
    macro_rules! from_points {
        ($p:expr) => {
            vec![$p.points().iter().map(|p| [p.x, p.y]).collect()]
        };
    }
    match shape {
        S::NullShape => vec![Vec::new()],
        S::Point(p) => vec![vec![[p.x, p.y]]],
        S::PointM(p) => vec![vec![[p.x, p.y]]],
        S::PointZ(p) => vec![vec![[p.x, p.y]]],
        S::Polyline(p) => from_parts!(p),
        S::PolylineM(p) => from_parts!(p),
        S::PolylineZ(p) => from_parts!(p),
        S::Polygon(p) => from_rings!(p),
        S::PolygonM(p) => from_rings!(p),
        S::PolygonZ(p) => from_rings!(p),
        S::Multipoint(p) => from_points!(p),
        S::MultipointM(p) => from_points!(p),
        S::MultipointZ(p) => from_points!(p),
        S::Multipatch(p) => p
            .patches()
            .iter()
            .map(|patch| patch.points().iter().map(|p| [p.x, p.y]).collect())
            .collect(),
    }
}

/// A shape's bounding box: the record's own STORED `Box` where the format has
/// one, else (Point) the point itself. `NaN`s for a Null shape.
fn shape_bbox(shape: &::shapefile::Shape) -> [f64; 4] {
    use ::shapefile::Shape as S;
    macro_rules! stored {
        ($p:expr) => {{
            let b = $p.bbox();
            let (x, y) = (b.x_range(), b.y_range());
            [x[0], y[0], x[1], y[1]]
        }};
    }
    match shape {
        S::NullShape => [f64::NAN; 4],
        S::Point(p) => [p.x, p.y, p.x, p.y],
        S::PointM(p) => [p.x, p.y, p.x, p.y],
        S::PointZ(p) => [p.x, p.y, p.x, p.y],
        S::Polyline(p) => stored!(p),
        S::PolylineM(p) => stored!(p),
        S::PolylineZ(p) => stored!(p),
        S::Polygon(p) => stored!(p),
        S::PolygonM(p) => stored!(p),
        S::PolygonZ(p) => stored!(p),
        S::Multipoint(p) => stored!(p),
        S::MultipointM(p) => stored!(p),
        S::MultipointZ(p) => stored!(p),
        S::Multipatch(p) => stored!(p),
    }
}

fn is_numeric(cell: Option<&::shapefile::dbase::FieldValue>) -> bool {
    use ::shapefile::dbase::FieldValue as V;
    matches!(
        cell,
        Some(V::Numeric(_)) | Some(V::Float(_)) | Some(V::Integer(_)) | Some(V::Double(_)) | None
    )
}

fn is_logical(cell: Option<&::shapefile::dbase::FieldValue>) -> bool {
    matches!(
        cell,
        Some(::shapefile::dbase::FieldValue::Logical(_)) | None
    )
}

/// A `.dbf` cell as `float64`: blank / missing / unparseable → `NaN`.
fn cell_f64(cell: Option<&::shapefile::dbase::FieldValue>) -> f64 {
    use ::shapefile::dbase::FieldValue as V;
    match cell {
        None => f64::NAN,
        Some(V::Numeric(v)) => v.unwrap_or(f64::NAN),
        Some(V::Float(v)) => v.map(f64::from).unwrap_or(f64::NAN),
        Some(V::Integer(v)) => f64::from(*v),
        Some(V::Double(v)) => *v,
        Some(V::Currency(v)) => *v,
        Some(V::Logical(v)) => v.map(|b| if b { 1.0 } else { 0.0 }).unwrap_or(f64::NAN),
        Some(other) => text_of(other).trim().parse::<f64>().unwrap_or(f64::NAN),
    }
}

fn cell_bool(cell: Option<&::shapefile::dbase::FieldValue>) -> bool {
    match cell {
        Some(::shapefile::dbase::FieldValue::Logical(v)) => v.unwrap_or(false),
        _ => false,
    }
}

/// A `.dbf` cell as text: missing → `""`; a `D` date → `YYYYMMDD`.
fn cell_text(cell: Option<&::shapefile::dbase::FieldValue>) -> String {
    match cell {
        None => String::new(),
        Some(v) => text_of(v),
    }
}

fn text_of(value: &::shapefile::dbase::FieldValue) -> String {
    use ::shapefile::dbase::FieldValue as V;
    match value {
        V::Character(v) => v.clone().unwrap_or_default().trim().to_string(),
        V::Memo(v) => v.trim().to_string(),
        V::Date(Some(d)) => format!("{:04}{:02}{:02}", d.year(), d.month(), d.day()),
        V::Date(None) => String::new(),
        other => format!("{other}"),
    }
}

/// Per-row deletion flags of a `.dbf` blob: `true` iff the row's flag byte is
/// `*` (0x2A). `dbase` (and Julia's DBFTables) use exactly this rule; pyshp
/// treats any non-space flag as deleted, which the Python reader normalizes
/// away. Reading the flags here is also what realigns the shapes with the LIVE
/// `.dbf` rows, since `dbase`'s record iterator skips deleted rows silently.
fn deletion_flags(raw: &[u8]) -> Vec<bool> {
    if raw.len() < 32 {
        return Vec::new();
    }
    let nrec = u32::from_le_bytes([raw[4], raw[5], raw[6], raw[7]]) as usize;
    let hdr = u16::from_le_bytes([raw[8], raw[9]]) as usize;
    let rec = u16::from_le_bytes([raw[10], raw[11]]) as usize;
    if rec < 1 || hdr < 32 || hdr + nrec * rec > raw.len() {
        return vec![false; nrec];
    }
    (0..nrec).map(|i| raw[hdr + i * rec] == 0x2A).collect()
}

/// The `.shp` + sidecar byte blobs of one shapefile, keyed by lowercase
/// extension. A shapefile is a **file set** but the content-addressed cache
/// holds ONE blob, so the fetchable form is a `.zip`; a bare `.shp` blob decodes
/// too, with geometry only. `member` names the `.shp` inside a zip; when omitted
/// the archive must contain exactly one.
fn shapefile_members(path: &Path, member: Option<&str>) -> Result<HashMap<String, Vec<u8>>> {
    let bytes = std::fs::read(path).map_err(|e| Error::io(Some(path.to_path_buf()), e))?;
    let mut out = HashMap::new();
    if bytes.len() < 2 || &bytes[..2] != b"PK" {
        out.insert("shp".to_string(), bytes);
        return Ok(out);
    }
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes))
        .map_err(|e| fmt_err(&format!("cannot open the zip blob: {e}")))?;
    let names: Vec<String> = (0..archive.len())
        .filter_map(|i| archive.by_index(i).ok().map(|f| f.name().to_string()))
        .filter(|n| !n.ends_with('/'))
        .collect();
    let mut shps: Vec<&String> = names
        .iter()
        .filter(|n| n.to_lowercase().ends_with(".shp"))
        .collect();
    shps.sort();
    let target = match member {
        Some(m) => {
            if !names.iter().any(|n| n == m) {
                return Err(fmt_err(&format!(
                    "zip member {m:?} not in the archive; .shp members present: {shps:?}"
                )));
            }
            m.to_string()
        }
        None if shps.len() == 1 => shps[0].clone(),
        None if shps.is_empty() => return Err(fmt_err("the zip contains no .shp member")),
        None => {
            return Err(fmt_err(&format!(
                "the zip contains {} .shp members; name one with reader_options.member: {shps:?}",
                shps.len()
            )))
        }
    };
    let stem = target[..target.len() - 4].to_lowercase();
    for name in &names {
        let lower = name.to_lowercase();
        let key = if *name == target {
            "shp".to_string()
        } else if let Some(ext) = lower.strip_prefix(&format!("{stem}.")) {
            if !matches!(ext, "dbf" | "shx" | "prj") {
                continue;
            }
            ext.to_string()
        } else {
            continue;
        };
        let mut buf = Vec::new();
        archive
            .by_name(name)
            .map_err(|e| fmt_err(&format!("cannot read zip member {name:?}: {e}")))?
            .read_to_end(&mut buf)
            .map_err(|e| Error::io(Some(path.to_path_buf()), e))?;
        out.insert(key, buf);
    }
    Ok(out)
}
