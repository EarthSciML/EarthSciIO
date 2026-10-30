# The streaming writer contract (spec/writer.md, RFC U12) — the handle behind
# `ZarrWriter`, and the read-back API.
#
# One logical store, `n_writers` writers. Each writer owns a REGION (a block of
# whole shards over some non-time dims) and writes only the shard objects of that
# region, whole and padded with fill, from its own buffers — no read-modify-write
# of a shared shard, no locks. Writers hand back a `CommitToken` from
# `write_flush!`; the OWNER (writer 0) verifies one token per writer (spec §5.3)
# and commits: grows `shape[time]` and writes `output_manifest.json` LAST. The
# manifest write is the commit point — readers trust only the manifest and the
# `zarr.json` shapes, so objects outside the committed state are invisible and
# are overwritten later. A `:single` writer is the owner of a one-writer store
# that commits on every `write_flush!`; when the store is also append-mode and
# static-free it keeps writing the v1 manifest (spec §8.1), byte-for-byte as
# before, under the same metadata locks.
#
# EarthSciIO stays MPI-free: tokens are plain values (`token_json`/`parse_token`
# are the documented JSON form) and the caller moves them.

# --- errors -----------------------------------------------------------------

"""
    RegionNotShardAligned(msg)

A writer's region (spec/writer.md §4) does not start on a shard boundary, does
not stop on one (or at the array end), or is out of bounds. Raised by
[`write_open!`]/[`write_resume`] before any I/O."""
struct RegionNotShardAligned <: Exception
    msg::String
end
Base.showerror(io::IO, e::RegionNotShardAligned) = print(io, "RegionNotShardAligned: ", e.msg)

"""
    CommitError(msg)

The tokens handed to a commit do not describe a consistent step (spec/writer.md
§5.3): a missing or duplicate writer, a divergent fingerprint, a gap or overlap
between regions, divergent records, or a rewrite of a committed record/static.
Nothing is written; the committed state is unchanged."""
struct CommitError <: Exception
    msg::String
end
Base.showerror(io::IO, e::CommitError) = print(io, "CommitError: ", e.msg)

# --- regions (spec §4) ------------------------------------------------------

# A region in canonical form: `dim => (start, stop)`, 0-based half-open, sorted by
# dim name. Empty = the whole extent.
const _Region = Vector{Pair{String,Tuple{Int,Int}}}

function _parse_region(region)::_Region
    region === nothing && return _Region()
    it = region isa NamedTuple ? pairs(region) : region
    out = _Region()
    for (k, v) in it
        length(v) == 2 || throw(ArgumentError(
            "region entry $k => $v: expected (start, stop), 0-based half-open"))
        push!(out, String(k) => (Int(v[1]), Int(v[2])))
    end
    sort!(out; by = first)
    allunique(first.(out)) || throw(ArgumentError("region names a dim twice: $region"))
    return out
end

# `nothing`, or `(kind, message)` with kind `:align` (RegionNotShardAligned) or
# `:arg` (a malformed region).
function _region_problem(r::_Region, s::OutputSchema)
    lens = Dict(s.dims)
    for (d, (a, b)) in r
        d == s.time_dim && return (:arg, "a region never names the time dim '$d'")
        haskey(lens, d) || return (:arg, "region names unknown dim '$d'")
        n, sh = lens[d], s.shard_shape[d]
        0 <= a < b <= n ||
            return (:align, "region $d=[$a, $b) is empty or outside [0, $n)")
        a % sh == 0 ||
            return (:align, "region $d=[$a, $b): start $a is not a multiple of the shard length $sh")
        (b % sh == 0 || b == n) || return (:align,
            "region $d=[$a, $b): stop $b is neither a multiple of the shard length $sh nor the dim length $n")
    end
    return nothing
end

function _check_region(region, s::OutputSchema)::_Region
    r = _parse_region(region)
    p = _region_problem(r, s)
    p === nothing && return r
    p[1] === :align ? throw(RegionNotShardAligned(p[2])) : throw(ArgumentError(p[2]))
end

_region_wire(r::_Region) = Pair{String,Vector{Int}}[d => Int[a, b] for (d, (a, b)) in r]

# --- canonical JSON (spec §3.1) + the token wire form -----------------------
#
# A minimal JSON emitter (`_stream_json`): a `Vector{<:Pair}` is an object in its own order (the
# token's documented key order), an `AbstractDict` an object with sorted keys.
# With only sorted dicts, ints and strings this IS `canonical_json`: Python's
# `json.dumps(F, sort_keys=True, separators=(",", ":"), ensure_ascii=True)`.

function _jstring(io::IO, s::AbstractString)
    print(io, '"')
    for c in s
        if c == '"'
            print(io, "\\\"")
        elseif c == '\\'
            print(io, "\\\\")
        elseif c == '\n'
            print(io, "\\n")
        elseif c == '\r'
            print(io, "\\r")
        elseif c == '\t'
            print(io, "\\t")
        elseif c == '\b'
            print(io, "\\b")
        elseif c == '\f'
            print(io, "\\f")
        elseif c < ' ' || c > '~'
            u = UInt32(c)
            if u > 0xffff               # astral: a UTF-16 surrogate pair, like Python
                u -= 0x10000
                print(io, "\\u", string(0xd800 + (u >> 10); base = 16, pad = 4),
                      "\\u", string(0xdc00 + (u & 0x3ff); base = 16, pad = 4))
            else
                print(io, "\\u", string(u; base = 16, pad = 4))
            end
        else
            print(io, c)
        end
    end
    print(io, '"')
end

_jwrite(io::IO, ::Nothing) = print(io, "null")
_jwrite(io::IO, x::Bool) = print(io, x ? "true" : "false")
_jwrite(io::IO, x::Integer) = print(io, string(x))
function _jwrite(io::IO, x::AbstractFloat)
    isfinite(x) || error("JSON cannot carry the non-finite number $x")
    print(io, string(Float64(x)))       # shortest round-trip form
end
_jwrite(io::IO, x::AbstractString) = _jstring(io, x)
_jwrite(io::IO, x::Symbol) = _jstring(io, String(x))
function _jwrite(io::IO, x::AbstractVector{<:Pair})
    print(io, '{')
    for (k, (key, v)) in enumerate(x)
        k > 1 && print(io, ',')
        _jstring(io, String(key)); print(io, ':'); _jwrite(io, v)
    end
    print(io, '}')
end
_jwrite(io::IO, x::AbstractDict) =
    _jwrite(io, [String(k) => x[k] for k in sort!([String(k) for k in keys(x)])])
function _jwrite(io::IO, x::Union{AbstractVector,Tuple})
    print(io, '[')
    for (k, v) in enumerate(x)
        k > 1 && print(io, ',')
        _jwrite(io, v)
    end
    print(io, ']')
end

_stream_json(x) = sprint(_jwrite, x)

"""
    schema_fingerprint(schema::OutputSchema) -> "sha256:<hex>"

The stream-schema fingerprint of spec/writer.md §3.1: the sha256 of the canonical
JSON of dims (time length `0` in append mode, `n_records` in indexed mode),
`time_dim`, `chunk_shape`, `shard_shape`, `profile`, `time_mode`, and the
`[name, dims, dtype]` of every streaming variable and static, in schema order.
Identical across the Julia / Python / Rust bindings."""
function schema_fingerprint(s::OutputSchema)
    idx = s.time_mode isa IndexedTime
    ntime = idx ? length(s.time_mode.times) : 0
    F = Dict{String,Any}(
        "chunk_shape" => s.chunk_shape,
        "dims"        => Any[Any[d, d == s.time_dim ? ntime : n] for (d, n) in s.dims],
        "profile"     => String(s.profile),
        "shard_shape" => s.shard_shape,
        "statics"     => Any[Any[nm, ov.dims, _v3_dtype(ov.dtype)] for (nm, ov) in s.statics],
        "time_dim"    => s.time_dim,
        "time_mode"   => idx ? "indexed" : "append",
        "vars"        => Any[Any[nm, ov.dims, _v3_dtype(ov.dtype)] for (nm, ov) in s.vars],
    )
    return "sha256:" * bytes2hex(sha256(_stream_json(F)))
end

const COMMIT_TOKEN_SCHEMA = "earthsciio/commit-token/v1"

