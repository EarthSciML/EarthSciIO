//! The active `csv` reader (`spec/registries.md` §2): a delimited-text points
//! table, matching the Python and Julia `CSVReader`.
//!
//! Each column becomes one rank-1 field on the single dimension `index`, keyed
//! by its header name; there are no coordinates. A column named in
//! `numeric_columns` is `float64`; every other column is a `string` field. When
//! `numeric_columns` is not given, a column is `float64` exactly when every one
//! of its cells parses as a number after trimming whitespace (the Python and
//! Julia fallback), so a digit-only identifier column such as OpenAQ's
//! `location_id` must be left out of `numeric_columns` to stay text.
//!
//! Reader options (through [`Reader::configured`]): `numeric_columns`
//! (`[string]`), `delimiter` (a one-character string, default `,`) and
//! `header_row` (the 0-based row holding the column names, default 0; rows
//! before it are dropped). Blank lines are skipped. Quoted fields are RFC 4180,
//! as Python's `csv` module reads them. A numeric cell that does not parse —
//! a blank one included — is an error naming the column and row, as it is in
//! Python and Julia; it is never a silent `NaN`.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;

use crate::error::{Error, Result};
use crate::format::{ArrayData, DType, NativeDataset, NativeField, Reader, Selection};

fn fmt_err(detail: impl Into<String>) -> Error {
    Error::Format {
        format: "csv".to_string(),
        detail: detail.into(),
    }
}

/// The `csv` format reader. The default instance (no options) infers numeric
/// columns, splits on `,` and reads the header from row 0.
#[derive(Debug, Clone)]
pub struct CsvReader {
    numeric_columns: Option<Vec<String>>,
    delimiter: u8,
    header_row: usize,
}

impl Default for CsvReader {
    fn default() -> Self {
        Self::new()
    }
}

impl CsvReader {
    /// The default reader: inferred numeric columns, `,`, header in row 0.
    pub fn new() -> Self {
        CsvReader {
            numeric_columns: None,
            delimiter: b',',
            header_row: 0,
        }
    }

    /// Read exactly these columns as `float64`; every other column is text.
    pub fn numeric_columns<I, S>(mut self, names: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.numeric_columns = Some(names.into_iter().map(Into::into).collect());
        self
    }

    /// The field delimiter (one ASCII byte).
    pub fn delimiter(mut self, delimiter: u8) -> Self {
        self.delimiter = delimiter;
        self
    }

    /// The 0-based (non-blank) row holding the column names.
    pub fn header_row(mut self, row: usize) -> Self {
        self.header_row = row;
        self
    }

    fn from_options(options: &serde_json::Map<String, serde_json::Value>) -> Result<Self> {
        let mut r = CsvReader::new();
        for (k, v) in options {
            match k.as_str() {
                "numeric_columns" => {
                    if v.is_null() {
                        continue;
                    }
                    let bad =
                        || fmt_err(format!("reader option {k:?} must be an array of strings"));
                    let names = v
                        .as_array()
                        .ok_or_else(bad)?
                        .iter()
                        .map(|x| x.as_str().map(str::to_string).ok_or_else(bad))
                        .collect::<Result<Vec<_>>>()?;
                    r = r.numeric_columns(names);
                }
                "delimiter" => {
                    let s = v.as_str().filter(|s| s.len() == 1).ok_or_else(|| {
                        fmt_err(format!(
                            "reader option {k:?} must be a one-character ASCII string"
                        ))
                    })?;
                    r = r.delimiter(s.as_bytes()[0]);
                }
                "header_row" => {
                    let n = v.as_u64().ok_or_else(|| {
                        fmt_err(format!(
                            "reader option {k:?} must be a non-negative integer (0-based)"
                        ))
                    })?;
                    r = r.header_row(n as usize);
                }
                other => {
                    return Err(fmt_err(format!(
                        "unknown reader option {other:?}; csv takes numeric_columns, \
                         delimiter, header_row"
                    )));
                }
            }
        }
        Ok(r)
    }

