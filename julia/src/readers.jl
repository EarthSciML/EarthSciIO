# Format readers — component (b) (esio-9nb.5). A reader opens a cached blob and
# returns RAW native-grid arrays keyed by the on-disk `file_variable` name. It
# applies ONLY the format/CF decode pinned by spec/conformance.md §3; it does
# NOT remap variable names or convert units (those stay in ESS — Risk R3).
#
# Readers register into `FORMAT_REGISTRY` by name (`netcdf`, `csv`) and are the
# decode half (conformance checks 3–4) that the cache layer (component a) left
# to this bead. A new format plugs in by adding a `Reader` subtype + a
# `read_native` method + one `register!` line — never a Provider change.

# --- native array containers ------------------------------------------------

"""
    NativeField(data, dims, attrs)

One native-grid array exactly as a reader decodes it: `data` is a Julia array
whose axes correspond, in order, to `dims` (the on-disk dimension names, file
order — e.g. `["time","latitude","longitude"]`). Numeric fields are `Float64`
with `NaN` for masked/`_FillValue` cells; raw integer reads (e.g. an undecoded
time axis) keep their stored integer type; text columns are `String`. `attrs`
carries decode-relevant metadata the reader must NOT act on but ESS needs —
notably a time axis's `units`/`calendar` (calendar decoding is ESS's job)."""
struct NativeField
    data::AbstractArray
    dims::Vector{String}
    attrs::Dict{String,Any}
end
NativeField(data::AbstractArray, dims::AbstractVector) =
    NativeField(data, String.(collect(dims)), Dict{String,Any}())

Base.size(f::NativeField) = size(f.data)
Base.eltype(f::NativeField) = eltype(f.data)

function Base.show(io::IO, f::NativeField)
    print(io, "NativeField(", eltype(f.data), " ", join(string.(size(f.data)), "×"),
          " dims=", f.dims, ")")
end

"""
    NativeDataset(variables, coords)

The native arrays a reader returns from one blob: `variables` (data fields keyed
by `file_variable`) and `coords` (the dimension-coordinate fields — e.g. the
grid's `latitude`/`longitude`/`time`). Both map `String` name → [`NativeField`].
`getindex` looks in `variables` then `coords`, so `nds["t2m"]` and `nds["time"]`
both resolve."""
struct NativeDataset
    variables::Dict{String,NativeField}
    coords::Dict{String,NativeField}
end
NativeDataset() = NativeDataset(Dict{String,NativeField}(), Dict{String,NativeField}())

function Base.getindex(nds::NativeDataset, name::AbstractString)
    k = String(name)
    haskey(nds.variables, k) && return nds.variables[k]
    haskey(nds.coords, k) && return nds.coords[k]
    throw(KeyError(k))
end
Base.haskey(nds::NativeDataset, name::AbstractString) =
    haskey(nds.variables, String(name)) || haskey(nds.coords, String(name))
"""Names of the data variables (not coordinates)."""
variable_names(nds::NativeDataset) = sort!(collect(keys(nds.variables)))
"""Names of the coordinate fields."""
coord_names(nds::NativeDataset) = sort!(collect(keys(nds.coords)))

function Base.show(io::IO, nds::NativeDataset)
    print(io, "NativeDataset(variables=", variable_names(nds),
          ", coords=", coord_names(nds), ")")
end

# --- NetCDF reader (NCDatasets) ---------------------------------------------

"""
    NetCDFReader()

The `netcdf` format reader, backed by NCDatasets. CF-decodes per
spec/conformance.md §3: applies `scale_factor`/`add_offset` (math in float64),
maps `_FillValue`/`missing_value` cells to `NaN`, and returns the time axis
**raw** (the stored integers/floats) with `units`+`calendar` carried in
`attrs` — calendar→wall-clock decoding is ESS's job, never the reader's.

NCDatasets exposes arrays in column-major (reversed) dimension order; this
reader permutes each array back to **file order** so `field.dims` and
`size(field.data)` match the on-disk layout (and the Python/xarray track)."""
struct NetCDFReader <: Reader end

# A CF time axis is one whose `units` is "<step> since <reference>" (hours since
# …, days since …). Matching xarray `decode_times=false`, such variables are
# returned raw; `mask_and_scale` still applies to everything else.
function _is_cf_time(attrib)::Bool
    haskey(attrib, "units") || return false
    return occursin(r"\bsince\b"i, strip(String(attrib["units"])))
end

