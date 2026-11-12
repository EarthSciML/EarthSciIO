---
title: "Streaming writer"
description: "Streaming a run's output into a sharded Zarr v3 store: region writes from many ranks without a gather, commit tokens delivered to one owner, indexed records, statics, blobs, read-back and resume."
type: "api"
---


| Operation | Kind | Bindings | Summary |
|---|---|---|---|
| [`read_blob`](/generated/api/ref/read_blob) | function | Julia, Rust | Read the committed blob `key` of `writer` under `scope` (`:static` or `(:record, i)`). |
| [`read_output_manifest`](/generated/api/ref/read_output_manifest) | function | Julia, Rust | Read a store's output manifest. |
| [`read_record`](/generated/api/ref/read_record) | function | Julia, Rust | Read committed record `index` of streaming variable `var` straight from the store (a directory or `file://` URL; never through the cache). |
| [`read_static`](/generated/api/ref/read_static) | function | Julia, Rust | Read the committed static `name` (dims order, region-sized, stored dtype). |
| [`schema_fingerprint`](/generated/api/ref/schema_fingerprint) | function | Julia, Rust | The stream-schema fingerprint of spec/writer.md §3.1: the sha256 of the canonical JSON of dims (time length `0` in append mode, `n_records` in indexed mode), `time_dim`,… |
| [`write_blob`](/generated/api/ref/write_blob) | function | Julia | Store a small rank-local payload: `scope` is `:static` or `(:record, i)`. |
| [`write_close`](/generated/api/ref/write_close) | function | Julia | Flush any pending data and finalize: a `:single` writer commits and rewrites the manifest; the owner commits its own token plus the other writers' `tokens`; a contributor only… |
| [`write_commit`](/generated/api/ref/write_commit) | function | Julia | Owner only: verify one token per writer (spec/writer.md §5.3) and commit them; raises [`CommitError`] (writing nothing) when they do not tile the store. |
| [`write_flush`](/generated/api/ref/write_flush) | function | Julia | Write every dirty shard of this writer's region and return the token naming what was written since the last token. |
| [`write_invalidate`](/generated/api/ref/write_invalidate) | function | Julia | Owner/single, indexed mode: mark committed records uncommitted so they may be rewritten (bumps `commit_seq`). |
| [`write_open`](/generated/api/ref/write_open) | function | Julia | Open an output store at `base_url` and return an opaque write handle threaded through the other `write_*` calls. |
| [`write_record`](/generated/api/ref/write_record) | function | Julia | Append ONE time record (append mode): `t` is the time coordinate value, `arrays` maps each variable this writer supplies to its gridded `Array` (the variable's non-time dims, in… |
| [`write_record_at`](/generated/api/ref/write_record_at) | function | Julia | Write record `index` (0-based) in indexed mode, in any order; `t` must equal the schema's `times[index]` bitwise. |
| [`write_resume`](/generated/api/ref/write_resume) | function | Julia | Reopen a store after preemption (spec/writer.md §7); in append mode committed records with `t > at` are discarded. |
| [`write_static`](/generated/api/ref/write_static) | function | Julia | Write the time-less variable `name` (region-shaped when it is partitioned, full shape when replicated — owner/single only). |
| [`commit_receipt`](/generated/api/ref/commit_receipt) | type | Rust | What a successful `commit` did. |
| [`commit_token`](/generated/api/ref/commit_token) | type | Julia, Rust | What one writer wrote since its previous token (spec/writer.md §5.2), returned by [`write_flush!`] and handed (by the caller — EarthSciIO is MPI-free) to the owner's… |
| [`indexed_time`](/generated/api/ref/indexed_time) | type | Julia, Rust | The `indexed` time mode (spec/writer.md §6.2): the time axis has a fixed length `n_records = length(times)` from [`write_open!`], the owner writes `times` as the time coordinate… |
| [`region`](/generated/api/ref/region) | type | Rust | A block of the non-time dims, `dim → start..stop` (0-based, half-open). |
| [`stream_schema`](/generated/api/ref/stream_schema) | type | Julia, Rust | The input to [`write_open!`] — a plain struct in this repo (the EarthSciAST `OutputSchema` maps onto it at the binding layer; no EarthSciAST dependency here). |
| [`stream_writer`](/generated/api/ref/stream_writer) | type | Julia, Rust | The `zarr` streaming writer (sharded Zarr v3). |
| [`writer_role`](/generated/api/ref/writer_role) | type | Rust | A writer's role in a store (`spec/writer.md` §5). |

[All topics](/generated/api)
