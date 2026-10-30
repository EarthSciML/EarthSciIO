# PrefetchGroup (spec/prefetch.md §3), driven by the committed scenarios
# `prefetch-equivalence` and `prefetch-errors` over their FULL grids, offline, plus
# the API contract, cancellation, the single-thread fallback and the waited_s
# claim. Epochs in the scenario files are 0-based; Julia's are 1-based, so every
# epoch read from them gets +1. Reuses `_scenario`, `_scenario_provider` and
# `bitwise_same` from test_windows.jl.

# Randomised decode latency (the binding's test hook), so workers finish in a
# different order from run to run and the equivalence cannot lean on timing.
const _JITTER = (key, t) -> sleep(rand() * 0.003)

# Block until `f()` holds, or fail the wait after `timeout` seconds.
function _wait_until(f; timeout = 30.0)
    t0 = time()
    while !f()
        time() - t0 > timeout && return false
        sleep(0.002)
    end
    return true
end

# Whether epoch `k`'s slot in the window is finished, head or not (`isready` only
# reports the head).
_slot_done(g, k) = lock(g.lock) do
    any(s -> s.epoch == k && all(j -> j.status in (:done, :failed), s.jobs), g.window)
end

_epochs(order, n) = order === :forward ? collect(1:n) :
                    order === :reverse ? collect(n:-1:1) : order

@testset "PrefetchGroup — the `prefetch-equivalence` scenario (§3.5)" begin
    sc = _scenario("prefetch-equivalence")
    cache = Cache(LocalStore(joinpath(CORPUS, "cache")); offline = true, verify = true)
    members() = Dict(String(m["key"]) => _scenario_provider(cache, sc["sources"][m["source"]],
                                                            m["records_per_sample"])
                     for m in sc["members"])
    B = Float64.(sc["expect"]["boundaries_s"])
    n = length(B)
    anchors = Dict(k => Float64.(v) for (k, v) in sc["expect"]["anchors_s"])
    changed_fwd = Dict(k => Bool.(v) for (k, v) in sc["expect"]["changed_forward"])

    # The synchronous reference: a FRESH provider's refresh(B[k]), per member.
    fresh = members()
    ref = Dict(k => [refresh(p, t) for t in B] for (k, p) in fresh)
    orders = Any[o isa AbstractString ? Symbol(o) : Int[Int(k) + 1 for k in o] for o in sc["orders"]]

    for order in orders, depth in sc["depth"], workers in sc["workers"], mb in sc["max_bytes"]
        label = "order=$(order isa Symbol ? order : "explicit") depth=$depth workers=$workers max_bytes=$mb"
        @testset "$label" begin
            g = PrefetchGroup(members(); depth = depth, workers = workers, max_bytes = mb,
                              _decode_hook = _JITTER)
            failures = String[]
            try
                @test boundaries(g) == B
                start!(g, order)
                prev = Dict{String,Float64}()
                seq = _epochs(order, n)
                for k in seq
                    ep = take!(g, k)
                    ep.epoch == k && ep.time == B[k] || push!(failures, "epoch $k: wrong epoch/time")
                    Set(keys(ep)) == Set(keys(ref)) || push!(failures, "epoch $k: members")
                    for (key, s) in ep
                        bitwise_same(s.data, ref[key][k]) ||
                            push!(failures, "epoch $k $key: data != refresh(B[k])")
                        s.anchor == anchors[key][k] ||
                            push!(failures, "epoch $k $key: anchor $(s.anchor)")
                        s.changed == (!haskey(prev, key) || prev[key] != s.anchor) ||
                            push!(failures, "epoch $k $key: changed=$(s.changed)")
                        order === :forward && s.changed != changed_fwd[key][k] &&
                            push!(failures, "epoch $k $key: changed_forward")
                        prev[key] = s.anchor
                    end
                end
                @test isempty(failures)
                isempty(failures) || @info "prefetch-equivalence mismatches" label failures
                # The order is exhausted; asking again is an error, not a wrap.
                @test_throws ArgumentError take!(g, first(seq))
                st = stats(g)
                @test st.epochs_decoded == length(seq)
                @test st.samples_decoded + st.samples_reused == length(keys(ref)) * length(seq)
                order isa Symbol && @test st.samples_reused == 6      # PS, every other epoch
                @test st.oversize_epochs == (mb === nothing ? 0 : st.epochs_decoded)
                @test st.peak_bytes > 0
                @test st.overlap == (Threads.nthreads() > 1)
            finally
                close(g)
            end
        end
    end

    # Once with `windows`: each member's data is the windowed vector, equal to
    # [sample_at(with_window(p, w), B[k]) for w in windows].
    W = sc["windows"]
    g = PrefetchGroup(members(); depth = 2, workers = 4, windows = W, _decode_hook = _JITTER)
    try
        start!(g, :forward)
        ok = true
        for k in 1:n, (key, s) in take!(g, k)
            ok &= bitwise_same(s.data, [sample_at(with_window(fresh[key], w), B[k]) for w in W])
        end
        @test ok
    finally
        close(g)
    end

    # ...and with a `transform`, run on the worker: data is transform(key, sample).
    tf = (key, d) -> (key, sum(sum(filter(!isnan, vec(f.data))) for f in values(d.variables)))
    g = PrefetchGroup(members(); depth = 3, workers = 2, transform = tf)
    try
        start!(g, :reverse)
        @test all(take!(g, k)[key].data == tf(key, ref[key][k]) for k in n:-1:1 for key in ("Q1",))
    finally
        close(g)
    end
