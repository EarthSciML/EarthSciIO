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
end

function Provider(cache::Cache, url; format::AbstractString,
                  cadence::Cadence = CONST, times = Float64[],
                  time_dim = nothing, variables = nothing,
                  reader_kwargs = NamedTuple(),
                  source_loader = nothing, auth_realm = nothing)
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
    url_for = url isa AbstractString ? (let u = String(url); _ -> u; end) : url
    return Provider(cache, String(format), cadence, tvec, url_for,
                    time_dim === nothing ? nothing : String(time_dim),
                    variables === nothing ? nothing : String.(collect(variables)),
                    Dict{Symbol,Any}(pairs(reader_kwargs)),
                    source_loader === nothing ? nothing : String(source_loader),
                    auth_realm === nothing ? nothing : String(auth_realm))
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

"""
    materialize(p::Provider, t::Real) -> NativeDataset
    materialize(p::Provider) -> NativeDataset

Return the native arrays for the source at time `t`. For a [`DISCRETE`] provider
with `time_dim`, the internal cadence axis is sliced to `t`'s record. The
no-argument form is for a [`CONST`] provider (a `DISCRETE` provider must be
given a time)."""
function materialize(p::Provider, t::Real)
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
