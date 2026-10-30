# The streaming writer contract (spec/writer.md, RFC U12): regions, roles and the
# commit protocol, indexed records, statics, blobs, dtypes, read-back and resume.
# Unit-level versions of the §10 conformance cases (the cross-language corpus is
# `conformance/write_spec_v2.json`, driven by `conformance/dumpers/write_v2_julia.jl`).
#
# Values come from the §10 generator, evaluated at GLOBAL indices, so a region
# writer's array is a slice of the single writer's array and every comparison is
# exact (`==`): all profiles are lossless and dtypes are preserved.

import Blosc
import CodecZstd
import JSON

const SP = (P = [3, 5, 11, 17, 23],)
const NLAT, NLON = 6, 6          # lat shards are 2 long, lon shards 4: lon's last
# block [4, 6) ends at the array end
const DT = 3600.0

function _sval(::Type{T}, seed, r, idx) where {T}
    base = seed + 13 * r + sum(((i + 1) * SP.P[j] for (j, i) in enumerate(idx)); init = 0)
    T === UInt8 && return UInt8(mod(base, 251))
    T <: Integer && return T(base)
    return T(base + 0.25)
end

const _DIMLEN = Dict("lat" => NLAT, "lon" => NLON)

# The generator over `region` for a variable with non-time `dims`.
function _sarr(::Type{T}, seed, r, dims, region = Dict()) where {T}
    rg = Dict(String(k) => v for (k, v) in region)
    lo = [haskey(rg, d) ? rg[d][1] : 0 for d in dims]
    hi = [haskey(rg, d) ? rg[d][2] : _DIMLEN[d] for d in dims]
    a = Array{T}(undef, (hi .- lo)...)
    for I in CartesianIndices(a)
        a[I] = _sval(T, seed, r, [lo[k] + I[k] - 1 for k in eachindex(dims)])
    end
    return a
end

const _SVARS = [
    ("temp", ["time", "lat", "lon"], Float64, 100),
    ("pres", ["time", "lat", "lon"], Float32, 200),
    ("gm", ["time"], Float64, 300),     # replicated
    ("zm", ["time", "lon"], Float64, 400),
]     # lacks lat

function _sschema(; vars = _SVARS, statics = [], time_mode = :append, profile = :diagnostic)
    return OutputSchema(;
        dims = ["time" => 0, "lat" => NLAT, "lon" => NLON],
        time_dim = "time",
        coords = [
            "lat" => (Float64[10i for i in 0:(NLAT - 1)], Dict{String, Any}("units" => "degrees_north")),
            "lon" => (Float64[i for i in 0:(NLON - 1)], Dict{String, Any}("units" => "degrees_east")),
            "time" => (Float64[], Dict{String, Any}("units" => "seconds")),
        ],
        vars = [nm => OutputVar(d, T; attrs = Dict("seed" => s)) for (nm, d, T, s) in vars],
        statics = [nm => OutputVar(d, T) for (nm, d, T, _) in statics],
        chunk_shape = Dict("time" => 1, "lat" => 1, "lon" => 2),
        shard_shape = Dict("time" => 2, "lat" => 2, "lon" => 4),
        profile = profile, time_mode = time_mode,
        attrs = Dict{String, Any}("title" => "stream test")
    )
end

# The arrays one writer supplies for generator record `r`.
function _sarrays(r, region = Dict(); contributor = false, vars = _SVARS)
    out = Dict{String, Any}()
    for (nm, d, T, s) in vars
        sd = filter(!=("time"), d)
        contributor && !any(x -> haskey(region, x), sd) && continue
        out[nm] = _sarr(T, s, r, sd, region)
    end
    return out
end

# Every committed record/static of a store, read back whole.
function _readall(store; vars = _SVARS)
    m = read_output_manifest(store)
    n = m isa OutputManifest ? m.n_records :
        sum(Int(b) - Int(a) for (a, b) in m["records"]["committed"]; init = 0)
    return Dict(nm => [read_record(store, nm, i) for i in 0:(n - 1)] for (nm, _, _, _) in vars)
end

