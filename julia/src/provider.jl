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
    [`refresh_times`] returns.
  * `time_dim` — when set on a `DISCRETE` provider whose single file holds the
    whole cadence on an internal axis (e.g. a daily file of hourly steps),
    [`materialize`]`(p, t)` slices that dimension to the tick's record.
  * `records_per_sample` — `nothing` or `1` (default) returns the SINGLE
    at-or-before record with `time_dim` DROPPED (held piecewise-constant); `2`
    returns the TWO bracketing records (floor + successor) with `time_dim`
    RETAINED at length 2 and a canonical 2-element `time_dim` coordinate of Unix
    epoch seconds, so a downstream model interpolates in time. `2` requires a
    `time_dim`; the successor is read across a file boundary when needed, and at
    the last cadence tick the bracket degenerates to `[last, last]` (equal
    timestamps) so the downstream weight clamps. The provider does pure I/O
    (returns N records) and performs no interpolation itself.
  * `variables` — restrict the returned data variables (coords always kept);
    `nothing` returns all.
  * `reader_kwargs` — extra keywords forwarded to [`read_native`] (e.g. the CSV
    reader's `numeric_columns`).
  * `source_loader` / `auth_realm` — recorded in the cache manifest on fetch."""
struct Provider
    cache::Cache
    format::String
    cadence::Cadence
    times::Vector{Float64}
    url_for::Function
    time_dim::Union{Nothing,String}
    variables::Union{Nothing,Vector{String}}
    reader_kwargs::Dict{Symbol,Any}
    source_loader::Union{Nothing,String}
    auth_realm::Union{Nothing,String}
    records_per_sample::Union{Nothing,Int}
end

function Provider(cache::Cache, url; format::AbstractString,
                  cadence::Cadence = CONST, times = Float64[],
                  time_dim = nothing, variables = nothing,
                  reader_kwargs = NamedTuple(),
                  source_loader = nothing, auth_realm = nothing,
                  records_per_sample = nothing)
    haskey(FORMAT_REGISTRY, format) || throw(ArgumentError(
        "format '$format' is not registered in the format registry; " *
        "registered: $(registered_names(FORMAT_REGISTRY))"))
    tvec = sort!(Float64[t for t in times])
    if cadence == CONST && !isempty(tvec)
        throw(ArgumentError(
            "a CONST provider has no refresh times, got $(length(tvec)) — use cadence=DISCRETE"))
    elseif cadence == DISCRETE && isempty(tvec)
        throw(ArgumentError(
            "a DISCRETE provider requires a non-empty cadence (times=...)"))
    end
    time_dim === nothing || cadence == DISCRETE ||
        throw(ArgumentError("time_dim only applies to a DISCRETE provider"))
    records_per_sample === nothing || records_per_sample == 1 || records_per_sample == 2 ||
        throw(ArgumentError(
            "records_per_sample must be 1 or 2, got $(repr(records_per_sample))"))
    records_per_sample != 2 || time_dim !== nothing ||
        throw(ArgumentError(
            "records_per_sample=2 needs a time_dim to bracket along"))
    url_for = url isa AbstractString ? (let u = String(url); _ -> u; end) : url
    return Provider(cache, String(format), cadence, tvec, url_for,
                    time_dim === nothing ? nothing : String(time_dim),
                    variables === nothing ? nothing : String.(collect(variables)),
                    Dict{Symbol,Any}(pairs(reader_kwargs)),
                    source_loader === nothing ? nothing : String(source_loader),
                    auth_realm === nothing ? nothing : String(auth_realm),
                    records_per_sample === nothing ? nothing : Int(records_per_sample))
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
solver (e.g. `PresetTimeCallback(refresh_times(p), …)`)."""
refresh_times(p::Provider) = copy(p.times)

"""True if `p`'s data is time-invariant ([`CONST`])."""
is_const(p::Provider) = p.cadence == CONST

# Fetch (cache) + decode (format reader) for the URL resolved at `t`.
function _load(p::Provider, t)
    entry = fetch_blob(p.cache, p.url_for(t);
                       source_loader = p.source_loader, auth_realm = p.auth_realm)
    nds = read_native(FORMAT_REGISTRY[p.format], entry.path; p.reader_kwargs...)
    return p.variables === nothing ? nds : _select(nds, p.variables)
end

function _select(nds::NativeDataset, want::Vector{String})
    keep = Dict{String,NativeField}()
    for name in want
        haskey(nds.variables, name) || throw(ArgumentError(
            "requested variable '$name' not in blob; present: $(variable_names(nds))"))
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
    throw(ArgumentError(
        "t=$t precedes the provider's first refresh time $(first(p.times))"))
end

# Slice `dim` out of every variable that carries it, at record `idx`; drop the
# now-singular dimension and its coordinate. Used for an internal-axis DISCRETE
# source (one file, many time records).
function _slice_dim(nds::NativeDataset, dim::String, idx::Integer)
    vars = Dict{String,NativeField}()
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

const _CF_UNIT_SECONDS = Dict{String,Float64}(
    "second" => 1.0, "seconds" => 1.0, "sec" => 1.0, "secs" => 1.0, "s" => 1.0,
    "minute" => 60.0, "minutes" => 60.0, "min" => 60.0, "mins" => 60.0,
    "hour" => 3600.0, "hours" => 3600.0, "hr" => 3600.0, "hrs" => 3600.0, "h" => 3600.0,
    "day" => 86400.0, "days" => 86400.0, "d" => 86400.0)

# Parse a CF reference date (the `<ref>` in "<unit> since <ref>") → Unix epoch
# seconds, or `nothing`. Handles "yyyy-mm-dd[ HH[:MM[:SS]]]" with an optional
# `T`/space separator and a trailing `Z`/`UTC`/±offset (treated as UTC).
function _parse_cf_reference(s)
    str = strip(replace(String(s), 'T' => ' '))
    str = strip(replace(str, r"\s*(Z|UTC|[+-]\d{2}:?\d{2}(:\d{2})?)\s*$" => ""))
    for fmt in (dateformat"yyyy-mm-dd HH:MM:SS", dateformat"yyyy-mm-dd HH:MM",
                dateformat"yyyy-mm-dd HH", dateformat"yyyy-mm-dd")
        dt = tryparse(DateTime, str, fmt)
        dt === nothing || return datetime2unix(dt)
    end
    return nothing
end

# Parse a CF "<unit> since <reference>" units string → (ref_epoch_seconds,
# unit_seconds), or `nothing` when it can't be decoded (non-"since" units, an
# unknown step, or an unparseable reference) — the caller then emits raw times.
function _cf_time_scale(units)
    units === nothing && return nothing
    m = match(r"^\s*([A-Za-z]+)\s+since\s+(.+?)\s*$", String(units))
    m === nothing && return nothing
    step = get(_CF_UNIT_SECONDS, lowercase(m.captures[1]), nothing)
    step === nothing && return nothing
    ref = _parse_cf_reference(m.captures[2])
    ref === nothing && return nothing
    return (ref, step)
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
# `nds0`; `dim` becomes a 2-element epoch-seconds coordinate `[t0, t1]`.
function _bracket_build(nds0::NativeDataset, i0::Integer,
                        nds1::NativeDataset, i1::Integer,
                        dim::String, t0::Float64, t1::Float64)
    vars = Dict{String,NativeField}()
    for (name, f0) in nds0.variables
        pos = findfirst(==(dim), f0.dims)
        if pos === nothing
            vars[name] = f0                                  # non-temporal: unchanged
        else
            stacked = _stack_bracket(f0.data, i0, nds1.variables[name].data, i1, pos)
            vars[name] = NativeField(stacked, copy(f0.dims), f0.attrs)
        end
    end
    coords = Dict{String,NativeField}()
    for (k, v) in nds0.coords
        k == dim && continue                                 # replaced below
        coords[k] = v
    end
    coords[dim] = NativeField(Float64[t0, t1], [dim],
        Dict{String,Any}("units" => "seconds since 1970-01-01T00:00:00Z",
                         "calendar" => "standard"))
    return NativeDataset(vars, coords)
end

function _bracket(p::Provider, t::Real)
    dim = p.time_dim::String
    rec0 = _tick_index(p, t)                 # floor tick == floor data index
    nds0 = _load(p, t)
    scale = _cf_time_scale(_time_units(nds0, dim))
    L = _time_len(nds0, dim)
    t0 = _raw_to_epoch(p.times[rec0], scale)

    if rec0 >= length(p.times)               # last tick / past end → degenerate
        i0 = min(rec0, L)                     # clamp to a valid record (never throw)
        return _bracket_build(nds0, i0, nds0, i0, dim, t0, t0)
    end

    t1 = _raw_to_epoch(p.times[rec0 + 1], scale)
    if rec0 + 1 <= L                         # successor in the same file
        return _bracket_build(nds0, rec0, nds0, rec0 + 1, dim, t0, t1)
    end
    nds1 = _load(p, p.times[rec0 + 1])       # successor is record 1 of the next file
    return _bracket_build(nds0, min(rec0, L), nds1, 1, dim, t0, t1)
end

"""
    materialize(p::Provider, t::Real) -> NativeDataset
    materialize(p::Provider) -> NativeDataset

Return the native arrays for the source at time `t`. For a [`DISCRETE`] provider
with `time_dim`, the internal cadence axis is sliced to `t`'s record — unless
`records_per_sample=2`, in which case the two bracketing records (floor +
successor) are returned with `time_dim` retained at length 2 and a 2-element
epoch-seconds `time_dim` coordinate (see the [`Provider`] `records_per_sample`
field). The no-argument form is for a [`CONST`] provider (a `DISCRETE` provider
must be given a time)."""
function materialize(p::Provider, t::Real)
    if p.records_per_sample == 2 && p.time_dim !== nothing
        return _bracket(p, t)
    end
    nds = _load(p, t)
    if p.time_dim !== nothing
        return _slice_dim(nds, p.time_dim, _tick_index(p, t))
    end
    return nds
end

function materialize(p::Provider)
    p.cadence == CONST || throw(ArgumentError(
        "a DISCRETE provider needs a time: materialize(p, t) / refresh(p, t)"))
    return materialize(p, 0.0)
end

"""
    refresh(p::Provider, t::Real) -> NativeDataset

Re-materialize at the cadence tick `t`. This is the call the solver makes from
its `PresetTimeCallback` at each of [`refresh_times`]`(p)`. Identical to
[`materialize`]`(p, t)`; named for the solver-side update site."""
refresh(p::Provider, t::Real) = materialize(p, t)

"""
    prefetch(p::Provider) -> Vector{CacheEntry}

Warm the cache for every URL the provider will need — the single URL for a
[`CONST`] provider, the unique per-tick URLs for a [`DISCRETE`] one — WITHOUT
decoding. Lets a caller pull all blobs up front (e.g. before a solve, or while
online) so later [`materialize`]/[`refresh`] calls hit a warm, offline-readable
cache. Returns the [`CacheEntry`] for each."""
function prefetch(p::Provider)
    urls = p.cadence == CONST ? String[p.url_for(0.0)] :
           unique(String[p.url_for(t) for t in p.times])
    return CacheEntry[fetch_blob(p.cache, u; source_loader = p.source_loader,
                                 auth_realm = p.auth_realm) for u in urls]
end
