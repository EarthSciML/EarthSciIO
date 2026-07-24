# Plain (non-Blosc) **zstd** codec backend for the `zarr` reader + writer.
#
# Loaded via `using CodecZstd` (a weakdep extension, exactly like the Blosc one in
# `EarthSciIOBloscExt.jl`): it supplies the more-specific `_zstd_decompress` /
# `_zstd_compress` methods, keeping a base EarthSciIO install free of the zstd
# stack.
#
# Why a PLAIN zstd codec at all, when Blosc already compresses with zstd? Because
# the Blosc *container* is what a WebAssembly/browser Zarr reader cannot decode:
# the `zarrs` crate's blosc support comes from `blosc-src`, whose vendored C
# sources do not build for `wasm32-unknown-unknown`, while the standard Zarr v3
# `zstd` codec is pure Rust there. The writer's `:wasm` profile therefore emits
# `[bytes(little), zstd]` inner chunks instead of `[bytes(little), blosc]`, and
# this extension is the Julia track's encode/decode for that chain.
module EarthSciIOZstdExt

import EarthSciIO: _zstd_compress, _zstd_decompress
import CodecZstd

# `transcode` is a Base generic that TranscodingStreams (a CodecZstd dependency)
# extends for Codec types, so it needs no import of its own here.

# More specific than the untyped fallbacks in `zarr.jl` / `zarr_write.jl`, so they
# win by dispatch when `using CodecZstd` is active (no method overwrite, which
# precompilation forbids).

# Decode one standard zstd FRAME (what the v3 `zstd` codec stores per inner chunk)
# back to the raw little-endian element bytes for the reader to reinterpret.
_zstd_decompress(raw::AbstractVector{UInt8}) =
    transcode(CodecZstd.ZstdDecompressor, Vector{UInt8}(raw))

# The WRITE mirror: compress one inner chunk's little-endian element bytes into a
# single standard zstd frame at `level`. `ZstdFrameCompressor` emits ONE complete
# frame (`ZSTD_e_end`), which is what zarr-python's `ZstdCodec` and the `zarrs`
# crate's zstd codec both expect and produce.
_zstd_compress(bytes::AbstractVector{UInt8}, level::Integer) =
    transcode(CodecZstd.ZstdFrameCompressor(; level = Int(level)),
              Vector{UInt8}(bytes))

end # module
