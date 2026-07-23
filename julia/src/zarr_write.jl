# The `zarr` WRITER — a streaming, sharded Zarr v3 output backend (the write
# mirror of ZarrReader). It hand-rolls Zarr v3 exactly as the reader hand-rolls
# v2: there is no high-level zarr writer library in the Julia track.
#
# Shape of the emitted store (RFC §16, normative):
#   * `zarr_format: 3` — one `zarr.json` per group and per array.
#   * The SHARDING codec (`sharding_indexed`): the array's `chunk_grid.chunk_shape`
#     is the SHARD (outer, write) shape; the sharding codec's `chunk_shape` is the
#     inner (read) chunk. One shard object packs many inner chunks (few large
#     objects). Inner-chunk pipeline = `[bytes(little), blosc(zstd+shuffle)]`;
#     index pipeline = `[bytes(little), crc32c]`, `index_location: "end"`.
#   * The `time` axis grows by array resize: each `write_record!` buffers one time
#     slice; a full time-shard flushes as ONE atomically-committed shard object,
#     after which each array's `zarr.json` `shape[time]` is rewritten.
#
# Commit discipline (identical to the read cache): every shard/metadata object is
# staged to `<base>/tmp/<uuid>.part` and `rename(2)`d into place; the per-store
# output manifest (`output_manifest.json`) records committed time-shards, the last
# durable `t`, the codec params, a schema fingerprint, and the base URL. The
# rename + manifest are the crash barrier — a reader never sees a partial shard.
#
# Conformance is TOLERANCE-BASED on decoded arrays (RFC §16), never byte-identity:
# Blosc container bytes are host/version dependent, so the round-trip asserts
# decoded values (within tol) + metadata equality, matching how the reader tests
# assert numcodecs<->Blosc.jl value-compatibility rather than byte-equality.

# --- pinned codec profiles (RFC §16) ----------------------------------------
# Diagnostic = zstd + byte-shuffle, moderate level. Checkpoint = lossless zstd
# (higher level; zstd is lossless — no lossy filter is ever applied). Pinned as
# constants so the Python/Rust ports match byte-for-byte on params.

"""A pinned Blosc codec profile: `cname`/`clevel`/`shuffle` (byte-shuffle)."""
struct BloscProfile
    cname::String
    clevel::Int
    shuffle::Bool
end

"""Diagnostic profile — Blosc **zstd** + byte-shuffle, moderate level (5)."""
const BLOSC_DIAGNOSTIC = BloscProfile("zstd", 5, true)
"""Checkpoint profile — **lossless** Blosc zstd (level 7) + byte-shuffle."""
const BLOSC_CHECKPOINT = BloscProfile("zstd", 7, true)

_profile(sym::Symbol) =
    sym === :diagnostic ? BLOSC_DIAGNOSTIC :
    sym === :checkpoint ? BLOSC_CHECKPOINT :
    error("unknown codec profile $sym (expected :diagnostic or :checkpoint)")

# --- the output schema ------------------------------------------------------

"""
    OutputVar(dims, dtype)

One streaming output variable: its on-disk dim names (file order, MUST include
the schema's `time_dim`) and its element type (`Float64`/`Float32`/`Int32`/`Int64`)."""
struct OutputVar
    dims::Vector{String}
    dtype::DataType
end
OutputVar(dims::AbstractVector, dtype::DataType) = OutputVar(String.(collect(dims)), dtype)

"""
    OutputSchema(; dims, time_dim, vars, chunk_shape, shard_shape,
                   coords=[], profile=:diagnostic, attrs=Dict(), time_dtype=Float64)

The input to [`write_open!`] — a plain struct in this repo (the EarthSciAST
`OutputSchema` maps onto it at the binding layer; no EarthSciAST dependency here).

Fields (all load-bearing for the binding to construct):
  * `dims::Vector{Pair{String,Int}}` — ORDERED dim name => length. The `time_dim`
    entry's length is a placeholder (0 is conventional); the time axis grows.
  * `time_dim::String` — the growable axis name.
  * `coords::Vector{Pair{String,Tuple{Vector,Dict{String,Any}}}}` — ORDERED static
    coordinate arrays: name => (values, attrs). 1-D over their own dim; written
    once at `write_open!`. An entry for `time_dim` supplies the time coordinate's
    attrs (its VALUES are ignored — they come from the `t` of each record).
  * `vars::Vector{Pair{String,OutputVar}}` — ORDERED streaming variables.
  * `chunk_shape::Dict{String,Int}` — dim name => INNER chunk length.
  * `shard_shape::Dict{String,Int}` — dim name => SHARD length (must be a multiple
    of the inner chunk length along every dim). `shard_shape[time_dim]` is the
    number of records packed per flushed shard object.
  * `profile::Symbol` — `:diagnostic` or `:checkpoint` (codec params).
  * `attrs::Dict{String,Any}` — group-level attributes.
  * `time_dtype::DataType` — element type of the time coordinate (default `Float64`)."""