# Reverse NCDatasets' storage (column-major) order to the file's logical order.
_to_file_order(a::AbstractArray) =
    ndims(a) > 1 ? permutedims(a, reverse(1:ndims(a))) : a

# Decode rule (spec/conformance.md §3): masked → Float64 with NaN; an unpacked
# pure-integer field keeps its integer logical type; every other numeric read is
# normalized to Float64 so float32-vs-float64 never diverges across languages.
function _finalize_numeric(a::AbstractArray)
    if Missing <: eltype(a)
        return map(x -> ismissing(x) ? NaN : Float64(x), a)   # → Array{Float64}, shape kept
    elseif eltype(a) <: Integer
        return collect(a)
    elseif eltype(a) <: AbstractFloat
        return Float64.(a)
    else
        return collect(a)
    end
end

function _carry_attrs(attrib)
    d = Dict{String,Any}()
    for k in ("units", "calendar")
        haskey(attrib, k) && (d[k] = String(attrib[k]))
    end
    return d
end

function read_native(::NetCDFReader, path::AbstractString)
    nds = NativeDataset()
    NCDatasets.NCDataset(String(path), "r") do ds
        dimset = Set(String.(collect(keys(ds.dim))))
        for vn in keys(ds)
            v = ds[vn]
            attrs = _carry_attrs(v.attrib)
            file_dims = reverse(String.(collect(NCDatasets.dimnames(v))))
            if _is_cf_time(v.attrib)
                # Raw, undecoded: read the underlying variable (no CF transform),
                # so a "hours since …" axis stays the stored integers.
                data = _to_file_order(Array(v.var))
            else
                # mask_and_scale: NCDatasets applies scale/offset + _FillValue→missing.
                data = _finalize_numeric(_to_file_order(Array(v)))
            end
            field = NativeField(data, file_dims, attrs)
            if String(vn) in dimset
                nds.coords[String(vn)] = field
            else
                nds.variables[String(vn)] = field
            end
        end
    end
    return nds
end

# --- CSV reader -------------------------------------------------------------

"""
    CSVReader()

The `csv` format reader — a second reader proving a non-NetCDF format plugs into
`FORMAT_REGISTRY` unchanged (spec/conformance.md). Columns named in
`numeric_columns` parse to `Float64` 1-D arrays keyed by the column
(`file_variable`) name; every other column is returned as a `String` array. All
fields have a single dimension `index`; there are no coordinates.

`numeric_columns` is REQUIRED by the loader spec and is not inferred: the corpus
`location_id` column is digit-only text (`"1"`,`"2"`) yet must stay a string, so
"parses as a number" is not a safe signal. When `numeric_columns === nothing`
the reader falls back to best-effort inference (every value parses as a float),
which the loader/`.esm` node should override. Quoted fields with embedded
delimiters are not handled (the points corpus has none); add that when a loader
needs it."""
struct CSVReader <: Reader end

_parses_float(s::AbstractString) = tryparse(Float64, strip(s)) !== nothing

function read_native(::CSVReader, path::AbstractString;
                     numeric_columns = nothing, delimiter::AbstractString = ",",
                     header_row::Integer = 1)
    rows = Vector{String}[]
    for ln in eachline(String(path))
        isempty(ln) && continue
        push!(rows, String.(split(rstrip(ln, ['\r']), delimiter)))
    end
    isempty(rows) && return NativeDataset()
    header = rows[header_row]
    body = rows[header_row+1:end]
    numset = numeric_columns === nothing ? nothing : Set(String.(collect(numeric_columns)))

    vars = Dict{String,NativeField}()
    for (j, col) in enumerate(header)
        name = String(col)
        vals = String[r[j] for r in body]
        isnum = numset === nothing ? all(_parses_float, vals) : (name in numset)
        data = isnum ? Float64[parse(Float64, strip(v)) for v in vals] : vals
        vars[name] = NativeField(data, ["index"], Dict{String,Any}())
    end
    return NativeDataset(vars, Dict{String,NativeField}())
end

# --- GeoTIFF reader (TiffImages backend via a weakdep extension) ------------

"""
    GeoTIFFReader()

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
returned bands (a requested-but-absent band is a `KeyError`)."""
struct GeoTIFFReader <: Reader end

