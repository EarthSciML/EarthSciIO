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
//! # Reader options
//!
//! The [`Reader::read_native`] trait takes no kwargs, so `member`/`kind` live in
//! the reader **instance** (configured at construction, like the GeoTIFF reader's
//! band handling). The default-registered [`Ff10Reader::new`] (`member = None`)
//! decodes a bare `.csv` — the committed conformance fixture + cross-language
//! crosscheck path.
//!
//! A zipped input is configured **by the loader**:
//! [`DataSource::reader_options`](crate::DataSource::reader_options) —
//! `{"member_glob": "*egu*", "skip_header_row": true}` — reaches this reader
//! through [`Reader::configured`], which is the Rust spelling of the
//! Python/Julia `reader_kwargs`. So an `.esm` document that declares the glob
//! and the header row is decoded correctly by any caller, rather than only by
//! one that hand-injects a configured reader through `Provider::with_formats`
//! (still supported, and still what the builder methods
//! [`Ff10Reader::member`], [`Ff10Reader::members`], [`Ff10Reader::member_glob`],
//! [`Ff10Reader::skip_header_row`] are for).

use std::collections::{BTreeSet, HashSet};
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

/// The active `ff10` reader. `member`/`members`/`member_glob`/
/// `skip_header_row`/`kind`/`numeric` are carried on the instance (see the
/// module docs on the reader-kwarg asymmetry).
#[derive(Debug, Clone)]
pub struct Ff10Reader {
    kind: Ff10Kind,
    member: Option<String>,
    members: Vec<String>,
    member_glob: Option<String>,
    skip_header_row: bool,
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
            members: Vec::new(),
            member_glob: None,
            skip_header_row: false,
            numeric: &FF10_POINT_NUMERIC,
        }
    }

    /// Extract the named member from a `.zip` blob (reader config; NOT part of the
    /// cache key — many members share one cached zip). Mutually exclusive with
    /// [`Self::members`]/[`Self::member_glob`].
    pub fn member(mut self, member: impl Into<String>) -> Self {
        self.member = Some(member.into());
        self
    }

    /// Select MULTIPLE members of a `.zip` blob by explicit name. Combined
    /// (union, deduplicated) with any [`Self::member_glob`] matches; the selected
    /// members are read and their rows concatenated in ascending lexicographic
    /// (byte) order of member name. A name absent from the archive is an error.
    /// Reader config; NOT part of the cache key.
    pub fn members<I, S>(mut self, members: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.members = members.into_iter().map(Into::into).collect();
        self
    }

    /// Select MULTIPLE members of a `.zip` blob by an fnmatch-style glob (`*` any
    /// run incl. empty, `?` one char, `[...]`/`[!...]` character class;
    /// case-sensitive; matched against the full member path, e.g. `*egu*`). A
    /// glob matching zero members is an error; directory placeholder entries
    /// (names ending in `/`) are never selected. Combined (union) with
    /// [`Self::members`]; concatenation is in ascending lexicographic (byte)
    /// order of member name. Reader config; NOT part of the cache key.
    pub fn member_glob(mut self, glob: impl Into<String>) -> Self {
        self.member_glob = Some(glob.into());
        self
    }

    /// Handle the EPA 2016fd-style column-header line: after comment (`#`) and
    /// blank lines are dropped, the first remaining line of each selected input
    /// (each selected zip member, or the bare file) must be a header row — its
    /// first comma-separated field, compared case-insensitively, must be
    /// `country_cd` — and exactly that one line is skipped per member. If the
    /// first field is anything else the reader errors (the option asserts a
    /// header row and never silently drops a data row).
    pub fn skip_header_row(mut self, yes: bool) -> Self {
        self.skip_header_row = yes;
        self
    }

    /// Select the point schema (the default; explicit for parity with the
    /// Julia/Python `kind="point"` kwarg).
    pub fn kind_point(mut self) -> Self {
        self.kind = Ff10Kind::Point;
        self
    }

    /// Build a configured reader from a loader's declared `reader_options` —
    /// the DECLARED form of the builder calls above, so an `.esm` data loader
    /// (not a caller with a hand-built registry) says which zip members hold
    /// its table and whether they carry a header line. Recognised keys:
    ///
    /// | key | type | meaning |
    /// |---|---|---|
    /// | `member` | string | [`Ff10Reader::member`] |
    /// | `members` | array of string | [`Ff10Reader::members`] |
    /// | `member_glob` | string | [`Ff10Reader::member_glob`] |
    /// | `skip_header_row` | bool | [`Ff10Reader::skip_header_row`] |
    /// | `kind` | `"point"` | the FF10 schema variant |
    ///
    /// Any other key — or a recognised key of the wrong type — is an error.
    fn from_options(options: &serde_json::Map<String, serde_json::Value>) -> Result<Self> {
        let mut r = Ff10Reader::new();
        for (k, v) in options {
            match k.as_str() {
                "member" => {
                    r = r.member(str_opt(k, v)?);
                }
                "members" => {
                    let arr = v.as_array().ok_or_else(|| {
                        fmt_err(&format!("reader option {k:?} must be an array of strings"))
                    })?;
                    let names: Result<Vec<String>> =
                        arr.iter().map(|m| str_opt(k, m).map(str::to_string)).collect();
                    r = r.members(names?);
                }
                "member_glob" => {
                    r = r.member_glob(str_opt(k, v)?);
                }
                "skip_header_row" => {
                    let b = v.as_bool().ok_or_else(|| {
                        fmt_err(&format!("reader option {k:?} must be a boolean"))
                    })?;
                    r = r.skip_header_row(b);
                }
                "kind" => match str_opt(k, v)? {
                    "point" => r = r.kind_point(),
                    other => {
                        return Err(fmt_err(&format!(
                            "reader option \"kind\": unknown FF10 schema variant {other:?} \
                             (only \"point\" ships today)"
                        )));
                    }
                },
                other => {
                    return Err(fmt_err(&format!(
                        "unknown reader option {other:?}; ff10 takes member, members, \
                         member_glob, skip_header_row, kind"
                    )));
                }
            }
        }
        Ok(r)
    }
}

