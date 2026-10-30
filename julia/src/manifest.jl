# The per-blob manifest (spec/cache-format.md §3, schemas/manifest.schema.json).
#
# Every cached blob has a sibling `meta/<key>.json` carrying its validation +
# provenance state. The format is identical across Python / Julia / Rust so a
# blob fetched by one language is re-validated by the others. Credentials are
# NEVER written here — only the auth realm name.

const MANIFEST_SCHEMA = "earthsciio/manifest/v1"

"""
    Manifest

Validation + provenance record for one cached blob. Field mapping (bead → here):
source-url → `url`, etag → `etag`, checksum → `sha256_content`,
fetched-at → `fetched_at`, byte-size → `bytes`.
"""
struct Manifest
    url::String
    etag::Union{String, Nothing}
    last_modified::Union{String, Nothing}
    sha256_content::String
    bytes::Int
    fetched_at::String
    source_loader::Union{String, Nothing}
    auth_realm::Union{String, Nothing}
end

_strornothing(::Nothing) = nothing
_strornothing(x::AbstractString) = String(x)

"""Manifest as an ordered-by-key `Dict` ready for JSON serialization. Optional
fields are emitted as JSON `null` (never omitted) — the key is always present."""
function manifest_dict(m::Manifest)
    return Dict{String, Any}(
        "schema" => MANIFEST_SCHEMA,
        "url" => m.url,
        "etag" => m.etag,
        "last_modified" => m.last_modified,
        "sha256_content" => m.sha256_content,
        "bytes" => m.bytes,
        "fetched_at" => m.fetched_at,
        "source_loader" => m.source_loader,
        "auth_realm" => m.auth_realm,
    )
end

"""Atomically write `m` to `path` as pretty, sorted JSON (a concurrent reader
never sees a partial manifest)."""
function write_manifest(path::AbstractString, m::Manifest)
    mkpath(dirname(path))
    tmp = string(path, ".", uuid4(), ".tmp")
    write(tmp, JSON.json(manifest_dict(m); pretty = true))
    Base.Filesystem.rename(tmp, path)
    return path
end

"""Read `meta/<key>.json` into a [`Manifest`], or `nothing` if absent."""
function read_manifest(path::AbstractString)::Union{Manifest, Nothing}
    isfile(path) || return nothing
    d = JSON.parsefile(path)
    return Manifest(
        String(d["url"]),
        _strornothing(get(d, "etag", nothing)),
        _strornothing(get(d, "last_modified", nothing)),
        String(d["sha256_content"]),
        Int(d["bytes"]),
        String(d["fetched_at"]),
        _strornothing(get(d, "source_loader", nothing)),
        _strornothing(get(d, "auth_realm", nothing)),
    )
end

# --- output manifest (the WRITE-path mirror; schema output-manifest/v1) ------
#
# Where `Manifest` records a fetched READ blob's validation/provenance, the
# OUTPUT manifest is the per-store durable record of a streaming write: which
# time-shards were committed (index + t-range), the last durable `t`, the
# format+codec params, a schema fingerprint (variable order, dims, dtype), and
# the store base URL. Because each shard is staged→atomically-renamed BEFORE the
# manifest is rewritten, the manifest is the crash barrier: a reader/rerun sees a
# consistent prefix (last durable `t`) and never a partial shard.

const OUTPUT_MANIFEST_SCHEMA = "earthsciio/output-manifest/v1"

"""One committed time-shard: its `index` along the time axis, the covered
`t_start`/`t_end` coordinate range, and the record count `n`."""
struct TimeShardRecord
    index::Int
    t_start::Float64
    t_end::Float64
    n::Int
end

_timeshard_dict(s::TimeShardRecord) = Dict{String, Any}(
    "index" => s.index, "t_start" => s.t_start, "t_end" => s.t_end, "n" => s.n
)
# (`n_records` is the Python v1 spelling of `n`.)
_timeshard_from(d) = TimeShardRecord(
    Int(d["index"]), Float64(d["t_start"]), Float64(d["t_end"]),
    Int(haskey(d, "n") ? d["n"] : d["n_records"])
)

