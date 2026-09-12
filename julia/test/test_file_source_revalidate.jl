# A `file://` entry is rechecked against its source before it is served
# (spec/cache-format.md §4.1 rung 0) — the regression suite for
# EarthSciML/EarthSciAST#293, and the Julia half of the same suite the Rust track
# carries in rust/tests/file_source_revalidate.rs and the Python track in
# tests/test_file_source_revalidate.py.
#
# The bug's whole signature is that it passes GREEN, so every test here
# reproduces the actual failure rather than the happy path: warm the cache from a
# local file, replace that file IN PLACE at the same path (so the resolved URL,
# and therefore the cache key, is unchanged), read again, and demand the new
# bytes. The report's corpus was 2.8 GB replaced at the same paths; the entry it
# caught held 371 bytes while the file on disk was 363.
#
# Two of these are the ones a plausible half-fix fails:
#
#   * a replacement of the SAME byte length (a float re-encode, a corrected
#     value) — a size-only check waves it through;
#   * an UNCHANGED file, which must still be served from the warm entry and must
#     NOT be re-ingested. A "fix" that simply stops caching passes every other
#     test here; that one counts the fetches reaching the transport to prove it
#     did not.

# --- a `file` transport that counts the fetches reaching it ------------------
# A cache hit never gets here, so the counter is the difference between "served
# warm" and "re-ingested".
mutable struct CountingFile <: EarthSciIO.Transport
    n::Int
end
EarthSciIO.schemes(::CountingFile) = ["file"]
function EarthSciIO.fetch!(t::CountingFile, url::AbstractString, dest::AbstractString;
                           kwargs...)
    t.n += 1
    return EarthSciIO.fetch!(EarthSciIO.FileTransport(), url, dest; kwargs...)
end

# Swap the `file` transport for a counting one, restoring the real one after.
function with_counting_file(f)
    orig = EarthSciIO.TRANSPORT_REGISTRY["file"]
    counter = CountingFile(0)
    register!(EarthSciIO.TRANSPORT_REGISTRY, "file", counter)
    try
        return f(counter)
    finally
        register!(EarthSciIO.TRANSPORT_REGISTRY, "file", orig)
    end
end

# --- 1. replaced in place, different length ---------------------------------

@testset "rung 0: a source replaced in place is re-ingested (#293)" begin
    # The report's own shape: same path, new contents, a different byte length.
    # Before the fix this returned the OLD corpus for the life of the cache dir.
    src = string(tempname(), ".csv")
    url = string("file://", src)
    root = mktempdir()

    with_counting_file() do counter
        c = Cache(LocalStore(root); offline = false)
        old = repeat(UInt8['o'], 371)          # the report's stale entry
        write(src, old)
        @test read(fetch_blob(c, url).path) == old
        @test counter.n == 1

        # Replace the corpus in place — same path, so the same resolved URL and
        # the same cache key.
        new = repeat(UInt8['n'], 363)          # the report's file-on-disk
        write(src, new)

        e = fetch_blob(c, url)
        @test read(e.path) == new
        @test e.status == :downloaded
        @test counter.n == 2
        # The manifest is the record the report found lying: it must now
        # describe the file that is actually there.
        @test e.manifest.bytes == 363
        @test e.manifest.sha256_content == bytes2hex(sha256(new))
    end
end

# --- 2. replaced in place, SAME length ---------------------------------------

@testset "rung 0: an equal-length replacement is still detected" begin
    # The sibling case a size-only check misses. The report's corpus happened to
    # change length; a float re-encode or a corrected value easily would not.
    src = string(tempname(), ".csv")
    url = string("file://", src)
    root = mktempdir()
    old = Vector{UInt8}("year,pm25\n2016,12.500\n")
    new = Vector{UInt8}("year,pm25\n2016,12.499\n")
    @test length(old) == length(new) && old != new   # the point of this test

    with_counting_file() do counter
        c = Cache(LocalStore(root); offline = false)
        write(src, old)
        @test read(fetch_blob(c, url).path) == old
        write(src, new)
        @test read(fetch_blob(c, url).path) == new
        @test counter.n == 2
    end
end

# --- 3. an unchanged file is still CACHED ------------------------------------

