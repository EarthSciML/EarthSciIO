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

# --- per-track option parity (spec/registries.json) --------------------------
#
# `spec/registries.json` is the MACHINE-READABLE contract an out-of-process
# caller reads, and it now says which decode options and metadata queries each
# TRACK provides. This asserts the Julia track really provides the ones it is
# listed under.
#
# The gap it closes: a decode option advertised in that file but implemented in
# one binding only reads back as a `MethodError` in the others, and nothing in
# the conformance corpus can catch it — a corpus case pins DECODED ARRAYS, and an
# option that does not exist produces no array to compare. The peers are
# `tests/test_registry_dispatch.py` and `rust/tests/registry_spec.rs`; all three
# read this same file, which is what makes the three tracks' claims about each
# other checkable.
@testset "registries.json — the options it declares for Julia are Julia's" begin
    spec = JSON.parsefile(joinpath(@__DIR__, "..", "..", "spec", "registries.json"))
    entries = spec["registries"]["format"]["entries"]
    checked = String[]
    for entry in entries
        haskey(entry, "reader_options") || continue
        push!(checked, entry["name"])
        for opt in entry["reader_options"]
            @test issubset(Set(opt["tracks"]), Set(["python", "julia", "rust"]))
        end
        want = Set(Symbol(o["name"]) for o in entry["reader_options"]
                   if "julia" in o["tracks"])
        have = Set(reader_option_keys(FORMAT_REGISTRY[entry["name"]]))
        @test want == have
    end
    # ...and the file really does declare them for the reader this PR is about,
    # so a `reader_options` block silently dropped from it cannot pass this.
    @test "netcdf" in checked

    # A declared metadata query must be ANSWERED here, not inherited from the
    # "I cannot answer that" fallback every reader gets for free.
    generic = which(EarthSciIO.dim_length, Tuple{Nothing,String,String})
    for entry in entries
        for q in get(entry, "metadata_queries", Any[])
            "julia" in q["tracks"] || continue
            @test q["name"] == "dim_length"     # the only one this shape describes
            reader = FORMAT_REGISTRY[entry["name"]]
            m = which(EarthSciIO.dim_length, Tuple{typeof(reader),String,String})
            @test m !== generic
        end
    end

    # The specific claim PR #4 could not make honestly: `records` is real in all
    # three tracks, while which Providers USE it stays a separate, per-track
    # performance decision (the Julia Provider does; Python and Rust keep a
    # decoded-file buffer that a per-tick narrowed decode would throw away).
    netcdf = only(e for e in entries if e["name"] == "netcdf")
    records = only(o for o in netcdf["reader_options"] if o["name"] == "records")
    @test Set(records["tracks"]) == Set(["python", "julia", "rust"])
    @test records["used_by_provider"] ==
          Dict("julia" => true, "python" => false, "rust" => false)
    # The `len -> indices` callable is Julia's alone, and the file says so.
    @test collect(keys(records["extensions"])) == ["julia"]
end

# --- per-track selection forms (spec/registries.json `selection.forms`) ------
#
# `registries.json` now records, per selection-capable format, which `select`
# FORMS (`axes`, `dims`) each track's reader takes (registries.md §2.2, "Two
# spellings, one vocabulary"). A form this track is listed under must be one the
# REAL reader honours: every committed corpus case of that format pinning a select
# in that form is decoded here by the registered reader with exactly that select,
# and must come back at the window's shape and dims. Before the `dims` form
# existed, a `{"dims": ...}` select was silently read as no select at all and
# returned the WHOLE array — which a shape check catches and a mere "it did not
# throw" would not. The capability pair the entry states is checked too.
@testset "registries.json — the selection forms it claims for Julia are Julia's" begin
    spec = JSON.parsefile(joinpath(@__DIR__, "..", "..", "spec", "registries.json"))
    corpus = normpath(joinpath(@__DIR__, "..", "..", "conformance", "corpus"))
    cases = [JSON.parsefile(joinpath(corpus, e["file"]))
             for e in JSON.parsefile(joinpath(corpus, "cases.json"))["cases"]]
    cache = Cache(LocalStore(joinpath(corpus, "cache")); offline = true, verify = true)
    claimed = String[]
    for entry in spec["registries"]["format"]["entries"]
        name = String(entry["name"])
        haskey(FORMAT_REGISTRY, name) || continue
        reader = FORMAT_REGISTRY[name]
        sel = get(entry, "selection", nothing)
        if sel === nothing
            # no `selection` block: the reader must not claim a capability the
            # machine-readable contract does not record
            @test !supports_selection(reader)
            continue
        end
        @test supports_selection(reader) == sel["supports_selection"]
        @test store_backed(reader) == sel["store_backed"]
        for f in sel["forms"]
            @test issubset(Set(f["tracks"]), Set(["python", "julia", "rust"]))
            "julia" in f["tracks"] || continue
            form = String(f["form"])
            push!(claimed, "$name:$form")
            pinned = [c for c in cases
                      if c["format"] == name && get(c, "select", nothing) isa AbstractDict &&
                         haskey(c["select"], form)]
            @test !isempty(pinned)               # a claim the corpus actually exercises
            for c in pinned
                nds = store_backed(reader) ?
                    read_store(reader, cache, c["resolved_url"];
                               variables = String.(c["variables"]), select = c["select"]) :
                    read_native(reader, joinpath(corpus, c["blob_path"]); select = c["select"])
                for (v, want) in c["expected"]["variables"]
                    @test nds[v].dims == want["dims"]
                    @test collect(size(nds[v].data)) == want["shape"]
                end
            end
        end
    end
    @test Set(claimed) == Set(["netcdf:axes", "netcdf:dims", "zarr:axes", "zarr:dims"])
end
