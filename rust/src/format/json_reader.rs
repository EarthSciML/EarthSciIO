//! The active `json` reader (`spec/registries.md` §2, "JSON points"): a JSON
//! array of records becomes a points table, as the Python `JSONReader` and the
//! Julia `JSONReader` decode it. One contract, pinned by
//! `conformance/corpus/scenarios/json-reader.json`, which all three suites run.
//!
//! The records are the array at `records_path` (object keys from the document
//! root; default none, the root itself). Each record must be an object. Its
//! fields are its leaves, a nested object flattened with its keys joined by `.`
//! (`coordinates.latitude`). The fields are the union over the records; a field
//! a record lacks is `null` there. Each field is one rank-1 array on `index`;
//! there are no coordinates.
//!
//! A field's type comes from its non-null values: numbers are `float64` (`null`
//! is NaN), strings are `string` (`null` is `""`), booleans are `bool` (a `null`
//! is an error, since a boolean has no missing value), and a field of nothing but
//! `null` is `float64` NaN. A field mixing kinds is an error. A field named in
//! `numeric_columns` is `float64` whatever its cells: a number as is, `null` as
//! NaN, a string holding a decimal number (after trimming ASCII whitespace)
//! parsed, anything else an error. `variables` projects the fields, and a field
//! that is not decoded may hold anything, an array included.
//!
//! Every error's detail starts with its code (`json_parse`,
//! `json_records_path`, `json_record_not_object`, `json_field_conflict`,
//! `json_nested_array`, `json_mixed_types`, `json_null_bool`,
//! `json_numeric_text`, `json_variables_missing`, `json_option`), the same codes
//! the Python and Julia readers raise.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;

use serde_json::Value;

use crate::error::{Error, Result};
use crate::format::{ArrayData, DType, NativeDataset, NativeField, Reader, Selection};

fn json_err(code: &str, detail: impl AsRef<str>) -> Error {
    Error::Format {
        format: "json".to_string(),
        detail: format!("{code}: {}", detail.as_ref()),
    }
}

/// The `json` format reader. The default instance reads the records from the
/// document root and infers every field's type.
#[derive(Debug, Clone, Default)]
pub struct JsonReader {
    records_path: Vec<String>,
    numeric_columns: HashSet<String>,
}

impl JsonReader {
    /// The default reader: records at the root, types inferred.
    pub fn new() -> Self {
        Self::default()
    }

    /// The object keys leading from the document root to the records array.
    pub fn records_path<I, S>(mut self, keys: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.records_path = keys.into_iter().map(Into::into).collect();
        self
    }

    /// Read these fields as `float64`, parsing a string cell that holds a number.
    pub fn numeric_columns<I, S>(mut self, names: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.numeric_columns = names.into_iter().map(Into::into).collect();
        self
    }

    fn from_options(options: &serde_json::Map<String, Value>) -> Result<Self> {
        let mut r = JsonReader::new();
        for (k, v) in options {
            match k.as_str() {
                "records_path" => r = r.records_path(str_list(k, v)?),
                "numeric_columns" => r = r.numeric_columns(str_list(k, v)?),
                other => {
                    return Err(json_err(
                        "json_option",
                        format!(
                            "unknown reader option {other:?}; json takes records_path, \
                             numeric_columns"
                        ),
                    ));
                }
            }
        }
        Ok(r)
    }
}

fn str_list(option: &str, v: &Value) -> Result<Vec<String>> {
    if v.is_null() {
        return Ok(Vec::new());
    }
    let bad = || {
        json_err(
            "json_option",
            format!("reader option {option:?} must be an array of strings"),
        )
    };
    v.as_array()
        .ok_or_else(bad)?
        .iter()
        .map(|x| x.as_str().map(str::to_string).ok_or_else(bad))
        .collect()
}

fn kind(v: Option<&Value>) -> &'static str {
    match v {
        None | Some(Value::Null) => "null",
        Some(Value::Bool(_)) => "bool",
        Some(Value::Number(_)) => "number",
        Some(Value::String(_)) => "string",
        Some(Value::Array(_)) => "array",
        Some(Value::Object(_)) => "object",
    }
}

/// The one text a `numeric_columns` string cell may hold: a JSON-number-shaped
/// decimal `[+-]?(digits[.digits*] | .digits)([eE][+-]?digits)?`, so no track
/// accepts `nan`, `inf` or `1_000` that another refuses.
fn is_numeric_text(s: &str) -> bool {
    let b = s.as_bytes();
    let mut i = 0;
    if i < b.len() && (b[i] == b'+' || b[i] == b'-') {
        i += 1;
    }
    let int_start = i;
    while i < b.len() && b[i].is_ascii_digit() {
        i += 1;
    }
    let int_digits = i - int_start;
    let mut frac_digits = 0;
    if i < b.len() && b[i] == b'.' {
        i += 1;
        let f = i;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
        frac_digits = i - f;
    }
    if int_digits == 0 && frac_digits == 0 {
        return false;
    }
    if i < b.len() && (b[i] == b'e' || b[i] == b'E') {
        i += 1;
        if i < b.len() && (b[i] == b'+' || b[i] == b'-') {
            i += 1;
        }
        let e = i;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
        if i == e {
            return false;
        }
    }
    i == b.len()
}

