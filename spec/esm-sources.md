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
and tried, in order, after the primary URL. Building the template refuses
`esm_template_reserved`, and `esm_template_unbound` or `esm_template_directive`
for any non-date token of the primary or of a mirror, by checking each token
where it stands (the text around a `{date:…}` token never forms a new token:
`{{date:%Y}x}` has one token). The `{date:…}` tokens are checked when expanded.
The scenario's `construction` cases pin this. The cache key is still
`sha256(primary URL)` (cache-format.md §1), whichever candidate supplied the
bytes.

## 2. Calendar cadences (esm-spec §8.3)

Fixture:
[`../conformance/corpus/scenarios/calendar-cadence.json`](../conformance/corpus/scenarios/calendar-cadence.json).

A source's `temporal.frequency` and `temporal.file_period` are ISO 8601
durations. A month or a year is not a fixed number of seconds, so EarthSciIO
reads each as a **calendar period** (Python `CalendarPeriod`, Julia
`CalendarPeriod`, Rust `CalendarPeriod`) and computes every anchor on the
proleptic Gregorian calendar in UTC. No track approximates a month by a mean
length.

### 2.1 Periods

`P[nY][nM][nW][nD][T[nH][nM][nS]]`, each `n` an unsigned integer and the
designators in this order, is the period of `months = 12·Y + M` and
`seconds = 604800·W + 86400·D + 3600·H + 60·M + S` (UTC days have 86400
seconds). Anything else — a fraction, a sign, a missing `P`, an empty `T`, a
designator out of place, an all-zero period — is an error with the code
`esm_period_invalid`. A period may also be built from a fixed duration
(a Python `timedelta`, a Rust `time::Duration`, a whole number of seconds),
which is `months = 0`.

### 2.2 Anchors

For a cadence with epoch `start` and period `p`, anchor `k` (any integer) is

```
anchor(k) = add_months(start, k · p.months) + k · p.seconds
```

where `add_months` moves the calendar month, keeps the time of day, and keeps
the day of the month, clamped to the last day of the target month. Every anchor
is computed from `start`, never from the previous anchor, so a cadence starting
on 31 January visits 29 February and then 31 March. `floor(t)` is the largest
`k` with `anchor(k) ≤ t`, and `ceil(t)` the smallest with `anchor(k) ≥ t`.

### 2.3 What the provider derives

With `f` the frequency and `F` the file period, both anchored at `start`:

| Quantity | Rule |
|---|---|
| refresh times over `[lower, upper)` | `anchor_f(k)` for `k = ceil_f(lower), …` while `< upper` (as before) |
| record anchor of a time `t` | `anchor_f(floor_f(t))` |
| file anchor of a record anchor `a` | `anchor_F(floor_F(a))` — the URL is expanded here |
| record index in its file | `floor_f(a) − ceil_f(file anchor)`, 0-based |
| next record (two-record bracket) | `anchor_f(floor_f(t) + 1)` |

A fixed-length cadence (`months = 0`) gives exactly the anchors it gave
before. The Python track still prefers the file's own time axis to locate a
record when it can decode one (provider.py `_record_index`); the rule above is
what all three compute otherwise, and what they agree on.

### 2.4 Julia

The Julia `Provider` used to take only a caller-made list of `times` in the
caller's unit, and located a record as `mod1(tick, file length)`, which is
wrong when the list does not start at a file's first record. It now also takes
a `SourceTemporal` and a run `window`, as Python and Rust do:
`Provider(cache, url; format, temporal, window)`. Times are then Unix seconds
(UTC). `refresh_times` returns §2.3's refresh times over the window, `upper`
being the window end (else `stop`) and never clipped to `stop`, as in Python
and Rust. A sample at any time from `start` on, inside the window or not, is
located by §2.3 on the calendar, as in Python and Rust.
