# The cadence Provider (esio-9nb.5) — the acceptance: "Provider over the shared
# fixture returns native arrays matching the Python track; CONST/DISCRETE
# correct; refresh_times() matches the cadence." Drives the FULL component
# (a)+(b) pipeline OFFLINE: cache (shared corpus) → format reader → native
# arrays, plus the cadence surface the solver consumes
# (materialize/refresh/refresh_times/prefetch). Reuses the corpus comparison
# helpers from test_readers.jl (included first by runtests.jl).

@testset "cadence Provider — materialize/refresh/refresh_times/prefetch (offline)" begin
    store = LocalStore(joinpath(CORPUS, "cache"))
    cache = Cache(store; offline = true, verify = true)
    era5 = "https://data.earthsci.dev/era5/2018/11/20181108.nc"
    openaq = "https://openaq-data-archive.s3.amazonaws.com/records/openaq/locationid=1/2018-11-08.csv"
    era5_case = JSON.parsefile(joinpath(CORPUS, "cases", "era5-grid-sub-tile.json"))

    @testset "CONST grid: empty cadence, native arrays match the oracle" begin
        p = const_provider(cache, era5; format = "netcdf", source_loader = "era5")
        @test is_const(p)
        @test refresh_times(p) == Float64[]          # CONST ⇒ never refreshes

        nds = materialize(p)
        @test Set(variable_names(nds)) == Set(["t2m", "sp"])
        @test Set(coord_names(nds)) == Set(["latitude", "longitude", "time"])

        # full native-array equality vs the Python track (checks 3–4 via Provider)
        for group in ("variables", "coords")
            for (name, spec) in era5_case["expected"][group]
                @test cmp_native_numeric(nds[name].data, spec["data"]) === nothing
            end
        end
        # the raw time axis is undecoded with its calendar carried for ESS
        @test eltype(nds["time"].data) == Int32
        @test nds["time"].attrs["calendar"] == "gregorian"
    end

    @testset "DISCRETE grid: refresh_times match cadence, per-tick slice" begin
        # cadence taken from the file's own (raw) time axis: [0.0, 1.0]
        full = materialize(const_provider(cache, era5; format = "netcdf"))
        times = Float64.(full["time"].data)
        p = discrete_provider(cache, era5, times; format = "netcdf", time_dim = "time")

        @test !is_const(p)
        @test refresh_times(p) == times              # matches the cadence

        s0 = refresh(p, 0.0)
        s1 = refresh(p, 1.0)
        @test s0["t2m"].dims == ["latitude", "longitude"]   # time record sliced out
        @test size(s0["t2m"].data) == (3, 3)
        @test s0["t2m"].data[1, 1] ≈ 282.5
        @test s1["t2m"].data[1, 1] ≈ 282.6           # a different record per tick
        @test isnan(s1["t2m"].data[3, 3])            # the masked cell survives the slice
        @test !haskey(s0, "time")                    # the sliced dim's coord is dropped
        @test refresh(p, 0.0)["sp"].data == s0["sp"].data   # refresh == materialize

        # a tick between grid points resolves to the active (last ≤ t) record
        @test materialize(p, 0.5)["t2m"].data == s0["t2m"].data
    end

    @testset "DISCRETE per-tick URLs (url-function form, no internal slice)" begin
        # url resolver form: the same fixture stands in for every tick; without
        # time_dim the provider returns the file's full native arrays per tick.
        p = discrete_provider(cache, _ -> era5, [0.0, 1.0]; format = "netcdf")
        @test refresh_times(p) == [0.0, 1.0]
        @test size(refresh(p, 1.0)["t2m"].data) == (2, 3, 3)
    end

    @testset "CSV points provider + variable selection" begin
        p = const_provider(cache, openaq; format = "csv", source_loader = "openaq",
                           reader_kwargs = (numeric_columns = ["latitude", "longitude", "value"],),
                           variables = ["value", "location_id"])
        nds = materialize(p)
        @test Set(variable_names(nds)) == Set(["value", "location_id"])   # restricted
        @test nds["value"].data == [152.3, 168.7, 98.1, 110.4]
        @test eltype(nds["value"].data) == Float64
        @test nds["location_id"].data == ["1", "1", "2", "2"]             # digit text stays string
    end

    @testset "prefetch warms the cache (offline hits, no decode)" begin
        p = const_provider(cache, era5; format = "netcdf")
        entries = prefetch(p)
        @test length(entries) == 1
        @test entries[1].status == :hit

        # DISCRETE per-tick URLs that collapse to one unique blob ⇒ one fetch
        pd = discrete_provider(cache, _ -> era5, [0.0, 1.0]; format = "netcdf")
        @test length(prefetch(pd)) == 1
    end

    @testset "construction + use guards" begin
        @test_throws ArgumentError const_provider(cache, era5; format = "netcdf", times = [1.0])
        @test_throws ArgumentError discrete_provider(cache, era5, Float64[]; format = "netcdf")
        @test_throws ArgumentError const_provider(cache, era5; format = "nonesuch")
        @test_throws ArgumentError Provider(cache, era5; format = "netcdf", time_dim = "time")  # CONST + time_dim
        # a DISCRETE provider needs an explicit time
        @test_throws ArgumentError materialize(discrete_provider(cache, era5, [0.0]; format = "netcdf"))
        # selecting a variable absent from the blob is a clear error
        bad = const_provider(cache, era5; format = "netcdf", variables = ["nope"])
        @test_throws ArgumentError materialize(bad)
    end
end
