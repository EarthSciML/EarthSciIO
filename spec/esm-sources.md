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
| refresh times over `[lower, upper)` | `anchor_f(k)` for `k = ceil_f(lower), …` while `< upper` (as before); possibly none |
| records serving `[lower, upper)` | §2.6 |
| record anchor of a time `t` | `anchor_f(floor_f(t))` |
| file anchor of a record anchor `a` | `anchor_F(floor_F(a))` — the URL is expanded here |
| record index in its file | `floor_f(a) − ceil_f(file anchor)`, 0-based |
| next record (two-record bracket) | `anchor_f(floor_f(t) + 1)` |

A fixed-length cadence (`months = 0`) gives exactly the anchors it gave
before. The record index in its file is the cadence offset: it is what every
track uses when the file has no time axis it can decode. When it has one, the
record is found on that axis instead (§2.5).

### 2.4 Julia

The Julia `Provider` used to take only a caller-made list of `times` in the
caller's unit, and located a record as `mod1(tick, file length)`, which is
wrong when the list does not start at a file's first record. It now also takes
a `SourceTemporal` and a run `window`, as Python and Rust do:
`Provider(cache, url; format, temporal, window)`. Times are then Unix seconds
(UTC). `refresh_times` returns §2.3's refresh times over the window, `upper`
being the window end (else `stop`), as in Python and Rust; a window is checked
as in §2.6 and may have no refresh times. A sample at any time from `start`
on, inside the window or not, is located by §2.3 on the calendar, its record in
the covering file found as in §2.5, as in Python and Rust.

### 2.5 The file's own time axis

Fixture:
[`../conformance/corpus/scenarios/decoded-time-axis.json`](../conformance/corpus/scenarios/decoded-time-axis.json),
written by `conformance/gen_time_axis.py`.

The cadence offset of §2.3 is exact for whole files, but wrong for a file
that lacks some records (a trimmed file) or whose records are not on the
cadence. So every track finds the record of an anchor `a` in the file covering
it this way (Python `provider._record_index`, Julia `_axis_record`, Rust
`time_axis::record_index`):

1. Take the file's coordinate named `time_dim`. If there is none, or it does
   not decode (below), or it is empty, use the cadence offset.
2. If `a` lies between the axis's earliest and latest times (inclusive), the
   record is the latest one at or before `a`; of equal times, the first in file
   order. The axis need not be sorted.
3. Otherwise use the cadence offset. A cadence offset outside the file is an
   error; one inside it is read, even before a trimmed file's first record.

The two-record bracket's successor is the record of the next anchor in the
file covering it (the same file or the next one), found as above, so a record
and its time agree: in a file with a gap the successor of the last record
before the gap is that record again, never the one after the gap. When the
successor's file cannot be read or its record is outside that file, the floor
record is held (`[r, r]`, equal times). The bracket's time coordinate is
always the two anchors, never the file's own times.

**Decoding.** `decode_time_axis(values, units, calendar)` (Python and Julia;
Rust `decode_time_axis(&values, units, calendar, fill)`) gives the axis as
Unix seconds, floored, or nothing. It is the Python track's decode (xarray's
`decode_cf_datetime`), which the other two reproduce:

