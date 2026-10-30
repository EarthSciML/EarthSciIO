# Julia track's WRITE driver for the v2 write-conformance corpus
# (`conformance/write_spec_v2.json`, spec/writer.md §10).
#
# Executes every case's script of writer operations — create / join / write /
# flush / commit / invalidate / resume / close / drop — against a fresh store at
# `OUT_ROOT/<case>/store/` with the Julia streaming writer (`EarthSciIO.ZarrWriter`,
# julia/src/zarr_stream.jl), and records each op's outcome in
# `OUT_ROOT/<case>/result.json`:
#
#     {"case": "<name>", "ops": [{"index": k, "op": "<op>", "ok": true|false,
#                                 "error": null | "RegionNotShardAligned" |
#                                          "CommitError" | "Error",
#                                 "message": "<text>"}]}
#
# Data values come from the §10 generator evaluated over each writer's region
# (global indices), so a region writer's arrays are slices of the single-writer
# arrays by construction. Every token is round-tripped through its JSON form
# (`token_json` -> `parse_token`) before it is delivered, as an MPI caller would.
#
# Usage:  julia --project="$(julia conformance/dumpers/julia_env.jl)" \
#             conformance/dumpers/write_v2_julia.jl SPEC.json OUT_ROOT

using EarthSciIO
import JSON

include(joinpath(@__DIR__, "codec_weakdeps.jl"))
load_codec_weakdeps!()

const DTYPE = Dict(
    "float64" => Float64, "float32" => Float32, "int32" => Int32,
    "int64" => Int64, "uint8" => UInt8
)

_attrs(d) = Dict{String, Any}(String(k) => v for (k, v) in something(get(d, "attrs", nothing), Dict()))

function build_schema(s)
    dims = [String(d[1]) => Int(d[2]) for d in s["dims"]]
    coords = Pair{String, Tuple{Vector, Dict{String, Any}}}[
        String(c["name"]) => (Float64[Float64(v) for v in c["values"]], _attrs(c))
            for c in s["coords"]
    ]
    var(v) = String(v["name"]) => OutputVar(
        String.(v["dims"]), DTYPE[String(v["dtype"])];
        attrs = _attrs(v)
    )
    tm = s["time_mode"]
    time_mode = String(tm["kind"]) == "indexed" ? IndexedTime(Float64.(tm["times"])) : :append
    return OutputSchema(;
        dims = dims, time_dim = String(s["time_dim"]),
        vars = [var(v) for v in s["vars"]],
        statics = [var(v) for v in s["statics"]],
        chunk_shape = Dict(String(k) => Int(v) for (k, v) in s["chunk_shape"]),
        shard_shape = Dict(String(k) => Int(v) for (k, v) in s["shard_shape"]),
        coords = coords, profile = Symbol(s["profile"]),
        attrs = _attrs(Dict("attrs" => s["group_attrs"])),
        time_mode = time_mode
    )
end

# --- the §10 generator ------------------------------------------------------

function gen_value(gen, ::Type{T}, seed::Int, r::Int, idx) where {T}
    P = gen["P"]
    base = seed + Int(gen["record_stride"]) * r + sum((i + 1) * Int(P[j]) for (j, i) in enumerate(idx); init = 0)
    T === UInt8 && return UInt8(mod(base, Int(gen["uint8_modulus"])))
    T <: Integer && return T(base)
    return T(base + Float64(gen["float_offset"]))
end

# The array of `v` over a writer's region: indexed in the variable's non-time dim
# order (Julia arrays are column-major, but the writer encodes C-order chunks
# itself, so the index -> value map is all that matters).
function gen_array(gen, v, time_dim, region, dimlen, r)
    T = DTYPE[String(v["dtype"])]
    dims = [String(d) for d in v["dims"] if d != time_dim]
    lo = [haskey(region, d) ? region[d][1] : 0 for d in dims]
    hi = [haskey(region, d) ? region[d][2] : dimlen[d] for d in dims]
    out = Array{T}(undef, (hi .- lo)...)
    for I in CartesianIndices(out)
        out[I] = gen_value(gen, T, Int(v["seed"]), r, [lo[k] + I[k] - 1 for k in eachindex(dims)])
    end
    return out
end

# --- the op interpreter -----------------------------------------------------

function parse_role(role)
    role == "single" && return (:single, 1, 0)
    haskey(role, "owner") && return (:owner, Int(role["owner"]), 0)
    haskey(role, "contributor") && return (:contributor, 1, Int(role["contributor"]))
    error("unknown role $role")
end

