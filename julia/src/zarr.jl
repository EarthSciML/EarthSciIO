# The `zarr` format reader — a STORE-BACKED chunked-array reader (Zarr v2).
#
# A Zarr v2 store is not one blob: each array's `.zarray`/`.zattrs` metadata and
# every chunk is its OWN object with its OWN URL, so "lazy partial read" is just
# "fetch only the chunk objects the selection intersects, each through the
# existing content-addressed cache" (spec/cloud-future.md §3; the zarr impl spec).
# No new cache-key scheme and no byte-range machinery are needed for the pinned
# v2 target.
#
# This reader therefore declares itself store-backed (`store_backed(::ZarrReader)
# = true`): the Provider hands it `(cache, base_url; variables, select)` instead
# of a single pre-fetched blob path (`_load` in provider.jl). It fetches each
# object it needs — `<base_url>/<array>/.zarray`, `.zattrs` (optional), and only
# the intersecting `<chunk_key>` chunk objects — via `fetch_blob(cache, url)`.
#
# Decode contract (spec/conformance.md §3, zarr notes): blosc (cname
# lz4/zstd/zlib/blosclz) / none decompression (c-blosc undoes the shuffle filter
# + multi-block container internally — supplied by the `Blosc` weakdep
# extension, mirroring the TiffImages pattern); C-order (or F-order) chunk unpack;
# endianness from the `dtype` typestr (`<f4`/`<f8` -> Float64), integer dtypes
# keep Int32/Int64; dim names from `_ARRAY_DIMENSIONS` (synthesized `dim_0…` if
# absent); NO coordinate arrays. fill_value is NOT mapped to NaN — a deliberate
# deviation (0.0 is real ISRM data); it fills only an ABSENT chunk object's region.

"""The `zarr` store-backed reader (Zarr v2 chunked arrays)."""
struct ZarrReader <: Reader end

store_backed(::ZarrReader) = true
supports_selection(::ZarrReader) = true

# --- .zarray / .zattrs metadata ---------------------------------------------

struct ZArrayMeta
    shape::Vector{Int}
    chunks::Vector{Int}
    byteorder::Char          # '<' little, '>' big, '|' n/a
    typechar::Char           # 'f' float, 'i'/'u' int
    itemsize::Int
    compressor::Union{Nothing,Dict{String,Any}}
    order::String            # "C" | "F"
    fill_value::Float64
    dim_sep::String          # "." | "/"
end

_ndim(m::ZArrayMeta) = length(m.shape)

function _parse_zarray(bytes)::ZArrayMeta
    d = JSON.parse(String(bytes))
    Int(get(d, "zarr_format", 2)) == 2 || error(
        "zarr reader supports zarr_format 2 only, got $(get(d,"zarr_format",nothing)) (v3 is future work)")
    shape = Int[Int(s) for s in d["shape"]]
    chunks = Int[Int(c) for c in d["chunks"]]
    length(shape) == length(chunks) ||
        error("zarr shape $shape and chunks $chunks rank mismatch")
    ts = String(d["dtype"])
    bo = ts[1] in ('<', '>', '|') ? ts[1] : '|'
    rest = ts[1] in ('<', '>', '|') ? ts[2:end] : ts
    tc = rest[1]
    isz = parse(Int, rest[2:end])
    filters = get(d, "filters", nothing)
    filters === nothing || error(
        "zarr reader does not support a filter pipeline yet (filters=$filters)")
    comp = get(d, "compressor", nothing)
    compressor = comp === nothing ? nothing : Dict{String,Any}(comp)
    order = String(get(d, "order", "C"))
    order in ("C", "F") || error("unknown zarr order '$order' (expected C or F)")
    fv = get(d, "fill_value", 0.0)
    fill_value = fv === nothing ? 0.0 : Float64(fv)
    sep = get(d, "dimension_separator", nothing)
    dim_sep = (sep === nothing || sep == "") ? "." : String(sep)
    return ZArrayMeta(shape, chunks, bo, tc, isz, compressor, order, fill_value, dim_sep)
end

