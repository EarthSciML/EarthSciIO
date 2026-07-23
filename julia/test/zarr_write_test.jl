# The `zarr` WRITER round-trip (Wave 1): write a small gridded dataset with
# `ZarrWriter` (sharded Zarr v3) across several `write_record!` calls, then read
# it back with the upgraded `ZarrReader`. Conformance is TOLERANCE-BASED on the
# DECODED arrays (RFC §16) + metadata equality — NOT byte-identity (Blosc
# container bytes are host/version dependent).
#
# The read side goes through the REAL reader path: a non-offline `Cache` with the
# `file://` transport fetches each `zarr.json` / shard object the writer emitted,
# so this exercises v3 detection, the sharding-codec decode, and array resize.

import Blosc   # activates EarthSciIOBloscExt for BOTH the encode and the decode

const WTOL = 1e-10

# Build the OutputSchema for a var `temp(time, lat, lon)` over lon=4, lat=3, with
# a static `lon` coordinate and a growing `time` axis. Inner chunks are small so
# a shard packs several; the time-shard holds 2 records so 5 records => 2 full
# shards + a partial trailing shard (the close-flush path).
function _demo_schema()
    return OutputSchema(;
        dims = ["time" => 0, "lat" => 3, "lon" => 4],
        time_dim = "time",
        coords = [
            "lon" => (Float64[10.0, 20.0, 30.0, 40.0], Dict{String,Any}("units" => "degrees_east")),
            "lat" => (Float64[1.0, 2.0, 3.0], Dict{String,Any}("units" => "degrees_north")),
            "time" => (Float64[], Dict{String,Any}("units" => "seconds")),   # attrs only
        ],
        vars = ["temp" => OutputVar(["time", "lat", "lon"], Float64)],
        chunk_shape = Dict("time" => 1, "lat" => 2, "lon" => 2),
        shard_shape = Dict("time" => 2, "lat" => 2, "lon" => 4),
        profile = :diagnostic,
        attrs = Dict{String,Any}("title" => "writer round-trip"),
    )
end

# The value at (time k, lat i, lon j), chosen so any decoded cell is self-checking.
_cell(k, i, j) = 1000.0 * k + 10.0 * i + j

@testset "zarr writer: sharded v3 round-trip (write -> read decoded values)" begin
    dir = mktempdir()
    store_dir = joinpath(dir, "out.zarr")
    base_url = "file://" * store_dir

    schema = _demo_schema()
    w = ZarrWriter()
    h = write_open!(w, nothing, base_url, schema)

    nrec = 5
    slices = Vector{Matrix{Float64}}()
    for k in 1:nrec
        a = Float64[_cell(k, i, j) for i in 1:3, j in 1:4]   # (lat, lon)
        push!(slices, a)
        write_record!(w, h, Float64(k) * 100.0, Dict("temp" => a))
    end
    man = write_close!(w, h)

    # the writer produced a real directory tree
    @test isfile(joinpath(store_dir, "zarr.json"))
    @test isfile(joinpath(store_dir, "temp", "zarr.json"))
    @test isfile(joinpath(store_dir, "output_manifest.json"))

    # --- read back through the real reader (file:// transport, non-offline) ---
    cache = Cache(LocalStore(joinpath(dir, "cache")); offline = false)

    nds = read_store(ZarrReader(), cache, base_url;
                     variables = ["temp", "lon", "lat", "time"])

    temp = nds.variables["temp"]
    @test temp.dims == ["time", "lat", "lon"]
    @test size(temp.data) == (nrec, 3, 4)
    @test eltype(temp.data) == Float64
    for k in 1:nrec, i in 1:3, j in 1:4
        @test isapprox(temp.data[k, i, j], _cell(k, i, j); atol = WTOL)
    end

    # coordinates decoded correctly
    @test isapprox(nds.variables["lon"].data, Float64[10, 20, 30, 40]; atol = WTOL)
    @test isapprox(nds.variables["lat"].data, Float64[1, 2, 3]; atol = WTOL)
    @test nds.variables["lon"].dims == ["lon"]
    @test isapprox(nds.variables["time"].data, Float64[100, 200, 300, 400, 500]; atol = WTOL)

    # --- output manifest fingerprint ---
    @test man.format == "zarr"
    @test man.zarr_format == 3
    @test man.profile == "diagnostic"
    @test man.n_records == nrec
    @test man.last_t == 500.0
    @test man.time_dim == "time"
    @test length(man.time_shards) == 3          # 2 full (2+2) + 1 partial (1)
    @test man.time_shards[end].n == 1
    @test man.codec["cname"] == "zstd"

    rd = read_output_manifest(joinpath(store_dir, "output_manifest.json"))
    @test rd !== nothing
    @test rd.n_records == nrec
    @test rd.last_t == 500.0
    @test first(v["name"] for v in rd.vars) == "temp"

    # --- array_shape probe reads only zarr.json (v3), reports the grown shape ---
    @test array_shape(ZarrReader(), cache, base_url, "temp") == (nrec, 3, 4)