/// A `reader_options` value that must be a string.
fn str_opt<'a>(key: &str, v: &'a serde_json::Value) -> Result<&'a str> {
    v.as_str()
        .ok_or_else(|| fmt_err(&format!("reader option {key:?} must be a string")))
}

impl Reader for Ff10Reader {
    fn formats(&self) -> &'static [&'static str] {
        &["ff10"]
    }

    fn extensions(&self) -> &'static [&'static str] {
        &["ff10", "csv"]
    }

    fn configured(
        &self,
        options: &serde_json::Map<String, serde_json::Value>,
    ) -> Result<Option<std::sync::Arc<dyn Reader>>> {
        if options.is_empty() {
            return Ok(None);
        }
        Ok(Some(std::sync::Arc::new(Ff10Reader::from_options(options)?)))
    }

    fn read_native(
        &self,
        blob_path: &Path,
        variables: &[String],
        _select: &Selection,
    ) -> Result<NativeDataset> {
        // Selection::All is the only variant today; the whole table is read.
        let Ff10Kind::Point = self.kind;
        let texts: Vec<String> = if !self.members.is_empty() || self.member_glob.is_some() {
            if self.member.is_some() {
                return Err(fmt_err(
                    "`member` is mutually exclusive with `members`/`member_glob`",
                ));
            }
            read_zip_selected(blob_path, &self.members, self.member_glob.as_deref())?
        } else if let Some(m) = &self.member {
            vec![read_zip_member(blob_path, m)?]
        } else {
            vec![std::fs::read_to_string(blob_path)
                .map_err(|e| Error::io(Some(blob_path.to_path_buf()), e))?]
        };
        let mut rows: Vec<Vec<String>> = Vec::new();
        for text in &texts {
            parse_rows(&mut rows, text, self.skip_header_row)?;
        }
        build_dataset(rows, self.numeric, variables)
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

/// Resolve `members`/`glob` against the archive and return the selected member
/// TEXTS in ascending lexicographic (byte) order of member name — the
/// deterministic concatenation order. An explicit name absent from the archive,
/// and a glob matching zero members, are errors. Selection considers only FILE
/// members: directory placeholder entries (names ending in `/`, e.g. the real
/// 2016fd zip's `…/ptegu/`) are ignored.
fn read_zip_selected(path: &Path, members: &[String], glob: Option<&str>) -> Result<Vec<String>> {
    let file = std::fs::File::open(path).map_err(|e| Error::io(Some(path.to_path_buf()), e))?;
    let mut archive = zip::ZipArchive::new(file).map_err(zip_err)?;
    let names: Vec<String> = archive
        .file_names()
        .filter(|n| !n.ends_with('/'))
        .map(str::to_string)
        .collect();

    // BTreeSet: dedup + the sorted (byte-lexicographic) concatenation order.
    let mut selected: BTreeSet<String> = BTreeSet::new();
    for m in members {
        if !names.iter().any(|n| n == m) {
            return Err(fmt_err(&format!(
                "zip member {m:?} not found in {}; members: {names:?}",
                path.display()
            )));
        }
        selected.insert(m.clone());
    }
    if let Some(g) = glob {
        let toks = glob_tokens(g);
        let hits: Vec<&String> = names.iter().filter(|n| glob_match(&toks, n)).collect();
        if hits.is_empty() {
            return Err(fmt_err(&format!(
                "member_glob {g:?} matched no members in {}; members: {names:?}",
                path.display()
            )));
        }
        for h in hits {
            selected.insert(h.clone());
        }
    }
    if selected.is_empty() {
        return Err(fmt_err("members/member_glob selected no zip members"));
    }

    let mut out = Vec::with_capacity(selected.len());
    for name in &selected {
        let mut zf = archive.by_name(name).map_err(zip_err)?;
        let mut text = String::new();
        zf.read_to_string(&mut text)
            .map_err(|e| Error::io(Some(path.to_path_buf()), e))?;
        out.push(text);
    }
    Ok(out)
}

/// One parsed element of an fnmatch-style glob pattern.
enum GlobTok {
    /// `*` — any run of characters, including empty.
    Star,
    /// `?` — exactly one character.
    Any,
    /// A literal character.
    Lit(char),
    /// `[...]` / `[!...]` — a (possibly negated) character class of literal
    /// chars and `a-z` ranges. An unclosed `[` is treated as a literal (fnmatch
    /// semantics).
    Class { neg: bool, ranges: Vec<(char, char)> },
}

/// Parse an fnmatch-style glob (`*`, `?`, `[...]`/`[!...]`; case-sensitive) into
/// tokens. Semantics match Python `fnmatch.fnmatchcase` and the Julia reader's
/// `_glob_regex` — the cross-language `member_glob` contract.
fn glob_tokens(pat: &str) -> Vec<GlobTok> {
    let chars: Vec<char> = pat.chars().collect();
    let mut toks = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        match chars[i] {
            '*' => toks.push(GlobTok::Star),
            '?' => toks.push(GlobTok::Any),
            '[' => {
                let mut j = i + 1;
                let neg = j < chars.len() && chars[j] == '!';
                if neg {
                    j += 1;
                }
                let body_start = j;
                if j < chars.len() && chars[j] == ']' {
                    j += 1; // a ']' first in the class is a literal member
                }
                while j < chars.len() && chars[j] != ']' {
                    j += 1;
                }
                if j >= chars.len() {
                    toks.push(GlobTok::Lit('[')); // unclosed class -> literal '['
                } else {
                    let body = &chars[body_start..j];
                    let mut ranges = Vec::new();
                    let mut k = 0;
                    while k < body.len() {
                        if k + 2 < body.len() && body[k + 1] == '-' {
                            ranges.push((body[k], body[k + 2]));
                            k += 3;
                        } else {
                            ranges.push((body[k], body[k]));
                            k += 1;
                        }
                    }
                    toks.push(GlobTok::Class { neg, ranges });
                    i = j;
                }
            }
            c => toks.push(GlobTok::Lit(c)),
        }
        i += 1;
    }
    toks
}

