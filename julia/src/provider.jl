# The cadence Provider — component (b) (esio-9nb.5). A Provider fulfils the ESS
# data-loader CONTRACT for one `.esm` node: it resolves a URL (per time, for a
# time-varying source), fetches it through the content-addressed cache
# (component a), decodes it with the named `FORMAT_REGISTRY` reader, and returns
# RAW native-grid arrays. Variable remap / unit conversion / regrid are NOT here
# — they stay in ESS/ESD (Risk R3).
#
# It provides DATA, not a solver (the sanctioned impure I/O boundary). The
# library EXPOSES the provider and its `refresh_times`; the USER/solver drives
# the discrete-cadence update (e.g. a DifferentialEquations `PresetTimeCallback`
# that calls `refresh(provider, t)` at each tick). No solver is embedded here —
# `library-exposes-rhs-not-solver`.

"""
    Cadence

Temporal cadence of a provider's data (the `.esm` node's `temporal-cadence`):

  * `CONST` — time-invariant. `refresh_times` is empty; the data is materialized
    once and never refreshed.
  * `DISCRETE` — changes at known time points. `refresh_times` returns that grid;
    the solver refreshes at each tick."""
@enum Cadence CONST DISCRETE

"""
    Provider(cache, url; format, cadence=CONST, times=Float64[], time_dim=nothing,
             variables=nothing, reader_kwargs=NamedTuple(),
             source_loader=nothing, auth_realm=nothing)

A data provider over the shared cache.

  * `url` — a resolved-URL `String` (constant source) OR a function `t -> url`
    (a time-varying source whose file/key changes per tick).
  * `format` — reader name in [`FORMAT_REGISTRY`] (`"netcdf"`, `"csv"`, …).
  * `cadence` — [`CONST`] or [`DISCRETE`]. `DISCRETE` requires non-empty `times`;
    `CONST` requires empty `times`.
  * `times` — the discrete cadence grid (sorted on construction); what
    [`refresh_times`] returns. It may span MANY files: pair it with a `t -> url`
    resolver and the cadence walks from one file to the next (see `time_dim`).
  * `time_dim` — when set on a `DISCRETE` provider whose files hold the cadence
    on an internal axis (e.g. a daily file of hourly steps),
    [`materialize`]`(p, t)` slices that dimension to the tick's record.

    The record is located WITHIN its file, not within the whole cadence: tick
    `i` is record `mod1(i, L)` of the file the resolver returns for it, where `L`
    is that file's length along `time_dim`. So a cadence of 56 three-hourly ticks
    over seven 8-record daily files reads record 1 of day 2 at tick 9, not
    record 9 of day 1 (which does not exist). A single file holding the whole
    cadence is the `L >= length(times)` case, where `mod1` is the identity — the
    behaviour is unchanged. This assumes files are UNIFORM in length and that
    their records are consecutive on the cadence; a short/trimmed file mid-run
    shifts every later tick (the Python track avoids that by matching the file's
    own decoded time axis, which the Julia track cannot do because `times` is a
    caller-defined grid rather than the file's raw axis).
  * `records_per_sample` — `nothing` or `1` (default) returns the SINGLE
    at-or-before record with `time_dim` DROPPED (held piecewise-constant); `2`
    returns the TWO bracketing records (floor + successor) with `time_dim`
    RETAINED at length 2 and a canonical 2-element `time_dim` coordinate of Unix
    epoch seconds (seconds from the `time_origin` when given), so a downstream
    model interpolates in time. `2` requires a
    `time_dim`; the successor is read across a file boundary when needed, and at
    the last cadence tick the bracket degenerates to `[last, last]` (equal
    timestamps) so the downstream weight clamps. The provider does pure I/O
    (returns N records) and performs no interpolation itself.
  * `variables` — restrict the returned data variables (coords always kept);
    `nothing` returns all.
  * `reader_kwargs` — extra keywords forwarded to [`read_native`] (e.g. the CSV
    reader's `numeric_columns`).
  * `source_loader` / `auth_realm` — recorded in the cache manifest on fetch.
  * `temporal`, `window` — a [`SourceTemporal`](@ref) and a run window
    `(start, stop)` in UTC, as in the Python and Rust tracks (spec/esm-sources.md
    §2.4), in place of `cadence`/`times`/`time_dim`. Times are then Unix
    seconds, and [`refresh_times`] returns the record anchors in the window
    (its end, else the source's `stop`), possibly none; [`window_records`](@ref)
    returns the records that serve it. A window that starts before the source's
    `start`, runs past its `stop` or is empty is refused with
    [`WindowNotCovered`](@ref) (spec/esm-sources.md §2.6).
    A sample at any `t` at or after the source start — inside the window or
    not — reads the record at or before `t` from its own file by the calendar
    (months and years exact), and a two-record bracket's successor is the next
    record unless it is at or past `stop`. `records_per_sample` here, else the
    temporal's, chooses one record or two. `url` is a `String`, a function of
    the file anchor in Unix seconds, or an [`EsmTemplate`](@ref) expanded there.
  * `time_origin` — with `temporal`, the instant the provider counts time from
    (spec/esm-sources.md §4), usually an esm model's time origin: a `DateTime`
    (UTC) or an ISO 8601 string, a whole second. Every time it takes (`t`, a
    `Real` window bound) and returns ([`refresh_times`], [`window_records`],
    the anchors and boundaries of a [`PrefetchGroup`], a two-record bracket's
    time coordinate, whose `units` become `seconds since <origin>`) is then
    seconds from it. `nothing` (the default) is the Unix epoch, as before. A
    malformed or fractional origin is [`TimeOriginInvalid`](@ref). A CONST
    provider has no times and ignores it; one built from `times` refuses it.
  * `mirrors` — fallback URLs tried in order when the primary fetch fails; the
    blob is still cached under the primary URL's key. With an
    [`EsmTemplate`](@ref) `url`, leave this empty and give the mirrors to the
    template: they are esm templates expanded at the same anchor
    (spec/esm-sources.md §1.5)."""
struct Provider
    cache::Cache
    format::String
    cadence::Cadence
    times::Vector{Float64}
    url_for::Function
    time_dim::Union{Nothing, String}
    variables::Union{Nothing, Vector{String}}
    reader_kwargs::Dict{Symbol, Any}
    source_loader::Union{Nothing, String}
    auth_realm::Union{Nothing, String}
    records_per_sample::Union{Nothing, Int}
    mirrors_for::Function
    plan::Any                     # `nothing`, or the `_TemporalPlan` of a `temporal` provider
end

# The clock of a `temporal` provider (spec/esm-sources.md §4): the time origin
# its ticks count from (`nothing` is the Unix epoch), the origin's Unix seconds,
# and the CF `units` of a tick, each computed once. Every tick it takes goes
# through `_instant` and every tick it returns through `_tick`: the one place
# the origin applies.
struct _Clock
    origin::Union{Nothing, DateTime}
    off::Float64
    units::String
end
_Clock(origin::Union{Nothing, DateTime}) =
    _Clock(origin, origin === nothing ? 0.0 : _unix(origin), _origin_units(origin))

# The tick (seconds from the origin) of the instant `a`: its Unix seconds minus
# the origin's, as every track computes it.
_tick(c::_Clock, a::DateTime) = _unix(a) - c.off
# The instant a tick `t` names; a `DateTime` (or an ISO 8601 string) is one as given.
_instant(c::_Clock, t::Real) = unix2datetime(Float64(t) + c.off)
_instant(::_Clock, t) = _utc(t)