"""
    CommitToken

What one writer wrote since its previous token (spec/writer.md §5.2), returned by
[`write_flush!`] and handed (by the caller — EarthSciIO is MPI-free) to the
owner's [`write_commit!`]/[`write_close!`]. Fields: `writer` id, its `region`
(`dim => (start, stop)`), the schema `fingerprint`, `records` (`(index, t)`,
ascending), `statics` (names, sorted), `blobs` (`(scope, record, key)`, sorted;
`record` is `nothing` for scope `"static"`) and the advisory `objects` (store
keys PUT, sorted). [`token_json`]/[`parse_token`] are its documented JSON form."""
struct CommitToken
    writer::Int
    region::_Region
    fingerprint::String
    records::Vector{Tuple{Int,Float64}}
    statics::Vector{String}
    blobs::Vector{Tuple{String,Union{Int,Nothing},String}}
    objects::Vector{String}
end

Base.:(==)(a::CommitToken, b::CommitToken) =
    a.writer == b.writer && a.region == b.region && a.fingerprint == b.fingerprint &&
    length(a.records) == length(b.records) &&
    all(x[1] == y[1] && _same_bits(x[2], y[2]) for (x, y) in zip(a.records, b.records)) &&
    a.statics == b.statics && isequal(a.blobs, b.blobs) && a.objects == b.objects

"""True when the token carries no record, static or blob (its `objects` aside)."""
Base.isempty(t::CommitToken) = isempty(t.records) && isempty(t.statics) && isempty(t.blobs)

_same_bits(a::Float64, b::Float64) = reinterpret(UInt64, a) == reinterpret(UInt64, b)

"""
    token_json(tok::CommitToken) -> String

The token's JSON (spec/writer.md §5.2), keys in the documented order, no
whitespace, `t` in shortest round-trip form."""
function token_json(t::CommitToken)
    return _stream_json(Pair{String,Any}[
        "schema"      => COMMIT_TOKEN_SCHEMA,
        "writer"      => t.writer,
        "region"      => _region_wire(t.region),
        "fingerprint" => t.fingerprint,
        "records"     => [Pair{String,Any}["index" => i, "t" => tt] for (i, tt) in t.records],
        "statics"     => t.statics,
        "blobs"       => [Pair{String,Any}["scope" => s, "record" => r, "key" => k]
                          for (s, r, k) in t.blobs],
        "objects"     => t.objects,
    ])
end

"""
    parse_token(json) -> CommitToken

Parse a token's JSON (a `String`, or an already-parsed object) — the inverse of
[`token_json`]. Errors on any `schema` label other than
`earthsciio/commit-token/v1`."""
parse_token(s::AbstractString) = parse_token(JSON.parse(s))
function parse_token(d::AbstractDict)
    get(d, "schema", nothing) == COMMIT_TOKEN_SCHEMA ||
        error("not a commit token: schema = $(repr(get(d, "schema", nothing)))")
    return CommitToken(
        Int(d["writer"]),
        _parse_region(d["region"]),
        String(d["fingerprint"]),
        Tuple{Int,Float64}[(Int(e["index"]), Float64(e["t"])) for e in d["records"]],
        String[String(x) for x in d["statics"]],
        Tuple{String,Union{Int,Nothing},String}[
            (String(b["scope"]), b["record"] === nothing ? nothing : Int(b["record"]),
             String(b["key"])) for b in d["blobs"]],
        String[String(x) for x in get(d, "objects", Any[])],
    )
end

# --- the handle -------------------------------------------------------------

# One open time shard's buffers: the time values, each supplied variable's
# region-shaped array (time axis = the shard's length), and which slots hold data.
mutable struct _ShardBuf
    tvals::Vector{Float64}
    data::Dict{String,Array}
    filled::Vector{Bool}
    dirty::Bool
end

# A blob's identity: (scope, record (-1 for "static"), writer, key).
const _BlobId = Tuple{String,Int,Int,String}

"""Opaque write handle threaded through the `write_*` calls of [`ZarrWriter`]."""
mutable struct ZarrWriteHandle
    base::String                       # output directory (local FS)
    schema::OutputSchema
    codec::CodecProfile
    role::Symbol                       # :single / :owner / :contributor
    n_writers::Int
    id::Int
    region::_Region
    lo::Dict{String,Int}               # non-time dim => region start (0-based)
    hi::Dict{String,Int}               # non-time dim => region stop (exclusive)
    version::Int                       # manifest version (1 or 2), fixed for life
    fingerprint::String
    created_at::String
    # committed state: authoritative for :single/:owner; a contributor's VIEW is
    # the manifest it read plus what it has tokened since (spec §6.2)
    committed_t::Vector{Float64}       # append: t of committed records 0..n-1
    rec_committed::Vector{Bool}        # indexed: per record
    rec_generation::Vector{Int}
    static_committed::Dict{String,Bool}
    static_generation::Dict{String,Int}
    blobs::Vector{_BlobId}             # committed blobs, sorted
    writers::Vector{Pair{Int,_Region}} # as of the last commit
    commit_seq::Int
    # write state
    n_written::Int                     # append: the next record index
    last_t::Union{Float64,Nothing}     # append: the last written t
    shards::Dict{Int,_ShardBuf}        # open time shards by index
    pending_records::Dict{Int,Float64} # written since the last token
    tokened::Dict{Int,Float64}         # tokened, not (known to be) committed
    pending_statics::Set{String}
    pending_blobs::Set{Tuple{String,Int,String}}
    objects::Set{String}
    closed::Bool
end

_indexed(h::ZarrWriteHandle) = h.schema.time_mode isa IndexedTime
_times(h::ZarrWriteHandle) = (h.schema.time_mode::IndexedTime).times
_ntime(h::ZarrWriteHandle) = length(_times(h))
_st(h::ZarrWriteHandle) = h.schema.shard_shape[h.schema.time_dim]
_locked(h::ZarrWriteHandle) = h.role === :single
_region_dims(h::ZarrWriteHandle) = Set{String}(first.(h.region))
_partitioned(rd, dims) = any(d -> d in rd, dims)
# Does this writer supply a variable over `dims`? Owner/single: every variable;
# a contributor: only the partitioned ones.
_supplies(h::ZarrWriteHandle, dims) =
    h.role !== :contributor || _partitioned(_region_dims(h), dims)

# Does this writer PUT the shard objects of a variable over `dims`? A variable
# that lacks some region dim is supplied by every writer, but only the writer
# whose region starts at 0 on each region dim it lacks writes it, so every shard
# object keeps exactly one writer (spec §4, partly partitioned arrays).
_writes_objects(h::ZarrWriteHandle, dims) =
    _supplies(h, dims) && all(h.lo[d] == 0 for d in _region_dims(h) if !(d in dims))

function _is_committed(h::ZarrWriteHandle, i::Int)
    _indexed(h) && return 0 <= i < _ntime(h) && h.rec_committed[i + 1]
    return 0 <= i < length(h.committed_t)
end

function _new_handle(base, s::OutputSchema, role::Symbol, n::Int, id::Int, r::_Region,
                     version::Int, created_at::AbstractString)
    rd = Dict(r)
    lo, hi = Dict{String,Int}(), Dict{String,Int}()
    for (d, len) in s.dims
        d == s.time_dim && continue
        lo[d], hi[d] = get(rd, d, (0, len))
    end
    N = s.time_mode isa IndexedTime ? length(s.time_mode.times) : 0
    return ZarrWriteHandle(
        base, s, _profile(s.profile), role, n, id, r, lo, hi, version,
        schema_fingerprint(s), String(created_at),
        Float64[], fill(false, N), zeros(Int, N),
        Dict{String,Bool}(nm => false for (nm, _) in s.statics),
        Dict{String,Int}(nm => 0 for (nm, _) in s.statics),
        _BlobId[], Pair{Int,_Region}[0 => r], 0,
        0, nothing, Dict{Int,_ShardBuf}(), Dict{Int,Float64}(), Dict{Int,Float64}(),
        Set{String}(), Set{Tuple{String,Int,String}}(), Set{String}(), false)
end