struct OutputSchema
    dims::Vector{Pair{String,Int}}
    time_dim::String
    coords::Vector{Pair{String,Tuple{Vector,Dict{String,Any}}}}
    vars::Vector{Pair{String,OutputVar}}
    chunk_shape::Dict{String,Int}
    shard_shape::Dict{String,Int}
    profile::Symbol
    attrs::Dict{String,Any}
    time_dtype::DataType
end

function OutputSchema(; dims, time_dim, vars, chunk_shape, shard_shape,
                      coords = Pair{String,Tuple{Vector,Dict{String,Any}}}[],
                      profile::Symbol = :diagnostic,
                      attrs::AbstractDict = Dict{String,Any}(),
                      time_dtype::DataType = Float64)
    d = Pair{String,Int}[String(k) => Int(v) for (k, v) in dims]
    co = Pair{String,Tuple{Vector,Dict{String,Any}}}[
        String(k) => (collect(vals), Dict{String,Any}(a)) for (k, (vals, a)) in coords]
    vs = Pair{String,OutputVar}[String(k) => v for (k, v) in vars]
    cs = Dict{String,Int}(String(k) => Int(v) for (k, v) in chunk_shape)
    ss = Dict{String,Int}(String(k) => Int(v) for (k, v) in shard_shape)
    return OutputSchema(d, String(time_dim), co, vs, cs, ss, profile,
                        Dict{String,Any}(attrs), time_dtype)
end

# --- the writer + its handle ------------------------------------------------

"""The `zarr` streaming writer (sharded Zarr v3). Registered `:active` in
[`WRITER_REGISTRY`]; the write mirror of [`ZarrReader`]."""
struct ZarrWriter <: Writer end

"""Opaque write handle threaded through [`write_record!`]/[`write_close!`]."""
mutable struct ZarrWriteHandle
    base::String                       # output directory (local FS)
    schema::OutputSchema
    dimlens::Dict{String,Int}          # non-time dim => fixed length
    codec::BloscProfile
    shard_time::Int                    # records per time-shard
    n_in_shard::Int                    # records buffered in the current shard
    shard_time_index::Int              # 0-based index of the current time-shard
    total_records::Int                 # durably committed records
    time_buffer::Vector                # current shard's time-coord values
    buffers::Dict{String,Array}        # var name => (shard_time × spatial...) buffer
    time_shards::Vector{TimeShardRecord}
    shard_t_start::Float64             # t of the first record in the current shard
    last_t::Union{Float64,Nothing}
end

# --- dtype <-> Zarr v3 data_type --------------------------------------------

_v3_dtype(::Type{Float64}) = "float64"
_v3_dtype(::Type{Float32}) = "float32"
_v3_dtype(::Type{Int32})   = "int32"
_v3_dtype(::Type{Int64})   = "int64"
_v3_dtype(::Type{UInt32})  = "uint32"
_v3_dtype(::Type{UInt64})  = "uint64"
_v3_dtype(T::DataType) = error("unsupported output dtype $T for Zarr v3")

_v3_fill(::Type{<:AbstractFloat}) = 0.0
_v3_fill(::Type{<:Integer}) = 0

# --- compression backend (Blosc weakdep; the write mirror of _blosc_decompress)

# Base fallback: the compress lives in the `Blosc` weakdep extension. A base
# install without `using Blosc` errors here with an install hint (mirrors the
# `_blosc_decompress` fallback in zarr.jl).
_blosc_compress(bytes, cname, clevel, shuffle, typesize) = error(
    "the zarr writer needs the Blosc backend for blosc compression: add " *
    "`using Blosc` so the EarthSciIOBloscExt extension supplies the encode " *
    "(kept a weakdep to keep a base EarthSciIO install light).")

# --- output base resolution -------------------------------------------------

function _output_base(u::AbstractString)
    startswith(u, "s3://") && error(
        "s3 output is a later wave (the S3Store is a registered stub); Wave 1 " *
        "is local/parallel-FS only")
    startswith(u, "file://") && return file_url_to_path(u)
    return String(u)