_expected(rs; vars = _SVARS) =
    Dict(nm => [_sarr(T, s, r, filter(!=("time"), d)) for r in rs] for (nm, d, T, s) in vars)

const ZW = ZarrWriter()
const QUAD = [
    (:owner, 0, Dict("lat" => (0, 4), "lon" => (0, 4))),
    (:contributor, 1, Dict("lat" => (0, 4), "lon" => (4, 6))),
    (:contributor, 2, Dict("lat" => (4, 6), "lon" => (0, 4))),
    (:contributor, 3, Dict("lat" => (4, 6), "lon" => (4, 6))),
]
# the two-writer split used below
const LA, LB = Dict("lat" => (0, 2)), Dict("lat" => (2, 6))

function _open_quad(store, schema)
    return [
        write_open!(
            ZW, nothing, store, schema; role = role, n_writers = 4, id = id,
            region = rg
        ) for (role, id, rg) in QUAD
    ]
end

_quad_rec!(hs, r) = for (h, (role, _, rg)) in zip(hs, QUAD)
    write_record!(ZW, h, DT * r, _sarrays(r, rg; contributor = role === :contributor))
end

@testset "zarr stream: schema fingerprint matches Python's canonical JSON" begin
    s = OutputSchema(;
        dims = ["time" => 0, "lat" => 4, "lon" => 4], time_dim = "time",
        vars = [
            "temp" => OutputVar(["time", "lat", "lon"], Float64),
            "gm" => OutputVar(["time"], Float32),
        ],
        statics = ["gé" => OutputVar(["lat", "lon"], UInt8)],
        chunk_shape = Dict("time" => 1, "lat" => 1, "lon" => 2),
        shard_shape = Dict("time" => 2, "lat" => 2, "lon" => 4)
    )
    # python3: json.dumps(F, sort_keys=True, separators=(",", ":"), ensure_ascii=True)
    @test schema_fingerprint(s) ==
        "sha256:1a15c0e63137c4929e8511d97eb2b5685418a9761185a8acfc494bf20db6125a"
end

@testset "zarr stream: commit token JSON round-trips" begin
    tok = CommitToken(
        1, ["lat" => (2, 4)], "sha256:ab", [(0, 0.0), (1, 3600.0), (2, 0.1)],
        ["dJ"], Tuple{String, Union{Int, Nothing}, String}[
            ("record", 1, "rng"), ("static", nothing, "s"),
        ],
        ["temperature/c/0/1/0"]
    )
    js = token_json(tok)
    @test startswith(
        js, "{\"schema\":\"earthsciio/commit-token/v1\",\"writer\":1," *
            "\"region\":{\"lat\":[2,4]},"
    )
    @test occursin("{\"index\":2,\"t\":0.1}", js)
    @test occursin("{\"scope\":\"static\",\"record\":null,\"key\":\"s\"}", js)
    @test parse_token(js) == tok
    @test parse_token(JSON.parse(js)) == tok
    @test_throws ErrorException parse_token("{\"schema\":\"nope\"}")
end

@testset "zarr stream: 2x2 region writers equal one writer bitwise" begin
    dir = mktempdir()
    s = _sschema()
    single = joinpath(dir, "single")
    h = write_open!(ZW, nothing, single, s)
    for r in 0:4
        @test write_record!(ZW, h, DT * r, _sarrays(r)) == r
    end
    m1 = write_close!(ZW, h)
    @test m1 isa OutputManifest && m1.n_records == 5

    quad = joinpath(dir, "quad")
    hs = _open_quad(quad, s)
    for r in 0:2
        _quad_rec!(hs, r)
    end
    toks = [write_flush!(ZW, h) for h in hs]
    @test [first.(t.records) for t in toks] == fill([0, 1, 2], 4)
    m = write_commit!(ZW, hs[1], toks)
    @test m["commit_seq"] == 1 && m["records"]["committed"] == [[0, 3]]
    for r in 3:4
        _quad_rec!(hs, r)
    end
    toks = [write_flush!(ZW, h) for h in hs[2:end]]
    for h in hs[2:end]
        @test write_close!(ZW, h) === nothing
    end
    m = write_close!(ZW, hs[1], toks)
    @test m["schema"] == "earthsciio/output-manifest/v2" && m["commit_seq"] == 2
    @test [w["id"] for w in m["writers"]] == 0:3

    a, b = _readall(single), _readall(quad)
    @test a == b == _expected(0:4)
    # a region read is the slice of the whole
    @test read_record(quad, "temp", 3; region = Dict("lat" => (2, 6))) ==
        read_record(single, "temp", 3)[3:6, :]
    @test eltype(read_record(quad, "pres", 0)) == Float32
