# CDS transport (spec/registries.md §1; esio-9nb.11): the submit → poll →
# download flow, exercised against a hermetic localhost mock of the CDS API v1
# (no external network — CI is fully offline). The live CDS pull is a MANUAL
# smoke test (needs a real key + a one-time ERA5 license acceptance on the CDS
# portal); see the EARTHSCI_LIVE block at the bottom.

using Sockets

# Run `f` with env vars temporarily set (nothing ⇒ unset); restored afterward.
function _with_env(f, kvs::Pair...)
    old = Dict(k => get(ENV, k, nothing) for (k, _) in kvs)
    try
        for (k, v) in kvs
            v === nothing ? delete!(ENV, k) : (ENV[k] = v)
        end
        return f()
    finally
        for (k, v) in old
            v === nothing ? delete!(ENV, k) : (ENV[k] = v)
        end
    end
end

function _read_cds_http_request(io)
    reqline = readline(io)
    parts = split(reqline)
    method = length(parts) >= 1 ? String(parts[1]) : ""
    path = length(parts) >= 2 ? String(parts[2]) : ""
    headers = Dict{String,String}()
    while true
        h = readline(io)
        isempty(h) && break
        kv = split(h, ":"; limit = 2)
        length(kv) == 2 && (headers[lowercase(strip(kv[1]))] = strip(kv[2]))
    end
    body = ""
    if haskey(headers, "content-length")
        n = parse(Int, headers["content-length"])
        body = String(read(io, n))
    end
    return method, path, headers, body
end

_respond_json(conn, json::AbstractString) = begin
    body = Vector{UInt8}(json)
    write(conn, "HTTP/1.1 200 OK\r\n", "Content-Type: application/json\r\n",
          "Content-Length: $(length(body))\r\nConnection: close\r\n\r\n")
    write(conn, body)
end

_respond_bytes(conn, payload::Vector{UInt8}) = begin
    write(conn, "HTTP/1.1 200 OK\r\n",
          "Content-Length: $(length(payload))\r\nConnection: close\r\n\r\n")
    write(conn, payload)
end

# Minimal CDS API v1 mock: POST .../execution ⇒ accepted job; GET .../jobs/<id>
# ⇒ "running" once then "successful" (exercises the poll loop); GET
# .../jobs/<id>/results ⇒ the asset href; GET /download/... ⇒ the payload
# bytes. Records every PRIVATE-TOKEN header seen and each request body.
function _start_cds_server(payload::Vector{UInt8})
    server = listen(Sockets.localhost, 0)
    port = Int(getsockname(server)[2])
    tokens = String[]
    bodies = String[]
    polls = Ref(0)
    @async begin
        try
            while true
                conn = accept(server)
                @async try
                    method, path, headers, body = _read_cds_http_request(conn)
                    push!(tokens, get(headers, "private-token", ""))
                    isempty(body) || push!(bodies, body)
                    host = "127.0.0.1:$port"
                    if method == "POST" && occursin("/execution", path)
                        _respond_json(conn, """{"jobID":"job-xyz","status":"accepted"}""")
                    elseif method == "GET" && endswith(path, "/results")
                        href = "http://$host/download/result.nc"
                        _respond_json(conn, """{"asset":{"value":{"href":"$href"}}}""")
                    elseif method == "GET" && occursin("/jobs/", path)
                        status = polls[] == 0 ? "running" : "successful"
                        polls[] += 1
                        _respond_json(conn, """{"status":"$status"}""")
                    elseif method == "GET" && occursin("/download/", path)
                        _respond_bytes(conn, payload)
                    else
                        write(conn, "HTTP/1.1 404 Not Found\r\nConnection: close\r\n\r\n")
                    end
                catch
                finally
                    close(conn)
                end
            end
        catch
            # listen socket closed ⇒ accept throws ⇒ task exits
        end
    end
    return server, port, tokens, bodies, polls
end

@testset "cds url codec + era5 request mapping (pure, no network)" begin
    req = era5_pressure_request(2020, 3;
        variables = ["temperature", "geopotential"],
        pressure_levels = [500, 1000], days = [1, 2], times = [0, 12],
        area = era5_area(north = 50.4, west = -10.9, south = 30.1, east = 5.2))
    @test req["variable"] == ["temperature", "geopotential"]   # explicit order preserved
    @test req["pressure_level"] == ["1000", "500"]             # always descending
    @test req["year"] == ["2020"]
    @test req["month"] == ["03"]
    @test req["day"] == ["01", "02"]
    @test req["time"] == ["00:00", "12:00"]
    @test req["data_format"] == "netcdf"
    @test req["area"] == [52, -12, 29, 7]                      # ±1° integer buffer

    # default day/time fill the month and the 24-hour clock
    full = era5_pressure_request(2021, 2)                       # 2021 not a leap year
    @test length(full["day"]) == 28
    @test full["day"][end] == "28"
    @test length(full["time"]) == 24
    @test full["time"][1] == "00:00" && full["time"][end] == "23:00"

    # the cds:// URL round-trips back to the dataset + request
    url = cds_url(ERA5_PL_DATASET, req)
    @test startswith(url, "cds://reanalysis-era5-pressure-levels?request=")
    ds, req2 = EarthSciIO.parse_cds_url(url)
    @test ds == ERA5_PL_DATASET
    @test req2["pressure_level"] == ["1000", "500"]
    @test req2["area"] == [52, -12, 29, 7]
    @test req2["variable"] == ["temperature", "geopotential"]

    # canonical: dict insertion order does NOT change the URL (shared-cache key
    # parity across processes/tracks)
    shuffled = Dict{String,Any}(reverse(collect(req)))
    @test cds_url(ERA5_PL_DATASET, shuffled) == url
    @test era5_pressure_url(2020, 3; variables = ["temperature", "geopotential"],
        pressure_levels = [1000, 500], days = [1, 2], times = [0, 12],
        area = era5_area(north = 50.4, west = -10.9, south = 30.1, east = 5.2)) == url

    # the cds transport is registered + active and dispatched by URL scheme
    @test haskey(TRANSPORT_REGISTRY, "cds")
    @test status_of(TRANSPORT_REGISTRY, "cds") == :active
    @test TRANSPORT_REGISTRY["cds"] isa CdsTransport
