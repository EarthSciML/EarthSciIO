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

# Days from 1970-01-01 to the proleptic Gregorian date y-m-d.
function _ta_days_from_civil(y::Integer, m::Integer, d::Integer)
    y = Int64(y) - (m <= 2)
    era = fld(y, 400)
    yoe = y - era * 400
    mp = (m + 9) % 12
    doy = div(153 * mp + 2, 5) + d - 1
    doe = yoe * 365 + div(yoe, 4) - div(yoe, 100) + doy
    return era * 146097 + doe - 719468
end

# The proleptic Gregorian year of a day count from 1970-01-01.
function _ta_year_of_days(z::Integer)
    z = Int64(z) + 719468
    era = fld(z, 146097)
    doe = z - era * 146097
    yoe = div(doe - div(doe, 1460) + div(doe, 36524) - div(doe, 146096), 365)
    doy = doe - (365 * yoe + div(yoe, 4) - div(yoe, 100))
    mp = div(5 * doy + 2, 153)
    m = mp < 10 ? mp + 3 : mp - 9
    return yoe + era * 400 + (m <= 2)
end

_ta_leap(y) = (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
_ta_month_days(y, m) = m == 2 ? (_ta_leap(y) ? 29 : 28) : (m in (4, 6, 9, 11) ? 30 : 31)

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

# A CF reference date → (epoch nanoseconds, UTC) or `nothing`. The forms read
# are spec/esm-sources.md §2.5's: `YYYY-M-D`, then optionally (`T` or spaces)
# a time `HH`, `H:M`, `H:M:S` or `H:M:S.fff…`, then optionally (after spaces)
# `Z`, `UTC`, `GMT` or an offset `±HH`, `±HHMM`, `±HH:MM`.
function _ta_reference(s::AbstractString)
    s = String(s)
    all(isascii, s) || return nothing
    y, n, i = _ta_digits(s, 1, 4)
    n == 4 && _ta_at(s, i, '-') || return nothing
    mo, n, i = _ta_digits(s, i + 1, 2)
    n >= 1 && _ta_at(s, i, '-') || return nothing
    d, n, i = _ta_digits(s, i + 1, 2)
    n >= 1 || return nothing
    hh = mi = ss = 0
    frac = Int128(0)
    j = i
    if _ta_at(s, j, 'T')
        j += 1
    else
        while _ta_space(s, j)
            j += 1
        end
    end
    if j <= ncodeunits(s) && isdigit(Char(codeunit(s, j)))
        hh, n, j = _ta_digits(s, j, 2)
        if _ta_at(s, j, ':')
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
        end
        i = j
    end
    while _ta_space(s, i)
        i += 1
    end
    off = 0
    rest = s[i:end]
    if isempty(rest) || lowercase(rest) in ("z", "utc", "gmt")
        off = 0
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
    else
        return nothing
    end
    (1 <= mo <= 12 && 1 <= d <= _ta_month_days(y, mo)) || return nothing
    (hh <= 23 && mi <= 59 && ss <= 59) || return nothing
    secs = Int128(_ta_days_from_civil(y, mo, d)) * 86400 + hh * 3600 + mi * 60 + ss - off
    return secs * _TA_NS + frac
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
_ta_year_ok(ns::Int128) = 1678 <= _ta_year_of_days(fld(_ta_floor_s(ns), 86400)) <= 2261
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
`proleptic_gregorian`, a NaN or `fill` value, or a date outside the years 1678 to
2261. The decode is the Python track's (xarray's), rule by rule. The
`NativeField` form reads `units`, `calendar`, `_FillValue` and `missing_value`
from the field's attributes."""
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
    if pf !== nothing && _ta_in_i64(ref)
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
            t = ref + ns
            (_ta_in_i64(ns) && _ta_in_i64(t)) || (ok = false; break)
            push!(out, _ta_floor_s(t))
        end
        ok && return out
    end
    # cftime: round to the microsecond; a "standard" reference before the
    # Gregorian reform is on the Julian calendar there, which is not decoded
    cf = get(_TA_CFTIME_UNITS, unit, nothing)
    cf === nothing && return nothing
    cal != "proleptic_gregorian" && ref < Int128(_ta_days_from_civil(1582, 10, 15)) * 86400 * _TA_NS &&
        return nothing
    out = Int64[]
    for v in vals
        us = Float64(v) * (Float64(cf) / 1000)
        abs(us) < 1.0e30 || return nothing
        t = ref + Int128(round(us, RoundNearestTiesAway)) * 1000
        _ta_year_ok(t) || return nothing
        push!(out, _ta_floor_s(t))
    end
    return out
end

function decode_time_axis(coord::NativeField)
    a = coord.attrs
    fill = Any[a[k] for k in ("_FillValue", "missing_value") if haskey(a, k)]
    return decode_time_axis(
        coord.data, get(a, "units", nothing), get(a, "calendar", nothing);
        fill = isempty(fill) ? nothing : reduce(vcat, fill)
    )
end

# The 0-based record of `anchor` in the file covering it (file anchor `fa`),
# given that file's `time_dim` coordinate (`nothing` when it has none). By the
# decoded axis when the anchor lies within its span — the latest record at or
# before the anchor, the first such in file order on a tie — else the cadence
# offset (spec/esm-sources.md §2.5; Python `provider._record_index`).
function _axis_record(temporal::SourceTemporal, coord, anchor::DateTime, fa::DateTime)
    if coord !== nothing
        times = decode_time_axis(coord)
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
    end
    return record_in_file(temporal, anchor, fa)
end
