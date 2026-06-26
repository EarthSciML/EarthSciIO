# Store backends (spec/registries.md §3, spec/cache-format.md §2,6).
#
# `LocalStore` is the active `$EARTHSCIDATADIR` filesystem backend; `S3Store` is
# a registered stub. The cache key is store-independent, so swapping local→s3
# changes only where blobs live — never the key scheme or any reader.

const CACHE_VERSION = "v1"

"""
    LocalStore(root)

Local-disk cache rooted at `root` (= `\$EARTHSCIDATADIR`). On-disk form
(spec/cache-format.md §2), everything under the version dir so a format bump
invalidates the whole cache by changing one path segment:

    <root>/v1/blobs/<key[:2]>/<key>.<ext>   the downloaded blob
    <root>/v1/meta/<key>.json               the manifest
    <root>/v1/locks/<key>.lock              per-blob advisory lock
    <root>/v1/tmp/<uuid>.part               atomic-rename staging
"""
struct LocalStore <: Store
    root::String
end
LocalStore(root::AbstractString) = LocalStore(String(root))

store_name(::LocalStore) = "local"

_vroot(s::LocalStore)       = joinpath(s.root, CACHE_VERSION)
_blobdir(s::LocalStore, k)  = joinpath(_vroot(s), "blobs", k[1:2])
_metapath(s::LocalStore, k) = joinpath(_vroot(s), "meta", string(k, ".json"))
_lockpath(s::LocalStore, k) = joinpath(_vroot(s), "locks", string(k, ".lock"))
_tmpdir(s::LocalStore)      = joinpath(_vroot(s), "tmp")

# Lookups are by <key>, never by extension (the suffix is human-debug only).
function get_blob(s::LocalStore, key::AbstractString)
    dir = _blobdir(s, key)
    isdir(dir) || return nothing
    for f in readdir(dir)
        dot = findfirst('.', f)
        stem = dot === nothing ? f : f[1:prevind(f, dot)]
        stem == key && return joinpath(dir, f)
    end
    return nothing
end

blob_exists(s::LocalStore, key::AbstractString) = get_blob(s, key) !== nothing

function staging_path(s::LocalStore)
    d = _tmpdir(s)
    mkpath(d)
    return joinpath(d, string(uuid4(), ".part"))
end

# Atomic commit: rename(2) the staged file into blobs/. The rename is the real
# guarantee — a reader never sees a partial file even without taking the lock.
function put_blob!(s::LocalStore, key::AbstractString, staged::AbstractString;
                   ext::AbstractString = "")
    dir = _blobdir(s, key)
    mkpath(dir)
    fname = if isempty(ext)
        key
    else
        string(key, startswith(ext, ".") ? ext : string(".", ext))
    end
    target = joinpath(dir, fname)
    Base.Filesystem.rename(staged, target)
    return target
end

get_meta(s::LocalStore, key::AbstractString) = read_manifest(_metapath(s, key))

function put_meta!(s::LocalStore, key::AbstractString, m::Manifest)
    return write_manifest(_metapath(s, key), m)
end

# Per-blob advisory lock (spec §6): `mkpidlock` with wait=true blocks until the
# lock is free, so a Julia process and a Python process racing the same URL is
# safe and results in exactly one download. The lock scope is one blob fetch.
function lock_key(f::Function, s::LocalStore, key::AbstractString)
    lp = _lockpath(s, key)
    mkpath(dirname(lp))
    lk = mkpidlock(lp; wait = true)
    try
        return f()
    finally
        close(lk)
    end
end

# --- s3 store (STUB; esio-9nb.8) --------------------------------------------

"""Registered stub for the future object-store backend (esio-9nb.8). Conditional
PUT / `If-None-Match` will be the lock analog. No network impl yet."""
struct S3Store <: Store end
store_name(::S3Store) = "s3"

const _S3_STUB = "s3 store is a registered STUB (esio-9nb.8); no object-store impl yet"
get_blob(::S3Store, ::AbstractString)            = error(_S3_STUB)
blob_exists(::S3Store, ::AbstractString)         = error(_S3_STUB)
staging_path(::S3Store)                          = error(_S3_STUB)
put_blob!(::S3Store, ::AbstractString, ::AbstractString; kwargs...) = error(_S3_STUB)
get_meta(::S3Store, ::AbstractString)            = error(_S3_STUB)
put_meta!(::S3Store, ::AbstractString, ::Manifest) = error(_S3_STUB)
lock_key(::Function, ::S3Store, ::AbstractString) = error(_S3_STUB)
