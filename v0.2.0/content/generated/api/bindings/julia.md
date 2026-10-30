---
title: "Julia API index"
description: "Every stable name Julia exports, and its extension seams."
type: "api"
---


**Package:** `EarthSciIO` 0.2.0

Documentation extracted with Base.Docs (Julia 1.13.3).

Extension-tier names are on the [Julia extension page](/generated/api/extension/julia).

## Stable names

| Julia | Canonical | Kind | Summary |
|---|---|---|---|
| `boundaries` | [`boundaries`](/generated/api/ref/boundaries) | function | The sorted, de-duplicated union `B` of the members' `refresh_times`, in their own cadence unit. |
| `Cache` | [`cache`](/generated/api/ref/cache) | type | The content-addressed cache. |
| `cache_key` | [`cache_key`](/generated/api/ref/cache_key) | function | `lowercase_hex(sha256(utf8(resolved_url)))`. |
| `CacheEntry` | [`cache_entry`](/generated/api/ref/cache_entry) | type | The outcome of [`fetch_blob`]: the resolved blob plus how it was obtained. |
| `CacheMiss` | [`cache_miss`](/generated/api/ref/cache_miss) | error | Raised in offline mode when the blob for a resolved URL is absent. |
| `cancel!` | [`cancel`](/generated/api/ref/cancel) | function | Drop all pending work at once (spec/prefetch.md §3.8). |
| `CommitError` | [`commit_error`](/generated/api/ref/commit_error) | type | The tokens handed to a commit do not describe a consistent step (spec/writer.md §5.3): a missing or duplicate writer, a divergent fingerprint, a gap or overlap between regions,… |
| `CommitToken` | [`commit_token`](/generated/api/ref/commit_token) | type | What one writer wrote since its previous token (spec/writer.md §5.2), returned by [`write_flush!`] and handed (by the caller — EarthSciIO is MPI-free) to the owner's… |
| `const_provider` | [`const_provider`](/generated/api/ref/const_provider) | function | Thin constructor for a time-invariant ([`CONST`]) provider. |
| `CSVReader` | [`csv_reader`](/generated/api/ref/csv_reader) | type | The `csv` format reader — a second reader proving a non-NetCDF format plugs into `FORMAT_REGISTRY` unchanged (spec/conformance.md). |
| `datadir` | [`data_dir`](/generated/api/ref/data_dir) | function | Resolve `$EARTHSCIDATADIR` (spec/cache-format.md §5). |
| `DimWindow` | [`dim_window`](/generated/api/ref/dim_window) | type | A named-dimension window: the Julia constructor for the wire form `select = {"dims": {...}}` (spec/prefetch.md §2.2). |
| `discrete_provider` | [`discrete_provider`](/generated/api/ref/discrete_provider) | function | Thin constructor for a time-varying ([`DISCRETE`]) provider over `times`. |
| `EpochSample` | [`epoch_sample`](/generated/api/ref/epoch_sample) | type | What `take!`) returns for one epoch: a [`Sample`] per member, indexed by member key (`ep["U"].data`, `ep["U"].anchor`, `ep["U"].changed`). |
| `FF10Reader` | [`ff10_reader`](/generated/api/ref/ff10_reader) | type | The `ff10` format reader — the RAW long-format FF10 **point** table (SMOKE / Emissions.jl `FF10_POINT`) as a `points` [`NativeDataset`] in **native units**. |
| `file_source_is_current` | [`file_source_is_current`](/generated/api/ref/file_source_is_current) | function | Rung 0 as a yes/no: is the source PROVABLY the file that was ingested? |
| `file_source_state` | [`file_source_state`](/generated/api/ref/file_source_state) | function | Rung 0 without a memo (spec/cache-format.md §4.1): is the `file://` source behind a cached entry still the file that was ingested into it? |
| `GeoTIFFReader` | [`geotiff_reader`](/generated/api/ref/geotiff_reader) | type | The `geotiff` format reader — raster bands on a native lon/lat (geographic) or x/y (projected) grid. |
| `IndexedTime` | [`indexed_time`](/generated/api/ref/indexed_time) | type | The `indexed` time mode (spec/writer.md §6.2): the time axis has a fixed length `n_records = length(times)` from [`write_open!`], the owner writes `times` as the time coordinate… |
| `IntegrityError` | [`integrity_error`](/generated/api/ref/integrity_error) | error | Raised when a blob's bytes do not match its manifest `sha256_content`. |
| `is_offline` | [`is_offline`](/generated/api/ref/is_offline) | function | True if this cache is in offline (cache-only) mode. |
| `Manifest` | [`manifest`](/generated/api/ref/manifest) | type | Validation + provenance record for one cached blob. |
| `materialize` | [`materialize`](/generated/api/ref/materialize) | function | Return the native arrays for the source at time `t`. |
| `NativeDataset` | [`native_dataset`](/generated/api/ref/native_dataset) | type | The native arrays a reader returns from one blob: `variables` (data fields keyed by `file_variable`) and `coords` (the dimension-coordinate fields — e.g. the grid's… |
| `NativeField` | [`native_field`](/generated/api/ref/native_field) | type | One native-grid array exactly as a reader decodes it: `data` is a Julia array whose axes correspond, in order, to `dims` (the on-disk dimension names, file order — e.g.… |
| `NetCDFReader` | [`netcdf_reader`](/generated/api/ref/netcdf_reader) | type | The `netcdf` format reader, backed by NCDatasets. |
| `OutputSchema` | [`stream_schema`](/generated/api/ref/stream_schema) | type | The input to [`write_open!`] — a plain struct in this repo (the EarthSciAST `OutputSchema` maps onto it at the binding layer; no EarthSciAST dependency here). |
| `ParquetReader` | [`parquet_reader`](/generated/api/ref/parquet_reader) | type | The `parquet` format reader — an Apache Parquet file as a **flat table**. |
| `prefetch` | [`prefetch`](/generated/api/ref/prefetch) | function | Warm the cache for every URL the provider will need — the single URL for a [`CONST`] provider, the unique per-tick URLs for a [`DISCRETE`] one — WITHOUT decoding. |
| `PrefetchGroup` | [`prefetch_group`](/generated/api/ref/prefetch_group) | type | Decode upcoming forcing epochs of several DISCRETE providers on background workers, in a declared order, under a memory budget — bitwise identical to calling [`sample_at`] /… |
| `Provider` | [`provider`](/generated/api/ref/provider) | type | A data provider over the shared cache. |
| `read_blob` | [`read_blob`](/generated/api/ref/read_blob) | function | Read the committed blob `key` of `writer` under `scope` (`:static` or `(:record, i)`). |
| `read_output_manifest` | [`read_output_manifest`](/generated/api/ref/read_output_manifest) | function | Read a store's output manifest. |
| `read_record` | [`read_record`](/generated/api/ref/read_record) | function | Read committed record `index` of streaming variable `var` straight from the store (a directory or `file://` URL; never through the cache). |
| `read_static` | [`read_static`](/generated/api/ref/read_static) | function | Read the committed static `name` (dims order, region-sized, stored dtype). |
| `refresh` | [`refresh`](/generated/api/ref/refresh) | function | Re-materialize at the cadence tick `t`. |
| `refresh_times` | [`refresh_times`](/generated/api/ref/refresh_times) | function | The discrete time points at which the data changes and the solver must [`refresh`]. |
| `RegionNotShardAligned` | [`region_not_shard_aligned`](/generated/api/ref/region_not_shard_aligned) | type | A writer's region (spec/writer.md §4) does not start on a shard boundary, does not stop on one (or at the array end), or is out of bounds. |
| `Sample` | [`prefetch_sample`](/generated/api/ref/prefetch_sample) | type | One member's sample for one epoch, as `take!`) returns it inside an [`EpochSample`]: |
| `sample_at` | [`sample_at`](/generated/api/ref/sample_at) | function | The dataset `refresh(p, t)` returns — data variables AND coordinates, with the same snap-down to the cadence anchor, the same file-local record, the same `records_per_sample = 2`… |
| `sample_windows_at` | [`sample_windows_at`](/generated/api/ref/sample_windows_at) | function | K windows at once: dataset `i` is identical — bitwise, coordinates included — to `sample_at(with_window(p, windows[i]), t)`, but the source is fetched and decoded ONCE, over the… |
| `schema_fingerprint` | [`schema_fingerprint`](/generated/api/ref/schema_fingerprint) | function | The stream-schema fingerprint of spec/writer.md §3.1: the sha256 of the canonical JSON of dims (time length `0` in append mode, `n_records` in indexed mode), `time_dim`,… |
| `select_form` | [`select_form`](/generated/api/ref/select_form) | function | The wire form of a window, `Dict("dims" => Dict(dim => selector, ...))` — what a reader's `select` option and `reader_kwargs[:select]` take (spec/prefetch.md §2.2). |
| `ShapefileReader` | [`shapefile_reader`](/generated/api/ref/shapefile_reader) | type | The `shapefile` format reader — an ESRI shapefile as a feature table. |
| `start!` | [`start`](/generated/api/ref/start) | function | Declare the order in which epochs will be taken and begin decoding ahead of it (spec/prefetch.md §3.3): `:forward` (`1, …, n`), `:reverse` (`n, …, 1` — the adjoint's backward… |
| `take!` | [`take`](/generated/api/ref/take) | function | Removes and returns a value from a `Channel` in order. |
| `with_window` | [`with_window`](/generated/api/ref/with_window) | function | A NEW provider that shares `p`'s cache, reader, URL resolver and cadence and has the window `w` (a [`DimWindow`] or its wire `Dict`) baked in as `reader_kwargs[:select]`. |
| `write_blob!` | [`write_blob`](/generated/api/ref/write_blob) | function | Store a small rank-local payload: `scope` is `:static` or `(:record, i)`. |
| `write_close!` | [`write_close`](/generated/api/ref/write_close) | function | Flush any pending data and finalize: a `:single` writer commits and rewrites the manifest; the owner commits its own token plus the other writers' `tokens`; a contributor only… |
| `write_commit!` | [`write_commit`](/generated/api/ref/write_commit) | function | Owner only: verify one token per writer (spec/writer.md §5.3) and commit them; raises [`CommitError`] (writing nothing) when they do not tile the store. |
| `write_flush!` | [`write_flush`](/generated/api/ref/write_flush) | function | Write every dirty shard of this writer's region and return the token naming what was written since the last token. |
| `write_invalidate!` | [`write_invalidate`](/generated/api/ref/write_invalidate) | function | Owner/single, indexed mode: mark committed records uncommitted so they may be rewritten (bumps `commit_seq`). |
| `write_open!` | [`write_open`](/generated/api/ref/write_open) | function | Open an output store at `base_url` and return an opaque write handle threaded through the other `write_*` calls. |
| `write_record!` | [`write_record`](/generated/api/ref/write_record) | function | Append ONE time record (append mode): `t` is the time coordinate value, `arrays` maps each variable this writer supplies to its gridded `Array` (the variable's non-time dims, in… |
| `write_record_at!` | [`write_record_at`](/generated/api/ref/write_record_at) | function | Write record `index` (0-based) in indexed mode, in any order; `t` must equal the schema's `times[index]` bitwise. |
| `write_resume` | [`write_resume`](/generated/api/ref/write_resume) | function | Reopen a store after preemption (spec/writer.md §7); in append mode committed records with `t > at` are discarded. |
| `write_static!` | [`write_static`](/generated/api/ref/write_static) | function | Write the time-less variable `name` (region-shaped when it is partitioned, full shape when replicated — owner/single only). |
| `ZarrReader` | [`zarr_reader`](/generated/api/ref/zarr_reader) | type | The `zarr` store-backed reader (Zarr v2 and sharded Zarr v3 chunked arrays). |
| `ZarrWriter` | [`stream_writer`](/generated/api/ref/stream_writer) | type | The `zarr` streaming writer (sharded Zarr v3). |
