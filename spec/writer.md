# The streaming writer contract (v2)

Status: normative. Proposed as RFC U12 by EarthSciIntegrator
(EarthSciML/EarthSciIO#9). Additive: a single-writer, append-only store with no
statics and no blobs still writes `earthsciio/output-manifest/v1`, unchanged.

This document consolidates the write boundary of the streaming-output-sinks RFC
(EarthSciAST, §5 and §16) and extends it with:

| # | Addition | What it is for |
|---|---|---|
| 1 | A Rust streaming handle (`ZarrStreamWriter`) | the Rust track gets `create / write_record / flush / close`, like Julia and Python |
| 2 | Regions | each MPI rank writes its own horizontal block of one logical array, without a gather |
| 3 | Roles and a commit protocol | one owner commits shared metadata from tokens the caller delivers; EarthSciIO stays MPI-free |
| 4 | Indexed time mode | records written by index, in any order, and rewritten (reverse-order gradients, checkpoint rings) |
| 5 | Statics, blobs, dtypes, read-back, resume | gradient fields, bitwise restart |

Every rule below is implemented natively in each binding
(`rust/src/format/zarr_stream.rs`, `julia/src/zarr_stream.jl`,
`earthsciio/backends/zarr_stream.py`). Keywords MUST / SHOULD are normative.

**What this is not.** Not MPI: tokens are plain values and the caller moves
them. Not a gather, and not shared-shard read-modify-write: misaligned regions
are refused. Not EarthSciAST's Sink protocol. Not NetCDF or parallel HDF5. Not a
codec change: the on-disk layout of every array is the v1 layout.

---

## 1. Store layout (unchanged from v1)

- `zarr_format: 3`; one `zarr.json` per group and per array.
- Every array uses the `sharding_indexed` codec: `chunk_grid.chunk_shape` is the
  SHARD shape, the sharding codec's `chunk_shape` is the inner chunk. Inner
  pipeline `[bytes(little), <profile compressor>]`, index pipeline
  `[bytes(little), crc32c]`, `index_location: "end"`.
- Shard object key: `<array>/c/<i0>/<i1>/…` (default key encoding, `/`).
- `fill_value` is `0` (`0.0` for floats). Never NaN.
- The time (record) axis may sit at any position in a variable's dims; writers
  SHOULD put it first.
- Profiles: `diagnostic` (Blosc zstd 5 + shuffle), `checkpoint` (Blosc zstd 7 +
  shuffle), `wasm` (plain v3 zstd 5). All are lossless.

## 2. dtypes

`float64`, `float32`, `int32`, `int64`, `uint8` MUST be supported for streaming
variables and statics in all three bindings (Julia additionally keeps
`uint32`/`uint64`). Values round-trip **bitwise**. The time coordinate is
`float64`.

## 3. The stream schema

A `StreamSchema` is the v1 output schema without data, plus:

| Field | Meaning |
|---|---|
| `dims` | ordered `name => length`. The time dim's length is ignored in `append` mode and MUST equal `n_records` in `indexed` mode (bindings MAY fill it in). |
| `time_dim` | the record axis |
| `chunk_shape`, `shard_shape` | per dim; every shard length a nonzero multiple of the inner chunk length |
| `coords` | ordered static coordinates `name => (values, attrs)`. An entry for `time_dim` supplies the time coordinate's attrs (its values are ignored). |
| `vars` | ordered streaming variables `name => (dims, dtype, attrs)`; dims MUST include `time_dim` |
| `statics` | ordered time-less variables `name => (dims, dtype, attrs)`; dims MUST NOT include `time_dim` |
| `time_mode` | `append` (default) or `indexed{n_records, times}` with `len(times) == n_records` |
| `group_attrs`, `profile` | as v1 |

### 3.1 Schema fingerprint

`fingerprint = "sha256:" + hex(sha256(canonical_json(F)))` where `F` is

```json
{
  "chunk_shape": {"<dim>": <int>, ...},
  "dims":        [["<dim>", <len>], ...],
  "profile":     "<profile>",
  "shard_shape": {"<dim>": <int>, ...},
  "statics":     [["<name>", ["<dim>", ...], "<dtype>"], ...],
  "time_dim":    "<dim>",
  "time_mode":   "append" | "indexed",
  "vars":        [["<name>", ["<dim>", ...], "<dtype>"], ...]
}
```

`dims` keeps schema order with the time dim's length set to `0` in `append` mode
and to `n_records` in `indexed` mode. `vars`/`statics` keep schema order.
`canonical_json` sorts object keys at every level, emits no whitespace
(separators `,` and `:`), integers in shortest decimal form, and strings as JSON
strings with non-ASCII escaped as `\uXXXX`. It contains no floats, so it is
reproducible in every language. Python: `json.dumps(F, sort_keys=True,
separators=(",", ":"), ensure_ascii=True)`.

## 4. Regions

A region is a mapping `{dim: [start, stop)}` over **non-time** dims, 0-based,
half-open. An omitted dim means the full extent; the empty region is the whole
array. Wire form: a JSON object `{"lat": [0, 48]}`; the canonical order is by
dim name.

- **Region dims.** The dims named in a writer's region are its *region dims*.
  Naming a dim at its full extent is allowed. All writers of one store MUST name
  the same set of region dims (checked at commit).
- **Partitioned and replicated.** A variable or static is *partitioned* when its
  dims include at least one region dim. Every writer supplies a partitioned
  variable shaped to its region (its dims minus the time dim, each region dim
  replaced by `stop - start`). Every other variable is *replicated*: only the
  owner (or a `Single` writer) supplies it, at full shape. A contributor that
  supplies a replicated variable gets an error. The time coordinate is
  replicated. A contributor's `write_record` arrays therefore contain exactly the
  partitioned variables.
- **Alignment.** For every region dim `d`: `0 <= start < stop <= len(d)`,
  `start % shard[d] == 0`, and `stop % shard[d] == 0 or stop == len(d)`.
  A violation raises **`RegionNotShardAligned`** at `create`/`join`/`resume`,
  before any I/O. Consequently every shard object of a partitioned array is
  owned by exactly one writer: no read-modify-write, no locks.
- **Partly partitioned arrays.** A partitioned variable that lacks some region
  dim (e.g. `[time, lon]` under a `{lat, lon}` split) is supplied by every
  writer, region-shaped over the region dims it has, but its shard objects are
  written only by the writer whose region starts at 0 on every region dim the
  variable lacks. Every shard object still has exactly one writer.
- Inner chunks are unconstrained: only shards are the unit of ownership.

## 5. Roles

| Role | Writer id | Writes |
|---|---|---|
| `Single` | 0 | everything; `flush` commits |
| `Owner{n_writers}` | 0 | group, coordinates, every array's `zarr.json`, replicated variables, its own region of partitioned ones, the manifest |
| `Contributor{id}` | `1 <= id < n_writers` | its region of partitioned variables and statics, its own blobs. **Never** a shared object (`zarr.json`, coordinates, manifest). |

`Single` is `Owner{1}` with the whole region that also commits on every flush.

### 5.1 Protocol

```
owner:         create(schema, Owner{n}, region_0)       # group, coords, array zarr.json, manifest v2 (commit_seq 0)
caller:        barrier
contributors:  join(store, Contributor{r}, region_r)    # read the manifest, rebuild + fingerprint-check the schema
all writers:   write_record / write_record_at / write_static / write_blob ...
all writers:   tok_r = flush()                          # PUT every dirty shard of the region; return a token
caller:        deliver tokens to the owner (MPI_Gather in rank order, or in memory)
owner:         commit([tok_0, ..., tok_{n-1}])          # verify; then shapes, manifest LAST
```

`create` for `Single` writes no manifest when the store is v1-shaped (§8); a
v2 `Single` store writes the v2 manifest at `create`. `join` is the only way a
contributor opens a store: it reads `output_manifest.json` (which MUST be v2),
rebuilds the stream schema from it (§8.2), checks the fingerprint (and, in the
bindings whose open takes a schema, that the caller's schema has the same
fingerprint), and validates its region. A missing or v1 manifest is an error.

A `Single` writer's `flush` commits only when something is pending (records,
statics or blobs written since the last commit); otherwise it is a no-op that
returns an empty token. An owner's `commit` always commits (and bumps
`commit_seq`), even when every token is empty.

