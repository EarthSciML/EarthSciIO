//! Building a loader from an esm `data_sources` entry (`spec/esm-sources.md` §3).
//!
//! [`DataSource::from_esm`] reads the parts of an entry that locate and decode
//! files — `source`, `temporal`, `reader_options` and the reader name in
//! `metadata.esio_format` — into a [`DataSource`]; [`Provider::new`] then binds
//! it to a cache and a run window. What a source's bytes MEAN (`select`,
//! `codes`, `record_filter`, `extent`, a parameter's `unit_conversion`) is the
//! document binding's job (EarthSciAST), which calls this for the bytes.
//!
//! [`Provider::new`]: crate::Provider::new

use std::collections::BTreeMap;

use time::format_description::well_known::Rfc3339;
use time::macros::format_description;
use time::{OffsetDateTime, PrimitiveDateTime, UtcOffset};

use crate::calendar::CalendarPeriod;
use crate::error::{Error, Result};
use crate::esm::{check_esm_template, esm_template_names};
use crate::provider::{DataSource, SourceTemporal};

/// What the caller adds to an esm entry when building a loader from it
/// ([`DataSource::from_esm`]).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EsmSourceOptions {
    /// The on-disk variables to read (empty reads every variable).
    pub variables: Vec<String>,
    /// Bindings for the entry's custom `{name}` substitutions (e.g. NEI's
    /// `{sector}`). `{var}` is bound to the variable when exactly one is read
    /// and this does not bind it.
    pub substitutions: BTreeMap<String, String>,
    /// A replacement for `source.url_template` (a deployment's local copy);
    /// it drops the entry's mirrors.
    pub url_override: Option<String>,
    /// Records per sample (1 or 2): the consumer's choice, not the entry's.
    pub records_per_sample: Option<u32>,
}

fn esm_err(code: &'static str, source: &str, detail: impl Into<String>) -> Error {
    Error::EsmSource {
        code,
        source: source.to_string(),
        detail: detail.into(),
    }
}

/// True when `t` begins with a URL scheme (`^[A-Za-z][A-Za-z0-9+.-]*://`).
fn has_scheme(t: &str) -> bool {
    let Some((scheme, _)) = t.split_once("://") else {
        return false;
    };
    let mut chars = scheme.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphabetic())
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '.' | '-'))
}

/// Check one location: a URL or substitution-led, and no `${…}`
/// (esm-spec §8.2.1; a `{` there would read as a substitution anyway).
fn check_url(name: &str, template: &str) -> Result<()> {
    if template.contains("${") {
        return Err(esm_err(
            "esm_source_url",
            name,
            format!("{template:?} names an environment variable, which esm-spec §8.2.1 refuses"),
        ));
    }
    if !(has_scheme(template) || template.starts_with('{')) {
        return Err(esm_err(
            "esm_source_url",
            name,
            format!(
                "{template:?} is not a URL; load the document with EarthSciAST, which \
                 resolves a relative location against the file that declared it \
                 (esm-spec §8.2.1), or pass url_override"
            ),
        ));
    }
    Ok(())
}

/// One ISO 8601 instant as UTC: RFC 3339, or a date-time or date with no
/// offset (read as UTC).
fn instant(name: &str, field: &str, value: &serde_json::Value) -> Result<OffsetDateTime> {
    let Some(text) = value.as_str() else {
        return Err(esm_err(
            "esm_source_temporal",
            name,
            format!("temporal.{field} must be a string"),
        ));
    };
    let s = text.trim();
    let bad = || {
        esm_err(
            "esm_source_temporal",
            name,
            format!("temporal.{field} {text:?} is not an ISO 8601 instant"),
        )
    };
    if let Ok(t) = OffsetDateTime::parse(s, &Rfc3339) {
        return Ok(t.to_offset(UtcOffset::UTC));
    }
    let s = s.replace(' ', "T");
    let forms = [
        format_description!("[year]-[month]-[day]T[hour]:[minute]:[second].[subsecond]"),
        format_description!("[year]-[month]-[day]T[hour]:[minute]:[second]"),
        format_description!("[year]-[month]-[day]T[hour]:[minute]"),
    ];
    for f in forms {
        if let Ok(t) = PrimitiveDateTime::parse(&s, f) {
            return Ok(t.assume_utc());
        }
    }
    time::Date::parse(&s, format_description!("[year]-[month]-[day]"))
        .map(|d| d.midnight().assume_utc())
        .map_err(|_| bad())
}

fn period(name: &str, field: &str, value: &str) -> Result<CalendarPeriod> {
    CalendarPeriod::parse(value)
        .map_err(|e| esm_err("esm_period_invalid", name, format!("temporal.{field}: {e}")))
}