# `_ARRAY_DIMENSIONS` or synthesized `dim_0…` names.
function _parse_zattrs(bytes, ndim::Int)::Vector{String}
    if bytes !== nothing
        d = JSON.parse(String(bytes))
        if haskey(d, "_ARRAY_DIMENSIONS")
            names = String[String(x) for x in d["_ARRAY_DIMENSIONS"]]
            length(names) == ndim && return names
        end
    end
    return String["dim_$(i)" for i in 0:(ndim - 1)]
end

# --- orthogonal selection ---------------------------------------------------

# `select` (from reader_kwargs) -> the per-axis selector vector, or `nothing`.
function _select_axes(select)
    select === nothing && return nothing
    if select isa AbstractDict && haskey(select, "axes")
        return collect(select["axes"])
    elseif select isa AbstractVector
        return collect(select)
    end
    return nothing
end

# One axis selector -> a tagged tuple.
function _parse_axis(spec)
    (spec === nothing || spec == "all") && return (:all,)
    if spec isa AbstractDict
        haskey(spec, "indices") && return (:indices, Int[Int(i) for i in spec["indices"]])
        if haskey(spec, "slice")
            s = spec["slice"]
            step = length(s) > 2 ? Int(s[3]) : 1
            return (:slice, Int(s[1]), Int(s[2]), step)
        end
        error("unrecognized axis selector: $spec")
    elseif spec isa AbstractVector
        return (:indices, Int[Int(i) for i in spec])
    end
    error("unrecognized axis selector: $spec")
end

# A tagged axis selector -> its ordered list of 0-based global indices.
function _resolve_axis(ax, dim_len::Int)::Vector{Int}
    if ax[1] === :all
        return collect(0:(dim_len - 1))
    elseif ax[1] === :indices
        for g in ax[2]
            (0 <= g < dim_len) || error("index $g out of range for dimension length $dim_len")
        end
        return copy(ax[2])
    elseif ax[1] === :slice
        _, start, stop, step = ax
        step >= 1 || error("slice step must be >= 1, got $step")
        return collect(start:step:(stop - 1))
    end
    error("unrecognized axis selector: $ax")
end

# --- chunk math -------------------------------------------------------------

_chunk_key(idxs, sep::AbstractString) = join((string(Int(c)) for c in idxs), sep)

# The SET of chunk-id tuples the orthogonal selection intersects (the crux of
# laziness: an unselected chunk is never in this list).
function _needed_chunks(sel_indices, chunks)
    ndim = length(chunks)
    per_dim = Vector{Vector{Int}}(undef, ndim)
    for d in 1:ndim
        cl = chunks[d]
        per_dim[d] = sort!(collect(Set(g ÷ cl for g in sel_indices[d])))
    end
    out = Vector{NTuple{ndim,Int}}()
    for combo in Iterators.product(per_dim...)
        push!(out, combo)
    end
    return out
end

# --- decompression ----------------------------------------------------------

# Base fallback: the blosc codec lives in the `Blosc` weakdep extension. A base
# install without `using Blosc` errors here with an install hint (mirrors the
# TiffImages/GeoTIFF pattern in readers.jl).
_blosc_decompress(raw) = error(
    "the zarr reader needs the Blosc backend for blosc-compressed chunks: add " *
    "`using Blosc` so the EarthSciIOBloscExt extension supplies the decode (kept " *
    "a weakdep to keep a base EarthSciIO install light, mirroring the TiffImages path).")

# The plain (non-Blosc) `zstd` codec decode lives in the `CodecZstd` weakdep
# extension (`using CodecZstd`), mirroring the Blosc weakdep above. It is what
# the writer's `:wasm` profile emits — a standard Zarr v3 `zstd` codec, which a
# wasm/browser reader can decode where Blosc cannot.
_zstd_decompress(raw) = error(
    "the zarr reader needs the CodecZstd backend for plain zstd-compressed " *
    "chunks (the `wasm` output profile): add `using CodecZstd` so the " *
    "EarthSciIOZstdExt extension supplies the decode (kept a weakdep to keep a " *
    "base EarthSciIO install light, mirroring the Blosc path).")