`close(tokens)`: for a `Single` writer, `flush()` and then rewrite the manifest
at the same `commit_seq`. For the owner, `flush()` its own pending data, then
`commit([own_token] ++ tokens)` where `tokens` are the **other** writers'
tokens, and return the manifest. For a `Single` writer `tokens` is empty. A
contributor's `close` takes no tokens; it errors if anything written has not
been flushed (deliver the token first).

### 5.2 The commit token

A `CommitToken` is a documented JSON object (mixed-language writers on one store
are permitted by the format but not tested in v1):

```json
{
  "schema": "earthsciio/commit-token/v1",
  "writer": 1,
  "region": {"lat": [2, 4]},
  "fingerprint": "sha256:…",
  "records": [{"index": 0, "t": 0.0}, {"index": 1, "t": 3600.0}],
  "statics": ["dJ_du0"],
  "blobs":   [{"scope": "record", "record": 1, "key": "rng"}],
  "objects": ["temperature/c/0/1/0", "…"]
}
```

`records` are the record indices written since the writer's last token, sorted
ascending, each with its `t`. `statics` and `blobs` likewise (statics sorted by
name, blobs by `(scope, record, key)`). `objects` lists the store keys PUT (or,
for an all-fill shard, erased) since the last token, sorted; it is advisory.
`t` round-trips through JSON exactly (shortest round-trip float formatting).

