---
title: "geotiff_reader"
description: "Stable type · Native arrays and readers"
type: "api"
api_tabs: true
---


**Tier:** stable · **Kind:** type · **Topic:** [Native arrays and readers](/generated/api/topics/native-arrays)

| Python | Julia | Rust |
|---|---|---|
| `GeoTIFFReader` | `GeoTIFFReader` | `GeoTiffReader` |


<div class="api-tabs">

<div class="api-tab" data-lang="python" data-label="Python">
<p class="api-tab-label">Python</p>

```python
class GeoTIFFReader
GeoTIFFReader.formats() -> list[str]
GeoTIFFReader.extensions() -> list[str]
GeoTIFFReader.open(blob_path: Any) -> Any
GeoTIFFReader.read_native(handle: Any, variables: Sequence[str] | None = None, select: Any | None = None, *, band_names: Sequence[str] | None = None, **_: Any) -> NativeDataset
```

The active `geotiff` reader — raster bands on a native lon/lat grid.

Decodes a GeoTIFF blob into a [`NativeDataset`](/generated/api/ref/native_dataset): one data variable per
raster band keyed `Band1`..`BandN` (1-based, the GDAL convention; the
LANDFIRE loader's `file_variable: "Band1"` matches), plus the cell-center
coordinate fields. Geographic rasters (the ArcGIS ImageServer `imageSR=4326`
responses) get `lon`/`lat` axes; projected rasters get `x`/`y`. Band
arrays are `float64` with the `GDAL_NODATA` sentinel mapped to `NaN`
(`spec/conformance.md` §3). Reader-only: no variable-name remap, no unit
conversion, no reprojection — those stay in ESS/ESD.

`reader_kwargs`: pass `band_names=[...]` to rename the bands positionally
(e.g. a single-band elevation raster → `["elevation"]`).

**Members**

- `read_native` — Decode `handle` into a [`NativeDataset`](/generated/api/ref/native_dataset) of raster bands + grid.

[Source](https://github.com/EarthSciML/EarthSciIO/blob/a49538bbd6ec318dde369d3496f6e8880586a73f/earthsciio/readers.py#L756)

</div>

<div class="api-tab" data-lang="julia" data-label="Julia">
<p class="api-tab-label">Julia</p>

```julia
struct GeoTIFFReader <: Reader
GeoTIFFReader()
```

```julia
GeoTIFFReader()
```

The `geotiff` format reader — raster bands on a native lon/lat (geographic) or
x/y (projected) grid. The decode half for the ArcGIS ImageServer `exportImage`
rasters the ESS loaders fetch (LANDFIRE fuel model, USGS 3DEP elevation). One
data variable per band keyed `Band1`..`BandN` (1-based, the GDAL convention; the
LANDFIRE loader's `file_variable: "Band1"` matches), plus the cell-center
coordinate fields. Geographic rasters (`imageSR=4326`) get `lon`/`lat` axes;
projected rasters get `x`/`y`. Band arrays are `Float64` with the `GDAL_NODATA`
sentinel mapped to `NaN` (spec/conformance.md §3). Reader-only: no variable-name
remap, no unit conversion, no reprojection — those stay in ESS/ESD.

The decode backend is the pure-Julia `TiffImages` package, loaded LAZILY via a
weakdep extension (`EarthSciIOTiffImagesExt`) — mirroring the Python reader's lazy
`tifffile` import, so a base EarthSciIO install stays light. Calling `read_native`
without `using TiffImages` throws a clear install hint.

`reader_kwargs`: `band_names=[...]` renames the bands positionally (e.g. a
single-band elevation raster → `["elevation"]`); `variables=[...]` restricts the
returned bands (a requested-but-absent band is a `KeyError`).

[Source](https://github.com/EarthSciML/EarthSciIO/blob/a49538bbd6ec318dde369d3496f6e8880586a73f/julia/src/readers.jl#L757)

</div>

<div class="api-tab" data-lang="rust" data-label="Rust">
<p class="api-tab-label">Rust</p>

```rust
pub struct GeoTiffReader;
impl GeoTiffReader { pub fn new() -> Self }
```

The active `geotiff` reader: pure-Rust raster decode + tiepoint georef.

[Source](https://github.com/EarthSciML/EarthSciIO/blob/a49538bbd6ec318dde369d3496f6e8880586a73f/rust/src/format/geotiff.rs#L44) · [Rust docs](https://docs.rs/earthsciio/latest/earthsciio/struct.GeoTiffReader.html)

</div>

</div>
