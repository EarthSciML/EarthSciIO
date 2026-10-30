# Epoch prefetch (spec/prefetch.md §3): decode upcoming forcing epochs on
# background workers, in an order the caller declares, under a memory budget —
# and hand back exactly what synchronous `refresh` would have. Scheduling changes
# WHEN a result is ready, never WHAT it is: every sample is a member's own
# `sample_at`, results are keyed by (order position, member) and delivered only by
# `take!`, and an error is stored against its slot and raised by the `take!` that
# reaches it.
#
# It is not a scheduler of solver steps, a time blend, an MPI layer or a GPU
# transfer layer: an epoch is a position in a list of times, a window is a plain
# selection, and samples are host arrays (spec/prefetch.md, "What this is not").
#
# Shape of the implementation. One `ReentrantLock` + `Threads.Condition` guards
# all scheduling state. The consumer (`start!`/`take!`/`isready`/`cancel!`, one
# thread) and the workers (`Threads.@spawn`) only ever touch that state under the
# lock; the decode itself — `sample_at`, which reads no shared mutable state — runs
# outside it. A slot is an ORDER POSITION (an explicit order may repeat an
# epoch); a job is one member's sample at one cadence anchor, shared by every slot
# whose epoch snaps to that anchor, so a coarse member is decoded once per anchor,
# not once per epoch.

"""
    Sample{T}

One member's sample for one epoch, as [`take!`](@ref take!(::PrefetchGroup, ::Integer))
returns it inside an [`EpochSample`]:

  * `data` — the member's [`sample_at`]`(B[k])` (a `NativeDataset`); with
    `windows`, the `Vector` [`sample_windows_at`]`(B[k], windows)`; with
    `transform`, `transform(key, <that>)`. **Read-only**: the same object is handed
    to every epoch that shares the anchor, so a caller that mutates copies first.
  * `anchor` — the member's cadence anchor for `B[k]`: the tick value from its
    `refresh_times` that `B[k]` snaps down to.
  * `changed` — whether `anchor` differs from this member's anchor at the
    previous `take!` of the current order; always `true` on the first `take!`
    after `start!`. A caller copies only the changed samples into its forcing
    buffers."""
struct Sample{T}
    data::T
    anchor::Float64
    changed::Bool
end

"""
    EpochSample

What [`take!`](@ref take!(::PrefetchGroup, ::Integer)) returns for one epoch: a
[`Sample`] per member, indexed by member key (`ep["U"].data`, `ep["U"].anchor`,
`ep["U"].changed`). `epoch` is the (1-based) epoch index and `time` its boundary
`B[epoch]`. Iterates as `key => Sample` pairs."""
struct EpochSample
    epoch::Int
    time::Float64
    samples::Dict{String,Sample}
end
Base.getindex(e::EpochSample, key) = e.samples[string(key)]
Base.haskey(e::EpochSample, key) = haskey(e.samples, string(key))
Base.keys(e::EpochSample) = keys(e.samples)
Base.length(e::EpochSample) = length(e.samples)
Base.iterate(e::EpochSample, s...) = iterate(e.samples, s...)
Base.show(io::IO, e::EpochSample) =
    print(io, "EpochSample(epoch=", e.epoch, ", time=", e.time, ", members=",
          sort!(collect(keys(e.samples))), ")")

# One member's sample at one cadence anchor. Shared by every slot in the window
# whose epoch snaps to that anchor (`refs` counts them).
mutable struct _PrefetchJob
    member::Int                 # index into the group's sorted members
    anchor::Float64             # NaN when the anchor itself could not be resolved
    t::Float64                  # the boundary it is decoded at
    status::Symbol              # :queued | :running | :done | :failed | :cancelled
    data::Any
    bytes::Int                  # §3.6 bytes of the decoded sample (before transform)
    err::Any
    refs::Int
end

# One position of the declared order, admitted into the decode window.
mutable struct _PrefetchSlot
    pos::Int
    epoch::Int
    jobs::Vector{_PrefetchJob}  # one per member, in key order
    counted::Bool               # its completion has been folded into the stats
end