"""
    OutputManifest

Per-store durable record of a Zarr v3 streaming write (schema
`earthsciio/output-manifest/v1`). `codec` pins the Blosc params; `dims`/`vars`
are the schema fingerprint (order is load-bearing); `time_shards` lists the
committed shards; `last_t`/`n_records` mark the durable prefix."""
struct OutputManifest
    base_url::String
    format::String
    zarr_format::Int
    profile::String
    codec::Dict{String, Any}
    time_dim::String
    dims::Vector{Pair{String, Int}}
    vars::Vector{Dict{String, Any}}     # ordered; each {name, dims, dtype}
    chunk_shape::Dict{String, Int}
    shard_shape::Dict{String, Int}
    time_shards::Vector{TimeShardRecord}
    last_t::Union{Float64, Nothing}
    n_records::Int
    created_at::String
end

function output_manifest_dict(m::OutputManifest)
    return Dict{String, Any}(
        "schema" => OUTPUT_MANIFEST_SCHEMA,
        "base_url" => m.base_url,
        "format" => m.format,
        "zarr_format" => m.zarr_format,
        "profile" => m.profile,
        "codec" => m.codec,
        "time_dim" => m.time_dim,
        "dims" => [Dict{String, Any}("name" => k, "length" => v) for (k, v) in m.dims],
        "vars" => m.vars,
        "chunk_shape" => m.chunk_shape,
        "shard_shape" => m.shard_shape,
        "time_shards" => [_timeshard_dict(s) for s in m.time_shards],
        "last_t" => m.last_t,
        "n_records" => m.n_records,
        "created_at" => m.created_at,
    )
end

"""Atomically write the output manifest `m` to `path` (a concurrent reader never
sees a partial manifest)."""
function write_output_manifest(path::AbstractString, m::OutputManifest)
    mkpath(dirname(path))
    tmp = string(path, ".", uuid4(), ".tmp")
    write(tmp, JSON.json(output_manifest_dict(m); pretty = true))
    Base.Filesystem.rename(tmp, path)
    return path
end

"""The v2 label (spec/writer.md §8.2): every store that is not single-writer,
append-mode and static-free. Represented as the parsed JSON object."""
const OUTPUT_MANIFEST_SCHEMA_V2 = "earthsciio/output-manifest/v2"

# `path` may be the manifest file itself, the store directory, or a `file://` URL
# of either.
function _output_manifest_path(path::AbstractString)
    p = startswith(path, "file://") ? file_url_to_path(path) : String(path)
    return isdir(p) ? joinpath(p, "output_manifest.json") : p
end

"""
    read_output_manifest(path) -> OutputManifest | AbstractDict | nothing

Read a store's output manifest. `path` is the `output_manifest.json` file, the
store directory, or a `file://` URL of either; `nothing` when absent. A v1
manifest (`earthsciio/output-manifest/v1`) parses to an [`OutputManifest`]; a v2
manifest (spec/writer.md §8.2) is returned as the parsed JSON object (keys as in
the spec: `commit_seq`, `records`, `statics`, `blobs`, `writers`, …). Any other
`schema` label is an error (readers pin the label exactly)."""
function read_output_manifest(path::AbstractString)
    p = _output_manifest_path(path)
    isfile(p) || return nothing
    d = JSON.parsefile(p)
    label = get(d, "schema", nothing)
    label == OUTPUT_MANIFEST_SCHEMA_V2 && return d
    label == OUTPUT_MANIFEST_SCHEMA ||
        error("unknown output manifest schema $(repr(label)) in $p")
    return _output_manifest_v1(d)
end

# The v1 parse. Tolerates the Python v1 spelling (`total_records`, `dims` as
# `[name, length]` pairs, `written_at`) so a Python-written v1 store reads back.
function _output_manifest_v1(d)
    lt = get(d, "last_t", nothing)
    dims = Pair{String, Int}[
        e isa AbstractVector ? String(e[1]) => Int(e[2]) :
            String(e["name"]) => Int(e["length"]) for e in d["dims"]
    ]
    n = haskey(d, "n_records") ? d["n_records"] : d["total_records"]
    return OutputManifest(
        String(d["base_url"]),
        String(d["format"]),
        Int(d["zarr_format"]),
        String(d["profile"]),
        Dict{String, Any}(d["codec"]),
        String(d["time_dim"]),
        dims,
        Vector{Dict{String, Any}}(Dict{String, Any}.(d["vars"])),
        Dict{String, Int}(String(k) => Int(v) for (k, v) in d["chunk_shape"]),
        Dict{String, Int}(String(k) => Int(v) for (k, v) in d["shard_shape"]),
        TimeShardRecord[_timeshard_from(s) for s in d["time_shards"]],
        lt === nothing ? nothing : Float64(lt),
        Int(n),
        String(get(d, "created_at", get(d, "written_at", ""))),
    )
end