end

# --- Zarr v3 metadata dicts -------------------------------------------------

_dims_tuple(shape_dict, dims) = Int[shape_dict[d] for d in dims]

function _sharding_codec(inner::Vector{Int}, codec::BloscProfile, typesize::Int)
    return Dict{String,Any}(
        "name" => "sharding_indexed",
        "configuration" => Dict{String,Any}(
            "chunk_shape" => inner,
            "codecs" => Any[
                Dict{String,Any}("name" => "bytes",
                                 "configuration" => Dict{String,Any}("endian" => "little")),
                Dict{String,Any}("name" => "blosc",
                                 "configuration" => Dict{String,Any}(
                                     "cname" => codec.cname,
                                     "clevel" => codec.clevel,
                                     "shuffle" => codec.shuffle ? "shuffle" : "noshuffle",
                                     "typesize" => typesize,
                                     "blocksize" => 0)),
            ],
            "index_codecs" => Any[
                Dict{String,Any}("name" => "bytes",
                                 "configuration" => Dict{String,Any}("endian" => "little")),
                Dict{String,Any}("name" => "crc32c"),
            ],
            "index_location" => "end",
        ),
    )
end

function _array_meta_dict(dims::Vector{String}, dtype::DataType, shape::Vector{Int},
                          schema::OutputSchema, codec::BloscProfile,
                          attrs::Dict{String,Any})
    inner = Int[schema.chunk_shape[d] for d in dims]
    shard = Int[schema.shard_shape[d] for d in dims]
    a = merge(attrs, Dict{String,Any}("_ARRAY_DIMENSIONS" => dims))
    return Dict{String,Any}(
        "zarr_format" => 3,
        "node_type"   => "array",
        "shape"       => shape,
        "data_type"   => _v3_dtype(dtype),
        "chunk_grid"  => Dict{String,Any}("name" => "regular",
            "configuration" => Dict{String,Any}("chunk_shape" => shard)),
        "chunk_key_encoding" => Dict{String,Any}("name" => "default",
            "configuration" => Dict{String,Any}("separator" => "/")),
        "fill_value"  => _v3_fill(dtype),
        "codecs"      => Any[_sharding_codec(inner, codec, sizeof(dtype))],
        "attributes"  => a,
        "dimension_names" => dims,
    )
end

_group_meta_dict(schema::OutputSchema) = Dict{String,Any}(
    "zarr_format" => 3, "node_type" => "group", "attributes" => schema.attrs)

_write_json(base, relpath, d) =
    put_object!(base, relpath, Vector{UInt8}(codeunits(JSON.json(d; pretty = true))))

# --- inner-chunk encode + shard assembly ------------------------------------

# C-order (row-major, last dim fastest) unravel of a linear index over `dims`.
function _c_unravel(lin::Int, dims::NTuple{N,Int}) where {N}
    coord = Vector{Int}(undef, N)
    rem = lin
    for d in N:-1:1
        coord[d] = rem % dims[d]
        rem ÷= dims[d]
    end
    return coord
end

# Extract one inner chunk (dims-order, `chunk`-shaped) from `data`. `gstart` is
# the chunk's global start per dim; `valid` the global valid length per dim;
# `base` maps global->data-local index (`data_index = global - base`; nonzero
# only for a growable/offset time axis). Returns `nothing` when the chunk has no
# real data (any dim's overlap is empty); else a fill-padded chunk array.
function _chunk_from(data::AbstractArray{T,N}, chunk::NTuple{N,Int}, gstart::NTuple{N,Int},
                     valid::NTuple{N,Int}, base::NTuple{N,Int}, fillval) where {T,N}
    lens = ntuple(d -> clamp(valid[d] - gstart[d], 0, chunk[d]), N)
    any(==(0), lens) && return nothing
    out = fill(T(fillval), chunk...)
    dst = ntuple(d -> 1:lens[d], N)
    src = ntuple(d -> (gstart[d] - base[d] + 1):(gstart[d] - base[d] + lens[d]), N)
    @inbounds out[dst...] = @view data[src...]
    return out
end

# Inner chunk array -> compressed bytes: C-order flatten -> little-endian bytes
# -> blosc (the `[bytes(little), blosc]` inner pipeline).
function _encode_chunk(chunk::AbstractArray{T,N}, codec::BloscProfile) where {T,N}
    flat = N == 1 ? vec(chunk) : vec(permutedims(chunk, reverse(1:N)))
    le = htol.(flat)
    bytes = Vector{UInt8}(reinterpret(UInt8, le))
    return _blosc_compress(bytes, codec.cname, codec.clevel, codec.shuffle, sizeof(T))
