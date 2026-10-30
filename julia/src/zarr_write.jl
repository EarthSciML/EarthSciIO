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
#     slice; a full time-shard is written as ONE atomically-committed shard object,
#     and a commit (`write_flush!` for a `:single` writer) rewrites each array's
#     `zarr.json` `shape[time]` and then the output manifest.
#
# This file holds the format pieces (profiles, the schema, `zarr.json` dicts, the
# inner-chunk encode + shard assembly). The streaming state machine — roles,
# regions, the commit protocol, indexed records, statics, blobs, read-back and
# resume (spec/writer.md) — lives in zarr_stream.jl.
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

# The inner (per-chunk) compressor a profile selects. Blosc(zstd) for the
# diagnostic/checkpoint profiles; the PLAIN v3 `zstd` codec (no Blosc) for the
# `wasm` profile, so a WebAssembly/browser Zarr reader can decode the store
# (`blosc-src`'s C sources don't target wasm32; the standard v3 zstd codec does).
abstract type CodecProfile end

"""A pinned Blosc codec profile: `cname`/`clevel`/`shuffle` (byte-shuffle)."""
struct BloscProfile <: CodecProfile
    cname::String
    clevel::Int
    shuffle::Bool
end

"""A pinned **standard Zarr v3 `zstd`** codec profile: `level`/`checksum`. Unlike
[`BloscProfile`] this is the plain v3 `zstd` codec, NOT the Blosc container, so a
wasm/browser Zarr reader can decode the store. Lossless; no byte-shuffle filter."""
struct ZstdProfile <: CodecProfile
    level::Int
    checksum::Bool
end

"""Diagnostic profile — Blosc **zstd** + byte-shuffle, moderate level (5)."""
const BLOSC_DIAGNOSTIC = BloscProfile("zstd", 5, true)
"""Checkpoint profile — **lossless** Blosc zstd (level 7) + byte-shuffle."""
const BLOSC_CHECKPOINT = BloscProfile("zstd", 7, true)
"""WASM profile — plain v3 **zstd** (level 5), no Blosc, so a wasm/browser Zarr
reader can decode the store. See [`ZstdProfile`]."""
const ZSTD_WASM = ZstdProfile(5, false)

_profile(sym::Symbol) =
    sym === :diagnostic ? BLOSC_DIAGNOSTIC :
    sym === :checkpoint ? BLOSC_CHECKPOINT :
    sym === :wasm ? ZSTD_WASM :
    error("unknown codec profile $sym (expected :diagnostic, :checkpoint, or :wasm)")

# --- the output schema ------------------------------------------------------

"""
    OutputVar(dims, dtype; attrs = Dict())

One output variable: its on-disk dim names (file order; a streaming variable's
dims MUST include the schema's `time_dim`, a static's MUST NOT), its element type
(`Float64`/`Float32`/`Int32`/`Int64`/`UInt8`, plus `UInt32`/`UInt64`), and
optional CF variable attributes `attrs` (e.g. `"units"`, `"standard_name"`, and
the `"coordinates"` attribute naming this variable's auxiliary coordinates).
The attrs are written verbatim into the array node's `attributes` alongside the
mechanical `_ARRAY_DIMENSIONS`/`dimension_names`."""
struct OutputVar
    dims::Vector{String}
    dtype::DataType
    attrs::Dict{String,Any}
end
OutputVar(dims::AbstractVector, dtype::DataType; attrs::AbstractDict = Dict{String,Any}()) =
    OutputVar(String.(collect(dims)), dtype, Dict{String,Any}(attrs))

"""
    IndexedTime(times)

The `indexed` time mode (spec/writer.md §6.2): the time axis has a fixed length
`n_records = length(times)` from [`write_open!`], the owner writes `times` as the
time coordinate at open, and records are written by index with
[`write_record_at!`], in any order. The default time mode is `:append`."""
struct IndexedTime
    times::Vector{Float64}
end
IndexedTime(times::AbstractVector) = IndexedTime(Float64[Float64(t) for t in times])

