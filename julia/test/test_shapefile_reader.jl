# The active `shapefile` reader (Julia track) — an ESRI shapefile as a feature
# table. Sibling of tests/test_shapefile_reader.py and rust/tests/shapefile_reader.rs.
#
# The decode backend is Shapefile.jl, supplied by the `EarthSciIOShapefileExt`
# weakdep extension (`using Shapefile` below triggers it). The fixture is the
# COMMITTED conformance blob `shapefile-polygon-zip` — the same bytes the Python
# and Rust tracks read — so this file checks the contract (one row per part, the
# esm-spec §8.6.1 padding, the `*`-only deletion rule, the dtype rules, the
# stored bbox, the `meta` fields) and the Julia-side seams the corpus case cannot
# reach with a single blob: member selection, a bare `.shp`, variable filtering
# and the reserved-name collision.
using EarthSciIO
using Test
using Shapefile          # triggers EarthSciIOShapefileExt — the decode backend
import ZipFile
import JSON

const SHP_CASE = JSON.parsefile(joinpath(@__DIR__, "..", "..", "conformance",
                                         "corpus", "cases", "shapefile-polygon-zip.json"))
const SHP_BLOB = joinpath(@__DIR__, "..", "..", "conformance", "corpus",
                          SHP_CASE["blob_path"])
const SHP_MEMBER = SHP_CASE["decode"]["member"]

# The corpus blob's members, so a test can rezip a variant of them.
function _shp_members()
    out = Pair{String,Vector{UInt8}}[]
    r = ZipFile.Reader(SHP_BLOB)
    try
        for f in r.files
            push!(out, f.name => read(f))
        end
    finally
        close(r)
    end
    return out
end

function _rezip(path, members)
    w = ZipFile.Writer(path)
    try
        for (name, bytes) in members
            f = ZipFile.addfile(w, name)
            write(f, bytes)
        end
    finally
        close(w)
    end
    return path
end

