---
title: "Python API index"
description: "Every stable name Python exports, and its extension seams."
type: "api"
---


**Package:** `earthsciio` 0.2.0

Documentation extracted with griffe 2.3.2.

Extension-tier names are on the [Python extension page](/generated/api/extension/python).

## Stable names

| Python | Canonical | Kind | Summary |
|---|---|---|---|
| `Cache` | [`cache`](/generated/api/ref/cache) | type | A content-addressed fetch cache over a pluggable `Store`. |
| `cache_key` | [`cache_key`](/generated/api/ref/cache_key) | function | Return the lowercase-hex sha256 of the resolved URL. |
| `CacheEntry` | [`cache_entry`](/generated/api/ref/cache_entry) | type | The result of a fetch: where the blob is + how it got there. |
| `CacheMiss` | [`cache_miss`](/generated/api/ref/cache_miss) | error | Raised in **offline mode** when the blob for a resolved URL is absent. |
| `CommitError` | [`commit_error`](/generated/api/ref/commit_error) | type | An owner's `commit` refused the delivered tokens (`spec/writer.md` §5.3): a missing or duplicate writer, a divergent fingerprint, a gap or an overlap between regions, divergent… |
| `CSVReader` | [`csv_reader`](/generated/api/ref/csv_reader) | type | The active `csv` reader — a non-NetCDF format behind the same registry. |
| `DataSource` | [`data_source`](/generated/api/ref/data_source) | type | The I/O-relevant projection of an ESS `DataSource` the Provider needs. |
| `DimWindow` | [`dim_window`](/generated/api/ref/dim_window) | type | A window that names its dimensions (`spec/prefetch.md` §2.2). |
| `EarthSciIOError` | [`error`](/generated/api/ref/error) | error | Base class for every EarthSciIO error. |
| `EpochSample` | [`epoch_sample`](/generated/api/ref/epoch_sample) | type | Every member's `Sample` for one epoch: `ep["U"].data`. |
| `FF10Reader` | [`ff10_reader`](/generated/api/ref/ff10_reader) | type | The active `ff10` reader — the RAW long-format FF10 **point** table. |
| `file_source_is_current` | [`file_source_is_current`](/generated/api/ref/file_source_is_current) | function | Rung 0 as a yes/no: is the source **provably** the file that was ingested? |
| `file_source_state` | [`file_source_state`](/generated/api/ref/file_source_state) | function | Rung 0 without a memo: what is the `file://` source behind this entry? |
| `GeoTIFFReader` | [`geotiff_reader`](/generated/api/ref/geotiff_reader) | type | The active `geotiff` reader — raster bands on a native lon/lat grid. |
| `IntegrityError` | [`integrity_error`](/generated/api/ref/integrity_error) | error | A cached or freshly downloaded blob failed its checksum / size check. |
| `Manifest` | [`manifest`](/generated/api/ref/manifest) | type | In-memory view of `meta/<key>.json` (see the schema for field meanings). |
| `NativeDataset` | [`native_dataset`](/generated/api/ref/native_dataset) | type | The native arrays from one blob: data `variables` + grid `coords`. |
| `NativeField` | [`native_field`](/generated/api/ref/native_field) | type | One native-grid array as a reader decodes it. |
| `NetCDFReader` | [`netcdf_reader`](/generated/api/ref/netcdf_reader) | type | The active `netcdf` reader, backed by xarray (netCDF4 engine). |
| `ParquetReader` | [`parquet_reader`](/generated/api/ref/parquet_reader) | type | The active `parquet` reader — an Apache Parquet file as a flat table. |
| `PrefetchGroup` | [`prefetch_group`](/generated/api/ref/prefetch_group) | type | Background decode of upcoming forcing epochs (`spec/prefetch.md` §3). |
| `Provider` | [`provider`](/generated/api/ref/provider) | type | A loader-bound provider of native-grid arrays, refreshed at the loader's cadence. |
| `RegionNotShardAligned` | [`region_not_shard_aligned`](/generated/api/ref/region_not_shard_aligned) | type | A writer region violates the alignment rule of `spec/writer.md` §4. |
| `resolve_cache_root` | [`data_dir`](/generated/api/ref/data_dir) | function | Resolve the cache root directory. |
| `resolve_offline` | [`is_offline`](/generated/api/ref/is_offline) | function | Resolve effective offline mode. |
| `Sample` | [`prefetch_sample`](/generated/api/ref/prefetch_sample) | type | One member's data for one epoch. |
| `sha256_file` | [`sha256_file`](/generated/api/ref/sha256_file) | function | sha256 of a file's contents, streamed so large blobs don't load to RAM. |
| `ShapefileReader` | [`shapefile_reader`](/generated/api/ref/shapefile_reader) | type | The active `shapefile` reader — an ESRI shapefile as a feature table. |
| `SourceTemporal` | [`source_temporal`](/generated/api/ref/source_temporal) | type | The temporal cadence of a DISCRETE loader (absent ⇒ CONST). |
| `Temporal` | [`temporal`](/generated/api/ref/temporal) | type | The freshness policy derived from a loader's `temporal` block. |
| `Window` | [`window`](/generated/api/ref/window) | type | A run window `(start, end)` — half-open `[start, end)`. |