function _check_schema(s::OutputSchema)
    any(d == s.time_dim for (d, _) in s.dims) ||
        error("time_dim '$(s.time_dim)' is not one of the schema's dims")
    for (d, _) in s.dims
        haskey(s.chunk_shape, d) || error("dim '$d' missing from chunk_shape")
        haskey(s.shard_shape, d) || error("dim '$d' missing from shard_shape")
        s.chunk_shape[d] > 0 && s.shard_shape[d] > 0 ||
            error("chunk/shard lengths of dim '$d' must be positive")
        s.shard_shape[d] % s.chunk_shape[d] == 0 ||
            error("shard_shape[$d]=$(s.shard_shape[d]) not a multiple of " *
                  "chunk_shape[$d]=$(s.chunk_shape[d])")
    end
    _profile(s.profile)
    for (nm, ov) in s.vars
        s.time_dim in ov.dims ||
            error("streaming var '$nm' must include the time dim '$(s.time_dim)'")
        _v3_dtype(ov.dtype)
    end
    for (nm, ov) in s.statics
        s.time_dim in ov.dims && error("static '$nm' must not include the time dim")
        isempty(ov.dims) && error("static '$nm' has no dims (0-d statics are unsupported)")
        _v3_dtype(ov.dtype)
    end
    allunique(vcat(first.(s.vars), first.(s.statics))) ||
        error("variable and static names must be unique")
    return nothing
end

_check_open(h::ZarrWriteHandle) = h.closed && error("the write handle is closed")

function _check_bound_region(h::ZarrWriteHandle, region)
    region === nothing && return nothing
    _parse_region(region) == h.region || throw(ArgumentError(
        "region $(region) differs from the region $(h.region) this handle was opened with"))
    return nothing
end

# --- encode: whole shards of a region ---------------------------------------

# Write every shard object of array `name` covering the box `[origin, vhi)`.
# `data` holds the box from `origin` (dims order, eltype `T`); `vhi` is the
# global end of the VALID data per dim (a shard past it is padded with fill, and
# an inner chunk wholly past it is left out of the shard). `origin` is
# shard-aligned on every dim, so each shard object is wholly owned by this writer.
function _put_region_shards!(h::ZarrWriteHandle, name::AbstractString, dims::Vector{String},
                             ::Type{T}, data::AbstractArray{T}, origin::Vector{Int},
                             vhi::Vector{Int}) where {T}
    s = h.schema
    N = length(dims)
    inner = ntuple(d -> s.chunk_shape[dims[d]], N)
    shard = ntuple(d -> s.shard_shape[dims[d]], N)
    ips = ntuple(d -> shard[d] ÷ inner[d], N)
    n_inner = prod(ips)
    fillv = _v3_fill(T)
    base = ntuple(d -> origin[d], N)
    valid = ntuple(d -> vhi[d], N)
    ranges = ntuple(d -> (origin[d] ÷ shard[d]):(cld(vhi[d], shard[d]) - 1), N)
    for sc in Iterators.product(ranges...)
        chunks = Vector{Union{Nothing,Vector{UInt8}}}(undef, n_inner)
        for ci in 0:(n_inner - 1)
            lc = _c_unravel(ci, ips)
            gstart = ntuple(d -> sc[d] * shard[d] + lc[d] * inner[d], N)
            ca = _chunk_from(data, inner, gstart, valid, base, fillv)
            chunks[ci + 1] = ca === nothing ? nothing : _encode_chunk(ca, h.codec)
        end
        key = string(name, "/c/", join((string(c) for c in sc), "/"))
        put_object!(h.base, key, _assemble_shard(chunks, n_inner))
        push!(h.objects, key)
    end
    return nothing
end

# --- decode: a box of an array, straight from the store ---------------------
#
# The read-back path (and a writer's reload of its region of a time shard) reads
# shard objects directly from the store directory — never through the
# content-addressed cache (spec §7) — and keeps the stored dtype.

const _V3_JL_DTYPE = Dict{String,DataType}(
    "float64" => Float64, "float32" => Float32, "int32" => Int32, "int64" => Int64,
    "uint8" => UInt8, "uint32" => UInt32, "uint64" => UInt64)

function _jl_dtype(s::AbstractString)
    haskey(_V3_JL_DTYPE, s) || error("unsupported zarr v3 data_type '$s'")
    return _V3_JL_DTYPE[s]
end

# (parsed v3 meta, element type) of array `name` from its `zarr.json`.
function _array_info(base::AbstractString, name::AbstractString)
    p = joinpath(base, name, "zarr.json")
    isfile(p) || error("no array '$name' in the store at $base")
    text = read(p, String)
    # (`String(::Vector{UInt8})` takes the vector, so parse from one `String`)
    return _parse_zarr_json(codeunits(text)), _jl_dtype(String(JSON.parse(text)["data_type"]))
end

# The box `[lo, hi)` (global, dims order) of array `name`; cells no stored inner
# chunk covers keep the fill value. Not bounded by `zarr.json`'s shape: a writer
# reloads shard objects that lie past the committed `shape[time]`.
function _read_region(base::AbstractString, name::AbstractString, m::ZV3Meta,
                      ::Type{T}, lo::Vector{Int}, hi::Vector{Int}) where {T}
    N = length(lo)
    out = fill(convert(T, m.fill_value), (hi .- lo)...)
    comp = m.blosc ? Dict{String,Any}("id" => "blosc") :
           m.zstd  ? Dict{String,Any}("id" => "zstd") : nothing
    ips = Tuple(m.ips)
    ranges = ntuple(d -> (lo[d] ÷ m.shard[d]):(cld(hi[d], m.shard[d]) - 1), N)
    for sc in Iterators.product(ranges...)
        key = "c" * m.sep * join((string(c) for c in sc), m.sep)
        path = joinpath(base, name, key)
        isfile(path) || continue                       # absent shard -> fill
        bytes = read(path)
        offsets, nb = _shard_index(m, bytes)
        for ci in 0:(prod(ips) - 1)
            lc = _c_unravel(ci, ips)
            gs = Int[sc[d] * m.shard[d] + lc[d] * m.inner[d] for d in 1:N]
            olo = max.(gs, lo)
            ohi = min.(gs .+ m.inner, hi)
            any(ohi .<= olo) && continue
            off, len = offsets[ci + 1], nb[ci + 1]
            (off == typemax(UInt64) || len == typemax(UInt64)) && continue
            raw = _decompress(comp, bytes[(Int(off) + 1):(Int(off) + Int(len))])
            flat = collect(reinterpret(T, Vector{UInt8}(raw)))
            flat = m.byteorder == '>' ? ntoh.(flat) : ltoh.(flat)
            chunk = N == 1 ? flat :
                    permutedims(reshape(flat, reverse(m.inner)...), N:-1:1)
            out[ntuple(d -> (olo[d] - lo[d] + 1):(ohi[d] - lo[d]), N)...] =
                @view chunk[ntuple(d -> (olo[d] - gs[d] + 1):(ohi[d] - gs[d]), N)...]
        end
    end
    return out
end

# The t values of committed records 0..n-1, read from the store's time coordinate.
function _read_committed_t(base::AbstractString, time_dim::AbstractString, n::Int)
    n == 0 && return Float64[]
    m, T = _array_info(base, time_dim)
    return Float64[Float64(x) for x in _read_region(base, time_dim, m, T, [0], [n])]
end

# --- time shard buffers -----------------------------------------------------

# Slots of time shard `k` this writer already knows to hold data in the store:
# committed (in its view), tokened, or pending.
function _known_slots(h::ZarrWriteHandle, k::Int)
    st = _st(h)
    lim = _indexed(h) ? _ntime(h) : typemax(Int)
    return [begin
        i = k * st + j - 1
        i < lim && (_is_committed(h, i) || haskey(h.tokened, i) ||
                    haskey(h.pending_records, i))
    end for j in 1:st]
end

