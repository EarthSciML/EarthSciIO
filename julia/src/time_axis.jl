# Finding a record by the file's own time axis (spec/esm-sources.md §2.5).
#
# A `temporal` provider locates a record in its file by the file's decoded CF
# time axis when it has one, and by the cadence offset (§2.3) otherwise. That is
# what the Python track does (`provider.py` `_record_index`, decoding through
# xarray's `decode_cf_datetime`), and this file reproduces its decode rule by
# rule, so a trimmed or irregular file gives the same record in every track. The
# shared fixture is `conformance/corpus/scenarios/decoded-time-axis.json`.

# Units xarray decodes through pandas, after lower-casing and adding a missing
# final "s": the value is truncated to whole nanoseconds.
const _TA_PANDAS_UNITS = Dict{String, Int64}(
    "nanoseconds" => 1, "microseconds" => 1_000, "milliseconds" => 1_000_000,
    "seconds" => 1_000_000_000, "minutes" => 60_000_000_000,
    "hours" => 3_600_000_000_000, "days" => 86_400_000_000_000
)

# Units cftime decodes (xarray's fallback, also for the spellings pandas does
# not know): the value is rounded to the nearest microsecond.
const _TA_CFTIME_UNITS = Dict{String, Int64}(
    "microsecond" => 1_000, "microseconds" => 1_000,
    "millisecond" => 1_000_000, "milliseconds" => 1_000_000,
    "msec" => 1_000_000, "msecs" => 1_000_000, "ms" => 1_000_000,
    "second" => 1_000_000_000, "seconds" => 1_000_000_000,
    "sec" => 1_000_000_000, "secs" => 1_000_000_000, "s" => 1_000_000_000,
    "minute" => 60_000_000_000, "minutes" => 60_000_000_000,
    "min" => 60_000_000_000, "mins" => 60_000_000_000,
    "hour" => 3_600_000_000_000, "hours" => 3_600_000_000_000,
    "hr" => 3_600_000_000_000, "hrs" => 3_600_000_000_000, "h" => 3_600_000_000_000,
    "day" => 86_400_000_000_000, "days" => 86_400_000_000_000, "d" => 86_400_000_000_000
)

const _TA_STANDARD_CALENDARS = ("standard", "gregorian", "proleptic_gregorian")
const _TA_NS = Int128(1_000_000_000)

# Days from 1970-01-01 to the proleptic Gregorian date y-m-d, or `nothing`.
function _ta_gregorian_days(y::Integer, m::Integer, d::Integer)
    (1 <= m <= 12 && 1 <= d <= Dates.daysinmonth(y, m)) || return nothing
    return Dates.value(Date(y, m, d)) - _TA_UNIX_DAY
end

# Days from 1970-01-01 to the date y-m-d of the Julian calendar (year 1 or
# later), or `nothing`. Dates has no Julian calendar, so this is the usual
# Julian Day Number arithmetic.
function _ta_julian_days(y::Integer, m::Integer, d::Integer)
    1 <= m <= 12 || return nothing
    len = m == 2 ? (y % 4 == 0 ? 29 : 28) : (m in (4, 6, 9, 11) ? 30 : 31)
    (y >= 1 && 1 <= d <= len) || return nothing
    a = div(14 - m, 12)
    yy = Int64(y) + 4800 - a
    mm = m + 12a - 3
    return d + div(153mm + 2, 5) + 365yy + div(yy, 4) - 32083 - _TA_UNIX_JD
end

const _TA_UNIX_DAY = Dates.value(Date(1970))   # Rata Die of 1970-01-01
const _TA_UNIX_JD = 2_440_588                  # its Julian Day Number

# Read 1 to `maxn` ASCII digits of `s` from byte `i`; (value, ndigits, next i).
function _ta_digits(s::String, i::Int, maxn::Int)
    v = 0
    n = 0
    while i <= ncodeunits(s) && n < maxn && isdigit(Char(codeunit(s, i)))
        v = 10v + (codeunit(s, i) - UInt8('0'))
        n += 1
        i += 1
    end
    return (v, n, i)
end

