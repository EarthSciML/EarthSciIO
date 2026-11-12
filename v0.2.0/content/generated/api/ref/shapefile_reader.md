---
title: "shapefile_reader"
description: "Stable type · Native arrays and readers"
type: "api"
api_tabs: true
---


**Tier:** stable · **Kind:** type · **Topic:** [Native arrays and readers](/generated/api/topics/native-arrays)

| Python | Julia | Rust |
|---|---|---|
| `ShapefileReader` | `ShapefileReader` | `ShapefileReader` |


<div class="api-tabs">

<div class="api-tab" data-lang="python" data-label="Python">
<p class="api-tab-label">Python</p>

```python
class ShapefileReader
ShapefileReader.formats() -> list[str]
ShapefileReader.extensions() -> list[str]
ShapefileReader.open(blob_path: Any) -> Any
ShapefileReader.read_native(handle: Any, variables: Sequence[str] | None = None, select: Any | None = None, *, member: str | None = None, numeric_columns: Sequence[str] | None = None, nvert_max: int | None = None, **_: Any) -> NativeDataset
```

The active `shapefile` reader — an ESRI shapefile as a feature table.

Decode is delegated to **pyshp** (`shapefile`), imported lazily so a
base install stays lean, mirroring the Julia `Shapefile.jl` extension and
the Rust `shapefile` crate. Nothing about the format is re-implemented
here; what this reader owns is the mapping onto the NATIVE-ARRAY contract.

**One row per PART.** A shapefile record may carry several parts — a
polygon's outer ring plus its holes, a county's mainland plus its islands, a
multi-part route. The op that consumes this geometry
(`polygon_intersection_area`, `intersect_polygon`) takes ONE ring, so a
reader that surfaced only the first part would silently drop the islands.
Each part therefore becomes one row of the `index` axis, with the record's
`.dbf` attributes REPLICATED across its parts and `shape_index` /
`part_index` / `n_parts` naming where the row came from. A layer whose
records are all single-part (the common case) decodes 1:1.

Variables:

`geometry`
    `float64[index, vertex, xy]` — the vertex rings, right-PADDED to the
    longest part by REPEATING the final vertex. That padding is the
    rectangular-vertex-storage convention `esm-spec` §8.6.1 pins: a binding
    evaluates such a ring as its deduplicated form, so a short ring padded
    this way has the same area as the ring itself. A Null shape's row is all
    `NaN`. Vertices are kept exactly as stored — the explicit closing
    vertex of a shapefile ring is NOT dropped (§8.6.1 makes dropping it the
    kernel's job) and no winding is normalized.
`shape_type`, `crs_wkt`
    `str[meta]` — one-element string fields: the layer's shape type, and
    the `.prj` WKT verbatim when the archive carries one (`crs_wkt` is
    absent otherwise). The native CRS is DECLARED, not acted on —
    **reprojection is ESD's job, never the reader's**. They are fields rather
    than field `attrs` because the Rust `NativeField` has no `attrs`,
    and the cross-language native-array equality check compares fields.
`n_vertices`
    `int64[index]` — real vertices in the part, before padding.
`shape_index`, `part_index`, `n_parts`
    `int64[index]` — the 0-based record index in the `.shp`, the 0-based
    part index inside it, and its part count.
`xmin`, `ymin`, `xmax`, `ymax`
    `float64[index]` — the parent RECORD's stored bounding box (a Point's
    own coordinate where the format stores no box), replicated to its parts.
    On disk, not computed: it is the shapefile's own broad-phase envelope.
one field per `.dbf` column
    `[index]`, replicated to parts. Numeric (`N`/`F`) columns become
    `float64` with a blank as `NaN`; `L` becomes `bool`; `D` and
    `C` stay `str`. A row whose deletion flag is `*` is dropped, and
    no other flag byte means deleted (`spec/conformance.md` §3).

READER-ONLY (Risk R3): no reprojection, no unit conversion, no ring
orientation fix, no polygon/hole classification, no name remap.

`reader_kwargs`: `member="path/in/zip"` names the `.shp` inside a zip
blob (sidecars are the same stem with `.dbf`/`.shx`/`.prj`); it never enters
the cache key. `numeric_columns=[...]` parses the named `C` columns as
`float64` — the `CSVReader`/`FF10Reader` spelling, for a text-typed
code column (a FIPS `GEOID`) a model wants as a number.

