"""
    EarthSciIO

Cross-language data-provider library — Julia track. It fulfils the ESS
data-loader CONTRACT for `.esm` nodes (URL / vars / native-grid /
temporal-cadence) on a content-addressed cache that is **shared** across the
Python / Julia / Rust tracks (key = `sha256(resolved_url)`), so a file fetched
by one language is reused byte-for-byte by the others.

This is the first data-loader machinery in the Julia track. It implements the
EarthSciIO spec (`spec/cache-format.md`, `spec/registries.md`,
`spec/offline-mode.md`, `spec/conformance.md`):

  * **component (a)** — `esio-9nb.4`: the cache. Content-addressed on
    `\$EARTHSCIDATADIR` (atomic-rename writes + per-blob advisory `mkpidlock`),
    ETag / Last-Modified conditional GET, content-hash integrity, TTL, OFFLINE
    mode (cache-only; a miss raises [`CacheMiss`]); the `transport`
    (http/file/+s3-stub) and `store` (local/+s3-stub) registries.
  * **component (b)** — `esio-9nb.5`: the format readers and the cadence
    [`Provider`]. [`NetCDFReader`] (NCDatasets) and [`CSVReader`] register into
    [`FORMAT_REGISTRY`] and return RAW native-grid arrays ([`read_native`]);
    [`Provider`] resolves+fetches+decodes per [`Cadence`] ([`CONST`]/[`DISCRETE`])
    and exposes [`materialize`]/[`refresh`]/[`refresh_times`]/[`prefetch`]. The
    library provides DATA, not a solver: it exposes `refresh_times`; the
    user/solver drives the discrete-cadence update (no solver embedded).

Variable remap and unit conversion stay in ESS; regrid stays in ESD/C4 — the
readers return arrays keyed by the on-disk `file_variable` name, unremapped.
"""
module EarthSciIO

using SHA: sha256
using Dates
using UUIDs: uuid4
import Downloads
import JSON
using FileWatching.Pidfile: mkpidlock
import NCDatasets

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

# CDS (Copernicus CDS) transport + ERA5 request mapping (esio-9nb.11)
export CdsTransport, CdsAuth, cds_url, cds_api_key, cds_api_endpoint, cds_retrieve
export ERA5_PL_DATASET, ERA5_PRESSURE_LEVELS_HPA, ERA5_VARIABLES
export era5_area, era5_pressure_request, era5_pressure_url

# format readers + native arrays (component b)
export NetCDFReader, CSVReader, read_native
export NativeField, NativeDataset, variable_names, coord_names

# cadence provider (component b)
export Provider, const_provider, discrete_provider, Cadence, CONST, DISCRETE
export materialize, refresh, refresh_times, prefetch, is_const

# errors
export CacheMiss, IntegrityError

include("registries.jl")
include("manifest.jl")
include("store.jl")
include("transport.jl")
include("cds.jl")
include("era5.jl")
include("cache.jl")
include("readers.jl")
include("provider.jl")

"""Register the built-in transports + stores into the shared registries.

`active` backends ship now; `stub` backends are registered now (so the
registry dispatch is complete and `esio-9nb.8` can exercise them) and gain
real implementations later — with zero change to caller code.
"""
function _register_defaults()
    # transport registry — keyed by URL scheme
    register!(TRANSPORT_REGISTRY, ("http", "https"), HttpTransport(); status = :active)
    register!(TRANSPORT_REGISTRY, "file", FileTransport(); status = :active)
    register!(TRANSPORT_REGISTRY, "cds", CdsTransport(); status = :active)
    register!(TRANSPORT_REGISTRY, "s3", S3Transport(); status = :stub)

    # store registry — keyed by store name; value is a factory `(; root, …) -> Store`
    register!(STORE_REGISTRY, "local",
              (; root = datadir(), _kw...) -> LocalStore(root); status = :active)
    register!(STORE_REGISTRY, "s3", (; _kw...) -> S3Store(); status = :stub)

    # format registry — readers returning native arrays (component b). NetCDF
    # (NCDatasets) + CSV are active; the future NetCDF→Zarr path stays a stub so
    # the seam is provable. A new format is one more register! line — never a
    # Provider change.
    register!(FORMAT_REGISTRY, "netcdf", NetCDFReader(); status = :active)
    register!(FORMAT_REGISTRY, "csv", CSVReader(); status = :active)
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
