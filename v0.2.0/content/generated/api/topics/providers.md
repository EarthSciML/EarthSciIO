---
title: "Providers"
description: "Binding a data source to the cache and a reader, and getting native-grid arrays out at the simulation's cadence."
type: "api"
---


| Operation | Kind | Bindings | Summary |
|---|---|---|---|
| [`const_provider`](/generated/api/ref/const_provider) | function | Julia | Thin constructor for a time-invariant ([`CONST`]) provider. |
| [`discrete_provider`](/generated/api/ref/discrete_provider) | function | Julia | Thin constructor for a time-varying ([`DISCRETE`]) provider over `times`. |
| [`materialize`](/generated/api/ref/materialize) | function | Julia | Return the native arrays for the source at time `t`. |
| [`prefetch`](/generated/api/ref/prefetch) | function | Julia | Warm the cache for every URL the provider will need — the single URL for a [`CONST`] provider, the unique per-tick URLs for a [`DISCRETE`] one — WITHOUT decoding. |
| [`refresh`](/generated/api/ref/refresh) | function | Julia | Re-materialize at the cadence tick `t`. |
| [`refresh_times`](/generated/api/ref/refresh_times) | function | Julia | The discrete time points at which the data changes and the solver must [`refresh`]. |
| [`data_source`](/generated/api/ref/data_source) | type | Python, Rust | The I/O-relevant projection of an ESS `DataSource` the Provider needs. |
| [`provider`](/generated/api/ref/provider) | type | Python, Julia, Rust | A loader-bound provider of native-grid arrays, refreshed at the loader's cadence. |
| [`source_temporal`](/generated/api/ref/source_temporal) | type | Python, Rust | The temporal cadence of a DISCRETE loader (absent ⇒ CONST). |
| [`window`](/generated/api/ref/window) | type | Python, Rust | A run window `(start, end)` — half-open `[start, end)`. |

[All topics](/generated/api)