# What a provider built from a `SourceTemporal` keeps (spec/esm-sources.md
# §2.4): the cadence, from which every sample is located on the calendar — any
# `t` at or after `temporal.start`, inside the window or not, as in Python and
# Rust — and the window `[lo, hi)` that `refresh_times` and `prefetch` walk.
# `closing` is the record at `hi` itself when there is one (`_closing_record`),
# which the closed window `[lo, hi]` adds. The record of an anchor in its file is
# found when the file is read, by the file's own time axis (§2.5,
# `_temporal_sample`); `axes` keeps the decoded axes of the last two files read,
# so a file's axis is decoded once however many samples it serves.
struct _TemporalPlan
    temporal::SourceTemporal
    lo::DateTime
    hi::DateTime
    closing::Union{Nothing, DateTime}
    axes::Vector{Pair{DateTime, Any}}
    lock::ReentrantLock
    clock::_Clock   # the time origin its ticks count from (spec/esm-sources.md §4)
end

# A tick of a `temporal` provider and the instant it names, through its clock.
_tick(plan::_TemporalPlan, a::DateTime) = _tick(plan.clock, a)
_instant(plan::_TemporalPlan, t) = _instant(plan.clock, t)

"""
    TimeOriginInvalid(msg)

A provider's `time_origin` that is not a whole-second UTC instant
(spec/esm-sources.md §4). The message starts with the code
`time_origin_invalid`, the same in every track."""
struct TimeOriginInvalid <: Exception
    msg::String
end
Base.showerror(io::IO, e::TimeOriginInvalid) = print(io, e.msg)

# A `time_origin` as a `DateTime`, or `nothing` (spec/esm-sources.md §4). Any
# nonzero fraction of a second is refused, however small: a string's fraction is
# checked as written, since a `DateTime` keeps only milliseconds.
function _time_origin(origin)
    origin === nothing && return nothing
    if origin isa AbstractString
        m = match(_ISO_INSTANT, strip(String(origin)))
        frac = m === nothing ? nothing : m.captures[7]
        frac === nothing || all(==('0'), frac) || throw(
            TimeOriginInvalid(
                "time_origin_invalid: the time origin $(repr(origin)) is not a whole second"
            )
        )
    end
    o = try
        origin isa Union{DateTime, AbstractString} || throw(ArgumentError("not a time"))
        _utc(origin)
    catch err
        err isa InterruptException && rethrow()
        throw(
            TimeOriginInvalid(
                "time_origin_invalid: $(repr(origin)) is not a DateTime or an ISO 8601 instant"
            )
        )
    end
    Dates.millisecond(o) == 0 || throw(
        TimeOriginInvalid("time_origin_invalid: the time origin $(o)Z is not a whole second")
    )
    return o
end

# The CF `units` of a time in seconds from `origin` (the Unix epoch when `nothing`).
function _origin_units(origin)
    o = origin === nothing ? DateTime(1970) : origin
    return "seconds since " * Dates.format(o, dateformat"yyyy-mm-ddTHH:MM:SS") * "Z"
end

"""
    WindowNotCovered(msg)

A provider's run window some stretch of which no record serves
(spec/esm-sources.md §2.6): it starts before the source's first record, runs
past the end of its data, or is empty. The message starts with the code
`esm_window_uncovered`, the same in every track."""
struct WindowNotCovered <: Exception
    msg::String
end
Base.showerror(io::IO, e::WindowNotCovered) = print(io, e.msg)

# Refuse a run window `[a, b)` some stretch of which no record serves
# (spec/esm-sources.md §2.6). The records serving it are the one in effect at
# `a`, every one whose anchor is inside it and, for a two-record bracket, the
# successor of the last of them; they cover it unless `a` precedes `start` (no
# record is in effect there yet) or `b` is past `stop` (the last record ends
# there). An empty or reversed window is refused too: no record serves it.
function _check_run_window(temporal::SourceTemporal, a::DateTime, b::DateTime)
    span = "the run window [$(a)Z, $(b)Z)"
    b <= a && throw(
        WindowNotCovered("esm_window_uncovered: $span is empty: its end must be after its start")
    )
    a < temporal.start && throw(
        WindowNotCovered(
            "esm_window_uncovered: $span starts before the source's first record, " *
                "at $(temporal.start)Z"
        )
    )
    temporal.stop !== nothing && b > temporal.stop && throw(
        WindowNotCovered(
            "esm_window_uncovered: $span runs past the end of the source's data, " *
                "at $(temporal.stop)Z"
        )
    )
    return nothing
end

# The plan of a `temporal` provider over `window` (UTC `DateTime`s, or seconds
# from `origin`, the Unix epoch when `nothing`) and its ticks, in seconds from
# `origin`: the record anchors in `[lo, hi)`, where `lo` is the
# window start (else the source start) and `hi` the window end, else the
# source's `stop` — exactly Python's and Rust's `refresh_times`. A window some
# stretch of which no record serves is refused (`_check_run_window`); one inside a
# single record's interval has no ticks, and is served by that record.
function _temporal_plan(temporal::SourceTemporal, window, origin = nothing)
    clock = _Clock(origin)
    at(x) = _instant(clock, x)
    if window !== nothing
        _check_run_window(temporal, at(window[1]), at(window[2]))
    end
    lo = window === nothing ? temporal.start : at(window[1])
    hi = window === nothing ? temporal.stop : at(window[2])
    hi === nothing && throw(
        ArgumentError(
            "a temporal provider needs a bounded window: pass window = (start, stop) " *
                "or give the SourceTemporal a stop"
        )
    )
    f, s = temporal.frequency, temporal.start
    ticks = Float64[]
    k = period_ceil(f, s, lo)               # one ceil, then each anchor by index
    a = period_anchor(f, s, k)
    while a < hi
        push!(ticks, _tick(clock, a))
        k += 1
        a = period_anchor(f, s, k)
    end
    closing = _closing_record(temporal, hi)
    plan = _TemporalPlan(temporal, lo, hi, closing, Pair{DateTime, Any}[], ReentrantLock(), clock)
    return plan, ticks
end

# The anchors (`DateTime`) of the records serving the run window of `plan`
# (spec/esm-sources.md §2.6): the one in effect at `lo`, each one after it while
# `< hi`, then `_end_records`.
function _window_records(plan::_TemporalPlan, bracket::Bool)
    f, s = plan.temporal.frequency, plan.temporal.start
    k = period_floor(f, s, plan.lo)
    out = DateTime[period_anchor(f, s, k)]
    a = period_anchor(f, s, k + 1)
    while a < plan.hi
        push!(out, a)
        k += 1
        a = period_anchor(f, s, k + 1)
    end
    return append!(out, _end_records(plan, bracket))
end

# The records the run window of `plan` needs at or after its end `hi`
# (spec/esm-sources.md §2.6, items 3 and 4): the record at `hi` itself when there
# is one (`plan.closing`, the refresh at the window end that
# `refresh_times_closed` adds), then, for a two-record bracket, the successor of
# the last record, unless it is at or past `stop`.
function _end_records(plan::_TemporalPlan, bracket::Bool)
    s, closing = plan.temporal, plan.closing
    out = closing === nothing ? DateTime[] : DateTime[closing]
    if bracket
        after = closing === nothing ? plan.hi : next_record_anchor(s, closing)
        nxt = _bracket_successor(s, after)
        nxt === nothing || push!(out, nxt)
    end
    return out
