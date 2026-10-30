# Named-dimension windows on a Provider (spec/prefetch.md §2): `DimWindow`, the
# Julia constructor for the `select = {"dims": {...}}` wire form; `with_window`,
# which BAKES a window into a new provider at construction; and
# `sample_windows_at`, which decodes K windows' bounding union once and slices
# each window back out by name.
#
# A window is a plain selection. It knows nothing of ranks, halos or MPI: which
# rank reads which block, and rebasing the model's index offsets to the window
# origin, are the integrator's (spec/prefetch.md §4).

"""
    DimWindow("lat" => (120, 218), "lon" => [1151, 0, 1], "lev" => "all")
    DimWindow(Dict("lat" => (120, 218), ...))

A named-dimension window: the Julia constructor for the wire form
`select = {"dims": {...}}` (spec/prefetch.md §2.2). Each pair names a dimension and
a selector, in the WIRE vocabulary — **0-based**, not Julia's:

  * `(start, stop)` — the half-open slice `start, …, stop-1`;
  * `(start, stop, step)` — the same, every `step`-th (`step >= 1`);
  * a `Vector` of integers — those indices, in that order, repeats kept (`[]` is a
    legal empty selection: a zero-length axis);
  * `"all"` (or `:all`) — the whole dimension;
  * a wire selector `Dict("slice" => [...])` / `Dict("indices" => [...])` as is.

A Julia range (`120:217`) is refused rather than guessed at: whether its end is
inclusive and whether it counts from 1 is exactly the off-by-one a window must not
be able to carry silently. Spell the slice `(120, 218)`.

The named form applies each selector to EVERY field and coordinate carrying that
dimension, whatever its rank, and leaves unnamed dimensions whole — so one window
covers a 2-D `ps(time, lat, lon)`, a 3-D `t(time, lev, lat, lon)` and a staggered
`u(time, lat, lon_edge)`. [`select_form`] gives the wire `Dict`; [`with_window`],
[`sample_windows_at`] and `PrefetchGroup(...; windows)` take either. (`Window`
already names the run window `(start, end)` in the Python and Rust tracks, hence
`DimWindow` in every binding.)"""
struct DimWindow
    dims::Vector{Pair{String, Any}}      # dim => wire selector, in the order given
    function DimWindow(dims::Vector{Pair{String, Any}})
        seen = Set{String}()
        for (d, _) in dims
            d in seen && throw(ArgumentError("DimWindow names dimension '$d' twice"))
            push!(seen, d)
        end
        return new(dims)
    end
end

DimWindow(ps::Pair...) =
    DimWindow(Pair{String, Any}[String(d) => _wire_selector(String(d), s) for (d, s) in ps])
DimWindow(d::AbstractDict) = DimWindow(pairs(d)...)

function Base.show(io::IO, w::DimWindow)
    print(io, "DimWindow(")
    join(io, (string(repr(d), " => ", repr(s)) for (d, s) in w.dims), ", ")
    return print(io, ")")
end

Base.:(==)(a::DimWindow, b::DimWindow) = select_form(a) == select_form(b)
Base.hash(w::DimWindow, h::UInt) = hash(select_form(w), hash(DimWindow, h))

# One `DimWindow` selector -> its wire spelling. Checked here, at construction, so
# a malformed window fails where it is written rather than inside a worker decode.
function _wire_selector(dim::String, s)
    (s == "all" || s === :all) && return "all"
    if s isa Tuple
        (length(s) in (2, 3) && all(x -> x isa Integer, s)) || throw(
            ArgumentError(
                "DimWindow selector for '$dim': a tuple is a 0-based half-open slice " *
                    "(start, stop) or (start, stop, step) of integers, got $(repr(s))"
            )
        )
        length(s) == 3 && s[3] < 1 && throw(
            ArgumentError(
                "DimWindow selector for '$dim': slice step must be >= 1, got $(s[3])"
            )
        )
        return Dict{String, Any}("slice" => Int[Int(x) for x in s])
    elseif s isa AbstractRange
        throw(
            ArgumentError(
                "DimWindow selector for '$dim': a Julia range ($(repr(s))) is ambiguous " *
                    "about its base and its end; a window is 0-based and half-open, so spell " *
                    "the slice as a tuple (start, stop) or list the indices"
            )
        )
    elseif s isa AbstractVector
        all(x -> x isa Integer, s) || throw(
            ArgumentError(
                "DimWindow selector for '$dim': an index list must hold integers, got $(repr(s))"
            )
        )
        return Dict{String, Any}("indices" => Int[Int(x) for x in s])
    elseif s isa AbstractDict
        ks = Set(String(k) for k in keys(s))
        (ks == Set(["slice"]) || ks == Set(["indices"])) || throw(
            ArgumentError(
                "DimWindow selector for '$dim': a wire selector is {\"slice\": [...]} or " *
                    "{\"indices\": [...]}, got keys $(sort!(collect(ks)))"
            )
        )
        k = only(ks)
        v = Int[Int(x) for x in s[k]]
        k == "slice" && !(length(v) in (2, 3)) && throw(
            ArgumentError(
                "DimWindow selector for '$dim': a slice is [start, stop] or [start, stop, step]"
            )
        )
        return Dict{String, Any}(k => v)
    end
    throw(
        ArgumentError(
            "DimWindow selector for '$dim' must be (start, stop[, step]), a vector of " *
                "indices, \"all\", or a wire selector Dict; got $(repr(s))"
        )
    )