# The real decode lives in ext/EarthSciIOTiffImagesExt.jl, whose method is typed
# `path::AbstractString` — strictly MORE specific than this untyped-`path` fallback,
# so when `using TiffImages` is active it wins by dispatch (no method overwrite,
# which precompilation forbids). This fallback fires only when the backend is absent.
read_native(::GeoTIFFReader, path; kwargs...) = error(
    "the geotiff reader needs the TiffImages backend: add `using TiffImages` so the " *
    "EarthSciIOTiffImagesExt extension supplies the decode (kept a weakdep to keep a " *
    "base EarthSciIO install light, mirroring the Python tifffile-optional path).")

# GTModelTypeGeoKey-style lookup (key 1024: 1=projected, 2=geographic) from a flat
# `GeoKeyDirectoryTag`. The directory is [version,keyRev,minorRev,nKeys,
# (KeyID,loc,count,value)*nKeys]; only INLINE keys (loc==0) carry their value in
# the 4th slot. Returns the value or `nothing`.
function _geotiff_geokey(geokeys, key_id::Integer)
    geokeys === nothing && return nothing
    g = Int[Int(v) for v in geokeys]
    length(g) < 4 && return nothing
    n = g[4]
    for k in 0:(n - 1)
        off = 4 + 4k                      # 0-based offset of the k-th key entry
        off + 4 <= length(g) || break
        g[off + 1] == key_id && g[off + 2] == 0 && return g[off + 4]
    end
    return nothing
end

# Parse the GDAL_NODATA sentinel (an ASCII tag, often null-terminated) → Float64,
# or `nothing` when absent/unparseable.
function _geotiff_nodata(raw)
    raw === nothing && return nothing
    s = raw isa AbstractVector{UInt8} ? String(copy(raw)) : String(raw)
    s = strip(replace(s, '\0' => ""))
    isempty(s) && return nothing
    return tryparse(Float64, s)
end

"""
    _assemble_geotiff(bands, tags; variables=nothing, band_names=nothing) -> NativeDataset

Build the GeoTIFF [`NativeDataset`] from decoded `bands` (each a `(height,width)`
`Matrix{Float64}` in file order, rows = y/lat) plus the raw IFD `tags` (Int tag id
→ value): cell-center axes from `ModelPixelScaleTag` (33550) + `ModelTiepointTag`
(33922) — `x = x0 + (col − i0 + 0.5)·sx`, `y = y0 − (row − j0 + 0.5)·sy`, GeoTIFF
model space being y-up while raster rows increase downward — the geographic vs
projected flag from `GeoKeyDirectoryTag` (34735) GTModelTypeGeoKey, and
`GDAL_NODATA` (42113) → `NaN`. Shared decode CONTRACT: the TiffImages backend (and
any future GDAL one) only supplies `bands`+`tags`, so the georef math lives once."""
function _assemble_geotiff(bands::AbstractVector, tags::AbstractDict;
                           variables = nothing, band_names = nothing)
    isempty(bands) && throw(ArgumentError("GeoTIFF has no raster bands"))
    nbands = length(bands)
    scale = get(tags, 33550, nothing)
    tie = get(tags, 33922, nothing)
    (scale === nothing || tie === nothing) && throw(ArgumentError(
        "GeoTIFF lacks ModelPixelScaleTag/ModelTiepointTag; cannot derive a grid " *
        "(a non-tiepoint affine georeferencing needs the GDAL backend)."))
    sx, sy = Float64(scale[1]), Float64(scale[2])
    i0, j0 = Float64(tie[1]), Float64(tie[2])
    x0, y0 = Float64(tie[4]), Float64(tie[5])
    H, W = size(bands[1])
    xs = Float64[x0 + (c - i0 + 0.5) * sx for c in 0:(W - 1)]
    ys = Float64[y0 - (r - j0 + 0.5) * sy for r in 0:(H - 1)]
    geographic = _geotiff_geokey(get(tags, 34735, nothing), 1024) != 1
    nodata = _geotiff_nodata(get(tags, 42113, nothing))

    names = band_names === nothing ? ["Band$(i)" for i in 1:nbands] :
            String[String(n) for n in band_names]
    length(names) == nbands || throw(ArgumentError(
        "band_names has $(length(names)) entries but the GeoTIFF has $nbands band(s)"))
    ydim, xdim = geographic ? ("lat", "lon") : ("y", "x")
    want = variables === nothing ? nothing : Set(String[String(v) for v in variables])
    if want !== nothing
        miss = sort!(String[v for v in want if !(v in names)])
        isempty(miss) || throw(KeyError(
            "requested bands not in GeoTIFF: $miss; present bands: $names"))
    end
    vars = Dict{String,NativeField}()
    for (nm, band) in zip(names, bands)
        want !== nothing && !(nm in want) && continue
        data = Array{Float64}(band)                  # copy: we may write NaN below
        if nodata !== nothing && !isnan(nodata)
            @inbounds for k in eachindex(data)
                data[k] == nodata && (data[k] = NaN)
            end
        end
        vars[nm] = NativeField(data, [ydim, xdim], Dict{String,Any}())
    end
    coords = Dict{String,NativeField}(
        xdim => NativeField(xs, [xdim], Dict{String,Any}()),
        ydim => NativeField(ys, [ydim], Dict{String,Any}()))
    return NativeDataset(vars, coords)