end

# `hi` when a record is anchored there — it is a cadence anchor before `stop` —
# else `nothing`: the refresh a closed window `[lo, hi]` adds
# (spec/esm-sources.md §2.3).
_closing_record(temporal::SourceTemporal, hi::DateTime) =
    _bracket_successor(temporal, hi) == hi ? hi : nothing

# The first record anchor at or after `hi`, unless it is at or past `stop` (no
# record is there): the record a two-record bracket needs past a window ending
# at `hi` (spec/esm-sources.md §2.6).
function _bracket_successor(temporal::SourceTemporal, hi::DateTime)
    nxt = first_record_anchor_from(temporal, hi)
    return temporal.stop === nothing || nxt < temporal.stop ? nxt : nothing
end

# Where `p`'s sample can change over its run window: its refresh times over the
# closed window (`refresh_times_closed`, so a run that ends on a record gets an
# epoch at its end) and, for a `temporal` provider, the window start, where the
# record in effect there takes over (spec/esm-sources.md §2.6) — the schedule a
# `PrefetchGroup` builds its epochs from (spec/prefetch.md §3.2).
function _epoch_starts(p::Provider)
    times = refresh_times_closed(p)
    p.plan === nothing && return times
    plan = p.plan::_TemporalPlan
    lo = _tick(plan, plan.lo)
    return !isempty(times) && first(times) == lo ? times : pushfirst!(times, lo)
end

function Provider(
        cache::Cache, url; format::AbstractString,
        cadence::Cadence = CONST, times = Float64[],
        time_dim = nothing, variables = nothing,
        reader_kwargs = NamedTuple(),
        source_loader = nothing, auth_realm = nothing,
        records_per_sample = nothing, mirrors = String[],
        temporal::Union{Nothing, SourceTemporal} = nothing, window = nothing,
        time_origin = nothing
    )
    plan = nothing
    file_url = url
    origin = _time_origin(time_origin)
    if temporal !== nothing
        (isempty(times) && time_dim === nothing && cadence == CONST) || throw(
            ArgumentError(
                "a `temporal` provider computes its own cadence: give no `cadence`, " *
                    "`times` or `time_dim`"
            )
        )
        plan, times = _temporal_plan(temporal, window, origin)
        cadence = DISCRETE
        time_dim = temporal.time_dim
        records_per_sample === nothing && (records_per_sample = temporal.records_per_sample)
        # The URL names a FILE: expand it at the tick's file anchor, in Unix
        # seconds (a tick `t` counts from the time origin).
        file_url = if url isa AbstractString
            String(url)
        else
            let f = url, clock = plan.clock
                t -> f(_unix(file_anchor(temporal, _instant(clock, t))))
            end
        end
    elseif window !== nothing
        throw(ArgumentError("`window` applies to a `temporal` provider only"))
    elseif origin !== nothing && cadence == DISCRETE
        throw(
            ArgumentError(
                "`time_origin` applies to a `temporal` provider: the `times` of this one " *
                    "are in the caller's own unit"
            )
        )
    end
    haskey(FORMAT_REGISTRY, format) || throw(
        ArgumentError(
            "format '$format' is not registered in the format registry; " *
                "registered: $(registered_names(FORMAT_REGISTRY))"
        )
    )
    tvec = sort!(Float64[t for t in times])
    if cadence == CONST && !isempty(tvec)
        throw(
            ArgumentError(
                "a CONST provider has no refresh times, got $(length(tvec)) — use cadence=DISCRETE"
            )
        )
    elseif cadence == DISCRETE && isempty(tvec) && plan === nothing
        throw(
            ArgumentError(
                "a DISCRETE provider requires a non-empty cadence (times=...)"
            )
        )
    end
    time_dim === nothing || cadence == DISCRETE ||
        throw(ArgumentError("time_dim only applies to a DISCRETE provider"))
    records_per_sample === nothing || records_per_sample == 1 || records_per_sample == 2 ||
        throw(
        ArgumentError(
            "records_per_sample must be 1 or 2, got $(repr(records_per_sample))"
        )
    )
    records_per_sample != 2 || time_dim !== nothing ||
        throw(
        ArgumentError(
            "records_per_sample=2 needs a time_dim to bracket along"
        )
    )
    kw = Dict{Symbol, Any}(pairs(reader_kwargs))
    _check_reader_kwargs(FORMAT_REGISTRY[format], String(format), kw)
    # A `time_dim` makes the RECORD AXIS the Provider's: it resolves a cadence
    # tick to that file's own record (`_file_record`) and slices — or brackets —
    # there. A `records` baked into `reader_kwargs` narrows the very same axis
    # BEHIND that resolution, so the tick is then located inside an
    # already-narrowed axis: with a one-record `records` every tick becomes
    # `mod1(tick, 1) == 1` and `refresh` hands back the SAME record at every
    # tick, and `records_per_sample=2` degenerates to `[r, r]` so a downstream
    # interpolation runs on a constant field. That is a silently wrong number, so
    # it is refused here rather than decoded (spec/registries.md §2.1, the same
    # rule that makes an unrecognised option an error rather than an ignored key).
    # `records = nothing` stays legal: it asks only to skip the pushdown and read
    # the record axis whole, which is what `_records_pushdown` then does.
    if time_dim !== nothing && get(kw, :records, nothing) !== nothing
        throw(
            ArgumentError(
                "reader option `records` may not be baked into a provider that owns a " *
                    "record axis (time_dim=$(repr(String(time_dim)))): the Provider resolves " *
                    "each cadence tick to a file-local record and pushes THAT down, so a " *
                    "caller-supplied `records` would narrow the axis the tick is located " *
                    "inside and every tick would read the same record. Drop it — or pass " *
                    "`records = nothing` to opt out of the pushdown and read the axis whole"
            )
        )
    end
    url_for = file_url isa AbstractString ? (
            let u = String(file_url)
                _ -> u
        end
        ) : file_url
    literal_mirrors = String[String(m) for m in mirrors]
    if url isa EsmTemplate
        isempty(literal_mirrors) || throw(
            ArgumentError(
                "an EsmTemplate url carries its own mirrors (EsmTemplate(t; mirrors = …)); " *
                    "a separate `mirrors` list would not be expanded at the anchor"
            )
        )
        mirrors_for = if plan === nothing
            t -> esm_mirrors(url, t)
        else
            let clock = plan.clock
                t -> esm_mirrors(url, _unix(file_anchor(temporal, _instant(clock, t))))
            end
        end
    else
        mirrors_for = _ -> literal_mirrors
    end
    return Provider(
        cache, String(format), cadence, tvec, url_for,
        time_dim === nothing ? nothing : String(time_dim),
        variables === nothing ? nothing : String.(collect(variables)),
        kw,
        source_loader === nothing ? nothing : String(source_loader),
        auth_realm === nothing ? nothing : String(auth_realm),
        records_per_sample === nothing ? nothing : Int(records_per_sample),
        mirrors_for, plan
    )
end