fn temporal(
    name: &str,
    node: Option<&serde_json::Value>,
    records_per_sample: Option<u32>,
) -> Result<Option<SourceTemporal>> {
    let Some(node) = node.filter(|v| !v.is_null()) else {
        return Ok(None);
    };
    let Some(obj) = node.as_object() else {
        return Err(esm_err(
            "esm_source_temporal",
            name,
            "temporal must be an object",
        ));
    };
    let text = |k: &str| {
        obj.get(k)
            .and_then(serde_json::Value::as_str)
            .filter(|s| !s.is_empty())
    };
    let Some(start_v) = obj
        .get("start")
        .filter(|v| !v.is_null() && v.as_str() != Some(""))
    else {
        return Ok(None); // no anchor: a time-invariant source
    };
    let (freq, fp) = (text("frequency"), text("file_period"));
    let (Some(f), Some(p)) = (freq.or(fp), fp.or(freq)) else {
        return Err(esm_err(
            "esm_source_temporal",
            name,
            format!("temporal anchors at {start_v} but declares neither frequency nor file_period"),
        ));
    };
    let f = period(name, "frequency", f)?;
    let p = period(name, "file_period", p)?;
    let start = instant(name, "start", start_v)?;
    let mut t = SourceTemporal::new(start, f, p).time_dim(text("time_variable").unwrap_or("time"));
    if let Some(end) = obj
        .get("end")
        .filter(|v| !v.is_null() && v.as_str() != Some(""))
    {
        t = t.end(instant(name, "end", end)?);
    }
    if let Some(n) = records_per_sample {
        t = t.records_per_sample(n);
    }
    if let Some(rpf) = obj.get("records_per_file").filter(|v| !v.is_null()) {
        if rpf.as_str() != Some("auto") {
            // The records of the first file: frequency anchors in [start, next file).
            let next_file = t.next_file_anchor(start);
            let mut n: i64 = 0;
            let mut a = t.first_record_anchor_from(start);
            while a < next_file {
                n += 1;
                a = t.next_record_anchor(a);
            }
            if rpf.as_i64() != Some(n) {
                return Err(esm_err(
                    "esm_source_records_per_file",
                    name,
                    format!(
                        "temporal.records_per_file is {rpf} but file_period / frequency \
                         gives {n} records per file"
                    ),
                ));
            }
        }
    }
    Ok(Some(t))
}

impl DataSource {
    /// The loader an esm `data_sources` entry named `name` declares
    /// (`spec/esm-sources.md` §3). `entry` is the entry as a document carries
    /// it after EarthSciAST has loaded it (locations absolute, a mount's
    /// `url_substitutions` applied).
    ///
    /// Errors are [`Error::EsmSource`] (codes `esm_source_*` and
    /// `esm_period_invalid`) or, for a template, [`Error::BadUrl`] carrying an
    /// `esm_template_*` code; each message names the source or the template.
    pub fn from_esm(
        name: &str,
        entry: &serde_json::Value,
        opts: &EsmSourceOptions,
    ) -> Result<DataSource> {
        let Some(obj) = entry.as_object() else {
            return Err(esm_err(
                "esm_source_invalid",
                name,
                "the entry must be an object",
            ));
        };
        let fmt = obj
            .get("metadata")
            .and_then(|m| m.get("esio_format"))
            .and_then(serde_json::Value::as_str)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| {
                esm_err(
                    "esm_source_no_format",
                    name,
                    "declares no metadata.esio_format, so the reader is unknown",
                )
            })?;
        let src = obj.get("source");
        let template = match &opts.url_override {
            Some(u) => u.clone(),
            None => src
                .and_then(|s| s.get("url_template"))
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
                .ok_or_else(|| esm_err("esm_source_url", name, "has no source.url_template"))?,
        };
        let mirrors: Vec<String> = if opts.url_override.is_some() {
            Vec::new()
        } else {
            match src.and_then(|s| s.get("mirrors")) {
                None | Some(serde_json::Value::Null) => Vec::new(),
                Some(serde_json::Value::Array(a)) => a
                    .iter()
                    .map(|m| {
                        m.as_str().map(str::to_string).ok_or_else(|| {
                            esm_err("esm_source_url", name, "every mirror must be a string")
                        })
                    })
                    .collect::<Result<_>>()?,
                Some(_) => {
                    return Err(esm_err(
                        "esm_source_url",
                        name,
                        "source.mirrors must be an array",
                    ))
                }
            }
        };
        check_url(name, &template)?;
        for m in &mirrors {
            check_url(name, m)?;
        }
        let mut subs = opts.substitutions.clone();
        let needs_var = std::iter::once(&template)
            .chain(mirrors.iter())
            .any(|t| esm_template_names(t).iter().any(|n| n == "var"));
        if needs_var && !subs.contains_key("var") && opts.variables.len() == 1 {
            subs.insert("var".to_string(), opts.variables[0].clone());
        }
        for t in std::iter::once(&template).chain(mirrors.iter()) {
            check_esm_template(t, &subs)?;
        }
        let reader_options = match obj.get("reader_options") {
            None | Some(serde_json::Value::Null) => serde_json::Map::new(),
            Some(serde_json::Value::Object(m)) => m.clone(),
            Some(_) => {
                return Err(esm_err(
                    "esm_source_invalid",
                    name,
                    "reader_options must be an object",
                ))
            }
        };
        let mut out = DataSource::new(name, fmt, template)
            .variables(opts.variables.clone())
            .mirrors(mirrors)
            .reader_options(reader_options)
            .esm_template(subs);
        if let Some(t) = temporal(name, obj.get("temporal"), opts.records_per_sample)? {
            out = out.temporal(t);
        }
        Ok(out)
    }
}