@testset "shapefile reader (Shapefile.jl backend)" begin
    @testset "registered active in the format registry" begin
        @test haskey(EarthSciIO.FORMAT_REGISTRY, "shapefile")
        @test EarthSciIO.status_of(EarthSciIO.FORMAT_REGISTRY, "shapefile") == :active
        @test EarthSciIO.FORMAT_REGISTRY["shapefile"] isa ShapefileReader
    end

    ds = read_native(ShapefileReader(), SHP_BLOB; member = SHP_MEMBER,
                     numeric_columns = ["CODE"])

    @testset "one row per part, attributes replicated" begin
        # 5 records, one `*`-deleted; record 1 has a mainland + an island.
        @test ds.variables["shape_index"].data == [0, 1, 1, 2, 4]
        @test ds.variables["part_index"].data == [0, 0, 1, 0, 0]
        @test ds.variables["n_parts"].data == [1, 2, 2, 1, 1]
        @test ds.variables["NAME"].data == ["Alpha", "Bravo", "Bravo", "Charlie", "Echo"]
        # The `*` row is gone; the NUL-flagged one ("Echo") is NOT a deletion.
        @test !("Deleted" in ds.variables["NAME"].data)
    end

    @testset "esm-spec 8.6.1 padding repeats the final vertex" begin
        g = ds.variables["geometry"]
        @test g.dims == ["index", "vertex", "xy"]
        @test size(g.data) == (5, 5, 2)
        @test ds.variables["n_vertices"].data == [5, 5, 4, 4, 5]
        # Row 3 ("Charlie") is a 4-vertex ring in a 5-vertex stack: the last slot
        # repeats the final vertex, and no slot is NaN.
        @test g.data[4, 5, :] == g.data[4, 4, :]
        @test !any(isnan, g.data)
    end

    @testset "nvert_max lets the document declare the vertex axis" begin
        wide = read_native(ShapefileReader(), SHP_BLOB; member = SHP_MEMBER, nvert_max = 8)
        @test size(wide.variables["geometry"].data) == (5, 8, 2)
        # The extra slots still repeat the final vertex, so the ring is unchanged.
        @test wide.variables["geometry"].data[3, 8, :] ==
              wide.variables["geometry"].data[3, 4, :]
        @test_throws ArgumentError read_native(ShapefileReader(), SHP_BLOB;
                                               member = SHP_MEMBER, nvert_max = 4)
    end

    @testset "the stored per-record bbox, replicated to parts" begin
        @test ds.variables["xmin"].data == [0.0, 4.0, 4.0, 0.0, 0.0]
        @test ds.variables["xmax"].data == [2.0, 8.0, 8.0, 2.0, 1.0]
        @test ds.variables["ymax"].data == [2.0, 2.0, 2.0, 6.0, 9.0]
    end

    @testset "dtypes: C/N/L and the numeric_columns override" begin
        @test eltype(ds.variables["NAME"].data) == String
        @test eltype(ds.variables["FLAG"].data) == Bool          # `L`, not Float64
        @test ds.variables["FLAG"].data == [true, false, false, true, true]
        @test isnan(ds.variables["EMIS"].data[4])                # blank `N` -> NaN
        @test ds.variables["CODE"].data == [1001.0, 17031.0, 17031.0, 6037.0, 36061.0]
        plain = read_native(ShapefileReader(), SHP_BLOB; member = SHP_MEMBER)
        @test eltype(plain.variables["CODE"].data) == String     # a `C` column by default
        @test_throws KeyError read_native(ShapefileReader(), SHP_BLOB;
                                          member = SHP_MEMBER, numeric_columns = ["NOPE"])
    end

    @testset "shape_type and the .prj ride as one-element `meta` fields" begin
        @test ds.variables["shape_type"].dims == ["meta"]
        @test ds.variables["shape_type"].data == ["Polygon"]
        @test startswith(ds.variables["crs_wkt"].data[1], "GEOGCS[")
    end

    @testset "member selection" begin
        mktempdir() do dir
            members = _shp_members()
            # A single `.shp` member needs no `member` option.
            one = _rezip(joinpath(dir, "one.zip"), members)
            @test read_native(ShapefileReader(), one).variables["n_parts"].data ==
                  ds.variables["n_parts"].data
            # Two layers in one archive: ambiguous without `member`.
            two = _rezip(joinpath(dir, "two.zip"),
                         vcat(members, [replace(n, "layer/" => "other/") => b
                                        for (n, b) in members]))
            @test_throws KeyError read_native(ShapefileReader(), two)
            @test read_native(ShapefileReader(), two;
                              member = "other/emis_polygons.shp"
                              ).variables["NAME"].data == ds.variables["NAME"].data
            @test_throws KeyError read_native(ShapefileReader(), two; member = "nope.shp")
        end
    end

    @testset "a bare .shp blob decodes geometry without attributes" begin
        mktempdir() do dir
            bare = joinpath(dir, "blob")
            write(bare, first(b for (n, b) in _shp_members() if endswith(n, ".shp")))
            g = read_native(ShapefileReader(), bare)
            # No `.dbf` => nothing is deleted, so the `*` record's shape is present.
            @test size(g.variables["geometry"].data, 1) == 6
            @test !haskey(g.variables, "NAME")
            @test !haskey(g.variables, "crs_wkt")
        end
    end

    @testset "variable selection" begin
        sel = read_native(ShapefileReader(), SHP_BLOB; member = SHP_MEMBER,
                          variables = ["geometry", "EMIS"])
        @test sort(collect(keys(sel.variables))) == ["EMIS", "geometry"]
        @test_throws KeyError read_native(ShapefileReader(), SHP_BLOB;
                                          member = SHP_MEMBER, variables = ["nope"])
    end

    @testset "a .dbf column named like a reader field is refused" begin
        mktempdir() do dir
            members = map(_shp_members()) do (name, bytes)
                if endswith(name, ".dbf")
                    b = copy(bytes)
                    b[33:43] = Vector{UInt8}("xmin\0\0\0\0\0\0\0")  # rename field 1
                    return name => b
                end
                return name => bytes
            end
            clash = _rezip(joinpath(dir, "clash.zip"), members)
            @test_throws ArgumentError read_native(ShapefileReader(), clash)
        end
    end
end