| Part | Rule |
|---|---|
| `units` | `<unit> since <reference>`, split at the last ` since ` with text on both sides; each part trimmed. No `units`, or no such split: not decoded |
| `calendar` | absent is `standard`. `standard`, `gregorian` and `proleptic_gregorian`, in any case, decode; any other calendar does not |
| unit, first reading | lower-cased, with an `s` added when it does not end in one: `nanoseconds`, `microseconds`, `milliseconds`, `seconds`, `minutes`, `hours`, `days`. An integer is multiplied exactly; a float is multiplied in double precision and truncated to the nanosecond. Used when the reference and every time fit in signed 64-bit nanoseconds |
| unit, second reading | otherwise, the lower-cased unit is one of `microsecond(s)`, `millisecond(s)`, `msec(s)`, `ms`, `second(s)`, `sec(s)`, `s`, `minute(s)`, `min(s)`, `hour(s)`, `hr(s)`, `h`, `day(s)`, `d`, and the value is rounded to the nearest microsecond and added to the reference as cftime reads it (below). Every date must fall in the years 1678 to 2261. Any other unit (`weeks`, `months`, `years`, …): not decoded |
| reference | `Y-M-D` with a year of one to four digits (a shorter year only when no other run of four digits follows), then optionally `T` or spaces and a time `HH`, `H:M`, `H:M:S` or `H:M:S.f…`, then optionally spaces and `Z`, `UTC`, `GMT` or an offset `±HH`, `±HHMM`, `±HH:MM`, which is subtracted. The first reading (pandas) needs a four-digit year, reads the date on the proleptic Gregorian calendar and applies every part. The second (cftime) reads the date on the calendar: on `standard` and `gregorian` a date before 1582-10-15 is a Julian date (there is no year 0, and 1582-10-05 to 10-14 do not exist), on `proleptic_gregorian` it is Gregorian (year 0 exists); it reads a time only when it has minutes and is one character (`T` or a space) after the date, and a `Z` or `±` zone only when it is at most one space after that, so otherwise the rest counts as midnight UTC; it cuts a fraction to the microsecond. An impossible date or time: not decoded |
| values | a NaN, or a value equal to the axis's `_FillValue` or `missing_value` (Julia and Rust; Python's reader has already masked it to NaN): not decoded. A `scale_factor`/`add_offset` is applied to the stored values after that check (Julia; the Python and Rust readers have already unpacked the axis) |
| result | the reference plus each value, floored to whole seconds |

Python accepts more reference spellings than the table lists, through pandas
(`2016/01/01`, `20160101`, a month name, …); Julia and Rust do not decode those,
so on such a file they fall back to the cadence offset where Python does not.
Python also reads a bare one-digit hour (`2016-05-01 6`) as midnight; Julia and
Rust refuse it. A short year followed by another run of four digits
(`1-1-1 00:00:00.0000`, `1-1-1 06:30 -0130`) is not padded by xarray, so pandas
reads it its own way (as 2001, when the values fit); Julia and Rust do not
decode it. None of these differences is in the fixture.

In Julia the record is chosen inside the `netcdf` reader's one open: the
`records` option takes a two-argument resolver `(len, axis) -> indices`, which
is handed the file's raw `time_dim` coordinate with its attributes
([registries.md](registries.md) §2, the `netcdf` reader's `records`).

### 2.6 The run window

Fixture: `windows`, and the `prefetch` entries of `providers`, in
[`../conformance/corpus/scenarios/calendar-cadence.json`](../conformance/corpus/scenarios/calendar-cadence.json).

A provider over a run window `[a, b)` (UTC, half-open) is served by these
records, in order (Python `Provider.window_records`, Julia `window_records`,
Rust `Provider::window_records`, each as record anchors):

1. the record in effect at `a`: `anchor_f(floor_f(a))`, whether or not it is
   timestamped inside the window;
2. every record after it whose anchor is `< b`;
3. with `records_per_sample = 2`, the record after the last of those,
   `anchor_f(ceil_f(b))`, unless it is at or past `end`. The bracket of the
   window's last interval interpolates toward it. When it is at or past
   `end`, the bracket holds the last record instead (§2.5).

A window inside one record's interval is served by that record alone (two
records with a bracket), and has no refresh times. Without a window, `a` is
`start` and `b` is `end`.

Records cover the data from `start` up to `end` (exclusive; open-ended when
there is no `end`), each record until the next one's anchor. So every
part of the window is served unless the provider is built with a window that

| Window | Why |
|---|---|
| starts before `start`, `a < start` | no record is in effect before the first one |
| ends past `end`, `b > end` | the last record stops at `end` |
| is empty, `b ≤ a` | no record serves it |

which every track refuses when the provider is built, before any I/O, with
the code `esm_window_uncovered` at the start of the message (Python
`WindowNotCovered`, Julia `WindowNotCovered`, Rust `Error::WindowNotCovered`).
A window passed to `prefetch` (Python `prefetch(window=...)`, Rust
`prefetch(window)`) is checked the same way, never clamped to the data.
A window before or after the data is never read as an empty run. With no
`end` there is no upper limit to check: a window past the last file that
exists fails when its file is read.

`prefetch` warms the files of these records: every file anchor from
`anchor_F(floor_F(a))` while `< b` and, for item 3, that record's file.

