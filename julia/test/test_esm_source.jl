# A provider from an esm data_sources entry (spec/esm-sources.md §3): the shared
# `esm-sources` scenario.
using Dates

@testset "provider_from_esm — the shared scenario" begin
    corpus = joinpath(@__DIR__, "..", "..", "conformance", "corpus")
    sc = JSON.parsefile(joinpath(corpus, "scenarios", "esm-sources.json"))
    cache = Cache(; root = joinpath(corpus, "cache"), offline = true)
    flat(x::AbstractVector) = reduce(vcat, (flat(e) for e in x); init = Float64[])
    flat(x::Real) = Float64[x]
    corder(a::AbstractArray) = vec(permutedims(a, reverse(1:ndims(a))))
    function build(case, rps = nothing)
        o = case["options"]
        window = haskey(o, "window_s") ? (Float64(o["window_s"][1]), Float64(o["window_s"][2])) : nothing
        return provider_from_esm(
            case["name"], case["entry"], cache;
            variables = String.(get(o, "variables", String[])),
            substitutions = Dict{String, String}(get(o, "substitutions", Dict())),
            url_override = get(o, "url_override", nothing),
            records_per_sample = rps, window = window
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
                for m in case["mentions"]
                    @test occursin(m, msg)
                end
                continue
            end
            exp = case["expect"]
            p = build(case)
            @test p.format == exp["format"]
            @test is_const(p) == exp["const"]
            @test refresh_times(p) == Float64.(exp["refresh_times_s"])
            exp["const"] || @test p.time_dim == exp["time_dim"]
            for f in exp["files"]
                t = Float64(f["t_s"])
                @test p.url_for(t) == f["url"]
                @test p.mirrors_for(t) == String.(f["mirrors"])
                if !exp["const"]
                    temporal = p.plan.temporal
                    a = record_anchor(temporal, t)
                    fa = file_anchor(temporal, a)
                    @test datetime2unix(fa) == f["file_anchor_s"]
                    @test record_in_file(temporal, a, fa) == f["record"]
                end
            end
            var = case["options"]["variables"][1]
            for smp in get(exp, "samples", Any[])
                nds = sample_at(build(case, smp["records_per_sample"]), Float64(smp["t_s"]))
                @test corder(nds[var].data) == flat(smp[var])
                haskey(smp, "time_s") &&
                    @test nds.coords[exp["time_dim"]].data == Float64.(smp["time_s"])
            end
        end
    end
end