end

# Pack encoded inner chunks (C-order over `inner_per_shard`) into one shard
# object: [chunk bodies][index][crc32c]. The index is one (offset,nbytes) uint64
# LE pair per inner chunk; a missing chunk is (2^64-1, 2^64-1). `index_location`
# is "end"; the trailing 4 bytes are the crc32c (LE) of the index bytes.
function _assemble_shard(chunks::Vector{Union{Nothing,Vector{UInt8}}}, n_inner::Int)
    body = IOBuffer()
    offsets = fill(typemax(UInt64), n_inner)
    nbytes  = fill(typemax(UInt64), n_inner)
    pos = 0
    for i in 1:n_inner
        c = chunks[i]
        c === nothing && continue
        offsets[i] = UInt64(pos)
        nbytes[i]  = UInt64(length(c))
        write(body, c)
        pos += length(c)
    end
    idx = IOBuffer()
    for i in 1:n_inner
        write(idx, htol(offsets[i]))
        write(idx, htol(nbytes[i]))
    end
    idxbytes = take!(idx)
    crc = crc32c(idxbytes)
    out = IOBuffer()
    write(out, take!(body))
    write(out, idxbytes)
    write(out, htol(UInt32(crc)))
    return take!(out)
end

# Write EVERY shard of one array. `data` is the source array (dims order); for a
# growable time axis, its time extent is the current shard slab and `time_base`
# maps global time -> slab-local. `valid` is the global valid length per dim and
# `time_shard_only` restricts the time-axis shard grid to the current shard.
function _write_array_shards!(base::AbstractString, name::AbstractString,
                              dims::Vector{String}, dtype::DataType,
                              data::AbstractArray, schema::OutputSchema,
                              codec::BloscProfile, valid::Vector{Int},
                              time_base::Vector{Int}, ti::Int,
                              time_shard_only::Union{Int,Nothing})
    N = length(dims)
    inner  = ntuple(d -> schema.chunk_shape[dims[d]], N)
    shard  = ntuple(d -> schema.shard_shape[dims[d]], N)
    ips    = ntuple(d -> shard[d] ÷ inner[d], N)          # inner chunks per shard
    n_inner = prod(ips)
    fillv  = _v3_fill(dtype)
    basetup = ntuple(d -> time_base[d], N)
    validtup = ntuple(d -> valid[d], N)

    # shard-grid extent per dim (how many shards along d)
    nshard = ntuple(d -> begin
        full = (d == ti && time_shard_only !== nothing) ?
               (time_shard_only + 1) * shard[d] : valid[d]
        max(1, cld(full, shard[d]))
    end, N)
    ranges = ntuple(d -> (d == ti && time_shard_only !== nothing) ?
                    (time_shard_only:time_shard_only) : (0:(nshard[d] - 1)), N)

    for scombo in Iterators.product(ranges...)
        chunks = Vector{Union{Nothing,Vector{UInt8}}}(undef, n_inner)
        for ci in 0:(n_inner - 1)
            lc = _c_unravel(ci, ips)
            gstart = ntuple(d -> scombo[d] * shard[d] + lc[d] * inner[d], N)
            ca = _chunk_from(data, inner, gstart, validtup, basetup, fillv)
            chunks[ci + 1] = ca === nothing ? nothing : _encode_chunk(ca, codec)
        end
        shardbytes = _assemble_shard(chunks, n_inner)
        key = string(name, "/c/", join((string(scombo[d]) for d in 1:N), "/"))
        put_object!(base, key, shardbytes)
    end
    return nothing
end

# --- static (time-independent) coordinate arrays ----------------------------

function _write_static_array!(base, name, values::AbstractVector, attrs, schema, codec)
    dims = [name]
    T = eltype(values)
    Tout = T <: Integer ? (sizeof(T) <= 4 ? Int32 : Int64) :
           (T <: AbstractFloat ? (T == Float32 ? Float32 : Float64) : Float64)
    data = collect(Tout, values)
    len = length(data)
    haskey(schema.chunk_shape, name) ||
        error("static coord '$name' has no chunk_shape entry")
    shape = [len]
    with_output_lock(base, name) do
        _write_json(base, "$name/zarr.json",
                    _array_meta_dict(dims, Tout, shape, schema, codec, Dict{String,Any}(attrs)))
    end
    _write_array_shards!(base, name, dims, Tout, data, schema, codec,
                         [len], [0], 0, nothing)
    return nothing