end

# --- FF10 point reader (SMOKE FF10_POINT / Emissions.jl oracle) --------------

# The 77 FF10 point column names, in file order. Copied from Emissions.jl
# `src/ff10.jl` `FF10_POINT_COLUMNS`; the first two use the SMOKE FF10_POINT spec
# names COUNTRY_CD / REGION_CD (Emissions.jl names them COUNTRY / FIPS — identical
# values, a positional alias documented in conformance/ff10_oracle_emissions.jl).
const FF10_POINT_COLUMNS = String[
    "COUNTRY_CD", "REGION_CD", "TRIBAL_CODE", "FACILITY_ID",
    "UNIT_ID", "REL_POINT_ID", "PROCESS_ID", "AGY_FACILITY_ID",
    "AGY_UNIT_ID", "AGY_REL_POINT_ID", "AGY_PROCESS_ID", "SCC",
    "POLID", "ANN_VALUE", "ANN_PCT_RED", "FACILITY_NAME",
    "ERPTYPE", "STKHGT", "STKDIAM", "STKTEMP",
    "STKFLOW", "STKVEL", "NAICS", "LONGITUDE",
    "LATITUDE", "LL_DATUM", "HORIZ_COLL_MTHD", "DESIGN_CAPACITY",
    "DESIGN_CAPACITY_UNITS", "REG_CODES", "FAC_SOURCE_TYPE", "UNIT_TYPE_CODE",
    "CONTROL_IDS", "CONTROL_MEASURES", "CURRENT_COST", "CUMULATIVE_COST",
    "PROJECTION_FACTOR", "SUBMITTER_FAC_ID", "CALC_METHOD", "DATA_SET_ID",
    "FACIL_CATEGORY_CODE", "ORIS_FACILITY_CODE", "ORIS_BOILER_ID", "IPM_YN",
    "CALC_YEAR", "DATE_UPDATED", "FUG_HEIGHT", "FUG_WIDTH_XDIM",
    "FUG_LENGTH_YDIM", "FUG_ANGLE", "ZIPCODE", "ANNUAL_AVG_HOURS_PER_YEAR",
    "JAN_VALUE", "FEB_VALUE", "MAR_VALUE", "APR_VALUE",
    "MAY_VALUE", "JUN_VALUE", "JUL_VALUE", "AUG_VALUE",
    "SEP_VALUE", "OCT_VALUE", "NOV_VALUE", "DEC_VALUE",
    "JAN_PCTRED", "FEB_PCTRED", "MAR_PCTRED", "APR_PCTRED",
    "MAY_PCTRED", "JUN_PCTRED", "JUL_PCTRED", "AUG_PCTRED",
    "SEP_PCTRED", "OCT_PCTRED", "NOV_PCTRED", "DEC_PCTRED",
    "COMMENT",
]

# The 42 FF10 point columns decoded to Float64 (blank → NaN). Everything else
# (IDs, codes, free-text FACILITY_NAME, temporal tokens CALC_YEAR/DATE_UPDATED)
# stays String so leading-zero codes (REGION_CD "01001", ZIPCODE "00000", SCC,
# POLID) never become floats. Overridable via the `numeric_columns` kwarg.
const FF10_POINT_NUMERIC = Set{String}([
    "ANN_VALUE", "ANN_PCT_RED", "STKHGT", "STKDIAM", "STKTEMP", "STKFLOW",
    "STKVEL", "LONGITUDE", "LATITUDE", "DESIGN_CAPACITY", "CURRENT_COST",
    "CUMULATIVE_COST", "PROJECTION_FACTOR", "FUG_HEIGHT", "FUG_WIDTH_XDIM",
    "FUG_LENGTH_YDIM", "FUG_ANGLE", "ANNUAL_AVG_HOURS_PER_YEAR",
    "JAN_VALUE", "FEB_VALUE", "MAR_VALUE", "APR_VALUE", "MAY_VALUE", "JUN_VALUE",
    "JUL_VALUE", "AUG_VALUE", "SEP_VALUE", "OCT_VALUE", "NOV_VALUE", "DEC_VALUE",
    "JAN_PCTRED", "FEB_PCTRED", "MAR_PCTRED", "APR_PCTRED", "MAY_PCTRED",
    "JUN_PCTRED", "JUL_PCTRED", "AUG_PCTRED", "SEP_PCTRED", "OCT_PCTRED",
    "NOV_PCTRED", "DEC_PCTRED",
])