@testset "rung 0: an unchanged source is served from the warm entry" begin
    # The proof that this is still a cache. A fix that "revalidated" by always
    # re-copying the file would pass every other test here and quietly turn the
    # cache into a no-op.
    src = string(tempname(), ".csv")
    url = string("file://", src)
    root = mktempdir()
    body = Vector{UInt8}("year,value\n2016,1.0\n")
    write(src, body)

    with_counting_file() do counter
        c = Cache(LocalStore(root); offline = false)
        first = fetch_blob(c, url)
        @test first.status == :downloaded
        @test counter.n == 1

        for _ in 1:3
            again = fetch_blob(c, url)
            @test again.status == :hit
            @test again.path == first.path
            @test read(again.path) == body
            @test again.manifest.fetched_at == first.manifest.fetched_at
        end

        # A fresh Cache over the same root (a new process — the case the
        # report's "warmed at different times" checkouts hit) is still a hit.
        @test fetch_blob(Cache(LocalStore(root); offline = false), url).status == :hit
        @test counter.n == 1          # an unchanged file must NOT be re-ingested
    end
end

# --- 4. a deleted source is an ERROR -----------------------------------------

@testset "rung 0: a deleted source behind a warm entry is an error" begin
    # "A worktree kept passing after its data path was pointed at a directory
    # that does not exist, because nothing was read at all." Absence must be loud.
    src = string(tempname(), ".csv")
    url = string("file://", src)
    root = mktempdir()
    write(src, b"present")

    c = Cache(LocalStore(root); offline = false)
    @test read(fetch_blob(c, url).path) == b"present"
    rm(src)
    @test_throws ErrorException fetch_blob(c, url)
end

@testset "rung 0: a deleted source DIRECTORY behind a warm entry is an error" begin
    dir = mktempdir()
    corpus = joinpath(dir, "characterization")
    mkpath(corpus)
    src = joinpath(corpus, "rates.csv")
    url = string("file://", src)
    root = mktempdir()
    write(src, b"rates")

    c = Cache(LocalStore(root); offline = false)
    @test read(fetch_blob(c, url).path) == b"rates"
    rm(corpus; recursive = true)
    @test_throws ErrorException fetch_blob(c, url)
end

# --- 5. remote entries are untouched -----------------------------------------

# A made-up REMOTE scheme: bytes from memory, no local file anywhere.
mutable struct MemTransport <: EarthSciIO.Transport
    body::Vector{UInt8}
    n::Int
end
EarthSciIO.schemes(::MemTransport) = ["mem"]
function EarthSciIO.fetch!(t::MemTransport, url::AbstractString, dest::AbstractString;
                           kwargs...)
    t.n += 1
    write(dest, t.body)
    return EarthSciIO.FetchResult(:downloaded, nothing, nothing, filesize(dest))
end

@testset "rung 0 is a file:// rung — a remote entry is not rechecked" begin
    # A remote source has no local truth to consult, and its warm entry must keep
    # being served without a re-fetch: "immutable by default" is what makes an
    # S3-backed store usable at all.
    mem = MemTransport(Vector{UInt8}("remote-bytes"), 0)
    register!(EarthSciIO.TRANSPORT_REGISTRY, "mem", mem)
    c = Cache(LocalStore(mktempdir()); offline = false)
    url = "mem://store/chunk/0.0.0"

    @test read(fetch_blob(c, url).path) == mem.body
    for _ in 1:3
        @test fetch_blob(c, url).status == :hit
    end
    @test mem.n == 1
end

# --- 6. the documented opt-out -----------------------------------------------

@testset "rung 0: the opt-out restores the stale serve" begin
    # `revalidate_file=false` (env: EARTHSCI_REVALIDATE_FILE=0) is the escape
    # hatch for a corpus known to be immutable. It restores the OLD behaviour
    # exactly — which is why it is off the default path.
    src = string(tempname(), ".csv")
    url = string("file://", src)
    root = mktempdir()

    with_counting_file() do counter
        c = Cache(LocalStore(root); offline = false, revalidate_file = false)
        write(src, b"old")
        @test read(fetch_blob(c, url).path) == b"old"
        write(src, b"new")
        @test read(fetch_blob(c, url).path) == b"old"     # opted out: warm wins
        @test counter.n == 1
    end
end

