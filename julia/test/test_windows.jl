# Named-dimension windows (spec/prefetch.md §1–2): the `select = {"dims": …}` form
# in the netcdf and zarr readers, `sample_at`, `DimWindow`, `with_window` and
# `sample_windows_at` — including the `multi-window` scenario, read from
# conformance/corpus/scenarios and run over its full grid. Reuses `CORPUS`
# (test_conformance.jl), `cmp_native_numeric` (test_readers.jl) and the zarr store
# helpers (`CountingStore`, `_z_zarray`, `_z_encode`, `_z_populate`, test_zarr.jl).

const SCENARIOS = joinpath(CORPUS, "scenarios")
_scenario(id) = JSON.parsefile(joinpath(SCENARIOS, id * ".json"))
_corpus_case(id) = JSON.parsefile(joinpath(CORPUS, "cases", id * ".json"))

# A scenario source's URL resolver, as the scenarios state it: the file whose
# anchor is `start + floor((t - start) / file_period) * file_period`.
function _scenario_url(src)
    start, period = Float64(src["start_s"]), Float64(src["file_period_s"])
    files = Dict(Float64(f["anchor_s"]) => String(f["url"]) for f in src["files"])
    return t -> files[start + floor((Float64(t) - start) / period) * period]
end

# A scenario's DISCRETE source as a Julia Provider, its times in Unix seconds.
_scenario_provider(cache, src, rps) =
    discrete_provider(
    cache, _scenario_url(src), Float64.(src["refresh_times_s"]);
    format = String(src["format"]), time_dim = String(src["time_dim"]),
    variables = String.(src["variables"]), records_per_sample = rps
)

# BITWISE equality of decoded samples — names, dims, attrs, eltypes, shapes and
# the bytes themselves (NaN for NaN, -0.0 apart from 0.0), strings by value. A
# windowed sample is a vector of datasets, compared element by element.
_field_bits(a) = isbitstype(eltype(a)) ? reinterpret(UInt8, vec(collect(a))) : vec(collect(a))
function bitwise_same(a::NativeDataset, b::NativeDataset)
    Set(keys(a.variables)) == Set(keys(b.variables)) || return false
    Set(keys(a.coords)) == Set(keys(b.coords)) || return false
    for (fa, fb) in Iterators.flatten(
            (
                ((a.variables[n], b.variables[n]) for n in keys(a.variables)),
                ((a.coords[n], b.coords[n]) for n in keys(a.coords)),
            )
        )
        fa.dims == fb.dims && fa.attrs == fb.attrs && size(fa.data) == size(fb.data) &&
            eltype(fa.data) == eltype(fb.data) && _field_bits(fa.data) == _field_bits(fb.data) ||
            return false
    end
    return true
end
bitwise_same(a::AbstractVector, b::AbstractVector) =
    length(a) == length(b) && all(bitwise_same(x, y) for (x, y) in zip(a, b))
bitwise_same(a, b) = isequal(a, b)

# The REFERENCE for "the full read sliced by name afterwards" — written here from
# the wire vocabulary alone, independently of the reader and of `sample_windows_at`.
function _ref_indices(s)
    s == "all" && return nothing
    haskey(s, "indices") && return Int[Int(i) for i in s["indices"]]
    sl = s["slice"]
    return collect(Int(sl[1]):(length(sl) > 2 ? Int(sl[3]) : 1):(Int(sl[2]) - 1))
end
function slice_by_name(nds::NativeDataset, window)
    named = select_form(window)["dims"]
    idx = Dict(String(d) => _ref_indices(s) for (d, s) in named)
    cut(f) = NativeField(
        f.data[
            (
                get(idx, d, nothing) === nothing ? Colon() : idx[d] .+ 1
                    for d in f.dims
            )...,
        ], copy(f.dims), copy(f.attrs)
    )
    return NativeDataset(
        Dict(n => cut(f) for (n, f) in nds.variables),
        Dict(n => cut(f) for (n, f) in nds.coords)
    )
end

