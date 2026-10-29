using EarthSciIO
using Test
using SHA: sha256
import JSON
import Blosc   # activates the EarthSciIOBloscExt weakdep ext for the zarr decode
import Shapefile  # activates the EarthSciIOShapefileExt weakdep ext for the shapefile decode
import Parquet2   # activates the EarthSciIOParquet2Ext weakdep ext for the parquet decode
import TiffImages  # activates the EarthSciIOTiffImagesExt weakdep ext for the geotiff decode

# The PrefetchGroup tests take a different path on one thread (decode inside
# take!, spec/prefetch.md §3.10) than on several; say which one this run took.
@info "EarthSciIO tests" Threads.nthreads()

@testset "EarthSciIO — Julia track: cache + transport + store + readers + provider" begin
    # component (a): cache / transport / store
    include("test_cache.jl")
    include("test_file_source_revalidate.jl")   # rung 0: the file:// source recheck
    include("test_registries.jl")
    include("test_conformance.jl")   # defines CORPUS; checks 1, 2, 5
    include("test_http.jl")
    include("test_cds.jl")
    include("test_concurrency.jl")
    # component (b): format readers + cadence provider (esio-9nb.5)
    include("test_readers.jl")       # checks 3, 4; defines the corpus comparison helpers
    include("test_geotiff_reader.jl")  # gap G3: geotiff reader via the TiffImages weakdep ext
    include("test_shapefile_reader.jl")  # shapefile reader via the Shapefile.jl weakdep ext
    include("test_parquet_reader.jl")  # parquet reader via the Parquet2.jl weakdep ext
    include("test_zarr.jl")          # store-backed zarr reader: lazy orthogonal selection
    include("zarr_write_test.jl")    # zarr v3 sharded WRITER round-trip (write -> read)
    include("test_provider.jl")      # cadence + the full offline pipeline; reuses helpers
    # epoch prefetch + named-dimension windows (spec/prefetch.md, RFC U11)
    include("test_windows.jl")       # `dims` select, sample_at, with_window, multi-window scenario
    include("test_prefetch.jl")      # PrefetchGroup: equivalence + errors scenarios, API, fallback
end