"""
    PrefetchGroup(members; depth = 2, max_bytes = nothing, fetch_ahead = nothing,
                  workers = 1, windows = nothing, transform = nothing)

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
`_overlap = false` forces the decode-inside-`take!` fallback."""
mutable struct PrefetchGroup
    keys::Vector{String}                      # byte-wise sorted member keys
    providers::Vector{Provider}               # in key order
    B::Vector{Float64}
    depth::Int
    max_bytes::Union{Nothing,Int}
    fetch_ahead::Int
    nworkers::Int
    windows::Union{Nothing,Vector{Dict{String,Any}}}
    transform::Any
    overlap::Bool
    decode_hook::Any
    lock::ReentrantLock
    cond::Threads.Condition
    # the current order
    status::Symbol                 # :new | :running | :failed | :cancelled | :closed
    order::Vector{Int}
    next::Int                      # the order position `take!` delivers next
    admitted::Int                  # the last order position admitted to the window
    stop_after::Int                # never admit past this position (a failed slot)
    window::Vector{_PrefetchSlot}  # admitted, not yet taken; window[1].pos == next
    queue::Vector{_PrefetchJob}    # decode jobs waiting for a worker
    live::Dict{Tuple{Int,Float64},_PrefetchJob}       # (member, anchor) -> job in the window
    last_taken::Vector{Union{Nothing,_PrefetchJob}}   # per member: its last delivered sample
    last_anchor::Vector{Union{Nothing,Float64}}       # per member, for `changed`
    last_bytes::Vector{Int}                           # per member; -1 = not yet known
    running::Int                   # decodes in progress (cancelled ones included)
    workers::Vector{Task}
    fetch_queue::Vector{Tuple{Int,String}}            # (member, url) to warm
    warmed::Set{String}
    warm_through::Int              # order positions whose files were queued to warm
    fatal::Any                     # a scheduler bug caught on a worker
    # statistics (§3.9), cumulative from construction
    waited_s::Float64
    decode_s::Float64
    peak_bytes::Int
    epochs_decoded::Int
    samples_decoded::Int
    samples_reused::Int
    oversize_epochs::Int
end

function PrefetchGroup(members::AbstractDict; depth::Integer = 2, max_bytes = nothing,
                       fetch_ahead = nothing, workers::Integer = 1, windows = nothing,
                       transform = nothing, _decode_hook = nothing,
                       _overlap::Bool = Threads.nthreads() > 1)
    isempty(members) && throw(ArgumentError(
        "a PrefetchGroup needs at least one member"))
    byname = Dict{String,Provider}()
    for (k, p) in members
        key = string(k)
        haskey(byname, key) && throw(ArgumentError(
            "two members share the key \"$key\" (keys are compared as strings)"))
        p isa Provider || throw(ArgumentError(
            "member \"$key\" is a $(typeof(p)), not a Provider"))
        p.cadence == DISCRETE || throw(ArgumentError(
            "member \"$key\" is CONST: a PrefetchGroup schedules DISCRETE providers " *
            "only — materialize a CONST source once, outside the group"))
        isempty(p.times) && throw(ArgumentError(
            "member \"$key\" has no refresh times to schedule"))
        byname[key] = p
    end
    depth >= 1 || throw(ArgumentError("depth must be at least 1, got $depth"))
    workers >= 1 || throw(ArgumentError("workers must be at least 1, got $workers"))
    max_bytes === nothing || (max_bytes isa Integer && max_bytes >= 0) || throw(ArgumentError(
        "max_bytes must be `nothing` or a non-negative integer, got $(repr(max_bytes))"))
    fa = fetch_ahead === nothing ? Int(depth) : fetch_ahead
    (fa isa Integer && fa >= 0) || throw(ArgumentError(
        "fetch_ahead must be `nothing` or a non-negative integer, got $(repr(fetch_ahead))"))
    keys_sorted = sort!(collect(keys(byname)))      # String `<` is byte-wise order
    provs = Provider[byname[k] for k in keys_sorted]
    wins = nothing
    if windows !== nothing
        wins = Dict{String,Any}[select_form(w) for w in windows]
        for (k, p) in zip(keys_sorted, provs), w in wins
            try
                _check_window(p, w)
            catch err
                err isa ArgumentError || rethrow()
                throw(ArgumentError("member \"$k\": " * err.msg))
            end
        end
    end
    B = sort!(unique!(reduce(vcat, (p.times for p in provs); init = Float64[])))
    nm = length(provs)
    lk = ReentrantLock()
    return PrefetchGroup(keys_sorted, provs, B, Int(depth),
                         max_bytes === nothing ? nothing : Int(max_bytes), Int(fa),
                         Int(workers), wins, transform, _overlap, _decode_hook,
                         lk, Threads.Condition(lk),
                         :new, Int[], 1, 0, typemax(Int), _PrefetchSlot[], _PrefetchJob[],
                         Dict{Tuple{Int,Float64},_PrefetchJob}(),
                         Union{Nothing,_PrefetchJob}[nothing for _ in 1:nm],
                         Union{Nothing,Float64}[nothing for _ in 1:nm], fill(-1, nm),
                         0, Task[], Tuple{Int,String}[], Set{String}(), 0, nothing,
                         0.0, 0.0, 0, 0, 0, 0, 0)