### 5.3 What `commit` verifies

All of the following, before writing anything. On any failure it raises
**`CommitError`** naming the gap, the overlap or the divergent writer, writes
nothing, and the committed state is unchanged.

1. Exactly one token per writer id `0 .. n_writers-1`.
2. Every token's fingerprint equals the owner's.
3. Every token names the same set of region dims; each region is aligned; the
   regions are pairwise disjoint (else *overlap* between writers `a` and `b`)
   and their volumes over the region dims sum to the full extent (else *gap*).
4. Every token carries the same record indices with bitwise-equal `t`.
   In `append` mode they MUST be exactly `n_committed .. n_committed+k-1`
   (the contiguous continuation of the committed prefix). In `indexed` mode
   each `t` MUST equal `times[index]` bitwise, and no index may currently be
   committed (see §6.2).
5. Every partitioned static in any token appears in every token; a replicated
   static appears only in the owner's token. No static may currently be
   committed.

Then, in order: grow `shape[time]` of the time coordinate and every streaming
variable to the new committed count (`append` mode only), update the manifest's
record / static / blob state, increment `commit_seq`, and write
`output_manifest.json` **last**. The manifest write is the commit point:
readers trust only the manifest and the `zarr.json` shapes, so objects outside
the committed state are invisible and are overwritten later.

A commit with no records, statics or blobs is valid and only bumps `commit_seq`.

### 5.4 Writes

- Each data object is atomic: stage-and-rename on a filesystem, a single PUT on
  an object store.
- A writer encodes and writes whole shards from its own buffers. A partial time
  shard is written padded with fill and rewritten whole when more records
  arrive. A fully-covered time shard is written as soon as it fills.
- A writer that opens (`join` or `resume`) an `append` store whose committed
  count `n` ends inside a time shard (`n % shard[time] != 0`) MUST first load its
  region of that shard's committed records `[n - n % shard[time], n)` from the
  store. Its next write of that shard rewrites them unchanged; without the
  reload it would overwrite committed records with fill.
- A writer MAY write shard objects beyond the committed `shape[time]` (it uses
  an in-memory shape that covers them); it MUST NOT write `zarr.json`.

## 6. Time modes

### 6.1 `append`

The record index is the running count. `write_record(t, arrays)` returns the
index. `t` values MUST be strictly increasing (append order is time order).

### 6.2 `indexed{n_records, times}`

The time axis has length `n_records` from `create`, and the owner writes the
time coordinate (`times`) at `create`. `shape[time]` never changes.

- `write_record_at(i, t, arrays)` accepts any order; `t` MUST equal `times[i]`
  bitwise; `0 <= i < n_records`.
- A time shard with unwritten records is flushed padded with fill and
  rewritten when they arrive; the writer keeps open shards' buffers. When a
  writer writes into a time shard whose buffer it no longer holds, it reloads
  its region of that shard from the store first.
- The manifest records, per record: `committed`, `t`, `generation` (the number
  of times the record has been committed; `0` = never).
- **Rewrite rule.** Writing a record that is currently committed is an error at
  `write_record_at`: the owner must first `invalidate([i, …])`, which marks the
  records uncommitted and writes the manifest (bumping `commit_seq`). A
  contributor knows the committed set from the manifest it read at `join` plus
  the records it has tokened since; when that view says `i` is committed it
  re-reads the manifest before raising. The same rule applies to statics via
  `invalidate_statics([name, …])`.