fn tok_match(t: &GlobTok, c: char) -> bool {
    match t {
        GlobTok::Star => unreachable!("Star handled by glob_match"),
        GlobTok::Any => true,
        GlobTok::Lit(l) => *l == c,
        GlobTok::Class { neg, ranges } => {
            let inside = ranges.iter().any(|&(lo, hi)| lo <= c && c <= hi);
            inside != *neg
        }
    }
}

/// Match tokens against `text` with classic single-pointer `*` backtracking.
fn glob_match(toks: &[GlobTok], text: &str) -> bool {
    let chars: Vec<char> = text.chars().collect();
    let (mut ti, mut si) = (0usize, 0usize);
    let mut star: Option<usize> = None;
    let mut mark = 0usize;
    while si < chars.len() {
        if ti < toks.len() && matches!(toks[ti], GlobTok::Star) {
            star = Some(ti);
            mark = si;
            ti += 1;
        } else if ti < toks.len()
            && !matches!(toks[ti], GlobTok::Star)
            && tok_match(&toks[ti], chars[si])
        {
            ti += 1;
            si += 1;
        } else if let Some(s) = star {
            ti = s + 1;
            mark += 1;
            si = mark;
        } else {
            return false;
        }
    }
    while ti < toks.len() && matches!(toks[ti], GlobTok::Star) {
        ti += 1;
    }
    ti == toks.len()
}

