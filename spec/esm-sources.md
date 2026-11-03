# Reading an esm data source

Status: normative. Fixture:
[`../conformance/corpus/scenarios/esm-url-templates.json`](../conformance/corpus/scenarios/esm-url-templates.json).

An `.esm` document names its inputs in `data_sources` (esm-spec §8). This page
fixes how EarthSciIO reads the parts of such an entry that locate files, so
that the Python, Julia and Rust tracks fetch the same file for the same entry
at the same time.

## 1. URL templates (esm-spec §8.2)

A `source.url_template`, and every entry of `source.mirrors`, is an **esm
template**: literal text with `{…}` tokens. EarthSciIO expands one with
`expand_esm_template(template, anchor, substitutions)` (Python and Julia) and
`expand_esm_template(template, Some(anchor), &substitutions)` (Rust), where
`anchor` is a UTC instant (the start of the file the URL names) and
`substitutions` maps names to strings.

### 1.1 Tokens

A token is `{name}` or `{name:format}`, where `name` matches
`[A-Za-z_][A-Za-z0-9_]*` and `format` is any text without `{` or `}`. Any other
brace is literal text, so the JSON in a `cds://` request (`{"variable": …}`)
passes through unchanged. Expansion is one left-to-right pass; inserted text is
never expanded again.

| Token | Expands to |
|---|---|
| `{date:<format>}` | `anchor` formatted by `<format>` (§1.2), in UTC |
| `{name}` | `substitutions[name]`, inserted verbatim |

### 1.2 Date directives

`<format>` is literal text and these directives. Every track implements this
table itself rather than calling its platform `strftime`, whose locale and
padding rules differ.

| Directive | Value | Example (2016-02-29T07:05:09Z) |
|---|---|---|
| `%Y` | year, 4 digits, zero-padded; years 0 to 9999 only | `2016` |
| `%y` | year modulo 100, 2 digits | `16` |
| `%m` | month 01–12 | `02` |
| `%d` | day of month 01–31 | `29` |
| `%j` | day of year 001–366 | `060` |
| `%H` | hour 00–23 | `07` |
| `%M` | minute 00–59 | `05` |
| `%S` | second 00–59 (fractional seconds are dropped, never rounded) | `09` |
| `%%` | a literal `%` | `%` |

### 1.3 Errors

Each is an error whose message contains its code, names the template, and
names the offending token or directive. No error is ever turned into a skipped
mirror or a literal URL.

| Code | When |
|---|---|
| `esm_template_unbound` | a `{name}` token whose name is not in `substitutions` (the message names it) |
| `esm_template_directive` | a `%` directive outside §1.2, a `%` at the end of a format, `{date}` or `{date:}` without a format, or a format on a non-date token (`{sector:x}`) |
| `esm_template_no_anchor` | a `{date:…}` token when no anchor was given |
| `esm_template_year_range` | `%Y` of a year outside 0–9999 |
| `esm_template_reserved` | `substitutions` binds the name `date` |

### 1.4 Custom substitutions

esm-spec §8.2 allows any name. A document that mounts a catalogue binds names
at load through the mount edge's `url_substitutions`, so a document that
EarthSciAST has loaded may carry none. Whatever is left (reseact's NEI 2016
source keeps `{sector}`) must be bound by the caller of EarthSciIO; an
unbound one is `esm_template_unbound` rather than a fetch of a URL with braces
in it. `esm_template_names(template)` returns the names a template needs,
in order of first appearance and without `date`, so a caller can check them
before any fetch.

### 1.5 Using a template in a provider

A loader whose URL is an esm template is built with `EsmTemplate` (Python
`EsmTemplate(template, substitutions)`, Julia `EsmTemplate(template;
substitutions)`, Rust `DataSource::new(name, format, template).esm_template(substitutions)`).
The provider expands it at each file anchor, as it already does for the
track-specific template forms. Mirrors given beside an esm template are esm
templates too: each is expanded at the same anchor with the same substitutions
and tried, in order, after the primary URL. The cache key is still
`sha256(primary URL)` (cache-format.md §1), whichever candidate supplied the
bytes.