end

@testset "cds auth — ~/.cdsapirc parsing + env precedence" begin
    dir = mktempdir()
    rc = joinpath(dir, ".cdsapirc")
    write(rc, "url: https://example.test/api\nkey: abcd-1234\n")
    parsed = EarthSciIO._read_cdsapirc(rc)
    @test parsed["url"] == "https://example.test/api"
    @test parsed["key"] == "abcd-1234"

    _with_env("CDSAPI_KEY" => nothing, "CDSAPI_URL" => nothing) do
        @test EarthSciIO.cds_api_key(rc = parsed) == "abcd-1234"
        @test EarthSciIO.cds_api_endpoint(rc = parsed) == "https://example.test/api"
    end
    _with_env("CDSAPI_KEY" => "env-key", "CDSAPI_URL" => "http://env/api") do
        @test EarthSciIO.cds_api_key(rc = parsed) == "env-key"        # env wins over rc
        @test EarthSciIO.cds_api_endpoint(rc = parsed) == "http://env/api"
    end
    # absent everywhere ⇒ a clear setup error
    _with_env("CDSAPI_KEY" => nothing) do
        @test_throws ErrorException EarthSciIO.cds_api_key(rc = Dict{String,String}())
    end
end

@testset "cds transport — submit/poll/download (mocked, localhost)" begin
    payload = rand(UInt8, 1500)
    server, port, tokens, bodies, polls = _start_cds_server(payload)
    try
        _with_env("CDSAPI_URL" => "http://127.0.0.1:$port",
                  "CDSAPI_KEY" => "secret-cds-token",
                  "CDSAPI_POLL_SECONDS" => "0.01") do
            root = mktempdir()
            c = Cache(LocalStore(root); offline = false)
            url = era5_pressure_url(2018, 11; variables = ["temperature"],
                pressure_levels = [1000, 500], days = [8], times = [0, 1],
                area = era5_area(north = 41, west = -122, south = 39, east = -120))

            # submit → poll(running→successful) → download
            e1 = fetch_blob(c, url; source_loader = "era5", auth_realm = "cds")
            @test e1.status == :downloaded
            @test read(e1.path) == payload
            @test e1.manifest.bytes == length(payload)
            @test e1.manifest.source_loader == "era5"
            @test e1.manifest.auth_realm == "cds"        # realm recorded; key NEVER stored

            # the credential is NEVER persisted — not in the manifest, not on disk
            @test !occursin("secret-cds-token", e1.manifest.url)
            metafile = joinpath(root, "v1", "meta", cache_key(url) * ".json")
            @test isfile(metafile)
            @test !occursin("secret-cds-token", read(metafile, String))

            # the PRIVATE-TOKEN header reached the server; the poll loop ran
            @test "secret-cds-token" in tokens
            @test polls[] >= 2
            # the submitted body carried the request under "inputs"
            @test any(b -> occursin("\"inputs\"", b) && occursin("temperature", b), bodies)

            # skip-if-exists: a second fetch is a fast-path hit, no CDS round-trip
            ntok = length(tokens)
            @test fetch_blob(c, url).status == :hit
            @test length(tokens) == ntok          # server saw no new requests

            # offline re-read of the cds-fetched blob
            co = Cache(LocalStore(root); offline = true)
            @test read(fetch_blob(co, url).path) == payload
        end
    finally
        close(server)
    end
end

# Opt-in live smoke test (spec/offline-mode.md §4): NEVER in CI. Requires a real
# CDS key (CDSAPI_KEY or ~/.cdsapirc) AND a one-time ERA5 pressure-levels license
# acceptance on the CDS portal. Set EARTHSCI_LIVE=1 to actually hit the network.
if lowercase(get(ENV, "EARTHSCI_LIVE", "")) in ("1", "true", "yes")
    @testset "cds transport — live smoke (EARTHSCI_LIVE)" begin
        root = mktempdir()
        c = Cache(LocalStore(root); offline = false)
        # a deliberately tiny request: one variable, one level, one hour, one cell
        url = era5_pressure_url(2018, 1; variables = ["temperature"],
            pressure_levels = [500], days = [1], times = [0],
            area = era5_area(north = 41, west = -122, south = 40, east = -121))
        e = fetch_blob(c, url; source_loader = "era5", auth_realm = "cds")
        @test e.status in (:downloaded, :hit)
        @test filesize(e.path) > 0
        @test fetch_blob(Cache(LocalStore(root); offline = true), url).status == :hit
    end
end
