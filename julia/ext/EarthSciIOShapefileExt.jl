# Shapefile decode backend for the `shapefile` reader (Julia track).
#
# Loaded via `using Shapefile` (a weakdep extension): it supplies the
# more-specific `read_native(::ShapefileReader, path; …)` method, keeping a base
# EarthSciIO install free of the Shapefile.jl stack — the Julia mirror of the
# Python reader's lazy `pyshp` import. The pure-Julia Shapefile.jl handles the
# `.shp` geometry container and (through DBFTables.jl) the `.dbf` attribute
# table; this extension only pulls the parts, the stored per-record bounding
# boxes and the attribute columns out and hands them to the core
# `_assemble_shapefile`, where the one-row-per-part / padding / dtype CONTRACT
# lives (shared with any future backend).
module EarthSciIOShapefileExt

import EarthSciIO: read_native, ShapefileReader, _assemble_shapefile, _shapefile_members
import Shapefile

# One shape's vertex rings/parts, in file order, as `(x, y)` tuples. A
# Polygon/Polyline carries explicit 0-based part offsets; a Point or MultiPoint
# has none and is ONE part (a MultiPoint's points stay together). A Null shape
# (`missing` in Shapefile.jl) is one EMPTY part, so a null row still occupies its
# slot in the record axis.
function _parts(shape)
    shape === missing && return [Tuple{Float64,Float64}[]]
    if hasproperty(shape, :points)
        pts = Tuple{Float64,Float64}[(Float64(p.x), Float64(p.y)) for p in shape.points]
        hasproperty(shape, :parts) || return [pts]
        offs = Int[Int(o) for o in shape.parts]
        isempty(offs) && return [pts]
        bounds = vcat(offs, length(pts))
        return [pts[(bounds[k] + 1):bounds[k + 1]] for k in 1:length(offs)]
    end
    return [Tuple{Float64,Float64}[(Float64(shape.x), Float64(shape.y))]]
end

# A shape's bounding box: the record's own stored `MBR` where the format has one,
# else (Point) the point itself. `NaN`s for a Null shape.
function _box(shape)
    shape === missing && return (NaN, NaN, NaN, NaN)
    if hasproperty(shape, :MBR)
        r = shape.MBR
        return (Float64(r.left), Float64(r.bottom), Float64(r.right), Float64(r.top))
    end
    x, y = Float64(shape.x), Float64(shape.y)
    return (x, y, x, y)
end

function read_native(::ShapefileReader, path::AbstractString;
                     member = nothing, variables = nothing,
                     numeric_columns = nothing, nvert_max = nothing, kwargs...)
    blobs = _shapefile_members(path, member)
    shp = haskey(blobs, "shx") ?
          read(IOBuffer(blobs["shp"]), Shapefile.Handle,
               read(IOBuffer(blobs["shx"]), Shapefile.IndexHandle)) :
          read(IOBuffer(blobs["shp"]), Shapefile.Handle)

    parts = [_parts(s) for s in shp.shapes]
    boxes = [_box(s) for s in shp.shapes]

    colnames = String[]
    colvalues = Any[]
    deleted = fill(false, length(shp.shapes))
    if haskey(blobs, "dbf")
        dbf = Shapefile.DBFTables.Table(IOBuffer(blobs["dbf"]))
        length(dbf) == length(shp.shapes) || error(
            "shapefile has $(length(shp.shapes)) shapes but $(length(dbf)) .dbf rows")
        deleted = collect(Shapefile.DBFTables.isdeleted(dbf))
        for name in propertynames(dbf)
            push!(colnames, String(name))
            push!(colvalues, collect(getproperty(dbf, name)))
        end
    end
    crs = haskey(blobs, "prj") ? String(copy(blobs["prj"])) : nothing

    return _assemble_shapefile(shp.header.shapecode, parts, boxes, colnames, colvalues,
                               deleted, crs; variables = variables,
                               numeric_columns = numeric_columns,
                               nvert_max = nvert_max)
end

end