end

@testset "zarr stream: a contributor never writes zarr.json or the manifest" begin
    dir = mktempdir()
    s = _sschema(; statics = [("st", ["lat", "lon"], Int64, 7)])
    hs = _open_quad(joinpath(dir, "q"), s)
    snapshot() = Dict(
        p => read(p) for (root, _, fs) in walkdir(joinpath(dir, "q"))
            for f in fs if f in ("zarr.json", "output_manifest.json")
            for p in (joinpath(root, f),)
    )
    before = snapshot()
    mt = Dict(p => mtime(p) for p in keys(before))
    c = hs[4]
    write_record!(ZW, c, 0.0, _sarrays(0, QUAD[4][3]; contributor = true))
    write_static!(ZW, c, "st", _sarr(Int64, 7, 0, ["lat", "lon"], QUAD[4][3]))
    write_blob!(ZW, c, :static, "rng", UInt8[1, 2])
    tok = write_flush!(ZW, c)
    write_close!(ZW, c)
    @test snapshot() == before
    @test all(mtime(p) == t for (p, t) in mt)
    # it PUT only shard objects of its own region (lat shard 2, lon shard 1) and
    # its blob — never `zm` (it lacks lat, and this writer's lat block is not at 0)
    @test !isempty(tok.objects)
    for k in tok.objects
        @test k == "blobs/static/3/rng" || occursin(r"^(temp|pres)/c/0/2/1$|^st/c/2/1$", k)
    end
    # a contributor supplying a replicated variable is refused
    hc = hs[2]
    @test_throws ErrorException write_record!(ZW, hc, 0.0, _sarrays(0, QUAD[2][3]))
end

@testset "zarr stream: misaligned regions are refused before any I/O" begin
    dir = mktempdir()
    s = _sschema()
    bad = joinpath(dir, "bad")
    @test_throws RegionNotShardAligned write_open!(
        ZW, nothing, bad, s; role = :owner,
        n_writers = 2, region = Dict("lat" => (0, 3))
    )
    @test !ispath(bad)
    store = joinpath(dir, "ok")
    write_open!(ZW, nothing, store, s; role = :owner, n_writers = 2, region = Dict("lat" => (0, 2)))
    @test_throws RegionNotShardAligned write_open!(
        ZW, nothing, store, s; role = :contributor,
        id = 1, region = Dict("lat" => (1, 6))
    )
    @test_throws RegionNotShardAligned write_open!(
        ZW, nothing, store, s; role = :contributor,
        id = 1, region = Dict("lon" => (2, 6))
    )
    @test_throws RegionNotShardAligned write_open!(
        ZW, nothing, store, s; role = :contributor,
        id = 1, region = Dict("lat" => (2, 7))
    )
    m = read_output_manifest(store)
    @test m["commit_seq"] == 0 && m["records"]["committed"] == []
    # the contributor's schema must match the store's
    other = _sschema(; profile = :checkpoint)
    @test_throws ErrorException write_open!(
        ZW, nothing, store, other; role = :contributor,
        id = 1, region = Dict("lat" => (2, 6))
    )
end