/// Parse one input's FF10 text into 77-field rows, appended to `rows`.
///
/// Skips empty + '#' comment lines; with `skip_header_row`, drops the asserted
/// `country_cd` header line (the first remaining line's first comma-separated
/// field must equal `country_cd` case-insensitively — anything else is an error,
/// so the option can never silently drop a data row); then RFC-4180 parses.
fn parse_rows(rows: &mut Vec<Vec<String>>, text: &str, skip_header_row: bool) -> Result<()> {
    let ncol = FF10_POINT_COLUMNS.len();

    let mut data_lines: Vec<&str> = text
        .lines()
        .filter(|ln| {
            let s = ln.trim_start();
            !s.is_empty() && !ln.trim().is_empty() && !s.starts_with('#')
        })
        .collect();

    if skip_header_row {
        let Some(first) = data_lines.first() else {
            return Err(fmt_err(
                "skip_header_row: no non-comment lines — the asserted header row is missing",
            ));
        };
        let first_field = first
            .split(',')
            .next()
            .unwrap_or("")
            .trim()
            .to_ascii_lowercase();
        if first_field != "country_cd" {
            return Err(fmt_err(&format!(
                "skip_header_row: first non-comment line does not start with a \
                 'country_cd' header field (got {first_field:?}); refusing to \
                 drop a data row"
            )));
        }
        data_lines.remove(0);
    }

    let data = data_lines.join("\n");
    let mut rdr = csv::ReaderBuilder::new()
        .has_headers(false)
        .flexible(false)
        .from_reader(data.as_bytes());
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
    Ok(())
}

/// Decode FF10 point text into 77 native fields on a single `index` dim
/// (bare-text convenience over [`parse_rows`] + [`build_dataset`]; test-only —
/// production goes through [`Ff10Reader::read_native`]).
#[cfg(test)]
fn decode(text: &str, numeric: &[&str], variables: &[String]) -> Result<NativeDataset> {
    let mut rows = Vec::new();
    parse_rows(&mut rows, text, false)?;
    build_dataset(rows, numeric, variables)
}

