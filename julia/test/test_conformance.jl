# Cross-language cache REUSE (spec/conformance.md, offline-mode §5).
#
# The committed corpus under conformance/corpus/cache is a pre-populated
# $EARTHSCIDATADIR written by the Python track's generate.py. This test points
# the Julia store at it (offline) and proves a blob cached by another language
# is found and validated byte-for-byte via the shared sha256(resolved_url) key
# — conformance checks 1 (cache-key agreement), 2 (manifest integrity), and 5
# (offline-only). Decode + native-array equality (checks 3–4) are the format
# reader's job (component b / esio-9nb.5).

const CORPUS = normpath(joinpath(@__DIR__, "..", "..", "conformance", "corpus"))

@testset "cross-language cache reuse — corpus, offline" begin
    @test isdir(joinpath(CORPUS, "cache", "v1", "blobs"))

    index = JSON.parsefile(joinpath(CORPUS, "cases.json"))
    @test index["cache_format_version"] == "v1"
    cases = index["cases"]
    @test length(cases) >= 1

    store = LocalStore(joinpath(CORPUS, "cache"))     # cache root = corpus cache dir
    cache = Cache(store; offline = true, verify = true)

    for entry in cases
        case = JSON.parsefile(joinpath(CORPUS, entry["file"]))
        id = case["id"]
        url = case["resolved_url"]
        key = case["cache_key"]

        @testset "$id" begin
            # 1. cache-key agreement — Julia hashes the URL to the same key
            @test cache_key(url) == key
            @test key == entry["cache_key"]

            # 2. the blob the Python track cached is found by the Julia store…
            bp = EarthSciIO.get_blob(store, key)
            @test bp !== nothing
            blob = read(bp)
            #    …and is byte-identical (manifest + case integrity)
            @test bytes2hex(sha256(blob)) == case["content_sha256"]
            @test length(blob) == case["bytes"]

            m = EarthSciIO.get_meta(store, key)
            @test m !== nothing
            @test m.sha256_content == case["content_sha256"]
            @test m.bytes == case["bytes"]
            @test m.url == url

            # 5. offline resolution returns that same blob — no socket, no fetch
            e = fetch_blob(cache, url)
            @test e.status == :hit
            @test e.path == bp
            @test read(e.path) == blob
        end
    end
end

@testset "corpus manifests validate against the manifest schema's required fields" begin
    metadir = joinpath(CORPUS, "cache", "v1", "meta")
    for f in readdir(metadir)
        endswith(f, ".json") || continue
        d = JSON.parsefile(joinpath(metadir, f))
        # required by schemas/manifest.schema.json
        for req in ("url", "sha256_content", "bytes", "fetched_at")
            @test haskey(d, req)
        end
        @test occursin(r"^[0-9a-f]{64}$", d["sha256_content"])
        # the file name is <key>.json and sha256(url) must equal that key
        key = first(splitext(f))
        @test cache_key(d["url"]) == key
    end
end