# The error `f()` raises (or `nothing`), and its message.
function _raised(f)
    try
        f()
        return nothing, ""
    catch err
        return err, sprint(showerror, err)
    end
end

@testset "netcdf reader — the named `dims` select (spec/prefetch.md §2.1)" begin
    reader = FORMAT_REGISTRY["netcdf"]
    case = _corpus_case("named-window-mixed-rank")
    blob = joinpath(CORPUS, case["blob_path"])
    full = read_native(reader, blob)
    sel = case["select"]

    # The gate: the windowed read IS the full read sliced by name, coordinates
    # included, bit for bit — at rank 1, 2, 3 (twice: `lon` and the staggered
    # `lon_edge`) and 4 alike, with time untouched.
    w = read_native(reader, blob; select = sel)
    @test bitwise_same(w, slice_by_name(full, sel))
    for (name, spec) in case["expected"]["variables"]
        @test w[name].dims == spec["dims"]
        @test collect(size(w[name].data)) == spec["shape"]
    end
    @test w["time"].data == full["time"].data
    @test w["time"].attrs == full["time"].attrs

    # A DimWindow is the same select, whether given as its wire form or as itself.
    dw = DimWindow("lat" => (1, 3), "lon" => [4, 0, 2], "lev" => (0, 3, 2), "lon_edge" => (2, 5))
    @test select_form(dw) == select_form(sel)
    @test bitwise_same(read_native(reader, blob; select = select_form(dw)), w)
    @test bitwise_same(read_native(reader, blob; select = dw), w)

    # Unnamed dimensions are "all": an empty map selects nothing away, and a time
    # dimension named "all" is not a time selection.
    @test bitwise_same(read_native(reader, blob; select = Dict("dims" => Dict())), full)
    @test bitwise_same(
        read_native(reader, blob; select = Dict("dims" => Dict("time" => "all"))),
        full
    )

    # Order and repetition are the caller's; a one-index axis is kept at length 1.
    rep = Dict(
        "dims" => Dict(
            "lon" => Dict("indices" => [4, 4, 1]),
            "lat" => Dict("indices" => [2])
        )
    )
    r = read_native(reader, blob; select = rep)
    @test bitwise_same(r, slice_by_name(full, rep))
    @test size(r["T"].data) == (2, 3, 1, 3)
    @test r["lon"].data == full["lon"].data[[5, 5, 2]]

    # An empty selection is legal: a zero-length axis, kept in `dims`.
    for ax in (Dict("indices" => Int[]), Dict("slice" => [2, 2]), Dict("slice" => [3, 1]))
        e = read_native(reader, blob; select = Dict("dims" => Dict("lat" => ax)))
        @test size(e["T"].data) == (2, 3, 0, 5)
        @test e["T"].dims == ["time", "lev", "lat", "lon"]
        @test size(e["lat"].data) == (0,)
        @test size(e["U_EDGE"].data) == (2, 0, 6)
    end

    # Exactly one of `axes` and `dims`.
    err, msg = _raised(
        () -> read_native(
            reader, blob;
            select = Dict("axes" => Any["all", "all"], "dims" => Dict("lat" => "all"))
        )
    )
    @test err isa ArgumentError
    @test occursin("both", msg)

    # A named dimension no field of the read carries is an error naming the ones
    # that are present — never a window that silently means nothing.
    err, msg = _raised(
        () -> read_native(
            reader, blob;
            select = Dict("dims" => Dict("depth" => Dict("indices" => [0])))
        )
    )
    @test err isa ArgumentError
    @test occursin("depth", msg) && occursin("lon_edge", msg)

    # Time is the Provider's: anything but "all" on it is refused.
    err, msg = _raised(
        () -> read_native(
            reader, blob;
            select = Dict("dims" => Dict("time" => Dict("indices" => [0])))
        )
    )
    @test err isa ArgumentError
    @test occursin("time", msg)

    # Bounds, against the FILE length: never a clamp, never a negative wrap.
    for ax in (
            Dict("slice" => [2, 9]), Dict("slice" => [-1, 2]), Dict("indices" => [4]),
            Dict("indices" => [-1]),
        )
        err, msg = _raised(
            () -> read_native(
                reader, blob;
                select = Dict("dims" => Dict("lat" => ax))
            )
        )
        @test err !== nothing
        @test occursin("out of range", msg)
    end

    # The window-wrap case: a seam-crossing `indices` list comes back in the order
    # given, and `lon_edge` — a different dimension — stays whole.
    wrap = _corpus_case("window-wrap")
    ww = read_native(reader, blob; select = wrap["select"])
    @test bitwise_same(ww, slice_by_name(full, wrap["select"]))
    @test ww["lon"].data == full["lon"].data[[4, 5, 1, 2]]
    @test size(ww["U_EDGE"].data) == size(full["U_EDGE"].data)
