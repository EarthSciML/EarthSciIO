# Julia track's READ driver for the v2 write-conformance corpus
# (`conformance/write_spec_v2.json`, spec/writer.md §10).
#
# Reads every `STORES_ROOT/<case>/store` (written by ANY track's v2 driver) through
# the Julia read-back API — `read_output_manifest`, `read_record`, `read_static`,
# `read_blob`, which read the store directly, never through the cache — and emits
# an `earthsciio/write-v2-dump/v1` dump that `conformance/crosscheck_write_v2.py`
# compares bitwise against the oracle.
#
# Dumps list values in C (row-major) order and shapes in the variable's dim order
# (time excluded). The read-back arrays are Julia arrays indexed in that dim
# order, so the C-order flatten is `permutedims` over the reversed axes.
#
# Usage:  julia --project="$(julia conformance/dumpers/julia_env.jl)" \
#             conformance/dumpers/read_v2_julia.jl SPEC.json STORES_ROOT WRITER_LABEL OUT.json

using EarthSciIO
import JSON

include(joinpath(@__DIR__, "codec_weakdeps.jl"))
load_codec_weakdeps!()

const DTYPE_NAME = Dict(
    Float64 => "float64", Float32 => "float32", Int32 => "int32",
    Int64 => "int64", UInt8 => "uint8", UInt32 => "uint32",
    UInt64 => "uint64"
)

_corder(a::AbstractArray{T, 0}) where {T} = [a[]]
_corder(a::AbstractVector) = collect(a)
_corder(a::AbstractArray) = vec(permutedims(a, ndims(a):-1:1))

function encode(a::AbstractArray)
    T = eltype(a)
    vals = T <: AbstractFloat ? Any[Float64(x) for x in _corder(a)] :
        Any[Int(x) for x in _corder(a)]
    return Dict("dtype" => DTYPE_NAME[T], "shape" => collect(Int, size(a)), "values" => vals)
end

function dump_case(case, store)
    m = read_output_manifest(store)
    m === nothing && error("no output manifest in $store")
    v1 = m isa OutputManifest
    committed = v1 ? collect(0:(m.n_records - 1)) :
        Int[i for (a, b) in m["records"]["committed"] for i in Int(a):(Int(b) - 1)]
    generations = nothing
    if !v1 && haskey(m["records"], "entries")
        generations = Dict(
            string(Int(e["index"])) => Int(e["generation"])
                for e in m["records"]["entries"]
        )
    end
    records = Dict{String, Any}()
    for v in case["schema"]["vars"]
        nm = String(v["name"])
        records[nm] = Dict(string(i) => encode(read_record(store, nm, i)) for i in committed)
    end
    statics = Dict{String, Any}()
    blobs = Any[]
    if !v1
        for s in m["statics"]
            Bool(s["committed"]) || continue
            statics[String(s["name"])] = encode(read_static(store, String(s["name"])))
        end
        for b in m["blobs"]
            scope = b["scope"] == "static" ? :static : (:record, Int(b["record"]))
            bytes = read_blob(store, scope, Int(b["writer"]), String(b["key"]))
            push!(
                blobs, Dict(
                    "scope" => b["scope"], "record" => b["record"],
                    "writer" => Int(b["writer"]), "key" => b["key"],
                    "hex" => bytes2hex(bytes)
                )
            )
        end
    end
    return Dict(
        "manifest_schema" => v1 ? "earthsciio/output-manifest/v1" : String(m["schema"]),
        "commit_seq" => v1 ? nothing : Int(m["commit_seq"]),
        "committed" => committed, "generations" => generations,
        "records" => records, "statics" => statics, "blobs" => blobs
    )
end

function main()
    length(ARGS) == 4 ||
        error("usage: read_v2_julia.jl SPEC.json STORES_ROOT WRITER_LABEL OUT.json")
    spec = JSON.parsefile(ARGS[1])
    root = abspath(ARGS[2])
    cases = Dict{String, Any}()
    for case in spec["cases"]
        name = String(case["name"])
        cases[name] = dump_case(case, joinpath(root, name, "store"))
    end
    out = Dict(
        "schema" => "earthsciio/write-v2-dump/v1", "reader" => "julia",
        "writer" => ARGS[3], "cases" => cases
    )
    open(ARGS[4], "w") do io
        write(io, JSON.json(out))
        write(io, "\n")
    end
    return println("[julia-reader-v2] read $(length(cases)) stores written by $(ARGS[3])")
end

main()