@testset "EARTHSCI_REVALIDATE_FILE — only an explicit denial switches it off" begin
    st = LocalStore(mktempdir())
    withenv("EARTHSCI_REVALIDATE_FILE" => nothing) do
        @test env_revalidate_file()
        @test Cache(st).revalidate_file
    end
    for off in ("0", "false", "NO", " off ")
        withenv("EARTHSCI_REVALIDATE_FILE" => off) do
            @test !env_revalidate_file()
            @test !Cache(st).revalidate_file
            @test Cache(st; revalidate_file = true).revalidate_file  # explicit wins
        end
    end
    # Anything else — including a typo — leaves the recheck ON.
    for on in ("1", "true", "yes", "", "offf", "maybe")
        withenv("EARTHSCI_REVALIDATE_FILE" => on) do
            @test env_revalidate_file()
        end
    end

    # ... and the env knob reaches the actual serve path.
    src = string(tempname(), ".csv")
    url = string("file://", src)
    root = mktempdir()
    write(src, b"old")
    withenv("EARTHSCI_REVALIDATE_FILE" => "0") do
        c = Cache(LocalStore(root); offline = false)
        @test read(fetch_blob(c, url).path) == b"old"
        write(src, b"new")
        @test read(fetch_blob(c, url).path) == b"old"
    end
end

# --- the existing blob-integrity check is a different question ---------------

@testset "verify=true alone never sees a replaced source" begin
    # Why `verify` did not catch any of this: it hashes the CACHED BLOB against
    # the manifest, i.e. the copy against the record of the copy. With the source
    # replaced it is perfectly consistent and perfectly wrong.
    src = string(tempname(), ".csv")
    url = string("file://", src)
    root = mktempdir()
    write(src, b"old")

    stale = Cache(LocalStore(root); offline = false, verify = true,
                  revalidate_file = false)
    @test read(fetch_blob(stale, url).path) == b"old"
    write(src, b"new-and-longer")
    @test read(fetch_blob(stale, url).path) == b"old"   # verify passes, happily

    # Rung 0 is the check that asks the source. Same cache root, same key.
    fixed = Cache(LocalStore(root); offline = false, verify = true)
    @test read(fetch_blob(fixed, url).path) == b"new-and-longer"
end

# --- the predicate itself, row by row of the §4.1 table ----------------------

@testset "file_source_is_current — the §4.1 table" begin
    mfor(body) = Manifest("file:///corpus/x.csv", nothing, nothing,
                          bytes2hex(sha256(body)), length(body),
                          "2026-01-01T00:00:00Z", nothing, nothing)
    dir = mktempdir()
    src = joinpath(dir, "x.csv")

    # unchanged ⇒ current
    body = Vector{UInt8}("year,value\n2016,1.0\n")
    write(src, body)
    @test file_source_is_current(src, mfor(body))

    # different length ⇒ not current
    @test !file_source_is_current(src, mfor(Vector{UInt8}("371-bytes-of-corpus")))

    # SAME length, different bytes ⇒ not current (the half a size check misses)
    same_len = mfor(Vector{UInt8}("year,value\n2016,9.0\n"))
    @test filesize(src) == same_len.bytes
    @test !file_source_is_current(src, same_len)

    # deleted ⇒ not current
    @test !file_source_is_current(joinpath(dir, "gone.csv"), mfor(b"whatever"))

    # a directory in place of the source ⇒ not current
    sub = joinpath(dir, "subdir")
    mkpath(sub)
    @test !file_source_is_current(sub, mfor(b"whatever"))
end

# --- a source this host cannot READ is not a source that changed -------------

# Can this process be locked out of a directory by permissions? Root cannot, and
# neither can Windows, so the abstention tests below have nothing to stand on
# there. Asked empirically rather than via geteuid: the question is whether the
# lockout WORKS, not who we are.
function _can_be_locked_out()
    Sys.iswindows() && return false
    dir = mktempdir()
    inner = joinpath(dir, "d")
    mkdir(inner)
    write(joinpath(inner, "probe"), b"x")
    chmod(inner, 0o000)
    blocked = try
        !isfile(joinpath(inner, "probe"))
    catch
        true
    finally
        chmod(inner, 0o755)
    end
    return blocked
end

