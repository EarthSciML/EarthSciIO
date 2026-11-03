# Calendar cadences (spec/esm-sources.md §2).
#
# A cadence's frequency and file period are ISO 8601 durations, and a month or a
# year is not a fixed number of seconds. `CalendarPeriod` holds a period as
# `months` and `seconds` and computes its anchors on the proleptic Gregorian
# calendar in UTC, exactly as the Python and Rust tracks do. `SourceTemporal`
# is the cadence of a time-varying source, as in those tracks.

"""
    CalendarPeriodError(msg)

An ISO 8601 period EarthSciIO cannot read; the message contains
`esm_period_invalid`."""
struct CalendarPeriodError <: Exception
    msg::String
end
Base.showerror(io::IO, e::CalendarPeriodError) = print(io, e.msg)

"""
    CalendarPeriod(months, seconds)
    CalendarPeriod(iso::AbstractString)
    CalendarPeriod(p::Dates.Period)

A cadence step of `months` calendar months plus `seconds` seconds (spec/esm-sources.md
§2.1). From an ISO 8601 string (`"P1M"`, `"PT3H"`), or from a `Dates` period
(`Month(1)`, `Hour(3)`)."""
struct CalendarPeriod
    months::Int
    seconds::Int
    function CalendarPeriod(months::Integer, seconds::Integer)
        (months < 0 || seconds < 0 || (months == 0 && seconds == 0)) && throw(
            CalendarPeriodError(
                "esm_period_invalid: a period must be positive, got months=$months seconds=$seconds"
            )
        )
        return new(Int(months), Int(seconds))
    end
end

const _ISO_PERIOD = r"^P(?:(\d+)Y)?(?:(\d+)M)?(?:(\d+)W)?(?:(\d+)D)?(?:T(?=\d)(?:(\d+)H)?(?:(\d+)M)?(?:(\d+)S)?)?$"

function CalendarPeriod(iso::AbstractString)
    m = match(_ISO_PERIOD, iso)
    (m === nothing || iso == "P") && throw(
        CalendarPeriodError("esm_period_invalid: cannot read $(repr(String(iso))) as an ISO 8601 period")
    )
    n = [c === nothing ? 0 : parse(Int, c) for c in m.captures]
    months = 12 * n[1] + n[2]
    seconds = 604800 * n[3] + 86400 * n[4] + 3600 * n[5] + 60 * n[6] + n[7]
    months == 0 && seconds == 0 && throw(
        CalendarPeriodError("esm_period_invalid: the period $(repr(String(iso))) is zero")
    )
    return CalendarPeriod(months, seconds)
end

CalendarPeriod(p::Union{Month, Year}) = CalendarPeriod(Dates.value(Month(p)), 0)
CalendarPeriod(p::Union{Week, Day, Hour, Minute, Second}) = CalendarPeriod(0, Dates.value(Second(p)))
CalendarPeriod(p::CalendarPeriod) = p

# A mean length, only to estimate an anchor index (never an anchor itself).
_approx_seconds(p::CalendarPeriod) = p.months * 2629746.0 + p.seconds

"""
    period_anchor(p, start, k) -> DateTime

Anchor `k` of the cadence `p` from `start`: `add_months(start, k·months) +
k·seconds`, the day of the month clamped to the target month's last day
(spec/esm-sources.md §2.2)."""
function period_anchor(p::CalendarPeriod, start::DateTime, k::Integer)
    out = start
    if p.months != 0
        d = Date(start)
        m0 = month(d) - 1 + k * p.months
        y, mo = year(d) + fld(m0, 12), mod(m0, 12) + 1
        dd = min(day(d), daysinmonth(y, mo))
        out = DateTime(Date(y, mo, dd)) + (start - DateTime(d))
    end
    p.seconds != 0 && (out += Second(k * p.seconds))
    return out
end