# The buffer of time shard `k`: held, or allocated fill-initialized. When the
# writer no longer holds a shard it has data in (after a resume, or an indexed
# rewrite of a shard it already wrote whole), it reloads its region of that shard
# from the store first, so the next write rewrites the shard whole (spec §6.2).
function _shard_buf!(h::ZarrWriteHandle, k::Int)
    haskey(h.shards, k) && return h.shards[k]
    s = h.schema
    st = _st(h)
    tbase = k * st
    buf = _ShardBuf(zeros(Float64, st), Dict{String,Array}(), fill(false, st), false)
    for (nm, ov) in s.vars
        _supplies(h, ov.dims) || continue
        shp = Tuple(d == s.time_dim ? st : h.hi[d] - h.lo[d] for d in ov.dims)
        buf.data[nm] = fill(convert(ov.dtype, _v3_fill(ov.dtype)), shp...)
    end
    known = _known_slots(h, k)
    if any(known)
        for (nm, ov) in s.vars
            _supplies(h, ov.dims) || continue
            lo = Int[d == s.time_dim ? tbase : h.lo[d] for d in ov.dims]
            hi = Int[d == s.time_dim ? tbase + st : h.hi[d] for d in ov.dims]
            m, _ = _array_info(h.base, nm)
            got = _read_region(h.base, nm, m, ov.dtype, lo, hi)
            ti = findfirst(==(s.time_dim), ov.dims)
            for j in 1:st
                # only the slots with data: anything else in the stored shard is
                # an uncommitted leftover of a dropped handle, replaced by fill
                known[j] && (selectdim(buf.data[nm], ti, j) .= selectdim(got, ti, j))
            end
        end
        for j in 1:st
            known[j] || continue
            i = tbase + j - 1
            buf.tvals[j] = _indexed(h) ? _times(h)[i + 1] :
                           haskey(h.pending_records, i) ? h.pending_records[i] :
                           haskey(h.tokened, i) ? h.tokened[i] : h.committed_t[i + 1]
            buf.filled[j] = true
        end
    end
    h.shards[k] = buf
    return buf
end

# Records of time shard `k` to encode: in append mode the filled prefix, in
# indexed mode the shard's whole extent (unwritten slots are fill).
_shard_nvalid(h::ZarrWriteHandle, k::Int, buf::_ShardBuf) =
    _indexed(h) ? min(_st(h), _ntime(h) - k * _st(h)) : something(findlast(buf.filled), 0)

function _shard_complete(h::ZarrWriteHandle, k::Int, buf::_ShardBuf)
    n = _indexed(h) ? min(_st(h), _ntime(h) - k * _st(h)) : _st(h)
    return all(@view buf.filled[1:n])
end

# Encode + PUT this writer's region of time shard `k`, whole and fill-padded.
function _write_time_shard!(h::ZarrWriteHandle, k::Int, buf::_ShardBuf)
    s = h.schema
    tbase = k * _st(h)
    nvalid = _shard_nvalid(h, k, buf)
    if nvalid > 0
        for (nm, ov) in s.vars
            _writes_objects(h, ov.dims) || continue
            origin = Int[d == s.time_dim ? tbase : h.lo[d] for d in ov.dims]
            vhi = Int[d == s.time_dim ? tbase + nvalid : h.hi[d] for d in ov.dims]
            _put_region_shards!(h, nm, ov.dims, ov.dtype, buf.data[nm], origin, vhi)
        end
        # the time coordinate is replicated: owner/single only, and in indexed
        # mode it was written whole at open
        if h.role !== :contributor && !_indexed(h)
            tv = Vector{s.time_dtype}(convert.(s.time_dtype, buf.tvals))
            _put_region_shards!(h, s.time_dim, [s.time_dim], s.time_dtype, tv,
                                [tbase], [tbase + nvalid])
        end
    end
    buf.dirty = false
    return nothing
end

function _check_record_arrays(h::ZarrWriteHandle, arrays)
    s = h.schema
    for (nm, ov) in s.vars
        if _supplies(h, ov.dims)
            haskey(arrays, nm) || error("write_record!: missing array for var '$nm'")
        elseif haskey(arrays, nm)
            error("contributor $(h.id) supplied the replicated variable '$nm' " *
                  "(only the owner writes variables without a region dim)")
        end
    end
    if h.role !== :single
        names = Set(first.(s.vars))
        for k in keys(arrays)
            String(k) in names || error("write_record!: '$k' is not a streaming variable")
        end
    end
    return nothing
end

function _put_record!(h::ZarrWriteHandle, i::Int, t::Float64, arrays)
    s = h.schema
    _check_record_arrays(h, arrays)
    st = _st(h)
    k, slot = i ÷ st, i % st + 1
    # validate every shape before touching a buffer
    for (nm, ov) in s.vars
        _supplies(h, ov.dims) || continue
        want = Tuple(h.hi[d] - h.lo[d] for d in ov.dims if d != s.time_dim)
        size(arrays[nm]) == want || error(
            "array for '$nm' has shape $(size(arrays[nm])); expected $want " *
            "(the variable's non-time dims $(filter(!=(s.time_dim), ov.dims)), region-sized)")
    end
    buf = _shard_buf!(h, k)
    for (nm, ov) in s.vars
        _supplies(h, ov.dims) || continue
        ti = findfirst(==(s.time_dim), ov.dims)
        selectdim(buf.data[nm], ti, slot) .= arrays[nm]
    end
    buf.tvals[slot] = t
    buf.filled[slot] = true
    buf.dirty = true
    h.pending_records[i] = t
    # a fully-covered time shard is written as soon as it fills (spec §5.4)
    if _shard_complete(h, k, buf)
        _write_time_shard!(h, k, buf)
        delete!(h.shards, k)
    end
    return nothing
end

# --- metadata + manifest ----------------------------------------------------

_var_shape(h::ZarrWriteHandle, ov::OutputVar, nt::Int) =
    Int[d == h.schema.time_dim ? nt : Dict(h.schema.dims)[d] for d in ov.dims]

_ntime_shape(h::ZarrWriteHandle) = _indexed(h) ? _ntime(h) : length(h.committed_t)

# The owner's create: group, coordinates, the time coordinate (written whole in
# indexed mode), and every variable's and static's `zarr.json`.
function _create_metadata!(h::ZarrWriteHandle)
    s, base, lk = h.schema, h.base, _locked(h)
    _maybe_locked(lk, base, "__group__") do
        _write_json(base, "zarr.json", _group_meta_dict(s))
    end
    for (nm, (vals, attrs)) in s.coords
        nm == s.time_dim && continue                    # attrs only; values from t
        _write_static_array!(base, nm, vals, attrs, s, h.codec; lock = lk)
    end
    nt = _ntime_shape(h)
    _maybe_locked(lk, base, s.time_dim) do
        _write_json(base, "$(s.time_dim)/zarr.json",
                    _array_meta_dict([s.time_dim], s.time_dtype, [nt], s, h.codec,
                                     _time_attrs(s)))
    end
    if _indexed(h) && nt > 0
        tv = Vector{s.time_dtype}(convert.(s.time_dtype, _times(h)))
        _put_region_shards!(h, s.time_dim, [s.time_dim], s.time_dtype, tv, [0], [nt])
    end
    for (nm, ov) in s.vars
        _maybe_locked(lk, base, nm) do
            _write_json(base, "$nm/zarr.json",
                        _array_meta_dict(ov.dims, ov.dtype, _var_shape(h, ov, nt), s,
                                         h.codec, ov.attrs))
        end
    end
    for (nm, ov) in s.statics
        shape = Int[Dict(s.dims)[d] for d in ov.dims]
        _maybe_locked(lk, base, nm) do
            _write_json(base, "$nm/zarr.json",
                        _array_meta_dict(ov.dims, ov.dtype, shape, s, h.codec, ov.attrs))
        end
    end
    empty!(h.objects)        # shared objects are not part of the owner's first token
    return nothing
end

# Grow `shape[time]` of the time coordinate and every streaming variable to the
# committed count (append mode).
function _update_shapes!(h::ZarrWriteHandle)
    s, lk = h.schema, _locked(h)
    nt = length(h.committed_t)
    _maybe_locked(lk, h.base, s.time_dim) do
        _write_json(h.base, "$(s.time_dim)/zarr.json",
                    _array_meta_dict([s.time_dim], s.time_dtype, [nt], s, h.codec,
                                     _time_attrs(s)))
    end
    for (nm, ov) in s.vars
        _maybe_locked(lk, h.base, nm) do
            _write_json(h.base, "$nm/zarr.json",
                        _array_meta_dict(ov.dims, ov.dtype, _var_shape(h, ov, nt), s,
                                         h.codec, ov.attrs))
        end
    end
    return nothing
end

# (index, t) of every committed record, ascending.
_committed_pairs(h::ZarrWriteHandle) = _indexed(h) ?
    Tuple{Int,Float64}[(i - 1, _times(h)[i]) for i in 1:_ntime(h) if h.rec_committed[i]] :
    Tuple{Int,Float64}[(i - 1, t) for (i, t) in enumerate(h.committed_t)]