end

@testset "PrefetchGroup — the `prefetch-errors` scenario (§3.7–3.8)" begin
    sc = _scenario("prefetch-errors")
    cache = Cache(LocalStore(joinpath(CORPUS, "cache")); offline = true, verify = true)
    src, rps = sc["source"], sc["records_per_sample"]
    members() = Dict("Q" => _scenario_provider(cache, src, rps))
    miss = sc["missing_from"] + 1                 # the first epoch of the missing file
    after = sc["cancel_after"]                    # epochs taken before the cancel
    p = _scenario_provider(cache, src, rps)
    B = refresh_times(p)
    ref = [refresh(p, B[k]) for k in 1:(miss - 1)]
    @test_throws CacheMiss refresh(p, B[miss])    # what take! must raise, unwrapped

    for depth in sc["depth"], workers in sc["workers"]
        @testset "depth=$depth workers=$workers" begin
            # (1) take! succeeds for every epoch before the missing file and raises
            # CacheMiss at its first epoch — not before, although the workers reach
            # it early — and the order is then over.
            g = PrefetchGroup(members(); depth = depth, workers = workers, _decode_hook = _JITTER)
            try
                start!(g, :forward)
                ok = true
                for k in 1:(miss - 1)
                    # With room ahead, let the failing slot finish BEFORE taking the
                    # one in front of it: its stored error must not leak out early.
                    if k == miss - 1 && depth > 1 && Threads.nthreads() > 1
                        @test _wait_until(() -> _slot_done(g, miss))
                    end
                    ok &= bitwise_same(take!(g, k)["Q"].data, ref[k])
                end
                @test ok
                @test_throws CacheMiss take!(g, miss)
                err = try
                    take!(g, miss + 1)
                catch e
                    e
                end
                @test err isa ArgumentError && occursin("start!", sprint(showerror, err))
                @test_throws ArgumentError take!(g, miss)
            finally
                close(g)
            end

            # (2) cancel after `cancel_after` epochs, resume from the next: the rest
            # equals the uninterrupted sequence, the failure included.
            g = PrefetchGroup(members(); depth = depth, workers = workers, _decode_hook = _JITTER)
            try
                start!(g, :forward)
                @test all(bitwise_same(take!(g, k)["Q"].data, ref[k]) for k in 1:after)
                cancel!(g)
                @test_throws ArgumentError take!(g, after + 1)
                @test !isready(g, after + 1)
                start!(g, :forward; from = after + 1)
                first_ep = take!(g, after + 1)
                @test first_ep["Q"].changed                       # first take after start!
                @test bitwise_same(first_ep["Q"].data, ref[after + 1])
                @test all(bitwise_same(take!(g, k)["Q"].data, ref[k]) for k in (after + 2):(miss - 1))
                @test_throws CacheMiss take!(g, miss)
            finally
                close(g)
            end
        end
    end

    # (3) A COLD cache — an empty offline root — raises CacheMiss at the first take.
    cold = Cache(LocalStore(mktempdir()); offline = true)
    g = PrefetchGroup(Dict("Q" => _scenario_provider(cold, src, rps)); workers = 2)
    try
        start!(g, :forward)
        @test_throws CacheMiss take!(g, 1)
    finally
        close(g)
    end