@testset "zarr stream: a gap or an overlap is a CommitError; the prefix stands" begin
    for (label, r9) in (("gap", (4, 6)), ("overlap", (0, 6)))
        dir = mktempdir()
        s = _sschema()
        store = joinpath(dir, label)
        w0 = write_open!(ZW, nothing, store, s; role = :owner, n_writers = 2, region = LA)
        w1 = write_open!(
            ZW, nothing, store, s; role = :contributor, n_writers = 2, id = 1,
            region = LB
        )
        write_record!(ZW, w0, 0.0, _sarrays(0, LA))
        write_record!(ZW, w1, 0.0, _sarrays(0, LB; contributor = true))
        write_commit!(ZW, w0, [write_flush!(ZW, w0), write_flush!(ZW, w1)])
        before = read(joinpath(store, "output_manifest.json"))
        # a late joiner (same id) takes a bad region; it joins after record 0 is
        # committed, so it must reload record 0 of its region of time shard 0
        R9 = Dict("lat" => r9)
        w9 = write_open!(
            ZW, nothing, store, s; role = :contributor, n_writers = 2, id = 1,
            region = R9
        )
        write_record!(ZW, w0, DT, _sarrays(1, LA))
        @test write_record!(ZW, w9, DT, _sarrays(1, R9; contributor = true)) == 1
        err = try
            write_commit!(ZW, w0, [write_flush!(ZW, w0), write_flush!(ZW, w9)]); nothing
        catch e
            e
        end
        @test err isa CommitError && occursin(label, err.msg)
        @test read(joinpath(store, "output_manifest.json")) == before
        @test _readall(store) == _expected([0])
    end
    # a missing writer is a CommitError too
    dir = mktempdir()
    w0 = write_open!(
        ZW, nothing, joinpath(dir, "m"), _sschema(); role = :owner,
        n_writers = 2, region = LA
    )
    @test_throws CommitError write_commit!(ZW, w0, [write_flush!(ZW, w0)])
end

@testset "zarr stream: CommitError on divergent tokens" begin
    dir = mktempdir()
    s = _sschema()
    store = joinpath(dir, "s")
    w0 = write_open!(ZW, nothing, store, s; role = :owner, n_writers = 2, region = LA)
    w1 = write_open!(
        ZW, nothing, store, s; role = :contributor, n_writers = 2, id = 1,
        region = LB
    )
    write_record!(ZW, w0, 0.0, _sarrays(0, LA))
    t0 = write_flush!(ZW, w0)
    t1 = write_flush!(ZW, w1)                          # no records: diverges
    @test_throws CommitError write_commit!(ZW, w0, [t0, t1])
    @test_throws CommitError write_commit!(ZW, w0, [t0, t0])          # duplicate writer
    bad = CommitToken(
        1, t1.region, "sha256:other", t0.records, String[],
        Tuple{String, Union{Int, Nothing}, String}[], String[]
    )
    @test_throws CommitError write_commit!(ZW, w0, [t0, bad])
    @test read_output_manifest(store)["commit_seq"] == 0
end

@testset "zarr stream: indexed records in reverse equal forward append" begin
    dir = mktempdir()
    n = 5
    times = [DT * i for i in 0:(n - 1)]
    fwd = joinpath(dir, "fwd")
    h = write_open!(ZW, nothing, fwd, _sschema())
    for r in 0:(n - 1)
        write_record!(ZW, h, times[r + 1], _sarrays(r))
    end
    write_close!(ZW, h)

    rev = joinpath(dir, "rev")
    s = _sschema(; time_mode = IndexedTime(times))
    @test Dict(s.dims)["time"] == n
    h = write_open!(ZW, nothing, rev, s)
    @test_throws ErrorException write_record!(ZW, h, 0.0, _sarrays(0))      # append call
    @test_throws ErrorException write_record_at!(ZW, h, 1, 1.0, _sarrays(1)) # wrong t
    for i in (n - 1):-1:0
        write_record_at!(ZW, h, i, times[i + 1], _sarrays(i))
    end
    m = write_close!(ZW, h)
    @test m["commit_seq"] == 1
    @test all(e["generation"] == 1 && e["committed"] for e in m["records"]["entries"])
    @test m["time_mode"] == Dict("kind" => "indexed", "n_records" => n)
    @test _readall(rev) == _readall(fwd) == _expected(0:(n - 1))

    # two region writers, the later indices committed first
    reg = joinpath(dir, "reg")
    w0 = write_open!(ZW, nothing, reg, s; role = :owner, n_writers = 2, region = LA)
    w1 = write_open!(
        ZW, nothing, reg, s; role = :contributor, n_writers = 2, id = 1,
        region = LB
    )
    put(i) = (
        write_record_at!(ZW, w0, i, times[i + 1], _sarrays(i, LA));
        write_record_at!(ZW, w1, i, times[i + 1], _sarrays(i, LB; contributor = true))
    )
    foreach(put, (4, 3))
    write_commit!(ZW, w0, [write_flush!(ZW, w0), write_flush!(ZW, w1)])
    foreach(put, (2, 1, 0))
    t1 = write_flush!(ZW, w1)
    write_close!(ZW, w1)
    m = write_close!(ZW, w0, [t1])
    @test m["commit_seq"] == 2
    @test _readall(reg) == _expected(0:(n - 1))
