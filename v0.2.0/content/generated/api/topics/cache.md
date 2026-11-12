---
title: "Cache"
description: "Fetching a resolved URL into the shared content-addressed cache, validating what is stored, and reading it back offline."
type: "api"
---


| Operation | Kind | Bindings | Summary |
|---|---|---|---|
| [`cache_key`](/generated/api/ref/cache_key) | function | Python, Julia, Rust | Return the lowercase-hex sha256 of the resolved URL. |
| [`data_dir`](/generated/api/ref/data_dir) | function | Python, Julia, Rust | Resolve the cache root directory. |
| [`file_source_is_current`](/generated/api/ref/file_source_is_current) | function | Python, Julia, Rust | Rung 0 as a yes/no: is the source **provably** the file that was ingested? |
| [`file_source_state`](/generated/api/ref/file_source_state) | function | Python, Julia, Rust | Rung 0 without a memo: what is the `file://` source behind this entry? |
| [`is_offline`](/generated/api/ref/is_offline) | function | Python, Julia, Rust | Resolve effective offline mode. |
| [`sha256_file`](/generated/api/ref/sha256_file) | function | Python, Rust | sha256 of a file's contents, streamed so large blobs don't load to RAM. |
| [`cache`](/generated/api/ref/cache) | type | Python, Julia, Rust | A content-addressed fetch cache over a pluggable `Store`. |
| [`cache_entry`](/generated/api/ref/cache_entry) | type | Python, Julia, Rust | The result of a fetch: where the blob is + how it got there. |
| [`manifest`](/generated/api/ref/manifest) | type | Python, Julia, Rust | In-memory view of `meta/<key>.json` (see the schema for field meanings). |
| [`temporal`](/generated/api/ref/temporal) | type | Python, Rust | The freshness policy derived from a loader's `temporal` block. |

[All topics](/generated/api)