end

@testset "netcdf reader — `dims` is checked against the PROJECTED fields" begin
    reader = FORMAT_REGISTRY["netcdf"]
    # monitor-metadata: value(site), char site_id(site, strlen), char flag(site),
    # char title(titlelen). `site` and `strlen` have no coordinate variable.
    blob = joinpath(CORPUS, _corpus_case("station-labels-text")["blob_path"])
    full = read_native(reader, blob)

    # A text field is windowed by name like any other.
    w = read_native(reader, blob; select = Dict("dims" => Dict("site" => Dict("indices" => [2, 0]))))
    @test w["site_id"].data == full["site_id"].data[[3, 1]]
    @test w["flag"].data == full["flag"].data[[3, 1]]
    @test w["value"].data == full["value"].data[[3, 1]]
    @test w["title"].data[] == full["title"].data[]        # rank 0: untouched

    # "The fields of the read" are the PROJECTED ones: with only the scalar
    # `title` kept, nothing returned carries `site`.
    sitesel = Dict("dims" => Dict("site" => Dict("indices" => [0])))
    err, msg = _raised(() -> read_native(reader, blob; variables = ["title"], select = sitesel))
    @test err isa ArgumentError
    @test occursin("site", msg)
    @test size(read_native(reader, blob; variables = ["value"], select = sitesel)["value"].data) == (1,)

    # A consumed string LENGTH is not a dimension of any decoded field, so naming it
    # is that same error — whatever the selector, "all" included — and it says
    # whose length it is. Applying it would truncate every string.
    for (dim, owner) in (("strlen", "site_id"), ("titlelen", "title"))
        for ax in (Dict("indices" => [0, 1]), "all")
            err, msg = _raised(() -> read_native(reader, blob; select = Dict("dims" => Dict(dim => ax))))
            @test err isa ArgumentError
            @test occursin("string LENGTH", msg) && occursin(owner, msg)
        end
    end
end