    /// The non-blank rows of the blob, RFC 4180 quoting honoured.
    fn rows(&self, path: &Path) -> Result<Vec<Vec<String>>> {
        let mut rdr = ::csv::ReaderBuilder::new()
            .has_headers(false)
            .flexible(true)
            .delimiter(self.delimiter)
            .from_path(path)
            .map_err(|e| fmt_err(format!("open {}: {e}", path.display())))?;
        let mut out = Vec::new();
        for rec in rdr.records() {
            let rec = rec.map_err(|e| fmt_err(format!("{}: {e}", path.display())))?;
            if rec.is_empty() {
                continue;
            }
            out.push(rec.iter().map(str::to_string).collect());
        }
        Ok(out)
    }
}

/// Whether `s` reads as a number the way Python's `float(s.strip())` and
/// Julia's `tryparse(Float64, strip(s))` agree on.
fn parses_float(s: &str) -> bool {
    s.trim().parse::<f64>().is_ok()
}

impl Reader for CsvReader {
    fn formats(&self) -> &'static [&'static str] {
        &["csv"]
    }

    fn extensions(&self) -> &'static [&'static str] {
        &["csv", "txt"]
    }

    fn configured(
        &self,
        options: &serde_json::Map<String, serde_json::Value>,
    ) -> Result<Option<Arc<dyn Reader>>> {
        if options.is_empty() {
            return Ok(None);
        }
        Ok(Some(Arc::new(CsvReader::from_options(options)?)))
    }

    fn read_native(
        &self,
        blob_path: &Path,
        variables: &[String],
        _select: &Selection,
    ) -> Result<NativeDataset> {
        // `supports_selection` is false: the Provider never hands this reader
        // a window, and row selection is esm-spec §8.9.2 work downstream.
        let rows = self.rows(blob_path)?;
        if rows.is_empty() {
            return Ok(NativeDataset::default());
        }
        let header = rows.get(self.header_row).ok_or_else(|| {
            fmt_err(format!(
                "header_row {} is past the {} non-blank rows of {}",
                self.header_row,
                rows.len(),
                blob_path.display()
            ))
        })?;
        let body = &rows[self.header_row + 1..];
        let want: Option<HashSet<&str>> =
            (!variables.is_empty()).then(|| variables.iter().map(String::as_str).collect());
        if let Some(w) = &want {
            let mut missing: Vec<&str> = w
                .iter()
                .copied()
                .filter(|v| !header.iter().any(|h| h == v))
                .collect();
            if !missing.is_empty() {
                missing.sort_unstable();
                return Err(fmt_err(format!(
                    "requested variables not in CSV: {missing:?}; present columns: {header:?}"
                )));
            }
        }
        let numset: Option<HashSet<&str>> = self
            .numeric_columns
            .as_ref()
            .map(|n| n.iter().map(String::as_str).collect());

        let mut variables_out: HashMap<String, NativeField> = HashMap::new();
        for (j, name) in header.iter().enumerate() {
            if want.as_ref().is_some_and(|w| !w.contains(name.as_str())) {
                continue;
            }
            let mut vals: Vec<&str> = Vec::with_capacity(body.len());
            for (i, r) in body.iter().enumerate() {
                let cell = r.get(j).ok_or_else(|| {
                    fmt_err(format!(
                        "row {} has {} fields; column {name:?} is field {}",
                        self.header_row + 1 + i,
                        r.len(),
                        j + 1
                    ))
                })?;
                vals.push(cell.as_str());
            }
            let numeric = match &numset {
                Some(n) => n.contains(name.as_str()),
                None => vals.iter().all(|v| parses_float(v)),
            };
            let n = vals.len();
            let (dtype, data) = if numeric {
                let mut out = Vec::with_capacity(n);
                for (i, v) in vals.iter().enumerate() {
                    out.push(v.trim().parse::<f64>().map_err(|_| {
                        fmt_err(format!(
                            "column {name:?} row {}: cannot parse {v:?} as a number",
                            self.header_row + 1 + i
                        ))
                    })?);
                }
                (DType::Float64, ArrayData::F64(out))
            } else {
                (
                    DType::Str,
                    ArrayData::Str(vals.iter().map(|v| v.to_string()).collect()),
                )
            };
            variables_out.insert(
                name.clone(),
                NativeField {
                    dtype,
                    dims: vec!["index".to_string()],
                    shape: vec![n],
                    data,
                    fill_value: None,
                },
            );
        }
        Ok(NativeDataset {
            variables: variables_out,
            coords: HashMap::new(),
        })
    }
}
