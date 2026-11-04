//! esm data-source URL templates (`spec/esm-sources.md` §1).
//!
//! An esm `source.url_template` (and every `source.mirrors` entry) is literal
//! text with `{date:<format>}` and `{name}` tokens (esm-spec §8.2). This module
//! expands one exactly as the Python and Julia tracks do: the date directives
//! are implemented here rather than through the `time` crate's format
//! description, whose syntax is not the esm one.

use std::collections::BTreeMap;
use std::fmt;

use time::OffsetDateTime;

/// An esm URL template could not be expanded. `code` is one of the
/// `esm_template_*` codes of `spec/esm-sources.md` §1.3; the message (its
/// `Display`) contains it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EsmTemplateError {
    /// The §1.3 error code, e.g. `"esm_template_unbound"`.
    pub code: &'static str,
    /// The template being expanded.
    pub template: String,
    /// What went wrong, naming the token or directive.
    pub detail: String,
}

impl fmt::Display for EsmTemplateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}: {} in esm URL template {:?}",
            self.code, self.detail, self.template
        )
    }
}

impl std::error::Error for EsmTemplateError {}

impl From<EsmTemplateError> for crate::Error {
    fn from(e: EsmTemplateError) -> Self {
        crate::Error::BadUrl {
            url: e.template.clone(),
            detail: e.to_string(),
        }
    }
}

fn err(code: &'static str, template: &str, detail: impl Into<String>) -> EsmTemplateError {
    EsmTemplateError {
        code,
        template: template.to_string(),
        detail: detail.into(),
    }
}

/// One token: its byte span in the template, its name and its optional format.
struct Token<'a> {
    start: usize,
    end: usize,
    name: &'a str,
    format: Option<&'a str>,
}

/// The tokens of `template`, left to right: `{name}` or `{name:format}` with
/// `name` matching `[A-Za-z_][A-Za-z0-9_]*` and `format` free of braces. Any
/// other brace is literal text.
fn tokens(template: &str) -> Vec<Token<'_>> {
    let b = template.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i] != b'{' {
            i += 1;
            continue;
        }
        let name_start = i + 1;
        let mut j = name_start;
        if j < b.len() && (b[j].is_ascii_alphabetic() || b[j] == b'_') {
            j += 1;
            while j < b.len() && (b[j].is_ascii_alphanumeric() || b[j] == b'_') {
                j += 1;
            }
            let name = &template[name_start..j];
            if j < b.len() && b[j] == b'}' {
                out.push(Token {
                    start: i,
                    end: j + 1,
                    name,
                    format: None,
                });
                i = j + 1;
                continue;
            }
            if j < b.len() && b[j] == b':' {
                let fmt_start = j + 1;
                let mut k = fmt_start;
                while k < b.len() && b[k] != b'{' && b[k] != b'}' {
                    k += 1;
                }
                if k < b.len() && b[k] == b'}' {
                    out.push(Token {
                        start: i,
                        end: k + 1,
                        name,
                        format: Some(&template[fmt_start..k]),
                    });
                    i = k + 1;
                    continue;
                }
            }
        }
        i += 1;
    }
    out
}

fn format_date(template: &str, fmt: &str, t: OffsetDateTime) -> Result<String, EsmTemplateError> {
    let mut out = String::new();
    let mut chars = fmt.chars();
    while let Some(c) = chars.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        let Some(d) = chars.next() else {
            return Err(err(
                "esm_template_directive",
                template,
                format!("a '%' ends the date format {fmt:?}"),
            ));
        };
        match d {
            'Y' => {
                let y = t.year();
                if !(0..=9999).contains(&y) {
                    return Err(err(
                        "esm_template_year_range",
                        template,
                        format!("%Y of year {y} is outside 0 to 9999"),
                    ));
                }
                out.push_str(&format!("{y:04}"));
            }
            'y' => out.push_str(&format!("{:02}", t.year().rem_euclid(100))),
            'm' => out.push_str(&format!("{:02}", u8::from(t.month()))),
            'd' => out.push_str(&format!("{:02}", t.day())),
            'j' => out.push_str(&format!("{:03}", t.ordinal())),
            'H' => out.push_str(&format!("{:02}", t.hour())),
            'M' => out.push_str(&format!("{:02}", t.minute())),
            'S' => out.push_str(&format!("{:02}", t.second())),
            '%' => out.push('%'),
            other => {
                return Err(err(
                    "esm_template_directive",
                    template,
                    format!(
                        "unsupported date directive %{other} \
                         (supported: %Y %y %m %d %j %H %M %S %%)"
                    ),
                ))
            }
        }
    }
    Ok(out)
}