_decompress(m::ZArrayMeta, raw) = _decompress(m.compressor, raw)
_decompress(::Nothing, raw) = raw
function _decompress(comp::AbstractDict, raw)
    id = lowercase(String(get(comp, "id", "")))
    if id == "blosc"
        return _blosc_decompress(raw)
    elseif id == "zstd"
        return _zstd_decompress(raw)
    elseif id in ("", "none")
        return raw
    else
        error("unsupported zarr compressor id '$id' (the Julia track supports blosc and zstd)")
    end
end

# Native element type + the finalized output element type (float -> Float64).
function _elt_type(m::ZArrayMeta)
    m.typechar == 'f' && m.itemsize == 4 && return Float32
    m.typechar == 'f' && m.itemsize == 8 && return Float64
    m.typechar in ('i', 'u') && m.itemsize == 4 && return Int32
    m.typechar in ('i', 'u') && m.itemsize == 8 && return Int64
    error("unsupported zarr dtype $(m.byteorder)$(m.typechar)$(m.itemsize)")
end
_out_type(m::ZArrayMeta) = _elt_type(m) <: AbstractFloat ? Float64 : _elt_type(m)

# Decompressed bytes -> a `chunks`-shaped array indexed in dims order (so
# `arr[a+1,b+1,…]` is the C-order chunk element `(a,b,…)`).
function _chunk_array(m::ZArrayMeta, raw)
    T = _elt_type(m)
    flat = collect(reinterpret(T, Vector{UInt8}(raw)))
    m.byteorder == '>' && (flat = bswap.(flat))
    ch = Tuple(m.chunks)
    if m.order == "C"
        return permutedims(reshape(flat, reverse(ch)...), reverse(1:_ndim(m)))
    else                                  # "F": first index varies fastest already
        return reshape(flat, ch...)
    end
end

# --- assembly ---------------------------------------------------------------

function _assemble(sel_indices, m::ZArrayMeta, buffers)
    ndim = _ndim(m)
    sel_shape = Tuple(length(s) for s in sel_indices)
    OT = _out_type(m)
    out = fill(convert(OT, m.fill_value), sel_shape...)
    chunks = m.chunks
    for I in CartesianIndices(sel_shape)
        gvec = ntuple(d -> sel_indices[d][I[d]], ndim)
        cvec = ntuple(d -> gvec[d] ÷ chunks[d], ndim)
        carr = buffers[cvec]
        carr === nothing && continue      # absent chunk object -> keep fill_value
        wvec = ntuple(d -> gvec[d] % chunks[d] + 1, ndim)   # 1-based within-chunk
        out[I] = convert(OT, carr[CartesianIndex(wvec)])
    end
    return out
end

# --- object fetch helpers ---------------------------------------------------

function _fetch_bytes(cache::Cache, url::AbstractString)
    entry = fetch_blob(cache, url)
    return read(entry.path)
end

function _fetch_bytes_optional(cache::Cache, url::AbstractString)
    try
        return _fetch_bytes(cache, url)
    catch e
        e isa CacheMiss && return nothing
        rethrow()
    end
end

# Probe fetch: returns bytes or `nothing` on ANY fetch failure (a missing object).
# Used only to detect v3 `zarr.json` vs v2 `.zarray` — a missing object is not an
# error there, it's the other layout (or absence). Distinct from
# `_fetch_bytes_optional`, which is v2-chunk-specific (only swallows `CacheMiss`).
function _probe_bytes(cache::Cache, url::AbstractString)
    try
        return _fetch_bytes(cache, url)
    catch
        return nothing
    end
end

# --- the store-backed entry point -------------------------------------------