# One entry per time shard holding >=1 committed record (spec §8.2).
function _time_shards(h::ZarrWriteHandle)
    st = _st(h)
    out = TimeShardRecord[]
    for (i, t) in _committed_pairs(h)
        k = i ÷ st
        if !isempty(out) && out[end].index == k
            e = out[end]
            out[end] = TimeShardRecord(k, min(e.t_start, t), max(e.t_end, t), e.n + 1)
        else
            push!(out, TimeShardRecord(k, t, t, 1))
        end
    end
    return out
end

function _last_t(h::ZarrWriteHandle)
    p = _committed_pairs(h)
    return isempty(p) ? nothing : maximum(last, p)
end

# Sorted disjoint half-open ranges `[[a, b), …]` of the sorted indices `idx`.
function _index_ranges(idx::Vector{Int})
    out = Vector{Vector{Int}}()
    for i in idx
        if !isempty(out) && out[end][2] == i
            out[end][2] = i + 1
        else
            push!(out, [i, i + 1])
        end
    end
    return out
end

function _with_attrs(d::Dict{String,Any}, attrs::Dict{String,Any})
    isempty(attrs) || (d["attrs"] = attrs)
    return d
end

# The manifest this handle's committed state describes: an `OutputManifest` (v1,
# exactly the fields Julia wrote before spec/writer.md) or the v2 object.
function _manifest_obj(h::ZarrWriteHandle)
    s = h.schema
    if h.version == 1
        vars = Dict{String,Any}[Dict{String,Any}("name" => nm, "dims" => ov.dims,
                                                 "dtype" => _v3_dtype(ov.dtype))
                                for (nm, ov) in s.vars]
        return OutputManifest(h.base, "zarr", 3, String(s.profile), _codec_manifest(h.codec),
                              s.time_dim, s.dims, vars, s.chunk_shape, s.shard_shape,
                              _time_shards(h), _last_t(h), length(h.committed_t),
                              rfc3339_utc())
    end
    nt = _ntime_shape(h)
    records = Dict{String,Any}(
        "committed" => _index_ranges(first.(_committed_pairs(h))))
    if _indexed(h)
        records["entries"] = [Dict{String,Any}("index" => i - 1, "t" => _times(h)[i],
                                               "committed" => h.rec_committed[i],
                                               "generation" => h.rec_generation[i])
                              for i in 1:_ntime(h)]
    end
    return Dict{String,Any}(
        "schema"      => OUTPUT_MANIFEST_SCHEMA_V2,
        "base_url"    => h.base,
        "format"      => "zarr",
        "zarr_format" => 3,
        "profile"     => String(s.profile),
        "codec"       => _codec_manifest(h.codec),
        "time_dim"    => s.time_dim,
        "dims"        => [Dict{String,Any}("name" => d, "length" => d == s.time_dim ? nt : n)
                          for (d, n) in s.dims],
        "vars"        => [_with_attrs(Dict{String,Any}("name" => nm, "dims" => ov.dims,
                                                       "dtype" => _v3_dtype(ov.dtype)), ov.attrs)
                          for (nm, ov) in s.vars],
        "chunk_shape" => s.chunk_shape,
        "shard_shape" => s.shard_shape,
        "time_shards" => [_timeshard_dict(x) for x in _time_shards(h)],
        "last_t"      => _last_t(h),
        "n_records"   => nt,
        "created_at"  => h.created_at,
        "schema_fingerprint" => h.fingerprint,
        "time_mode"   => _indexed(h) ?
                         Dict{String,Any}("kind" => "indexed", "n_records" => _ntime(h)) :
                         Dict{String,Any}("kind" => "append"),
        "writers"     => [Dict{String,Any}("id" => i, "region" => Dict{String,Any}(
                              d => [a, b] for (d, (a, b)) in r)) for (i, r) in h.writers],
        "records"     => records,
        "statics"     => [_with_attrs(Dict{String,Any}(
                              "name" => nm, "dims" => ov.dims, "dtype" => _v3_dtype(ov.dtype),
                              "committed" => h.static_committed[nm],
                              "generation" => h.static_generation[nm]), ov.attrs)
                          for (nm, ov) in s.statics],
        "blobs"       => [Dict{String,Any}("scope" => sc, "record" => r < 0 ? nothing : r,
                                           "writer" => w, "key" => k)
                          for (sc, r, w, k) in h.blobs],
        "commit_seq"  => h.commit_seq,
    )
end

# Write the manifest (the commit point) and return it.
function _write_manifest!(h::ZarrWriteHandle)
    m = _manifest_obj(h)
    path = joinpath(h.base, "output_manifest.json")
    _maybe_locked(_locked(h), h.base, "__manifest__") do
        if m isa OutputManifest
            write_output_manifest(path, m)
        else
            put_object!(h.base, "output_manifest.json",
                        Vector{UInt8}(codeunits(JSON.json(m; pretty = true))))
        end
    end
    return m
end

# --- rebuilding a handle from a store (join / resume) -----------------------

function _stored_attrs(base::AbstractString, name::AbstractString)
    p = joinpath(base, name, "zarr.json")
    isfile(p) || return Dict{String,Any}()
    a = get(JSON.parsefile(p), "attributes", nothing)
    a === nothing && return Dict{String,Any}()
    return Dict{String,Any}(String(k) => v for (k, v) in a if k != "_ARRAY_DIMENSIONS")
end

# The stream schema a manifest describes (spec §8.2). Attributes (variables, the
# time coordinate, the group) come from the stored `zarr.json` objects, so an
# owner that rewrites shapes after a resume keeps them. Static coordinate arrays
# are not rebuilt: they are written once, at create.
function _schema_from_manifest(m, base::AbstractString)
    if m isa OutputManifest
        dims = copy(m.dims)
        time_dim, profile = m.time_dim, Symbol(m.profile)
        vdefs = [(String(v["name"]), String.(v["dims"]), String(v["dtype"])) for v in m.vars]
        sdefs = Tuple{String,Vector{String},String}[]
        chunk, shard = m.chunk_shape, m.shard_shape
        time_mode = :append
    else
        time_dim = String(m["time_dim"])
        tm = m["time_mode"]
        time_mode = if String(tm["kind"]) == "indexed"
            ents = sort(collect(m["records"]["entries"]); by = e -> Int(e["index"]))
            IndexedTime(Float64[Float64(e["t"]) for e in ents])
        else
            :append
        end
        dims = Pair{String,Int}[String(e["name"]) =>
                                (String(e["name"]) == time_dim && time_mode === :append ? 0 :
                                 Int(e["length"])) for e in m["dims"]]
        profile = Symbol(m["profile"])
        vdefs = [(String(v["name"]), String.(v["dims"]), String(v["dtype"])) for v in m["vars"]]
        sdefs = [(String(v["name"]), String.(v["dims"]), String(v["dtype"])) for v in m["statics"]]
        chunk = Dict{String,Int}(String(k) => Int(v) for (k, v) in m["chunk_shape"])
        shard = Dict{String,Int}(String(k) => Int(v) for (k, v) in m["shard_shape"])
    end
    mk(defs) = Pair{String,OutputVar}[nm => OutputVar(d, _jl_dtype(dt);
                                                      attrs = _stored_attrs(base, nm))
                                      for (nm, d, dt) in defs]
    _, tdt = _array_info(base, time_dim)
    coords = Pair{String,Tuple{Vector,Dict{String,Any}}}[
        time_dim => (Float64[], _stored_attrs(base, time_dim))]
    gp = joinpath(base, "zarr.json")
    gattrs = isfile(gp) ? Dict{String,Any}(get(JSON.parsefile(gp), "attributes", Dict{String,Any}())) :
             Dict{String,Any}()
    return OutputSchema(; dims = dims, time_dim = time_dim, vars = mk(vdefs),
                        chunk_shape = chunk, shard_shape = shard, coords = coords,
                        profile = profile, attrs = gattrs, time_dtype = tdt,
                        statics = mk(sdefs), time_mode = time_mode)
end

