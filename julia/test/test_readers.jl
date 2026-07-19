# Format-reader decode parity — conformance checks 3 (decode) + 4 (native-array
# equality), the half component (a)'s test_conformance.jl left to component (b)
# (esio-9nb.5). This is the Julia mirror of conformance/verify.py: open each
# committed corpus blob with the FORMAT_REGISTRY reader its case names, CF-decode
# per spec/conformance.md §3, and assert the native arrays equal the case's
# `expected` (the Python/xarray oracle) within the spec's tolerances.

# --- comparison helpers (mirror verify.py _flat / _cmp_numeric / _cmp_string) -

# Recursive flatten of a nested `expected.data` array (C/row-major order).
function _flat(x)
    out = Any[]
    rec(y) = y isa AbstractVector ? foreach(rec, y) : push!(out, y)
    rec(x)
    return out
end

# C-order (row-major) flatten of a native array whose axes are in file (`dims`)
# order — matches numpy `.reshape(-1)` on xarray's `.values`. NCDatasets/Julia
# are column-major, so reverse the dims before `vec`.
_corder(a::AbstractVector) = collect(a)
_corder(a::AbstractArray) = vec(permutedims(a, reverse(1:ndims(a))))

const READER_ATOL = 1e-6
const READER_RTOL = 1e-9

# Returns `nothing` on match, else an error string (so the testset can @test it).
function cmp_native_numeric(got::AbstractArray, expected_nested)
    g = Float64[ismissing(x) ? NaN : Float64(x) for x in _corder(got)]
    e = Float64[v === nothing ? NaN : Float64(v) for v in _flat(expected_nested)]
    length(g) == length(e) || return "shape $(length(g)) != expected $(length(e))"
    gn, en = isnan.(g), isnan.(e)
    gn == en || return "NaN/fill mask mismatch"
    keep = .!gn
    if !all(isapprox.(g[keep], e[keep]; atol = READER_ATOL, rtol = READER_RTOL))
        return "value mismatch (max abs diff $(maximum(abs.(g[keep] .- e[keep]); init = 0.0)))"
    end
    return nothing
end

function cmp_native_string(got, expected_nested)
    g = String[string(x) for x in _corder(collect(got))]
    e = String[string(v) for v in _flat(expected_nested)]
    g == e || return "string mismatch $g != $e"
    return nothing
end

# The native-field schema's `dtype` ↔ the Julia element type the reader returns.
function dtype_ok(data, dt::AbstractString)
    dt == "string"  && return eltype(data) <: AbstractString
    dt == "float64" && return eltype(data) == Float64
    dt == "int32"   && return eltype(data) == Int32
    dt == "int64"   && return eltype(data) == Int64
    return true
end

# --- the decode conformance pass -------------------------------------------

@testset "format readers — decode + native-array equality (checks 3–4)" begin
    index = JSON.parsefile(joinpath(CORPUS, "cases.json"))
    @test length(index["cases"]) >= 1

    for entry in index["cases"]
        case = JSON.parsefile(joinpath(CORPUS, entry["file"]))
        id = case["id"]
        fmt = case["format"]
        blob = joinpath(CORPUS, case["blob_path"])

        @testset "$id ($fmt)" begin
            # the reader is resolved by name through the registry — dispatch is
            # the architectural seam (a new format is one register! line).
            @test haskey(FORMAT_REGISTRY, fmt)
            @test status_of(FORMAT_REGISTRY, fmt) == :active
            reader = FORMAT_REGISTRY[fmt]

            if store_backed(reader)
                # Store-backed (zarr): the reader is handed (cache, base_url;
                # variables, select) — a Zarr store is many objects, not one blob.
                zcache = Cache(LocalStore(joinpath(CORPUS, "cache")); offline = true, verify = true)
                vars = String[String(v) for v in case["variables"]]
                nds = read_store(reader, zcache, case["resolved_url"];
                                 variables = vars, select = case["select"])
            else
                kwargs = if fmt == "csv"
                    (; numeric_columns = String.(case["decode"]["numeric_columns"]))
                else
                    NamedTuple()
                end
                nds = read_native(reader, blob; kwargs...)
            end

            for (name, spec) in case["expected"]["variables"]
                @test haskey(nds, name)
                field = nds[name]
                @test dtype_ok(field.data, spec["dtype"])
                err = spec["dtype"] == "string" ?
                      cmp_native_string(field.data, spec["data"]) :
                      cmp_native_numeric(field.data, spec["data"])
                err === nothing || @info "decode mismatch" id name err
                @test err === nothing
            end

            for (name, spec) in case["expected"]["coords"]
                @test haskey(nds, name)
                field = nds[name]
                @test dtype_ok(field.data, spec["dtype"])
                @test cmp_native_numeric(field.data, spec["data"]) === nothing
                # a CF time axis stays RAW with units/calendar carried (not decoded)
                if haskey(spec, "units")
                    @test field.attrs["units"] == spec["units"]
                end
                if haskey(spec, "calendar")
                    @test field.attrs["calendar"] == spec["calendar"]
                end
            end
        end
    end
end

@testset "reader edge cases" begin
    # zarr is now active + store-backed: read_store requires an explicit variable
    # list (the store cannot be enumerated without a consolidated .zmetadata).
    @test status_of(FORMAT_REGISTRY, "zarr") == :active
    @test store_backed(FORMAT_REGISTRY["zarr"])
    @test_throws ErrorException read_store(FORMAT_REGISTRY["zarr"], Cache(; offline = true),
                                           "s3://b/z"; variables = nothing)

    # CSV inference fallback: with no numeric_columns, digit-only TEXT would be
    # mis-inferred as numeric — which is exactly why the loader must pass the
    # list. Here every value is a real number, so inference is safe.
    tmp = joinpath(mktempdir(), "nums.csv")
    write(tmp, "a,b\n1.5,2\n3.5,4\n")
    nds = read_native(CSVReader(), tmp)            # no numeric_columns => infer
    @test eltype(nds["a"].data) == Float64
    @test nds["a"].data == [1.5, 3.5]
    @test eltype(nds["b"].data) == Float64
end