fn trim_ascii_ws(s: &str) -> &str {
    s.trim_matches(|c| matches!(c, ' ' | '\t' | '\n' | '\r'))
}

fn finite(f: f64, field: &str, row: usize, shown: &dyn std::fmt::Display) -> Result<f64> {
    if f.is_infinite() {
        return Err(json_err(
            "json_parse",
            format!("field {field:?} record {row}: number {shown} is outside the float64 range"),
        ));
    }
    // A zero is +0.0 whatever its sign: serde_json parses the integer `-0` to
    // -0.0 and cannot tell it from `-0.0`, so no track keeps the sign of either.
    Ok(if f == 0.0 { 0.0 } else { f })
}

fn number(n: &serde_json::Number, field: &str, row: usize) -> Result<f64> {
    // `as_f64` is the nearest float64 (ties to even) for an i64/u64; a longer
    // integer was already parsed to f64 by serde_json.
    let f = n.as_f64().unwrap_or(f64::INFINITY);
    finite(f, field, row, n)
}

/// A record's leaves, borrowed from the parsed document: a field that is never
/// decoded (a large `sensors` array) is never copied.
fn flatten<'a>(
    obj: &'a serde_json::Map<String, Value>,
    prefix: &str,
    flat: &mut HashMap<String, &'a Value>,
    objpaths: &mut HashSet<String>,
    row: usize,
) -> Result<()> {
    for (k, v) in obj {
        let name = format!("{prefix}{k}");
        if let Value::Object(inner) = v {
            objpaths.insert(name.clone());
            flatten(inner, &format!("{name}."), flat, objpaths, row)?;
            continue;
        }
        if flat.contains_key(&name) {
            return Err(json_err(
                "json_field_conflict",
                format!("record {row}: field {name:?} appears twice once flattened"),
            ));
        }
        flat.insert(name, v);
    }
    Ok(())
}

fn column(name: &str, vals: &[Option<&Value>], numeric: bool) -> Result<(DType, ArrayData)> {
    if let Some(row) = vals.iter().position(|v| matches!(v, Some(Value::Array(_)))) {
        return Err(json_err(
            "json_nested_array",
            format!(
                "field {name:?} record {row} is an array; a points field is one value per \
                 record (leave the field out of `variables`)"
            ),
        ));
    }
    if numeric {
        let mut out = Vec::with_capacity(vals.len());
        for (i, v) in vals.iter().enumerate() {
            out.push(match v {
                None | Some(Value::Null) => f64::NAN,
                Some(Value::Number(n)) => number(n, name, i)?,
                Some(Value::String(s)) if is_numeric_text(trim_ascii_ws(s)) => {
                    let t = trim_ascii_ws(s);
                    let f = t.parse::<f64>().map_err(|_| {
                        json_err(
                            "json_numeric_text",
                            format!("field {name:?} record {i}: cannot read {s:?} as a number"),
                        )
                    })?;
                    finite(f, name, i, &t)?
                }
                Some(other) => {
                    return Err(json_err(
                        "json_numeric_text",
                        format!("field {name:?} record {i}: cannot read {other} as a number"),
                    ));
                }
            });
        }
        return Ok((DType::Float64, ArrayData::F64(out)));
    }
    let present: BTreeSet<&str> = vals
        .iter()
        .map(|v| kind(*v))
        .filter(|k| *k != "null")
        .collect();
    if present.len() > 1 {
        return Err(json_err(
            "json_mixed_types",
            format!("field {name:?} mixes JSON kinds {present:?}"),
        ));
    }
    match present.iter().next().copied().unwrap_or("null") {
        "number" | "null" => {
            let mut out = Vec::with_capacity(vals.len());
            for (i, v) in vals.iter().enumerate() {
                out.push(match v {
                    Some(Value::Number(n)) => number(n, name, i)?,
                    _ => f64::NAN,
                });
            }
            Ok((DType::Float64, ArrayData::F64(out)))
        }
        "string" => Ok((
            DType::Str,
            ArrayData::Str(
                vals.iter()
                    .map(|v| match v {
                        Some(Value::String(s)) => s.clone(),
                        _ => String::new(),
                    })
                    .collect(),
            ),
        )),
        _ => {
            let mut out = Vec::with_capacity(vals.len());
            for (i, v) in vals.iter().enumerate() {
                match v {
                    Some(Value::Bool(b)) => out.push(*b),
                    _ => {
                        return Err(json_err(
                            "json_null_bool",
                            format!(
                                "field {name:?} record {i} is null in a boolean field, which \
                                 has no missing value"
                            ),
                        ));
                    }
                }
            }
            Ok((DType::Bool, ArrayData::Bool(out)))
        }
    }
}