# Fetch (or reuse) the blob of the file for time `t`, with its mirrors.
_fetch_at(p::Provider, t) = fetch_blob(
    p.cache, p.url_for(t);
    source_loader = p.source_loader, auth_realm = p.auth_realm,
    mirrors = p.mirrors_for(t)
)

# The loader's declared decode options, checked against the bound reader AT
# CONSTRUCTION (spec/registries.md §2.1). An unrecognised option is an ERROR
# here, never an ignored key, so a mis-typed `member_filter` cannot quietly
# select nothing and surface much later as wrong numbers. The accepted set is
# the reader's own keyword declaration (`reader_option_keys`), so this can never
# disagree with what `read_native` / `read_store` will actually honour.
function _check_reader_kwargs(reader, format::String, kw::AbstractDict{Symbol, <:Any})
    isempty(kw) && return nothing
    accepted = reader_option_keys(reader)
    accepted === nothing && return nothing        # a reader that slurps everything
    unknown = sort!(String[String(k) for k in keys(kw) if !(k in accepted)])
    isempty(unknown) && return nothing
    throw(
        ArgumentError(
            "the '$format' reader does not recognise reader option(s) " *
                "$(join(unknown, ", ")); it takes $(join(sort!(String.(accepted)), ", ")). " *
                "An unrecognised option is an error rather than an ignored key " *
                "(spec/registries.md §2.1) — a mis-typed option would otherwise decode " *
                "something else, silently"
        )
    )
end

"""Thin constructor for a time-invariant ([`CONST`]) provider."""
const_provider(cache::Cache, url; kwargs...) =
    Provider(cache, url; cadence = CONST, kwargs...)

"""Thin constructor for a time-varying ([`DISCRETE`]) provider over `times`."""
discrete_provider(cache::Cache, url, times; kwargs...) =
    Provider(cache, url; cadence = DISCRETE, times = times, kwargs...)

"""
    refresh_times(p::Provider) -> Vector{Float64}

The discrete time points at which the data changes and the solver must
[`refresh`]. Empty for a [`CONST`] provider; the sorted cadence grid for a
[`DISCRETE`] one. The library exposes these; the user wires them into the
solver (e.g. `PresetTimeCallback(refresh_times(p), …)`).

For a `temporal` provider these are the record anchors in the half-open window
`[lo, hi)`: a refresh at exactly the window end is not among them;
[`refresh_times_closed`](@ref) includes it."""
refresh_times(p::Provider) = copy(p.times)

"""
    refresh_times_closed(p::Provider) -> Vector{Float64}

[`refresh_times`](@ref) over the CLOSED window `[lo, hi]` (spec/esm-sources.md
§2.3): the same anchors plus the one at the window end itself, when the window
end is a cadence anchor before the source's `stop`. The tstops of a solver that
integrates over `[t0, tf]` and must refresh at `tf` when `tf` lands on a record.
With no window (`hi` is `stop`, where no record is) it equals `refresh_times`,
as it does for a provider over a caller-made `times` grid, which has no window."""
function refresh_times_closed(p::Provider)
    times = refresh_times(p)
    p.plan === nothing && return times
    closing = (p.plan::_TemporalPlan).closing
    closing === nothing || push!(times, _tick(p.plan::_TemporalPlan, closing))
    return times
end

"""
    window_records(p::Provider) -> Vector{Float64}

The anchors, in seconds from the provider's `time_origin` when it has one
(Unix seconds otherwise), of the records that serve a `temporal` provider's run
window, in order (spec/esm-sources.md §2.6): the record in effect at the window
start, every record whose anchor is inside the window, the record at the window
end itself when there is one ([`refresh_times_closed`](@ref) adds it) and, with
`records_per_sample = 2`, the record after the last of these that its bracket
reads (unless that is at or past the source's `stop`).

Unlike [`refresh_times`](@ref) it is never empty for a `temporal` provider: a
window inside one record's interval has no refresh times and is served by that
record. Empty for a provider not built from a [`SourceTemporal`](@ref)."""
function window_records(p::Provider)
    p.plan === nothing && return Float64[]
    plan = p.plan::_TemporalPlan
    return Float64[
        _tick(plan, a) for a in _window_records(plan, p.records_per_sample == 2)
    ]
end

"""True if `p`'s data is time-invariant ([`CONST`])."""
is_const(p::Provider) = p.cadence == CONST

# The format-reader instance backing `p` (mirrors the `_load` lookup so
# `supports_selection`/`array_shape` resolve the SAME reader the sample path uses).
_reader_for(p::Provider) = FORMAT_REGISTRY[p.format]

# Fetch (cache) + decode (format reader) for the URL resolved at `t`.
#
# `select` is an optional PER-CALL projection override (component (b) pushdown):
# `nothing` uses the baked `reader_kwargs[:select]`, a non-`nothing` value
# OVERRIDES it for this call only. Any reader that answers `supports_selection`
# can honour it — a store-backed one by fetching fewer objects, a whole-file one
# by materialising only the requested hyperslab of the blob it already fetched.
# A `select` handed to a reader that does neither is a clear error, raised BEFORE
# any fetch (the fetch-full fallback belongs to the EarthSciAST caller, not here).
function _load(p::Provider, t; select = nothing, records = nothing)
    reader = _reader_for(p)
    # Store-backed readers (e.g. zarr) are handed (cache, base_url; variables,
    # select): a Zarr `url` is a directory-like prefix, not a fetchable blob, so
    # the reader fetches individual objects on demand. Active whole-file readers
    # inherit `store_backed = false`.
    if store_backed(reader)
        # Effective select: a per-call `select` OVERRIDES the baked
        # reader_kwargs[:select]. Forward it explicitly and splat the REST of
        # reader_kwargs (with `:select` removed, so `select` is never passed twice).
        records === nothing || throw(
            ArgumentError(
                "a store-backed reader takes its record selection through `select`, " *
                    "not `records`"
            )
        )
        effective = select === nothing ? get(p.reader_kwargs, :select, nothing) : select
        rest = Dict{Symbol, Any}(k => v for (k, v) in p.reader_kwargs if k !== :select)
        return read_store(
            reader, p.cache, p.url_for(t);
            variables = p.variables, select = effective, rest...
        )
    end
    # Whole-file reader. A `select` — per-call, else the baked one — reaches it
    # only if it declares `supports_selection`; the netcdf reader does, and honours
    # it at DECODE time (same blob, same cache key, only the requested hyperslab
    # materialised). Checked before the fetch so a refusal costs nothing.
    effective = select === nothing ? get(p.reader_kwargs, :select, nothing) : select
    effective === nothing || supports_selection(reader) || throw(
        ArgumentError(
            "reader $(typeof(reader)) does not support select/pushdown"
        )
    )
    entry = _fetch_at(p, t)
    # PROJECTION PUSHDOWN for a whole-file reader that declares a `variables`
    # decode option (the `parquet` reader): the loader's `variables` reach the
    # READER, so only those column chunks come off disk — spec/conformance.md §3
    # "Projection pushdown" — instead of being applied to an already-decoded
    # dataset. That is not only a speed matter on a table dozens of columns
    # wide: a column the document never named must not be able to FAIL the read
    # (a null-bearing integer column with no `null_int` declared would), which
    # is what the Python and Rust tracks already do. A reader with no such
    # option keeps the read-everything-then-`_select` path below.
    #
    # An EMPTY `variables` means "every variable", NOT "no variables"
    # (spec/conformance.md §3; the Python and Rust tracks both test the list for
    # emptiness before using it). Normalised once here so the pushdown and the
    # `_select` below agree — an empty list used to hand back an empty dataset.
    wanted = (p.variables === nothing || isempty(p.variables)) ? nothing : p.variables
    # `:select` is forwarded explicitly (the per-call value wins over the baked
    # one), so it is stripped from the splat rather than passed twice.
    kw = Dict{Symbol, Any}(k => v for (k, v) in p.reader_kwargs if k !== :select)
    effective === nothing || (kw[:select] = effective)
    if wanted !== nothing && !haskey(kw, :variables)
        opts = reader_option_keys(reader)
        if opts !== nothing && :variables in opts
            kw[:variables] = wanted
        end
    end
    # RECORD PUSHDOWN (`_load_ticks`): the records this file must yield, already
    # resolved to the file's own 0-based numbering by the caller — the cadence
    # never reaches the reader. Only set on the pushdown path, which has already
    # checked the reader declares the option.
    records === nothing || (kw[:records] = records)
    nds = read_native(reader, entry.path; kw...)
    return wanted === nothing ? nds : _select(nds, wanted)