end

@testset "PrefetchGroup — construction and API contract (§3.1–3.4, 3.8)" begin
    cache = Cache(LocalStore(joinpath(CORPUS, "cache")); offline = true, verify = true)
    src = _scenario("prefetch-equivalence")["sources"]["hourly"]
    q() = _scenario_provider(cache, src, 1)
    n = length(src["refresh_times_s"])
    mixed = _corpus_case("named-window-mixed-rank")["resolved_url"]

    # Construction errors.
    @test_throws ArgumentError PrefetchGroup(Dict{String,Provider}())
    @test_throws ArgumentError PrefetchGroup(Dict("C" => const_provider(cache, mixed; format = "netcdf")))
    @test_throws ArgumentError PrefetchGroup(Dict("x" => 1))
    @test_throws ArgumentError PrefetchGroup(Dict(:Q => q(), "Q" => q()))
    @test_throws ArgumentError PrefetchGroup(Dict("Q" => q()); depth = 0)
    @test_throws ArgumentError PrefetchGroup(Dict("Q" => q()); workers = 0)
    @test_throws ArgumentError PrefetchGroup(Dict("Q" => q()); max_bytes = -1)
    @test_throws ArgumentError PrefetchGroup(Dict("Q" => q()); fetch_ahead = -1)
    # every window must pass with_window's preconditions for every member
    @test_throws ArgumentError PrefetchGroup(Dict("Q" => q()); windows = [DimWindow("time" => "all")])
    @test_throws ArgumentError PrefetchGroup(Dict("Q" => with_window(q(), DimWindow()));
                                             windows = [DimWindow("lat" => (0, 1))])

    g = PrefetchGroup(Dict(:Q => q()); depth = 2, workers = 2)    # keys taken as strings
    try
        @test boundaries(g) == Float64.(src["refresh_times_s"])
        @test !isready(g, 1)
        @test_throws ArgumentError take!(g, 1)                     # before start!

        # Bad orders are refused BEFORE the current order is cancelled.
        start!(g, :forward)
        @test take!(g, 1)["Q"].changed
        for bad in (:sideways, Int[], [0], [n + 1], Any["a"])
            @test_throws ArgumentError start!(g, bad)
        end
        @test_throws ArgumentError start!(g, :forward; from = n + 5)
        # Asking for anything but the next entry is an error naming it, and
        # consumes nothing.
        err = try
            take!(g, 5)
        catch e
            e
        end
        @test err isa ArgumentError && occursin("expected epoch 2", sprint(showerror, err))
        ep = take!(g, 2)
        @test ep.epoch == 2 && ep["Q"].anchor == ep.time && ep[:Q] === ep["Q"]
        @test !stats(g).overlap || _wait_until(() -> isready(g, 3))
        @test !isready(g, 12)                                      # not in the window

        # `from` trims the order at its first matching entry; repeats are legal.
        start!(g, :reverse; from = 5)
        @test [take!(g, k).epoch for k in 5:-1:1] == [5, 4, 3, 2, 1]
        @test_throws ArgumentError take!(g, 1)                     # exhausted
        start!(g, [3, 1, 4, 1, 5, 9, 2, 6]; from = 1)
        seq = [1, 4, 1, 5, 9, 2, 6]
        eps = [take!(g, k) for k in seq]
        @test [e.epoch for e in eps] == seq
        @test all(e["Q"].changed for e in eps)                     # every anchor differs

        start!(g, [7, 7, 7])                                       # a repeated anchor
        a, b, c = take!(g, 7), take!(g, 7), take!(g, 7)
        @test a["Q"].changed && !b["Q"].changed && !c["Q"].changed
        @test a["Q"].data === b["Q"].data === c["Q"].data          # shared, read-only

        # cancel!(wait = true) returns with nothing running; the group is reusable.
        start!(g, :forward)
        cancel!(g; wait = true)
        @test g.running == 0
        @test_throws ArgumentError take!(g, 1)
        start!(g, :forward)
        @test take!(g, 1)["Q"].changed
        st = stats(g)
        @test st.decode_s > 0 && st.waited_s >= 0
    finally
        close(g)
    end
    close(g)                                                       # idempotent
    @test_throws ArgumentError start!(g, :forward)
    @test_throws ArgumentError take!(g, 1)
    @test_throws ArgumentError cancel!(g)
    @test isempty(g.workers)
