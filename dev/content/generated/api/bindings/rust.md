---
title: "Rust API index"
description: "Every stable name Rust exports, and its extension seams."
type: "api"
---


**Package:** `earthsciio` 0.4.0

Documentation extracted with rustdoc JSON (rustdoc 1.101.0-nightly (21b707e3f 2026-09-30)), format_version 61.

Extension-tier names are on the [Rust extension page](/generated/api/extension/rust).

## Stable names

| Rust | Canonical | Kind | Summary |
|---|---|---|---|
| `Cache` | [`cache`](/generated/api/ref/cache) | type | The cache fetcher. |
| `cache_key` | [`cache_key`](/generated/api/ref/cache_key) | function | Compute the cache key for a resolved URL: `sha256(utf8(url))`, lowercase hex. |
| `CachedBlob` | [`cache_entry`](/generated/api/ref/cache_entry) | type | A resolved cache entry: the blob path plus its manifest. |
| `CalendarPeriod` | [`calendar_period`](/generated/api/ref/calendar_period) | type | A cadence step of whole calendar `months` plus whole `seconds` (`spec/esm-sources.md` §2.1): `P1M` is one month, `P1Y` twelve, `PT3H` 10800 seconds, `P1D` 86400 seconds (a UTC… |
| `CommitReceipt` | [`commit_receipt`](/generated/api/ref/commit_receipt) | type | What a successful `commit` did. |
| `CommitToken` | [`commit_token`](/generated/api/ref/commit_token) | type | The value a writer hands the owner after `flush` (`spec/writer.md` §5.2). |
| `CsvReader` | [`csv_reader`](/generated/api/ref/csv_reader) | type | The `csv` format reader. |
| `data_dir` | [`data_dir`](/generated/api/ref/data_dir) | function | Resolve the cache root `$EARTHSCIDATADIR`. |
| `DataSource` | [`data_source`](/generated/api/ref/data_source) | type | The minimal parsed `DataSource` the Provider needs: where the bytes are, how to decode them, which variables to read, and the temporal cadence (absent ⇒ CONST). |
| `DataSource::from_esm` | [`source_from_esm`](/generated/api/ref/source_from_esm) | function |  |
| `decode_coord_times` | [`decode_coord_times`](/generated/api/ref/decode_coord_times) | function | `decode_time_axis` of a decoded coordinate: its values, `units`, `calendar` and the fill value the reader kept. |
| `decode_time_axis` | [`decode_time_axis`](/generated/api/ref/decode_time_axis) | function | The Unix seconds (UTC, floored to the second) of a raw CF time axis, or `None` when it is not decoded (`spec/esm-sources.md` §2.5). |
| `DimWindow` | [`dim_window`](/generated/api/ref/dim_window) | type | A named-dimension window, built one dimension at a time — the Rust spelling of `spec/prefetch.md` §2.2's `DimWindow` (the run window `(start, end)` already owns the name `Window`). |
| `EpochSample` | [`epoch_sample`](/generated/api/ref/epoch_sample) | type | What `PrefetchGroup::take` returns: one `Sample` per member key. |
| `Error` | [`error`](/generated/api/ref/error) | error | Errors raised by the cache, transports, and stores. |
| `esm_template_names` | [`esm_template_names`](/generated/api/ref/esm_template_names) | function | The substitution names `template` needs, in order of first appearance, without `date`. |
| `EsmSourceOptions` | [`esm_source_options`](/generated/api/ref/esm_source_options) | type | What the caller adds to an esm entry when building a loader from it (`DataSource::from_esm`). |
| `EsmTemplateError` | [`esm_template_error`](/generated/api/ref/esm_template_error) | error | An esm URL template could not be expanded. |
| `expand_esm_template` | [`expand_esm_template`](/generated/api/ref/expand_esm_template) | function | Expand an esm URL template (`spec/esm-sources.md` §1) at `anchor` (a UTC instant; an offset is converted to UTC) with `substitutions`. |
| `Ff10Reader` | [`ff10_reader`](/generated/api/ref/ff10_reader) | type | The active `ff10` reader. |
| `file_source_is_current` | [`file_source_is_current`](/generated/api/ref/file_source_is_current) | function | Rung 0 as a yes/no: is the source **provably** the file that was ingested? |
| `file_source_state` | [`file_source_state`](/generated/api/ref/file_source_state) | function | Rung 0 without a memo: is the `file://` source behind a cached entry still the file that was ingested into it? |
| `GeoTiffReader` | [`geotiff_reader`](/generated/api/ref/geotiff_reader) | type | The active `geotiff` reader: pure-Rust raster decode + tiepoint georef. |
| `is_offline` | [`is_offline`](/generated/api/ref/is_offline) | function | Resolve the effective offline flag: the explicit argument wins; otherwise fall back to `EARTHSCI_OFFLINE`. |
| `JsonReader` | [`json_reader`](/generated/api/ref/json_reader) | type | The `json` format reader. |
| `Manifest` | [`manifest`](/generated/api/ref/manifest) | type | Validation + provenance record stored alongside every cached blob. |
| `NativeDataset` | [`native_dataset`](/generated/api/ref/native_dataset) | type | A decoded native dataset: data variables keyed by on-disk name, plus the native coordinates of the loader's grid. |
| `NativeField` | [`native_field`](/generated/api/ref/native_field) | type | One native-grid array, keyed by its on-disk `file_variable` name and validated against `spec/schemas/native-field.schema.json`. |
| `NetcdfReader` | [`netcdf_reader`](/generated/api/ref/netcdf_reader) | type | The active `netcdf` reader: pure-Rust NetCDF-3 + NetCDF-4/HDF5 decode. |
| `Order` | [`prefetch_order`](/generated/api/ref/prefetch_order) | type | The order in which epochs will be taken (`spec/prefetch.md` §3.3). |
| `ParquetReader` | [`parquet_reader`](/generated/api/ref/parquet_reader) | type | The active `parquet` reader: an Apache Parquet file as a flat table on `index`. |
| `PrefetchGroup` | [`prefetch_group`](/generated/api/ref/prefetch_group) | type | Background decode of upcoming epochs across several providers, bitwise identical to synchronous `Provider::refresh`. |
| `Provider` | [`provider`](/generated/api/ref/provider) | type | A loader-bound provider of native-grid arrays, refreshed at the loader's cadence. |
| `ProviderTime` | [`provider_time`](/generated/api/ref/provider_time) | type | A time a `Provider` takes (`spec/esm-sources.md` §4): an instant, or a number of seconds from the loader's `time_origin` (the Unix epoch when it has none). |
| `read_blob` | [`read_blob`](/generated/api/ref/read_blob) | function | Read a committed blob. |
| `read_output_manifest` | [`read_output_manifest`](/generated/api/ref/read_output_manifest) | function | Read and parse `output_manifest.json` (v1 or v2). |
| `read_record` | [`read_record`](/generated/api/ref/read_record) | function | Read the `region` of committed record `index` of variable `var` (shape: the variable's dims minus time, region-sized). |
| `read_static` | [`read_static`](/generated/api/ref/read_static) | function | Read the `region` of committed static `name`. |
| `Region` | [`region`](/generated/api/ref/region) | type | A block of the non-time dims, `dim → start..stop` (0-based, half-open). |
| `Role` | [`writer_role`](/generated/api/ref/writer_role) | type | A writer's role in a store (`spec/writer.md` §5). |
| `Sample` | [`prefetch_sample`](/generated/api/ref/prefetch_sample) | type | One member's part of an epoch (`spec/prefetch.md` §3.4). |
| `schema_fingerprint` | [`schema_fingerprint`](/generated/api/ref/schema_fingerprint) | function | The schema fingerprint (`spec/writer.md` §3.1): `"sha256:" + hex` over the canonical JSON of the schema's structural fields. |
| `sha256_file` | [`sha256_file`](/generated/api/ref/sha256_file) | function | Stream a file through sha256 without loading it fully into memory. |
| `ShapefileReader` | [`shapefile_reader`](/generated/api/ref/shapefile_reader) | type | The `shapefile` format reader. |
| `SourceTemporal` | [`source_temporal`](/generated/api/ref/source_temporal) | type | The temporal nature of a loader that refreshes at a cadence (plan §4.2). |
| `StreamSchema` | [`stream_schema`](/generated/api/ref/stream_schema) | type | The stream schema: the v1 `OutputSchema` without data, plus a time mode, statics and per-variable dtypes. |
| `Temporal` | [`temporal`](/generated/api/ref/temporal) | type | A loader's temporal nature, supplying the TTL rung of the ladder. |
| `TimeMode` | [`indexed_time`](/generated/api/ref/indexed_time) | type | How the record axis is addressed (`spec/writer.md` §6). |
| `Window` | [`window`](/generated/api/ref/window) | type | A run window `(start, end)` — half-open `[start, end)`. |
| `ZarrReader` | [`zarr_reader`](/generated/api/ref/zarr_reader) | type | The store-backed `zarr` reader (Zarr v3 + v2 chunked arrays, via `zarrs`). |
| `ZarrStreamWriter` | [`stream_writer`](/generated/api/ref/stream_writer) | type | The streaming writer handle (`spec/writer.md`). |