# Load the committed state a manifest records into a fresh handle.
function _load_state!(h::ZarrWriteHandle, m)
    td = h.schema.time_dim
    if m isa OutputManifest
        h.committed_t = _read_committed_t(h.base, td, m.n_records)
        return h
    end
    h.commit_seq = Int(m["commit_seq"])
    if _indexed(h)
        fill!(h.rec_committed, false)
        for e in m["records"]["entries"]
            i = Int(e["index"]) + 1
            h.rec_committed[i] = Bool(e["committed"])
            h.rec_generation[i] = Int(e["generation"])
        end
    else
        n = 0
        for (a, b) in m["records"]["committed"]
            Int(a) == n || error("append-mode manifest has non-prefix committed ranges")
            n = Int(b)
        end
        h.committed_t = _read_committed_t(h.base, td, n)
    end
    for e in m["statics"]
        h.static_committed[String(e["name"])] = Bool(e["committed"])
        h.static_generation[String(e["name"])] = Int(e["generation"])
    end
    h.blobs = sort!(_BlobId[(String(b["scope"]), b["record"] === nothing ? -1 : Int(b["record"]),
                             Int(b["writer"]), String(b["key"])) for b in m["blobs"]])
    h.writers = Pair{Int,_Region}[Int(w["id"]) => _parse_region(w["region"]) for w in m["writers"]]
    return h
end

# A contributor re-reads the manifest before refusing a rewrite (spec §6.2): the
# owner may have invalidated the record since the contributor's view was taken.
function _refresh_view!(h::ZarrWriteHandle)
    m = read_output_manifest(h.base)
    m isa AbstractDict || return h
    for e in get(m["records"], "entries", Any[])
        i = Int(e["index"]) + 1
        h.rec_committed[i] = Bool(e["committed"])
        h.rec_generation[i] = Int(e["generation"])
    end
    for e in m["statics"]
        h.static_committed[String(e["name"])] = Bool(e["committed"])
    end
    h.commit_seq = Int(m["commit_seq"])
    return h
end

function _check_role(role::Symbol, n::Int, id::Int, r::_Region, s::OutputSchema)
    role in (:single, :owner, :contributor) ||
        throw(ArgumentError("role must be :single, :owner or :contributor, got $role"))
    if role === :contributor
        id >= 1 || throw(ArgumentError("a contributor's id is 1..n_writers-1, got $id"))
        n > 1 && id >= n && throw(ArgumentError("contributor id $id >= n_writers $n"))
    else
        id == 0 || throw(ArgumentError("the $role writer is writer 0, got id = $id"))
        n >= 1 || throw(ArgumentError("n_writers must be >= 1"))
        role === :single && n != 1 &&
            throw(ArgumentError("a :single writer has n_writers = 1 (use :owner)"))
    end
    if role === :single
        lens = Dict(s.dims)
        all(ab == (0, lens[d]) for (d, ab) in r) || throw(ArgumentError(
            "a :single writer writes the whole extent; use :owner for a region"))
    end
    return nothing
end

# --- write_open! / join -----------------------------------------------------

function write_open!(::ZarrWriter, store, base_url::AbstractString, schema::OutputSchema;
                     role::Symbol = :single, n_writers::Integer = 1, id::Integer = 0,
                     region = nothing)
    _check_schema(schema)
    r = _check_region(region, schema)                    # before any I/O
    _check_role(role, Int(n_writers), Int(id), r, schema)
    role === :contributor && return _join(base_url, schema, Int(n_writers), Int(id), r)

    base = _output_base(base_url)
    mkpath(base)
    v1 = role === :single && schema.time_mode === :append && isempty(schema.statics)
    h = _new_handle(base, schema, role, Int(n_writers), 0, r, v1 ? 1 : 2, rfc3339_utc())
    _create_metadata!(h)
    # a v2 store has a manifest (commit_seq 0) from create; a v1 one from its first commit
    v1 || _write_manifest!(h)
    return h
end

# A contributor opens the store the owner created (spec §5.1 `join`).
function _join(base_url, schema::Union{OutputSchema,Nothing}, n::Int, id::Int, r::_Region)
    base = _output_base(base_url)
    m = read_output_manifest(base)
    m === nothing && error("join: no output manifest at $base (the owner creates the store first)")
    m isa AbstractDict || error(
        "join: the store at $base has a v1 (single-writer) manifest; contributors join v2 stores")
    rebuilt = _schema_from_manifest(m, base)
    fp = schema_fingerprint(rebuilt)
    String(m["schema_fingerprint"]) == fp ||
        error("join: the manifest's schema_fingerprint does not match the schema it describes")
    schema === nothing || schema_fingerprint(schema) == fp ||
        error("join: the caller's schema fingerprint $(schema_fingerprint(schema)) differs " *
              "from the store's $fp")
    s = schema === nothing ? rebuilt : schema
    r = _check_region(r, s)
    h = _new_handle(base, s, :contributor, max(n, id + 1), id, r, 2, String(m["created_at"]))
    _load_state!(h, m)
    h.n_written = length(h.committed_t)
    h.last_t = isempty(h.committed_t) ? nothing : h.committed_t[end]
    return h
end

# --- write_resume -----------------------------------------------------------

function write_resume(::ZarrWriter, store, base_url::AbstractString;
                      role::Symbol = :single, n_writers::Integer = 1, id::Integer = 0,
                      region = nothing, at = nothing, schema = nothing)
    base = _output_base(base_url)
    m = read_output_manifest(base)
    m === nothing && error("resume: no output manifest at $base")
    v1 = m isa OutputManifest
    v1 && role !== :single &&
        error("resume: the store at $base is a v1 single-writer store; resume it as :single")
    rebuilt = _schema_from_manifest(m, base)
    fp = schema_fingerprint(rebuilt)
    v1 || String(m["schema_fingerprint"]) == fp ||
        error("resume: the manifest's schema_fingerprint does not match the schema it describes")
    schema === nothing || schema_fingerprint(schema) == fp ||
        error("resume: the caller's schema fingerprint differs from the store's $fp")
    s = schema === nothing ? rebuilt : schema
    r = _check_region(region, s)
    n = Int(n_writers)
    role === :contributor && (n = max(n, Int(id) + 1))
    _check_role(role, n, Int(id), r, s)
    created = v1 ? m.created_at : String(m["created_at"])
    h = _new_handle(base, s, role, n, Int(id), r, v1 ? 1 : 2, created)
    _load_state!(h, m)

    if _indexed(h)
        at === nothing || error("resume: `at` must be nothing in indexed mode")
    else
        n0 = length(h.committed_t)
        keep = at === nothing ? n0 : something(findlast(<=(Float64(at)), h.committed_t), 0)
        if keep < n0
            resize!(h.committed_t, keep)
            # blobs of the discarded records go with them
            filter!(b -> !(b[1] == "record" && b[2] >= keep), h.blobs)
            if role !== :contributor
                v1 || (h.commit_seq += 1)
                _update_shapes!(h)
                _write_manifest!(h)
            end
        end
        h.n_written = keep
        h.last_t = keep == 0 ? nothing : h.committed_t[end]
        # the open time shard (if any) is reloaded from the store on its next
        # write (`_shard_buf!`), so later flushes rewrite it whole
    end
    return h
end

# --- records, statics, blobs ------------------------------------------------

function write_record!(::ZarrWriter, h::ZarrWriteHandle, t, arrays; region = nothing)
    _check_open(h)
    _check_bound_region(h, region)
    _indexed(h) && error("write_record! is the append-mode call; use write_record_at! " *
                         "in indexed mode")
    t64 = Float64(t)
    h.last_t === nothing || t64 > h.last_t ||
        error("write_record!: t = $t64 is not after the previous record's t = $(h.last_t) " *
              "(append order is time order)")
    i = h.n_written
    _put_record!(h, i, t64, arrays)
    h.n_written += 1
    h.last_t = t64
    return i
end

"""
    write_record_at!(w, h, index, t, arrays; region = nothing)

Indexed mode: write record `index` (0-based, any order). `t` must equal
`times[index]` bitwise. A committed record must be invalidated first."""
function write_record_at!(::ZarrWriter, h::ZarrWriteHandle, index::Integer, t, arrays;
                          region = nothing)
    _check_open(h)
    _check_bound_region(h, region)
    _indexed(h) || error("write_record_at! is the indexed-mode call; use write_record! " *
                         "in append mode")
    i = Int(index)
    0 <= i < _ntime(h) || error("record index $i outside 0..$(_ntime(h) - 1)")
    t64 = Float64(t)
    _same_bits(t64, _times(h)[i + 1]) ||
        error("record $i: t = $t64 is not times[$i] = $(_times(h)[i + 1])")
    if _is_committed(h, i)
        h.role === :contributor && _refresh_view!(h)
        _is_committed(h, i) && error(
            "record $i is committed: the owner must write_invalidate! it before it is rewritten")
    end
    _put_record!(h, i, t64, arrays)
    return nothing
