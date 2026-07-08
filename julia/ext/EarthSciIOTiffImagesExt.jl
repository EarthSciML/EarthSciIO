# GeoTIFF decode backend for the `geotiff` reader (gap G3, Julia track).
#
# Loaded via `using TiffImages` (a weakdep extension): it supplies the
# more-specific `read_native(::GeoTIFFReader, path; …)` method, keeping a base
# EarthSciIO install free of the TiffImages stack — the Julia mirror of the Python
# reader's lazy `tifffile` import. The pure-Julia TiffImages handles the TIFF
# container (strips/tiles, LZW/PackBits/Deflate) + exposes the raw IFD tags; this
# extension only pulls the band pixels + the georef tags out and hands them to the
# core `_assemble_geotiff`, where the cell-center axis / nodata decode CONTRACT
# lives (shared with any future GDAL backend).
module EarthSciIOTiffImagesExt

import EarthSciIO: read_native, GeoTIFFReader, _assemble_geotiff
import TiffImages

# Raw IFD tags of the first page → Dict(tag id::Int => value). Each IFD entry is
# `id => [Tag]`; the value we want is the single tag's `.data` (a scalar / vector /
# ASCII string, e.g. ModelPixelScaleTag → `[sx, sy, sz]`, GDAL_NODATA → a String).
function _tiff_tags(img)
    ifd = TiffImages.ifds(img)
    ifd isa AbstractVector && (ifd = first(ifd))     # multi-page: georef is on page 1
    out = Dict{Int,Any}()
    for (k, v) in ifd
        tag = v isa AbstractVector ? first(v) : v
        out[Int(k)] = getfield(tag, :data)
    end
    return out
end

# Unwrap a single-field pixel wrapper down to its innermost RAW stored scalar, so the
# pixel bits reinterpret to that scalar rather than to a normalized colour value.
# TiffImages wraps a single band as `Gray{T}`, and — crucially — stores an integer
# band (e.g. LANDFIRE's S16 fuel codes) as a FIXED-POINT pixel `Gray{Q0f15}` /
# `Gray{N0f15}`, whose scalar `Float64` value is the raw code DIVIDED by 2^15. Reading
# that fixed-point scalar as-is turns fuel codes 2..99 into 6e-5..3e-3 — degenerate.
# Unwrapping to the raw `Int16`/`UInt16` storage recovers the true integer codes, while
# a genuine float band (`Gray{Float32}` elevation) unwraps to `Float32` unchanged.
function _raw_scalar_type(@nospecialize(T))
    while isbitstype(T) && fieldcount(T) == 1 && fieldtype(T, 1) <: Real && fieldtype(T, 1) !== T
        T = fieldtype(T, 1)
    end
    return T
end

# Decoded raster bands as `(height, width)` `Float64` matrices in file order
# (rows = y/lat), each pixel reinterpreted to its raw stored scalar (see above — no
# fixed-point/colour normalization). A 3-D array carries its bands on the trailing axis.
function _tiff_bands(img)
    a = collect(img)
    unwrap = m -> (R = _raw_scalar_type(eltype(m));
        R <: Real ? Float64.(reinterpret(R, m)) :
        error("unsupported GeoTIFF pixel type $(eltype(m)): the ESS loaders are " *
              "single-band LANDFIRE/USGS rasters (multi-channel colour is not decoded)."))
    if ndims(a) == 2
        return [unwrap(a)]
    elseif ndims(a) == 3
        return [unwrap(collect(@view a[:, :, s])) for s in axes(a, 3)]
    else
        error("unsupported GeoTIFF array with ndims=$(ndims(a))")
    end
end

function read_native(::GeoTIFFReader, path::AbstractString;
                     variables = nothing, band_names = nothing, _...)
    img = TiffImages.load(String(path))
    return _assemble_geotiff(_tiff_bands(img), _tiff_tags(img);
                             variables = variables, band_names = band_names)
end

end # module