# Blosc decode backend for the `zarr` reader (Julia track).
#
# Loaded via `using Blosc` (a weakdep extension): it supplies the more-specific
# `_blosc_decompress(raw::AbstractVector{UInt8})` method, keeping a base EarthSciIO
# install free of the Blosc stack — the Julia mirror of the Python reader's lazy
# `numcodecs` import and the GeoTIFF `TiffImages` weakdep. Blosc.jl bundles
# c-blosc, so `Blosc.decompress` transparently handles lz4/zstd/zlib/blosclz AND
# undoes the shuffle filter and multi-block containers.
module EarthSciIOBloscExt

import EarthSciIO: _blosc_decompress
import Blosc

# More specific than the untyped `_blosc_decompress(raw)` fallback in zarr.jl, so
# it wins by dispatch when `using Blosc` is active (no method overwrite, which
# precompilation forbids). `Blosc.decompress(UInt8, buf)` returns the raw
# uncompressed bytes (shuffle + codec undone) for the reader to reinterpret.
_blosc_decompress(raw::AbstractVector{UInt8}) =
    Blosc.decompress(UInt8, Vector{UInt8}(raw))

end # module