end

Base.show(io::IO, g::PrefetchGroup) =
    print(io, "PrefetchGroup(", length(g.keys), " members, ", length(g.B),
          " epochs, depth=", g.depth, ", status=:", g.status, ")")

"""
    boundaries(g::PrefetchGroup) -> Vector{Float64}

The sorted, de-duplicated union `B` of the members' `refresh_times`, in their own
cadence unit. Epoch `k` (1-based) is `[B[k], B[k+1])`, the last one open on the
right. Boundaries are solver tstops, so no step straddles one, and a step's epoch
is `searchsortedlast(B, t0)` in both directions (spec/prefetch.md §4)."""
boundaries(g::PrefetchGroup) = copy(g.B)

"""
    stats(g::PrefetchGroup) -> NamedTuple

Cumulative from construction (`start!` does not reset them; spec/prefetch.md §3.9):

| field | meaning |
|---|---|
| `waited_s` | seconds `take!` spent blocked waiting for a slot (in the single-thread fallback, its own inline decode) |
| `decode_s` | worker seconds in `sample_at` / `sample_windows_at` / `transform` |
| `peak_bytes` | the most slot bytes (§3.6) held at once |
| `epochs_decoded` | slots whose samples were all produced (decoded or reused) |
| `samples_decoded` | member samples decoded |
| `samples_reused` | member samples served from one already held (a repeated anchor) |
| `oversize_epochs` | slots admitted although larger than `max_bytes` |
| `overlap` | whether decoding ran beside the consumer (`false` in the fallback) |"""
stats(g::PrefetchGroup) = lock(g.lock) do
    (waited_s = g.waited_s, decode_s = g.decode_s, peak_bytes = g.peak_bytes,
     epochs_decoded = g.epochs_decoded, samples_decoded = g.samples_decoded,
     samples_reused = g.samples_reused, oversize_epochs = g.oversize_epochs,
     overlap = g.overlap)
end

_check_open(g::PrefetchGroup) =
    g.status === :closed && throw(ArgumentError("this PrefetchGroup is closed"))

# The epoch list an `order` argument names, checked before anything is cancelled.
function _resolve_order(g::PrefetchGroup, order)
    n = length(g.B)
    if order isa Symbol || order isa AbstractString
        o = Symbol(order)
        o === :forward && return collect(1:n)
        o === :reverse && return collect(n:-1:1)
        throw(ArgumentError(
            "order must be :forward, :reverse or a Vector{Int} of epochs, got $(repr(order))"))
    end
    order isa AbstractVector || throw(ArgumentError(
        "order must be :forward, :reverse or a Vector{Int} of epochs, got $(typeof(order))"))
    isempty(order) && throw(ArgumentError("an explicit order must name at least one epoch"))
    all(x -> x isa Integer, order) || throw(ArgumentError(
        "an explicit order is a list of (1-based) epoch indices, got $(repr(order))"))
    bad = sort!(unique!(Int[k for k in order if !(1 <= k <= n)]))
    isempty(bad) || throw(ArgumentError(
        "epoch(s) $bad are outside 1:$n (this group has $n epochs)"))
    return Int[k for k in order]
end