/// Assemble parsed 77-field rows into the native `points` dataset.
fn build_dataset(
    rows: Vec<Vec<String>>,
    numeric: &[&str],
    variables: &[String],
) -> Result<NativeDataset> {
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

    /// The lowercase `country_cd,region_cd,…` header line the EPA 2016fd members
    /// carry (77 fields, NOT a `#` comment).
    fn header_line() -> String {
        FF10_POINT_COLUMNS
            .iter()
            .map(|c| c.to_ascii_lowercase())
            .collect::<Vec<_>>()
            .join(",")
    }

    /// One 77-field row with the given POLID/ANN_VALUE (plus fixed ids).
    fn tiny_row(fac: &str, polid: &str, ann: &str) -> String {
        let idx = |name: &str| FF10_POINT_COLUMNS.iter().position(|&c| c == name).unwrap();
        let mut r = vec![String::new(); FF10_POINT_COLUMNS.len()];
        r[idx("COUNTRY_CD")] = "US".into();
        r[idx("REGION_CD")] = "01001".into();
        r[idx("FACILITY_ID")] = fac.into();
        r[idx("POLID")] = polid.into();
        r[idx("ANN_VALUE")] = ann.into();
        r.join(",")
    }

    /// A zip with two `*egu*` members + one excluded member, each carrying a
    /// `#` comment block and the `country_cd` header line.
    fn egu_zip(dir: &Path) -> std::path::PathBuf {
        let member = |rows: &[String]| {
            format!(
                "#FORMAT=FF10_POINT\n{}\n{}\n",
                header_line(),
                rows.join("\n")
            )
        };
        let zpath = dir.join("2016fd_inputs_point.zip");
        let f = std::fs::File::create(&zpath).unwrap();
        let mut zw = zip::ZipWriter::new(f);
        // A glob-matching DIRECTORY placeholder entry (like the real 2016fd
        // `…/ptegu/`) — selection must ignore it (file members only).
        zw.add_directory::<_, ()>("point_egu", zip::write::SimpleFileOptions::default())
            .unwrap();
        // Written in NON-sorted order to prove the read order is sorted-name.
        for (name, rows) in [
            ("point/egu_beta.csv", vec![tiny_row("F202", "NOX", "333.3")]),
            (
                "point/egu_alpha.csv",
                vec![tiny_row("F101", "NOX", "111.1"), tiny_row("F101", "SO2", "22.2")],
            ),
            ("point/ptnonipm.csv", vec![tiny_row("F999", "NOX", "999.9")]),
        ] {
            zw.start_file::<_, ()>(name, zip::write::SimpleFileOptions::default())
                .unwrap();
            zw.write_all(member(&rows).as_bytes()).unwrap();
        }
        zw.finish().unwrap();
        zpath
    }

    #[test]
    fn member_glob_concatenates_in_sorted_name_order() {
        let dir = tempfile::tempdir().unwrap();
        let zpath = egu_zip(dir.path());
        let reader = Ff10Reader::new().member_glob("*egu*").skip_header_row(true);
        let ds = reader.read_native(&zpath, &[], &Selection::All).unwrap();
        // alpha (2 rows) sorts before beta (1 row); ptnonipm excluded.
        assert_eq!(strs(&ds, "FACILITY_ID"), &["F101", "F101", "F202"]);
        assert_eq!(f64s(&ds, "ANN_VALUE"), &[111.1, 22.2, 333.3]);
        assert!(!strs(&ds, "FACILITY_ID").contains(&"F999".to_string()));
    }

    #[test]
    fn member_glob_zero_matches_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let zpath = egu_zip(dir.path());
        let err = Ff10Reader::new()
            .member_glob("*nope*")
            .read_native(&zpath, &[], &Selection::All)
            .unwrap_err();
        assert!(err.to_string().contains("matched no members"), "{err}");
    }

    #[test]
    fn explicit_members_union_glob_and_missing_name_errors() {
        let dir = tempfile::tempdir().unwrap();
        let zpath = egu_zip(dir.path());
        // explicit list ∪ glob, deduplicated, sorted: alpha + beta + ptnonipm.
        let ds = Ff10Reader::new()
            .members(["point/ptnonipm.csv", "point/egu_alpha.csv"])
            .member_glob("*egu*")
            .skip_header_row(true)
            .read_native(&zpath, &[], &Selection::All)
            .unwrap();
        assert_eq!(strs(&ds, "FACILITY_ID"), &["F101", "F101", "F202", "F999"]);
        // an explicit member absent from the archive is an error.
        let err = Ff10Reader::new()
            .members(["point/absent.csv"])
            .skip_header_row(true)
            .read_native(&zpath, &[], &Selection::All)
            .unwrap_err();
        assert!(err.to_string().contains("not found"), "{err}");
    }

    #[test]
    fn skip_header_row_skips_exactly_once_per_member_and_asserts() {
        let dir = tempfile::tempdir().unwrap();
        let zpath = egu_zip(dir.path());
        // Without skip_header_row, the header line is a 77-field data row that
        // dies at the numeric parse of ann_value.
        let err = Ff10Reader::new()
            .member_glob("*egu*")
            .read_native(&zpath, &[], &Selection::All)
            .unwrap_err();
        assert!(err.to_string().contains("cannot parse"), "{err}");

        // singular member + skip_header_row composes.
        let ds = Ff10Reader::new()
            .member("point/egu_beta.csv")
            .skip_header_row(true)
            .read_native(&zpath, &[], &Selection::All)
            .unwrap();
        assert_eq!(strs(&ds, "FACILITY_ID"), &["F202"]);

        // asserting a header on a member that has none is an error, never a
        // silently-dropped data row.
        let bare = dir.path().join("noheader.csv");
        std::fs::write(&bare, fixture_text()).unwrap();
        let err = Ff10Reader::new()
            .skip_header_row(true)
            .read_native(&bare, &[], &Selection::All)
            .unwrap_err();
        assert!(err.to_string().contains("refusing to drop"), "{err}");
    }

    #[test]
    fn member_is_exclusive_with_members_and_glob() {
        let dir = tempfile::tempdir().unwrap();
        let zpath = egu_zip(dir.path());
        let err = Ff10Reader::new()
            .member("point/egu_alpha.csv")
            .member_glob("*egu*")
            .read_native(&zpath, &[], &Selection::All)
            .unwrap_err();
        assert!(err.to_string().contains("mutually exclusive"), "{err}");
    }

    #[test]
    fn glob_matcher_semantics() {
        let m = |p: &str, t: &str| glob_match(&glob_tokens(p), t);
        assert!(m("*egu*", "point/egucems_2016fd.csv"));
        assert!(m("*egu*", "egu"));
        assert!(!m("*egu*", "point/ptnonipm.csv"));
        assert!(m("?gu", "egu"));
        assert!(!m("?gu", "eegu"));
        assert!(m("egu[12].csv", "egu1.csv"));
        assert!(!m("egu[12].csv", "egu3.csv"));
        assert!(m("egu[!12].csv", "egu3.csv"));
        assert!(m("a-c.csv", "a-c.csv"));
        assert!(m("egu[a-c].csv", "egub.csv"));
        assert!(!m("egu[a-c].csv", "egud.csv"));
        assert!(m("lit[", "lit[")); // unclosed class is a literal
        assert!(!m("*EGU*", "point/egucems.csv")); // case-sensitive
    }

    /// The DECLARED spelling of `member_glob` + `skip_header_row`: a loader's
    /// `reader_options` produce the same reader as the builder calls, so an
    /// `.esm` that declares them decodes the zip without a caller-injected
    /// registry.
    #[test]
    fn reader_options_configure_the_same_reader_as_the_builder() {
        let dir = tempfile::tempdir().unwrap();
        let zpath = egu_zip(dir.path());
        let built = Ff10Reader::new()
            .member_glob("*egu*")
            .skip_header_row(true)
            .read_native(&zpath, &[], &Selection::All)
            .unwrap();

        let opts: serde_json::Map<String, serde_json::Value> = serde_json::from_str(
            r#"{"member_glob": "*egu*", "skip_header_row": true, "kind": "point"}"#,
        )
        .unwrap();
        let declared = Ff10Reader::new()
            .configured(&opts)
            .unwrap()
            .expect("non-empty options yield a configured reader")
            .read_native(&zpath, &[], &Selection::All)
            .unwrap();

        assert_eq!(strs(&declared, "FACILITY_ID"), strs(&built, "FACILITY_ID"));
        assert_eq!(f64s(&declared, "ANN_VALUE"), f64s(&built, "ANN_VALUE"));

        // An empty option set leaves the registered reader alone.
        assert!(
            Ff10Reader::new()
                .configured(&serde_json::Map::new())
                .unwrap()
                .is_none()
        );
    }

    /// A mis-typed or wrongly-typed option is an ERROR, never a silently
    /// ignored key — the whole point of routing options through the reader.
    #[test]
    fn unknown_or_ill_typed_reader_options_are_rejected() {
        let bad = |json: &str| -> String {
            let opts: serde_json::Map<String, serde_json::Value> =
                serde_json::from_str(json).unwrap();
            match Ff10Reader::new().configured(&opts) {
                Err(e) => e.to_string(),
                Ok(_) => panic!("option set {json} must be rejected"),
            }
        };
        // The typo the shim's `member_filter` spelling would have been.
        assert!(bad(r#"{"member_filter": "*egu*"}"#).contains("unknown reader option"));
        assert!(bad(r#"{"skip_header_row": "yes"}"#).contains("must be a boolean"));
        assert!(bad(r#"{"member_glob": 7}"#).contains("must be a string"));
        assert!(bad(r#"{"kind": "nonpoint"}"#).contains("unknown FF10 schema variant"));
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