"""
    read_store(::ZarrReader, cache, base_url; variables, select=nothing, _...)

Read `variables` (array names) from the Zarr store at `base_url` under an
orthogonal `select`. Reads BOTH Zarr **v2** (`.zarray`/`.zattrs`, plain chunk
objects) and Zarr **v3** (`zarr.json`, the sharding codec) — the layout is
detected per array by probing for `zarr.json` first, then `.zarray`. `variables`
is REQUIRED (the store cannot be enumerated). `select` (`Dict("axes"=>[...])`) is
applied to each array whose rank matches the axis count; other-rank arrays read
whole."""
function read_store(::ZarrReader, cache::Cache, base_url::AbstractString;
                    variables = nothing, select = nothing, _...)
    (variables === nothing || isempty(variables)) && error(
        "the zarr reader requires an explicit list of variables (arrays); the " *
        "store cannot be enumerated without consolidated metadata")
    base = rstrip(String(base_url), '/')
    axes_spec = _select_axes(select)

    vars = Dict{String,NativeField}()
    for array in variables
        arr = String(array)
        # Probe v2 `.zarray` FIRST: the existing corpus is all-v2, so a v2 array
        # costs exactly one metadata fetch (no extra probe) — preserving the
        # laziness fetch-count contract. Only a v2-miss falls back to v3 `zarr.json`.
        z2 = _probe_bytes(cache, "$base/$arr/.zarray")
        vars[arr] = z2 !== nothing ?
            _read_v2_array(cache, base, arr, z2, axes_spec) :
            _read_v3_array(cache, base, arr,
                           _fetch_bytes(cache, "$base/$arr/zarr.json"), axes_spec)
    end
    return NativeDataset(vars, Dict{String,NativeField}())
end

# --- v2 array read (the original path; `zbytes` = already-fetched .zarray) ---

function _read_v2_array(cache::Cache, base::AbstractString, arr::AbstractString,
                        zbytes, axes_spec)
    meta = _parse_zarray(zbytes)
    ndim = _ndim(meta)
    zattrs = _fetch_bytes_optional(cache, "$base/$arr/.zattrs")
    dims = _parse_zattrs(zattrs, ndim)

    axes = (axes_spec !== nothing && length(axes_spec) == ndim) ?
           [_parse_axis(a) for a in axes_spec] : [(:all,) for _ in 1:ndim]
    sel_indices = [_resolve_axis(axes[d], meta.shape[d]) for d in 1:ndim]

    buffers = Dict{NTuple{ndim,Int},Any}()
    for ck in _needed_chunks(sel_indices, meta.chunks)
        url = "$base/$arr/" * _chunk_key(ck, meta.dim_sep)
        raw = _fetch_bytes_optional(cache, url)
        buffers[ck] = raw === nothing ? nothing : _chunk_array(meta, _decompress(meta, raw))
    end
    data = _assemble(sel_indices, meta, buffers)
    return NativeField(data, dims, Dict{String,Any}())
end

# --- v3 array read (the sharding codec) -------------------------------------
#
# A Zarr v3 array is one `zarr.json` + shard objects under `<arr>/c/<shard coords>`.
# The array's `chunk_grid.chunk_shape` is the SHARD (outer) shape; the
# `sharding_indexed` codec's `chunk_shape` is the INNER (read) chunk. Each shard
# packs its inner chunks back-to-back followed by an index at the END: one
# (offset,nbytes) uint64-LE pair per inner chunk (missing = 2^64-1), plus a
# trailing crc32c (LE) over the index bytes. We reuse the v2 chunk decode
# (`_chunk_array`/`_assemble`) by treating the INNER chunk as the unit: resolve
# the selection's needed inner chunks, group them by shard, fetch each shard once,
# slice + blosc-decode the needed inner chunks, then assemble.

struct ZV3Meta
    shape::Vector{Int}
    shard::Vector{Int}          # outer chunk (chunk_grid.chunk_shape)
    inner::Vector{Int}          # sharding codec inner chunk_shape
    ips::Vector{Int}            # inner chunks per shard, per dim
    byteorder::Char
    typechar::Char
    itemsize::Int
    blosc::Bool
    zstd::Bool                  # plain v3 `zstd` codec (the `wasm` write profile)
    fill_value::Float64
    sep::String
    has_crc::Bool
    index_end::Bool
    dims::Vector{String}
end

