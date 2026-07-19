# Julia track's native-array dumper for the cross-language conformance harness.
#
# Drives the **Julia Provider** (`EarthSciIO.const_provider`) over every committed
# corpus case, fully OFFLINE (the cache is rooted at the corpus and refuses the
# network), and emits the decoded native arrays as a canonical JSON dump in the
# SAME schema as the Python (`dump_python.py`) and Rust
# (`rust/examples/conformance_dump.rs`) dumpers. The cross-language comparator
# (`conformance/crosscheck.py`) diffs the three dumps + the corpus oracle to prove
# native-array equality across all three tracks (`esio-9nb.9`).
#
# Dump schema — `earthsciio/native-dump/v1` (see `conformance/CROSSLANG.md`).
# `data` is flattened **row-major (C order)** per `shape`; NCDatasets/Julia store
# column-major, so each array is permuted to file order before a C-order `vec`
# (mirrors `julia/test/test_readers.jl` `_corder`). A masked / `_FillValue` cell
# is `null` (== NaN); strings are emitted verbatim. A case whose `format` has no
# active reader in this track is `status="skipped"` (explicit, never dropped).
#
# Usage:  julia --project=julia conformance/dumpers/dump_julia.jl [out.json]

using EarthSciIO
import JSON

# The store-backed `zarr` case decodes blosc chunks via the `EarthSciIOBloscExt`
# weakdep extension (`using Blosc`). Blosc is kept a weakdep (light base install,
# mirroring TiffImages), so it is not importable under `--project=julia`; add it
# to a temporary environment stacked on LOAD_PATH and retry the extension load.
# In an env that already carries Blosc (e.g. the test target) the direct import
# succeeds and this is a no-op. Requires network only if Blosc is not yet in the
# depot.
if Base.get_extension(EarthSciIO, :EarthSciIOBloscExt) === nothing
    try
        @eval import Blosc
    catch
        import Pkg
        _juliaproj = normpath(joinpath(@__DIR__, "..", "..", "julia"))
        _bloscenv = mktempdir()
        Pkg.activate(_bloscenv; io = devnull)
        Pkg.add("Blosc"; io = devnull)
        Pkg.activate(_juliaproj; io = devnull)
        push!(LOAD_PATH, _bloscenv)
        @eval import Blosc
    end
    Base.retry_load_extensions()
end

# Row-major (C-order) flatten of a native array whose axes are in file (`dims`)
# order — matches numpy `.reshape(-1)` on the Python track's arrays.
_corder(a::AbstractVector) = collect(a)
_corder(a::AbstractArray) = vec(permutedims(a, reverse(1:ndims(a))))

# Encode one NativeField to the dump schema (dtype/dims/shape/data).
function encode_field(field)
    data = field.data
    dims = collect(String.(field.dims))
    if eltype(data) <: AbstractString
        vals = Any[String(x) for x in data]
        return Dict("dtype" => "string", "dims" => dims,
                    "shape" => [length(vals)], "data" => vals)
    end
    flat = _corder(data)
    et = eltype(data)
    if et <: AbstractFloat
        dtype = "float64"
        vals = Any[isnan(x) ? nothing : Float64(x) for x in flat]
    elseif et <: Integer
        dtype = et == Int32 ? "int32" : "int64"
        vals = Any[Int(x) for x in flat]
    else
        error("unexpected numeric eltype $et in field with dims $dims")
    end
    return Dict("dtype" => dtype, "dims" => dims,
                "shape" => collect(Int, size(data)), "data" => vals)
end

# A coord is a field plus the CF units/calendar it carries (if any).
function encode_coord(field)
    enc = encode_field(field)
    for k in ("units", "calendar")
        haskey(field.attrs, k) && (enc[k] = String(field.attrs[k]))
    end
    return enc
end

# Run the Julia Provider over one corpus case and encode its native arrays. Skips
# (without error) a case whose format has no active reader, matching the Rust
# track (netcdf only) so the harness reports the gap instead of failing.
function dump_case(corpus, case)
    fmt = String(case["format"])
    if !haskey(FORMAT_REGISTRY, fmt) || status_of(FORMAT_REGISTRY, fmt) != :active
        return Dict("format" => fmt, "status" => "skipped",
                    "reason" => "no active reader registered for format '$fmt' in the Julia track")
    end
    # An OFFLINE cache rooted at the corpus: each case resolves from disk by its
    # sha256(resolved_url) key; verify=true checks the blob against its manifest.
    cache = Cache(LocalStore(joinpath(corpus, "cache")); offline = true, verify = true)
    url = String(case["resolved_url"])
    provider = if fmt == "csv"
        # numeric_columns is REQUIRED (digit-only text like location_id must stay
        # a string); the corpus case pins the list.
        nc = String.(case["decode"]["numeric_columns"])
        const_provider(cache, url; format = fmt, reader_kwargs = (; numeric_columns = nc))
    elseif fmt == "ff10"
        # FF10 point: the case pins the 42 numeric columns + schema kind; member is
        # nothing (the committed fixture is the already-extracted CSV member).
        nc = String.(case["decode"]["numeric_columns"])
        kind = String(get(case["decode"], "kind", "point"))
        const_provider(cache, url; format = fmt,
                       reader_kwargs = (; numeric_columns = nc, kind = kind, member = nothing))
    elseif fmt == "zarr"
        # Store-backed: `url` is the store base; `variables` names the arrays (no
        # .zmetadata to enumerate); `select` (the orthogonal selection) rides in
        # reader_kwargs and drives lazy chunk fetch.
        vars = String[String(v) for v in case["variables"]]
        const_provider(cache, url; format = fmt, variables = vars,
                       reader_kwargs = (; select = case["select"]))
    else
        const_provider(cache, url; format = fmt)
    end
    nds = materialize(provider)  # CONST: read the single corpus blob once
    return Dict(
        "format" => fmt, "status" => "decoded",
        "variables" => Dict(n => encode_field(f) for (n, f) in nds.variables),
        "coords" => Dict(n => encode_coord(f) for (n, f) in nds.coords),
    )
end

function main()
    corpus = normpath(joinpath(@__DIR__, "..", "corpus"))
    index = JSON.parsefile(joinpath(corpus, "cases.json"))
    cases = Dict{String,Any}()
    for entry in index["cases"]
        case = JSON.parsefile(joinpath(corpus, entry["file"]))
        cases[String(case["id"])] = dump_case(corpus, case)
    end
    active = sort!([n for n in registered_names(FORMAT_REGISTRY)
                    if status_of(FORMAT_REGISTRY, n) == :active])
    out = Dict(
        "schema" => "earthsciio/native-dump/v1",
        "language" => "julia",
        "provider" => "EarthSciIO.const_provider",
        "readers" => active,
        "cases" => cases,
    )
    text = JSON.json(out, 2)
    if !isempty(ARGS)
        open(ARGS[1], "w") do io
            write(io, text)
            write(io, "\n")
        end
    else
        println(text)
    end
end

main()