## 3. A provider from an esm data source

Fixture:
[`../conformance/corpus/scenarios/esm-sources.json`](../conformance/corpus/scenarios/esm-sources.json).

Every track builds a provider straight from one `data_sources` entry:

| Track | Call |
|---|---|
| Python | `provider_from_esm(name, entry, cache, *, variables, substitutions=None, url_override=None, records_per_sample=None, window=None)`; `source_from_esm(…)` returns the `DataSource` alone |
| Julia | `provider_from_esm(name, entry, cache; variables, substitutions, url_override, records_per_sample, window)` |
| Rust | `DataSource::from_esm(name, &entry, &EsmSourceOptions { variables, substitutions, url_override, records_per_sample })`, then `Provider::new(source, cache, window)` |

`entry` is the entry's JSON value as a document carries it after EarthSciAST
has loaded it, so its locations are already absolute (esm-spec §8.2.1) and a
mount's `url_substitutions` already applied (§4.7). `window` is the run window
in UTC.

### 3.1 What is read

| Entry field | Reading |
|---|---|
| `metadata.esio_format` | the reader name (`netcdf`, `zarr`, …). This is a convention, not an esm-spec field; it moves to the field the format defines when there is one |
| `source.url_template`, `source.mirrors` | esm templates (§1), expanded at each file anchor with `substitutions`. `{var}` is bound to the variable when exactly one is read and the caller did not bind it |
| `temporal` | absent, or with neither `start`, `frequency` nor `file_period`: a time-invariant provider, whose templates cannot hold a `{date:…}` token. Otherwise `start`, `end` (the exclusive end of the data), `frequency` and `file_period` (§2; either alone stands for both) and `time_variable` (the record dimension, default `time`). A field that is `null` or `""` is absent; a present one must be a string |
| `temporal.start`, `temporal.end` | an ISO 8601 instant: `YYYY-MM-DD`, or a date and time joined by `T` or a space, as `HH:MM`, `HH:MM:SS` or `HH:MM:SS.fff…`, then `Z`, a `±HH:MM` offset (converted to UTC) or nothing (read as UTC) |
| `temporal.records_per_file` | when an integer, it must equal the number of records in the first file (§2.3); `"auto"` is not checked |
| `reader_options` | the reader's decode options, checked against the reader when the provider is built |

`url_override` replaces `url_template` and drops the mirrors (a deployment's
local copy). It is an esm template too, so it cannot carry
`${EARTHSCIDATADIR}` (whose `{…}` would read as a substitution): a caller
expands that itself. `records_per_sample` (1 or 2) is the consumer's choice and is not
read from the entry, since esm-spec §8 has no field for it yet.

### 3.2 What is not read here

`select`, `codes`, `record_filter`, `extent` and a parameter's
`unit_conversion` say what a source's values mean to the model; the document
binding (EarthSciAST's `providers_from_document`) applies them to what this
provider returns. `kind` needs nothing from EarthSciIO: a `points` source is
read by a table reader (`csv`, `parquet`, `ff10`, `shapefile`) like any other.
`determinism`, `reference` and the rest of `metadata` are not read.

### 3.3 Errors

The message contains the code and the source's name, `esm_template_*` codes
included: a template error of the entry is raised as the track's esm source
error (Python `EsmSourceError`, its `__cause__` the `EsmTemplateError`; Julia
`EsmSourceError`; Rust `Error::EsmSource`) carrying the template code.

| Code | When |
|---|---|
| `esm_source_no_format` | no `metadata.esio_format` |
| `esm_source_url` | no `url_template`; `mirrors` that is not an array of strings; a template that is neither a URL nor substitution-led (a relative path EarthSciAST did not resolve); a `${…}` in a template (esm-spec §8.2.1) |
| `esm_source_temporal` | a `start` with neither `frequency` nor `file_period`; a `frequency` or `file_period` with no `start`; an instant that cannot be read; a `time_variable` that is not a string |
| `esm_source_records_per_file` | `records_per_file` disagrees with the cadence |
| `esm_period_invalid` | a period that §2.1 refuses, or one that is not a string |
| `esm_template_*` | §1.3, for the primary template or any mirror, e.g. a substitution nothing bound, or a `{date:…}` in a time-invariant source (`esm_template_no_anchor`) |