end

# --- write_open! ------------------------------------------------------------

function write_open!(::ZarrWriter, store, base_url::AbstractString, schema::OutputSchema)
    base = _output_base(base_url)
    mkpath(base)
    codec = _profile(schema.profile)
    dimlens = Dict{String,Int}(k => v for (k, v) in schema.dims if k != schema.time_dim)

    # validate the chunk/shard grid
    for (d, _) in schema.dims
        haskey(schema.chunk_shape, d) || error("dim '$d' missing from chunk_shape")
        haskey(schema.shard_shape, d) || error("dim '$d' missing from shard_shape")
        schema.shard_shape[d] % schema.chunk_shape[d] == 0 ||
            error("shard_shape[$d]=$(schema.shard_shape[d]) not a multiple of " *
                  "chunk_shape[$d]=$(schema.chunk_shape[d])")
    end
    shard_time = schema.shard_shape[schema.time_dim]

    # group metadata
    with_output_lock(base, "__group__") do
        _write_json(base, "zarr.json", _group_meta_dict(schema))
    end

    # static coords (values known now) — written once
    time_attrs = Dict{String,Any}()
    for (nm, (vals, attrs)) in schema.coords
        if nm == schema.time_dim
            time_attrs = attrs                      # attrs only; values come from t
            continue
        end
        _write_static_array!(base, nm, vals, attrs, schema, codec)
    end

    # time coordinate (growable, 1-D) + streaming vars: initial shape[time]=0
    with_output_lock(base, schema.time_dim) do
        _write_json(base, "$(schema.time_dim)/zarr.json",
                    _array_meta_dict([schema.time_dim], schema.time_dtype, [0],
                                     schema, codec, time_attrs))
    end
    for (nm, ov) in schema.vars
        schema.time_dim in ov.dims ||
            error("streaming var '$nm' must include the time dim '$(schema.time_dim)'")
        shape = _init_var_shape(ov, schema)
        with_output_lock(base, nm) do
            _write_json(base, "$nm/zarr.json",
                        _array_meta_dict(ov.dims, ov.dtype, shape, schema, codec,
                                         Dict{String,Any}()))
        end
    end

    h = ZarrWriteHandle(base, schema, dimlens, codec, shard_time, 0, 0, 0,
                        Vector{schema.time_dtype}(undef, 0),
                        Dict{String,Array}(), TimeShardRecord[], 0.0, nothing)
    _alloc_buffers!(h)
    return h
end

# var shape with time set to `nt` (dims order)
function _init_var_shape(ov::OutputVar, schema::OutputSchema, nt::Int = 0)
    dimlen = Dict(schema.dims)
    return Int[d == schema.time_dim ? nt : dimlen[d] for d in ov.dims]
end

# allocate fresh fill-initialized shard buffers (time axis = shard_time)
function _alloc_buffers!(h::ZarrWriteHandle)
    s = h.schema
    h.time_buffer = Vector{s.time_dtype}(undef, h.shard_time)
    h.buffers = Dict{String,Array}()
    for (nm, ov) in s.vars
        shp = Tuple(d == s.time_dim ? h.shard_time : h.dimlens[d] for d in ov.dims)
        h.buffers[nm] = fill(_v3_fill(ov.dtype) |> ov.dtype, shp...)
    end
    return nothing
end

# --- write_record! ----------------------------------------------------------

function write_record!(::ZarrWriter, h::ZarrWriteHandle, t, arrays; region = nothing)
    region === nothing || error("write_record! `region` is reserved (nothing in Wave 1)")
    s = h.schema
    slot = h.n_in_shard + 1
    h.n_in_shard == 0 && (h.shard_t_start = Float64(t))
    h.time_buffer[slot] = convert(s.time_dtype, t)

    for (nm, ov) in s.vars
        haskey(arrays, nm) || error("write_record! missing array for var '$nm'")
        a = arrays[nm]
        ti = findfirst(==(s.time_dim), ov.dims)
        # place the spatial slice into buffer[..., slot along time, ...]
        idx = ntuple(d -> d == ti ? (slot:slot) : Colon(), length(ov.dims))
        dst = @view h.buffers[nm][idx...]
        # `a` has the var's dims minus the time axis, in order
        @inbounds dropdims(dst; dims = ti) .= a
    end

    h.n_in_shard += 1
    h.last_t = Float64(t)
    if h.n_in_shard == h.shard_time
        _flush_shard!(h)
    end
    return nothing