`write_record` (no index) is an error in `indexed` mode, and `write_record_at`
is an error in `append` mode.

## 7. Statics, blobs, read-back, resume

- **`write_static(name, array)`** writes a time-less variable. Its array is
  region-shaped when partitioned, full-shaped when replicated (owner only). It is
  encoded and written immediately. The static's `zarr.json` (full shape) is
  written by the owner at `create`.
- **`write_blob(scope, key, bytes)`** stores a small rank-local payload at
  `blobs/static/<writer>/<key>` (scope `static`) or
  `blobs/record/<i>/<writer>/<key>` (scope `record(i)`). `key` matches
  `[A-Za-z0-9._-]+` and is not `.` or `..`. Blobs are committed like records (they appear in the
  token and, once committed, in the manifest). A blob is one object written
  atomically, so rewriting a committed blob is allowed and replaces it.
  `invalidate([i, …])` also uncommits the `record(i)` blobs.
- **Read-back** reads committed data directly from the store, never through the
  content-addressed cache:
  - `read_output_manifest(store)` → v1 or v2 manifest (parsed).
  - `read_record(store, var, i, region=None)` → the region of record `i`
    (shape: var dims minus time, region-sized). Errors if `i` is not committed.
  - `read_static(store, name, region=None)`; errors if not committed.
  - `read_blob(store, scope, writer, key)`; errors if not committed.
- **`resume(store, role, region, at=None)`** reopens a store after preemption.
  - `append`: the committed records are truncated to those with `t <= at`
    (`at = None` keeps all). When that drops at least one record, the
    owner/`Single` writer rewrites the shapes and the manifest (incrementing
    `commit_seq` in v2); otherwise it writes nothing. Every writer computes the same
    truncated count from the manifest and reloads its region of the open time
    shard, so later flushes rewrite whole shards. Re-emitted records are bitwise
    identical under D10, so truncate-and-rewrite is idempotent.
  - `indexed`: nothing is truncated; `at` MUST be `None`.
  - Uncommitted objects left by the dropped handles are ignored and overwritten.

## 8. The output manifest

### 8.1 Which version

A store writes **`earthsciio/output-manifest/v1`** (byte-compatible with what
each binding wrote before this spec, apart from `created_at`) when ALL hold:
role `Single`, `append` mode, no statics in the schema. `write_blob` on such a
v1 store raises (blobs need a v2 store: use `Owner{1}`, which is otherwise
equivalent). Resuming keeps the version. Every other
store writes **`earthsciio/output-manifest/v2`**. A store never changes version.
Readers MUST pin the label exactly.

### 8.2 `v2`

`v2` contains every `v1` field, in the Julia/Rust spelling, plus the v2 fields:

| Field | Meaning |
|---|---|
| `schema` | `"earthsciio/output-manifest/v2"` |
| `base_url`, `format` (`"zarr"`), `zarr_format` (3), `profile`, `codec`, `time_dim`, `chunk_shape`, `shard_shape`, `created_at` | as v1 |
| `dims` | `[{name, length}]`, the time dim's length = the array's `shape[time]` (committed count in `append`, `n_records` in `indexed`) |
| `vars` | `[{name, dims, dtype, attrs?}]` |
| `time_shards` | `[{index, t_start, t_end, n}]` over **committed** records: in `indexed` mode one entry per time shard holding ≥1 committed record, `n` = its committed count, `t_start`/`t_end` = min/max committed `t` |
| `last_t` | the largest committed `t`, or `null` |
| `n_records` | `shape[time]` (as `dims`) |
| `schema_fingerprint` | §3.1 |
| `time_mode` | `{"kind": "append"}` or `{"kind": "indexed", "n_records": N}` |
| `writers` | `[{id, region}]`, ordered by id, as of the last commit (at `create`: the owner only) |
| `records` | `{"committed": [[start, stop), …]}` — sorted disjoint half-open ranges of committed indices. In `indexed` mode also `"entries": [{index, t, committed, generation}]`, one per index `0..N-1`. |
| `statics` | `[{name, dims, dtype, committed, generation, attrs?}]` in schema order |
| `blobs` | `[{scope, record, writer, key}]` committed blobs, sorted by `(scope, record, writer, key)`; `record` is `null` for `static` |
| `commit_seq` | number of commits (and invalidations, and resume truncations) so far; `0` at `create` |