end

"""
    supports_selection(p::Provider) -> Bool

True when `p`'s format reader can honour an orthogonal `select` **without
materialising the whole array**. A caller uses this to decide whether to push a
projection down (via `materialize(p, t; select=…)`) or to read whole and slice on
its own side.

It says nothing about what is FETCHED — pair it with [`store_backed`] for that:

| `supports_selection` | `store_backed` | what a `select` saves | reader |
|---|---|---|---|
| `true` | `true`  | fetch **and** decode — only the intersecting objects are downloaded | `zarr` |
| `true` | `false` | decode only — the whole blob is still fetched under the same cache key | `netcdf` |
| `false` | — | nothing; a `select` is an error | `csv`, `ff10`, `parquet`, … |"""
supports_selection(p::Provider) = supports_selection(_reader_for(p))

"""
    store_backed(p::Provider) -> Bool

True when `p`'s format reader reads a directory-like STORE (a Zarr v2 store, whose
`.zarray`/`.zattrs`/chunks are each their own object) rather than one fetchable
blob. Read with [`supports_selection`] it tells a caller whether a pushed-down
`select` shrinks the DOWNLOAD or only the decode — see the table there."""
store_backed(p::Provider) = store_backed(_reader_for(p))

"""
    array_shape(p::Provider, var::AbstractString) -> Union{Nothing,NTuple{N,Int}}

The full native (dims-order) shape of on-disk array `var`, for a honour/refuse
pushdown decision. For a store-backed zarr provider this reads ONLY the array's
`.zarray` metadata (never a chunk); `nothing` for a whole-file reader (whose
shape is not knowable without reading the blob)."""
function array_shape(p::Provider, var::AbstractString)
    reader = _reader_for(p)
    store_backed(reader) || return nothing
    t0 = p.cadence == CONST ? 0.0 :
        p.plan === nothing ? first(p.times) :
        _tick(p.plan, record_anchor(p.plan.temporal, p.plan.lo))
    return array_shape(reader, p.cache, p.url_for(t0), var)
end

function _select(nds::NativeDataset, want::Vector{String})
    keep = Dict{String, NativeField}()
    for name in want
        haskey(nds.variables, name) || throw(
            ArgumentError(
                "requested variable '$name' not in blob; present: $(variable_names(nds))"
            )
        )
        keep[name] = nds.variables[name]
    end
    return NativeDataset(keep, nds.coords)
end

# Index of the cadence tick active at `t`: exact match, else the last tick ≤ t
# (the currently-in-effect record). Errors if `t` precedes the first tick.
function _tick_index(p::Provider, t::Real)
    tf = Float64(t)
    i = findfirst(==(tf), p.times)
    i === nothing || return i
    j = searchsortedlast(p.times, tf)
    j >= 1 && return j
    throw(
        ArgumentError(
            "t=$t precedes the provider's first refresh time $(first(p.times))"
        )
    )
end

# The record index of cadence tick `tick` WITHIN its own file, given that file's
# length `len` along `time_dim`. The cadence is global and files are local: tick
# 9 of a 56-tick cadence laid over 8-record daily files is record 1 of day 2, not
# record 9 of day 1. `len <= 0` means nothing in the blob carries `time_dim`, so
# there is no wrap to apply and the tick passes through (the caller's slice then
# reports the real out-of-range error rather than a silently wrong record).
_file_record(tick::Integer, len::Integer) = len <= 0 ? Int(tick) : mod1(Int(tick), Int(len))

# --- record pushdown --------------------------------------------------------
# Can this provider hand its record CHOICE to the reader, instead of decoding the
# whole cadence axis and slicing afterwards? Three things must hold: the reader
# declares a `records` decode option (the same `reader_option_keys` rule the
# `variables` projection uses), the caller has not baked a `records` of its own
# into `reader_kwargs` (only `nothing` can be baked beside a `time_dim` — the
# constructor refuses any other value, because narrowing the record axis behind
# the Provider's own tick→record resolution reads the wrong record), and the
# source is a whole-file blob — a store-backed
# reader narrows records through `select`, whose chunk arithmetic already shrinks
# the FETCH, and must not be handed a second, conflicting mechanism.
function _records_pushdown(p::Provider)
    p.time_dim === nothing && return false
    reader = _reader_for(p)
    store_backed(reader) && return false
    haskey(p.reader_kwargs, :records) && return false
    opts = reader_option_keys(reader)
    return opts !== nothing && :records in opts
end

# Decode the file holding cadence tick `ticks[1]`, keeping ONLY the records those
# ticks name, and return `(nds, positions)` — `positions[k]` is where `ticks[k]`
# sits along `time_dim` in `nds`.
#
# Every tick in `ticks` must land in the SAME file; the caller checks that by URL
# before asking. The cadence→record arithmetic (`_file_record`, i.e. `mod1` over
# the file's own length) stays HERE, on the side that owns the cadence: the
# reader is handed absolute 0-based record indices and never learns of `times`,
# `records_per_sample` or a file boundary. That is why the length has to be read
# from metadata first — a record cannot be chosen before the decode without it.
#
# A reader that takes no `records` option falls back to the historical path:
# decode the axis whole, return the record indices to slice at. The two paths
# differ in what is DECODED and in nothing else.
function _load_ticks(p::Provider, ticks::AbstractVector{<:Integer}; select = nothing)
    return _load_records(
        p, p.times[first(ticks)], len -> Int[_file_record(tk, len) for tk in ticks];
        select = select
    )
end

