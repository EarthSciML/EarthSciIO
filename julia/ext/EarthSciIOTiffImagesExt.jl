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

# Decoded raster bands as `(height, width)` `Float64` matrices in file order
# (rows = y/lat). TiffImages returns a single-channel raster as a `Gray{T}` matrix
# (one isbits field of the stored scalar `T`); reinterpret to that scalar so a raw
# integer/float pixel is preserved (no color normalization). A 3-D array carries
# its bands on the trailing sample axis.
function _tiff_bands(img)
    a = collect(img)
    if ndims(a) == 2
        T = eltype(a)
        if T <: Real
            return [Array{Float64}(a)]
        elseif isbitstype(T) && fieldcount(T) == 1 && fieldtype(T, 1) <: Real
            return [Float64.(reinterpret(fieldtype(T, 1), a))]
        else
            error("unsupported GeoTIFF pixel type $T: the ESS loaders are single-band " *
                  "LANDFIRE/USGS rasters (multi-channel colour is not decoded).")
        end
    elseif ndims(a) == 3
        return [Float64.(@view a[:, :, s]) for s in axes(a, 3)]
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