`join` and `resume` rebuild the stream schema from `dims`, `time_dim`, `vars`,
`statics`, `chunk_shape`, `shard_shape`, `profile`, `time_mode` and (indexed)
`records.entries[*].t`. JSON Schema: [schemas/output-manifest.schema.json](schemas/output-manifest.schema.json)
(v1 and v2).

## 9. Per-binding API

### Rust — `earthsciio::format::zarr_stream`

```rust
pub enum TimeMode { Append, Indexed { n_records: usize, times: Vec<f64> } }
pub enum Role { Single, Owner { n_writers: usize }, Contributor { id: usize } }
pub struct Region(pub Vec<(String, std::ops::Range<usize>)>);   // empty = whole extent
pub enum StreamDType { F64, F32, I32, I64, U8 }   // `DType` is the reader's
pub enum TypedArray<'a> { F64(&'a [f64]), F32(&'a [f32]), I32(&'a [i32]), I64(&'a [i64]), U8(&'a [u8]) }
pub type RecordArrays<'a> = BTreeMap<String, TypedArray<'a>>;     // row-major, region shape, no time dim
pub enum BlobScope { Static, Record(u64) }

impl<S: ReadableWritableStorageTraits + 'static> ZarrStreamWriter<S> {
    pub fn create(store: Arc<S>, base_url: &str, schema: StreamSchema, role: Role, region: Region) -> Result<Self>;
    pub fn join(store: Arc<S>, base_url: &str, role: Role, region: Region) -> Result<Self>;
    pub fn resume(store: Arc<S>, base_url: &str, role: Role, region: Region, at: Option<f64>) -> Result<Self>;
    pub fn write_record(&mut self, t: f64, arrays: &RecordArrays) -> Result<u64>;
    pub fn write_record_at(&mut self, index: u64, t: f64, arrays: &RecordArrays) -> Result<()>;
    pub fn write_static(&mut self, name: &str, array: TypedArray<'_>) -> Result<()>;
    pub fn write_blob(&mut self, scope: BlobScope, key: &str, bytes: &[u8]) -> Result<()>;
    pub fn flush(&mut self) -> Result<CommitToken>;                         // Single: also commits
    pub fn invalidate(&mut self, indices: &[u64]) -> Result<()>;            // owner / Single
    pub fn invalidate_statics(&mut self, names: &[&str]) -> Result<()>;     // owner / Single
    pub fn commit(&mut self, tokens: &[CommitToken]) -> Result<CommitReceipt>;   // owner
    pub fn close(self, tokens: &[CommitToken]) -> Result<Option<Value>>;    // owner/Single: Some(manifest)
}
pub fn read_output_manifest<S>(store: &S) -> Result<Value>;
pub fn read_record<S>(store: Arc<S>, var: &str, index: u64, region: &Region) -> Result<TypedVec>;   // TypedVec: owned TypedArray
pub fn read_static<S>(store: Arc<S>, name: &str, region: &Region) -> Result<TypedVec>;
pub fn read_blob<S>(store: &S, scope: BlobScope, writer: usize, key: &str) -> Result<Vec<u8>>;
```

On `wasm32` only `Role::Single` is accepted. `write_zarr_v3` and
`write_all_to_store` are `create(Single)` → one `write_record` per time step →
`close`.

### Julia

