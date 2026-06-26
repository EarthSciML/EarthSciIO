# Concurrency (spec/cache-format.md §6): the per-blob advisory lock + atomic
# rename must guarantee that N racing fetchers of the same URL produce exactly
# ONE download and a single, intact blob — within one process (cooperative
# tasks) and, the real contract, across SEPARATE processes.

using Distributed

# Helper: assert the cache ended in a single, correct state after a race.
function _assert_single_intact(root, url, payload)
    key = cache_key(url)
    store = LocalStore(root)
    bp = EarthSciIO.get_blob(store, key)
    @test bp !== nothing
    @test read(bp) == payload
    # exactly one blob file under the fan-out dir
    blobdir = joinpath(root, "v1", "blobs", key[1:2])
    @test count(f -> startswith(f, key), readdir(blobdir)) == 1
    # manifest consistent with the blob
    m = EarthSciIO.get_meta(store, key)
    @test m !== nothing
    @test m.sha256_content == bytes2hex(sha256(payload))
    @test m.bytes == length(payload)
end

@testset "concurrency — intra-process race (cooperative tasks)" begin
    src = string(tempname(), ".nc")
    payload = rand(UInt8, 4096)
    write(src, payload)
    root = mktempdir()
    url = string("file://", src)

    n = 24
    statuses = Vector{Symbol}(undef, n)
    @sync for i in 1:n
        @async begin
            c = Cache(LocalStore(root); offline = false)
            statuses[i] = fetch_blob(c, url).status
        end
    end
    @test count(==(:downloaded), statuses) == 1     # lock + re-check => one fetch
    @test count(==(:hit), statuses) == n - 1
    _assert_single_intact(root, url, payload)
end

@testset "concurrency — multi-process race (mkpidlock across processes)" begin
    src = string(tempname(), ".nc")
    payload = rand(UInt8, 1_048_576)                # 1 MiB: widen the race window
    write(src, payload)
    root = mktempdir()
    url = string("file://", src)

    np = 4
    proj = dirname(Base.active_project())             # the instantiated test env
    pids = addprocs(np; exeflags = `--project=$proj`)
    try
        Distributed.@everywhere using EarthSciIO
        # Wall-clock barrier: every worker starts the fetch at the same instant
        # so they genuinely contend for the per-blob lock.
        start_at = time() + 2.0
        statuses = pmap(1:np) do _
            while time() < start_at
                sleep(0.005)
            end
            c = Cache(LocalStore(root); offline = false)
            string(fetch_blob(c, url).status)
        end
        @test count(==("downloaded"), statuses) == 1     # exactly one process fetched
        @test count(==("hit"), statuses) == np - 1
    finally
        rmprocs(pids)
    end
    _assert_single_intact(root, url, payload)
end
