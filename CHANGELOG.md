# Changelog

All three language tracks (Rust `earthsciio` on crates.io, Python `earthsciio`
on PyPI, Julia `EarthSciIO` in General) ship under one version number. The
package version is independent of the `.esm` format version.

## Unreleased

### All tracks

- **feat:** a provider can count time from a time origin, such as an esm
  model's (`spec/esm-sources.md` §4, #37). Built with one (Python and Julia
  `time_origin=` on `Provider` and `provider_from_esm`; Rust
  `DataSource::time_origin(origin)`, so `DataSource::from_esm(…)?.time_origin(origin)`),
  every time it takes and returns is seconds from that origin: `sample_at`,
  `refresh`, `sample_windows_at`, the run window and `prefetch(window)`,
  `refresh_times`, `window_records`, a `PrefetchGroup`'s boundaries and
  anchors, and a two-record bracket's time coordinate (`units` `seconds since
  <origin>`). A caller no longer converts to Unix seconds on every call. It is
  opt-in: an entry does not carry the model's origin, so without one nothing
  changes. The origin must be a whole second, any nonzero fraction refused
  however small (`time_origin_invalid`: Python and Julia `TimeOriginInvalid`,
  Rust `Error::Time`), and a `PrefetchGroup` refuses members on different
  origins (a Julia member built from `times` counts as one without). A time
  returned is the instant's Unix seconds minus the origin's, fraction
  included, in every track: Rust no longer drops the fraction of a
  fractional-second window start (its group boundaries, `Sample::anchor`
  and the `decode_latency` hook's anchor now match Python's and Julia's). Python's provider times may now be
  numbers (seconds from the origin, Unix seconds without one) as well as
  `datetime`s (`TimeLike`); Rust's `sample_at`, `refresh`,
  `refresh_with_select` and `sample_windows_at` take `impl Into<ProviderTime>`
  (an `OffsetDateTime` or an `f64`), and `DataSource::run_window` /
  `Provider::run_window` build a run window from either. Rust `DataSource`
  gains a public `time_origin` field.
- **fix (breaking):** the three tracks now agree on which records serve a run
  window, and refuse the same windows (`spec/esm-sources.md` §2.6). A window
  is served by the record in effect at its start, every record inside it and,
  with `records_per_sample = 2`, the record after it that the bracket of its
  last interval reads. Julia no longer refuses a window that holds no record
  anchor but lies inside one record's interval ("holds no record"); such a
  window has no refresh times. All three refuse, when the provider is built
  and with the code `esm_window_uncovered`, a window that starts before the
  source's `start`, ends past its `end`, or is empty (Python and Julia
  `WindowNotCovered`, Rust `Error::WindowNotCovered`). Before, Python and Rust
  clamped such a window to the data and could run on no records at all, for
  example a model time read as 1970. New `window_records` lists the records
  serving the window, and `prefetch` with a two-record bracket now also warms
  the file of the record after the window.
- **fix:** a `PrefetchGroup` schedules a member whose run window holds no
  record anchor. Its boundaries are now the members' refresh times and
  run-window starts, so the record in effect at a window start gets an epoch
  (`spec/prefetch.md` §3.2). `prefetch(window)` (Python, Rust) checks the window
  it is given as the constructor does, instead of clamping it to the data. In
  Python, a naive window bound is UTC, as everywhere else, instead of raising
  `TypeError` against an aware `start`.

## v0.3.0

Changes since v0.2.0. A minor bump because of the **breaking** changes
below: Rust `SourceTemporal.frequency` and `.file_period` are now
`CalendarPeriod`s, and Julia's csv `header_row` is now 0-based.

Data sources straight from an esm file (`spec/esm-sources.md`): URL templates
with mirrors, calendar cadences, a provider built from a `data_sources` entry,
and records found by the file's own time axis, alike in all three tracks. Also
new `json` and Rust `csv` readers.

### All tracks

- **feat:** a `json` reader (Python and Julia `JSONReader`, Rust `JsonReader`),
  registered under `json`, which `spec/registries.md` already listed as active
  for points sources. A JSON array of records, found at `records_path`, becomes
  a points table: nested objects are flattened into dotted field names, numbers
  are float64, strings are strings, booleans are bool, all on `index`, and
  `numeric_columns` reads a field holding numbers as text. `variables` is a
  projection, so a field that holds arrays can be left undecoded. Errors carry
  `json_*` codes shared by the three tracks. A number outside the float64
  range, a lone UTF-16 surrogate escape, or nesting deeper than 127 levels is
  a parse error anywhere in the document, decoded or not, and a zero is always
  +0.0. The shared fixture
  `conformance/corpus/scenarios/json-reader.json` (52 cases) runs in every
  test suite, and the cross-language corpus case `openaq-stations-json` has
  three-way equality. `spec/registries.json` lists the reader's options.
- **feat:** esm URL templates (`spec/esm-sources.md` §1). `expand_esm_template`
  expands an esm `source.url_template` — `{date:<format>}` with the directives
  `%Y %y %m %d %j %H %M %S %%`, and custom `{name}` substitutions such as
  `{sector}` — identically in Python, Julia and Rust, pinned by the shared
  `esm-url-templates` scenario. `esm_template_names` lists the substitutions
  a template needs. Errors carry an `esm_template_*` code; an unbound
  substitution is an error, never a URL with braces in it.
- **feat:** a provider takes an esm template as its URL (Python `EsmTemplate`,
  Julia `EsmTemplate`, Rust `DataSource::esm_template`), with mirrors that are
  esm templates expanded at the same anchor and tried in order after the
  primary URL. Every track refuses an unbound or malformed name in the
  primary OR a mirror when the template is built, and checks the tokens in
  place, so `{{date:%Y}x}` is accepted (the shared scenario's
  `construction` cases pin both). Each file carries the mirrors resolved at
  its own anchor to the fetch; there is no URL-to-mirrors memo.

- **feat:** calendar cadences (`spec/esm-sources.md` §2). A source's frequency
  and file period are `CalendarPeriod`s (whole months plus whole seconds, read
  from ISO 8601 durations), so `P1M` and `P1Y` anchors fall on calendar months
  and years. Every anchor is computed from the start, the day clamped to the
  month's last day. `SourceTemporal` gains `record_anchor`,
  `next_record_anchor`, `first_record_anchor_from`, `file_anchor`,
  `next_file_anchor` and `record_in_file`, pinned by the shared
  `calendar-cadence` scenario (with small synthetic monthly and yearly files in
  the corpus).

- **feat:** a provider straight from an esm `data_sources` entry
  (`spec/esm-sources.md` §3): Python `provider_from_esm` / `source_from_esm`,
  Julia `provider_from_esm`, Rust `DataSource::from_esm(name, &entry,
  &EsmSourceOptions { variables, substitutions, url_override,
  records_per_sample })`. Each reads the reader name from
  `metadata.esio_format`, the URL template and mirrors as esm templates (binding
  `{var}` when one variable is read), the calendar `temporal` block (checking
  `records_per_file` against the cadence) and `reader_options`. A source it
  cannot read fails with an `esm_source_*` code and the source's name; a
  template error of the entry (an unbound mirror substitution, a `{date:…}` in
  a time-invariant source) is restated the same way with its `esm_template_*`
  code. Pinned by the shared `esm-sources` scenario, which includes reseact's
  GEOS-FP A1 and NEI 2016 entries.

- **fix:** the three tracks read an ISO 8601 instant alike: a `T` or space
  separator, a `Z` or `±HH:MM` offset (converted to UTC) or none (UTC), and a
  fraction of any length. Julia's string `start`/`stop` of a `SourceTemporal`
  accepted only a UTC suffix; Python's `parse_rfc3339` on 3.9 and 3.10 refused
  a fraction that was not 3 or 6 digits.

- **feat:** a time-varying provider finds a record in its file by the file's
  own CF time axis when it can decode one, and by the cadence offset otherwise
  (`spec/esm-sources.md` §2.5). Python already did; Julia and Rust now do the
  same, rule by rule, so a file that lacks some records or whose records are
  off the cadence is read right in every track. `decode_time_axis` (all three
  tracks; Rust also `decode_coord_times`) exposes the CF decode. Pinned by the
  shared `decoded-time-axis` scenario, with small synthetic trimmed, irregular
  and noleap files in the corpus. Python's decode no longer fails on an axis
  with a missing entry; it falls back to the cadence offset.

- **fix:** a two-record sample's successor is the record of the next anchor,
  found the same way as the first record, in every track. It was the next
  record in file order, so in a file with a gap (or an unsorted axis) the
  sample paired a record with a later one and labelled it with the next
  anchor's time. Julia and Rust now also decode a time axis whose reference
  date precedes 1582-10-15 or has a short year (`hours since 1-1-1`), reading
  it as Python's cftime fallback does (a Julian date on the `standard`
  calendar); Julia unpacks a `scale_factor`/`add_offset` time axis before
  decoding it; and the Rust netcdf reader masks an integer variable with two
  different sentinels (`_FillValue` and `missing_value`) to NaN in float64 as
  the other tracks do, instead of keeping only the first. New cases in the
  `decoded-time-axis` scenario: a file with a gap, a packed axis, an axis with
  two fill values, and the cftime reference forms.

### Rust

- **feat:** a `csv` reader (`CsvReader`), registered under `csv`, which
  `spec/registries.md` already listed as active. It decodes as the Python and
  Julia readers do: `numeric_columns` (else a column is numeric when every cell
  parses), `delimiter`, `header_row`, every other column a string field, all on
  `index`, and a `variables` projection. The cross-language case
  `openaq-points-slice` now has three-way equality.
- **breaking:** `SourceTemporal.frequency` and `.file_period` are
  `CalendarPeriod` values instead of `time::Duration`. `SourceTemporal::new`
  still accepts a `Duration`; one that is not a positive whole number of
  seconds (a fraction of a second is no longer dropped) is refused by
  `Provider::new` with `esm_period_invalid`, as Python refuses it. Monthly
  cadences are no longer approximated by a fixed number of seconds. New
  `Error::BadPeriod` (`esm_period_invalid`).

### All tracks (csv)

- **breaking (Julia):** the csv reader's `header_row` is now 0-based (default
  0), as it is in Python and Rust, so one document picks the same row in every
  track. A Julia caller passing `header_row = n` must now pass `n - 1`.
- **fix:** a csv header that repeats a column name is an error instead of one
  column silently replacing the other, and a `numeric_columns` entry the header
  lacks is an error instead of being ignored.
- **fix:** csv text is read as UTF-8 in every track (Python used the locale
  encoding) and a leading byte-order mark is dropped (Python and Julia kept it
  in the first column name).
- **fix:** a `null` csv option is unset in every track. Rust errors now name the
  file line rather than a count of non-blank rows.
- `spec/registries.json` lists `txt` as a csv extension, as the readers do.

### Python

- **feat:** `SourceTemporal(frequency=…, file_period=…)` also takes a
  `CalendarPeriod` or an ISO 8601 string; a `timedelta` works as before.

### Julia

- **feat:** the `netcdf` reader's `records` option takes a two-argument
  resolver `(len, axis) -> indices`, handed the dimension's own raw coordinate
  inside the one open; the temporal `Provider` uses it to find records by the
  file's time axis.
- **feat:** `Provider(cache, url; format, temporal = SourceTemporal(…), window)`
  builds a time-varying provider from a cadence and a run window, as Python
  and Rust do, in Unix seconds (UTC). Each tick reads its own record of its own
  file by the calendar, rather than `mod1(tick, file length)`, which was wrong
  whenever the ticks did not start at a file's first record. `refresh_times`
  returns the anchors in the window, and a time outside the window is read
  from the calendar too, as in Python and Rust.
- **feat:** mirror failover. `fetch_blob(…; mirrors)` tries each mirror after
  the primary URL and caches the blob under the primary's key, and `Provider`
  takes `mirrors`. As in Python and Rust, rung 0 of the `file://` recheck
  serves a present blob whose primary source is missing when mirrors were
  given (`spec/cache-format.md` §4.1).