end

@testset "zarr stream: rewriting a committed indexed record needs invalidate" begin
    dir = mktempdir()
    times = [0.0, DT]
    store = joinpath(dir, "ring")
    h = write_open!(ZW, nothing, store, _sschema(; time_mode = IndexedTime(times)))
    write_record_at!(ZW, h, 0, 0.0, _sarrays(0))
    write_record_at!(ZW, h, 1, DT, _sarrays(1))
    write_flush!(ZW, h)
    @test_throws ErrorException write_record_at!(ZW, h, 0, 0.0, _sarrays(7))
    write_invalidate!(ZW, h, [0])
    m = read_output_manifest(store)
    @test m["commit_seq"] == 2 && m["records"]["committed"] == [[1, 2]]
    @test_throws ErrorException read_record(store, "temp", 0)      # not committed now
    write_record_at!(ZW, h, 0, 0.0, _sarrays(7))
    write_flush!(ZW, h)
    m = write_close!(ZW, h)
    @test m["commit_seq"] == 3
    @test [e["generation"] for e in m["records"]["entries"]] == [2, 1]
    @test _readall(store) == _expected([7, 1])
end

@testset "zarr stream: partial-shard resume continues bitwise" begin
    dir = mktempdir()
    s = _sschema()
    store = joinpath(dir, "single")
    h = write_open!(ZW, nothing, store, s)
    for r in 0:2
        write_record!(ZW, h, DT * r, _sarrays(r))
    end
    write_flush!(ZW, h)                    # commits 0..2 mid-shard
    write_record!(ZW, h, DT * 3, _sarrays(3))     # fills shard 1: PUT, not committed
    @test read_output_manifest(store).n_records == 3
    # the handle is dropped; resume and continue
    h2 = write_resume(ZW, nothing, store)
    for r in 3:4
        write_record!(ZW, h2, DT * r, _sarrays(r))
    end
    m = write_close!(ZW, h2)
    @test m isa OutputManifest && m.n_records == 5
    @test _readall(store) == _expected(0:4)

    # the same with two region writers (v2): resume truncating nothing is no commit
    reg = joinpath(dir, "reg")
    A, B = LA, LB
    w0 = write_open!(ZW, nothing, reg, s; role = :owner, n_writers = 2, region = A)
    w1 = write_open!(ZW, nothing, reg, s; role = :contributor, n_writers = 2, id = 1, region = B)
    for r in 0:2
        write_record!(ZW, w0, DT * r, _sarrays(r, A))
        write_record!(ZW, w1, DT * r, _sarrays(r, B; contributor = true))
    end
    write_commit!(ZW, w0, [write_flush!(ZW, w0), write_flush!(ZW, w1)])
    write_record!(ZW, w0, DT * 3, _sarrays(3, A))
    write_record!(ZW, w1, DT * 3, _sarrays(3, B; contributor = true))
    write_flush!(ZW, w0); write_flush!(ZW, w1)                  # tokens lost
    v0 = write_resume(ZW, nothing, reg; role = :owner, n_writers = 2, region = A)
    v1 = write_resume(ZW, nothing, reg; role = :contributor, n_writers = 2, id = 1, region = B)
    @test read_output_manifest(reg)["commit_seq"] == 1
    for r in 3:4
        write_record!(ZW, v0, DT * r, _sarrays(r, A))
        write_record!(ZW, v1, DT * r, _sarrays(r, B; contributor = true))
    end
    t1 = write_flush!(ZW, v1)
    @test_throws ErrorException write_record!(ZW, v1, DT * 2, _sarrays(2, B; contributor = true))
    write_close!(ZW, v1)
    m = write_close!(ZW, v0, [t1])
    @test m["commit_seq"] == 2
    @test _readall(reg) == _expected(0:4)