@testset "zarr reader — `dims` fetches only the window's chunk objects" begin
    case = _corpus_case("named-window-zarr")
    url = case["resolved_url"]
    vars = String.(case["variables"])
    store = CountingStore(LocalStore(joinpath(CORPUS, "cache")))
    cache = Cache(store; offline = true, verify = true)

    # One window, two ranks; every chunk object outside it is POISONED (a valid
    # blosc container of the wrong size), so reading one would fail the read.
    nds = read_store(ZarrReader(), cache, url; variables = vars, select = case["select"])
    for (name, spec) in case["expected"]["variables"]
        @test nds[name].dims == spec["dims"]
        @test collect(size(nds[name].data)) == spec["shape"]
        @test cmp_native_numeric(nds[name].data, spec["data"]) === nothing
    end
    clean = Set(cache_key(o["url"]) for o in case["objects"] if !get(o, "poisoned", false))
    poisoned = Set(cache_key(o["url"]) for o in case["objects"] if get(o, "poisoned", false))
    @test Set(store.gets) == clean
    @test isempty(intersect(Set(store.gets), poisoned))

    # The poison is real: a window that intersects a poisoned chunk fails.
    @test_throws Exception read_store(
        ZarrReader(), cache, url; variables = vars,
        select = Dict("dims" => Dict("lat" => Dict("slice" => [0, 2])))
    )

    # Every array's metadata is read and every selector bounds-checked BEFORE the
    # first chunk: a selector out of range for T3 alone costs no chunk at all.
    meta = Set(
        cache_key(o["url"]) for o in case["objects"]
            if endswith(o["url"], ".zarray") || endswith(o["url"], ".zattrs")
    )
    for bad in (
            Dict("lat" => Dict("slice" => [2, 4]), "lev" => Dict("indices" => [3])),
            Dict("lon" => Dict("indices" => [5, 3]), "depth" => "all"),
        )
        empty!(store.gets)
        err, msg = _raised(
            () -> read_store(
                ZarrReader(), cache, url; variables = vars,
                select = Dict("dims" => bad)
            )
        )
        @test err !== nothing
        @test issubset(Set(store.gets), meta)
    end
    err, msg = _raised(
        () -> read_store(
            ZarrReader(), cache, url; variables = vars,
            select = Dict("dims" => Dict("depth" => "all"))
        )
    )
    @test err isa ArgumentError && occursin("depth", msg) && occursin("lev", msg)
    @test_throws ArgumentError read_store(
        ZarrReader(), cache, url; variables = vars,
        select = Dict("axes" => Any["all"], "dims" => Dict())
    )

    # Time, and SYNTHESIZED names, on a Julia-built store: `v[time, x]` with
    # `_ARRAY_DIMENSIONS`, and `u` with no `.zattrs` at all (so `dim_0`, `dim_1`).
    zb = "s3://earthsci-fixtures/named-mini.zarr"
    objs = Dict{String, Vector{UInt8}}()
    objs["$zb/v/.zarray"] = _z_zarray((2, 3), (1, 3), "<f8")
    objs["$zb/v/.zattrs"] = Vector{UInt8}(codeunits(JSON.json(Dict("_ARRAY_DIMENSIONS" => ["time", "x"]))))
    objs["$zb/u/.zarray"] = _z_zarray((2, 3), (1, 3), "<f8")
    for r in 0:1
        objs["$zb/v/$r.0"] = _z_encode(reshape(Float64[10r, 10r + 1, 10r + 2], 1, 3))
        objs["$zb/u/$r.0"] = _z_encode(reshape(Float64[100r, 100r + 1, 100r + 2], 1, 3))
    end
    zc = Cache(_z_populate(mktempdir(), objs); offline = true, verify = false)
    err, msg = _raised(
        () -> read_store(
            ZarrReader(), zc, zb; variables = ["v"],
            select = Dict("dims" => Dict("time" => Dict("indices" => [0])))
        )
    )
    @test err isa ArgumentError && occursin("time", msg)
    v = read_store(
        ZarrReader(), zc, zb; variables = ["v"],
        select = Dict("dims" => Dict("time" => "all", "x" => Dict("indices" => [2, 0])))
    )
    @test v["v"].data == [2.0 0.0; 12.0 10.0]
    u = read_store(
        ZarrReader(), zc, zb; variables = ["u"],
        select = Dict("dims" => Dict("dim_1" => Dict("slice" => [1, 3])))
    )
    @test u["u"].dims == ["dim_0", "dim_1"]
    @test u["u"].data == [1.0 2.0; 101.0 102.0]
end