"""
    OutputSchema(; dims, time_dim, vars, chunk_shape, shard_shape,
                   coords=[], statics=[], time_mode=:append,
                   profile=:diagnostic, attrs=Dict(), time_dtype=Float64)

The input to [`write_open!`] — a plain struct in this repo (the EarthSciAST
`OutputSchema` maps onto it at the binding layer; no EarthSciAST dependency here).
It is the spec/writer.md §3 *stream schema*.

Fields (all load-bearing for the binding to construct):
  * `dims::Vector{Pair{String,Int}}` — ORDERED dim name => length. In `:append`
    mode the `time_dim` entry's length is a placeholder (0 is conventional); the
    time axis grows. In indexed mode it is `n_records` (filled in when 0).
  * `time_dim::String` — the record axis name.
  * `coords::Vector{Pair{String,Tuple{Vector,Dict{String,Any}}}}` — ORDERED static
    coordinate arrays: name => (values, attrs). 1-D over their own dim; written
    once at `write_open!`. An entry for `time_dim` supplies the time coordinate's
    attrs (its VALUES are ignored — they come from the `t` of each record).
  * `vars::Vector{Pair{String,OutputVar}}` — ORDERED streaming variables.
  * `chunk_shape::Dict{String,Int}` — dim name => INNER chunk length.
  * `shard_shape::Dict{String,Int}` — dim name => SHARD length (must be a multiple
    of the inner chunk length along every dim). `shard_shape[time_dim]` is the
    number of records packed per shard object.
  * `profile::Symbol` — `:diagnostic`/`:checkpoint` (Blosc-zstd inner codec) or
    `:wasm` (plain v3 zstd inner codec, no Blosc — wasm/browser-loadable).
  * `attrs::Dict{String,Any}` — group-level attributes.
  * `time_dtype::DataType` — element type of the time coordinate (default `Float64`).
  * `statics::Vector{Pair{String,OutputVar}}` — ORDERED time-less variables
    (gradient fields, …) written once each with [`write_static!`].
  * `time_mode` — `:append` (default) or an [`IndexedTime`]."""
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
    statics::Vector{Pair{String,OutputVar}}
    time_mode::Union{Symbol,IndexedTime}
end

function OutputSchema(; dims, time_dim, vars, chunk_shape, shard_shape,
                      coords = Pair{String,Tuple{Vector,Dict{String,Any}}}[],
                      profile::Symbol = :diagnostic,
                      attrs::AbstractDict = Dict{String,Any}(),
                      time_dtype::DataType = Float64,
                      statics = Pair{String,OutputVar}[],
                      time_mode::Union{Symbol,IndexedTime} = :append)
    time_mode isa Symbol && time_mode !== :append && error(
        "unknown time_mode $time_mode (expected :append or IndexedTime(times))")
    td = String(time_dim)
    d = Pair{String,Int}[String(k) => Int(v) for (k, v) in dims]
    if time_mode isa IndexedTime
        # the time dim's length IS n_records in indexed mode (filled in when 0)
        n = length(time_mode.times)
        for (k, (nm, len)) in enumerate(d)
            nm == td || continue
            len == 0 || len == n || error(
                "indexed time_mode has $n records but dims[$td] = $len")
            d[k] = nm => n
        end
    end
    co = Pair{String,Tuple{Vector,Dict{String,Any}}}[
        String(k) => (collect(vals), Dict{String,Any}(a)) for (k, (vals, a)) in coords]
    vs = Pair{String,OutputVar}[String(k) => v for (k, v) in vars]
    st = Pair{String,OutputVar}[String(k) => v for (k, v) in statics]
    cs = Dict{String,Int}(String(k) => Int(v) for (k, v) in chunk_shape)
    ss = Dict{String,Int}(String(k) => Int(v) for (k, v) in shard_shape)
    return OutputSchema(d, td, co, vs, cs, ss, profile,
                        Dict{String,Any}(attrs), time_dtype, st, time_mode)
end

"""The `zarr` streaming writer (sharded Zarr v3). Registered `:active` in
[`WRITER_REGISTRY`]; the write mirror of [`ZarrReader`]. Its handle and the
streaming contract live in zarr_stream.jl."""
struct ZarrWriter <: Writer end

# --- dtype <-> Zarr v3 data_type --------------------------------------------

_v3_dtype(::Type{Float64}) = "float64"
_v3_dtype(::Type{Float32}) = "float32"
_v3_dtype(::Type{Int32})   = "int32"
_v3_dtype(::Type{Int64})   = "int64"
_v3_dtype(::Type{UInt32})  = "uint32"
_v3_dtype(::Type{UInt64})  = "uint64"
_v3_dtype(::Type{UInt8})   = "uint8"
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

