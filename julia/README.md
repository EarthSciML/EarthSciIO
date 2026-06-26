# EarthSciIO.jl

The Julia track of [EarthSciIO](../README.md) — the cross-language data-provider
library. This package is **component (a)** (`esio-9nb.4`): URL download + a
content-addressed cache that is **shared** with the Python and Rust tracks, so a
file fetched by one language is reused byte-for-byte by the others.

It is the first data-loader machinery in the Julia track, implemented against
the language-neutral spec in [`../spec`](../spec) and the conformance corpus in
[`../conformance`](../conformance).

## What's here

| Piece | Spec | Notes |
|---|---|---|
| Content-addressed cache | [cache-format.md](../spec/cache-format.md) | key = `sha256(resolved_url)`; `$EARTHSCIDATADIR` layout; atomic-rename writes + `mkpidlock` advisory lock |
| Offline mode | [offline-mode.md](../spec/offline-mode.md) | cache-only; a miss raises `CacheMiss`; no socket opened |
| `transport` / `format` / `store` registries | [registries.md](../spec/registries.md) | `http`/`file` transports + `local` store active; `s3`/`zarr` registered as stubs |
| Manifest | [schemas/manifest.schema.json](../spec/schemas/manifest.schema.json) | per-blob validation/provenance; never stores credentials |

The `format` readers (native-array decode) and the cadence `Provider` are
**component (b)** (`esio-9nb.5`); they register into `FORMAT_REGISTRY` without
changing anything here.

## Usage

```julia
using EarthSciIO

# online: fetch a resolved URL into the shared cache
cache = Cache(; root = "/scratch.local/$(ENV["USER"])/earthsci-cache")
entry = fetch_blob(cache, "https://data.earthsci.dev/era5/2018/11/20181108.nc")
entry.path        # path to the cached blob (native source bytes)
entry.status      # :downloaded | :hit | :not_modified

# offline (hermetic CI / conformance): cache-only, a miss raises CacheMiss
offline = Cache(; root = corpus_cache_dir, offline = true)
fetch_blob(offline, url)
```

`offline` defaults to the `EARTHSCI_OFFLINE` environment variable; an explicit
argument wins. `EARTHSCIDATADIR` selects the cache root (default on
`/scratch.local`, never `/u`).

## Tests

```bash
julia --project=julia -e 'using Pkg; Pkg.test()'   # offline + a hermetic localhost server
EARTHSCI_LIVE=1 julia --project=julia -e 'using Pkg; Pkg.test()'  # + opt-in live network smoke
```

The suite covers the fetch/cache/offline cycle, the cross-language reuse of the
committed corpus (shared `sha256` layout), registry dispatch, and the
multi-process locking contract.
