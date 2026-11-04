# Finding a record by the file's own time axis (spec/esm-sources.md §2.5): the
# shared `decoded-time-axis` scenario — the CF decode, and a `temporal` Provider
# over trimmed, irregular and undecodable files in the corpus.
using Dates

@testset "decoded time axis — the shared scenario" begin
    corpus = joinpath(@__DIR__, "..", "..", "conformance", "corpus")
    sc = JSON.parsefile(joinpath(corpus, "scenarios", "decoded-time-axis.json"))
    flat(x::AbstractVector) = reduce(vcat, (flat(e) for e in x); init = Float64[])
    flat(x::Real) = Float64[x]
    corder(a::AbstractArray) = vec(permutedims(a, reverse(1:ndims(a))))

    @testset "decode $(c["id"])" for c in sc["decode"]
        vals = c["dtype"] == "float64" ?
            Float64[v === nothing ? NaN : Float64(v) for v in c["values"]] :
            Int64[Int64(v) for v in c["values"]]
        got = decode_time_axis(vals, c["units"], c["calendar"])
        want = c["seconds"] === nothing ? nothing : Int64.(c["seconds"])
        @test got == want
    end

    cache = Cache(; root = joinpath(corpus, "cache"), offline = true)
    for (name, src) in sc["providers"], rps in (1, 2)
        @testset "provider $name, $rps record(s) per sample" begin
            temporal = SourceTemporal(
                Float64(src["start_s"]), src["frequency"], src["file_period"];
                stop = Float64(src["end_s"]), time_dim = src["time_dim"],
                records_per_sample = rps
            )
            p = Provider(
                cache, EsmTemplate(src["url_template"]); format = src["format"],
                variables = String.(src["variables"]), temporal = temporal,
                window = (Float64(src["window_s"][1]), Float64(src["window_s"][2]))
            )
            for smp in src["samples"]
                smp["records_per_sample"] == rps || continue
                nds = sample_at(p, Float64(smp["t_s"]))
                @test corder(nds["T"].data) == flat(smp["T"])
                if rps == 2
                    @test nds.coords["time"].data == Float64.(smp["time_s"])
                end
            end
            for err in src["errors"]
                err["records_per_sample"] == rps || continue
                @test_throws Exception sample_at(p, Float64(err["t_s"]))
            end
        end
    end
end
