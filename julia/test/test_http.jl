# HTTP transport (spec/registries.md §1): real download + conditional-GET
# revalidation, exercised against a hermetic localhost server (no external
# network). The offline conformance suite opens no socket; this online-path
# unit test deliberately does, against 127.0.0.1 only.

using Sockets

function _read_http_request(io)
    line = readline(io)
    headers = Dict{String,String}()
    while true
        h = readline(io)
        isempty(h) && break
        kv = split(h, ":"; limit = 2)
        length(kv) == 2 && (headers[lowercase(strip(kv[1]))] = strip(kv[2]))
    end
    return line, headers
end

# Minimal HTTP/1.1 server: 200 with body+ETag, or 304 when If-None-Match
# matches. One request per connection (Connection: close). Serves until the
# listen socket is closed.
function _start_test_server(payload::Vector{UInt8}, etag::AbstractString)
    server = listen(Sockets.localhost, 0)
    port = Int(getsockname(server)[2])
    task = @async begin
        try
            while true
                conn = accept(server)
                @async try
                    _, headers = _read_http_request(conn)
                    if occursin(etag, get(headers, "if-none-match", ""))
                        write(conn, "HTTP/1.1 304 Not Modified\r\n",
                              "ETag: \"$etag\"\r\nConnection: close\r\n\r\n")
                    else
                        write(conn, "HTTP/1.1 200 OK\r\n",
                              "Content-Length: $(length(payload))\r\n",
                              "ETag: \"$etag\"\r\nConnection: close\r\n\r\n")
                        write(conn, payload)
                    end
                catch
                finally
                    close(conn)
                end
            end
        catch
            # listen socket closed -> accept throws -> exit
        end
    end
    return server, port, task
end

@testset "http transport — download + conditional GET (localhost)" begin
    payload = rand(UInt8, 2048)
    etag = "abc123"
    server, port, _ = _start_test_server(payload, etag)
    try
        root = mktempdir()
        url = "http://127.0.0.1:$port/data.nc"
        c = Cache(LocalStore(root); offline = false, verify = true)

        # initial GET -> downloaded, ETag captured into the manifest
        e1 = fetch_blob(c, url; source_loader = "httploader")
        @test e1.status == :downloaded
        @test read(e1.path) == payload
        @test e1.manifest.etag !== nothing
        @test occursin(etag, e1.manifest.etag)
        @test e1.manifest.bytes == length(payload)

        # forced revalidation -> server sees If-None-Match -> 304 -> reuse blob
        e2 = fetch_blob(c, url; revalidate = true)
        @test e2.status == :not_modified
        @test read(e2.path) == payload

        # plain re-fetch -> fast-path hit
        @test fetch_blob(c, url).status == :hit

        # TTL: age the manifest, then a finite ttl makes the blob stale and
        # forces the same conditional GET -> 304 (spec §4 step 3).
        key = cache_key(url)
        m = EarthSciIO.get_meta(c.store, key)
        aged = EarthSciIO.Manifest(m.url, m.etag, m.last_modified, m.sha256_content,
                                   m.bytes, "2000-01-01T00:00:00Z", m.source_loader,
                                   m.auth_realm)
        EarthSciIO.put_meta!(c.store, key, aged)
        @test fetch_blob(c, url; ttl = 3600).status == :not_modified

        # offline re-read of the http-fetched blob
        co = Cache(LocalStore(root); offline = true, verify = true)
        @test read(fetch_blob(co, url).path) == payload
    finally
        close(server)
    end
end

# Opt-in live smoke test (spec/offline-mode.md §4): never in CI. Set
# EARTHSCI_LIVE=1 to actually hit the network and seed a fixture.
if lowercase(get(ENV, "EARTHSCI_LIVE", "")) in ("1", "true", "yes")
    @testset "http transport — live smoke (EARTHSCI_LIVE)" begin
        root = mktempdir()
        url = "https://raw.githubusercontent.com/EarthSciML/EarthSciIO/main/README.md"
        c = Cache(LocalStore(root); offline = false)
        e = fetch_blob(c, url)
        @test e.status in (:downloaded, :hit)
        @test filesize(e.path) > 0
        # now reuse it offline
        co = Cache(LocalStore(root); offline = true)
        @test fetch_blob(co, url).status == :hit
    end
end