mutable struct Writer_
    h::Any
    role::Symbol
    region::Dict{String, Tuple{Int, Int}}
end

error_kind(e) = e isa RegionNotShardAligned ? "RegionNotShardAligned" :
    e isa CommitError ? "CommitError" : "Error"

function run_case(case, gen, out_root)
    name = String(case["name"])
    sdict = case["schema"]
    schema = build_schema(sdict)
    time_dim = schema.time_dim
    dimlen = Dict(schema.dims)
    store = joinpath(out_root, name, "store")
    rm(store; force = true, recursive = true)
    mkpath(dirname(store))
    zw = ZarrWriter()
    writers = Dict{String, Writer_}()
    tokens = Dict{String, CommitToken}()
    vars = sdict["vars"]
    statics = Dict(String(s["name"]) => s for s in sdict["statics"])

    function arrays_for(wr::Writer_, r)
        out = Dict{String, Any}()
        for v in vars
            vd = [String(d) for d in v["dims"]]
            partitioned = any(d -> haskey(wr.region, d), vd)
            wr.role === :contributor && !partitioned && continue
            out[String(v["name"])] = gen_array(gen, v, time_dim, wr.region, dimlen, r)
        end
        return out
    end

    results = Any[]
    for (k, op) in enumerate(case["ops"])
        kind = String(op["op"])
        wname = String(get(op, "writer", ""))
        ok, err, msg = true, nothing, ""
        try
            if kind in ("create", "join", "resume")
                role, n, id = parse_role(op["role"])
                region = Dict{String, Tuple{Int, Int}}(
                    String(d) => (Int(ab[1]), Int(ab[2]))
                        for (d, ab) in op["region"]
                )
                h = if kind == "resume"
                    at = op["at"]
                    write_resume(
                        zw, nothing, store; role = role, n_writers = n, id = id,
                        region = region, at = at === nothing ? nothing : Float64(at),
                        schema = schema
                    )
                else
                    write_open!(
                        zw, nothing, store, schema; role = role, n_writers = n,
                        id = id, region = region
                    )
                end
                writers[wname] = Writer_(h, role, region)
            elseif kind == "write_record"
                wr = writers[wname]
                write_record!(zw, wr.h, Float64(op["t"]), arrays_for(wr, Int(op["r"])))
            elseif kind == "write_record_at"
                wr = writers[wname]
                i = Int(op["index"])
                t = schema.time_mode.times[i + 1]
                write_record_at!(zw, wr.h, i, t, arrays_for(wr, Int(op["r"])))
            elseif kind == "write_static"
                wr = writers[wname]
                s = statics[String(op["name"])]
                write_static!(
                    zw, wr.h, String(op["name"]),
                    gen_array(gen, s, time_dim, wr.region, dimlen, 0)
                )
            elseif kind == "write_blob"
                wr = writers[wname]
                scope = op["scope"] == "static" ? :static : (:record, Int(op["record"]))
                write_blob!(zw, wr.h, scope, String(op["key"]), hex2bytes(String(op["hex"])))
            elseif kind == "flush"
                tok = write_flush!(zw, writers[wname].h)
                tokens[String(op["token"])] = parse_token(token_json(tok))
            elseif kind == "commit"
                write_commit!(zw, writers[wname].h, CommitToken[tokens[t] for t in op["tokens"]])
            elseif kind == "invalidate"
                write_invalidate!(zw, writers[wname].h, Int.(op["indices"]))
            elseif kind == "close"
                write_close!(zw, writers[wname].h, CommitToken[tokens[t] for t in op["tokens"]])
            elseif kind == "drop"
                delete!(writers, wname)
            else
                error("unknown op '$kind'")
            end
        catch e
            ok, err, msg = false, error_kind(e), sprint(showerror, e)
        end
        push!(
            results, Dict(
                "index" => k - 1, "op" => kind, "ok" => ok,
                "error" => err, "message" => msg
            )
        )
    end
    open(joinpath(out_root, name, "result.json"), "w") do io
        write(io, JSON.json(Dict("case" => name, "ops" => results), 1))
        write(io, "\n")
    end
    nfail = count(r -> !r["ok"], results)
    return println("[julia-writer-v2] $name: $(length(results)) ops ($nfail raised)")
end

function main()
    length(ARGS) == 2 || error("usage: write_v2_julia.jl SPEC.json OUT_ROOT")
    spec = JSON.parsefile(ARGS[1])
    out_root = abspath(ARGS[2])
    mkpath(out_root)
    for case in spec["cases"]
        run_case(case, spec["generator"], out_root)
    end
    return
end

main()
