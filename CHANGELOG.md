# Changelog

All three language tracks (Rust `earthsciio` on crates.io, Python `earthsciio`
on PyPI, Julia `EarthSciIO` in General) ship under one version number. The
package version is independent of the `.esm` format version.

## Unreleased

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
