---
title: "ff10_reader"
description: "Stable type · Native arrays and readers"
type: "api"
api_tabs: true
---


**Tier:** stable · **Kind:** type · **Topic:** [Native arrays and readers](/generated/api/topics/native-arrays)

| Python | Julia | Rust |
|---|---|---|
| `FF10Reader` | `FF10Reader` | `Ff10Reader` |


<div class="api-tabs">

<div class="api-tab" data-lang="python" data-label="Python">
<p class="api-tab-label">Python</p>

```python
class FF10Reader
FF10Reader.formats() -> list[str]
FF10Reader.extensions() -> list[str]
FF10Reader.open(blob_path: Any) -> Any
FF10Reader.read_native(handle: Any, variables: Sequence[str] | None = None, select: Any | None = None, *, member: str | None = None, members: Sequence[str] | None = None, member_glob: str | None = None, skip_header_row: bool = False, kind: str = 'point', numeric_columns: Sequence[str] | None = None, delimiter: str = ',', comment: str = '#', **_: Any) -> NativeDataset
```

The active `ff10` reader — the RAW long-format FF10 **point** table.

Unlike [`CSVReader`](/generated/api/ref/csv_reader) (which only skips empty lines and does not apply a
fixed positional schema), this reader (a) skips the leading `#` comment
header block (`#FORMAT=…`, `#COUNTRY`, …), (b) applies the fixed 77-column
`FF10_POINT_COLUMNS` schema positionally — FF10 data rows carry no clean
header row, so the names come from the schema constant exactly as Emissions.jl
supplies them — and (c) does RFC-4180 quote handling (via `csv`) so a
free-text `FACILITY_NAME` may embed the delimiter.

Each of the 77 columns becomes one [`NativeField`](/generated/api/ref/native_field) on a single `index`
dim (one index per data row); there are no coordinates (`LONGITUDE` /
`LATITUDE` are ordinary variables — a points table has no gridded axis). The
42 numeric columns parse to `float64` (blank -> `NaN`); the other 35 stay
`str` (blank -> `""`).

READER-ONLY (Risk R3): no pollutant pivot (POLID stays a data column, rows are
not reshaped), no unit conversion (STKHGT/STKDIAM stay feet, STKTEMP °F,
STKFLOW ft³/s, STKVEL ft/s, ANN_VALUE tons/yr), no FIPS/SCC normalization, no
EGU/pollutant filter — those move downstream into the `.esm`.

`reader_kwargs`: `member="path/in/zip"` extracts a named member from a
`.zip` blob (the whole zip stays the content-addressed cached blob; the
member is reader config so it never enters the cache key). `members` (an
explicit list of member names) and/or `member_glob` (an fnmatch-style
pattern — `*`, `?`, `[...]`, case-sensitive, matched against the full
member path, e.g. `*egu*`) select MULTIPLE members: the selection is the
union of the explicit list and the glob matches, deduplicated, and the
members are read and their rows concatenated in ascending lexicographic
(byte) order of member name. A `members` name absent from the archive, and
a `member_glob` matching zero members, are errors; directory placeholder
entries (names ending in `/`) are never selected; `member` (singular)
is mutually exclusive with `members`/`member_glob`. Like `member`,
none of these enter the cache key. `skip_header_row=True` handles the EPA
2016fd-style column-header line: after comment (`#`) and blank lines are
dropped, the first remaining line of each selected input (each selected zip
member, or the bare file) must be a header row — its first
delimiter-separated field, compared case-insensitively, must be
`country_cd` — and exactly that one line is skipped per member; if the
first field is anything else the reader errors (the option asserts a header
row and never silently drops a data row). `kind="point"` selects the
schema (only point ships). `numeric_columns`, `delimiter`, `comment`
override the defaults.

**Members**

- `read_native` — Decode an FF10 point blob into a `points` [`NativeDataset`](/generated/api/ref/native_dataset).