end

"""
    select_form(w::DimWindow) -> Dict{String,Any}
    select_form(sel::AbstractDict) -> Dict{String,Any}

The wire form of a window, `Dict("dims" => Dict(dim => selector, ...))` — what a
reader's `select` option and `reader_kwargs[:select]` take (spec/prefetch.md
§2.2). Given a wire `Dict` already, it is validated and normalised: it must be the
named `dims` form, since a positional `axes` list cannot be applied by name and so
is not a window."""
select_form(w::DimWindow) =
    Dict{String, Any}("dims" => Dict{String, Any}(d => s for (d, s) in w.dims))

function select_form(sel::AbstractDict)
    ks = Set(String(k) for k in keys(sel))
    ks == Set(["dims"]) || throw(
        ArgumentError(
            "a window is the named form {\"dims\": {...}} (a DimWindow); got a select " *
                "with keys $(sort!(collect(ks))) — a positional `axes` select cannot be " *
                "applied by name, and a select carrying both is an error"
        )
    )
    named = sel[first(k for k in keys(sel) if String(k) == "dims")]
    named isa AbstractDict || throw(
        ArgumentError(
            "select[\"dims\"] must map dimension names to selectors, got $(typeof(named))"
        )
    )
    return select_form(DimWindow(named))
end

select_form(x) = throw(
    ArgumentError(
        "a window is a DimWindow or a wire Dict(\"dims\" => ...), got $(typeof(x))"
    )
)

# A DimWindow wherever a reader parses a `select` (e.g. baked straight into
# `reader_kwargs[:select]`) means its wire form.
_select_form(w::DimWindow) = _select_form(select_form(w))

# The checks `with_window` makes — before any I/O — and `sample_windows_at` and
# `PrefetchGroup` repeat for every window (spec/prefetch.md §2.3).
function _check_window(p::Provider, sel::Dict{String, Any})
    get(p.reader_kwargs, :select, nothing) === nothing || throw(
        ArgumentError(
            "this provider already bakes a `select` into its reader_kwargs; two windows " *
                "do not compose — build the windowed provider from one without a select"
        )
    )
    supports_selection(p) || throw(
        ArgumentError(
            "the '$(p.format)' reader does not support selection (`supports_selection` " *
                "is false), so it cannot take a window"
        )
    )
    if p.time_dim !== nothing && haskey(sel["dims"], p.time_dim)
        throw(
            ArgumentError(
                "the window names '$(p.time_dim)', this provider's time_dim; records are " *
                    "the Provider's to choose (its cadence and records_per_sample), never a " *
                    "window's"
            )
        )
    end
    return nothing
end

# `p` with `reader_kwargs` replaced — sharing its cache, reader, resolver and
# cadence. Bypasses the constructor's checks, which `p` already passed and which a
# `select` cannot change.
_with_reader_kwargs(p::Provider, kw::Dict{Symbol, Any}) =
    Provider(
    p.cache, p.format, p.cadence, p.times, p.url_for, p.time_dim,
    p.variables, kw, p.source_loader, p.auth_realm, p.records_per_sample
)

function _with_select(p::Provider, sel::Dict{String, Any})
    kw = copy(p.reader_kwargs)
    kw[:select] = sel
    return _with_reader_kwargs(p, kw)
end

"""
    with_window(p::Provider, w) -> Provider

A NEW provider that shares `p`'s cache, reader, URL resolver and cadence and has
the window `w` (a [`DimWindow`] or its wire `Dict`) baked in as
`reader_kwargs[:select]`. `p` is unchanged. Raised at once, before any I/O, when
`p` already bakes a `select` (two windows do not compose), when its reader does not
answer [`supports_selection`], or when `w` names `p`'s `time_dim` — records are the
Provider's (spec/prefetch.md §2.3).

A window is bound at construction, never per call: the per-call
`materialize(p, t; select = …)` stays a peek. Everything else about the window —
a named dimension the read lacks, a selector out of range, a non-`"all"` time
dimension — is the reader's to refuse, at the first sample."""
function with_window(p::Provider, w)
    sel = select_form(w)
    _check_window(p, sel)
    return _with_select(p, sel)
end

