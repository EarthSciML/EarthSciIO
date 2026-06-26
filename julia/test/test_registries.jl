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
    @test status_of(TRANSPORT_REGISTRY, "s3") == :stub
    @test EarthSciIO.schemes(TRANSPORT_REGISTRY["http"]) == ["http", "https"]
    # the s3 transport is a registered stub: present, but errors if used
    @test_throws ErrorException EarthSciIO.fetch!(S3Transport(), "s3://b/k", tempname())
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

@testset "format registry — zarr stub now; readers are esio-9nb.5" begin
    @test haskey(FORMAT_REGISTRY, "zarr")
    @test status_of(FORMAT_REGISTRY, "zarr") == :stub
    @test_throws ErrorException EarthSciIO.read_native(FORMAT_REGISTRY["zarr"])
end
