"""ERA5 → CDS request mapping (port of ``era5.jl``'s ``ERA5PressureLevelFileSet``).

The :mod:`earthsciio.backends.cds` transport speaks generic CDS; this module is
the **loader-specific** layer that turns ERA5 pressure-level loader fields — the
declared variable list, the spatial domain, the requested levels and the
simulation time span — into the CDS request Dict and the resolved ``cds://`` URL
the cache fetches. It ports only the *request-building* half of ``era5.jl``
(dataset id, variable list, ``pressure_level``, ``area`` from the domain bbox,
and the ``year``/``month``/``day``/``time`` fields); the NetCDF read/interp half
stays in the format reader + ESS/ESD, per the EarthSciIO boundary.

Nothing here opens a socket or reads a file — it is pure data mapping, so it is
import-safe and unit-testable offline. Hand the URL it produces to
``Cache.fetch(...)`` to actually retrieve the month.
"""

from __future__ import annotations

import calendar
import math
from datetime import datetime, timedelta, timezone
from typing import Iterable, List, Mapping, Sequence, Tuple

from .backends.cds import encode_cds_url

__all__ = [
    "ERA5_DATASET",
    "ERA5_PRESSURE_LEVELS_HPA",
    "ERA5_PLEV_TO_INDEX",
    "ERA5_VARIABLES",
    "era5_area_from_bbox",
    "era5_levels_from_indices",
    "era5_request",
    "era5_cds_url",
    "era5_months_in_span",
]

#: The CDS dataset id for ERA5 reanalysis on pressure levels.
ERA5_DATASET = "reanalysis-era5-pressure-levels"

#: ERA5 pressure levels (hPa), surface (highest pressure) → top (lowest).
#: 1-based level index 1 = 1000 hPa … index 37 = 1 hPa (matches ``era5.jl``).
ERA5_PRESSURE_LEVELS_HPA: Tuple[int, ...] = (
    1000, 975, 950, 925, 900, 875, 850, 825, 800, 775, 750,
    700, 650, 600, 550, 500, 450, 400, 350, 300,
    250, 225, 200, 175, 150, 125, 100,
    70, 50, 30, 20, 10, 7, 5, 3, 2, 1,
)

#: Reverse lookup: pressure (hPa) → 1-based level index.
ERA5_PLEV_TO_INDEX = {p: i + 1 for i, p in enumerate(ERA5_PRESSURE_LEVELS_HPA)}

#: CDS API variable name → NetCDF short name (the available pressure-level vars).
ERA5_VARIABLES = {
    "temperature": "t",
    "u_component_of_wind": "u",
    "v_component_of_wind": "v",
    "vertical_velocity": "w",
    "specific_humidity": "q",
    "relative_humidity": "r",
    "geopotential": "z",
    "divergence": "d",
    "vorticity": "vo",
    "ozone_mass_mixing_ratio": "o3",
    "fraction_of_cloud_cover": "cc",
    "specific_cloud_ice_water_content": "ciwc",
    "specific_cloud_liquid_water_content": "clwc",
    "specific_rain_water_content": "crwc",
    "specific_snow_water_content": "cswc",
    "potential_vorticity": "pv",
}

#: ``era5.jl`` pads the requested domain by ±3 h before deciding which months to
#: pull (so interpolation near a boundary has data on both sides).
_DEFAULT_BUFFER = timedelta(hours=3)


def era5_area_from_bbox(
    min_lon: float, min_lat: float, max_lon: float, max_lat: float
) -> List[int]:
    """The CDS ``area`` ``[north, west, south, east]`` for a lon/lat bbox (degrees).

    Mirrors ``era5.jl``: round *outward* with a 1° margin so the requested grid
    fully covers the domain — north/east ``ceil(+1)``, west/south ``floor(-1)``.
    Inputs are degrees; ``min_*``/``max_*`` need not be pre-ordered.
    """
    lo_lon, hi_lon = sorted((min_lon, max_lon))
    lo_lat, hi_lat = sorted((min_lat, max_lat))
    north = math.ceil(hi_lat + 1)
    west = math.floor(lo_lon - 1)
    south = math.floor(lo_lat - 1)
    east = math.ceil(hi_lon + 1)
    return [north, west, south, east]


def era5_levels_from_indices(indices: Iterable[float]) -> List[int]:
    """Map 1-based ERA5 level indices to pressures in hPa (``era5.jl`` ordering).

    The loader carries vertical coordinates as level *indices*; CDS wants
    pressures. Index 1 → 1000 hPa … index 37 → 1 hPa. Indices are rounded to the
    nearest integer (the loader may hand fractional level coordinates) and must
    fall in ``1..len(ERA5_PRESSURE_LEVELS_HPA)``.
    """
    levels: List[int] = []
    n = len(ERA5_PRESSURE_LEVELS_HPA)
    for raw in indices:
        idx = int(round(raw))
        if not 1 <= idx <= n:
            raise ValueError(f"ERA5 level index {raw!r} out of range 1..{n}")
        levels.append(ERA5_PRESSURE_LEVELS_HPA[idx - 1])
    return levels