# a ZArrayMeta view of a v3 array's INNER chunk, so `_chunk_array`/`_assemble`
# (written for v2) decode v3 inner chunks unchanged.
_inner_zarray(m::ZV3Meta) = ZArrayMeta(
    m.shape, m.inner, m.byteorder, m.typechar, m.itemsize,
    m.blosc ? Dict{String,Any}("id" => "blosc") :
    m.zstd  ? Dict{String,Any}("id" => "zstd")  : nothing,
    "C", m.fill_value, "/")

_v3_typecode(dt::AbstractString) =
    dt == "float64" ? ('f', 8) : dt == "float32" ? ('f', 4) :
    dt == "int32"   ? ('i', 4) : dt == "int64"   ? ('i', 8) :
    dt == "uint32"  ? ('u', 4) : dt == "uint64"  ? ('u', 8) :
    error("unsupported zarr v3 data_type '$dt' (the Julia track supports f4/f8/i4/i8/u4/u8)")

function _parse_zarr_json(bytes)::ZV3Meta
    d = JSON.parse(String(bytes))
    Int(get(d, "zarr_format", 3)) == 3 ||
        error("v3 parser got zarr_format $(get(d,"zarr_format",nothing))")
    get(d, "node_type", "array") == "array" ||
        error("zarr.json is a $(get(d,"node_type",nothing)) node, not an array")
    shape = Int[Int(s) for s in d["shape"]]
    tc, isz = _v3_typecode(String(d["data_type"]))

    cg = d["chunk_grid"]
    String(cg["name"]) == "regular" || error("unsupported chunk_grid '$(cg["name"])'")
    shard = Int[Int(s) for s in cg["configuration"]["chunk_shape"]]

    # locate the sharding_indexed codec
    codecs = get(d, "codecs", Any[])
    shc = nothing
    for c in codecs
        String(get(c, "name", "")) == "sharding_indexed" && (shc = c; break)
    end
    shc === nothing &&
        error("zarr v3 reader supports the sharding_indexed codec only (none found)")
    scfg = shc["configuration"]
    inner = Int[Int(s) for s in scfg["chunk_shape"]]

    byteorder = '<'
    blosc = false
    zstd = false
    for c in get(scfg, "codecs", Any[])
        nm = String(get(c, "name", ""))
        if nm == "bytes"
            en = String(get(get(c, "configuration", Dict()), "endian", "little"))
            byteorder = en == "big" ? '>' : '<'
        elseif nm == "blosc"
            blosc = true
        elseif nm == "zstd"
            zstd = true
        elseif nm == "transpose"
            error("zarr v3 reader does not support the transpose codec (C-order only)")
        end
    end

    has_crc = any(String(get(c, "name", "")) == "crc32c"
                  for c in get(scfg, "index_codecs", Any[]))
    index_end = String(get(scfg, "index_location", "end")) == "end"

    fv = get(d, "fill_value", 0.0)
    fill_value = fv === nothing ? 0.0 : Float64(fv)

    sep = "/"
    cke = get(d, "chunk_key_encoding", nothing)
    if cke !== nothing
        s = get(get(cke, "configuration", Dict()), "separator", "/")
        sep = String(s)
    end

    ndim = length(shape)
    ips = Int[shard[k] ÷ inner[k] for k in 1:ndim]

    attrs = get(d, "attributes", Dict())
    dims = if haskey(attrs, "_ARRAY_DIMENSIONS")
        String[String(x) for x in attrs["_ARRAY_DIMENSIONS"]]
    elseif haskey(d, "dimension_names") && d["dimension_names"] !== nothing
        String[String(x) for x in d["dimension_names"]]
    else
        String["dim_$(i)" for i in 0:(ndim - 1)]
    end
    length(dims) == ndim || (dims = String["dim_$(i)" for i in 0:(ndim - 1)])

    return ZV3Meta(shape, shard, inner, ips, byteorder, tc, isz, blosc, zstd,
                   fill_value, sep, has_crc, index_end, dims)
end