"""
    start!(g::PrefetchGroup, order; from = nothing) -> g

Declare the order in which epochs will be taken and begin decoding ahead of it
(spec/prefetch.md §3.3): `:forward` (`1, …, n`), `:reverse` (`n, …, 1` — the
adjoint's backward sweep), or an explicit `Vector{Int}` of 1-based epochs, repeats
legal (a revolve schedule). An empty list or an out-of-range epoch is an error.
`from = k` trims the order to begin at its first entry equal to `k` — the restart
form, `start!(g, :forward; from = r)` — and an epoch absent from the order is an
error. Any previous order is cancelled first (`cancel!(g; wait = false)`), and the
`changed` flags restart: the first `take!` after `start!` reports every member
changed."""
function start!(g::PrefetchGroup, order; from = nothing)
    epochs = lock(g.lock) do
        _check_open(g)
        _resolve_order(g, order)
    end
    if from !== nothing
        i = from isa Integer ? findfirst(==(from), epochs) : nothing
        i === nothing && throw(ArgumentError(
            "from = $(repr(from)) is not an epoch of this order; it must name an " *
            "entry to resume at"))
        epochs = epochs[i:end]
    end
    lock(g.lock) do
        _drop_order!(g)
        g.order = epochs
        g.next = 1
        g.admitted = 0
        g.stop_after = typemax(Int)
        g.warm_through = 0
        fill!(g.last_anchor, nothing)
        g.status = :running
        if g.overlap && isempty(g.workers)
            for _ in 1:g.nworkers
                push!(g.workers, errormonitor(Threads.@spawn _prefetch_worker(g, false)))
            end
            # Warm-ups get a task of their own, so a long download never sits in
            # front of a decode the consumer is about to need.
            push!(g.workers, errormonitor(Threads.@spawn _prefetch_worker(g, true)))
        end
        _advance!(g)
    end
    return g
end

"""
    take!(g::PrefetchGroup, k::Integer) -> EpochSample

Block until epoch `k` is decoded and return it (spec/prefetch.md §3.3–3.4). `k`
must be the next entry of the order declared by [`start!`]; anything else is an
error naming the epoch expected, and consumes nothing. Taking after the order is
exhausted, before any `start!`, after [`cancel!`], or after a `take!` raised a
stored error is an error too: call `start!` again to re-seek.

A decode (or `transform`) error is raised by the `take!` of the slot it belongs to
and never earlier — if several members fail, the first by key order — exactly as
`refresh` would raise it (no wrapper)."""
function Base.take!(g::PrefetchGroup, k::Integer)
    lock(g.lock)
    try
        _check_open(g)
        g.status === :new && throw(ArgumentError(
            "take! before start!: declare an order with start!(g, :forward) first"))
        g.status === :cancelled && throw(ArgumentError(
            "take! after cancel!: call start! to declare a new order"))
        g.status === :failed && throw(ArgumentError(
            "the order ended at an epoch whose decode failed (its take! raised the " *
            "error); call start! again — e.g. start!(g, order; from = <next epoch>)"))
        g.next > length(g.order) && throw(ArgumentError(
            "the order is exhausted (all $(length(g.order)) entries taken); call " *
            "start! to re-seek"))
        expected = g.order[g.next]
        k == expected || throw(ArgumentError(
            "take! expected epoch $expected, the next entry of the order (position " *
            "$(g.next) of $(length(g.order))), but was asked for epoch $k; nothing " *
            "was consumed"))
        isempty(g.window) && _advance!(g)
        slot = g.window[1]
        t0 = time()
        g.overlap || _decode_inline!(g, slot)
        while !all(j -> j.status === :done || j.status === :failed, slot.jobs)
            g.fatal === nothing || break
            wait(g.cond)
        end
        g.waited_s += time() - t0
        g.fatal === nothing || error("PrefetchGroup worker failed: $(g.fatal)")
        for j in slot.jobs                       # key order: the first failure wins
            if j.status === :failed
                _drop_order!(g)
                g.status = :failed
                throw(j.err)
            end
        end
        popfirst!(g.window)
        samples = Dict{String,Sample}()
        for j in slot.jobs
            i = j.member
            samples[g.keys[i]] = Sample(j.data, j.anchor,
                                        g.last_anchor[i] === nothing || g.last_anchor[i] != j.anchor)
            g.last_anchor[i] = j.anchor
            g.last_taken[i] = j
            j.refs -= 1
            j.refs == 0 && get(g.live, (i, j.anchor), nothing) === j &&
                delete!(g.live, (i, j.anchor))
        end
        g.next += 1
        _advance!(g)
        return EpochSample(slot.epoch, g.B[slot.epoch], samples)
    finally
        unlock(g.lock)
    end