end

@testset "zarr writer: orthogonal select reads back a sub-region (v3 sharding)" begin
    dir = mktempdir()
    store_dir = joinpath(dir, "sel.zarr")
    base_url = "file://" * store_dir

    schema = _demo_schema()
    w = ZarrWriter()
    h = write_open!(w, nothing, base_url, schema)
    for k in 1:4
        a = Float64[_cell(k, i, j) for i in 1:3, j in 1:4]
        write_record!(w, h, Float64(k), Dict("temp" => a))
    end
    write_close!(w, h)

    cache = Cache(LocalStore(joinpath(dir, "cache")); offline = false)
    # time index 2 (0-based), lats {0,2}, lons {1,3}
    sel = Dict("axes" => Any[Dict("indices" => [2]),
                             Dict("indices" => [0, 2]),
                             Dict("indices" => [1, 3])])
    nds = read_store(ZarrReader(), cache, base_url; variables = ["temp"], select = sel)
    t = nds.variables["temp"]
    @test size(t.data) == (1, 2, 2)
    # 0-based sel -> 1-based cell(k=3, i∈{1,3}, j∈{2,4})
    @test isapprox(t.data[1, 1, 1], _cell(3, 1, 2); atol = WTOL)
    @test isapprox(t.data[1, 1, 2], _cell(3, 1, 4); atol = WTOL)
    @test isapprox(t.data[1, 2, 1], _cell(3, 3, 2); atol = WTOL)
    @test isapprox(t.data[1, 2, 2], _cell(3, 3, 4); atol = WTOL)
end

@testset "zarr writer: checkpoint profile is lossless" begin
    dir = mktempdir()
    base_url = "file://" * joinpath(dir, "ckpt.zarr")
    schema = OutputSchema(;
        dims = ["time" => 0, "cell" => 6],
        time_dim = "time",
        coords = Pair{String,Tuple{Vector,Dict{String,Any}}}[],
        vars = ["q" => OutputVar(["time", "cell"], Float64)],
        chunk_shape = Dict("time" => 2, "cell" => 3),
        shard_shape = Dict("time" => 2, "cell" => 6),
        profile = :checkpoint)
    w = ZarrWriter()
    h = write_open!(w, nothing, base_url, schema)
    vals = [Float64[k + c / 7 for c in 1:6] for k in 1:3]
    for k in 1:3
        write_record!(w, h, Float64(k), Dict("q" => vals[k]))
    end
    m = write_close!(w, h)
    @test m.profile == "checkpoint"

    cache = Cache(LocalStore(joinpath(dir, "cache")); offline = false)
    nds = read_store(ZarrReader(), cache, base_url; variables = ["q"])
    q = nds.variables["q"]
    @test size(q.data) == (3, 6)
    for k in 1:3, c in 1:6
        @test q.data[k, c] == vals[k][c]        # lossless: exact equality
    end
end