"""The largest `k` with `period_anchor(p, start, k) <= t`."""
function period_floor(p::CalendarPeriod, start::DateTime, t::DateTime)
    if p.months == 0
        return fld(Dates.value(t - start), 1000 * p.seconds)
    end
    k = floor(Int, (Dates.value(t - start) / 1000) / _approx_seconds(p))
    while period_anchor(p, start, k) > t
        k -= 1
    end
    while period_anchor(p, start, k + 1) <= t
        k += 1
    end
    return k
end

"""The smallest `k` with `period_anchor(p, start, k) >= t`."""
function period_ceil(p::CalendarPeriod, start::DateTime, t::DateTime)
    k = period_floor(p, start, t)
    return period_anchor(p, start, k) == t ? k : k + 1
end

_utc(t::DateTime) = t
_utc(t::Real) = unix2datetime(t)
_utc(t::AbstractString) = _parse_utc(t)
_unix(t::DateTime) = datetime2unix(t)

# An ISO 8601 instant, read as UTC; a trailing `Z` is accepted.
function _parse_utc(s::AbstractString)
    str = replace(strip(String(s)), r"(Z|z|\+00:?00)$" => "")
    for fmt in (dateformat"yyyy-mm-ddTHH:MM:SS.s", dateformat"yyyy-mm-ddTHH:MM:SS", dateformat"yyyy-mm-ddTHH:MM", dateformat"yyyy-mm-dd")
        dt = tryparse(DateTime, str, fmt)
        dt === nothing || return dt
    end
    throw(ArgumentError("cannot read $(repr(String(s))) as a UTC instant (yyyy-mm-ddTHH:MM:SSZ)"))
end

"""
    SourceTemporal(start, frequency, file_period; stop=nothing, time_dim="time",
                   records_per_sample=nothing)

The cadence of a time-varying source, as in the Python and Rust tracks: records
every `frequency`, files every `file_period`, both anchored at `start`
(spec/esm-sources.md §2). `start` and `stop` are UTC (`DateTime`, Unix seconds
or an ISO 8601 string); `stop` is the exclusive end of the data. The periods are
[`CalendarPeriod`](@ref)s or anything one is built from (`"P1M"`, `Hour(1)`).
`records_per_sample` is 1 (default) or 2, as for [`Provider`](@ref)."""
struct SourceTemporal
    start::DateTime
    frequency::CalendarPeriod
    file_period::CalendarPeriod
    stop::Union{Nothing, DateTime}
    time_dim::String
    records_per_sample::Union{Nothing, Int}
end

function SourceTemporal(
        start, frequency, file_period;
        stop = nothing, time_dim::AbstractString = "time", records_per_sample = nothing
    )
    return SourceTemporal(
        _utc(start), CalendarPeriod(frequency), CalendarPeriod(file_period),
        stop === nothing ? nothing : _utc(stop), String(time_dim),
        records_per_sample === nothing ? nothing : Int(records_per_sample)
    )
end

"""The frequency anchor at or before `t` (a `DateTime`)."""
record_anchor(s::SourceTemporal, t) =
    period_anchor(s.frequency, s.start, period_floor(s.frequency, s.start, _utc(t)))
"""The frequency anchor after the one at or before `t`."""
next_record_anchor(s::SourceTemporal, t) =
    period_anchor(s.frequency, s.start, period_floor(s.frequency, s.start, _utc(t)) + 1)
"""The first frequency anchor at or after `t`."""
first_record_anchor_from(s::SourceTemporal, t) =
    period_anchor(s.frequency, s.start, period_ceil(s.frequency, s.start, _utc(t)))
"""The file-period anchor at or before `t` (the URL is expanded there)."""
file_anchor(s::SourceTemporal, t) =
    period_anchor(s.file_period, s.start, period_floor(s.file_period, s.start, _utc(t)))
"""The file-period anchor after the one at or before `t`."""
next_file_anchor(s::SourceTemporal, t) =
    period_anchor(s.file_period, s.start, period_floor(s.file_period, s.start, _utc(t)) + 1)
"""The 0-based record of frequency anchor `a` in the file starting at `fa`."""
record_in_file(s::SourceTemporal, a, fa) =
    period_floor(s.frequency, s.start, _utc(a)) - period_ceil(s.frequency, s.start, _utc(fa))
