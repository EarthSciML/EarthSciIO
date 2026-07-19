//! Pure-Rust **FF10 point** reader behind the `format` registry — the RAW
//! long-format FF10 point table (SMOKE / Emissions.jl `FF10_POINT`) as a `points`
//! [`NativeDataset`] in **native units**. Decode parity with the Python
//! ([`earthsciio.readers.FF10Reader`]) and Julia (`FF10Reader`) readers.
//!
//! # Why a new reader (not the generic CSV path)
//!
//! FF10 data rows carry no clean header row and the file opens with a `#` comment
//! header block (`#FORMAT=…`, `#COUNTRY`, …); the 77 column names come from the
//! fixed [`FF10_POINT_COLUMNS`] schema exactly as Emissions.jl supplies them. A
//! free-text `FACILITY_NAME` may embed the delimiter, so RFC-4180 quoting is
//! required (via the pure-Rust `csv` crate — the same "C compiler alone, no
//! bindgen" build story as the rest of the crate).
//!
//! # Reader-only (Risk R3)
//!
//! POLID stays a data column (**no pollutant pivot**); `STKHGT`/`STKDIAM` stay
//! feet, `STKTEMP` °F, `STKFLOW` ft³/s, `STKVEL` ft/s, `ANN_VALUE` tons/yr (**no
//! unit conversion**); no FIPS/SCC normalization; no EGU/pollutant filter — those
//! transforms move downstream into the `.esm`.
//!
//! # Reader-kwarg asymmetry
//!
//! The [`Reader::read_native`] trait takes no kwargs and the Rust
//! `DataLoader`/`Provider` carry no `reader_kwargs`, so `member`/`kind` live in
//! the reader **instance** (configured at construction, like the GeoTIFF reader's
//! band handling). The default-registered [`Ff10Reader::new`] (`member = None`)
//! decodes a bare `.csv` — the committed conformance fixture + cross-language
//! crosscheck path. For the zipped tutorial input, a caller injects a
//! member-configured reader through the existing `Provider::with_formats` seam.

use std::collections::HashSet;
use std::io::Read;
use std::path::Path;

use crate::error::{Error, Result};

use super::{ArrayData, DType, NativeDataset, NativeField, Reader, Selection};

/// The 77 FF10 point column names, in file order. Copied from Emissions.jl
/// `src/ff10.jl` `FF10_POINT_COLUMNS`; the first two use the SMOKE FF10_POINT
/// spec names COUNTRY_CD / REGION_CD (Emissions.jl: COUNTRY / FIPS — identical
/// values, a positional alias).
pub const FF10_POINT_COLUMNS: [&str; 77] = [
    "COUNTRY_CD", "REGION_CD", "TRIBAL_CODE", "FACILITY_ID",
    "UNIT_ID", "REL_POINT_ID", "PROCESS_ID", "AGY_FACILITY_ID",
    "AGY_UNIT_ID", "AGY_REL_POINT_ID", "AGY_PROCESS_ID", "SCC",
    "POLID", "ANN_VALUE", "ANN_PCT_RED", "FACILITY_NAME",
    "ERPTYPE", "STKHGT", "STKDIAM", "STKTEMP",
    "STKFLOW", "STKVEL", "NAICS", "LONGITUDE",
    "LATITUDE", "LL_DATUM", "HORIZ_COLL_MTHD", "DESIGN_CAPACITY",
    "DESIGN_CAPACITY_UNITS", "REG_CODES", "FAC_SOURCE_TYPE", "UNIT_TYPE_CODE",
    "CONTROL_IDS", "CONTROL_MEASURES", "CURRENT_COST", "CUMULATIVE_COST",
    "PROJECTION_FACTOR", "SUBMITTER_FAC_ID", "CALC_METHOD", "DATA_SET_ID",
    "FACIL_CATEGORY_CODE", "ORIS_FACILITY_CODE", "ORIS_BOILER_ID", "IPM_YN",
    "CALC_YEAR", "DATE_UPDATED", "FUG_HEIGHT", "FUG_WIDTH_XDIM",
    "FUG_LENGTH_YDIM", "FUG_ANGLE", "ZIPCODE", "ANNUAL_AVG_HOURS_PER_YEAR",
    "JAN_VALUE", "FEB_VALUE", "MAR_VALUE", "APR_VALUE",
    "MAY_VALUE", "JUN_VALUE", "JUL_VALUE", "AUG_VALUE",
    "SEP_VALUE", "OCT_VALUE", "NOV_VALUE", "DEC_VALUE",
    "JAN_PCTRED", "FEB_PCTRED", "MAR_PCTRED", "APR_PCTRED",
    "MAY_PCTRED", "JUN_PCTRED", "JUL_PCTRED", "AUG_PCTRED",
    "SEP_PCTRED", "OCT_PCTRED", "NOV_PCTRED", "DEC_PCTRED",
    "COMMENT",
];