end

"""
    isready(g::PrefetchGroup, k::Integer) -> Bool

Non-blocking: whether `take!(g, k)` would return at once — `k` is the next epoch of
the order and all its member samples are finished (decoded, or failed: a stored
error is raised by `take!`, never here). `false` before `start!`, after `cancel!`,
and for any epoch other than the next one, decoded or not. In the
single-thread fallback a slot is decoded by its `take!`, so it is ready only when
every member sample is a reused one."""
function Base.isready(g::PrefetchGroup, k::Integer)
    lock(g.lock) do
        g.status === :running || return false
        isempty(g.window) && return false
        slot = g.window[1]  # the head: the only slot `take!` can deliver
        slot.epoch == k || return false
        return all(j -> j.status === :done || j.status === :failed, slot.jobs)
    end
end

"""
    cancel!(g::PrefetchGroup; wait = false) -> g

Drop all pending work at once (spec/prefetch.md §3.8). A decode already running —
possibly inside a C library — runs to completion and its result is discarded; with
`wait = true` this returns only after those decodes finish. The group is reusable:
call [`start!`] to declare a new order. `take!` before then is an error."""
function cancel!(g::PrefetchGroup; wait::Bool = false)
    lock(g.lock) do
        _check_open(g)
        _drop_order!(g)
        g.status === :new || (g.status = :cancelled)
        if wait
            while g.running > 0
                Base.wait(g.cond)
            end
        end
    end
    return g
end

"""
    close(g::PrefetchGroup)

`cancel!(g; wait = true)`, then stop and join the worker tasks. The group is
unusable afterwards; closing twice is a no-op."""
function Base.close(g::PrefetchGroup)
    workers = lock(g.lock) do
        g.status === :closed && return Task[]
        _drop_order!(g)
        # Not :running any more, so a decode landing while we wait admits nothing.
        g.status = :cancelled
        while g.running > 0
            wait(g.cond)
        end
        g.status = :closed
        notify(g.cond)
        w = copy(g.workers)
        empty!(g.workers)
        w
    end
    foreach(wait, workers)
    return nothing
end

# --- scheduling (all under g.lock) ------------------------------------------

# Forget the current order: queued decodes and warm-ups are dropped, a running
# decode is marked so its result is discarded when it lands, and the window is
# emptied. Delivered samples (`last_taken`) are kept — decoding is deterministic,
# so a later order may serve one again instead of decoding it twice.
function _drop_order!(g::PrefetchGroup)
    for slot in g.window, j in slot.jobs
        j.status in (:queued, :running) && (j.status = :cancelled)
    end
    empty!(g.window)
    empty!(g.queue)
    empty!(g.live)
    empty!(g.fetch_queue)
    notify(g.cond)
    return nothing
end

# The anchor `t` snaps to for provider `p` — the tick value `sample_at(p, t)`
# resolves — or the exception `refresh(p, t)` would raise (a time before the
# member's first anchor), to be stored against the slot and raised by its take!.
function _anchor(p::Provider, t::Float64)
    try
        return p.times[_tick_index(p, t)]
    catch err
        return err
    end
end

# Bytes held by the window (§3.6): every distinct member sample a slot ahead of
# the consumer references, at its decoded size, or — still in flight — at that
# member's last decoded size.
function _held_bytes(g::PrefetchGroup)
    seen = Set{UInt}()
    total = 0
    for slot in g.window, j in slot.jobs
        id = objectid(j)
        id in seen && continue
        push!(seen, id)
        total += j.status === :done ? j.bytes :
                 j.status in (:queued, :running) ? max(g.last_bytes[j.member], 0) : 0
    end
    return total
end

# Admit order positions into the decode window while the budget allows, fold any
# newly finished slots into the statistics, queue warm-ups past the window, and
# wake whoever waits on the group.
function _advance!(g::PrefetchGroup)
    g.status === :running && _admit!(g)
    _count_finished!(g)
    g.status === :running && g.overlap && _queue_warmups!(g)
    notify(g.cond)
    return nothing
end