def _normalize_variables(variables: Iterable[str]) -> List[str]:
    """Canonical (sorted, de-duplicated) CDS variable list.

    CDS treats ``variable`` as a set, so sorting it makes the resulting request —
    and therefore the cache key — independent of the caller's ordering. Unknown
    names are rejected up front so a typo fails loud instead of yielding an empty
    CDS result.
    """
    out = sorted(set(variables))
    if not out:
        raise ValueError("ERA5 request needs at least one variable")
    unknown = [v for v in out if v not in ERA5_VARIABLES]
    if unknown:
        raise ValueError(
            f"unknown ERA5 variable(s): {unknown}; valid names: "
            f"{sorted(ERA5_VARIABLES)}"
        )
    return out


def era5_request(
    year: int,
    month: int,
    days: Sequence[int],
    variables: Iterable[str],
    pressure_levels: Iterable[int],
    area: Sequence[int],
    *,
    product_type: str = "reanalysis",
    data_format: str = "netcdf",
    download_format: str = "unarchived",
) -> dict:
    """Build the CDS request Dict for one ERA5 month (port of ``era5.jl``).

    ``variables`` are CDS long names, ``pressure_levels`` hPa values (emitted as
    strings sorted high→low, as ``era5.jl`` does), ``area`` is
    ``[north, west, south, east]``, and ``time`` is the full 24 hourly steps. The
    field set and value shapes match the Julia client so Python and Julia
    retrievals hit the same CDS cache server-side.
    """
    if not days:
        raise ValueError("ERA5 request needs at least one day")
    if len(area) != 4:
        raise ValueError(f"ERA5 area must be [N, W, S, E], got {area!r}")
    levels_desc = sorted({int(p) for p in pressure_levels}, reverse=True)
    if not levels_desc:
        raise ValueError("ERA5 request needs at least one pressure level")
    # variable/pressure_level/day lists are sorted + de-duplicated so the request
    # — hence the cache key — is independent of caller order (matches the Rust
    # and Julia tracks; CDS treats each as a set).
    days_sorted = sorted({int(d) for d in days})
    return {
        "product_type": [product_type],
        "variable": _normalize_variables(variables),
        "pressure_level": [str(p) for p in levels_desc],
        "year": [str(year)],
        "month": [f"{month:02d}"],
        "day": [f"{d:02d}" for d in days_sorted],
        "time": [f"{h:02d}:00" for h in range(24)],
        "data_format": data_format,
        "download_format": download_format,
        "area": [int(a) for a in area],
    }


def era5_cds_url(
    year: int,
    month: int,
    days: Sequence[int],
    variables: Iterable[str],
    pressure_levels: Iterable[int],
    area: Sequence[int],
    **kwargs,
) -> str:
    """The resolved ``cds://`` URL for one ERA5 month (request → cache key).

    Thin composition of :func:`era5_request` and
    :func:`earthsciio.backends.cds.encode_cds_url`; hand the result to
    ``Cache.fetch(...)``. Identical loader fields yield an identical URL, so the
    cache skips a month it already holds.
    """
    request = era5_request(
        year, month, days, variables, pressure_levels, area, **kwargs
    )
    return encode_cds_url(ERA5_DATASET, request)


def era5_months_in_span(
    start: datetime,
    end: datetime,
    *,
    buffer: timedelta = _DEFAULT_BUFFER,
) -> List[Tuple[int, int, List[int]]]:
    """Months (and the days within each) covering ``[start, end]`` for ERA5.

    Returns ``[(year, month, [day, ...]), ...]`` in chronological order — the
    file partition ``era5.jl`` retrieves one request per month. The span is
    padded by ``buffer`` on each side (default ±3 h, matching the Julia loader)
    so a boundary query has data to interpolate from; pass ``buffer=timedelta(0)``
    for an exact, un-padded span (the local-mirror behavior). Naive datetimes are
    treated as UTC.
    """
    t_start = _as_utc(start) - buffer
    t_end = _as_utc(end) + buffer
    if t_end < t_start:
        raise ValueError("ERA5 span end precedes start")

    out: List[Tuple[int, int, List[int]]] = []
    year, month = t_start.year, t_start.month
    while (year, month) <= (t_end.year, t_end.month):
        last_dom = calendar.monthrange(year, month)[1]
        first_day = t_start.day if (year, month) == (t_start.year, t_start.month) else 1
        last_day = t_end.day if (year, month) == (t_end.year, t_end.month) else last_dom
        out.append((year, month, list(range(first_day, last_day + 1))))
        month += 1
        if month > 12:
            year, month = year + 1, 1
    return out


def _as_utc(t: datetime) -> datetime:
    return t if t.tzinfo is not None else t.replace(tzinfo=timezone.utc)