end

@testset "zarr stream: resume truncates to t <= at" begin
    dir = mktempdir()
    store = joinpath(dir, "trunc")
    h = write_open!(ZW, nothing, store, _sschema())
    for r in 0:4
        write_record!(ZW, h, DT * r, _sarrays(r))
    end
    write_flush!(ZW, h)
    h2 = write_resume(ZW, nothing, store; at = 2 * DT)
    m = read_output_manifest(store)
    @test m isa OutputManifest && m.n_records == 3 && m.last_t == 2 * DT
    @test JSON.parsefile(joinpath(store, "temp", "zarr.json"))["shape"] == [3, NLAT, NLON]
    @test_throws ErrorException write_record!(ZW, h2, 2 * DT, _sarrays(2))  # not after last t
    @test write_record!(ZW, h2, 3 * DT, _sarrays(13)) == 3
    write_close!(ZW, h2)
    @test _readall(store) == _expected([0, 1, 2, 13])
    # indexed stores never truncate
    ix = joinpath(dir, "ix")
    write_close!(ZW, write_open!(ZW, nothing, ix, _sschema(; time_mode = IndexedTime([0.0]))))
    @test_throws ErrorException write_resume(ZW, nothing, ix; at = 0.0)
end

@testset "zarr stream: dtypes round-trip bitwise (incl. UInt8)" begin
    dir = mktempdir()
    vars = [
        ("v_$(T)", ["time", "lat", "lon"], T, 10k + 1)
            for (k, T) in enumerate((Float64, Float32, Int32, Int64, UInt8))
    ]
    for profile in (:diagnostic, :wasm)
        store = joinpath(dir, string(profile))
        h = write_open!(ZW, nothing, store, _sschema(; vars = vars, profile = profile))
        for r in 0:2
            write_record!(ZW, h, DT * r, _sarrays(r; vars = vars))
        end
        write_close!(ZW, h)
        got = _readall(store; vars = vars)
        @test got == _expected(0:2; vars = vars)
        for (nm, _, T, _) in vars
            @test eltype(got[nm][1]) == T
        end
        @test JSON.parsefile(joinpath(store, "v_UInt8", "zarr.json"))["data_type"] == "uint8"
    end
end

