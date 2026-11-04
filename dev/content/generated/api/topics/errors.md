---
title: "Errors"
description: "What fails and how: offline misses, integrity failures and the general error."
type: "api"
---


| Operation | Kind | Bindings | Summary |
|---|---|---|---|
| [`commit_error`](/generated/api/ref/commit_error) | type | Python, Julia | An owner's `commit` refused the delivered tokens (`spec/writer.md` §5.3): a missing or duplicate writer, a divergent fingerprint, a gap or an overlap between regions, divergent… |
| [`region_not_shard_aligned`](/generated/api/ref/region_not_shard_aligned) | type | Python, Julia | A writer region violates the alignment rule of `spec/writer.md` §4. |
| [`cache_miss`](/generated/api/ref/cache_miss) | error | Python, Julia | Raised in **offline mode** when the blob for a resolved URL is absent. |
| [`error`](/generated/api/ref/error) | error | Python, Rust | Base class for every EarthSciIO error. |
| [`integrity_error`](/generated/api/ref/integrity_error) | error | Python, Julia | A cached or freshly downloaded blob failed its checksum / size check. |

[All topics](/generated/api)
