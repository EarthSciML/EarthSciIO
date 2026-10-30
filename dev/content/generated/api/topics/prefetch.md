---
title: "Prefetch and windows"
description: "Decoding upcoming forcing epochs on background workers, bitwise identical to refresh, and windowing a provider by named dimension so each rank reads only its block."
type: "api"
---


| Operation | Kind | Bindings | Summary |
|---|---|---|---|
| [`boundaries`](/generated/api/ref/boundaries) | function | Julia | The sorted, de-duplicated union `B` of the members' `refresh_times`, in their own cadence unit. |
| [`cancel`](/generated/api/ref/cancel) | function | Julia | Drop all pending work at once (spec/prefetch.md §3.8). |
| [`sample_at`](/generated/api/ref/sample_at) | function | Julia | The dataset `refresh(p, t)` returns — data variables AND coordinates, with the same snap-down to the cadence anchor, the same file-local record, the same `records_per_sample = 2`… |
| [`sample_windows_at`](/generated/api/ref/sample_windows_at) | function | Julia | K windows at once: dataset `i` is identical — bitwise, coordinates included — to `sample_at(with_window(p, windows[i]), t)`, but the source is fetched and decoded ONCE, over the… |
| [`select_form`](/generated/api/ref/select_form) | function | Julia | The wire form of a window, `Dict("dims" => Dict(dim => selector, ...))` — what a reader's `select` option and `reader_kwargs[:select]` take (spec/prefetch.md §2.2). |
| [`start`](/generated/api/ref/start) | function | Julia | Declare the order in which epochs will be taken and begin decoding ahead of it (spec/prefetch.md §3.3): `:forward` (`1, …, n`), `:reverse` (`n, …, 1` — the adjoint's backward… |
| [`take`](/generated/api/ref/take) | function | Julia | Removes and returns a value from a `Channel` in order. |
| [`with_window`](/generated/api/ref/with_window) | function | Julia | A NEW provider that shares `p`'s cache, reader, URL resolver and cadence and has the window `w` (a [`DimWindow`] or its wire `Dict`) baked in as `reader_kwargs[:select]`. |
| [`dim_window`](/generated/api/ref/dim_window) | type | Python, Julia, Rust | A window that names its dimensions (`spec/prefetch.md` §2.2). |
| [`epoch_sample`](/generated/api/ref/epoch_sample) | type | Python, Julia, Rust | Every member's `Sample` for one epoch: `ep["U"].data`. |
| [`prefetch_group`](/generated/api/ref/prefetch_group) | type | Python, Julia, Rust | Background decode of upcoming forcing epochs (`spec/prefetch.md` §3). |
| [`prefetch_order`](/generated/api/ref/prefetch_order) | type | Rust | The order in which epochs will be taken (`spec/prefetch.md` §3.3). |
| [`prefetch_sample`](/generated/api/ref/prefetch_sample) | type | Python, Julia, Rust | One member's data for one epoch. |

[All topics](/generated/api)
