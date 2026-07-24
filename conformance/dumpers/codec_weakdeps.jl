# Shared weakdep loader for the Julia conformance dumpers.
#
# EarthSciIO keeps its chunk codecs in weakdep EXTENSIONS so a base install stays
# light (the same culture as the Python reader's lazy `numcodecs` import):
#
#   * `Blosc`     -> `EarthSciIOBloscExt` — the Blosc(zstd)+shuffle container used
#                    by the `:diagnostic` / `:checkpoint` output profiles.
#   * `CodecZstd` -> `EarthSciIOZstdExt`  — the PLAIN Zarr v3 `zstd` codec used by
#                    the `:wasm` output profile. That profile exists because a
#                    WebAssembly/browser Zarr reader cannot decode the Blosc
#                    container (`zarrs`' blosc support comes from `blosc-src`,
#                    whose vendored C sources don't build for
#                    `wasm32-unknown-unknown`), while the standard v3 `zstd` codec
#                    is pure Rust there.
#
# Both are loaded here so ONE write/read driver covers every codec profile. The
# strategy per package mirrors what the dumpers have always done for Blosc: try a
# direct import (it works when the package is already resolvable, e.g. via a
# stacked env or the test target); on failure stack a temp env carrying it onto
# LOAD_PATH and retry. `Base.retry_load_extensions()` then activates the
# extensions.

function _load_weakdep!(name::AbstractString)
    try
        @eval import $(Symbol(name))
        return true
    catch
        try
            juliaproj = normpath(joinpath(@__DIR__, "..", "..", "julia"))
            env = mktempdir()
            # Each statement gets its OWN `@eval` so it runs in the latest world
            # age: under Julia >= 1.12 touching the `Pkg` binding in the same
            # block that imported it is a prior-world access (a hard error in
            # future versions, a loud warning today).
            @eval import Pkg
            @eval Pkg.activate($env; io = devnull)
            @eval Pkg.add($name; io = devnull)
            @eval Pkg.activate($juliaproj; io = devnull)
            push!(LOAD_PATH, env)
            @eval import $(Symbol(name))
            return true
        catch err
            @warn "could not load codec weakdep $name; stores using it will fail" err
            return false
        end
    end
end

"""
    load_codec_weakdeps!()

Load the `Blosc` and `CodecZstd` weakdeps and activate the corresponding
EarthSciIO extensions, so this process can encode/decode BOTH the Blosc-based
(`:diagnostic`/`:checkpoint`) and the plain-zstd (`:wasm`) codec profiles.
"""
function load_codec_weakdeps!()
    if Base.get_extension(EarthSciIO, :EarthSciIOBloscExt) === nothing
        _load_weakdep!("Blosc")
    end
    if Base.get_extension(EarthSciIO, :EarthSciIOZstdExt) === nothing
        _load_weakdep!("CodecZstd")
    end
    Base.retry_load_extensions()
    return nothing
end
