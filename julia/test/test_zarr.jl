# The `zarr` store-backed reader (Zarr v2) — chunk math, orthogonal selection,
# the partial edge chunk, fill_value != NaN, and the load-bearing LAZINESS
# capability (a runtime index list fetches ONLY the intersecting chunk objects).
#
# Two complementary checks:
#   * against the COMMITTED corpus store (blosc-encoded by numcodecs in the Python
#     generator) — so this also proves numcodecs <-> Blosc.jl byte-compatibility;
#   * against a Julia-built POISON store — non-selected chunks hold undecodable
#     garbage, so any over-fetch blosc-errors instead of silently succeeding.

import Blosc   # activates the EarthSciIOBloscExt weakdep extension

const ZCORPUS = normpath(joinpath(@__DIR__, "..", "..", "conformance", "corpus"))
const ZBASE = "s3://earthsci-fixtures/mini.zarr"

# C-order flatten of a native array in file (dims) order (mirrors dump_julia).
_corder(a::AbstractVector) = collect(a)
_corder(a::AbstractArray) = vec(permutedims(a, reverse(1:ndims(a))))

@testset "zarr: chunk math" begin
    @test EarthSciIO._chunk_key((0, 5, 0), ".") == "0.5.0"
    @test EarthSciIO._chunk_key((3,), ".") == "3"
    @test EarthSciIO._chunk_key((1, 2), "/") == "1/2"

    # dim1 chunk_len 100: indices [0,250,260] -> chunks {0,2}; chunk 1 skipped.
    got = EarthSciIO._needed_chunks([[1], [0, 250, 260], [0]], [1, 100, 1])
    @test Set(got) == Set([(1, 0, 0), (1, 2, 0)])

    # a 3-index selection over 525 chunks touches <= 3 chunks, never 525.
    got2 = EarthSciIO._needed_chunks([[0], [50, 12345, 52000], [0]], [1, 100, 52411])
    @test Set(c[2] for c in got2) == Set([0, 123, 520])
    @test length(got2) == 3

    @test EarthSciIO._resolve_axis(EarthSciIO._parse_axis("all"), 4) == [0, 1, 2, 3]
    @test EarthSciIO._resolve_axis(EarthSciIO._parse_axis(Dict("indices" => [3, 0, 1])), 4) == [3, 0, 1]
    @test EarthSciIO._resolve_axis(EarthSciIO._parse_axis(Dict("slice" => [1, 8, 2])), 10) == [1, 3, 5, 7]
end

@testset "zarr: read_store over the committed corpus (numcodecs<->Blosc.jl)" begin
    cache = Cache(LocalStore(joinpath(ZCORPUS, "cache")); offline = true, verify = true)
    sel = Dict("axes" => Any[Dict("indices" => [1]), Dict("indices" => [1, 4]), "all"])
    nds = read_store(ZarrReader(), cache, "s3://earthsci-fixtures/isrm-mini.zarr";
                     variables = ["field3d", "pop1d"], select = sel)

    f3 = nds.variables["field3d"]
    @test f3.dims == ["layer", "y", "x"]
    @test size(f3.data) == (1, 2, 4)
    @test eltype(f3.data) == Float64
    @test _corder(f3.data) == Float64[110, 111, 112, 113, 140, 141, 142, 143]

    p1 = nds.variables["pop1d"]
    @test p1.dims == ["cell"]
    @test p1.data == Float64[1, 3, 5, 7, 9, 11, 13, 15]  # rank 1 != 3 axes -> whole
end

@testset "zarr: full read + partial edge chunk (fill_value 0.0 not -> NaN)" begin
    cache = Cache(LocalStore(joinpath(ZCORPUS, "cache")); offline = true, verify = true)
    nds = read_store(ZarrReader(), cache, "s3://earthsci-fixtures/isrm-mini.zarr";
                     variables = ["field3d"], select = nothing)
    f3 = nds.variables["field3d"]
    @test size(f3.data) == (2, 5, 4)               # full array
    @test vec(f3.data[2, 5, :]) == Float64[140, 141, 142, 143]  # partial edge chunk row
    @test !any(isnan, f3.data)                     # zeros stay zeros, never NaN
end

# --- Julia-built poison store: the LAZINESS proof --------------------------- #