# Parse a shard's index into (offsets, nbytes) vectors (C-order over `ips`).
function _shard_index(m::ZV3Meta, shardbytes::Vector{UInt8})
    n_inner = prod(m.ips)
    idxlen = n_inner * 16 + (m.has_crc ? 4 : 0)
    length(shardbytes) >= idxlen ||
        error("shard object too small ($(length(shardbytes))B) for a $(n_inner)-chunk index")
    m.index_end || error("zarr v3 reader supports index_location=end only")
    tail = shardbytes[(end - idxlen + 1):end]
    body = m.has_crc ? tail[1:(end - 4)] : tail
    words = reinterpret(UInt64, body)
    offsets = Vector{UInt64}(undef, n_inner)
    nbytes = Vector{UInt64}(undef, n_inner)
    for i in 1:n_inner
        offsets[i] = ltoh(words[2i - 1])
        nbytes[i]  = ltoh(words[2i])
    end
    return offsets, nbytes
end

# C-order linear index (last dim fastest) of `coord` over grid `dims`.
function _c_linear(coord, dims)
    lin = 0
    for k in 1:length(dims)
        lin = lin * dims[k] + coord[k]
    end
    return lin
end

function _read_v3_array(cache::Cache, base::AbstractString, arr::AbstractString,
                        zjson, axes_spec)
    m = _parse_zarr_json(zjson)
    ndim = length(m.shape)
    im = _inner_zarray(m)

    axes = (axes_spec !== nothing && length(axes_spec) == ndim) ?
           [_parse_axis(a) for a in axes_spec] : [(:all,) for _ in 1:ndim]
    sel_indices = [_resolve_axis(axes[d], m.shape[d]) for d in 1:ndim]

    # needed INNER chunks, grouped by the shard that contains them
    needed = _needed_chunks(sel_indices, m.inner)          # inner-chunk id tuples
    by_shard = Dict{NTuple{ndim,Int},Vector{NTuple{ndim,Int}}}()
    for ic in needed
        sc = ntuple(d -> ic[d] ÷ m.ips[d], ndim)
        push!(get!(by_shard, sc, NTuple{ndim,Int}[]), ic)
    end

    buffers = Dict{NTuple{ndim,Int},Any}()
    for (sc, inners) in by_shard
        skey = join((string(sc[d]) for d in 1:ndim), m.sep)
        shardbytes = Vector{UInt8}(_fetch_bytes(cache, "$base/$arr/c/$skey"))
        offsets, nb = _shard_index(m, shardbytes)
        for ic in inners
            lc = ntuple(d -> ic[d] % m.ips[d], ndim)       # local coord within shard
            pos = _c_linear(lc, m.ips) + 1
            off, len = offsets[pos], nb[pos]
            if off == typemax(UInt64) || len == typemax(UInt64)
                buffers[ic] = nothing                       # missing inner chunk -> fill
            else
                raw = shardbytes[(Int(off) + 1):(Int(off) + Int(len))]
                buffers[ic] = _chunk_array(im, _decompress(im, raw))
            end
        end
    end

    data = _assemble(sel_indices, im, buffers)
    return NativeField(data, m.dims, Dict{String,Any}())
end

"""
    array_shape(::ZarrReader, cache, base_url, var) -> NTuple{N,Int}

The full (dims-order) shape of array `var` in the Zarr store at `base_url`,
learned by fetching ONLY that array's metadata object (v3 `zarr.json` or v2
`.zarray`) — NEVER a chunk/shard. A lightweight honour/refuse probe for
projection-pushdown decisions."""
function array_shape(::ZarrReader, cache::Cache, base_url::AbstractString,
                     var::AbstractString)
    base = rstrip(String(base_url), '/')
    arr = String(var)
    # v2 `.zarray` first (see `read_store`): an all-v2 store keeps the single-fetch
    # honour/refuse probe exact; only a v2-miss reads the v3 `zarr.json`.
    z2 = _probe_bytes(cache, "$base/$arr/.zarray")
    z2 !== nothing && return Tuple(_parse_zarray(z2).shape)
    return Tuple(_parse_zarr_json(_fetch_bytes(cache, "$base/$arr/zarr.json")).shape)
end