/// The 42 FF10 point columns decoded to `f64` (blank → `NaN`); every other column
/// (ids/codes/free-text/temporal tokens) stays `Str` so leading-zero codes
/// (REGION_CD `"01001"`, ZIPCODE `"00000"`, SCC, POLID) never become floats.
pub const FF10_POINT_NUMERIC: [&str; 42] = [
    "ANN_VALUE", "ANN_PCT_RED", "STKHGT", "STKDIAM", "STKTEMP", "STKFLOW",
    "STKVEL", "LONGITUDE", "LATITUDE", "DESIGN_CAPACITY", "CURRENT_COST",
    "CUMULATIVE_COST", "PROJECTION_FACTOR", "FUG_HEIGHT", "FUG_WIDTH_XDIM",
    "FUG_LENGTH_YDIM", "FUG_ANGLE", "ANNUAL_AVG_HOURS_PER_YEAR",
    "JAN_VALUE", "FEB_VALUE", "MAR_VALUE", "APR_VALUE", "MAY_VALUE", "JUN_VALUE",
    "JUL_VALUE", "AUG_VALUE", "SEP_VALUE", "OCT_VALUE", "NOV_VALUE", "DEC_VALUE",
    "JAN_PCTRED", "FEB_PCTRED", "MAR_PCTRED", "APR_PCTRED", "MAY_PCTRED",
    "JUN_PCTRED", "JUL_PCTRED", "AUG_PCTRED", "SEP_PCTRED", "OCT_PCTRED",
    "NOV_PCTRED", "DEC_PCTRED",
];

/// The FF10 schema variant. Only `Point` (77 columns) ships today; the 45-col
/// nonpoint/onroad/nonroad schemas can be added as further variants behind the
/// same `ff10` reader (one more const each; no new registration).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Ff10Kind {
    /// The 77-column FF10 point schema.
    Point,
}

/// The active `ff10` reader. `member`/`kind`/`numeric` are carried on the
/// instance (see the module docs on the reader-kwarg asymmetry).
#[derive(Debug, Clone)]
pub struct Ff10Reader {
    kind: Ff10Kind,
    member: Option<String>,
    numeric: &'static [&'static str],
}

impl Default for Ff10Reader {
    fn default() -> Self {
        Self::new()
    }
}

impl Ff10Reader {
    /// The default reader: point kind, `member = None` (decodes a bare `.csv`),
    /// numeric = the 42-name [`FF10_POINT_NUMERIC`] const.
    pub fn new() -> Self {
        Self {
            kind: Ff10Kind::Point,
            member: None,
            numeric: &FF10_POINT_NUMERIC,
        }
    }

    /// Extract the named member from a `.zip` blob (reader config; NOT part of the
    /// cache key — many members share one cached zip).
    pub fn member(mut self, member: impl Into<String>) -> Self {
        self.member = Some(member.into());
        self
    }

    /// Select the point schema (the default; explicit for parity with the
    /// Julia/Python `kind="point"` kwarg).
    pub fn kind_point(mut self) -> Self {
        self.kind = Ff10Kind::Point;
        self
    }
}

