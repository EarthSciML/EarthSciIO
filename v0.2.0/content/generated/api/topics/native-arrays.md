---
title: "Native arrays and readers"
description: "The raw native-grid arrays a provider returns, and the format readers that decode cached files into them."
type: "api"
---


| Operation | Kind | Bindings | Summary |
|---|---|---|---|
| [`csv_reader`](/generated/api/ref/csv_reader) | type | Python, Julia | The active `csv` reader — a non-NetCDF format behind the same registry. |
| [`ff10_reader`](/generated/api/ref/ff10_reader) | type | Python, Julia, Rust | The active `ff10` reader — the RAW long-format FF10 **point** table. |
| [`geotiff_reader`](/generated/api/ref/geotiff_reader) | type | Python, Julia, Rust | The active `geotiff` reader — raster bands on a native lon/lat grid. |
| [`native_dataset`](/generated/api/ref/native_dataset) | type | Python, Julia, Rust | The native arrays from one blob: data `variables` + grid `coords`. |
| [`native_field`](/generated/api/ref/native_field) | type | Python, Julia, Rust | One native-grid array as a reader decodes it. |
| [`netcdf_reader`](/generated/api/ref/netcdf_reader) | type | Python, Julia, Rust | The active `netcdf` reader, backed by xarray (netCDF4 engine). |
| [`parquet_reader`](/generated/api/ref/parquet_reader) | type | Python, Julia, Rust | The active `parquet` reader — an Apache Parquet file as a flat table. |
| [`shapefile_reader`](/generated/api/ref/shapefile_reader) | type | Python, Julia, Rust | The active `shapefile` reader — an ESRI shapefile as a feature table. |
| [`zarr_reader`](/generated/api/ref/zarr_reader) | type | Julia, Rust | The `zarr` store-backed reader (Zarr v2 and sharded Zarr v3 chunked arrays). |

[All topics](/generated/api)