_ta_at(s::String, i::Int, c::Char) = i <= ncodeunits(s) && Char(codeunit(s, i)) == c
_ta_space(s::String, i::Int) = i <= ncodeunits(s) && codeunit(s, i) in (UInt8(' '), UInt8('\t'))

# A CF reference date in the forms spec/esm-sources.md §2.5 lists: `Y-M-D` (a
# year of one to four digits), then optionally (`T` or spaces) a time `HH`,
# `H:M`, `H:M:S` or `H:M:S.fff…`, then optionally (after spaces) `Z`, `UTC`,
# `GMT` or an offset `±HH`, `±HHMM`, `±HH:MM`. The date itself is checked by
# whichever reading uses it (`_ta_pandas_ns`, `_ta_cftime_ns`), since the two
# read it on different calendars.
struct _TARef
    y::Int
    ydigits::Int
    mo::Int
    d::Int
    hh::Int
    mi::Int
    ss::Int
    frac::Int128        # nanoseconds
    clock::Symbol       # :none, :hour (a bare `HH`), or :minutes
    clock_gap::Int      # characters between the date and the time (`T` is one)
    off::Int            # seconds east of UTC
    zone::Bool          # spelled `Z` or `±…` (not `UTC`/`GMT`, not absent)
    zone_gap::Int       # spaces before the zone
end

function _ta_reference(s::AbstractString)
    s = String(s)
    all(isascii, s) || return nothing
    y, ydigits, i = _ta_digits(s, 1, 4)
    ydigits >= 1 && _ta_at(s, i, '-') || return nothing
    # xarray pads a short year to four digits only when the reference has no
    # four-digit run elsewhere; otherwise pandas reads it its own way.
    ydigits < 4 && occursin(r"\d{4}", s) && return nothing
    mo, n, i = _ta_digits(s, i + 1, 2)
    n >= 1 && _ta_at(s, i, '-') || return nothing
    d, n, i = _ta_digits(s, i + 1, 2)
    n >= 1 || return nothing
    hh = mi = ss = 0
    frac = Int128(0)
    clock = :none
    j = i
    if _ta_at(s, j, 'T')
        j += 1
    else
        while _ta_space(s, j)
            j += 1
        end
    end
    clock_gap = j - i
    if j <= ncodeunits(s) && isdigit(Char(codeunit(s, j)))
        hh, n, j = _ta_digits(s, j, 2)
        if _ta_at(s, j, ':')
            clock = :minutes
            mi, n, j = _ta_digits(s, j + 1, 2)
            n >= 1 || return nothing
            if _ta_at(s, j, ':')
                ss, n, j = _ta_digits(s, j + 1, 2)
                n >= 1 || return nothing
                if _ta_at(s, j, '.')
                    k = j + 1
                    nd = 0
                    while k <= ncodeunits(s) && isdigit(Char(codeunit(s, k)))
                        nd < 9 && (frac = 10frac + (codeunit(s, k) - UInt8('0')))
                        nd += 1
                        k += 1
                    end
                    nd >= 1 || return nothing
                    frac *= Int128(10)^max(0, 9 - nd)
                    j = k
                end
            end
        elseif n != 2
            return nothing   # a bare hour must have two digits
        else
            clock = :hour
        end
        i = j
    else
        clock_gap = 0
    end
    zone_gap = 0
    while _ta_space(s, i)
        i += 1
        zone_gap += 1
    end
    off = 0
    zone = false
    rest = s[i:end]
    if isempty(rest) || lowercase(rest) in ("utc", "gmt")
        off = 0
    elseif lowercase(rest) == "z"
        zone = true
    elseif rest[1] in ('+', '-')
        sign = rest[1] == '+' ? 1 : -1
        oh, n, k = _ta_digits(rest, 2, 2)
        n == 2 || return nothing
        om = 0
        if k <= ncodeunits(rest)
            _ta_at(rest, k, ':') && (k += 1)
            om, n, k = _ta_digits(rest, k, 2)
            n == 2 && k > ncodeunits(rest) || return nothing
        end
        (oh <= 23 && om <= 59) || return nothing
        off = sign * (oh * 3600 + om * 60)
        zone = true
    else
        return nothing
    end
    (hh <= 23 && mi <= 59 && ss <= 59) || return nothing
    return _TARef(y, ydigits, mo, d, hh, mi, ss, frac, clock, clock_gap, off, zone, zone_gap)
