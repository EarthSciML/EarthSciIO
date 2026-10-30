---
title: "prefetch_group"
description: "Stable type · Prefetch and windows"
type: "api"
api_tabs: true
---


**Tier:** stable · **Kind:** type · **Topic:** [Prefetch and windows](/generated/api/topics/prefetch)

| Python | Julia | Rust |
|---|---|---|
| `PrefetchGroup` | `PrefetchGroup` | `PrefetchGroup` |


<div class="api-tabs">

<div class="api-tab" data-lang="python" data-label="Python">
<p class="api-tab-label">Python</p>

```python
class PrefetchGroup
PrefetchGroup(members: Mapping, *, depth: int = 2, max_bytes: int | None = None, fetch_ahead: int | None = None, workers: int = 1, windows: Sequence[Any] | None = None, transform: Callable[[str, Any], Any] | None = None, executor: str = 'thread', _decode_latency: Callable[[], float] | None = None)
PrefetchGroup.boundaries() -> list[_dt.datetime]
PrefetchGroup.start(order: Any = 'forward', from_: int | None = None) -> None
PrefetchGroup.take(k: int) -> EpochSample
PrefetchGroup.ready(k: int) -> bool
PrefetchGroup.cancel(wait: bool = False) -> None
PrefetchGroup.close() -> None
PrefetchGroup.stats() -> dict[str, Any]
```

Background decode of upcoming forcing epochs (`spec/prefetch.md` §3).

**Parameters**

- `members` (`Mapping`) — `{key: Provider}`, every one DISCRETE with an enumerable schedule
  (non-empty `refresh_times`). A
  CONST or unbounded member, or an empty map, is a `ValueError`:
  materialize a CONST source once, outside the group.
- `depth` (`int`) — Epoch slots held ahead of the consumer, decoded or decoding, counting
  the one `take` returns next (at least 1).
- `max_bytes` (`int | None`) — Optional cap on the bytes of the held slots' member samples (array
  bytes before `transform`; a sample shared by several slots counts
  once). A slot is admitted only if what is held or in flight plus its
  estimate — each member's last decoded size — fits; while some size is
  still unknown, only when nothing else is held. The head slot is always
  admitted, alone if it is oversize (`stats()["oversize_epochs"]`).
- `fetch_ahead` (`int | None`) — Slots past the decode window whose files are downloaded (not decoded)
  into the disk cache; defaults to `depth`, `0` disables.
  Best-effort: it never blocks a take, its errors are dropped (the
  decode raises the real one), and store-backed and offline members are
  skipped (there is nothing such a warm could download).
- `workers` (`int`) — Decode workers (threads, or processes with `executor="process"`).
- `windows` (`Sequence[Any] | None`) — Optional named windows: each member's data becomes
  `sample_windows_at(B[k], windows)`. Every window must satisfy
  `with_window`'s preconditions for
  every member, checked here.
- `transform` (`Callable[[str, Any], Any] | None`) — Optional `transform(key, data) -> T` run on the worker after the
  decode; its errors are stored like decode errors.
- `executor` (`str`) — `"thread"` (default) or `"process"` (members and `transform` must
  pickle; checked here).

**Members**

- `boundaries` — The sorted, de-duplicated union of the members' `refresh_times`. Epoch `k` is `[B[k], B[k+1])`; the last is open on the right.
- `start` — Declare the order epochs will be taken in, cancelling any previous one.
- `take` — Block until epoch `k` — the next entry of the order — is decoded, and return it.
- `ready` — Whether `take` `(k)` would return at once: `k` is the next entry of the order and its slot is decoded (or failed — its error is still raised only by `take`). Never blocks, never raises for a decode error; `False` before `start`, after `cancel`, and for any epoch other than the next one, decoded or not.
- `cancel` — Drop queued work at once. A decode already running (inside a C library, it cannot be interrupted) finishes and its result is discarded; `wait=True` returns only after those decodes finish. `take` is an error until the next `start`.
- `close` — `cancel(wait=True)`, then join the workers; the group is unusable afterwards. Idempotent.
- `stats` — Cumulative statistics since construction (`start` does not reset them): `waited_s`, `decode_s`, `peak_bytes`, `epochs_decoded`, `samples_decoded`, `samples_reused`, `oversize_epochs`, `overlap` (§3.9).

