using EarthSciIO
using Test
using SHA: sha256
import JSON

@testset "EarthSciIO — Julia track: cache + transport + store + readers + provider" begin
    # component (a): cache / transport / store
    include("test_cache.jl")
    include("test_registries.jl")
    include("test_conformance.jl")   # defines CORPUS; checks 1, 2, 5
    include("test_http.jl")
    include("test_cds.jl")
    include("test_concurrency.jl")
    # component (b): format readers + cadence provider (esio-9nb.5)
    include("test_readers.jl")       # checks 3, 4; defines the corpus comparison helpers
    include("test_geotiff_reader.jl")  # gap G3: geotiff reader via the TiffImages weakdep ext
    include("test_provider.jl")      # cadence + the full offline pipeline; reuses helpers
end