end

function write_static!(::ZarrWriter, h::ZarrWriteHandle, name::AbstractString, array)
    _check_open(h)
    s = h.schema
    k = findfirst(p -> p.first == name, s.statics)
    k === nothing && error("'$name' is not a static of the schema")
    ov = s.statics[k].second
    _supplies(h, ov.dims) || error(
        "contributor $(h.id) cannot write the replicated static '$name' " *
        "(only the owner writes statics without a region dim)")
    if h.static_committed[name]
        h.role === :contributor && _refresh_view!(h)
        h.static_committed[name] && error(
            "static '$name' is committed: write_invalidate_statics! it before it is rewritten")
    end
    want = Tuple(h.hi[d] - h.lo[d] for d in ov.dims)
    size(array) == want ||
        error("static '$name' has shape $(size(array)); expected $want (region-sized)")
    data = Array{ov.dtype}(undef, want...)
    data .= array
    _writes_objects(h, ov.dims) &&
        _put_region_shards!(h, name, ov.dims, ov.dtype, data,
                            Int[h.lo[d] for d in ov.dims], Int[h.hi[d] for d in ov.dims])
    push!(h.pending_statics, String(name))
    return nothing
end

_blob_scope(s::Symbol) = s === :static ? ("static", nothing) :
    throw(ArgumentError("blob scope is :static or (:record, i), got $s"))
_blob_scope(s::AbstractString) = _blob_scope(Symbol(s))
_blob_scope(p::Pair) = _blob_scope((p.first, p.second))
function _blob_scope(t::Tuple)
    (length(t) == 2 && Symbol(t[1]) === :record && t[2] isa Integer && t[2] >= 0) ||
        throw(ArgumentError("blob scope is :static or (:record, i), got $t"))
    return ("record", Int(t[2]))
end

_blob_key(scope, record, writer, key) =
    scope == "static" ? "blobs/static/$writer/$key" : "blobs/record/$record/$writer/$key"

function write_blob!(::ZarrWriter, h::ZarrWriteHandle, scope, key::AbstractString,
                     bytes::AbstractVector{UInt8})
    _check_open(h)
    h.version == 1 && error("write_blob! needs a v2 store: this :single append store " *
                            "without statics writes the v1 manifest (open with role = :owner, " *
                            "n_writers = 1 instead)")
    (occursin(r"^[A-Za-z0-9._-]+$", key) && key != "." && key != "..") ||
        throw(ArgumentError("blob key '$key' must match [A-Za-z0-9._-]+"))
    sc, rec = _blob_scope(scope)
    path = _blob_key(sc, rec, h.id, key)
    put_object!(h.base, path, Vector{UInt8}(bytes))       # one atomic object
    push!(h.objects, path)
    push!(h.pending_blobs, (sc, rec === nothing ? -1 : rec, String(key)))
    return nothing
end

# --- flush / commit / invalidate / close ------------------------------------

function _take_token!(h::ZarrWriteHandle)
    recs = sort!(Tuple{Int,Float64}[(i, t) for (i, t) in h.pending_records]; by = first)
    blobs = Tuple{String,Union{Int,Nothing},String}[(sc, r < 0 ? nothing : r, k)
                                                    for (sc, r, k) in sort!(collect(h.pending_blobs))]
    tok = CommitToken(h.id, copy(h.region), h.fingerprint, recs,
                      sort!(collect(h.pending_statics)), blobs, sort!(collect(h.objects)))
    merge!(h.tokened, h.pending_records)
    empty!(h.pending_records); empty!(h.pending_statics)
    empty!(h.pending_blobs); empty!(h.objects)
    return tok
end

function write_flush!(::ZarrWriter, h::ZarrWriteHandle)
    _check_open(h)
    for k in sort!(collect(keys(h.shards)))
        buf = h.shards[k]
        buf.dirty && _write_time_shard!(h, k, buf)
    end
    tok = _take_token!(h)
    if h.role === :single
        # commits only when something is pending; else a no-op (spec §5.1)
        isempty(tok) || _commit!(h, [tok])
    elseif h.role === :contributor
        # its committed view: the manifest it read plus what it has tokened
        for (i, t) in tok.records
            _indexed(h) ? (h.rec_committed[i + 1] = true) : push!(h.committed_t, t)
        end
        for nm in tok.statics
            h.static_committed[nm] = true
        end
    end
    return tok
end

# Verify `tokens` (spec §5.3) and commit them. Writes nothing on any failure.
function _commit!(h::ZarrWriteHandle, tokens::AbstractVector)
    s = h.schema
    n = h.n_writers
    # 1. exactly one token per writer id
    byid = Dict{Int,CommitToken}()
    for t in tokens
        t isa CommitToken || throw(ArgumentError("commit takes CommitTokens, got $(typeof(t))"))
        0 <= t.writer < n ||
            throw(CommitError("a token from writer $(t.writer), but the store has writers 0..$(n - 1)"))
        haskey(byid, t.writer) && throw(CommitError("two tokens from writer $(t.writer)"))
        byid[t.writer] = t
    end
    for i in 0:(n - 1)
        haskey(byid, i) || throw(CommitError("no token from writer $i"))
    end
    toks = [byid[i] for i in 0:(n - 1)]
    # 2. one schema
    for t in toks
        t.fingerprint == h.fingerprint || throw(CommitError(
            "writer $(t.writer)'s schema fingerprint $(t.fingerprint) differs from the owner's"))
    end
    # 3. the regions tile the store
    rd = Set{String}(first.(toks[1].region))
    for t in toks
        Set{String}(first.(t.region)) == rd || throw(CommitError(
            "writer $(t.writer) names region dims $(sort(first.(t.region))), writer 0 " *
            "names $(sort(collect(rd)))"))
        p = _region_problem(t.region, s)
        p === nothing || throw(CommitError("writer $(t.writer): $(p[2])"))
    end
    for a in 1:n, b in (a + 1):n
        ra, rb = Dict(toks[a].region), Dict(toks[b].region)
        all(d -> max(ra[d][1], rb[d][1]) < min(ra[d][2], rb[d][2]), rd) && throw(CommitError(
            "overlap between writers $(a - 1) $(toks[a].region) and $(b - 1) $(toks[b].region)"))
    end
    lens = Dict(s.dims)
    total = prod((lens[d] for d in rd); init = 1)
    vol = sum(prod((ab[2] - ab[1] for (_, ab) in t.region); init = 1) for t in toks)
    vol == total || throw(CommitError(
        "gap: the writers' regions cover $vol of the $total cells over the region dims " *
        "$(sort(collect(rd)))"))
    # 4. records
    ref = toks[1].records
    for t in toks[2:end]
        (length(t.records) == length(ref) &&
         all(x[1] == y[1] && _same_bits(x[2], y[2]) for (x, y) in zip(t.records, ref))) ||
            throw(CommitError("writer $(t.writer) carries records $(first.(t.records)) but " *
                              "writer 0 carries $(first.(ref)) (or their t differ)"))
    end
    if _indexed(h)
        for (i, t) in ref
            0 <= i < _ntime(h) || throw(CommitError("record index $i outside 0..$(_ntime(h) - 1)"))
            _same_bits(t, _times(h)[i + 1]) ||
                throw(CommitError("record $i: t = $t is not times[$i] = $(_times(h)[i + 1])"))
            h.rec_committed[i + 1] && throw(CommitError(
                "record $i is committed; invalidate it before rewriting"))
        end
    else
        nc = length(h.committed_t)
        first.(ref) == collect(nc:(nc + length(ref) - 1)) || throw(CommitError(
            "append records $(first.(ref)) are not the continuation $nc.. of the committed prefix"))
    end
    # 5. statics
    for t in toks, nm in t.statics
        k = findfirst(p -> p.first == nm, s.statics)
        k === nothing && throw(CommitError("writer $(t.writer)'s token names unknown static '$nm'"))
        if _partitioned(rd, s.statics[k].second.dims)
            for u in toks
                nm in u.statics || throw(CommitError(
                    "partitioned static '$nm' is in writer $(t.writer)'s token but not in " *
                    "writer $(u.writer)'s"))
            end
        else
            t.writer == 0 || throw(CommitError(
                "replicated static '$nm' is in contributor $(t.writer)'s token"))
        end
        h.static_committed[nm] && throw(CommitError(
            "static '$nm' is committed; invalidate it before rewriting"))
    end

    # --- verified: apply, then shapes, then the manifest LAST
    if _indexed(h)
        for (i, _) in ref
            h.rec_committed[i + 1] = true
            h.rec_generation[i + 1] += 1
        end
    else
        append!(h.committed_t, last.(ref))
    end
    for nm in union((t.statics for t in toks)...)
        h.static_committed[nm] = true
        h.static_generation[nm] += 1
    end
    for t in toks, (sc, r, k) in t.blobs
        id = (sc, r === nothing ? -1 : r, t.writer, k)
        id in h.blobs || push!(h.blobs, id)      # a rewritten blob replaces itself
    end
    sort!(h.blobs)
    h.writers = Pair{Int,_Region}[t.writer => t.region for t in toks]
    h.commit_seq += 1
    empty!(h.tokened)
    !_indexed(h) && !isempty(ref) && _update_shapes!(h)
    return _write_manifest!(h)
