---
title: "Other"
description: "Stable operations not yet placed in a topic."
type: "api"
---


| Operation | Kind | Bindings | Summary |
|---|---|---|---|
| [`decode_coord_times`](/generated/api/ref/decode_coord_times) | function | Rust | `decode_time_axis` of a decoded coordinate: its values, `units`, `calendar` and the fill value the reader kept. |
| [`decode_time_axis`](/generated/api/ref/decode_time_axis) | function | Python, Julia, Rust | The Unix seconds (UTC, floored to the second) of a raw CF time axis, or `None` when it cannot be decoded (`spec/esm-sources.md` §2.5). |
| [`esm_template_names`](/generated/api/ref/esm_template_names) | function | Python, Julia, Rust | The substitution names `template` needs, in order of first appearance, without `date`. |
| [`expand_esm_template`](/generated/api/ref/expand_esm_template) | function | Python, Julia, Rust | Expand an esm URL template at `anchor` (a UTC instant; a naive datetime is read as UTC) with `substitutions`. |
| [`provider_from_esm`](/generated/api/ref/provider_from_esm) | function | Python, Julia | A `Provider` over an esm `data_sources` entry, bound to `cache` and the run `window` (UTC datetimes, or seconds from `time_origin`). |
| [`source_from_esm`](/generated/api/ref/source_from_esm) | function | Python, Rust | The `DataSource` an esm `data_sources` entry declares (spec/esm-sources.md §3). |
| [`window_records`](/generated/api/ref/window_records) | function | Julia | The anchors, in seconds from the provider's `time_origin` when it has one (Unix seconds otherwise), of the records that serve a `temporal` provider's run window, in order… |
| [`calendar_period`](/generated/api/ref/calendar_period) | type | Python, Julia, Rust | A cadence step of `months` calendar months plus `seconds` seconds. |
| [`esm_source_options`](/generated/api/ref/esm_source_options) | type | Rust | What the caller adds to an esm entry when building a loader from it (`DataSource::from_esm`). |
| [`esm_template`](/generated/api/ref/esm_template) | type | Python, Julia | A loader URL that is an esm template (spec/esm-sources.md §1.5). |
| [`json_reader`](/generated/api/ref/json_reader) | type | Python, Julia, Rust | The active `json` reader: a JSON array of records becomes a points table. |
| [`provider_time`](/generated/api/ref/provider_time) | type | Python, Rust | A time a provider takes: a `datetime.datetime` (an instant; naive is UTC), or a number of seconds from the provider's `time_origin` (the Unix epoch when it has none),… |
| [`calendar_period_error`](/generated/api/ref/calendar_period_error) | error | Python, Julia | An ISO 8601 period EarthSciIO cannot read; the message contains `esm_period_invalid`. |
| [`esm_source_error`](/generated/api/ref/esm_source_error) | error | Python, Julia | An esm data source EarthSciIO cannot read. |
| [`esm_template_error`](/generated/api/ref/esm_template_error) | error | Python, Julia, Rust | An esm URL template could not be expanded. |
| [`time_origin_invalid`](/generated/api/ref/time_origin_invalid) | error | Python, Julia | A provider's `time_origin` that is not a whole-second UTC instant (spec/esm-sources.md §4). |
| [`window_not_covered`](/generated/api/ref/window_not_covered) | error | Python, Julia | A provider's run window that some stretch of has no record in effect (spec/esm-sources.md §2.6): it starts before the source's first record, runs past the end of its data, or is… |

[All topics](/generated/api)