impl Reader for JsonReader {
    fn formats(&self) -> &'static [&'static str] {
        &["json"]
    }

    fn extensions(&self) -> &'static [&'static str] {
        &["json"]
    }

    fn configured(
        &self,
        options: &serde_json::Map<String, Value>,
    ) -> Result<Option<Arc<dyn Reader>>> {
        if options.is_empty() {
            return Ok(None);
        }
        Ok(Some(Arc::new(JsonReader::from_options(options)?)))
    }

    fn read_native(
        &self,
        blob_path: &Path,
        variables: &[String],
        select: &Selection,
    ) -> Result<NativeDataset> {
        // `supports_selection` is false, so the Provider refuses a window before
        // the fetch; a direct caller gets the json_option the other tracks raise.
        // Row selection is esm-spec §8.9.2 work downstream.
        if !matches!(select, Selection::All) {
            return Err(json_err(
                "json_option",
                "the json reader supports no selection (supports_selection is false)",
            ));
        }
        let bytes = std::fs::read(blob_path)
            .map_err(|e| json_err("json_parse", format!("open {}: {e}", blob_path.display())))?;
        let text = std::str::from_utf8(&bytes).map_err(|e| {
            json_err(
                "json_parse",
                format!("{}: not UTF-8 ({e})", blob_path.display()),
            )
        })?;
        let doc: Value = serde_json::from_str(text)
            .map_err(|e| json_err("json_parse", format!("{}: {e}", blob_path.display())))?;

        let mut node = &doc;
        for (depth, key) in self.records_path.iter().enumerate() {
            let place = if depth == 0 {
                "the root".to_string()
            } else {
                format!("records_path {:?}", &self.records_path[..depth])
            };
            let obj = node.as_object().ok_or_else(|| {
                json_err(
                    "json_records_path",
                    format!("{place} is a JSON {}, not an object", kind(Some(node))),
                )
            })?;
            node = obj.get(key).ok_or_else(|| {
                let members: BTreeSet<&String> = obj.keys().collect();
                json_err(
                    "json_records_path",
                    format!("{place} has no member {key:?}; members: {members:?}"),
                )
            })?;
        }
        let records = node.as_array().ok_or_else(|| {
            let place = if self.records_path.is_empty() {
                "the root".to_string()
            } else {
                format!("records_path {:?}", self.records_path)
            };
            json_err(
                "json_records_path",
                format!(
                    "{place} is a JSON {}, not an array of records",
                    kind(Some(node))
                ),
            )
        })?;

        let mut flats: Vec<HashMap<String, &Value>> = Vec::with_capacity(records.len());
        let mut objpaths: HashSet<String> = HashSet::new();
        let mut names: BTreeSet<String> = BTreeSet::new();
        for (i, rec) in records.iter().enumerate() {
            let obj = rec.as_object().ok_or_else(|| {
                json_err(
                    "json_record_not_object",
                    format!("record {i} is a JSON {}, not an object", kind(Some(rec))),
                )
            })?;
            let mut flat = HashMap::new();
            flatten(obj, "", &mut flat, &mut objpaths, i)?;
            names.extend(flat.keys().cloned());
            flats.push(flat);
        }

        let decode: Vec<String> = if variables.is_empty() {
            names.iter().cloned().collect()
        } else {
            let want: BTreeSet<String> = variables.iter().cloned().collect();
            let missing: Vec<&String> = want.iter().filter(|v| !names.contains(*v)).collect();
            if !missing.is_empty() {
                return Err(json_err(
                    "json_variables_missing",
                    format!(
                        "requested variables not in the JSON records: {missing:?}; present \
                         fields: {names:?}"
                    ),
                ));
            }
            want.into_iter().collect()
        };

        let mut out: HashMap<String, NativeField> = HashMap::new();
        for name in decode {
            if objpaths.contains(&name) {
                return Err(json_err(
                    "json_field_conflict",
                    format!("field {name:?} is a value in one record and an object in another"),
                ));
            }
            let vals: Vec<Option<&Value>> = flats.iter().map(|f| f.get(&name).copied()).collect();
            let (dtype, data) = column(&name, &vals, self.numeric_columns.contains(&name))?;
            out.insert(
                name,
                NativeField {
                    dtype,
                    dims: vec!["index".to_string()],
                    shape: vec![flats.len()],
                    data,
                    fill_value: None,
                },
            );
        }
        Ok(NativeDataset {
            variables: out,
            coords: HashMap::new(),
        })
    }
}
