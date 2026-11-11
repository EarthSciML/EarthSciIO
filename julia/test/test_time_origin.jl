# A provider that counts time from a time origin (spec/esm-sources.md §4): the
# shared `time-origin` scenario, plus the Julia spellings.
using Dates

@testset "time origin — the shared scenario" begin
    corpus = joinpath(@__DIR__, "..", "..", "conformance", "corpus")
    sc = JSON.parsefile(joinpath(corpus, "scenarios", "time-origin.json"))
    cache = Cache(; root = joinpath(corpus, "cache"), offline = true)
    flat(x::AbstractVector) = reduce(vcat, (flat(e) for e in x); init = Float64[])
    flat(x::Real) = Float64[x]
    corder(a::AbstractArray) = vec(permutedims(a, reverse(1:ndims(a))))
    function build(
            case, rps = nothing; time_origin = case["options"]["time_origin"],
            window = (Float64(case["options"]["window"][1]), Float64(case["options"]["window"][2]))
        )
        o = case["options"]
        return provider_from_esm(
            case["name"], case["entry"], cache;
            variables = String.(o["variables"]),
            substitutions = Dict{String, String}(o["substitutions"]),
            records_per_sample = rps,
            window = window, time_origin = time_origin
        )
    end
    for case in sc["cases"]
        @testset "$(case["id"])" begin
            if haskey(case, "error")
                msg = try
                    build(case)
                    ""
                catch e
                    sprint(showerror, e)
                end
                @test occursin(case["error"], msg)
                continue
            end
            exp = case["expect"]
            @test refresh_times(build(case)) == Float64.(exp["refresh_times"])
            for (rps, want) in exp["window_records"]
                @test window_records(build(case, parse(Int, rps))) == Float64.(want)
            end
            g = PrefetchGroup(Dict("m" => build(case, 1)); depth = 2)
            try
                @test boundaries(g) == Float64[e["boundary"] for e in exp["epochs"]]
                start!(g, :forward)
                for (k, e) in enumerate(exp["epochs"])
                    @test take!(g, k)["m"].anchor == e["anchor"]
                end
            finally
                close(g)
            end
            var = case["options"]["variables"][1]
            tdim = case["entry"]["temporal"]["time_variable"]
            for smp in exp["samples"]
                p = build(case, smp["records_per_sample"])
                nds = sample_at(p, Float64(smp["t"]))
                @test corder(nds[var].data) == flat(smp[var])
                @test corder(refresh(p, smp["t"])[var].data) == flat(smp[var])
                if haskey(smp, "time")
                    @test nds.coords[tdim].data == Float64.(smp["time"])
                    @test nds.coords[tdim].attrs["units"] == exp["time_units"]
                end
            end
        end
    end

    nei = only(c for c in sc["cases"] if c["id"] == "nei-monthly")
    @testset "origin spellings agree" begin
        for o in (DateTime(2016), "2016-01-01T00:00:00Z", "2016-01-01")
            @test refresh_times(build(nei; time_origin = o)) == [0.0, 2678400.0, 5184000.0]
        end
    end
    @testset "a DateTime window bound is still an instant" begin
        o = nei["options"]
        p = provider_from_esm(
            nei["name"], nei["entry"], cache;
            variables = String.(o["variables"]),
            substitutions = Dict{String, String}(o["substitutions"]),
            window = (DateTime(2016), DateTime(2016, 4)), time_origin = DateTime(2016)
        )
        @test refresh_times(p) == [0.0, 2678400.0, 5184000.0]
    end
    @testset "a malformed or fractional origin is refused" begin
        @test_throws TimeOriginInvalid build(nei; time_origin = DateTime(2016, 1, 1, 0, 0, 0, 500))
        @test_throws TimeOriginInvalid build(nei; time_origin = "January")
        @test_throws TimeOriginInvalid build(nei; time_origin = 0.0)
    end
    @testset "a times-based provider refuses an origin" begin
        @test_throws ArgumentError Provider(
            cache, "file:///x.nc"; format = "netcdf", cadence = EarthSciIO.DISCRETE,
            times = [0.0, 1.0], time_origin = DateTime(2016)
        )
    end
    @testset "with_window keeps the origin" begin
        p = build(nei)
        w = with_window(p, DimWindow())
        @test refresh_times(w) == refresh_times(p)
        @test w.plan.clock.origin == DateTime(2016)
    end
    @testset "a group refuses members on two clocks" begin
        a = build(nei)
        b = build(nei; time_origin = DateTime(2016, 1, 2), window = (0.0, 86400.0))
        @test_throws ArgumentError PrefetchGroup(Dict("a" => a, "b" => b))
    end
    @testset "a times-based member has no origin" begin
        # Its ticks are in the caller's unit, with no origin (it cannot take
        # one): it never joins a member on an origin, as Python and Rust refuse
        # a member without one beside a member with one.
        ticks() = Provider(
            cache, "file:///x.nc"; format = "netcdf", cadence = EarthSciIO.DISCRETE,
            times = [0.0, 2678400.0]
        )
        msg = try
            PrefetchGroup(Dict("a" => build(nei), "t" => ticks()))
            ""
        catch e
            e isa ArgumentError ? e.msg : sprint(showerror, e)
        end
        @test occursin("time origins differ", msg)
        # Beside a member without an origin it is accepted, as before.
        o = nei["options"]
        unix = provider_from_esm(
            nei["name"], nei["entry"], cache;
            variables = String.(o["variables"]),
            substitutions = Dict{String, String}(o["substitutions"]),
            window = (DateTime(2016), DateTime(2016, 4))
        )
        g = PrefetchGroup(Dict("u" => unix, "t" => ticks()))
        close(g)
    end
end
