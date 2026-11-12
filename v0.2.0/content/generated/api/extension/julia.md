---
title: "Julia extension seams"
description: "Julia's extension-tier names: documented, reachable, not portable."
type: "api"
---


Extension-tier names are part of this binding's documented surface but are not harmonized across bindings, and may change in a minor release.

6 names.

### `cds_api_key` {#x-cds_api_key}

*function* · canonical name `cds_api_key`

```julia
cds_api_key(; rc)
```

```julia
cds_api_key(; rc=_read_cdsapirc()) -> String
```

The CDS API key, resolved from `ENV["CDSAPI_KEY"]` then the `key:` line of
`~/.cdsapirc`. Errors with setup guidance if neither is present.

[Source](https://github.com/EarthSciML/EarthSciIO/blob/a49538bbd6ec318dde369d3496f6e8880586a73f/julia/src/cds.jl#L62)

### `CdsTransport` {#x-cdstransport}

*type* · canonical name `cds_transport`

```julia
struct CdsTransport <: Transport
CdsTransport()
```

`cds://` transport: runs the CDS submit → poll → download flow into the
cache's staging file. The dataset + request are carried in the URL (see
[`cds_url`]); present blobs are immutable (a closed past period), so the cache's
content-addressed fast path is the skip-if-exists — `conditional` is unused
because CDS has no conditional-GET.

[Source](https://github.com/EarthSciML/EarthSciIO/blob/a49538bbd6ec318dde369d3496f6e8880586a73f/julia/src/cds.jl#L250)

### `FORMAT_REGISTRY` {#x-format_registry}

*type* · canonical name `format_registry`

```julia
const FORMAT_REGISTRY::Registry{Any}
```

Format/reader registry, keyed by format name. Readers are component (b);
the `zarr` stub is registered now to prove the seam.

[Source](https://github.com/EarthSciML/EarthSciIO/blob/a49538bbd6ec318dde369d3496f6e8880586a73f/julia/src/registries.jl#L85)

### `Reader` {#x-reader}

*type* · canonical name `reader`

```julia
abstract type Reader
```

Open a cached blob and return CF-decoded native-grid arrays keyed by the
on-disk `file_variable` name. Keyed by format name. Implemented by component
(b) (`esio-9nb.5`); declared here so the registry seam is complete.

[Source](https://github.com/EarthSciML/EarthSciIO/blob/a49538bbd6ec318dde369d3496f6e8880586a73f/julia/src/registries.jl#L21)

### `Store` {#x-store}

*type* · canonical name `store`

```julia
abstract type Store
```

Physical home of the content-addressed cache (blobs/meta/locks). Keyed by
store name. Realizes `spec/cache-format.md`; the cache key is store-independent.

[Source](https://github.com/EarthSciML/EarthSciIO/blob/a49538bbd6ec318dde369d3496f6e8880586a73f/julia/src/registries.jl#L17)

### `Transport` {#x-transport}

*type* · canonical name `transport`

```julia
abstract type Transport
```

Fetch a resolved URL's bytes into the cache. Keyed by URL scheme.
Bypassed entirely in offline mode (never constructed when `offline=true`).

[Source](https://github.com/EarthSciML/EarthSciIO/blob/a49538bbd6ec318dde369d3496f6e8880586a73f/julia/src/registries.jl#L13)