end

# --- flush ------------------------------------------------------------------

function _flush_shard!(h::ZarrWriteHandle)
    n = h.n_in_shard
    n == 0 && return nothing
    s = h.schema
    base = h.base
    tbase = h.shard_time_index * h.shard_time    # global time offset of this slab

    # 1) write the covering shard objects (durable BEFORE any shape bump)
    #    time coord (1-D)
    tvalid = tbase + n
    _write_array_shards!(base, s.time_dim, [s.time_dim], s.time_dtype,
                         h.time_buffer, s, h.codec, [tvalid], [tbase], 1,
                         h.shard_time_index)
    #    streaming vars
    for (nm, ov) in s.vars
        ti = findfirst(==(s.time_dim), ov.dims)
        valid = Int[dn == s.time_dim ? tvalid : h.dimlens[dn] for dn in ov.dims]
        tb = Int[i == ti ? tbase : 0 for i in 1:length(ov.dims)]
        _write_array_shards!(base, nm, ov.dims, ov.dtype, h.buffers[nm], s, h.codec,
                             valid, tb, ti, h.shard_time_index)
    end

    # 2) record the committed time-shard + advance durable counters
    push!(h.time_shards, TimeShardRecord(h.shard_time_index, h.shard_t_start,
                                         h.last_t === nothing ? h.shard_t_start : h.last_t, n))
    h.total_records += n
    h.shard_time_index += 1
    h.n_in_shard = 0

    # 3) bump each growable array's shape[time] (atomic, guarded)
    _update_shapes!(h)
    # 4) refresh the output manifest (crash-recovery record)
    _write_output_manifest!(h)

    _alloc_buffers!(h)
    return nothing
end

function _update_shapes!(h::ZarrWriteHandle)
    s = h.schema
    nt = h.total_records
    with_output_lock(h.base, s.time_dim) do
        _write_json(h.base, "$(s.time_dim)/zarr.json",
                    _array_meta_dict([s.time_dim], s.time_dtype, [nt], s, h.codec,
                                     _time_attrs(s)))
    end
    for (nm, ov) in s.vars
        shape = _init_var_shape(ov, s, nt)
        with_output_lock(h.base, nm) do
            _write_json(h.base, "$nm/zarr.json",
                        _array_meta_dict(ov.dims, ov.dtype, shape, s, h.codec,
                                         Dict{String,Any}()))
        end
    end
    return nothing
end

_time_attrs(s::OutputSchema) = something(
    findfirst_coord_attrs(s, s.time_dim), Dict{String,Any}())

function findfirst_coord_attrs(s::OutputSchema, name::AbstractString)
    for (nm, (_, attrs)) in s.coords
        nm == name && return attrs
    end
    return nothing
end

# --- output manifest --------------------------------------------------------

function _write_output_manifest!(h::ZarrWriteHandle)
    s = h.schema
    codec = Dict{String,Any}("id" => "blosc", "cname" => h.codec.cname,
                             "clevel" => h.codec.clevel,
                             "shuffle" => h.codec.shuffle ? "shuffle" : "noshuffle")
    vars = Vector{Dict{String,Any}}()
    for (nm, ov) in s.vars
        push!(vars, Dict{String,Any}("name" => nm, "dims" => ov.dims,
                                     "dtype" => _v3_dtype(ov.dtype)))
    end
    m = OutputManifest(
        h.base, "zarr", 3, String(s.profile), codec, s.time_dim,
        s.dims, vars, s.chunk_shape, s.shard_shape,
        copy(h.time_shards), h.last_t, h.total_records, rfc3339_utc())
    with_output_lock(h.base, "__manifest__") do
        write_output_manifest(joinpath(h.base, "output_manifest.json"), m)
    end
    return m
end

# --- write_close! -----------------------------------------------------------

function write_close!(::ZarrWriter, h::ZarrWriteHandle)
    h.n_in_shard > 0 && _flush_shard!(h)        # partial trailing shard
    # consolidated metadata is per-array `zarr.json` (already durable + up to
    # date); rewrite the group node + the final output manifest as the close record.
    with_output_lock(h.base, "__group__") do
        _write_json(h.base, "zarr.json", _group_meta_dict(h.schema))
    end
    return _write_output_manifest!(h)
end
