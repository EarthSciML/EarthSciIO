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
