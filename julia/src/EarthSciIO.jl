"""
    EarthSciIO

Cross-language data-provider library — Julia track, component (a):
URL download + a content-addressed cache that is **shared** across the
Python / Julia / Rust tracks (key = `sha256(resolved_url)`), so a file
fetched by one language is reused byte-for-byte by the others.

This is the first data-loader machinery in the Julia track. It implements
the EarthSciIO spec (`spec/cache-format.md`, `spec/registries.md`,
`spec/offline-mode.md`):

  * content-addressed cache on `\$EARTHSCIDATADIR` (atomic-rename writes +
    per-blob advisory `mkpidlock` for multi-process safety),
  * ETag / Last-Modified conditional GET, content-hash integrity, TTL,
  * OFFLINE mode (cache-only; a miss raises [`CacheMiss`]),
  * the `transport` (http/file/+s3-stub) and `store` (local/+s3-stub)
    registries — new backends register under a new name without touching
    the Provider API.

The `format` registry (readers returning native arrays) and the cadence
`Provider` are component (b) — `esio-9nb.5` — and register into
[`FORMAT_REGISTRY`] without changing anything here.
"""
module EarthSciIO

using SHA: sha256
using Dates
using UUIDs: uuid4
import Downloads
import JSON
using FileWatching.Pidfile: mkpidlock

# interfaces + the three extensibility registries
export Registry, register!, registered_names, status_of
export TRANSPORT_REGISTRY, FORMAT_REGISTRY, STORE_REGISTRY
export Transport, Store, Reader

# cache + store + transport
export Cache, CacheEntry, fetch_blob, cache_key, datadir, is_offline
export Store, LocalStore, S3Store, make_store
export Manifest
export Transport, HttpTransport, FileTransport, S3Transport
export AuthResolver, NoAuth, BearerAuth

# errors
export CacheMiss, IntegrityError

include("registries.jl")
include("manifest.jl")
include("store.jl")
include("transport.jl")
include("cache.jl")

"""Register the built-in transports + stores into the shared registries.

`active` backends ship now; `stub` backends are registered now (so the
registry dispatch is complete and `esio-9nb.8` can exercise them) and gain
real implementations later — with zero change to caller code.
"""
function _register_defaults()
    # transport registry — keyed by URL scheme
    register!(TRANSPORT_REGISTRY, ("http", "https"), HttpTransport(); status = :active)
    register!(TRANSPORT_REGISTRY, "file", FileTransport(); status = :active)
    register!(TRANSPORT_REGISTRY, "s3", S3Transport(); status = :stub)

    # store registry — keyed by store name; value is a factory `(; root, …) -> Store`
    register!(STORE_REGISTRY, "local",
              (; root = datadir(), _kw...) -> LocalStore(root); status = :active)
    register!(STORE_REGISTRY, "s3", (; _kw...) -> S3Store(); status = :stub)

    # format registry — readers are component (b) (esio-9nb.5); the future
    # NetCDF→Zarr path is registered now as a stub so the seam is provable.
    register!(FORMAT_REGISTRY, "zarr",
              StubReader("zarr", "chunked store; reader impl lands with esio-9nb.8");
              status = :stub)
    return nothing
end

function __init__()
    _register_defaults()
    return nothing
end

end # module