## v0.2.0

Changes since v0.1.4. A minor bump because the writers now reject
non-increasing time values; see the **Breaking** notes below.

RFC U12 from EarthSciIntegrator (#9): a Rust streaming Zarr v3 writer, and in
every track region writes, a commit protocol and indexed records. The contract
is the new [`spec/writer.md`](spec/writer.md). A single-writer, append-only
store without statics still writes `earthsciio/output-manifest/v1` in the same
spelling each track wrote before; everything else writes the new
`earthsciio/output-manifest/v2` ([schema](spec/schemas/output-manifest.schema.json)).
EarthSciIO stays MPI-free: commit tokens are plain JSON values that the caller
moves.

### All tracks

- **feat:** region writes. A writer is bound at open to a shard-aligned block of
  the non-time dims, so every shard object has one writer and each rank writes
  its block of one logical array without a gather or a lock. A misaligned region
  raises `RegionNotShardAligned` before any I/O.
- **feat:** roles and commit tokens. Writers `flush` into a `CommitToken`; one
  owner's `commit` checks that the tokens tile the extent and agree on records,
  `t` and the schema fingerprint, then writes shapes and the manifest last (the
  commit point). A gap, an overlap or a divergent writer raises `CommitError` and
  commits nothing. Contributors never write `zarr.json` or the manifest.
- **feat:** indexed time mode: records written by index in any order (reverse
  gradients, checkpoint rings), with per-record generations and `invalidate`
  before a committed record is rewritten.
- **feat:** statics (time-less variables), rank-local blobs, `uint8`, a
  read-back API (`read_output_manifest`, `read_record`, `read_static`,
  `read_blob`) that reads committed data bitwise, and `resume` after preemption
  (append mode truncates to `t <= at`).
- **feat(conformance):** `conformance/write_spec_v2.json` (14 cases, generated by
  `gen_write_spec_v2.py`), per-track `write_v2`/`read_v2` drivers, the bitwise
  comparator `crosscheck_write_v2.py`, and `run_write_conformance_v2.sh`, which
  runs every writer against every reader. Rust now reads written stores too.

### Rust

- **feat:** `ZarrStreamWriter` (`create / join / resume / write_record /
  write_record_at / write_static / write_blob / flush / commit / invalidate /
  close`), `StreamSchema`, `TypedArray`/`TypedVec` (f64, f32, i32, i64, u8) and
  the read-back functions. `write_zarr_v3` and `write_all_to_store` are now
  thin wrappers over it. Their `zarr.json` and manifest output is unchanged; the
  order of inner chunks inside a shard object may differ, the decoded arrays do
  not. On wasm32 only `Role::Single` is accepted. **Breaking:** a time
  coordinate in `coords` must now hold exactly `dims[time_dim]` strictly
  increasing, non-NaN values (append-mode `t`, `spec/writer.md` §6.1); this is
  checked before any I/O. Before, any values were written verbatim.
- **feat:** `Error::RegionNotShardAligned` and `Error::Commit`.

### Python

- **feat:** `earthsciio.backends.zarr_stream`; `ZarrWriter` gains `role`,
  `region`, `write_record_at`, `write_static`, `write_blob`, `write_commit`,
  `write_invalidate`, `write_resume`, and `write_flush` returns a `CommitToken`.
  `OutputSchema` gains `statics` and `time_mode` (`IndexedTime`).
  **Breaking:** `write_record` now requires strictly increasing `t`
  (`spec/writer.md` §6.1), as the Julia and Rust writers do.
  `RegionNotShardAligned` and `CommitError` are exported from `earthsciio`.
  Shards are written as whole-shard selections, which zarr-python 3 encodes
  without reading back.

### Julia

- **feat:** `write_open!` takes `role`, `n_writers`, `id` and `region`, and
  `write_record!`'s reserved `region` is live. New: `write_record_at!`,
  `write_static!`, `write_blob!`, `write_commit!`, `write_invalidate!`,
  `write_invalidate_statics!`, `write_resume`, `read_record`, `read_static`,
  `read_blob`, `IndexedTime`, `CommitToken`. `read_output_manifest` also reads v2
  and Python's v1 spelling; `ZarrReader` decodes `uint8`.
- **change:** a single writer no longer commits (bumps `shape[time]` and
  rewrites the manifest) each time a time shard fills; it commits at
  `write_flush!` and `write_close!`, as `spec/writer.md` specifies. Full shard
  objects are still written as soon as they fill.

RFC U11 from EarthSciIntegrator (#8): asynchronous epoch prefetch and per-rank
index-range windows. Additive; the cache format stays `v1` and no cache key
changes. The contract is the new [`spec/prefetch.md`](spec/prefetch.md).

### All tracks

- **feat:** `sample_at(t)` returns what `refresh(t)` would, coordinates
  included, without touching any provider buffer, and is safe to call from
  several threads. `refresh` is now `sample_at` followed by the buffer update.
- **feat:** named-dimension windows. A `select` may name its dimensions,
  `{"dims": {"lat": {"slice": [a, b]}, "lon": {"indices": [...]}}}`, instead of
  listing positional `axes`, so one window covers 2-D, 3-D and staggered fields
  alike. The `netcdf` and `zarr` readers take it in every track; the zarr
  reader still fetches only the chunk objects the window intersects. `DimWindow`
  builds one.
- **feat:** `with_window(w)` binds a window to a new provider at construction,
  keeping the decoded-file reuse path, and `sample_windows_at(t, [w...])`
  decodes the bounding union of K windows once.
- **feat:** `PrefetchGroup` decodes upcoming forcing epochs on background
  workers while the caller computes, in forward, reverse or an explicit (revolve)
  order, bounded by an epoch depth and a byte budget, with cancellation and
  stored-until-`take` errors. Every `take(k)` is bitwise identical to
  `sample_at(boundaries()[k])`.
- **feat(conformance):** decode cases `named-window-mixed-rank`,
  `named-window-zarr` (unselected chunk objects poisoned) and `window-wrap`, and
  the `multi-window`, `prefetch-equivalence` and `prefetch-errors` scenarios,
  which each track's own suite runs against the committed corpus. `verify.py`
  checks scenario blobs and skips poisoned objects.

### Python

- **feat:** `earthsciio.prefetch` (`PrefetchGroup`, `Sample`, `EpochSample`),
  thread executor by default and `executor="process"` for GIL-bound decodes.
  Sample arrays are marked read-only. `Cache`, `Provider` and every
  `EarthSciIOError` now pickle, so a `CacheMiss` crosses a process boundary as
  itself. `bench/prefetch_overlap.py` measures the hidden refresh fraction.
- **fix:** NetCDF decodes take one process-wide lock: netCDF4/HDF5 is not
  assumed thread-safe.

### Julia

- **feat:** `sample_at`, `DimWindow`/`select_form`, `with_window`,
  `sample_windows_at`, and `PrefetchGroup` on `Threads.@spawn` workers
  (`start!`, `take!`, `isready`, `cancel!`, `close`, `stats`). With one thread
  the group decodes inside `take!` and reports `overlap = false`.
- **fix:** NetCDF decodes take one process-wide lock, and the shared HTTP
  downloader handle is locked for fetches from worker threads.
- **change:** a DISCRETE provider *without* a `time_dim` now resolves its URL
  at the tick in effect rather than at the raw query time, so `t` before the
  first tick raises `ArgumentError` (as it already did with a `time_dim`)
  instead of reading `url_for(t)`. `sample_at` must depend on `t` only through
  its anchor.

### Rust

- **feat:** `Selection::Named` (and `Selection::from_json`, the wire parser the
  dumper and tests share), `DimWindow`, `Provider::sample_at(&self)`,
  `with_window`, `sample_windows_at`, a compile-time `Provider: Send + Sync`
  assertion, and `earthsciio::prefetch` (`PrefetchGroup<T>`, `Order`, `Sample`,
  `EpochSample`, `PrefetchStats`) on `std::thread`, no async runtime.
- **change:** a failed `refresh` no longer leaves `coords()` half-updated, and
  the zarr reader resolves every array's metadata before fetching any chunk,
  so a bad second array fails before the first array's chunks are fetched.

## v0.1.4

Changes since v0.1.3. The headline fix is the `file://` cache revalidation
(PR #6, EarthSciML/EarthSciAST#293).

### All tracks

- **fix(cache):** a warm cache entry for a `file://` source is now rechecked
  against the file it was ingested from (length, then sha256) before it is
  served. A file replaced in place is re-ingested instead of served stale
  forever. The recheck answers `current` / `replaced` / `missing` / `unknown`:
  an unreadable path abstains, a genuinely missing file is an error, and with
  mirrors a missing canonical abstains. The hash is memoised per cache object
  against `(length, mtime)`. It is on by default; `EARTHSCI_REVALIDATE_FILE=0`
  opts out. Not a cache-format change: existing caches revalidate correctly.
  (6821a52, b58d664, 0045876)
- **fix(cache):** the recheck's hash memo follows git's racy-timestamp rule: a
  digest taken within 2 s of the source's mtime is never reused, so a
  same-size replacement within one timestamp tick is caught on filesystems with
  whole-second (Lustre, ext3, HFS+) or two-second (FAT) mtimes.
- **fix:** the decode-time `select` vocabulary behaves alike at its edges in
  every track: over-long and negative slices are bounds errors, an empty axis
  is a zero-length axis kept in `dims`, a baked `select` on a reader that honours
  none is an error before the fetch, and CF-time `since` is case-insensitive.
  (608cdae)
- **feat:** a `parquet` format reader in every track (Rust over arrow-rs, Python
  over pyarrow via the new `parquet` extra, Julia over the Parquet2.jl weakdep
  extension), with `variables` pushdown, `float_columns`, `null_int` and
  `null_string`, and a cross-language `parquet` corpus case.
  (b186725, 66a07f4, 1b8d2d2, cc2184c, 03a8f14, and follow-ups)
- **feat:** the netcdf reader honours a `select` at decode time and a `records`
  pushdown, and the `era5-window-slice` corpus case covers it.
  (37a370d, a9e691a, 7b82be8, 453dce0, a8406f7, 0ef6920, 92498d4)
- **feat(spec):** per-track reader-option availability is machine-readable.
  (ebdc55b)

### Rust

- **feat(transport):** `s3://` reads are signed for buckets named in
  `EARTHSCI_S3_SIGNED_BUCKETS` (or `S3Transport::signing_buckets`); every other
  bucket stays anonymous. Needs the `object-store` feature. (a7f937a)
- **feat(s3):** per-bucket options via `EARTHSCI_S3_BUCKET_OPTIONS` (for example
  requester-pays), and a per-bucket region. A malformed spec is an error.
  (50bc088)
- **fix:** a netcdf `char` / `NC_STRING` variable decodes to a string field
  instead of being skipped. (354655f)
- **fix:** a requested `variables` name absent from a netcdf blob is an error
  listing what is present, as in the other readers and tracks. (b73ef37)
- **fix:** parquet `float_columns` no longer promotes a binary column, and
  decodes a uint64 past `i64::MAX`. (a30c9e4, 130d696)
- New public API: `ParquetReader`, `Records`, `SourceRevalidator`,
  `SourceState`, `file_source_state`, `file_source_is_current`,
  `REVALIDATE_FILE_ENV`, `CacheBuilder::revalidate_file_sources`, the
  `s3_config` module, `file_url_to_path`. Additive only.
- `rust-version` is now 1.85 (arrow-rs's minimum).

### Python

- **feat:** `ParquetReader` and the `parquet` extra (`pyarrow>=14`). (66a07f4)
- New public API: `SourceRevalidator`, `file_source_state`,
  `file_source_is_current`, `resolve_revalidate_file`, `REVALIDATE_FILE_ENV`,
  `dim_length`. Additive only.
- **fix(tests):** the netcdf text tests skip without the `netcdf` extra. (42eeb4e)

### Julia

- **feat:** a netcdf `char` array is a string field, not a `Char` matrix.
  (18ad76b)
- **feat:** the netcdf reader takes `variables`, so projection pushes down, and
  the Provider pushes its record bracket into the netcdf decode. (11fb37a, 0ef6920)
- **fix:** an empty `variables` list reads every column, as in the other tracks
  (it read none). (fa19379)
- **fix:** a baked `records` beside a `time_dim` no longer reads the same record
  forever. (fb5d15b)
- **fix(http):** a per-request timeout no longer caps blob size, progress is
  per attempt with a whole-call bound, and a per-object store read gets its own
  shorter fetch budget. (5d19092, 8a4bf2b, b986d67)
- **fix(parquet):** narrow decimals decode, including Parquet2.jl's unsigned
  read of a narrow FLBA decimal. (5f332ee, 543ff68)

### Conformance and spec

- Conformance runners in all tracks read the `parquet` case; the Julia weakdeps
  and EarthSciIO resolve as one environment. (2fe8af6, d07263e, 767521e, 7490a5a,
  0035151)
- Spec records the parquet decode contract, backend limits, fill-value
  reporting, and that a wrong number is never a permitted divergence.