end

# The reference as pandas reads it, in epoch nanoseconds, or `nothing`: a
# four-digit year on the proleptic Gregorian calendar, every part of the time
# and zone applied.
function _ta_pandas_ns(r::_TARef)
    r.ydigits == 4 || return nothing
    days = _ta_gregorian_days(r.y, r.mo, r.d)
    days === nothing && return nothing
    return (Int128(days) * 86400 + r.hh * 3600 + r.mi * 60 + r.ss - r.off) * _TA_NS + r.frac
end

# The reference as cftime reads it, in epoch nanoseconds, or `nothing`. On the
# `standard`/`gregorian` calendar a date before 1582-10-15 is a Julian date (and
# 1582-10-05 to 10-14 do not exist); `proleptic_gregorian` is Gregorian
# throughout. cftime reads a time only with minutes and right after the date (one
# separator), and a `Z`/`±` zone only right after that (at most one space);
# otherwise it stops reading, so the rest counts as midnight UTC. Fractions are
# cut to the microsecond.
function _ta_cftime_ns(r::_TARef, proleptic::Bool)
    ymd = (r.y, r.mo, r.d)
    days = if proleptic || ymd >= (1582, 10, 15)
        _ta_gregorian_days(r.y, r.mo, r.d)
    elseif ymd > (1582, 10, 4)
        nothing   # the days the reform skipped
    else
        _ta_julian_days(r.y, r.mo, r.d)
    end
    days === nothing && return nothing
    clock = r.clock === :minutes && r.clock_gap == 1
    zone = r.zone && r.zone_gap <= 1 && (clock || r.clock === :none)
    hh, mi, ss, frac = clock ? (r.hh, r.mi, r.ss, fld(r.frac, 1000) * 1000) : (0, 0, 0, Int128(0))
    off = zone ? r.off : 0
    return (Int128(days) * 86400 + hh * 3600 + mi * 60 + ss - off) * _TA_NS + frac
end

# Split "<unit> since <reference>" as Python's `re.match(r"(.+) since (.+)")`
# does: the last " since " with something before and after it.
function _ta_split_units(units::String)
    occursin('\n', units) && return nothing
    r = findlast(" since ", units)
    while r !== nothing
        if first(r) > 1 && last(r) < ncodeunits(units)
            return (strip(units[1:prevind(units, first(r))]), strip(units[nextind(units, last(r)):end]))
        end
        r = first(r) > 1 ? findprev(" since ", units, prevind(units, first(r))) : nothing
    end
    return nothing
end

_ta_floor_s(ns::Int128) = Int64(fld(ns, _TA_NS))
function _ta_year_ok(ns::Int128)
    days = fld(ns, 86400 * _TA_NS)
    abs(days) < 10_000_000 || return false
    return 1678 <= Dates.year(Date(Dates.UTD(Int64(days) + _TA_UNIX_DAY))) <= 2261
end
const _TA_I64 = (Int128(typemin(Int64)) + 1, Int128(typemax(Int64)))
_ta_in_i64(x::Int128) = _TA_I64[1] <= x <= _TA_I64[2]

