# The active `geotiff` reader (gap G3, Julia track) — raster bands → native grid.
#
# Julia sibling of tests/test_geotiff_reader.py. The decode backend is the
# pure-Julia TiffImages package, supplied by the `EarthSciIOTiffImagesExt` weakdep
# extension (`using TiffImages` below triggers it). The fixtures
# (test/fixtures/*.tif) were authored with the SAME georef tags the Python test
# writes via tifffile (ModelPixelScale/ModelTiepoint/GeoKeyDirectory/GDAL_NODATA),
# so the two tracks decode the identical contract: bands → Band1..N, cell-center
# axes, GDAL_NODATA → NaN, geographic→lon/lat vs projected→x/y.
using EarthSciIO
using Test
using TiffImages          # triggers EarthSciIOTiffImagesExt — the decode backend

const GEO_FIX = joinpath(@__DIR__, "fixtures", "fuel_model_geographic.tif")
const PROJ_FIX = joinpath(@__DIR__, "fixtures", "elev_projected.tif")

# Fixture parameters (must match test/fixtures generation): a 4(lat)×3(lon) raster,
# EPSG:4326, north-up, top-left CORNER at (LON0,LAT0), 0.5° cells, cell (2,2)=NODATA.
const LON0, LAT0, RES, NODATA = -121.5, 40.0, 0.5, -9999.0
const EXP_LON = [-121.25, -120.75, -120.25]          # LON0 + (col+0.5)*RES
const EXP_LAT = [39.75, 39.25, 38.75, 38.25]         # LAT0 - (row+0.5)*RES

@testset "geotiff reader (TiffImages backend, gap G3)" begin
    @testset "registered active in the format registry" begin
        @test haskey(EarthSciIO.FORMAT_REGISTRY, "geotiff")
        @test EarthSciIO.status_of(EarthSciIO.FORMAT_REGISTRY, "geotiff") == :active
        @test EarthSciIO.FORMAT_REGISTRY["geotiff"] isa GeoTIFFReader
    end

    @testset "decode: band → Band1, georef → lon/lat, GDAL_NODATA → NaN" begin
        nds = read_native(GeoTIFFReader(), GEO_FIX)
        @test nds isa EarthSciIO.NativeDataset
        @test variable_names(nds) == ["Band1"]           # 1-based GDAL convention
        @test coord_names(nds) == ["lat", "lon"]         # geographic CRS → lon/lat

        band = nds["Band1"]
        @test band.dims == ["lat", "lon"]                # on-disk (row=lat, col=lon)
        @test size(band.data) == (4, 3)
        @test eltype(band.data) == Float64               # numeric → float64 (§3)
        @test isnan(band.data[2, 2])                     # GDAL_NODATA sentinel → NaN
        # every other cell is its raw value (row-major 0..11), untouched
        expected = reshape(Float64.(0:11), 3, 4)'        # row-major fill
        for i in 1:4, j in 1:3
            (i == 2 && j == 2) && continue
            @test band.data[i, j] == expected[i, j]
        end

        @test isapprox(nds["lon"].data, EXP_LON; atol = 1e-12)
        @test isapprox(nds["lat"].data, EXP_LAT; atol = 1e-12)
        @test nds["lon"].dims == ["lon"]
        @test nds["lat"].dims == ["lat"]
    end

    @testset "projected raster → x/y axes" begin
        nds = read_native(GeoTIFFReader(), PROJ_FIX)
        @test coord_names(nds) == ["x", "y"]             # GTModelTypeGeoKey=1 (projected)
        @test nds["Band1"].dims == ["y", "x"]
        @test size(nds["Band1"].data) == (2, 3)
    end

    @testset "band_names renames positionally; variables restricts" begin
        nds = read_native(GeoTIFFReader(), GEO_FIX; band_names = ["fuel"])
        @test variable_names(nds) == ["fuel"]
        @test nds["fuel"].dims == ["lat", "lon"]

        sel = read_native(GeoTIFFReader(), GEO_FIX; variables = ["Band1"])
        @test variable_names(sel) == ["Band1"]
        @test_throws Exception read_native(GeoTIFFReader(), GEO_FIX; band_names = ["a", "b"])
        @test_throws Exception read_native(GeoTIFFReader(), GEO_FIX; variables = ["Band9"])
    end
end