impl Reader for Ff10Reader {
    fn formats(&self) -> &'static [&'static str] {
        &["ff10"]
    }

    fn extensions(&self) -> &'static [&'static str] {
        &["ff10", "csv"]
    }

    fn read_native(
        &self,
        blob_path: &Path,
        variables: &[String],
        _select: &Selection,
    ) -> Result<NativeDataset> {
        // Selection::All is the only variant today; the whole table is read.
        let Ff10Kind::Point = self.kind;
        let text = match &self.member {
            Some(m) => read_zip_member(blob_path, m)?,
            None => std::fs::read_to_string(blob_path)
                .map_err(|e| Error::io(Some(blob_path.to_path_buf()), e))?,
        };
        decode(&text, self.numeric, variables)
    }
}

/// Read a named member of a zip archive as UTF-8 text, via the pure-Rust `zip`
/// crate.
fn read_zip_member(path: &Path, member: &str) -> Result<String> {
    let file = std::fs::File::open(path).map_err(|e| Error::io(Some(path.to_path_buf()), e))?;
    let mut archive = zip::ZipArchive::new(file).map_err(zip_err)?;
    let mut zf = archive
        .by_name(member)
        .map_err(|_| fmt_err(&format!("zip member {member:?} not found in {}", path.display())))?;
    let mut text = String::new();
    zf.read_to_string(&mut text)
        .map_err(|e| Error::io(Some(path.to_path_buf()), e))?;
    Ok(text)
}

/// Decode FF10 point text into 77 native fields on a single `index` dim.
fn decode(text: &str, numeric: &[&str], variables: &[String]) -> Result<NativeDataset> {
    let ncol = FF10_POINT_COLUMNS.len();

    // Skip empty + '#' comment lines, then RFC-4180 parse each data line.
    let data: String = text
        .lines()
        .filter(|ln| {
            let s = ln.trim_start();
            !s.is_empty() && !ln.trim().is_empty() && !s.starts_with('#')
        })
        .collect::<Vec<_>>()
        .join("\n");

    let mut rdr = csv::ReaderBuilder::new()
        .has_headers(false)
        .flexible(false)
        .from_reader(data.as_bytes());

    let mut rows: Vec<Vec<String>> = Vec::new();
    for rec in rdr.records() {
        let rec = rec.map_err(csv_err)?;
        if rec.len() != ncol {
            return Err(fmt_err(&format!(
                "FF10 point row has {} fields, expected {ncol}",
                rec.len()
            )));
        }
        rows.push(rec.iter().map(str::to_string).collect());
    }

    let numset: HashSet<&str> = numeric.iter().copied().collect();
    let want: Option<HashSet<&str>> = if variables.is_empty() {
        None
    } else {
        Some(variables.iter().map(String::as_str).collect())
    };
    if let Some(w) = &want {
        for v in w {
            if !FF10_POINT_COLUMNS.contains(v) {
                return Err(fmt_err(&format!("requested FF10 column {v:?} not in schema")));
            }
        }
    }

    let nrows = rows.len();
    let mut out = NativeDataset::default();
    for (j, &name) in FF10_POINT_COLUMNS.iter().enumerate() {
        if let Some(w) = &want {
            if !w.contains(name) {
                continue;
            }
        }
        let field = if numset.contains(name) {
            let mut vals = Vec::with_capacity(nrows);
            for r in &rows {
                let s = r[j].trim();
                vals.push(if s.is_empty() {
                    f64::NAN
                } else {
                    s.parse::<f64>().map_err(|_| {
                        fmt_err(&format!("column {name}: cannot parse {:?} as f64", r[j]))
                    })?
                });
            }
            NativeField {
                dtype: DType::Float64,
                dims: vec!["index".to_string()],
                shape: vec![nrows],
                data: ArrayData::F64(vals),
                fill_value: None,
            }
        } else {
            let vals: Vec<String> = rows.iter().map(|r| r[j].clone()).collect();
            NativeField {
                dtype: DType::Str,
                dims: vec!["index".to_string()],
                shape: vec![nrows],
                data: ArrayData::Str(vals),
                fill_value: None,
            }
        };
        out.variables.insert(name.to_string(), field);
    }
    Ok(out)
}