`write_open!(w, store, base_url, schema; role = :single, n_writers = 1, id = 0,
region = nothing)` (roles `:single`, `:owner`, `:contributor`; a contributor's
open reads the manifest and checks `schema`'s fingerprint); `write_record!(w, h,
t, arrays; region = nothing)` (a non-`nothing` `region` must equal the bound
region, and the call returns the record index); `write_record_at!`,
`write_static!`, `write_blob!`, `write_flush!` (returns a `CommitToken`),
`write_commit!`, `write_invalidate!`, `write_invalidate_statics!`,
`write_resume`, `write_close!(w, h, tokens = CommitToken[])`, `read_record`,
`read_static`, `read_blob`, `read_output_manifest`. `OutputSchema` gains
`statics`, `time_mode` (`:append` or `IndexedTime(times)`) and `UInt8`.
Regions use the wire form `"lat" => (0, 48)`, 0-based half-open. Single-writer
v1 behaviour and its locks are unchanged; multi-writer stores take no metadata
locks (the owner is the only metadata writer).

### Python

`ZarrWriter.write_open(base_url, schema, role="single", n_writers=1, id=0,
region=None)`; `write_record(h, t, arrays, region=None)` → index;
`write_record_at`, `write_static`, `write_blob`, `write_flush(h) ->
CommitToken`, `write_commit(h, tokens)`, `write_invalidate`,
`write_invalidate_statics`, `write_resume(base_url, role=…, n_writers=…, id=…,
region=…, at=None)`, `write_close(h, tokens=())`; module functions
`read_output_manifest`, `read_record`, `read_static`, `read_blob`. Regions are
`{"lat": (0, 48)}`. `OutputSchema` gains `statics`, `time_mode`
(`"append"` or `IndexedTime(times)`) and `uint8`. Errors:
`earthsciio.errors.RegionNotShardAligned`, `earthsciio.errors.CommitError`.

## 10. Conformance (`conformance/write_spec_v2.json`)

The v2 write corpus is a list of **cases**; each case is a stream schema plus a
script of operations that a per-language driver executes against a fresh store.
Data values are generated, not stored, so the corpus stays small.

**Generator.** For a variable with integer `seed`, record `r` (`r = 0` for a
static) and global non-time indices `(i_0, …, i_{k-1})` in the variable's dim
order (time excluded):

```
base = seed + 13*r + Σ_j (i_j + 1) * P[j],   P = [3, 5, 11, 17, 23]
value = base + 0.25   (float32, float64)
      = base          (int32, int64)
      = base % 251    (uint8)
```

All values are exact in every dtype. A driver builds each array for its
writer's region from this formula.

**Operations** (`writer` names a handle; `expect_error` ∈
`{"RegionNotShardAligned", "CommitError", "Error"}` marks an op that MUST fail
and leave the store's committed state unchanged):

| op | fields |
|---|---|
| `create` | `writer`, `role` (`"single"` / `{"owner": n}`), `region` |
| `join` | `writer`, `role` (`{"contributor": id}`), `region` |
| `resume` | `writer`, `role`, `region`, `at` (number or null) |
| `write_record` | `writer`, `t`, `r` (generator record number) |
| `write_record_at` | `writer`, `index`, `r` (`t` is `times[index]`) |
| `write_static` | `writer`, `name` |
| `write_blob` | `writer`, `scope` (`"static"`/`"record"`), `record`, `key`, `hex` |
| `flush` | `writer`, `token` (name under which the returned token is kept) |
| `commit` | `writer`, `tokens` (names) |
| `invalidate` | `writer`, `indices` |
| `close` | `writer`, `tokens` (names; the other writers' tokens) |
| `drop` | `writer` (discard the handle without flushing) |

**Expected state.** Each case lists `expect`: the committed record indices with
the generator record `r` each holds, the committed statics, the committed blobs
(`writer`, `key`, `hex`), the manifest `schema` label, and `commit_seq`. Each
driver writes `<out>/<case>/store/`; each reader driver reads every store's
committed data through its read-back API into `earthsciio/write-v2-dump/v1`;
`conformance/crosscheck_write_v2.py` compares every dump against the oracle
**bitwise** (all profiles are lossless and dtypes are preserved).

Cases: `region-tiling` (one writer vs 2×2 region writers, decoded arrays
bitwise equal), `region-misaligned` (error at open), `region-gap` and
`region-overlap` (`CommitError`, committed prefix unchanged), `indexed-reverse`
(descending `write_record_at` equals forward append), `partial-shard-resume`
(flush mid-shard, drop uncommitted handles, resume, continue = uninterrupted),
`truncate-on-resume`, `dtypes` (f32/i32/i64/u8), `statics-and-blobs`,
`manifest-v1-compat` (a single-writer append store's manifest equals the v1
manifest, `created_at` aside).

## 11. Open questions (from the RFC, unresolved)

1. `commit` does not HEAD-check the objects named in tokens (cheap on a
   filesystem, costly on S3); `objects` is advisory.
2. `fill_value` stays 0.
3. Whether EarthSciAST's tolerance-consistent checkpoint sink adopts this layout.
4. Tokens are documented JSON; mixed-language multi-writer is not tested in v1.
5. Encoding on a background thread is not done; `write_record` may block on
   compression when a time shard fills.