# Decode the file `p.url_for(t_url)` names, keeping only the records
# `recs(len)` lists (1-based, `len` the file's length along `time_dim`), and
# return `(nds, positions)` — `positions[k]` is where `recs(len)[k]` sits along
# `time_dim` in `nds`. `_load_ticks` and a `temporal` provider's calendar
# sampling both read through here; only how the records are chosen differs.
function _load_records(p::Provider, t_url, recs; select = nothing)
    dim = p.time_dim::String
    if !_records_pushdown(p)
        nds = _load(p, t_url; select = select)
        return (nds, recs(_time_len(nds, dim)))
    end
    # The record a tick names is `mod1(tick, len)`, so the pushdown needs the
    # file's length along `dim` — which only the decode's own open knows. Rather
    # than open the blob twice (measured: on a GEOS-FP A1 file the extra open
    # costs more than the records it saves), hand the reader THIS CLOSURE: it is
    # called with `len` inside the reader's single open, and the cadence
    # arithmetic still happens here, in the Provider, exactly as before.
    fired = Ref(false)
    nwant = Ref(0)
    resolve = function (len::Integer)
        fired[] = true
        want = recs(Int(len))
        nwant[] = length(want)
        return Int[r - 1 for r in want]   # 0-based
    end
    nds = _load(
        p, t_url; select = select,
        records = Dict{String, Any}("dim" => dim, "indices" => resolve)
    )
    # The resolver is not called when the blob has no such dimension. That is the
    # `len <= 0` case of `_file_record`, and it must behave exactly as it always
    # did: nothing was narrowed, so the tick passes through to the slice.
    fired[] && return (nds, collect(1:nwant[]))
    return (nds, recs(_time_len(nds, dim)))
end

# Slice `dim` out of every variable that carries it, at record `idx`; drop the
# now-singular dimension and its coordinate. Used for an internal-axis DISCRETE
# source (many time records per file).
function _slice_dim(nds::NativeDataset, dim::String, idx::Integer)
    vars = Dict{String, NativeField}()
    for (name, f) in nds.variables
        pos = findfirst(==(dim), f.dims)
        if pos === nothing
            vars[name] = f
        else
            sliced = collect(selectdim(f.data, pos, idx))
            vars[name] = NativeField(sliced, [d for d in f.dims if d != dim], f.attrs)
        end
    end
    coords = Dict(k => v for (k, v) in nds.coords if k != dim)
    return NativeDataset(vars, coords)
end

# --- 2-record bracket mode (records_per_sample=2) ---------------------------
# Mirrors the Python `Provider._refresh_bracket`: return the TWO records that
# bracket `t` (the floor tick + its successor) with `time_dim` RETAINED at length
# 2 and a canonical 2-element epoch-seconds `time_dim` coordinate, so a downstream
# model interpolates in time. The floor record is `_tick_index` (the same data
# index `_slice_dim` uses); the successor is the next data index along the axis,
# or — when that overruns the file — record 1 of the next file (`url_for(t_next)`,
# re-decoded since the Julia provider keeps no file buffer). At the last cadence
# tick there is no successor: the bracket degenerates to `[last, last]` (equal
# timestamps) so the downstream weight clamps — bracket mode never throws at the
# end of data.

# Parse a CF "<unit> since <reference>" units string → (ref_epoch_seconds,
# unit_seconds), or `nothing` when it can't be decoded (non-"since" units, an
# unknown step, or an unparseable reference) — the caller then emits raw times.
# The units are read as `decode_time_axis` reads them (time_axis.jl): any unit
# cftime knows, and the reference on the proleptic Gregorian calendar with its
# zone applied; the calendar attribute is not consulted.
function _cf_time_scale(units)
    units === nothing && return nothing
    sp = _ta_split_units(String(units))
    sp === nothing && return nothing
    step = get(_TA_CFTIME_UNITS, lowercase(sp[1]), nothing)
    step === nothing && return nothing
    r = _ta_reference(sp[2])
    r === nothing && return nothing
    ref = _ta_pandas_ns(r)
    ref === nothing && return nothing
    return (Float64(fld(ref, _TA_NS)) + Float64(mod(ref, _TA_NS)) / 1.0e9, step / 1.0e9)
end

# Convert a raw cadence-grid value to Unix epoch seconds via a decoded CF scale;
# with no scale (units absent/undecodable) fall back to the raw value (documented
# deviation from the epoch-seconds contract).
_raw_to_epoch(raw, scale) =
    scale === nothing ? Float64(raw) : (scale[1] + Float64(raw) * scale[2])

# The CF `units` string of the `dim` coordinate in `nds`, or `nothing`.
function _time_units(nds::NativeDataset, dim::String)
    haskey(nds.coords, dim) || return nothing
    return get(nds.coords[dim].attrs, "units", nothing)
end

# Length of `nds` along `dim` (from the coordinate, else the first variable
# carrying it); 0 if nothing carries it.
function _time_len(nds::NativeDataset, dim::String)
    if haskey(nds.coords, dim)
        c = nds.coords[dim]
        pos = findfirst(==(dim), c.dims)
        pos === nothing || return size(c.data, pos)
    end
    for f in values(nds.variables)
        pos = findfirst(==(dim), f.dims)
        pos === nothing || return size(f.data, pos)
    end
    return 0
end

# Stack record `i0` of `a0` and record `i1` of `a1` along axis `pos`, keeping that
# axis at length 2 (floor, then successor). Range-index each so the axis survives,
# then `cat` — this is uniform across same-file, cross-file, and degenerate cases.
_stack_bracket(a0, i0::Integer, a1, i1::Integer, pos::Integer) =
    collect(cat(selectdim(a0, pos, i0:i0), selectdim(a1, pos, i1:i1); dims = pos))

# Assemble the 2-record bracket: every variable carrying `dim` gets records
# (`nds0`,`i0`) and (`nds1`,`i1`) stacked to a size-2 `dim` axis (dims/order/attrs
# preserved); non-temporal variables and non-`dim` coords pass through from
# `nds0`; `dim` becomes a 2-element coordinate `[t0, t1]` in seconds since
# `units`' instant (the Unix epoch, else the provider's time origin).
function _bracket_build(
        nds0::NativeDataset, i0::Integer,
        nds1::NativeDataset, i1::Integer,
        dim::String, t0::Float64, t1::Float64,
        units::String = "seconds since 1970-01-01T00:00:00Z"
    )
    vars = Dict{String, NativeField}()
    for (name, f0) in nds0.variables
        pos = findfirst(==(dim), f0.dims)
        if pos === nothing
            vars[name] = f0                                  # non-temporal: unchanged
        else
            stacked = _stack_bracket(f0.data, i0, nds1.variables[name].data, i1, pos)
            vars[name] = NativeField(stacked, copy(f0.dims), f0.attrs)
        end
    end
    coords = Dict{String, NativeField}()
    for (k, v) in nds0.coords
        k == dim && continue                                 # replaced below
        coords[k] = v
    end
    coords[dim] = NativeField(
        Float64[t0, t1], [dim],
        Dict{String, Any}(
            "units" => units,
            "calendar" => "standard"
        )
    )
    return NativeDataset(vars, coords)
end

