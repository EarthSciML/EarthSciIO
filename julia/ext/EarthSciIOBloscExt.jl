# Blosc decode backend for the `zarr` reader (Julia track).
#
# Loaded via `using Blosc` (a weakdep extension): it supplies the more-specific
# `_blosc_decompress(raw::AbstractVector{UInt8})` method, keeping a base EarthSciIO
# install free of the Blosc stack — the Julia mirror of the Python reader's lazy
# `numcodecs` import and the GeoTIFF `TiffImages` weakdep. Blosc.jl bundles
# c-blosc, so `Blosc.decompress` transparently handles lz4/zstd/zlib/blosclz AND
# undoes the shuffle filter and multi-block containers.
module EarthSciIOBloscExt

import EarthSciIO: _blosc_decompress, _blosc_compress
import Blosc

# More specific than the untyped `_blosc_decompress(raw)` fallback in zarr.jl, so
# it wins by dispatch when `using Blosc` is active (no method overwrite, which
# precompilation forbids). `Blosc.decompress(UInt8, buf)` returns the raw
# uncompressed bytes (shuffle + codec undone) for the reader to reinterpret.
_blosc_decompress(raw::AbstractVector{UInt8}) =
    Blosc.decompress(UInt8, Vector{UInt8}(raw))

# The WRITE mirror: the more-specific `_blosc_compress` for the zarr WRITER (the
# `blosc` inner codec of the sharding pipeline). `bytes` are the inner chunk's
# little-endian element bytes; `typesize` is the element byte size so c-blosc
# applies the byte-shuffle filter over whole elements. The blosc container is
# self-describing, so `_blosc_decompress` (and numcodecs on the Python side)
# undoes shuffle + codec from the header — no metadata dependency.
function _blosc_compress(bytes::AbstractVector{UInt8}, cname::AbstractString,
                         clevel::Integer, shuffle::Bool, typesize::Integer)
    Blosc.set_compressor(String(cname))
    return Blosc.compress(Vector{UInt8}(bytes);
                          level = Int(clevel), shuffle = shuffle,
                          itemsize = Int(typesize))
end

end # module
