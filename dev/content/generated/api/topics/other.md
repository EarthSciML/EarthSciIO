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
| [`provider_from_esm`](/generated/api/ref/provider_from_esm) | function | Python, Julia | A `Provider` over an esm `data_sources` entry, bound to `cache` and the run `window` (UTC datetimes). |
| [`source_from_esm`](/generated/api/ref/source_from_esm) | function | Python, Rust | The `DataSource` an esm `data_sources` entry declares (spec/esm-sources.md §3). |
| [`calendar_period`](/generated/api/ref/calendar_period) | type | Python, Julia, Rust | A cadence step of `months` calendar months plus `seconds` seconds. |
| [`esm_source_options`](/generated/api/ref/esm_source_options) | type | Rust | What the caller adds to an esm entry when building a loader from it (`DataSource::from_esm`). |
| [`esm_template`](/generated/api/ref/esm_template) | type | Python, Julia | A loader URL that is an esm template (spec/esm-sources.md §1.5). |
| [`json_reader`](/generated/api/ref/json_reader) | type | Python, Julia, Rust | The active `json` reader: a JSON array of records becomes a points table. |
| [`calendar_period_error`](/generated/api/ref/calendar_period_error) | error | Python, Julia | An ISO 8601 period EarthSciIO cannot read; the message contains `esm_period_invalid`. |
| [`esm_source_error`](/generated/api/ref/esm_source_error) | error | Python, Julia | An esm data source EarthSciIO cannot read. |
| [`esm_template_error`](/generated/api/ref/esm_template_error) | error | Python, Julia, Rust | An esm URL template could not be expanded. |

[All topics](/generated/api)