fn fmt_err(detail: &str) -> Error {
    Error::Format {
        format: "ff10".to_string(),
        detail: detail.to_string(),
    }
}

fn csv_err(e: csv::Error) -> Error {
    fmt_err(&e.to_string())
}

fn zip_err(e: zip::result::ZipError) -> Error {
    fmt_err(&e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// A tiny FF10 point blob: a `#` header block + 3 data rows. Rows 1 & 2
    /// (NOX/SO2) share one stack (F001/U1/R1/P1 + stack params + lon/lat),
    /// differing only in POLID/ANN_VALUE. Row 1 has a quoted-comma FACILITY_NAME
    /// and a blank DESIGN_CAPACITY (numeric → NaN).
    fn fixture_text() -> String {
        let idx = |name: &str| FF10_POINT_COLUMNS.iter().position(|&c| c == name).unwrap();
        let mkrow = |over: &[(&str, &str)]| -> String {
            let mut r = vec![String::new(); FF10_POINT_COLUMNS.len()];
            for (k, v) in over {
                r[idx(k)] = (*v).to_string();
            }
            // RFC-4180 quote the FACILITY_NAME if it embeds a comma.
            let fi = idx("FACILITY_NAME");
            if r[fi].contains(',') {
                r[fi] = format!("\"{}\"", r[fi]);
            }
            r.join(",")
        };
        let stack: Vec<(&str, &str)> = vec![
            ("COUNTRY_CD", "US"), ("REGION_CD", "01001"), ("FACILITY_ID", "F001"),
            ("UNIT_ID", "U1"), ("REL_POINT_ID", "R1"), ("PROCESS_ID", "P1"),
            ("SCC", "0030700101"), ("FACILITY_NAME", "Autauga Plant, Unit 1"),
            ("STKHGT", "100.0"), ("STKTEMP", "500.0"),
            ("LONGITUDE", "-86.51045"), ("LATITUDE", "32.43878"), ("ZIPCODE", "00000"),
        ];
        let mut r1 = stack.clone();
        r1.extend_from_slice(&[("POLID", "NOX"), ("ANN_VALUE", "123.45")]);
        let mut r2 = stack.clone();
        r2.extend_from_slice(&[("POLID", "SO2"), ("ANN_VALUE", "67.89")]);
        let r3: Vec<(&str, &str)> = vec![
            ("COUNTRY_CD", "US"), ("REGION_CD", "01001"), ("FACILITY_ID", "F002"),
            ("POLID", "PM25"), ("ANN_VALUE", "4.2"), ("FACILITY_NAME", "Plain Name"),
        ];
        format!(
            "#FORMAT=FF10_POINT\n#COUNTRY US\n\n{}\n{}\n{}\n",
            mkrow(&r1), mkrow(&r2), mkrow(&r3)
        )
    }

    fn strs<'a>(ds: &'a NativeDataset, name: &str) -> &'a [String] {
        match &ds.variables[name].data {
            ArrayData::Str(v) => v,
            _ => panic!("{name} not a string field"),
        }
    }

    fn f64s<'a>(ds: &'a NativeDataset, name: &str) -> &'a [f64] {
        match &ds.variables[name].data {
            ArrayData::F64(v) => v,
            _ => panic!("{name} not a f64 field"),
        }
    }

    #[test]
    fn header_quote_empty_typing() {
        let ds = decode(&fixture_text(), &FF10_POINT_NUMERIC, &[]).unwrap();
        assert_eq!(ds.variables.len(), 77);
        assert!(ds.coords.is_empty()); // points table: no gridded axis
        assert_eq!(ds.variables["ANN_VALUE"].dims, vec!["index".to_string()]);

        // '#' header + blank line skipped -> 3 rows.
        assert_eq!(strs(&ds, "POLID").len(), 3);
        assert_eq!(strs(&ds, "POLID"), &["NOX", "SO2", "PM25"]);
        assert_eq!(f64s(&ds, "ANN_VALUE"), &[123.45, 67.89, 4.2]);

        // leading-zero codes stay strings.
        assert_eq!(strs(&ds, "REGION_CD"), &["01001", "01001", "01001"]);
        assert_eq!(strs(&ds, "SCC")[0], "0030700101");
        assert_eq!(strs(&ds, "ZIPCODE")[0], "00000");

        // quoted comma preserved verbatim (quotes stripped).
        assert_eq!(strs(&ds, "FACILITY_NAME")[0], "Autauga Plant, Unit 1");
        assert_eq!(strs(&ds, "FACILITY_NAME")[2], "Plain Name");

        // blank numeric -> NaN; blank string -> "".
        assert!(f64s(&ds, "DESIGN_CAPACITY")[0].is_nan());
        assert_eq!(strs(&ds, "TRIBAL_CODE")[0], "");
    }

    #[test]
    fn multi_pollutant_same_stack_no_pivot() {
        let ds = decode(&fixture_text(), &FF10_POINT_NUMERIC, &[]).unwrap();
        // rows 1 & 2 share the stack, differ only in POLID/ANN_VALUE.
        assert_eq!(strs(&ds, "FACILITY_ID")[0], "F001");
        assert_eq!(strs(&ds, "FACILITY_ID")[1], "F001");
        assert_eq!(f64s(&ds, "STKHGT")[0], 100.0);
        assert_eq!(f64s(&ds, "STKHGT")[1], 100.0);
        // native units retained (feet / °F), not converted.
        assert_eq!(f64s(&ds, "STKTEMP")[0], 500.0);
    }

    #[test]
    fn absent_variable_is_an_error() {
        let err = decode(&fixture_text(), &FF10_POINT_NUMERIC, &["NOPE".to_string()])
            .unwrap_err();
        assert!(matches!(err, Error::Format { .. }));
    }

    #[test]
    fn zip_member_equals_bare() {
        let text = fixture_text();
        let bare = decode(&text, &FF10_POINT_NUMERIC, &[]).unwrap();

        // Build a zip holding the CSV as member `inv/point.csv`.
        let dir = tempfile::tempdir().unwrap();
        let zpath = dir.path().join("2016fd_inputs_point.zip");
        {
            let f = std::fs::File::create(&zpath).unwrap();
            let mut zw = zip::ZipWriter::new(f);
            zw.start_file::<_, ()>("inv/point.csv", zip::write::SimpleFileOptions::default())
                .unwrap();
            zw.write_all(text.as_bytes()).unwrap();
            zw.finish().unwrap();
        }

        let reader = Ff10Reader::new().kind_point().member("inv/point.csv");
        let zipped = reader
            .read_native(&zpath, &[], &Selection::All)
            .expect("zip decode");
        assert_eq!(f64s(&zipped, "ANN_VALUE"), f64s(&bare, "ANN_VALUE"));
        assert_eq!(strs(&zipped, "POLID"), strs(&bare, "POLID"));
        assert_eq!(strs(&zipped, "FACILITY_NAME"), strs(&bare, "FACILITY_NAME"));

        // A missing member is a clear error, not a silent empty.
        let miss = Ff10Reader::new().member("nope.csv");
        assert!(miss.read_native(&zpath, &[], &Selection::All).is_err());
    }

    #[test]
    fn bare_csv_via_read_native_path() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("ff10_point.csv");
        std::fs::write(&p, fixture_text()).unwrap();
        let ds = Ff10Reader::new()
            .read_native(&p, &[], &Selection::All)
            .unwrap();
        assert_eq!(ds.variables.len(), 77);
        assert_eq!(strs(&ds, "POLID"), &["NOX", "SO2", "PM25"]);
    }
}