"""
    FF10Reader()

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
config so it never enters the cache key). `kind="point"` selects the schema (only
point ships). `numeric_columns`, `delimiter`, `comment` override the defaults;
`variables=[…]` restricts the returned columns (default = all 77)."""
struct FF10Reader <: Reader end

# RFC-4180 quote-aware split of ONE line into fields. A field may be wrapped in
# `"`, may contain the delimiter inside quotes (the free-text FACILITY_NAME), and
# `""` inside a quoted field is a literal quote. Quotes are stripped; the inner
# content is verbatim. The CSVReader's naive `split` cannot do any of this — the
# documented reason ff10 needs its own reader.
function _split_ff10_fields(line::AbstractString, delim::AbstractChar)
    fields = String[]
    buf = IOBuffer()
    inquote = false
    chars = collect(line)
    k = 1
    L = length(chars)
    while k <= L
        c = chars[k]
        if inquote
            if c == '"'
                if k < L && chars[k+1] == '"'   # escaped quote ""
                    write(buf, '"'); k += 1
                else
                    inquote = false
                end
            else
                write(buf, c)
            end
        else
            if c == '"'
                inquote = true
            elseif c == delim
                push!(fields, String(take!(buf)))
            else
                write(buf, c)
            end
        end
        k += 1
    end
    push!(fields, String(take!(buf)))
    return fields
end

# Read a named member of a zip archive as text (UTF-8), via ZipFile.jl. The
# member is reader config — NOT part of the cache key — because one cached
# `2016fd_inputs_point.zip` holds many member CSVs several loaders read.
function _ff10_member_text(path::AbstractString, member::AbstractString)
    reader = ZipFile.Reader(String(path))
    try
        for f in reader.files
            f.name == member && return String(read(f))
        end
        throw(ArgumentError("zip member $(repr(member)) not found in $(path); " *
            "members: $(String[f.name for f in reader.files])"))
    finally
        close(reader)
    end
end

function read_native(::FF10Reader, path::AbstractString;
                     member = nothing, kind::AbstractString = "point",
                     numeric_columns = FF10_POINT_NUMERIC,
                     delimiter::AbstractString = ",", comment::AbstractString = "#",
                     variables = nothing)
    kind == "point" || throw(ArgumentError(
        "FF10Reader only supports kind=\"point\" (got $(repr(kind))); the 45-col " *
        "nonpoint/onroad/nonroad schemas are not implemented yet"))
    text = member === nothing ? read(String(path), String) :
           _ff10_member_text(String(path), String(member))
    delim = first(delimiter)
    ncol = length(FF10_POINT_COLUMNS)

    rows = Vector{String}[]
    for ln in split(text, '\n')
        s = strip(ln)
        (isempty(s) || startswith(s, comment)) && continue
        fields = _split_ff10_fields(rstrip(ln, ['\r']), delim)
        length(fields) == ncol || throw(ArgumentError(
            "FF10 point row has $(length(fields)) fields, expected $ncol; " *
            "row=$(repr(ln))"))
        push!(rows, fields)
    end

    numset = Set(String.(collect(numeric_columns)))
    want = variables === nothing ? nothing : Set(String[String(v) for v in variables])
    if want !== nothing
        miss = sort!(String[v for v in want if !(v in FF10_POINT_COLUMNS)])
        isempty(miss) || throw(KeyError("requested FF10 columns not in schema: $miss"))
    end

    vars = Dict{String,NativeField}()
    for (j, name) in enumerate(FF10_POINT_COLUMNS)
        want !== nothing && !(name in want) && continue
        col = String[r[j] for r in rows]
        data = if name in numset
            Float64[isempty(strip(v)) ? NaN : parse(Float64, strip(v)) for v in col]
        else
            col
        end
        vars[name] = NativeField(data, ["index"], Dict{String,Any}())
    end
    return NativeDataset(vars, Dict{String,NativeField}())
end