function _z_zarray(shape, chunks, dtype)
    d = Dict("zarr_format" => 2, "shape" => collect(shape), "chunks" => collect(chunks),
             "dtype" => dtype,
             "compressor" => Dict("id" => "blosc", "cname" => "lz4", "clevel" => 5,
                                  "shuffle" => 1, "blocksize" => 0),
             "fill_value" => 0.0, "order" => "C", "filters" => nothing,
             "dimension_separator" => nothing)
    return Vector{UInt8}(codeunits(JSON.json(d)))
end

function _z_encode(chunk::AbstractArray)
    Blosc.set_compressor("lz4")
    flatC = _corder(chunk)                          # C-order bytes
    return Blosc.compress(flatC; level = 5, shuffle = true, itemsize = sizeof(eltype(chunk)))
end

function _z_populate(root, objects)
    store = LocalStore(root)
    for (url, data) in objects
        key = cache_key(url)
        staged = EarthSciIO.staging_path(store)
        write(staged, data)
        EarthSciIO.put_blob!(store, key, staged; ext = "")
        m = EarthSciIO.Manifest(url, nothing, nothing, bytes2hex(sha256(data)),
                                length(data), "2026-06-26T00:00:00Z", nothing, nothing)
        EarthSciIO.put_meta!(store, key, m)
    end
    return store
end

@testset "zarr: laziness never touches unselected (poison) chunks" begin
    tmp = mktempdir()
    objs = Dict{String,Vector{UInt8}}()
    objs["$ZBASE/sr/.zarray"] = _z_zarray((3, 500, 1), (1, 100, 1), "<f4")
    objs["$ZBASE/sr/.zattrs"] =
        Vector{UInt8}(codeunits(JSON.json(Dict("_ARRAY_DIMENSIONS" => ["layer", "source", "receptor"]))))
    # 3 layers x 5 source-chunks x 1 = 15 chunks. Only layer 0, source-chunks {0,3}
    # are valid; every other chunk is poison (garbage that fails blosc decode).
    for c0 in 0:2, c1 in 0:4
        key = "$ZBASE/sr/$c0.$c1.0"
        if c0 == 0 && (c1 == 0 || c1 == 3)
            objs[key] = _z_encode(fill(Float32(c0 * 1000 + c1), (1, 100, 1)))
        else
            objs[key] = Vector{UInt8}(b"\x00POISON-not-a-blosc-container\xff")
        end
    end
    store = _z_populate(tmp, objs)
    cache = Cache(store; offline = true, verify = true)

    sel = Dict("axes" => Any[Dict("indices" => [0]),
                             Dict("indices" => [5, 12, 305, 340]), "all"])
    nds = read_store(ZarrReader(), cache, ZBASE; variables = ["sr"], select = sel)
    f = nds.variables["sr"]
    @test size(f.data) == (1, 4, 1)
    @test vec(f.data) == Float64[0, 0, 3, 3]   # sources 5,12->chunk0; 305,340->chunk3

    # Control: a selection that DOES hit a poison chunk decode-errors.
    badsel = Dict("axes" => Any[Dict("indices" => [0]), Dict("indices" => [150]), "all"])
    @test_throws Exception read_store(ZarrReader(), cache, ZBASE;
                                      variables = ["sr"], select = badsel)
end

@testset "zarr: registry dispatch + store-backed provider seam" begin
    @test status_of(FORMAT_REGISTRY, "zarr") == :active
    @test store_backed(FORMAT_REGISTRY["zarr"])
    @test !store_backed(FORMAT_REGISTRY["netcdf"])

    cache = Cache(LocalStore(joinpath(ZCORPUS, "cache")); offline = true, verify = true)
    p = const_provider(cache, "s3://earthsci-fixtures/isrm-mini.zarr";
                       format = "zarr", variables = ["field3d"],
                       reader_kwargs = (; select = Dict("axes" =>
                           Any[Dict("indices" => [1]), Dict("indices" => [1, 4]), "all"])))
    nds = materialize(p)
    @test size(nds.variables["field3d"].data) == (1, 2, 4)
end

@testset "zarr: s3 transport rewrite" begin
    @test status_of(TRANSPORT_REGISTRY, "s3") == :active
    @test EarthSciIO.s3_https_url("s3://inmap-model/isrm_v1.2.1.zarr/PrimaryPM25/0.5.0") ==
          "https://inmap-model.s3.us-east-2.amazonaws.com/isrm_v1.2.1.zarr/PrimaryPM25/0.5.0"
    @test EarthSciIO.s3_https_url("s3://b/k/o"; ) isa String
    withenv("EARTHSCI_S3_REGION" => "eu-west-1") do
        @test EarthSciIO.resolve_s3_region() == "eu-west-1"
    end
end
