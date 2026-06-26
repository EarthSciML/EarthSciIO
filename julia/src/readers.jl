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