function _bracket(p::Provider, t::Real; select = nothing)
    dim = p.time_dim::String
    tick = _tick_index(p, t)                 # floor tick on the GLOBAL cadence
    # Resolve the URL at the TICK time, not at the query time `t`: the record we
    # are about to slice is the tick's, so it must come from the tick's file. The
    # two agree for any resolver that is constant across a file's own span, but
    # only the tick time is guaranteed to name the file that holds the record.
    u0 = p.url_for(p.times[tick])
    last_tick = tick >= length(p.times)      # no successor → degenerate bracket
    same_file = !last_tick && p.url_for(p.times[tick + 1]) == u0
    # ONE read covers the same-file bracket: both records come out of the same
    # decode, and with a `records` pushdown that decode materialises those two
    # records and no others. The end-of-data and cross-file cases ask this file
    # for the floor record alone — the successor is either absent (it degenerates
    # to the floor) or in the next file, read below.
    ticks0 = same_file ? Int[tick, tick + 1] : Int[tick]
    nds0, pos0 = _load_ticks(p, ticks0; select = select)
    scale = _cf_time_scale(_time_units(nds0, dim))
    t0 = _raw_to_epoch(p.times[tick], scale)

    if last_tick                             # last tick / past end → degenerate
        return _bracket_build(nds0, pos0[1], nds0, pos0[1], dim, t0, t0)
    end

    t1 = _raw_to_epoch(p.times[tick + 1], scale)
    if same_file                             # successor in the same file
        return _bracket_build(nds0, pos0[1], nds0, pos0[2], dim, t0, t1)
    end
    # Successor: record 1 of the NEXT file, re-decoded because the provider keeps
    # no file buffer (and, with the pushdown, one record of it rather than all).
    nds1, pos1 = _load_ticks(p, Int[tick + 1]; select = select)
    return _bracket_build(nds0, pos0[1], nds1, pos1[1], dim, t0, t1)
end

# --- a `temporal` provider: records found by the file's own time axis --------
# spec/esm-sources.md §2.5, the Python track's `Provider._sample` and
# `_sample_bracket` step for step. The record of an anchor in its file is
# `_axis_record`: by the file's decoded time axis when the anchor lies in its
# span, else the cadence offset. A trimmed or irregular file is read right.

# The record anchor `t` (seconds from the time origin, Unix seconds without
# one) snaps to, as a `DateTime`, or the error for a `t` before the source start.
function _temporal_anchor(p::Provider, t::Real)
    s = p.plan.temporal
    a = record_anchor(s, _instant(p.plan, t))
    a < s.start && throw(
        ArgumentError("t=$t precedes the source start $(s.start)")
    )
    return a
end

# 1-based record `rec + 1` of a file `len` long along `dim`, or a clear error
# when the calendar puts it outside the file (`len <= 0`: the blob has no such
# axis, and the slice passes through as for any provider).
function _calendar_record(rec::Integer, len::Integer, dim::String)
    (len <= 0 || 0 <= rec < len) || throw(
        ArgumentError(
            "cadence record $rec out of range for dimension '$dim' (length $len)"
        )
    )
    return Int(rec) + 1
end

# The decoded time axis of the file at file anchor `fa`, if one of the last two
# files read: `(true, times)`, else `(false, nothing)`.
function _axis_cached(plan::_TemporalPlan, fa::DateTime)
    return lock(plan.lock) do
        i = findfirst(e -> e.first == fa, plan.axes)
        i === nothing ? (false, nothing) : (true, plan.axes[i].second)
    end
end

function _axis_store!(plan::_TemporalPlan, fa::DateTime, times)
    lock(plan.lock) do
        filter!(e -> e.first != fa, plan.axes)
        push!(plan.axes, fa => times)
        length(plan.axes) > 2 && popfirst!(plan.axes)
    end
    return times
end

# Decode the file at file anchor `fa` (the one covering `anchor`), letting
# `choose(len, times)` pick its 0-based records from the file's length along
# `time_dim` and its decoded time axis (`nothing` when it has none or it does not
# decode); return `(nds, positions)`, `positions[k]` being where the k-th chosen
# record sits along `time_dim` in `nds`. With a reader that takes a `records`
# pushdown, `choose` runs inside the reader's one open and only the chosen
# records are decoded; the axis is read there too, unless this file's is cached.
function _load_located(p::Provider, fa::DateTime, anchor::DateTime, choose; select = nothing)
    plan = p.plan::_TemporalPlan
    dim = p.time_dim::String
    t_url = _tick(plan, anchor)
    cached, times = _axis_cached(plan, fa)
    decode(axis) = _axis_store!(plan, fa, axis === nothing ? nothing : decode_time_axis(axis))
    if !_records_pushdown(p)
        nds = _load(p, t_url; select = select)
        cached || (times = decode(get(nds.coords, dim, nothing)))
        idx = choose(_time_len(nds, dim), times)
        return (nds, Int[i + 1 for i in idx])
    end
    fired = Ref(false)
    n = Ref(0)
    pick = function (len, ts)
        fired[] = true
        r = choose(Int(len), ts)
        n[] = length(r)
        return r
    end
    # A one-argument resolver is not handed the axis, so the reader skips it.
    resolve = cached ? (len::Integer -> pick(len, times)) :
        ((len::Integer, axis) -> pick(len, decode(axis)))
    nds = _load(
        p, t_url; select = select,
        records = Dict{String, Any}("dim" => dim, "indices" => resolve)
    )
    fired[] && return (nds, collect(1:n[]))
    # No `time_dim` in the blob: nothing was narrowed (as `_load_ticks`).
    return (nds, Int[i + 1 for i in choose(0, nothing)])
end

# Thrown by a successor's record chooser when that record is outside its file,
# so the reader stops before decoding anything; the bracket then holds.
struct _NoSuccessor <: Exception end

function _temporal_sample(p::Provider, t::Real; select = nothing)
    plan = p.plan::_TemporalPlan
    temporal = plan.temporal
    dim = p.time_dim::String
    anchor = _temporal_anchor(p, t)
    fa = file_anchor(temporal, anchor)
    if p.records_per_sample != 2
        nds, pos = _load_located(
            p, fa, anchor,
            (len, times) -> (
                r = _axis_record(temporal, times, anchor, fa);
                _calendar_record(r, len, dim);
                Int[r]
            );
            select = select
        )
        return _slice_dim(nds, dim, pos[1])
    end
    # The bracket: the record of `anchor` and the record of the next anchor, each
    # found by its own file's time axis, so a record and its time agree even in a
    # file with a gap or an unsorted axis. With no successor (the end of the
    # data, a successor file that cannot be read, or a successor record outside
    # its file) the floor record is held: `[rec0, rec0]` with equal times.
    nxt = next_record_anchor(temporal, anchor)
    has_succ = temporal.stop === nothing || nxt < temporal.stop
    fa1 = file_anchor(temporal, nxt)
    same_file = has_succ && fa1 == fa
    held = Ref(true)
    choose0 = function (len, times)
        r = _axis_record(temporal, times, anchor, fa)
        _calendar_record(r, len, dim)
        if same_file
            r1 = _axis_record(temporal, times, nxt, fa)
            if 0 <= r1 < len
                held[] = false
                return r1 == r ? Int[r] : Int[r, r1]
            end
        end
        return Int[r]
    end
    nds0, pos0 = _load_located(p, fa, anchor, choose0; select = select)
    units = plan.clock.units
    t0 = _tick(plan, anchor)
    t1 = _tick(plan, nxt)
    if same_file && !held[]
        return _bracket_build(nds0, pos0[1], nds0, pos0[end], dim, t0, t1, units)
    end
    if has_succ && !same_file
        succ = try
            choose1 = function (len, times)
                r = _axis_record(temporal, times, nxt, fa1)
                0 <= r < len || throw(_NoSuccessor())
                return Int[r]
            end
            nds1, pos1 = _load_located(p, fa1, nxt, choose1; select = select)
            (nds1, pos1[1])
        catch err
            err isa InterruptException && rethrow()
            nothing
        end
        succ === nothing ||
            return _bracket_build(nds0, pos0[1], succ[1], succ[2], dim, t0, t1, units)
    end
    return _bracket_build(nds0, pos0[1], nds0, pos0[1], dim, t0, t0, units)