@testset "zarr stream: statics and blobs over two region writers" begin
    dir = mktempdir()
    sts = [
        ("dJ", ["lat", "lon"], Float64, 500), ("dJ_dlon", ["lon"], Float64, 600),
        ("count", ["lat", "lon"], UInt8, 700),
    ]
    s = _sschema(; statics = sts)
    store = joinpath(dir, "sb")
    A, B = LA, LB
    w0 = write_open!(ZW, nothing, store, s; role = :owner, n_writers = 2, region = A)
    w1 = write_open!(ZW, nothing, store, s; role = :contributor, n_writers = 2, id = 1, region = B)
    @test_throws ErrorException write_static!(ZW, w1, "dJ_dlon", _sarr(Float64, 600, 0, ["lon"]))
    for r in 0:1
        write_record!(ZW, w0, DT * r, _sarrays(r, A))
        write_record!(ZW, w1, DT * r, _sarrays(r, B; contributor = true))
    end
    for (nm, d, T, seed) in sts
        write_static!(ZW, w0, nm, _sarr(T, seed, 0, d, A))
        "lat" in d && write_static!(ZW, w1, nm, _sarr(T, seed, 0, d, B))
    end
    @test_throws ErrorException read_static(store, "dJ")            # not committed yet
    write_blob!(ZW, w0, :static, "rng", UInt8[0, 1, 2, 3])
    write_blob!(ZW, w1, :static, "rng", UInt8[0xff, 0x10])
    write_blob!(ZW, w1, (:record, 1), "ctrl.state", hex2bytes("deadbeef"))
    @test_throws ArgumentError write_blob!(ZW, w1, :static, "a/b", UInt8[1])
    t1 = write_flush!(ZW, w1)
    @test t1.statics == ["count", "dJ"]
    @test t1.blobs == [("record", 1, "ctrl.state"), ("static", nothing, "rng")]
    write_close!(ZW, w1)
    m = write_close!(ZW, w0, [t1])
    @test m["commit_seq"] == 1
    @test [st["generation"] for st in m["statics"]] == [1, 1, 1]
    for (nm, d, T, seed) in sts
        @test read_static(store, nm) == _sarr(T, seed, 0, d)
    end
    @test read_static(store, "dJ"; region = B) == _sarr(Float64, 500, 0, ["lat", "lon"], B)
    @test read_blob(store, :static, 0, "rng") == UInt8[0, 1, 2, 3]
    @test read_blob(store, :static, 1, "rng") == UInt8[0xff, 0x10]
    @test read_blob(store, (:record, 1), 1, "ctrl.state") == hex2bytes("deadbeef")
    @test_throws ErrorException read_blob(store, (:record, 0), 1, "ctrl.state")
    @test length(m["blobs"]) == 3 && m["blobs"][1]["scope"] == "record"
end

@testset "zarr stream: Single + append + no statics keeps the v1 manifest" begin
    dir = mktempdir()
    store = joinpath(dir, "v1")
    h = write_open!(ZW, nothing, store, _sschema())
    @test !isfile(joinpath(store, "output_manifest.json"))     # v1: none until a commit
    @test_throws ErrorException write_blob!(ZW, h, :static, "x", UInt8[1])
    for r in 0:2
        write_record!(ZW, h, DT * r, _sarrays(r))
    end
    @test first.(write_flush!(ZW, h).records) == [0, 1, 2]   # commits
    tok = write_flush!(ZW, h)
    @test isempty(tok)                                   # nothing pending: no-op
    m = write_close!(ZW, h)
    d = JSON.parsefile(joinpath(store, "output_manifest.json"))
    @test d["schema"] == "earthsciio/output-manifest/v1"
    @test sort(collect(keys(d))) == sort(
        [
            "schema", "base_url", "format", "zarr_format",
            "profile", "codec", "time_dim", "dims", "vars", "chunk_shape", "shard_shape",
            "time_shards", "last_t", "n_records", "created_at",
        ]
    )
    @test d["dims"][1] == Dict("name" => "time", "length" => 0)
    @test [x["n"] for x in d["time_shards"]] == [2, 1]
    @test m.n_records == 3
    # an Owner{1} store with the same schema is v2 (and can hold blobs)
    o = write_open!(ZW, nothing, joinpath(dir, "o1"), _sschema(); role = :owner)
    write_blob!(ZW, o, :static, "x", UInt8[1])
    @test write_close!(ZW, o)["schema"] == "earthsciio/output-manifest/v2"
end

@testset "zarr stream: an owner off the origin writes replicated vars and statics" begin
    # The owner holds lat [2, 6): no writer's region starts at 0 on lat, but the
    # replicated var, the time coordinate and the replicated static are still
    # the owner's to write.
    dir = mktempdir()
    sts = [("dJ", ["lat", "lon"], Float64, 500), ("dJ_dlon", ["lon"], Float64, 600)]
    s = _sschema(; statics = sts)
    store = joinpath(dir, "off")
    w0 = write_open!(ZW, nothing, store, s; role = :owner, n_writers = 2, region = LB)
    w1 = write_open!(ZW, nothing, store, s; role = :contributor, n_writers = 2, id = 1, region = LA)
    for r in 0:2
        write_record!(ZW, w0, DT * (r + 1), _sarrays(r, LB))
        write_record!(ZW, w1, DT * (r + 1), _sarrays(r, LA; contributor = true))
    end
    for (nm, d, T, seed) in sts
        write_static!(ZW, w0, nm, _sarr(T, seed, 0, d, LB))
        "lat" in d && write_static!(ZW, w1, nm, _sarr(T, seed, 0, d, LA))
    end
    t1 = write_flush!(ZW, w1)
    write_close!(ZW, w1)
    write_close!(ZW, w0, [t1])
    @test _readall(store) == _expected(0:2)
    for (nm, d, T, seed) in sts
        @test read_static(store, nm) == _sarr(T, seed, 0, d)
    end
    tm = JSON.parsefile(joinpath(store, "time", "zarr.json"))
    @test tm["shape"] == [3]
    @test isfile(joinpath(store, "time", "c", "0"))