function _admit!(g::PrefetchGroup)
    nm = length(g.providers)
    while g.admitted < length(g.order) && g.admitted < g.stop_after
        nwin = length(g.window)
        nwin >= g.depth && break
        pos = g.admitted + 1
        k = g.order[pos]
        t = g.B[k]
        # Per member: an existing sample at the same anchor (in the window, or the
        # one last delivered) to share, a fresh decode, or the anchor's own error.
        plan = Vector{Any}(undef, nm)
        estimate = 0
        unknown = false
        for i in 1:nm
            a = _anchor(g.providers[i], t)
            if a isa Exception
                plan[i] = a
                continue
            end
            j = get(g.live, (i, a), nothing)
            if j === nothing && g.last_taken[i] !== nothing && g.last_taken[i].anchor == a
                j = g.last_taken[i]
                estimate += j.bytes                  # held again once a slot holds it
            end
            if j !== nothing
                plan[i] = j
            else
                plan[i] = a
                g.last_bytes[i] < 0 ? (unknown = true) : (estimate += g.last_bytes[i])
            end
        end
        # The head slot is always admitted (`nwin == 0`); behind it a slot must fit.
        if g.max_bytes !== nothing && nwin > 0
            unknown && break
            _held_bytes(g) + estimate > g.max_bytes && break
        end
        jobs = Vector{_PrefetchJob}(undef, nm)
        for i in 1:nm
            pl = plan[i]
            if pl isa _PrefetchJob
                pl.refs += 1
                g.live[(i, pl.anchor)] = pl
                g.samples_reused += 1
                jobs[i] = pl
                pl.status === :failed && (g.stop_after = min(g.stop_after, pos))
            elseif pl isa Exception
                jobs[i] = _PrefetchJob(i, NaN, t, :failed, nothing, 0, pl, 1)
                g.stop_after = min(g.stop_after, pos)
            else
                j = _PrefetchJob(i, pl, t, :queued, nothing, 0, nothing, 1)
                g.live[(i, pl)] = j
                push!(g.queue, j)
                jobs[i] = j
            end
        end
        push!(g.window, _PrefetchSlot(pos, k, jobs, false))
        g.admitted = pos
    end
    return nothing
end

# Fold newly finished slots into `epochs_decoded` / `oversize_epochs`, and the
# bytes now held into `peak_bytes`.
function _count_finished!(g::PrefetchGroup)
    for slot in g.window
        slot.counted && continue
        all(j -> j.status === :done || j.status === :failed, slot.jobs) || continue
        slot.counted = true
        all(j -> j.status === :done, slot.jobs) || continue
        g.epochs_decoded += 1
        bytes = sum(j -> j.bytes, unique(slot.jobs); init = 0)
        g.max_bytes !== nothing && bytes > g.max_bytes && (g.oversize_epochs += 1)
    end
    held = 0
    seen = Set{UInt}()
    for slot in g.window, j in slot.jobs
        (j.status === :done && !(objectid(j) in seen)) || continue
        push!(seen, objectid(j))
        held += j.bytes
    end
    g.peak_bytes = max(g.peak_bytes, held)
    return nothing
end

# Fetch-ahead (§3.6): queue the files of the next `fetch_ahead` order positions
# past the decode window to be DOWNLOADED (never decoded) by idle workers. Store-
# backed members are skipped: their objects are known only once a decode resolves
# the chunk keys.
function _queue_warmups!(g::PrefetchGroup)
    g.fetch_ahead > 0 || return nothing
    upto = min(length(g.order), g.admitted + g.fetch_ahead, g.stop_after)
    from = max(g.warm_through, g.admitted) + 1
    for pos in from:upto
        t = g.B[g.order[pos]]
        for (i, p) in enumerate(g.providers)
            store_backed(p) && continue
            for u in _sample_urls(p, t)
                u in g.warmed && continue
                push!(g.warmed, u)
                push!(g.fetch_queue, (i, u))
            end
        end
    end
    g.warm_through = max(g.warm_through, upto)
    return nothing
end

# The URLs `sample_at(p, t)` will read: the anchor tick's file, and with a
# two-record bracket the successor tick's. Best-effort, like the warm-up itself.
function _sample_urls(p::Provider, t::Float64)
    try
        tick = _tick_index(p, t)
        urls = String[p.url_for(p.times[tick])]
        if p.records_per_sample == 2 && p.time_dim !== nothing && tick < length(p.times)
            push!(urls, p.url_for(p.times[tick + 1]))
        end
        return unique!(urls)
    catch
        return String[]
    end