@testset "rung 0 abstains on a source it cannot read" begin
    # Warming a cache where /corpus is mounted and reading it where it is not is
    # an ordinary HPC shape, and it used to work: the entry was served warm. The
    # first cut of this rung turned every `stat` failure into a re-ingest, which
    # turned that shape into a hard error. Only a genuine "no such file" is
    # evidence of a deletion; "permission denied" is evidence of nothing at all.
    if !_can_be_locked_out()
        @test_skip "needs a POSIX unprivileged user to make a directory unreadable"
    else
        dir = mktempdir()
        corpus = joinpath(dir, "corpus")
        mkdir(corpus)
        src = joinpath(corpus, "x.nc")
        write(src, b"warm-corpus")
        url = string("file://", src)
        root = mktempdir()

        with_counting_file() do counter
            c = Cache(LocalStore(root); offline = false)
            @test fetch_blob(c, url).status == :downloaded
            @test counter.n == 1

            chmod(corpus, 0o000)     # the closest a test gets to "not mounted"
            try
                e = fetch_blob(c, url)
                @test e.status == :hit
                @test read(e.path) == b"warm-corpus"
                @test counter.n == 1     # nothing re-ingested: nothing was learned
            finally
                chmod(corpus, 0o755)
            end
        end
    end
end

@testset "only a genuine absence reads as missing" begin
    mfor(body) = Manifest("file:///corpus/x.csv", nothing, nothing,
                          bytes2hex(sha256(body)), length(body),
                          "2026-01-01T00:00:00Z", nothing, nothing)
    dir = mktempdir()
    body = b"corpus"
    present = joinpath(dir, "present.nc")
    write(present, body)
    m = mfor(body)

    @test EarthSciIO.file_source_state(present, m) === EarthSciIO.SOURCE_CURRENT
    @test EarthSciIO.file_source_state(joinpath(dir, "gone.nc"), m) ===
          EarthSciIO.SOURCE_MISSING
    # A directory standing where the corpus was is the report's "pointed at a
    # path that is not there any more" shape.
    @test EarthSciIO.file_source_state(dir, m) === EarthSciIO.SOURCE_MISSING
    # A path *under a file* is a broken path, not a deletion: this host may
    # simply have the wrong mount.
    @test EarthSciIO.file_source_state(joinpath(present, "deeper.nc"), m) ===
          EarthSciIO.SOURCE_UNKNOWN

    if _can_be_locked_out()
        locked = joinpath(dir, "locked")
        mkdir(locked)
        hidden = joinpath(locked, "x.nc")
        write(hidden, body)
        chmod(locked, 0o000)
        state = try
            EarthSciIO.file_source_state(hidden, m)
        finally
            chmod(locked, 0o755)
        end
        @test state === EarthSciIO.SOURCE_UNKNOWN
    end
end

# --- the fingerprint memo ----------------------------------------------------

@testset "an unchanged source is read once, not once per call" begin
    # `fetch_blob` is called per TICK on this track (Provider keeps no
    # decoded-file buffer), so hashing on every call made rung 0 cost the whole
    # corpus per tick. A digest is kept against the (size, mtime) it was computed
    # for and reused while both hold.
    dir = mktempdir()
    src = joinpath(dir, "x.nc")
    body = rand(UInt8, 4096)
    write(src, body)
    m = Manifest("file:///corpus/x.nc", nothing, nothing, bytes2hex(sha256(body)),
                 length(body), "2026-01-01T00:00:00Z", nothing, nothing)

    rev = EarthSciIO.SourceRevalidator()
    for _ in 1:25
        @test EarthSciIO.source_state(rev, src, m) === EarthSciIO.SOURCE_CURRENT
    end
    @test rev.full_reads == 1     # 24 of the 25 calls came from the memo
end

@testset "the memo is dropped when the source changes" begin
    dir = mktempdir()
    src = joinpath(dir, "x.csv")
    body = b"year,value\n2016,1.0\n"
    write(src, body)
    m = Manifest("file:///corpus/x.csv", nothing, nothing, bytes2hex(sha256(body)),
                 length(body), "2026-01-01T00:00:00Z", nothing, nothing)

    rev = EarthSciIO.SourceRevalidator()
    @test EarthSciIO.source_state(rev, src, m) === EarthSciIO.SOURCE_CURRENT
    # Same length, different bytes — only the mtime betrays it, which is exactly
    # the case the memo has to get right.
    sleep(0.05)
    write(src, b"year,value\n2016,9.0\n")
    @test EarthSciIO.source_state(rev, src, m) === EarthSciIO.SOURCE_REPLACED
    @test rev.full_reads == 2
end