# The write mirror for the `wasm` (plain v3 zstd) profile: the `zstd` encode lives
# in the `CodecZstd` weakdep extension (`using CodecZstd`). A base install without
# it errors here with an install hint, exactly like `_blosc_compress`.
_zstd_compress(bytes, level) = error(
    "the zarr writer needs the CodecZstd backend for the wasm (plain zstd) " *
    "profile: add `using CodecZstd` so the EarthSciIOZstdExt extension supplies " *
    "the encode (kept a weakdep to keep a base EarthSciIO install light).")

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

# The inner (per-chunk) compressor codec dict a profile selects: Blosc(zstd) for
# the diagnostic/checkpoint profiles, or the plain v3 `zstd` codec for the wasm
# profile (no Blosc — a wasm/browser reader can decode it).
_inner_codec_meta(codec::BloscProfile, typesize::Int) =
    Dict{String,Any}("name" => "blosc",
                     "configuration" => Dict{String,Any}(
                         "cname" => codec.cname,
                         "clevel" => codec.clevel,
                         "shuffle" => codec.shuffle ? "shuffle" : "noshuffle",
                         "typesize" => typesize,
                         "blocksize" => 0))
_inner_codec_meta(codec::ZstdProfile, ::Int) =
    Dict{String,Any}("name" => "zstd",
                     "configuration" => Dict{String,Any}(
                         "level" => codec.level,
                         "checksum" => codec.checksum))

function _sharding_codec(inner::Vector{Int}, codec::CodecProfile, typesize::Int)
    return Dict{String,Any}(
        "name" => "sharding_indexed",
        "configuration" => Dict{String,Any}(
            "chunk_shape" => inner,
            "codecs" => Any[
                Dict{String,Any}("name" => "bytes",
                                 "configuration" => Dict{String,Any}("endian" => "little")),
                _inner_codec_meta(codec, typesize),
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
                          schema::OutputSchema, codec::CodecProfile,
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

# Metadata writes of a `:single` writer run under the per-object advisory lock
# (the v1 behaviour); a multi-writer store takes none — its owner is the only
# metadata writer (spec/writer.md §9).
_maybe_locked(f, lock::Bool, base, name) = lock ? with_output_lock(f, base, name) : f()

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

# The compress step of the inner pipeline, dispatched on the profile: Blosc for
# the diagnostic/checkpoint profiles, plain v3 zstd for the wasm profile.
_compress(codec::BloscProfile, bytes, typesize) =
    _blosc_compress(bytes, codec.cname, codec.clevel, codec.shuffle, typesize)
_compress(codec::ZstdProfile, bytes, ::Int) = _zstd_compress(bytes, codec.level)

# Inner chunk array -> compressed bytes: C-order flatten -> little-endian bytes
# -> compressor (the `[bytes(little), <compressor>]` inner pipeline).
function _encode_chunk(chunk::AbstractArray{T,N}, codec::CodecProfile) where {T,N}
    flat = N == 1 ? vec(chunk) : vec(permutedims(chunk, reverse(1:N)))
    le = htol.(flat)
    bytes = Vector{UInt8}(reinterpret(UInt8, le))
    return _compress(codec, bytes, sizeof(T))
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
                              codec::CodecProfile, valid::Vector{Int},
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

function _write_static_array!(base, name, values::AbstractVector, attrs, schema, codec;
                              lock::Bool = true)
    dims = [name]
    T = eltype(values)
    Tout = T <: Integer ? (sizeof(T) <= 4 ? Int32 : Int64) :
           (T <: AbstractFloat ? (T == Float32 ? Float32 : Float64) : Float64)
    data = collect(Tout, values)
    len = length(data)
    haskey(schema.chunk_shape, name) ||
        error("static coord '$name' has no chunk_shape entry")
    shape = [len]
    _maybe_locked(lock, base, name) do
        _write_json(base, "$name/zarr.json",
                    _array_meta_dict(dims, Tout, shape, schema, codec, Dict{String,Any}(attrs)))
    end
    _write_array_shards!(base, name, dims, Tout, data, schema, codec,
                         [len], [0], 0, nothing)
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

# The manifest's `codec` record, dispatched on the profile (mirrors the Python /
# Rust writers' manifest codec dicts).
_codec_manifest(c::BloscProfile) =
    Dict{String,Any}("id" => "blosc", "cname" => c.cname, "clevel" => c.clevel,
                     "shuffle" => c.shuffle ? "shuffle" : "noshuffle")
_codec_manifest(c::ZstdProfile) =
    Dict{String,Any}("id" => "zstd", "level" => c.level, "checksum" => c.checksum)
