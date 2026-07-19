# The three extensibility registries (spec/registries.md): dispatch by name,
# active vs stub status, and the "unknown name is a registration gap" contract.

@testset "generic Registry: register / lookup / unknown-name error" begin
    r = Registry{Transport}("demo")
    t = HttpTransport()
    register!(r, "x", t)
    register!(r, ("y", "z"), FileTransport(); status = :stub)
    @test r["x"] === t
    @test haskey(r, "y") && haskey(r, "z")
    @test status_of(r, "x") == :active
    @test status_of(r, "y") == :stub
    @test registered_names(r) == ["x", "y", "z"]
    @test get(r, "missing", nothing) === nothing
    @test_throws ArgumentError r["missing"]            # gap, not a Provider edit
end

@testset "transport registry — keyed by URL scheme" begin
    @test TRANSPORT_REGISTRY["http"] isa HttpTransport
    @test TRANSPORT_REGISTRY["https"] isa HttpTransport
    @test TRANSPORT_REGISTRY["file"] isa FileTransport
    @test TRANSPORT_REGISTRY["s3"] isa S3Transport
    @test status_of(TRANSPORT_REGISTRY, "http") == :active
    @test status_of(TRANSPORT_REGISTRY, "file") == :active
    # the s3 transport is now ACTIVE: an anonymous s3:// -> regional-HTTPS rewriter
    # over the http transport (the rewrite is pure + testable without a socket).
    @test status_of(TRANSPORT_REGISTRY, "s3") == :active
    @test EarthSciIO.schemes(TRANSPORT_REGISTRY["http"]) == ["http", "https"]
    @test EarthSciIO.s3_https_url("s3://bucket/era5/2018/20181108.nc") ==
          "https://bucket.s3.us-east-2.amazonaws.com/era5/2018/20181108.nc"
    @test_throws ErrorException EarthSciIO.s3_https_url("https://not-s3/x")
end

@testset "store registry — keyed by store name, value is a factory" begin
    s = make_store("local"; root = "/tmp/whatever")
    @test s isa LocalStore
    @test s.root == "/tmp/whatever"
    @test EarthSciIO.store_name(s) == "local"
    @test status_of(STORE_REGISTRY, "local") == :active
    @test make_store("s3") isa S3Store
    @test status_of(STORE_REGISTRY, "s3") == :stub
    @test_throws ErrorException EarthSciIO.get_blob(S3Store(), "deadbeef")
end

@testset "format registry — zarr active + store-backed" begin
    @test haskey(FORMAT_REGISTRY, "zarr")
    @test status_of(FORMAT_REGISTRY, "zarr") == :active
    @test FORMAT_REGISTRY["zarr"] isa ZarrReader
    @test store_backed(FORMAT_REGISTRY["zarr"])          # handed (cache, base_url)
    @test !store_backed(FORMAT_REGISTRY["netcdf"])       # whole-file readers untouched
end
