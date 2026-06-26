# ERA5 pressure-level request mapping for the `cds` transport.
#
# Ports the request-building half of EarthSciData.jl's `ERA5PressureLevelFileSet`
# (era5.jl): it turns an ERA5 loader's declared fields — variables, pressure
# levels, spatial area, and the time window (year/month/day/time) — into a CDS
# `reanalysis-era5-pressure-levels` request and a content-addressable `cds://`
# URL (see [`cds_url`]). Fetching that URL through the cache dispatches the
# `cds` transport (submit/poll/download) and content-addresses the result.
#
# Only the I/O-relevant projection of the loader lives here. Variable remap and
# unit conversion stay in ESS; regrid stays in ESD — the fetched NetCDF is
# decoded by the `netcdf` reader into RAW native-grid arrays (Risk R3).

"""The CDS dataset identifier for ERA5 data on pressure levels."""
const ERA5_PL_DATASET = "reanalysis-era5-pressure-levels"

# ERA5 pressure levels in hPa, surface (highest pressure) → top (lowest).
const ERA5_PRESSURE_LEVELS_HPA = [
    1000, 975, 950, 925, 900, 875, 850, 825, 800, 775, 750,
    700, 650, 600, 550, 500, 450, 400, 350, 300,
    250, 225, 200, 175, 150, 125, 100,
    70, 50, 30, 20, 10, 7, 5, 3, 2, 1,
]

# CDS API variable name → short name as stored in the NetCDF file. The native
# reader keys arrays by the on-disk short name; remap to schema names is ESS.
const ERA5_VARIABLES = Dict(
    "temperature" => "t",
    "u_component_of_wind" => "u",
    "v_component_of_wind" => "v",
    "vertical_velocity" => "w",
    "specific_humidity" => "q",
    "relative_humidity" => "r",
    "geopotential" => "z",
    "divergence" => "d",
    "vorticity" => "vo",
    "ozone_mass_mixing_ratio" => "o3",
    "fraction_of_cloud_cover" => "cc",
    "specific_cloud_ice_water_content" => "ciwc",
    "specific_cloud_liquid_water_content" => "clwc",
    "specific_rain_water_content" => "crwc",
    "specific_snow_water_content" => "cswc",
    "potential_vorticity" => "pv",
)

"""
    era5_area(; north, west, south, east) -> [N, W, S, E]

The CDS `area` sub-tile (degrees, order North/West/South/East) for a domain's
lon/lat bounds, with the reference's ±1° integer buffer so interpolation has
edge data. Mirrors `ERA5PressureLevelFileSet`'s area computation."""
function era5_area(; north::Real, west::Real, south::Real, east::Real)
    return [ceil(Int, north + 1), floor(Int, west - 1),
            floor(Int, south - 1), ceil(Int, east + 1)]
end

_era5_day(d::Integer) = lpad(d, 2, '0')
_era5_day(d::AbstractString) = lpad(d, 2, '0')
_era5_time(h::Integer) = string(lpad(h, 2, '0'), ":00")
_era5_time(h::AbstractString) = String(h)

"""
    era5_pressure_request(year, month; kwargs...) -> Dict

Build the CDS request for one month-file of ERA5 pressure-level data, mirroring
`ERA5PressureLevelFileSet`. Keyword fields are the ERA5 loader's declared
fields:

  * `variables` — CDS variable names (default: all of [`ERA5_VARIABLES`], sorted
    so the default request is deterministic across processes/tracks).
  * `pressure_levels` — hPa levels (default: all 37); emitted sorted descending.
  * `days` — day numbers in the month (default: every day).
  * `times` — hours `0:23` or `"HH:MM"` strings (default: all 24 hours).
  * `area` — `[N, W, S, E]` sub-tile (see [`era5_area`]); omitted ⇒ global.

The result is the `Dict` of CDS request parameters; pass it to [`cds_url`] (or
use [`era5_pressure_url`]) to get the cache key / fetch URL."""
function era5_pressure_request(year::Integer, month::Integer;
        variables::AbstractVector{<:AbstractString} = sort(collect(keys(ERA5_VARIABLES))),
        pressure_levels::AbstractVector{<:Real} = ERA5_PRESSURE_LEVELS_HPA,
        days = nothing, times = nothing, area = nothing,
        product_type = ["reanalysis"],
        data_format::AbstractString = "netcdf",
        download_format::AbstractString = "unarchived")
    dys = days === nothing ?
        [lpad(d, 2, '0') for d in 1:Dates.daysinmonth(Date(year, month))] :
        [_era5_day(d) for d in days]
    tms = times === nothing ?
        [_era5_time(h) for h in 0:23] :
        [_era5_time(h) for h in times]
    req = Dict{String,Any}(
        "product_type" => collect(product_type),
        "variable" => collect(variables),
        "pressure_level" => [string(p) for p in sort(collect(pressure_levels); rev = true)],
        "year" => [string(year)],
        "month" => [lpad(month, 2, '0')],
        "day" => dys,
        "time" => tms,
        "data_format" => data_format,
        "download_format" => download_format,
    )
    area === nothing || (req["area"] = collect(area))
    return req
end

"""
    era5_pressure_url(year, month; kwargs...) -> String

The content-addressable `cds://` URL for one month-file of ERA5 pressure-level
data. `kwargs` are forwarded to [`era5_pressure_request`]. Fetch it through the
cache to dispatch the `cds` transport:

```julia
url = era5_pressure_url(2018, 11; variables = ["temperature"],
                        area = era5_area(north = 41, west = -122, south = 39, east = -120))
entry = fetch_blob(cache, url; source_loader = "era5", auth_realm = "cds")
```
"""
era5_pressure_url(year::Integer, month::Integer; kwargs...) =
    cds_url(ERA5_PL_DATASET, era5_pressure_request(year, month; kwargs...))
