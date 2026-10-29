# Epoch prefetch and named-dimension windows (v1)

Status: normative. Additive: the cache format stays **`v1`**, no cache key
changes, and nothing here changes what `refresh` or `materialize` return.
Proposed as RFC U11 by EarthSciIntegrator (EarthSciML/EarthSciIO#8).

This document adds four things to the Provider surface
([README.md](README.md#the-provider-surface-context-detailed-impl-is-the-per-track-beads)),
each specified once here and implemented natively in Python, Julia and Rust:

| # | Addition | What it is for |
|---|---|---|
| 1 | `sample_at(t)` | the data `refresh(t)` would return, computed without touching any provider buffer, so it may run on a worker thread |
| 2 | `select = {"dims": {...}}` and `DimWindow` | a window that names its dimensions, so ONE window covers 2-D and 3-D fields and staggered dims alike |
| 3 | `with_window(w)` and `sample_windows_at(t, [w...])` | a window bound to a provider at construction, and K windows decoded in one pass |
| 4 | `PrefetchGroup` | background decode of upcoming forcing epochs, in a caller-declared order, under a memory budget, bitwise identical to synchronous `refresh` |

**What this is not.** It is not a scheduler of solver steps and not a time
blend: interpolation stays in the model. It knows no MPI, rank, halo exchange or
scatter: a window is a plain selection and an epoch is a position in a list of
times. It is not a GPU transfer layer: samples are host arrays. It is not a
decoded-data cache across runs, and `prefetch` keeps its meaning (warm the disk
cache, decode nothing).

---

## 1. `sample_at(t)`

`sample_at(t)` returns the dataset `refresh(t)` would return — data variables
**and** coordinates — with the same snap-down to the cadence anchor, the same
file-local record resolution, the same bracket (`records_per_sample = 2`) and the
same end-of-data rules, and it raises what `refresh(t)` would raise. It:

- reads and writes **no** provider buffer (no current record, no current
  anchor, no decoded-file LRU);
- may be called **concurrently from several threads** on one provider;
- depends on `t` only through the cadence anchor `t` snaps to. Two times that
  snap to one anchor yield equal samples. (`PrefetchGroup` relies on this to
  decode a coarse-cadence member once per anchor, not once per epoch.)

A CONST provider's `sample_at(t)` is its `materialize()` result.

`refresh(t)` is re-specified as **`sample_at(t)` followed by the buffer
update**, sharing one code path, so the two cannot drift. A binding may still
feed that path from its own decoded-file buffer (Python and Rust amortise one
whole-file decode over every tick in the file). Decoding a file is
deterministic, so a buffered and a fresh decode are the same arrays.

| binding | spelling | returns |
|---|---|---|
| Python | `Provider.sample_at(t: datetime) -> NativeDataset` | exactly `refresh(t)`'s value |
| Julia | `sample_at(p::Provider, t::Real) -> NativeDataset` | exactly `materialize(p, t)`'s value (the Julia Provider holds no buffer, so this is the same function) |
| Rust | `Provider::sample_at(&self, t: OffsetDateTime) -> Result<NativeDataset>` | `variables` = what `refresh(t)` returns when it returns `Some`; `coords` = what `coords()` holds after that `refresh(t)` |

Rust: `Provider` is `Send + Sync`, and the crate asserts it at compile time.

---

## 2. Named-dimension windows

### 2.1 The `dims` form of `select`

```
select = {"dims": {"<dim>": "all" | {"slice": [start, stop, step?]} | {"indices": [i, ...]}, ...}}
```

The selector vocabulary is the existing one ([conformance.md](conformance.md#decode)
"NetCDF decode notes"): 0-based, half-open slices with `step` default 1,
`indices` returned in the order given with repetition kept, every index
bounds-checked against the FILE dimension length before anything is read (an
out-of-range or negative index is an error, never a clamp or a wrap), a
dimension never dropped (one index comes back at length 1), and an empty
selection legal (a zero-length axis kept in `dims`).

What `dims` changes is only how a selector finds its axis:

- **Exactly one of `axes` and `dims`.** A `select` carrying both is an error.
- **By name, at every rank.** Each named selector applies to every decoded
  field and every coordinate that carries that dimension, whatever the field's
  rank. There is no positional match and no rank match: a 2-D `ps(time, lat,
  lon)`, a 3-D `t(time, lev, lat, lon)` and a staggered `u(time, lat, lon_edge)`
  are all windowed by one `{"lat": …, "lon": …, "lon_edge": …}`.
- **Unnamed dimensions are `"all"`.** `{"dims": {}}` selects nothing away.
- **A named dimension that no field of the read carries is an error** naming
  the dimensions that are present. "The fields of the read" are the decoded
  data variables (after the `variables` projection) and the coordinates
  returned with them. A window never silently means nothing.
- **A consumed string length is not a dimension** of any decoded field
  ([conformance.md](conformance.md#decode), "NetCDF text variables"), so naming
  one is that same error. It is never applied (it would truncate every string).
- **Time.** A selector other than `"all"` on a time axis is refused by every
  reader: record selection is the Provider's. In `netcdf` a dimension is the
  time axis by the existing `axes` rule (a same-named coordinate with CF
  `"<step> since <ref>"` units, or a dimension named `time`); a `zarr` store
  carries no coordinate variables, so there the name is the whole rule (`time`,
  compared case-insensitively). `"time": "all"` is legal. The positional zarr
  form keeps its historical behaviour and refuses nothing; the named form is
  new, so it starts from the rule every reader shares.
- **Wraparound.** A window across a periodic longitude seam is an `indices`
  list (`[1150, 1151, 0, 1, …]`) and comes back in the order given.
- **The gate.** A `dims`-windowed read is cell-for-cell identical to the full
  read sliced by name afterwards, coordinates included — the existing gate,
  extended to the new spelling.

| reader | `axes` | `dims` | what a `dims` window saves |
|---|---|---|---|
| `netcdf` | yes | yes | decode only (same blob, same key) |
| `zarr` | yes | yes | fetch **and** decode: only the chunk objects the window intersects are fetched. Dimension names come from v2 `_ARRAY_DIMENSIONS` or v3 `dimension_names` (synthesized `dim_0…` names are nameable too) |
| every other | — | — | nothing: a `select` is an error before any fetch, as today |

A window never changes the cache key ([registries.md](registries.md) §2.2).

### 2.2 `DimWindow`

A convenience constructor for the wire form, in every binding. `Window` is
already the name of the run window `(start, end)` in Python and Rust, so the
spatial window is **`DimWindow`** everywhere.

| binding | construct | to the wire form |
|---|---|---|
| Python | `DimWindow({"lat": (120, 218), "lon": [1151, 0, 1], "lev": "all"})` — a 2- or 3-tuple is a `slice`, a list of ints is `indices`, `"all"` and selector dicts pass through | `w.select` → `{"dims": {...}}` |
| Julia | `DimWindow("lat" => (120, 218), "lon" => [1151, 0, 1])` — same meanings, **0-based** (the wire vocabulary, not Julia's) | `select_form(w)` → `Dict("dims" => ...)` |
| Rust | `DimWindow::new().range("lat", 120, 218).indices("lon", vec![1151, 0, 1])` (`.step_range(dim, a, b, s)`, `.all(dim)`) | `Selection::from(w)` → `Selection::Named(..)` |

`Selection` is `#[non_exhaustive]`, so `Selection::Named(BTreeMap<String,
AxisSelect>)` is not a breaking change. Every binding also accepts the wire
dict / `Selection` directly wherever a `DimWindow` is accepted.

### 2.3 `with_window(w)`

`with_window(w)` returns a **new** provider that shares the original's cache and
reader and has `w` baked in as its `select` (Python/Julia
`reader_kwargs["select"]`, Rust `DataSource::select`). The original is
unchanged. It is an error, raised at once and before any I/O, when:

- the provider already bakes a `select` (two windows do not compose);
- the provider's reader does not answer `supports_selection`;
- `w` names the provider's `time_dim` at all, even as `"all"` (records are the
  Provider's, and a window has no business on that axis);
- `w` is not in the named form (a positional window cannot be checked against
  `time_dim` without opening the file).

**A window is bound at construction, never per call.** A baked window keeps the
decoded-file reuse path (Python/Rust decode each windowed file once, not once
per tick); a per-call `select` stays what it is today, a peek that reads fresh.

### 2.4 `sample_windows_at(t, [w1, …, wK])`

Returns K datasets; dataset `i` is identical — bitwise, coordinates included —
to `with_window(wi).sample_at(t)`. It fetches and decodes the **bounding union**
once: per dimension, the sorted distinct union of the windows' indices (or
`"all"` when any window leaves that dimension unnamed or names it `"all"`), then
slices each window back out of the union by name, restoring its own order and
repetitions. The same preconditions as `with_window` apply to every window. An
empty list returns an empty list without fetching anything.

This is for chunk layouts where a window cannot reduce decompression (GEOS-FP
0.25° A3dyn is chunked one global level per chunk): one rank per node decodes
its neighbours' windows in one pass and the integrator scatters them.

---

## 3. `PrefetchGroup`

### 3.1 Shape

```
PrefetchGroup(members: {key: Provider}, *, depth=2, max_bytes=None, fetch_ahead=None,
              workers=1, windows=None, transform=None)
  boundaries() -> [time]          # sorted ∪ refresh_times of the members; epoch k = [B[k], B[k+1])
  start(order, from=None)         # "forward" | "reverse" | [epoch, ...]; cancels any previous order
  take(k) -> EpochSample          # blocks; k must be the next entry of the order
  ready(k) -> bool                # non-blocking
  cancel(wait=false)              # drop pending work; discard in-flight results
  close()                         # cancel(wait=true), join workers
  stats() -> {waited_s, decode_s, peak_bytes, epochs_decoded, samples_decoded,
              samples_reused, oversize_epochs, overlap}
```

### 3.2 Members and epochs

- **Members** are keyed by string and must all be DISCRETE with an enumerable
  schedule (non-empty `refresh_times`). A CONST member, an unbounded member, or
  an empty member map is an error at construction: the caller materializes a
  CONST source once, outside the group.
- All members share one time type and unit (Python `datetime`, Rust Unix
  seconds `f64`, Julia the providers' own `Float64` cadence unit). That is the
  caller's to keep: the group cannot check it in Julia.
- **`boundaries()`** is the sorted, de-duplicated union of the members'
  `refresh_times`. Epoch `k` is the interval `[B[k], B[k+1])`, and the last
  epoch is open on the right. Epoch indices are 0-based in Python and Rust and
  on the wire, 1-based in Julia.
- **What epoch `k` is, per member:** `sample_at(B[k])`. A member whose own
  cadence is coarser than the union snaps `B[k]` down to its anchor, so its
  sample repeats across several epochs; a time before a member's first anchor
  raises what that member's `refresh` would.

### 3.3 Orders

`start(order, from=None)` declares the order in which epochs will be taken, and
cancels any previous order. `order` is

- `"forward"`: `0, 1, …, n-1` (Julia `:forward`, Rust `Order::Forward`);
- `"reverse"`: `n-1, …, 0` — the adjoint's backward sweep;
- an explicit list of epochs (Julia `Vector{Int}`, 1-based; Rust
  `Order::Explicit(Vec<usize>)`) — a revolve schedule. Repeats are legal. An
  empty list or an out-of-range epoch is an error at `start`.

`from` (an epoch index) trims the order to begin at its first entry equal to
`from`; an epoch absent from the order is an error. `start("forward", from=r)`
is how a restart resumes.

`take(k)` returns the next entry's sample. Asking for any epoch other than the
next entry is an error naming the expected epoch, and consumes nothing. Asking
after the order is exhausted, before any `start`, or after `cancel` is an
error. To re-seek, call `start` again.

`take`, `start`, `ready` and `cancel` are called from one thread — the
consumer's. Only decoding runs elsewhere.

### 3.4 What `take(k)` returns

An `EpochSample`: for every member key, a `Sample`

```
Sample = { data, anchor, changed }
```

- `data` — the member's `sample_at(B[k])`; with `windows`, the list
  `sample_windows_at(B[k], windows)`; with `transform`, `transform(key, <that>)`.
  `data` is **read-only**: a binding may hand the same object to several
  epochs (a repeated anchor), so a caller that mutates copies first.
- `anchor` — the member's cadence anchor for `B[k]` (Python `datetime`, Rust
  Unix seconds `f64`, Julia the tick value from `refresh_times`).
- `changed` — whether `anchor` differs from this member's anchor at the
  previous `take` of the current order. Always true on the first `take` after
  `start`. A caller copies only the changed samples into its forcing buffers.

### 3.5 The equivalence gate (normative, bitwise)

For every member, every order, every `depth`, `workers` and `max_bytes`, and
any decode timing:

```
take(k)[key].data  ==  members[key].sample_at(B[k])  ==  what members[key].refresh(B[k]) returns
```

— variables, coordinates, attributes, dims and dtypes, bit for bit, NaN for
NaN. With `windows` the right-hand side is `[with_window(w).sample_at(B[k]) for
w in windows]`; with `transform`, `transform(key, …)` of it. Scheduling changes
**when** a result is ready, never **what** it is. Workers may finish in any
order; results are keyed by epoch position and member and are delivered only by
`take`. Decode is elementwise (no reductions), and each output cell is written
once from one value.

### 3.6 The memory budget

- **`depth`** (default 2, at least 1) bounds the epoch slots held ahead of the
  consumer — decoded or decoding — counting the one `take` returns next.
- **`max_bytes`** (default none) bounds their bytes. A slot's bytes are the
  array bytes of its members' decoded datasets (before `transform`; string
  fields at their UTF-8 length). A member sample shared by several slots counts
  once. A slot is admitted only if the bytes already held or in flight plus its
  estimate fit; the estimate is each member's last decoded size, and while some
  member's size is still unknown the slot is admitted only when nothing else is
  held or in flight.
- **The head slot is always admitted.** A slot larger than `max_bytes` is still
  decoded — alone, since nothing is admitted behind it — and counted in
  `stats().oversize_epochs`.
- **`fetch_ahead`** (default `depth`; `0` disables) warms the disk cache for the
  next `fetch_ahead` slots past the decode window: their files are downloaded,
  not decoded, so this is bounded by disk, not RAM. It is best-effort: it never
  blocks a `take`, its errors are dropped (the decode raises the real one at
  `take`), and store-backed members are skipped (their objects are known only
  once a decode resolves the chunk keys).
- **Decoded-file reuse is a binding choice** and sits outside `max_bytes`; each
  binding documents its bound. Python and Rust keep at most two decoded files
  per member (the Provider's LRU size), shared across workers, with a file
  being decoded by one worker awaited by the others rather than decoded twice.
  Julia decodes per sample with its `records` pushdown and holds no file.

### 3.7 Errors

- A decode (or `transform`) error is stored against its `(epoch position, key)`
  and raised by `take(k)` of that position — **never earlier**, never from
  `start` or `ready`. If several members fail, the error raised is the first by
  key order (byte-wise string order). It is the error `refresh` would have
  raised: an offline cold cache raises `CacheMiss`, not a wrapper.
- Scheduling stops past a failed slot: nothing behind it is admitted. After
  `take` raises a stored error the order is over; the next `take` is an error
  telling the caller to `start` again. `start(order, from=k+1)` resumes.

### 3.8 Cancellation and lifetime

- `cancel(wait=false)` drops queued work at once. A decode already running (and
  inside a C library) runs to completion and its result is discarded. With
  `wait=true` it returns after those decodes finish. The group is reusable
  after `start`.
- `close()` is `cancel(wait=true)` and joins the workers; the group is then
  unusable. Python: a context manager. Julia: `close(g)`. Rust: `Drop`.
- `start` on a group with work in flight cancels it first (`wait=false`).

### 3.9 Statistics

Cumulative from construction; `start` does not reset them.

| field | meaning |
|---|---|
| `waited_s` | seconds `take` spent blocked waiting for a slot |
| `decode_s` | worker seconds spent in `sample_at` / `sample_windows_at` / `transform` |
| `peak_bytes` | the largest number of slot bytes (§3.6) held at once |
| `epochs_decoded` | slots whose samples were produced (decoded or reused) |
| `samples_decoded` | member samples decoded |
| `samples_reused` | member samples served from one already held (a repeated anchor) |
| `oversize_epochs` | slots admitted although larger than `max_bytes` |
| `overlap` | whether decoding ran beside the consumer (false when a binding fell back to decoding inside `take`) |

### 3.10 Threads, locks and the GIL

- **Python** runs workers on threads by default (`executor="thread"`).
  `executor="process"` decodes in worker processes for when the GIL limits
  overlap; its members (and `transform`) must be picklable, which is checked at
  construction. NetCDF decodes take one process-wide lock: netCDF4/HDF5 is not
  assumed thread-safe (*assumption, to verify*: §5).
- **Julia** spawns workers with `Threads.@spawn`. With `Threads.nthreads() ==
  1` the group decodes inside `take!` and reports `overlap = false`. NetCDF
  decodes take one process-wide lock: NCDatasets/NetCDF-C is not assumed
  thread-safe (*assumption, to verify*). Zarr decodes run concurrently.
- **Rust** runs workers on `std::thread`; no async runtime is required. The
  pure-Rust NetCDF reader needs no lock, so members decode in parallel.

### 3.11 Per-binding API

**Python** (`earthsciio.prefetch`, re-exported at the top level):

```python
w = DimWindow({"lat": (120, 218), "lon": (300, 497)})
prov = {s: providers[s].with_window(w) for s in discrete}
with PrefetchGroup(prov, depth=2, max_bytes=8 << 30, workers=2, executor="thread") as g:
    B = g.boundaries()                    # list[datetime]
    g.start("forward")
    for k in range(len(B)):
        ep = g.take(k)                    # ep["U"].data, ep["U"].anchor, ep["U"].changed
    g.start("reverse")
```

**Julia** (`EarthSciIO.PrefetchGroup`):

```julia
w = DimWindow("lat" => (120, 218), "lon" => (300, 497))     # 0-based half-open
g = PrefetchGroup(Dict(s => with_window(p, w) for (s, p) in discrete); depth = 2, workers = 2)
B = boundaries(g)                     # Vector{Float64}
start!(g, :forward)                   # or :reverse, or a Vector{Int} of 1-based epochs; from = k
for k in eachindex(B); ep = take!(g, k); end   # ep["U"].data / .anchor / .changed
close(g)
```

`isready(g, k)` is `ready`, `cancel!(g; wait = false)` is `cancel`, `stats(g)`
is `stats`.

**Rust** (`earthsciio::prefetch`, re-exported at the crate root):

```rust
let w = DimWindow::new().range("lat", 120, 218).range("lon", 300, 497);
let mut g = PrefetchGroup::builder()
    .member("U", Arc::new(p_u.with_window(w.clone())?))
    .member("V", Arc::new(p_v.with_window(w)?))
    .depth(2).max_bytes(8 << 30).workers(4)
    .build()?;                            // PrefetchGroup<NativeDataset>
let b: Vec<f64> = g.boundaries();         // Unix seconds, as refresh_times
g.start(Order::Forward, None)?;           // Order::Reverse | Order::Explicit(Vec<usize>)
for k in 0..b.len() { let ep = g.take(k)?; let u = ep.get("U").unwrap(); /* u.changed */ }
// Drop cancels and joins.
```

`.windows(ws)` makes the member data `Vec<NativeDataset>`; `.transform(f)` with
`f: Fn(&str, D) -> T + Send + Sync` makes it `T`. `Sample<T>::data` is an
`Arc<T>` (shared, read-only, §3.4).

---

## 4. How an integrator drives it

```
per rank:  P[s] = provider(s).with_window(W[rank][s])      # halo included
           G = PrefetchGroup(P, depth, max_bytes, workers);  B = G.boundaries();  tape.epochs = B
forward:   G.start(forward, from = restart_epoch)
           for k: ep = G.take(k); copy changed samples into the rank's forcing buffers
                  integrate macro steps with B[k] <= t0 < tf <= B[k+1]
backward:  G.start(reverse)            # or the revolve access list
           for k descending: ep = G.take(k); per macro step in reverse: restore, replay, adjoint
```

Boundaries are solver tstops, so no step straddles one, and a step's epoch is
`searchsortedlast(B, t0)` in both directions. For a chunk layout a window cannot
shrink, one rank per node builds its group with `windows = [W[r] for r on
node]` and scatters with MPI — that choice, the scatter, and rebasing the
model's index offsets to the window origin are the integrator's. A node's ranks
share one disk cache, and concurrent fetches of one URL download once
([cache-format.md](cache-format.md)).

---

## 5. Open questions

1. The default `depth` at 0.25°: one epoch ahead or two, given the residency
   cost (+18.8 % decode at 73 GB RSS). To settle after re-measuring refresh on
   a `main` with the `variables`/`select`/`records` pushdowns.
2. Should decode keep source precision (Float32) instead of widening to
   Float64? The widening was 53 % of the measured refresh profile. A separate,
   cross-track RFC: it changes the §3 decode contract.
3. Are netCDF4-python and NetCDF-C (via NCDatasets) safe to call from several
   threads at once? If so, the Python and Julia decode locks can go.
4. Should a decoded-epoch spill for the backward sweep be an EarthSciIO feature,
   or integrator code on top of the Zarr writer?
5. Should `boundaries()` accept an explicit clip window `(t0, tf)`, or keep
   relying on each member's construction window?

---

## 6. Conformance

Decode cases (the existing runner, three tracks, [conformance.md](conformance.md) §1):

| id | pins |
|---|---|
| `named-window-mixed-rank` | a NetCDF blob with 2-D, 3-D and 4-D fields and a staggered `lon_edge` beside `lon`, windowed by one `dims` select: by name at every rank, a permuted `indices`, a stepped `slice`, the staggered dim independently, coordinates sliced with the data, CF unpacking and a masked cell inside the window, time untouched |
| `named-window-zarr` | the same window shape on a store-backed Zarr store with arrays of two ranks. Every chunk object the window does not intersect is **poisoned** (present in the cache, manifest-valid, undecodable), so a reader that fetches one fails |
| `window-wrap` | an `indices` window across the last/first longitude of the mixed-rank blob, returned in the order given |

Scenarios (`conformance/corpus/scenarios/*.json`), run by each track's own test
suite against the committed corpus, offline — they pin behaviour over a
sequence of calls, which a single decoded dataset cannot:

| id | pins |
|---|---|
| `multi-window` | `sample_windows_at` equals K separate `with_window(w).sample_at`, bitwise, for overlapping, permuted, wrapping and empty windows |
| `prefetch-equivalence` | a three-file hourly source (`records_per_sample` 1 and 2) beside a two-file two-hourly one, so the union and `changed` are exercised. For every order (forward, reverse, explicit with repeats), `depth` in {1, 2, 3}, `workers` in {1, 4} and a tiny `max_bytes`, the `take` sequence equals the synchronous `refresh` sequence bitwise, with decode latency randomized by a test hook so completion order varies |
| `prefetch-errors` | the same hourly source declared one file longer than the corpus holds: the missing file raises `CacheMiss` at `take(k)` of its first epoch and not before; `cancel` then `start(from = k)` reproduces the uninterrupted sequence; a cold cache raises `CacheMiss` at `take` |

Per binding: Rust asserts `Provider: Send + Sync` at compile time; Julia tests the
single-thread fallback; and each binding shows `stats().waited_s` falling to
about zero when synthetic compute exceeds decode time.
