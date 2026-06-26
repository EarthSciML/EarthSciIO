using EarthSciIO
using Test
using SHA: sha256
import JSON

@testset "EarthSciIO — Julia track, component (a): cache + transport + store" begin
    include("test_cache.jl")
    include("test_registries.jl")
    include("test_conformance.jl")
    include("test_http.jl")
    include("test_concurrency.jl")
end
