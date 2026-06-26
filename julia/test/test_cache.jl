# Cache core: key derivation, $EARTHSCIDATADIR, offline mode, the fetch +
# cache + offline re-read cycle, manifest + integrity. (spec/cache-format.md,
# spec/offline-mode.md)

@testset "cache_key — spec §1 worked examples + byte-range" begin
    @test cache_key("https://data.earthsci.dev/era5/2018/11/20181108.nc") ==
          "11cdcec111409f586e6afc432e1a6da47e6f97ccf3715e5db8554632b00671c1"
    @test cache_key(
        "https://openaq-data-archive.s3.amazonaws.com/records/openaq/locationid=1/2018-11-08.csv") ==
          "69dd26b950e43cb2182e3b4d02e89e09bfb798b13469183ca2dad15c5794379a"
    # a byte-slice is its own cache entry (#bytes=a-b appended before hashing)
    @test cache_key("http://x/a.nc", (0, 99)) != cache_key("http://x/a.nc")
    @test cache_key("http://x/a.nc", (0, 99)) ==
          cache_key("http://x/a.nc#bytes=0-99")
end

@testset "datadir / EARTHSCIDATADIR (spec §5: env wins, default on /scratch.local)" begin
    withenv("EARTHSCIDATADIR" => "/scratch.local/me/cache") do
        @test datadir() == "/scratch.local/me/cache"
    end
    withenv("EARTHSCIDATADIR" => nothing, "USER" => "tester") do
        d = datadir()
        @test startswith(d, "/scratch.local/")      # NEVER /u — hard rule (R6)
        @test occursin("tester", d)
    end
end

@testset "offline flag — EARTHSCI_OFFLINE vs explicit (offline-mode §1)" begin
    st = LocalStore(mktempdir())
    for truthy in ("1", "true", "yes", "YES", "True")
        withenv("EARTHSCI_OFFLINE" => truthy) do
            @test is_offline(Cache(st))
        end
    end
    withenv("EARTHSCI_OFFLINE" => "0") do
        @test !is_offline(Cache(st))
    end
    withenv("EARTHSCI_OFFLINE" => nothing) do
        @test !is_offline(Cache(st))
        @test is_offline(Cache(st; offline = true))       # explicit arg wins
    end
    withenv("EARTHSCI_OFFLINE" => "1") do
        @test !is_offline(Cache(st; offline = false))     # explicit arg wins
    end
end

@testset "fetch + cache + offline re-read (file:// transport)" begin
    src = string(tempname(), ".nc")
    payload = rand(UInt8, 512)
    write(src, payload)
    root = mktempdir()
    url = string("file://", src)
    key = cache_key(url)

    c = Cache(LocalStore(root); offline = false, verify = true)
    e1 = fetch_blob(c, url; source_loader = "testloader")
    @test e1.status == :downloaded
    @test e1.key == key
    @test read(e1.path) == payload

    # on-disk layout (spec §2): blobs/<key[:2]>/<key>.<ext>, meta/<key>.json
    @test basename(dirname(e1.path)) == key[1:2]
    @test endswith(e1.path, string(key, ".nc"))
    @test isfile(joinpath(root, "v1", "meta", string(key, ".json")))

    # manifest field mapping (spec §3)
    m = e1.manifest
    @test m !== nothing
    @test m.url == url
    @test m.bytes == length(payload)
    @test m.sha256_content == bytes2hex(sha256(payload))
    @test !isempty(m.fetched_at)
    @test m.source_loader == "testloader"
    @test m.auth_realm === nothing

    # second online fetch -> cache hit, same blob (no re-download)
    e2 = fetch_blob(c, url)
    @test e2.status == :hit
    @test e2.path == e1.path

    # offline re-read -> hit, identical bytes, no transport constructed
    co = Cache(LocalStore(root); offline = true, verify = true)
    e3 = fetch_blob(co, url)
    @test e3.status == :hit
    @test read(e3.path) == payload
end

@testset "offline miss raises CacheMiss carrying url + key (offline-mode §2)" begin
    co = Cache(LocalStore(mktempdir()); offline = true)
    url = "https://data.earthsci.dev/era5/2099/01/20990101.nc"
    @test_throws CacheMiss fetch_blob(co, url)
    err = nothing
    try
        fetch_blob(co, url)
    catch e
        err = e
    end
    @test err isa CacheMiss
    @test err.url == url
    @test err.key == cache_key(url)
end

@testset "integrity: a tampered cached blob raises IntegrityError (verify=true)" begin
    src = string(tempname(), ".bin")
    write(src, b"the original bytes")
    root = mktempdir()
    url = string("file://", src)

    c = Cache(LocalStore(root); offline = false)
    e = fetch_blob(c, url)
    write(e.path, b"corrupted in place")            # silent on-disk corruption

    cv = Cache(LocalStore(root); offline = true, verify = true)
    @test_throws IntegrityError fetch_blob(cv, url)
    # without verify, presence alone is the offline check (spec §4)
    cn = Cache(LocalStore(root); offline = true, verify = false)
    @test fetch_blob(cn, url).status == :hit
end

@testset "Julia-written manifest matches manifest.schema.json shape (cross-lang reuse)" begin
    src = string(tempname(), ".nc")
    write(src, rand(UInt8, 64))
    root = mktempdir()
    url = string("file://", src)
    fetch_blob(Cache(LocalStore(root); offline = false), url;
               source_loader = "era5", auth_realm = "cds")
    raw = JSON.parsefile(joinpath(root, "v1", "meta", string(cache_key(url), ".json")))
    # exactly the schema's 9 properties (additionalProperties: false)
    @test Set(keys(raw)) == Set(["schema", "url", "etag", "last_modified",
                                 "sha256_content", "bytes", "fetched_at",
                                 "source_loader", "auth_realm"])
    @test raw["schema"] == "earthsciio/manifest/v1"
    @test occursin(r"^[0-9a-f]{64}$", raw["sha256_content"])
    @test raw["bytes"] == 64
    @test occursin(r"^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z$", raw["fetched_at"])  # RFC3339 UTC
    @test raw["auth_realm"] == "cds"       # realm recorded; the credential is NEVER stored
    @test raw["etag"] === nothing          # file transport carries no ETag
end

@testset "file:// transport expands \${EARTHSCIDATADIR} mirror templates (nei2016 pattern)" begin
    mirror = mktempdir()
    fpath = joinpath(mirror, "nei2016", "egu.nc")
    mkpath(dirname(fpath))
    write(fpath, b"mirror-bytes")
    root = mktempdir()
    withenv("EARTHSCIDATADIR" => mirror) do
        url = "file://\${EARTHSCIDATADIR}/nei2016/egu.nc"
        e = fetch_blob(Cache(LocalStore(root); offline = false), url)
        @test e.status == :downloaded
        @test read(e.path) == b"mirror-bytes"
    end
end

@testset "TTL: an old fetched_at is computed as stale (spec §4 step 3)" begin
    @test EarthSciIO._age_seconds("2000-01-01T00:00:00Z") > 1.0e6   # decades of seconds
    @test EarthSciIO._age_seconds("not-a-timestamp") === nothing
end

@testset "Cache(; store=\"local\", root=...) resolves through the store registry" begin
    root = mktempdir()
    c = Cache(; store = "local", root = root, offline = true)
    @test is_offline(c)
    @test c.store isa LocalStore
    @test c.store.root == root
end