end

@testset "zarr stream: a contributor's refresh reloads its whole view" begin
    dir = mktempdir()
    times = [0.0, DT]
    s = _sschema(; time_mode = IndexedTime(times))
    store = joinpath(dir, "refresh")
    w0 = write_open!(ZW, nothing, store, s; role = :owner, n_writers = 2, region = LA)
    w1 = write_open!(ZW, nothing, store, s; role = :contributor, n_writers = 2, id = 1, region = LB)
    for r in 0:1
        write_record_at!(ZW, w0, r, times[r + 1], _sarrays(r, LA))
        write_record_at!(ZW, w1, r, times[r + 1], _sarrays(r, LB; contributor = true))
    end
    write_blob!(ZW, w1, (:record, 0), "k", UInt8[1])
    write_commit!(ZW, w0, [write_flush!(ZW, w0), write_flush!(ZW, w1)])
    write_invalidate!(ZW, w0, [0])
    # the contributor's view says record 0 is committed: it re-reads the
    # manifest, which has invalidated it and dropped its record(0) blob
    write_record_at!(ZW, w1, 0, 0.0, _sarrays(7, LB; contributor = true))
    @test isempty(w1.blobs)
    @test w1.commit_seq == 2
    @test first.(w1.writers) == [0, 1]           # as of the last commit, not of the join
    write_record_at!(ZW, w0, 0, 0.0, _sarrays(7, LA))
    m = write_commit!(ZW, w0, [write_flush!(ZW, w0), write_flush!(ZW, w1)])
    @test [e["generation"] for e in m["records"]["entries"]] == [2, 1]
    @test _readall(store) == _expected([7, 1])
    # committed again: a rewrite without invalidate is refused after the refresh
    @test_throws ErrorException write_record_at!(ZW, w1, 0, 0.0, _sarrays(8, LB; contributor = true))
end

@testset "zarr stream: a failed commit changes nothing and can be retried" begin
    dir = mktempdir()
    times = [0.0, DT]
    store = joinpath(dir, "flaky")
    h = write_open!(
        ZW, nothing, store, _sschema(; time_mode = IndexedTime(times));
        role = :owner, n_writers = 1
    )
    write_record_at!(ZW, h, 0, 0.0, _sarrays(0))
    write_record_at!(ZW, h, 1, DT, _sarrays(1))
    tok = write_flush!(ZW, h)
    chmod(store, 0o555)                 # the manifest write fails
    try
        @test_throws Exception write_commit!(ZW, h, [tok])
    finally
        chmod(store, 0o755)
    end
    @test h.commit_seq == 0 && !any(h.rec_committed)
    m = write_commit!(ZW, h, [tok])
    @test m["commit_seq"] == 1 && m["records"]["committed"] == [[0, 2]]
end

@testset "zarr stream: blob keys '.' and '..' are refused" begin
    dir = mktempdir()
    h = write_open!(
        ZW, nothing, joinpath(dir, "b"), _sschema(; statics = [("dJ", ["lat", "lon"], Float64, 500)]);
        role = :owner, n_writers = 1
    )
    @test_throws ArgumentError write_blob!(ZW, h, :static, ".", UInt8[1])
    @test_throws ArgumentError write_blob!(ZW, h, :static, "..", UInt8[1])
end