/// Expand an esm URL template (`spec/esm-sources.md` §1) at `anchor` (a UTC
/// instant; an offset is converted to UTC) with `substitutions`.
pub fn expand_esm_template(
    template: &str,
    anchor: Option<OffsetDateTime>,
    substitutions: &BTreeMap<String, String>,
) -> Result<String, EsmTemplateError> {
    if substitutions.contains_key("date") {
        return Err(err(
            "esm_template_reserved",
            template,
            "the substitution name 'date' is reserved for {date:<format>}",
        ));
    }
    let when = anchor.map(|a| a.to_offset(time::UtcOffset::UTC));
    let mut out = String::with_capacity(template.len());
    let mut last = 0;
    for tok in tokens(template) {
        out.push_str(&template[last..tok.start]);
        if tok.name == "date" {
            let fmt = tok.format.filter(|f| !f.is_empty()).ok_or_else(|| {
                err(
                    "esm_template_directive",
                    template,
                    "{date} needs a format, e.g. {date:%Y%m%d}",
                )
            })?;
            let t = when.ok_or_else(|| {
                err(
                    "esm_template_no_anchor",
                    template,
                    "{date:...} needs a time and none was given",
                )
            })?;
            out.push_str(&format_date(template, fmt, t)?);
        } else {
            if let Some(f) = tok.format {
                return Err(err(
                    "esm_template_directive",
                    template,
                    format!(
                        "the substitution {{{}}} takes no format (got {{{}:{f}}})",
                        tok.name, tok.name
                    ),
                ));
            }
            let v = substitutions.get(tok.name).ok_or_else(|| {
                err(
                    "esm_template_unbound",
                    template,
                    format!("the substitution {{{}}} is not bound", tok.name),
                )
            })?;
            out.push_str(v);
        }
        last = tok.end;
    }
    out.push_str(&template[last..]);
    Ok(out)
}

/// The substitution names `template` needs, in order of first appearance,
/// without `date`.
pub fn esm_template_names(template: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for tok in tokens(template) {
        if tok.name != "date" && !out.iter().any(|n| n == tok.name) {
            out.push(tok.name.to_string());
        }
    }
    out
}

/// Check every non-date token of `template` against `substitutions` without
/// an anchor: a reserved, unbound or formatted substitution fails here, at
/// construction, rather than at the first fetch.
pub(crate) fn check_esm_template(
    template: &str,
    substitutions: &BTreeMap<String, String>,
) -> Result<(), EsmTemplateError> {
    if substitutions.contains_key("date") {
        return expand_esm_template(template, None, substitutions).map(|_| ());
    }
    for tok in tokens(template) {
        if tok.name == "date" {
            continue;
        }
        if let Some(f) = tok.format {
            return Err(err(
                "esm_template_directive",
                template,
                format!(
                    "the substitution {{{}}} takes no format (got {{{}:{f}}})",
                    tok.name, tok.name
                ),
            ));
        }
        if !substitutions.contains_key(tok.name) {
            return Err(err(
                "esm_template_unbound",
                template,
                format!("the substitution {{{}}} is not bound", tok.name),
            ));
        }
    }
    Ok(())
}