[Source](https://github.com/EarthSciML/EarthSciIO/blob/a49538bbd6ec318dde369d3496f6e8880586a73f/earthsciio/readers.py#L1038)

</div>

<div class="api-tab" data-lang="julia" data-label="Julia">
<p class="api-tab-label">Julia</p>

```julia
struct FF10Reader <: Reader
FF10Reader()
```

```julia
FF10Reader()
```

The `ff10` format reader — the RAW long-format FF10 **point** table (SMOKE /
Emissions.jl `FF10_POINT`) as a `points` [`NativeDataset`] in **native units**.

Unlike [`CSVReader`] (which only skips empty lines and splits naively), this
reader (a) skips the leading `#` comment header block (`#FORMAT=…`, `#COUNTRY`,
…), (b) applies the fixed 77-column [`FF10_POINT_COLUMNS`] schema — FF10 data
rows carry no clean header row, so the names come from the schema constant
exactly as Emissions.jl supplies them — and (c) does RFC-4180 quote handling so a
free-text `FACILITY_NAME` may embed the delimiter (`"Autauga Plant, Unit 1"`).

Each of the 77 columns becomes one [`NativeField`] on a single `index` dim (one
index per data row); there are no coordinates (`LONGITUDE`/`LATITUDE` are ordinary
variables — a points table has no gridded axis). The 42 numeric columns parse to
`Float64` (blank → `NaN`); the other 35 (IDs/codes/free-text) stay `String`
(blank → `""`).

READER-ONLY (Risk R3): NO pollutant pivot (POLID stays a data column, rows are
not reshaped), NO unit conversion (`STKHGT`/`STKDIAM` stay feet, `STKTEMP` °F,
`STKFLOW` ft³/s, `STKVEL` ft/s, `ANN_VALUE` tons/yr), NO FIPS/SCC normalization,
NO EGU/pollutant filter — those transforms move DOWNSTREAM into the `.esm`.

`reader_kwargs`: `member="path/in/zip"` extracts a named member from a `.zip`
blob (the whole zip stays the cached content-addressed blob; the member is reader
config so it never enters the cache key). `members=[…]` (an explicit list of
member names) and/or `member_glob="*egu*"` (fnmatch-style — `*`, `?`, `[...]`,
case-sensitive, matched against the full member path) select MULTIPLE members:
the selection is the union of the explicit list and the glob matches,
deduplicated, and the members are read and their rows concatenated in ascending
lexicographic (byte) order of member name. An explicit name absent from the
archive, and a glob matching zero members, are errors; directory placeholder
entries (names ending in `/`) are never selected; `member` (singular) is
mutually exclusive with `members`/`member_glob`. None of these enter the cache
key. `skip_header_row=true` handles the EPA 2016fd-style column-header line:
after comment (`#`) and blank lines are dropped, the first remaining line of
each selected input (each selected zip member, or the bare file) must be a
header row — its first delimiter-separated field, compared case-insensitively,
must be `country_cd` — and exactly that one line is skipped per member; if the
first field is anything else the reader errors (the option asserts a header row
and never silently drops a data row). `kind="point"` selects the schema (only
point ships). `numeric_columns`, `delimiter`, `comment` override the defaults;
`variables=[…]` restricts the returned columns (default = all 77).

[Source](https://github.com/EarthSciML/EarthSciIO/blob/a49538bbd6ec318dde369d3496f6e8880586a73f/julia/src/readers.jl#L933)

</div>

<div class="api-tab" data-lang="rust" data-label="Rust">
<p class="api-tab-label">Rust</p>

```rust
pub struct Ff10Reader {
    /* private fields */
}

impl Ff10Reader { pub fn new() -> Self }

impl Ff10Reader { pub fn member(self, member: impl Into<String>) -> Self }

impl Ff10Reader { pub fn members<I, S>(self, members: I) -> Self where I: IntoIterator<Item = S>, S: Into<String> }

impl Ff10Reader { pub fn member_glob(self, glob: impl Into<String>) -> Self }

impl Ff10Reader { pub fn skip_header_row(self, yes: bool) -> Self }

impl Ff10Reader { pub fn kind_point(self) -> Self }
```

The active `ff10` reader. `member`/`members`/`member_glob`/
`skip_header_row`/`kind`/`numeric` are carried on the instance (see the
module docs on the reader-kwarg asymmetry).

[Source](https://github.com/EarthSciML/EarthSciIO/blob/a49538bbd6ec318dde369d3496f6e8880586a73f/rust/src/format/ff10.rs#L195) · [Rust docs](https://docs.rs/earthsciio/latest/earthsciio/struct.Ff10Reader.html)

</div>

</div>