end

# The tick value `sample_at(p, t)` resolves: the record anchor (seconds from the
# time origin) for a `temporal` provider, else the cadence tick in effect at
# `t`. Raises what `sample_at(p, t)` raises for a `t` before the first one.
_sample_anchor(p::Provider, t::Real) =
    p.plan === nothing ? p.times[_tick_index(p, t)] :
    _tick(p.plan, _temporal_anchor(p, t))

# The file times (ticks, one per distinct URL) that `sample_at(p, t)` reads: the
# anchor's file and, for a two-record bracket, the successor's.
function _sample_url_times(p::Provider, t::Real)
    if p.plan === nothing
        tick = _tick_index(p, t)
        ts = Float64[p.times[tick]]
        if p.records_per_sample == 2 && p.time_dim !== nothing && tick < length(p.times)
            push!(ts, p.times[tick + 1])
        end
        return ts
    end
    s = p.plan.temporal
    a = _temporal_anchor(p, t)
    ts = Float64[_tick(p.plan, a)]
    if p.records_per_sample == 2
        nxt = next_record_anchor(s, a)
        (s.stop === nothing || nxt < s.stop) && push!(ts, _tick(p.plan, nxt))
    end
    return ts
end

"""
    materialize(p::Provider, t::Real; select=nothing) -> NativeDataset
    materialize(p::Provider; select=nothing) -> NativeDataset

Return the native arrays for the source at time `t`. For a [`DISCRETE`] provider
with `time_dim`, the internal cadence axis is sliced to `t`'s record — unless
`records_per_sample=2`, in which case the two bracketing records (floor +
successor) are returned with `time_dim` retained at length 2 and a 2-element
epoch-seconds `time_dim` coordinate (see the [`Provider`] `records_per_sample`
field). The no-argument form is for a [`CONST`] provider (a `DISCRETE` provider
must be given a time).

`select` is an optional PER-CALL projection pushdown (in EarthSciIO's native
select shape, e.g. `Dict("axes"=>[...])` with 0-based indices). When supplied it
OVERRIDES any baked `reader_kwargs[:select]` for this call only, letting a caller
(EarthSciAST) push a projection down at sample time without rebuilding the
provider. Any reader that answers [`supports_selection`] can honour it — the
`zarr` reader by fetching only the intersecting chunk objects, the `netcdf` one by
materialising only the requested hyperslab of the blob it already fetched;
passing `select` to a reader that cannot is an `ArgumentError`."""
function materialize(p::Provider, t::Real; select = nothing)
    p.plan === nothing || return _temporal_sample(p, t; select = select)
    if p.records_per_sample == 2 && p.time_dim !== nothing
        return _bracket(p, t; select = select)
    end
    if p.time_dim !== nothing
        tick = _tick_index(p, t)
        # The TICK's file (see `_bracket`), and — where the reader takes a
        # `records` pushdown — only the tick's record out of it.
        nds, pos = _load_ticks(p, Int[tick]; select = select)
        return _slice_dim(nds, p.time_dim, pos[1])
    end
    # A DISCRETE source with one file per tick resolves its URL at the TICK too,
    # not at the query time: the value at `t` is the value of the tick in effect,
    # so it depends on `t` only through that anchor (spec/prefetch.md §1 — the
    # property `PrefetchGroup` relies on to decode a coarse member once per
    # anchor) and a `t` before the first tick raises as it does with a
    # `time_dim`. A CONST source has no ticks and reads its one URL.
    p.cadence == DISCRETE && return _load(p, p.times[_tick_index(p, t)]; select = select)
    return _load(p, t; select = select)
end

function materialize(p::Provider; select = nothing)
    p.cadence == CONST || throw(
        ArgumentError(
            "a DISCRETE provider needs a time: materialize(p, t) / refresh(p, t)"
        )
    )
    return materialize(p, 0.0; select = select)
end

"""
    refresh(p::Provider, t::Real; select=nothing) -> NativeDataset

Re-materialize at the cadence tick `t`. This is the call the solver makes from
its `PresetTimeCallback` at each of [`refresh_times`]`(p)`. Identical to
[`materialize`]`(p, t)`; named for the solver-side update site. `select` is the
per-call projection pushdown override (see [`materialize`])."""
refresh(p::Provider, t::Real; select = nothing) = materialize(p, t; select = select)

"""
    sample_at(p::Provider, t::Real) -> NativeDataset
    sample_at(p::Provider) -> NativeDataset

The dataset `refresh(p, t)` returns — data variables AND coordinates, with the same
snap-down to the cadence anchor, the same file-local record, the same
`records_per_sample = 2` bracket and the same end-of-data rules — and it raises
what `refresh(p, t)` raises (spec/prefetch.md §1). It depends on `t` only through
the anchor `t` snaps to, so two times in one cadence interval give equal samples.
The no-argument form is a [`CONST`] provider's [`materialize`]`(p)`.

It is safe to call from several threads at once on one provider, which is what
[`PrefetchGroup`] does. In the Python and Rust tracks `sample_at` is new because
their `refresh` updates a decoded-file buffer; the Julia [`Provider`] holds no
buffer at all (immutable, decoding per sample with its `records` pushdown), so
here it is the same function as [`materialize`], under the name the cross-track
contract gives it. The shared state it does touch is thread-safe: the cache takes
a per-blob lock to fill a miss, and every NetCDF-C access is serialised by one
process-wide lock."""
sample_at(p::Provider, t::Real) = materialize(p, t)
sample_at(p::Provider) = materialize(p)

"""
    prefetch(p::Provider) -> Vector{CacheEntry}

Warm the cache for every URL the provider will need — the single URL for a
[`CONST`] provider, the unique per-tick URLs for a [`DISCRETE`] one — WITHOUT
decoding. Lets a caller pull all blobs up front (e.g. before a solve, or while
online) so later [`materialize`]/[`refresh`] calls hit a warm, offline-readable
cache. Returns the [`CacheEntry`] for each."""
function prefetch(p::Provider)
    ts = Float64[]
    seen = Set{String}()
    for t in (p.cadence == CONST ? Float64[0.0] : p.plan === nothing ? p.times : _plan_file_times(p.plan, p.records_per_sample == 2))
        u = p.url_for(t)
        u in seen && continue
        push!(seen, u)
        push!(ts, t)
    end
    return CacheEntry[_fetch_at(p, t) for t in ts]
end

# A `temporal` provider's files over its window, as Python's and Rust's
# `prefetch`: each file anchor (seconds from the time origin, or Unix seconds
# when none) from the one at or before `lo`
# while before `hi`, then the files of the records past the window it needs
# (`_end_records`): the record at `hi`, which `refresh_times_closed` refreshes
# at, and a two-record bracket's successor (spec/esm-sources.md §2.6).
function _plan_file_times(plan::_TemporalPlan, bracket::Bool = false)
    s, F = plan.temporal, plan.temporal.file_period
    k = period_floor(F, s.start, plan.lo)
    out = Float64[]
    a = period_anchor(F, s.start, k)
    while a < plan.hi
        push!(out, _tick(plan, a))
        k += 1
        a = period_anchor(F, s.start, k)
    end
    for r in _end_records(plan, bracket)
        fa = _tick(plan, file_anchor(s, r))
        (isempty(out) || last(out) != fa) && push!(out, fa)
    end
    return out
end