@testset "sample_at — refresh's data, from any thread" begin
    cache = Cache(LocalStore(joinpath(CORPUS, "cache")); offline = true, verify = true)
    src = _scenario("multi-window")["discrete"]["source"]
    for rps in (1, 2)
        p = _scenario_provider(cache, src, rps)
        ts = refresh_times(p)
        for t in ts
            s = sample_at(p, t)
            @test bitwise_same(s, refresh(p, t))
            # It depends on `t` only through the anchor `t` snaps to.
            @test bitwise_same(sample_at(p, t + 1799.5), s)
        end
        # ...and raises what refresh raises.
        @test_throws ArgumentError sample_at(p, first(ts) - 1)
        @test_throws ArgumentError refresh(p, first(ts) - 1)

        # Safe from several threads at once on ONE provider: every concurrent
        # sample equals the sequential one.
        want = [sample_at(p, t) for t in ts]
        got = Vector{Any}(undef, 4 * length(ts))
        @sync for i in eachindex(got)
            Threads.@spawn got[i] = sample_at(p, ts[mod1(i, length(ts))])
        end
        @test all(bitwise_same(got[i], want[mod1(i, length(ts))]) for i in eachindex(got))
    end

    # CONST: sample_at is materialize.
    mixed = _corpus_case("named-window-mixed-rank")["resolved_url"]
    pc = const_provider(cache, mixed; format = "netcdf")
    @test bitwise_same(sample_at(pc), materialize(pc))
    @test bitwise_same(sample_at(pc, 123.0), materialize(pc))

    # A one-file-per-tick DISCRETE source resolves its URL at the ANCHOR, not at
    # the query time, so it too depends on `t` only through the anchor.
    era5 = "https://data.earthsci.dev/era5/2018/11/20181108.nc"
    asked = Float64[]
    pd = discrete_provider(cache, t -> (push!(asked, t); era5), [0.0, 1.0]; format = "netcdf")
    sample_at(pd, 0.75)
    @test last(asked) == 0.0
    @test_throws ArgumentError sample_at(pd, -0.5)
end

@testset "DimWindow + with_window — a window bound at construction (§2.2–2.3)" begin
    cache = Cache(LocalStore(joinpath(CORPUS, "cache")); offline = true, verify = true)
    src = _scenario("multi-window")["discrete"]["source"]
    p = _scenario_provider(cache, src, 1)
    w = DimWindow("lat" => (1, 2), "lon" => [2, 0])

    # DimWindow spellings -> the wire vocabulary, 0-based.
    @test select_form(
        DimWindow(
            "a" => (1, 5, 2), "b" => Int[], "c" => :all, "d" => "all",
            "e" => Dict("indices" => [3])
        )
    ) ==
        Dict{String, Any}(
        "dims" => Dict{String, Any}(
            "a" => Dict{String, Any}("slice" => [1, 5, 2]),
            "b" => Dict{String, Any}("indices" => Int[]), "c" => "all", "d" => "all",
            "e" => Dict{String, Any}("indices" => [3])
        )
    )
    @test select_form(DimWindow()) == Dict{String, Any}("dims" => Dict{String, Any}())
    @test DimWindow(Dict("lat" => (1, 2))) == DimWindow("lat" => (1, 2))
    for bad in (
            () -> DimWindow("lat" => 1:3), () -> DimWindow("lat" => (1, 2, 3, 4)),
            () -> DimWindow("lat" => (1, 5, 0)), () -> DimWindow("lat" => [1.5]),
            () -> DimWindow("lat" => (1, 2), "lat" => [0]),
            () -> DimWindow("lat" => Dict("range" => [1, 2])),
            () -> select_form(Dict("axes" => Any["all"])),
            () -> select_form(Dict("axes" => Any["all"], "dims" => Dict())),
        )
        @test_throws ArgumentError bad()
    end

    # A NEW provider sharing the cache, with the window baked; the original is
    # unchanged.
    pw = with_window(p, w)
    @test pw isa Provider && pw !== p
    @test pw.cache === p.cache
    @test pw.reader_kwargs[:select] == select_form(w)
    @test !haskey(p.reader_kwargs, :select)
    for t in refresh_times(p)
        @test bitwise_same(sample_at(pw, t), slice_by_name(sample_at(p, t), w))
    end
    @test bitwise_same(
        sample_at(with_window(p, select_form(w)), first(refresh_times(p))),
        sample_at(pw, first(refresh_times(p)))
    )

    # Refused at once and before any I/O — this provider's resolver throws if it
    # is ever asked for a URL.
    noio = discrete_provider(
        cache, t -> error("with_window must not do I/O"),
        Float64.(src["refresh_times_s"]); format = "netcdf",
        time_dim = "time"
    )
    err, msg = _raised(() -> with_window(with_window(noio, w), w))      # two windows
    @test err isa ArgumentError && occursin("already bakes", msg)
    err, msg = _raised(() -> with_window(noio, DimWindow("time" => "all")))  # the time_dim
    @test err isa ArgumentError && occursin("time_dim", msg)
    openaq = "https://openaq-data-archive.s3.amazonaws.com/records/openaq/locationid=1/2018-11-08.csv"
    csv = const_provider(
        cache, openaq; format = "csv",
        reader_kwargs = (numeric_columns = ["latitude", "longitude", "value"],)
    )
    err, msg = _raised(() -> with_window(csv, w))                         # no selection
    @test err isa ArgumentError && occursin("supports_selection", msg)
    @test with_window(noio, w) isa Provider                               # a bare window is fine