end

function write_commit!(::ZarrWriter, h::ZarrWriteHandle, tokens::AbstractVector)
    _check_open(h)
    h.role === :owner || error("write_commit! is the owner's call (role = :owner)")
    return _commit!(h, tokens)
end

function write_invalidate!(::ZarrWriter, h::ZarrWriteHandle, indices)
    _check_open(h)
    h.role === :contributor && error("write_invalidate! is the owner's (or single writer's) call")
    _indexed(h) || error("write_invalidate! applies to indexed mode only")
    idx = Int[Int(i) for i in indices]
    for i in idx
        0 <= i < _ntime(h) || error("record index $i outside 0..$(_ntime(h) - 1)")
    end
    for i in idx
        h.rec_committed[i + 1] = false
    end
    # the record(i) blobs are uncommitted with their record (spec §7)
    filter!(b -> !(b[1] == "record" && b[2] in idx), h.blobs)
    h.commit_seq += 1
    _write_manifest!(h)
    return nothing
end

function write_invalidate_statics!(::ZarrWriter, h::ZarrWriteHandle, names)
    _check_open(h)
    h.role === :contributor &&
        error("write_invalidate_statics! is the owner's (or single writer's) call")
    nms = String[String(n) for n in names]
    for nm in nms
        haskey(h.static_committed, nm) || error("'$nm' is not a static of the schema")
    end
    for nm in nms
        h.static_committed[nm] = false
    end
    h.commit_seq += 1
    _write_manifest!(h)
    return nothing
end

"""
    write_close!(w, h, tokens = CommitToken[])

`:single`: flush (committing pending data), rewrite the group node and the
manifest at the same `commit_seq`; returns the manifest (an [`OutputManifest`]
for a v1 store, the parsed v2 object otherwise). `:owner`: flush its own data,
commit `[own token; tokens]` (the OTHER writers' tokens) and return the manifest.
`:contributor`: takes no tokens, errors if anything written has not been flushed,
returns `nothing`."""
function write_close!(w::ZarrWriter, h::ZarrWriteHandle, tokens::AbstractVector = CommitToken[])
    _check_open(h)
    if h.role === :contributor
        isempty(tokens) || error("a contributor's close takes no tokens")
        unflushed = !isempty(h.pending_records) || !isempty(h.pending_statics) ||
                    !isempty(h.pending_blobs) || any(b -> b.dirty, values(h.shards))
        unflushed && error("contributor $(h.id) closes with unflushed writes: " *
                           "write_flush! and deliver the token first")
        h.closed = true
        return nothing
    elseif h.role === :single
        isempty(tokens) || error("a :single writer's close takes no tokens")
        write_flush!(w, h)
        _maybe_locked(true, h.base, "__group__") do
            _write_json(h.base, "zarr.json", _group_meta_dict(h.schema))
        end
        m = _write_manifest!(h)
    else
        tok = write_flush!(w, h)
        m = _commit!(h, vcat(CommitToken[tok], collect(CommitToken, tokens)))
    end
    h.closed = true
    return m
end

# --- read-back (spec §7) ----------------------------------------------------

_store_root(store::AbstractString) = _output_base(store)

function _read_manifest_or_error(base::AbstractString)
    m = read_output_manifest(base)
    m === nothing && error("no output manifest at $base")
    return m
end

_manifest_time_dim(m) = m isa OutputManifest ? m.time_dim : String(m["time_dim"])

function _record_committed(m, i::Int)
    m isa OutputManifest && return 0 <= i < m.n_records
    return any(Int(a) <= i < Int(b) for (a, b) in m["records"]["committed"])
end

# The box of array `name` (dims `dims`) a read-back region selects, with `fixed`
# pinning the time dim (if any) to one index.
function _read_box(base, name, dims::Vector{String}, region, time_dim, fixed::Int)
    m, T = _array_info(base, name)
    r = Dict(_parse_region(region))
    for d in keys(r)
        d in dims || throw(ArgumentError("region dim '$d' is not a dim of '$name' $dims"))
    end
    lo, hi = Int[], Int[]
    for (k, d) in enumerate(dims)
        if d == time_dim
            push!(lo, fixed); push!(hi, fixed + 1)
        else
            a, b = get(r, d, (0, m.shape[k]))
            0 <= a < b <= m.shape[k] ||
                throw(ArgumentError("region $d=[$a, $b) outside [0, $(m.shape[k]))"))
            push!(lo, a); push!(hi, b)
        end
    end
    return _read_region(base, name, m, T, lo, hi)
end

function _array_dims(base, name)
    m, _ = _array_info(base, name)
    return m.dims
end

"""
    read_record(store, var, index; region = nothing) -> Array

Read committed record `index` of streaming variable `var` straight from the store
(a directory or `file://` URL; never through the cache). The result is indexed
in the variable's dim order with the time dim dropped (a time-only variable
gives a 0-dimensional array), region-sized, in the stored dtype. Errors if the
record is not committed."""
function read_record(store, var::AbstractString, index::Integer; region = nothing)
    base = _store_root(store)
    m = _read_manifest_or_error(base)
    i = Int(index)
    _record_committed(m, i) || error("record $i of the store at $base is not committed")
    td = _manifest_time_dim(m)
    dims = _array_dims(base, var)
    ti = findfirst(==(td), dims)
    ti === nothing && error("'$var' is not a streaming variable (no '$td' dim)")
    a = _read_box(base, var, dims, region, td, i)
    return dropdims(a; dims = ti)
end

"""
    read_static(store, name; region = nothing) -> Array

Read the committed static `name` (dims order, region-sized, stored dtype).
Errors if it is not committed."""
function read_static(store, name::AbstractString; region = nothing)
    base = _store_root(store)
    m = _read_manifest_or_error(base)
    m isa OutputManifest && error("a v1 store has no statics")
    e = findfirst(x -> String(x["name"]) == name, collect(m["statics"]))
    e === nothing && error("'$name' is not a static of the store at $base")
    Bool(collect(m["statics"])[e]["committed"]) || error("static '$name' is not committed")
    dims = _array_dims(base, name)
    return _read_box(base, name, dims, region, _manifest_time_dim(m), -1)
end

"""
    read_blob(store, scope, writer, key) -> Vector{UInt8}

Read the committed blob `key` of `writer` under `scope` (`:static` or
`(:record, i)`). Errors if it is not committed."""
function read_blob(store, scope, writer::Integer, key::AbstractString)
    base = _store_root(store)
    m = _read_manifest_or_error(base)
    m isa OutputManifest && error("a v1 store has no blobs")
    sc, rec = _blob_scope(scope)
    found = any(m["blobs"]) do b
        String(b["scope"]) == sc && b["record"] == rec && Int(b["writer"]) == writer &&
            String(b["key"]) == key
    end
    found || error("blob ($sc, $rec, writer $writer, '$key') is not committed")
    return read(joinpath(base, _blob_key(sc, rec, writer, key)))
end
