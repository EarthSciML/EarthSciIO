# EarthSciIO

Cross-language (Python / Julia / Rust) data-provider library that fulfills the
EarthSci data-loader contract: it resolves a loader's URL at a time, downloads
and caches the file in a **shared content-addressed cache**, and reads it into
**native-grid arrays** the simulation consumes at its cadence.

EarthSciIO is the sanctioned **impure I/O boundary** of the EarthSci stack — it
provides *data*, never a solver. Variable-remap stays in EarthSciSerialization
(ESS); regrid stays in EarthSciDiscretizations (ESD). See the boundary writeup
in [`spec/README.md`](spec/README.md).

## Status

Greenfield. The language-neutral **provider spec** lands first and gates the
per-language implementations:

- **[`spec/`](spec/)** — the contract: shared
  [cache + manifest format](spec/cache-format.md)
  (`key = sha256(resolved_url)`), the three extensibility
  [registries](spec/registries.md) (transport / format / store), the
  [offline-mode](spec/offline-mode.md) contract, and the
  [conformance](spec/conformance.md) corpus format. JSON Schemas in
  [`spec/schemas/`](spec/schemas).
- **[`conformance/`](conformance/)** — offline golden fixtures + expected native
  arrays + a reference runner. `python3 conformance/verify.py` validates the
  corpus offline.

Architecture is **idiomatic-per-language against the shared spec** (not a
Rust+FFI core), extensible by construction so an S3 transport / object-store
backend / Zarr reader slot in without touching the Provider API.

## Python (`earthsciio/`)

The Python track ships the **URL download + content-addressed cache** behind the
ESS opener/fetcher seam (`esio-9nb.2`). It registers the active `http`/`file`
transports and the `local` store into the three registries, alongside the
`s3`/`zarr` cloud stubs.

```python
from earthsciio import Cache

cache = Cache()                                   # root = $EARTHSCIDATADIR
entry = cache.fetch(
    "https://data.earthsci.dev/era5/2018/11/20181108.nc",
    source_loader="era5",
    mirrors=["https://mirror.example/era5/20181108.nc"],   # tried in order
)
entry.path        # local blob (fetched once, shared across processes + languages)
entry.status      # "downloaded" | "hit" | "not_modified"

# Offline (cache-only, hermetic) — never touches the network:
hit = Cache(offline=True).fetch(...)              # raises CacheMiss on a miss
```

`fetch` is content-addressed by `sha256(resolved_url)`, validates via
checksum / conditional-GET / TTL, writes atomically under a per-blob `flock`
(safe for many processes on one `/scratch.local` cache), and records a sidecar
manifest. Auth is a pluggable realm → resolver seam
(`StaticHeaderAuth.bearer(...)` / `.header(...)`); credentials never reach the
manifest. Run the suite with `pytest` (fully offline/hermetic).