end

@testset "sample_windows_at — the `multi-window` scenario (§2.4)" begin
    sc = _scenario("multi-window")
    cache = Cache(LocalStore(joinpath(CORPUS, "cache")); offline = true, verify = true)

    # CONST: the mixed-rank blob under overlapping, permuted, repeated, wrapping
    # and EMPTY windows.
    c = sc["const"]
    pc = const_provider(cache, c["url"]; format = c["format"], variables = String.(c["variables"]))
    W = c["windows"]
    got = sample_windows_at(pc, W)
    whole = materialize(pc)
    @test length(got) == length(W)
    for (i, w) in enumerate(W)
        @test bitwise_same(got[i], sample_at(with_window(pc, w)))
        @test bitwise_same(got[i], slice_by_name(whole, w))
    end
    @test bitwise_same(sample_windows_at(pc, 0.0, W), got)
    @test bitwise_same(sample_windows_at(pc, DimWindow.(c["windows"][i]["dims"] for i in eachindex(W))), got)

    # DISCRETE: the hourly source, records_per_sample 1 and 2, at every refresh
    # time — file seams included.
    d = sc["discrete"]
    for rps in d["records_per_sample"]
        p = _scenario_provider(cache, d["source"], rps)
        for t in Float64.(d["source"]["refresh_times_s"])
            g = sample_windows_at(p, t, d["windows"])
            sep = [sample_at(with_window(p, w), t) for w in d["windows"]]
            @test bitwise_same(g, sep)
            @test all(
                bitwise_same(g[i], slice_by_name(sample_at(p, t), w))
                    for (i, w) in enumerate(d["windows"])
            )
        end
    end

    # An empty list returns an empty list without fetching anything.
    noio = const_provider(cache, "https://nowhere.invalid/never.nc"; format = "netcdf")
    @test sample_windows_at(noio, Any[]) == NativeDataset[]

    # The with_window preconditions apply to every window, before any I/O.
    p1 = _scenario_provider(cache, d["source"], 1)
    t1 = first(refresh_times(p1))
    @test_throws ArgumentError sample_windows_at(
        p1, t1, [
            DimWindow("lat" => (0, 1)),
            DimWindow("time" => "all"),
        ]
    )
    @test_throws ArgumentError sample_windows_at(
        with_window(p1, DimWindow()), t1,
        [DimWindow("lat" => (0, 1))]
    )
    # ...and the reader's own checks still bind each window: a dimension the read
    # lacks, a time selection, and an index out of range — the last two where the
    # union reads that dimension whole, so the slice-back has to check them itself.
    err, msg = _raised(() -> sample_windows_at(pc, [DimWindow("lat" => [0]), DimWindow("depth" => "all")]))
    @test err isa ArgumentError && occursin("depth", msg)
    err, msg = _raised(() -> sample_windows_at(pc, [DimWindow("time" => [0]), DimWindow("lat" => [0])]))
    @test err isa ArgumentError && occursin("time", msg)
    err, msg = _raised(() -> sample_windows_at(pc, [DimWindow("lon" => [7]), DimWindow("lat" => [0])]))
    @test err !== nothing && occursin("out of range", msg)
    err, msg = _raised(() -> sample_windows_at(pc, [DimWindow("lon" => [7]), DimWindow("lon" => [0])]))
    @test err !== nothing && occursin("out of range", msg)
end
