---
title: "API reference"
description: "Every stable operation of the three EarthSciIO bindings, side by side."
type: "api"
api_tabs: true
---


EarthSciIO has three bindings — Python, Julia and Rust — that download data into one shared content-addressed cache and read it as native-grid arrays. This reference has one page per **stable** operation, with a tab for each binding showing its spelling, signatures and documentation, taken from the binding's own documentation tooling, along with runnable examples. The examples run in CI, so what is shown here is what the code does.

The surface has two tiers. **Stable** names are harmonized across bindings and break only at a major version. **Extension** names are a binding's own documented seams; they may differ between bindings and may change in a minor release, and each binding lists them on one page.

## Guides

- [Getting started](/generated/api/guides/getting-started) — Fetch a file into the shared cache, read it back offline, and decode it into native-grid arrays.

## Topics

- [Cache](/generated/api/topics/cache) (10) — Fetching a resolved URL into the shared content-addressed cache, validating what is stored, and reading it back offline.
- [Providers](/generated/api/topics/providers) (10) — Binding a data source to the cache and a reader, and getting native-grid arrays out at the simulation's cadence.
- [Prefetch and windows](/generated/api/topics/prefetch) (13) — Decoding upcoming forcing epochs on background workers, bitwise identical to refresh, and windowing a provider by named dimension so each rank reads only its block.
- [Native arrays and readers](/generated/api/topics/native-arrays) (9) — The raw native-grid arrays a provider returns, and the format readers that decode cached files into them.
- [Streaming writer](/generated/api/topics/writer) (22) — Streaming a run's output into a sharded Zarr v3 store: region writes from many ranks without a gather, commit tokens delivered to one owner, indexed records, statics, blobs, read-back and resume.
- [Errors](/generated/api/topics/errors) (5) — What fails and how: offline misses, integrity failures and the general error.

Or browse [all stable operations A–Z](/generated/api/ref).

## Bindings

| Binding | Package | Version | Docs from | Stable | Extension |
|---|---|---|---|---|---|
| [Python](/generated/api/bindings/python) | `earthsciio` | 0.2.0 | griffe 2.3.2 | 31 | [6](/generated/api/extension/python) |
| [Julia](/generated/api/bindings/julia) | `EarthSciIO` | 0.2.0 | Base.Docs (Julia 1.13.3) | 59 | [6](/generated/api/extension/julia) |
| [Rust](/generated/api/bindings/rust) | `earthsciio` | 0.2.0 | rustdoc JSON (rustdoc 1.101.0-nightly (21b707e3f 2026-09-30)), format_version 61 | 40 | [1](/generated/api/extension/rust) |

[Documentation coverage](/generated/api/coverage) lists the stable names each binding has not documented yet.
