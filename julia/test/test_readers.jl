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

            kwargs = if fmt == "csv"
                (; numeric_columns = String.(case["decode"]["numeric_columns"]))
            elseif fmt == "ff10"
                (; numeric_columns = String.(case["decode"]["numeric_columns"]),
                   kind = String(get(case["decode"], "kind", "point")))
            else
                NamedTuple()
            end
            nds = read_native(reader, blob; kwargs...)

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
    # the registered zarr stub is a clear, named error — not a silent miss
    @test status_of(FORMAT_REGISTRY, "zarr") == :stub
    @test_throws ErrorException read_native(FORMAT_REGISTRY["zarr"], "x.zarr")

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

# --- FF10 point reader unit tests -------------------------------------------

# A tiny FF10 point blob: a `#` header block + 3 data rows. Two rows (NOX/SO2)
# share ONE stack (same F001/U1/R1/P1 + stack params + lon/lat), differing only
# in POLID/ANN_VALUE — the reader must NOT pivot/aggregate. Row 1 has a quoted
# FACILITY_NAME with an embedded comma and a blank DESIGN_CAPACITY (numeric->NaN).
function _ff10_fixture_text()
    cols = length(EarthSciIO.FF10_POINT_COLUMNS)
    idx = Dict(n => j for (j, n) in enumerate(EarthSciIO.FF10_POINT_COLUMNS))
    function mkrow(over)
        r = fill("", cols)
        for (k, v) in over
            r[idx[k]] = v
        end
        # FACILITY_NAME may embed a comma -> RFC-4180 quote it.
        nm = r[idx["FACILITY_NAME"]]
        occursin(',', nm) && (r[idx["FACILITY_NAME"]] = "\"" * nm * "\"")
        return join(r, ',')
    end
    stack = ["COUNTRY_CD"=>"US", "REGION_CD"=>"01001", "FACILITY_ID"=>"F001",
             "UNIT_ID"=>"U1", "REL_POINT_ID"=>"R1", "PROCESS_ID"=>"P1",
             "SCC"=>"0030700101", "FACILITY_NAME"=>"Autauga Plant, Unit 1",
             "STKHGT"=>"100.0", "STKTEMP"=>"500.0", "LONGITUDE"=>"-86.51045",
             "LATITUDE"=>"32.43878", "ZIPCODE"=>"00000"]
    lines = ["#FORMAT=FF10_POINT", "#COUNTRY US", "",
             mkrow([stack; ["POLID"=>"NOX", "ANN_VALUE"=>"123.45"]]),
             mkrow([stack; ["POLID"=>"SO2", "ANN_VALUE"=>"67.89"]]),
             mkrow(["COUNTRY_CD"=>"US", "REGION_CD"=>"01001", "FACILITY_ID"=>"F002",
                    "POLID"=>"PM25", "ANN_VALUE"=>"4.2",
                    "FACILITY_NAME"=>"Plain Name"])]
    return join(lines, '\n') * '\n'
end

@testset "FF10 reader — header/quote/empty typing" begin
    tmp = joinpath(mktempdir(), "ff10_point.csv")
    write(tmp, _ff10_fixture_text())
    nds = read_native(FF10Reader(), tmp)

    # 77 columns, all on a single `index` dim, no coords.
    @test length(nds.variables) == 77
    @test isempty(nds.coords)
    @test nds["ANN_VALUE"].dims == ["index"]

    # `#` header + blank line skipped -> exactly 3 data rows.
    @test length(nds["POLID"].data) == 3

    # numeric vs string typing.
    @test eltype(nds["ANN_VALUE"].data) == Float64
    @test nds["ANN_VALUE"].data == [123.45, 67.89, 4.2]
    @test eltype(nds["POLID"].data) <: AbstractString

    # leading-zero codes stay strings (never floats).
    @test nds["REGION_CD"].data == ["01001", "01001", "01001"]
    @test nds["SCC"].data[1] == "0030700101"
    @test nds["ZIPCODE"].data[1] == "00000"

    # quoted comma preserved verbatim (quotes stripped).
    @test nds["FACILITY_NAME"].data[1] == "Autauga Plant, Unit 1"
    @test nds["FACILITY_NAME"].data[3] == "Plain Name"

    # blank numeric cell -> NaN; blank string cell -> "".
    @test isnan(nds["DESIGN_CAPACITY"].data[1])
    @test nds["TRIBAL_CODE"].data[1] == ""

    # multi-pollutant-same-stack: rows 1 & 2 share the stack, differ in POLID/ANN.
    @test nds["FACILITY_ID"].data[1] == nds["FACILITY_ID"].data[2] == "F001"
    @test nds["STKHGT"].data[1] == nds["STKHGT"].data[2] == 100.0
    @test nds["POLID"].data[1:2] == ["NOX", "SO2"]
    @test nds["ANN_VALUE"].data[1:2] == [123.45, 67.89]
end

import ZipFile

@testset "FF10 reader — zip member extraction" begin
    dir = mktempdir()
    csvpath = joinpath(dir, "point.csv")
    text = _ff10_fixture_text()
    write(csvpath, text)
    # bare-csv decode (the conformance path).
    bare = read_native(FF10Reader(), csvpath)

    # build a zip holding the CSV as member `inv/point.csv`.
    zippath = joinpath(dir, "2016fd_inputs_point.zip")
    w = ZipFile.Writer(zippath)
    f = ZipFile.addfile(w, "inv/point.csv")
    write(f, text)
    close(w)

    zipped = read_native(FF10Reader(), zippath; member = "inv/point.csv")
    @test zipped["ANN_VALUE"].data == bare["ANN_VALUE"].data
    @test zipped["POLID"].data == bare["POLID"].data
    @test zipped["FACILITY_NAME"].data == bare["FACILITY_NAME"].data
    # a missing member is a clear error, not a silent empty.
    @test_throws ArgumentError read_native(FF10Reader(), zippath; member = "nope.csv")
end