**Members**

- `read_native` — Decode a shapefile blob into a one-row-per-part feature table.

[Source](https://github.com/EarthSciML/EarthSciIO/blob/a49538bbd6ec318dde369d3496f6e8880586a73f/earthsciio/readers.py#L1322)

</div>

<div class="api-tab" data-lang="julia" data-label="Julia">
<p class="api-tab-label">Julia</p>

```julia
struct ShapefileReader <: Reader
ShapefileReader()
```

```julia
ShapefileReader()
```

The `shapefile` format reader — an ESRI shapefile as a feature table.

Decode is delegated to **Shapefile.jl**, loaded LAZILY via a weakdep extension
(`EarthSciIOShapefileExt`, active on `using Shapefile`) — mirroring the Python
reader's lazy `pyshp` import and the Rust `shapefile` crate. Calling
`read_native` without `using Shapefile` throws a clear install hint.

**One row per PART.** A shapefile record may carry several parts — a polygon's
outer ring plus its holes, a county's mainland plus its islands. The op that
consumes this geometry (`polygon_intersection_area`, `intersect_polygon`) takes
ONE ring, so a reader that surfaced only the first part would silently drop the
islands. Each part becomes one row of the `index` axis, with the record's `.dbf`
attributes REPLICATED across its parts; a single-part layer decodes 1:1.

Variables: `geometry` (`Float64[index, vertex, xy]`, right-PADDED to the longest
part by REPEATING the final vertex — the `esm-spec` §8.6.1 rectangular-storage
convention, which a binding evaluates as the deduplicated ring; a Null shape's
row is all `NaN`), `shape_type` and — when the archive carries a `.prj` —
`crs_wkt` (`String[meta]`, one element each: the layer's shape type and the
projection WKT verbatim; the native CRS is DECLARED, not acted on),
`n_vertices`/`shape_index`/`part_index`/
`n_parts` (`Int64[index]`), `xmin`/`ymin`/`xmax`/`ymax` (`Float64[index]`, the
parent record's STORED bounding box replicated to its parts) and one field per
`.dbf` column (`N`/`F` -> Float64 with blank as `NaN`, `L` -> Bool, `D`/`C` ->
String). A row whose deletion flag is `*` is dropped, and no other flag byte
means deleted (spec/conformance.md §3).

READER-ONLY (Risk R3): no reprojection, no unit conversion, no ring orientation
fix, no polygon/hole classification, no name remap.

`reader_kwargs`: `member="path/in/zip"` names the `.shp` inside a zip blob
(sidecars are the same stem with `.dbf`/`.shx`/`.prj`); it never enters the
cache key. `numeric_columns=[...]` parses the named `C` columns as Float64 — the
`CSVReader`/`FF10Reader` spelling, for a text-typed code column (a FIPS `GEOID`)
a model wants as a number. `nvert_max=N` pads `geometry` to exactly `N` vertex
slots instead of to the longest part, so a DOCUMENT declares the vertex-axis
length rather than inheriting a number the file happens to have (a longer part
is an error naming it, never a silent truncation). `variables=[...]` restricts
the returned fields.

[Source](https://github.com/EarthSciML/EarthSciIO/blob/a49538bbd6ec318dde369d3496f6e8880586a73f/julia/src/readers.jl#L1434)

</div>

<div class="api-tab" data-lang="rust" data-label="Rust">
<p class="api-tab-label">Rust</p>

```rust
pub struct ShapefileReader {
    /* private fields */
}

impl ShapefileReader { pub fn new() -> Self }

impl ShapefileReader { pub fn member(self, member: impl Into<String>) -> Self }

impl ShapefileReader { pub fn numeric_columns<I, S>(self, columns: I) -> Self where I: IntoIterator<Item = S>, S: Into<String> }

impl ShapefileReader { pub fn nvert_max(self, n: usize) -> Self }
```

The `shapefile` format reader. See the module docs for the decode contract.

[Source](https://github.com/EarthSciML/EarthSciIO/blob/a49538bbd6ec318dde369d3496f6e8880586a73f/rust/src/format/shapefile.rs#L72) · [Rust docs](https://docs.rs/earthsciio/latest/earthsciio/struct.ShapefileReader.html)

</div>

</div>