[Source](https://github.com/EarthSciML/EarthSciIO/blob/a49538bbd6ec318dde369d3496f6e8880586a73f/earthsciio/prefetch.py#L357)

</div>

<div class="api-tab" data-lang="julia" data-label="Julia">
<p class="api-tab-label">Julia</p>

```julia
mutable struct PrefetchGroup
PrefetchGroup(keys::Vector{String}, providers::Vector{Provider}, B::Vector{Float64}, depth::Int64, max_bytes::Union{Nothing, Int64}, fetch_ahead::Int64, nworkers::Int64, windows::Union{Nothing, Vector{Dict{String, Any}}}, transform, overlap::Bool, decode_hook, lock::ReentrantLock, cond::Base.GenericCondition{ReentrantLock}, status::Symbol, order::Vector{Int64}, next::Int64, admitted::Int64, stop_after::Int64, window::Vector{EarthSciIO._PrefetchSlot}, queue::Vector{EarthSciIO._PrefetchJob}, live::Dict{Tuple{Int64, Float64}, EarthSciIO._PrefetchJob}, last_taken::Vector{Union{Nothing, EarthSciIO._PrefetchJob}}, last_anchor::Vector{Union{Nothing, Float64}}, last_bytes::Vector{Int64}, running::Int64, workers::Vector{Task}, fetch_queue::Vector{Tuple{Int64, String}}, warmed::Set{String}, warm_through::Int64, fatal, waited_s::Float64, decode_s::Float64, peak_bytes::Int64, epochs_decoded::Int64, samples_decoded::Int64, samples_reused::Int64, oversize_epochs::Int64)
PrefetchGroup(keys, providers, B, depth, max_bytes, fetch_ahead, nworkers, windows, transform, overlap, decode_hook, lock, cond, status, order, next, admitted, stop_after, window, queue, live, last_taken, last_anchor, last_bytes, running, workers, fetch_queue, warmed, warm_through, fatal, waited_s, decode_s, peak_bytes, epochs_decoded, samples_decoded, samples_reused, oversize_epochs)
PrefetchGroup(members::AbstractDict; depth, max_bytes, fetch_ahead, workers, windows, transform, _decode_hook, _overlap)
```

```julia
PrefetchGroup(members; depth = 2, max_bytes = nothing, fetch_ahead = nothing,
              workers = 1, windows = nothing, transform = nothing)
```

Decode upcoming forcing epochs of several DISCRETE providers on background
workers, in a declared order, under a memory budget — bitwise identical to calling
[`sample_at`] / [`refresh`] synchronously (spec/prefetch.md §3).

`members` maps a key (a string; any key is taken as `string(key)`) to a DISCRETE
[`Provider`]. All members must share one time unit — the providers' own `Float64`
cadence unit — which the group cannot check. A CONST member (materialize it once,
outside the group) or an empty map is an error at construction.

  * [`boundaries`]`(g)` — the sorted union `B` of the members' `refresh_times`.
    Epoch `k` (1-based) is `[B[k], B[k+1])`; per member it is `sample_at(B[k])`, so
    a coarser member repeats its sample across the epochs that snap to one anchor
    (decoded once, served again: `samples_reused`).
  * [`start!`]`(g, order; from = nothing)` — `:forward`, `:reverse`, or an explicit
    `Vector{Int}` of epochs (repeats legal: a revolve schedule). `from = k` trims
    the order to begin at its first entry equal to `k` (a restart). Cancels any
    previous order.
  * `take!(g, k)` — blocks until epoch `k` — which must be the next entry of the
    order — is decoded, and returns its [`EpochSample`]. A stored decode error is
    raised HERE, never earlier, as the error `refresh` would raise (an offline cold
    cache raises [`CacheMiss`], not a wrapper); the order is then over.
  * `isready(g, k)` — non-blocking: would `take!(g, k)` return at once?
  * [`cancel!`]`(g; wait = false)` — drop queued work; a decode already running
    completes and is discarded (`wait = true` waits for it). Reusable after `start!`.
  * `close(g)` — `cancel!(g; wait = true)`, then join the workers; the group is
    unusable afterwards. Always close a group: its workers wait on it until then.
  * [`stats`]`(g)` — cumulative `waited_s`, `decode_s`, `peak_bytes`,
    `epochs_decoded`, `samples_decoded`, `samples_reused`, `oversize_epochs` and
    `overlap`.

The budget (§3.6): `depth` (≥ 1) bounds the epoch slots held ahead of the consumer,
decoded or decoding, counting the one `take!` returns next. `max_bytes` bounds
their bytes — a member's array bytes before `transform`, strings at their UTF-8
length, a sample shared by several slots counted once; a slot is admitted only if
it fits beside what is held, estimated from each member's last decoded size, and
while some size is still unknown only when nothing else is held. The head slot is
always admitted, so a slot larger than `max_bytes` is still decoded, alone, and
counted in `oversize_epochs`. `fetch_ahead` (default `depth`, `0` disables) warms
the disk cache — downloads, never decodes — for that many slots past the decode
window, best-effort and skipped for store-backed members. The Julia Provider holds
no decoded file, so there is no decoded-file reuse to bound beyond `max_bytes`.

`windows` (a vector of [`DimWindow`]s or wire `Dict`s) makes each member's `data`
the vector `sample_windows_at(p, B[k], windows)`; every window must pass the
[`with_window`] preconditions for every member, checked here. `transform(key,
data)` post-processes a sample on the worker; its result is `data`, and its error
is stored like a decode error.

`workers` decode tasks are spawned with `Threads.@spawn` at the first `start!`. With
`Threads.nthreads() == 1` there is nothing to overlap with: the group then decodes
inside `take!` and reports `overlap = false`. NetCDF decodes are serialised by the
reader's process-wide lock; zarr decodes run concurrently.

Two keywords exist for tests only: `_decode_hook(key, t)` is called on the worker
before each decode (to randomise decode latency, so completion order varies), and
`_overlap = false` forces the decode-inside-`take!` fallback.

[Source](https://github.com/EarthSciML/EarthSciIO/blob/a49538bbd6ec318dde369d3496f6e8880586a73f/julia/src/prefetch.jl#L88)

</div>

<div class="api-tab" data-lang="rust" data-label="Rust">
<p class="api-tab-label">Rust</p>

```rust
pub struct PrefetchGroup<T = NativeDataset> {
    /* private fields */
}

impl PrefetchGroup<NativeDataset> { pub fn builder() -> PrefetchBuilder<NativeDataset, NativeDataset> }

impl<T: Send + Sync + 'static> PrefetchGroup<T> { pub fn boundaries(&self) -> Vec<f64> }

impl<T: Send + Sync + 'static> PrefetchGroup<T> { pub fn start(&mut self, order: Order, from: Option<usize>) -> Result<()> }

impl<T: Send + Sync + 'static> PrefetchGroup<T> { pub fn take(&mut self, k: usize) -> Result<EpochSample<T>> }

impl<T: Send + Sync + 'static> PrefetchGroup<T> { pub fn ready(&self, k: usize) -> bool }

impl<T: Send + Sync + 'static> PrefetchGroup<T> { pub fn cancel(&mut self, wait_for_running: bool) }

impl<T: Send + Sync + 'static> PrefetchGroup<T> { pub fn close(&mut self) }

impl<T: Send + Sync + 'static> PrefetchGroup<T> { pub fn stats(&self) -> PrefetchStats }
```

Background decode of upcoming epochs across several providers, bitwise
identical to synchronous `Provider::refresh`. See the module documentation
and `spec/prefetch.md` §3.

```rust
use std::sync::Arc;
use earthsciio::{DimWindow, Order, PrefetchGroup};

let w = DimWindow::new().range("lat", 120, 218).range("lon", 300, 497);
let mut g = PrefetchGroup::builder()
    .member("U", Arc::new(p_u.with_window(w.clone())?))
    .member("V", Arc::new(p_v.with_window(w)?))
    .depth(2).max_bytes(8 << 30).workers(4)
    .build()?;
let b: Vec<f64> = g.boundaries();          // Unix seconds, as refresh_times
g.start(Order::Forward, None)?;
for k in 0..b.len() {
    let ep = g.take(k)?;
    let u = ep.get("U").unwrap();
    if u.changed { /* copy u.data into the forcing buffer */ }
}
// Drop cancels and joins.
```

[Source](https://github.com/EarthSciML/EarthSciIO/blob/a49538bbd6ec318dde369d3496f6e8880586a73f/rust/src/prefetch.rs#L466) · [Rust docs](https://docs.rs/earthsciio/latest/earthsciio/prefetch/struct.PrefetchGroup.html)

</div>

</div>
