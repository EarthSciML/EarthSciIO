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
    etag::Union{String,Nothing}
    last_modified::Union{String,Nothing}
    sha256_content::String
    bytes::Int
    fetched_at::String
    source_loader::Union{String,Nothing}
    auth_realm::Union{String,Nothing}
end

_strornothing(::Nothing) = nothing
_strornothing(x::AbstractString) = String(x)

"""Manifest as an ordered-by-key `Dict` ready for JSON serialization. Optional
fields are emitted as JSON `null` (never omitted) — the key is always present."""
function manifest_dict(m::Manifest)
    return Dict{String,Any}(
        "schema"         => MANIFEST_SCHEMA,
        "url"            => m.url,
        "etag"           => m.etag,
        "last_modified"  => m.last_modified,
        "sha256_content" => m.sha256_content,
        "bytes"          => m.bytes,
        "fetched_at"     => m.fetched_at,
        "source_loader"  => m.source_loader,
        "auth_realm"     => m.auth_realm,
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
function read_manifest(path::AbstractString)::Union{Manifest,Nothing}
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
