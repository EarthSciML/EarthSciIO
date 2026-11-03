# The `json` points reader against the shared cross-language cases, which the
# Python (`tests/test_json_reader.py`) and Rust (`rust/tests/json_reader.rs`)
# suites read too.

using EarthSciIO
using Test
import JSON

const JSON_READER_CASES = JSON.parse(
    read(
        joinpath(@__DIR__, "..", "..", "conformance", "corpus", "scenarios", "json-reader.json"),
        String
    )
)["cases"]

function _json_case_decode(dir, case)
    blob = joinpath(dir, String(case["id"]) * ".json")
    write(blob, String(case["document"]))
    opts = Dict{Symbol, Any}(Symbol(k) => v for (k, v) in get(case, "options", Dict()))
    vars = get(case, "variables", nothing)
    vars === nothing || (opts[:variables] = String.(vars))
    return read_native(JSONReader(), blob; opts...)
end

@testset "json reader: shared cases" begin
    mktempdir() do dir
        for case in JSON_READER_CASES
            id = String(case["id"])
            @testset "$id" begin
                if haskey(case, "error")
                    err = try
                        _json_case_decode(dir, case)
                        nothing
                    catch e
                        e
                    end
                    @test err isa JSONReaderError
                    err isa JSONReaderError && @test err.code == case["error"]
                    err isa JSONReaderError || @info "unexpected" id err
                    continue
                end
                nds = _json_case_decode(dir, case)
                want = case["expected"]["fields"]
                @test Set(keys(nds.variables)) == Set(String.(collect(keys(want))))
                @test isempty(nds.coords)
                for (name, spec) in want
                    f = nds.variables[String(name)]
                    @test f.dims == ["index"]
                    @test length(f.data) == case["expected"]["length"]
                    dt = spec["dtype"]
                    if dt == "string"
                        @test f.data isa Vector{String}
                        @test f.data == String.(spec["data"])
                    elseif dt == "bool"
                        @test f.data isa Vector{Bool}
                        @test f.data == Bool.(spec["data"])
                    else
                        @test f.data isa Vector{Float64}
                        for (got, exp) in zip(f.data, spec["data"])
                            exp === nothing ? (@test isnan(got)) : (@test got == Float64(exp))
                        end
                    end
                end
            end
        end
    end
end

@testset "json reader: registry and provider" begin
    @test haskey(FORMAT_REGISTRY, "json")
    @test status_of(FORMAT_REGISTRY, "json") == :active
    @test Set(reader_option_keys(JSONReader())) == Set([:variables, :records_path, :numeric_columns])
    case = only(c for c in JSON_READER_CASES if c["id"] == "station-discovery")
    mktempdir() do dir
        blob = joinpath(dir, "stations.json")
        write(blob, String(case["document"]))
        cache = Cache(LocalStore(joinpath(dir, "cache")))
        p = const_provider(
            cache, "file://" * blob; format = "json", source_loader = "stations",
            variables = ["coordinates.latitude", "name"],
            reader_kwargs = (; records_path = ["results"])
        )
        nds = materialize(p)
        @test Set(keys(nds.variables)) == Set(["coordinates.latitude", "name"])
        @test nds.variables["name"].data == ["Site A", "Site B"]
        # An unrecognised option is refused when the provider is built.
        @test_throws ArgumentError const_provider(
            cache, "file://" * blob; format = "json",
            reader_kwargs = (; record_path = ["results"])
        )
    end
end