end

@testset "PrefetchGroup — the single-thread fallback decodes inside take! (§3.10)" begin
    cache = Cache(LocalStore(joinpath(CORPUS, "cache")); offline = true, verify = true)
    src = _scenario("prefetch-equivalence")["sources"]["hourly"]
    p = _scenario_provider(cache, src, 2)
    B = refresh_times(p)
    ref = [refresh(p, t) for t in B]

    # By default the fallback is what one thread gets.
    g0 = PrefetchGroup(Dict("Q" => p))
    @test stats(g0).overlap == (Threads.nthreads() > 1)
    close(g0)

    # Forced here so it is exercised whatever the thread count: no workers, nothing
    # decoded ahead (so nothing is ready before its take!), and the same samples.
    g = PrefetchGroup(Dict("Q" => p); depth = 3, workers = 4, _overlap = false)
    try
        start!(g, :forward)
        @test isempty(g.workers)
        @test !isready(g, 1)
        sleep(0.05)
        @test !isready(g, 1)                                   # still: nobody decodes ahead
        @test all(bitwise_same(take!(g, k)["Q"].data, ref[k]) for k in eachindex(B))
        st = stats(g)
        @test st.overlap == false
        @test st.samples_decoded == length(B)
        @test st.waited_s > 0                                  # take! did the decoding
    finally
        close(g)
    end
end

@testset "PrefetchGroup — waited_s falls to ~0 when compute exceeds decode time" begin
    cache = Cache(LocalStore(joinpath(CORPUS, "cache")); offline = true, verify = true)
    src = _scenario("prefetch-equivalence")["sources"]["hourly"]
    # Synthetic decode latency (s), spent BLOCKING the worker's thread the way a C
    # decode does. Not `sleep`: a libuv timer can fire late while the consumer's
    # thread is itself blocked below, which would charge timer slack to the decode.
    decode = 0.02
    hook = (key, t) -> Libc.systemsleep(decode)
    drive(compute) = begin
        g = PrefetchGroup(Dict("Q" => _scenario_provider(cache, src, 1)); depth = 2,
                          workers = 1, _decode_hook = hook)
        try
            start!(g, :forward)
            for k in eachindex(boundaries(g))
                compute > 0 && Libc.systemsleep(compute)  # the solver, not yielding
                take!(g, k)
            end
            stats(g)
        finally
            close(g)
        end
    end
    # `idle` first: any compilation lands in its first take!, inflating only it.
    idle = drive(0.0)
    busy = drive(5 * decode)
    if Threads.nthreads() > 1
        # Decoding ran beside the consumer: with compute longer than a decode, every
        # slot is ready by the time it is asked for. Without compute, take! waits.
        @test busy.overlap
        @test busy.waited_s < 0.25 * idle.waited_s
        @test busy.waited_s < 2 * decode
        @test idle.waited_s > 6 * decode
    else
        # One thread: nothing to overlap with; take! decodes, and reports so.
        @test !busy.overlap
        @test busy.waited_s >= 12 * decode
    end
end