end

# --- decoding ----------------------------------------------------------------

# §3.6 bytes of a decoded sample: array bytes of every variable and coordinate,
# strings at their UTF-8 length; a windowed sample is the sum over its windows.
_sample_bytes(nds::NativeDataset) =
    sum(_field_bytes, values(nds.variables); init = 0) +
    sum(_field_bytes, values(nds.coords); init = 0)
_sample_bytes(v::AbstractVector) = sum(_sample_bytes, v; init = 0)
_sample_bytes(x) = Base.summarysize(x)

function _field_bytes(f::NativeField)
    T = eltype(f.data)
    T <: AbstractString && return sum(ncodeunits, f.data; init = 0)
    isbitstype(T) && return sizeof(T) * length(f.data)
    return Base.summarysize(f.data)
end

# Decode one job OUTSIDE the lock, then publish it under the lock unless it was
# cancelled meanwhile. The decode is the member's own `sample_at` (or
# `sample_windows_at`), so it is bitwise what `refresh` returns: scheduling can
# change only when this runs, never what it computes.
function _run_job!(g::PrefetchGroup, job::_PrefetchJob)
    key = g.keys[job.member]
    p = g.providers[job.member]
    t0 = time()
    data, nbytes, err = try
        g.decode_hook === nothing || g.decode_hook(key, job.t)
        d = g.windows === nothing ? sample_at(p, job.t) :
            sample_windows_at(p, job.t, g.windows)
        n = _sample_bytes(d)
        g.transform === nothing || (d = g.transform(key, d))
        (d, n, nothing)
    catch e
        (nothing, 0, e)
    end
    dt = time() - t0
    lock(g.lock) do
        g.running -= 1
        g.decode_s += dt
        if job.status === :running
            if err === nothing
                job.data, job.bytes, job.status = data, nbytes, :done
                g.samples_decoded += 1
                g.last_bytes[job.member] = nbytes
            else
                job.err, job.status = err, :failed
                # Scheduling stops past a failed slot (§3.7): nothing behind the
                # first slot holding this sample is admitted.
                for slot in g.window
                    any(j -> j === job, slot.jobs) || continue
                    g.stop_after = min(g.stop_after, slot.pos)
                    break
                end
            end
        end
        _advance!(g)
    end
    return nothing
end

# The single-thread fallback: decode the head slot's queued samples here, inside
# `take!`, on the consumer's own thread (called with g.lock held; nothing else can
# take it, there being no workers).
function _decode_inline!(g::PrefetchGroup, slot::_PrefetchSlot)
    for j in slot.jobs
        j.status === :queued || continue
        filter!(q -> q !== j, g.queue)
        j.status = :running
        g.running += 1
        _run_job!(g, j)
    end
    return nothing
end

# A worker: a decode worker takes the next decode, the one fetcher the next
# warm-up; else it sleeps on the condition. Exits when the group closes. Warming
# runs on its own task so it never delays a slot the consumer is about to need.
function _prefetch_worker(g::PrefetchGroup, fetcher::Bool)
    try
        while true
            job, warm = lock(g.lock) do
                while true
                    g.status === :closed && return (nothing, nothing)
                    if fetcher
                        isempty(g.fetch_queue) || return (nothing, popfirst!(g.fetch_queue))
                    elseif !isempty(g.queue)
                        j = popfirst!(g.queue)
                        j.status = :running
                        g.running += 1
                        return (j, nothing)
                    end
                    wait(g.cond)
                end
            end
            if job !== nothing
                _run_job!(g, job)
            elseif warm !== nothing
                _warm!(g, warm...)
            else
                return nothing
            end
        end
    catch err
        lock(g.lock) do
            g.fatal = sprint(showerror, err, catch_backtrace())
            notify(g.cond)
        end
        rethrow()
    end
end

# Download one file into the disk cache, decoding nothing. Its errors are dropped:
# the decode that needs the file raises the real one at `take!`.
function _warm!(g::PrefetchGroup, member::Int, url::String)
    p = g.providers[member]
    try
        fetch_blob(p.cache, url; source_loader = p.source_loader, auth_realm = p.auth_realm)
    catch
    end
    return nothing
end