"""
    sample_windows_at(p::Provider, t::Real, windows) -> Vector{NativeDataset}
    sample_windows_at(p::Provider, windows) -> Vector{NativeDataset}

K windows at once: dataset `i` is identical — bitwise, coordinates included — to
`sample_at(with_window(p, windows[i]), t)`, but the source is fetched and decoded
ONCE, over the windows' bounding union (spec/prefetch.md §2.4). Per dimension the
union is the sorted distinct union of the windows' indices, or the whole dimension
when any window leaves it unnamed or names it `"all"`; each window is then sliced
back out of the union by name, in its own order and with its own repetitions.

This is for chunk layouts a window cannot shrink (GEOS-FP 0.25° A3dyn is chunked
one global level per chunk): one rank per node decodes its neighbours' windows in
one pass and the integrator scatters them. The `with_window` preconditions apply
to every window, before any I/O; an empty `windows` returns `NativeDataset[]`
without fetching anything. The two-argument form is for a [`CONST`] provider."""
function sample_windows_at(p::Provider, t::Real, windows)
    sels = Dict{String, Any}[select_form(w) for w in windows]
    isempty(sels) && return NativeDataset[]
    foreach(s -> _check_window(p, s), sels)
    named = [_select_form(s)[2] for s in sels]
    union_sel, union_idx = _window_union(named)
    u = sample_at(_with_select(p, union_sel), t)
    return NativeDataset[_slice_window(u, nm, union_idx) for nm in named]
end

function sample_windows_at(p::Provider, windows)
    p.cadence == CONST || throw(
        ArgumentError(
            "a DISCRETE provider needs a time: sample_windows_at(p, t, windows)"
        )
    )
    return sample_windows_at(p, 0.0, windows)
end

# A parsed selector's 0-based indices, WITHOUT a bounds check (no length is known
# before the read). Only used to build and look up the union; every index it
# yields is bounds-checked by the reader when the union is read, because the union
# names it.
function _window_indices(ax)
    ax[1] === :indices && return ax[2]
    _, start, stop, step = ax
    step >= 1 || error("slice step must be >= 1, got $step")
    return collect(start:step:(stop - 1))
end

# The bounding union of K parsed named windows, as a wire `select` plus, for each
# dimension the union narrows, its sorted distinct 0-based indices.
#
# EVERY dimension any window names is named in the union — `"all"` where the union
# is the whole dimension — so the reader checks each against the fields of the read
# exactly as it would for that window alone: a window naming a dimension the read
# lacks fails the union read with the reader's own error.
function _window_union(named::Vector{Dict{String, Any}})
    wire = Dict{String, Any}()
    union_idx = Dict{String, Vector{Int}}()
    for d in sort!(unique!([k for nm in named for k in keys(nm)]))
        if all(nm -> haskey(nm, d) && nm[d][1] !== :all, named)
            idx = sort!(
                unique!(
                    reduce(
                        vcat, (_window_indices(nm[d]) for nm in named);
                        init = Int[]
                    )
                )
            )
            union_idx[d] = idx
            wire[d] = Dict{String, Any}("indices" => idx)
        else
            wire[d] = "all"
        end
    end
    return Dict{String, Any}("dims" => wire), union_idx
end

# Window `nm` sliced out of the union read `u`. A dimension the union narrowed is
# looked up in its sorted index list (every index of `nm` is in it, and the reader
# bounds-checked them all). A dimension the union read WHOLE was not checked for
# this window, so the reader's own checks run here against the decoded length,
# which is the file length: a non-`"all"` time dimension is refused and every index
# is bounds-checked by the same `_resolve_axis`, with the same error.
function _slice_window(
        u::NativeDataset, nm::Dict{String, Any},
        union_idx::Dict{String, Vector{Int}}
    )
    pos = Dict{String, Vector{Int}}()                 # 1-based positions into `u`
    for (d, ax) in nm
        ax[1] === :all && continue
        if haskey(union_idx, d)
            ui = union_idx[d]
            pos[d] = Int[searchsortedfirst(ui, i) for i in _window_indices(ax)]
        else
            units = haskey(u.coords, d) ? get(u.coords[d].attrs, "units", nothing) : nothing
            _is_time_dim_named(d, units) && throw(
                ArgumentError(
                    "select asks for a subset of the time dimension '$d'; record " *
                        "selection is the Provider's (it owns the cadence: " *
                        "`records_per_sample`), not the reader's — a time dimension may only " *
                        "be \"all\""
                )
            )
            pos[d] = _resolve_axis(ax, _dataset_dim_length(u, d)) .+ 1
        end
    end
    slice(f::NativeField) = any(d -> haskey(pos, d), f.dims) ?
        NativeField(
            f.data[(haskey(pos, d) ? pos[d] : Colon() for d in f.dims)...],
            copy(f.dims), copy(f.attrs)
        ) :
        NativeField(copy(f.data), copy(f.dims), copy(f.attrs))
    return NativeDataset(
        Dict{String, NativeField}(n => slice(f) for (n, f) in u.variables),
        Dict{String, NativeField}(n => slice(f) for (n, f) in u.coords)
    )
end

# The length of `nds` along dimension `d`, from whichever field carries it (the
# union read named `d`, so the reader has already proved one does).
function _dataset_dim_length(nds::NativeDataset, d::String)
    for f in Iterators.flatten((values(nds.coords), values(nds.variables)))
        k = findfirst(==(d), f.dims)
        k === nothing || return size(f.data, k)
    end
    throw(ArgumentError("no field of the read carries dimension '$d'"))
end