"""
    decode_time_axis(values, units, calendar = nothing; fill = nothing)
        -> Union{Nothing, Vector{Int64}}
    decode_time_axis(coord::NativeField) -> Union{Nothing, Vector{Int64}}

The Unix seconds (UTC, floored to the second) of a raw CF time axis, or
`nothing` when it is not decoded (spec/esm-sources.md §2.5). A `temporal`
[`Provider`] uses this to find a record in a file by the file's own time axis,
and falls back to the cadence offset when it gets `nothing`: no `units`, units
without `since`, a calendar other than `standard`, `gregorian` or
`proleptic_gregorian`, a NaN or `fill` value, or a date that is not
representable (outside 1677-09-21 to 2262-04-11 on the first reading, outside
the years 1678 to 2261 on the second). The decode is the Python track's
(xarray's), rule by rule. The `NativeField` form reads `units`, `calendar`,
`_FillValue` and `missing_value` from the field's attributes, and applies a
`scale_factor`/`add_offset` after the fill check, as the Python reader does."""
function decode_time_axis(values, units, calendar = nothing; fill = nothing)
    units === nothing && return nothing
    split = _ta_split_units(String(units))
    split === nothing && return nothing
    unit = lowercase(split[1])
    cal = calendar === nothing ? "standard" : lowercase(String(calendar))
    cal in _TA_STANDARD_CALENDARS || return nothing
    ref = _ta_reference(split[2])
    ref === nothing && return nothing
    vals = vec(collect(values))
    fills = fill === nothing ? () : fill
    for v in vals
        (v isa Real) || return nothing
        v isa AbstractFloat && !isfinite(v) && return nothing
        any(f -> v == f, fills) && return nothing
    end
    # pandas: truncate to the nanosecond, while the reference and every time fit
    # in int64 nanoseconds
    punit = endswith(unit, "s") ? unit : unit * "s"
    pf = get(_TA_PANDAS_UNITS, punit, nothing)
    pref = _ta_pandas_ns(ref)
    if pf !== nothing && pref !== nothing && _ta_in_i64(pref)
        out = Int64[]
        ok = true
        for v in vals
            if v isa Integer
                ns = Int128(v) * pf
            else
                p = Float64(v) * Float64(pf)
                abs(p) < 9.2e18 || (ok = false; break)
                ns = Int128(trunc(p))
            end
            t = pref + ns
            (_ta_in_i64(ns) && _ta_in_i64(t)) || (ok = false; break)
            push!(out, _ta_floor_s(t))
        end
        ok && return out
    end
    # cftime: its own reading of the reference, each value rounded to the
    # microsecond
    cf = get(_TA_CFTIME_UNITS, unit, nothing)
    cf === nothing && return nothing
    cref = _ta_cftime_ns(ref, cal == "proleptic_gregorian")
    cref === nothing && return nothing
    out = Int64[]
    for v in vals
        us = Float64(v) * (Float64(cf) / 1000)
        abs(us) < 1.0e30 || return nothing
        t = cref + Int128(round(us, RoundNearestTiesAway)) * 1000
        _ta_year_ok(t) || return nothing
        push!(out, _ta_floor_s(t))
    end
    return out
end

_ta_attr(x::AbstractArray) = Float64(first(x))
_ta_attr(x) = Float64(x)

function decode_time_axis(coord::NativeField)
    a = coord.attrs
    fill = Any[a[k] for k in ("_FillValue", "missing_value") if haskey(a, k)]
    fill = isempty(fill) ? nothing : reduce(vcat, fill)
    data = coord.data
    if haskey(a, "scale_factor") || haskey(a, "add_offset")
        # CF unpacking, after the fill check on the stored values
        # (spec/conformance.md §3): the axis the Python reader hands its decode
        if fill !== nothing && any(v -> any(==(v), fill), data)
            return nothing
        end
        sf = haskey(a, "scale_factor") ? _ta_attr(a["scale_factor"]) : 1.0
        ao = haskey(a, "add_offset") ? _ta_attr(a["add_offset"]) : 0.0
        data = Float64.(data) .* sf .+ ao
        fill = nothing
    end
    return decode_time_axis(
        data, get(a, "units", nothing), get(a, "calendar", nothing); fill = fill
    )
end

# The 0-based record of `anchor` in the file covering it (file anchor `fa`),
# given that file's decoded time axis (`decode_time_axis`; `nothing` when it has
# none or it does not decode). By the axis when the anchor lies within its span —
# the latest record at or before the anchor, the first such in file order on a
# tie — else the cadence offset (spec/esm-sources.md §2.5; Python
# `provider._record_index`).
function _axis_record(temporal::SourceTemporal, times, anchor::DateTime, fa::DateTime)
    if times !== nothing && !isempty(times)
        a = Int64(fld(Dates.value(anchor - DateTime(1970)), 1000))
        if minimum(times) <= a <= maximum(times)
            best = 0
            for (i, ti) in enumerate(times)
                if ti <= a && (best == 0 || ti > times[best])
                    best = i
                end
            end
            return best - 1
        end
    end
    return record_in_file(temporal, anchor, fa)
end
