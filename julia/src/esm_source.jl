# A provider from an esm `data_sources` entry (spec/esm-sources.md §3).
#
# `provider_from_esm` reads the parts of an entry that locate and decode files —
# `source`, `temporal`, `reader_options` and the reader name — into a
# `Provider`. What a source's values MEAN to the model (`select`, `codes`,
# `record_filter`, `extent`, a parameter's `unit_conversion`) is the document
# binding's job (EarthSciAST), which calls this for the bytes.

"""
    EsmSourceError(code, source, detail)

An esm data source EarthSciIO cannot read. `code` is one of the `esm_source_*`
codes of spec/esm-sources.md §3.3 (or `esm_period_invalid`); the message contains
it and the source's name."""
struct EsmSourceError <: Exception
    code::String
    source::String
    detail::String
end
Base.showerror(io::IO, e::EsmSourceError) =
    print(io, e.code, ": data source ", repr(e.source), ": ", e.detail)

const _URL_SCHEME = r"^[A-Za-z][A-Za-z0-9+.-]*://"

_get(d, k) = d isa AbstractDict ? get(d, k, nothing) : nothing

function _esm_check_url(name::String, t::String)
    occursin("\${", t) && throw(
        EsmSourceError(
            "esm_source_url", name,
            "$(repr(t)) names an environment variable, which esm-spec §8.2.1 refuses"
        )
    )
    (occursin(_URL_SCHEME, t) || startswith(t, "{")) || throw(
        EsmSourceError(
            "esm_source_url", name,
            "$(repr(t)) is not a URL; load the document with EarthSciAST, which resolves " *
                "a relative location against the file that declared it (esm-spec §8.2.1), " *
                "or pass url_override"
        )
    )
    return nothing
end

function _esm_instant(name::String, field::String, v)
    v isa AbstractString || throw(
        EsmSourceError("esm_source_temporal", name, "temporal.$field must be a string")
    )
    try
        return _parse_utc(v)
    catch
        throw(
            EsmSourceError(
                "esm_source_temporal", name,
                "temporal.$field $(repr(String(v))) is not an ISO 8601 instant"
            )
        )
    end
end

function _esm_period(name::String, field::String, v)
    try
        return CalendarPeriod(String(v))
    catch e
        e isa CalendarPeriodError || rethrow()
        throw(EsmSourceError("esm_period_invalid", name, "temporal.$field: $(e.msg)"))
    end
end

function _esm_temporal(name::String, node, records_per_sample)
    node === nothing && return nothing
    node isa AbstractDict || throw(
        EsmSourceError("esm_source_temporal", name, "temporal must be an object")
    )
    start = _get(node, "start")
    (start === nothing || start == "") && return nothing
    freq = _get(node, "frequency")
    fp = _get(node, "file_period")
    freq == "" && (freq = nothing)
    fp == "" && (fp = nothing)
    freq === nothing && fp === nothing && throw(
        EsmSourceError(
            "esm_source_temporal", name,
            "temporal anchors at $(repr(start)) but declares neither frequency nor file_period"
        )
    )
    f = _esm_period(name, "frequency", something(freq, fp))
    p = _esm_period(name, "file_period", something(fp, freq))
    st = _esm_instant(name, "start", start)
    stop = _get(node, "end")
    tv = _get(node, "time_variable")
    temporal = SourceTemporal(
        st, f, p;
        stop = (stop === nothing || stop == "") ? nothing : _esm_instant(name, "end", stop),
        time_dim = (tv === nothing || tv == "") ? "time" : String(tv),
        records_per_sample = records_per_sample
    )
    rpf = _get(node, "records_per_file")
    if rpf !== nothing && rpf != "auto"
        nxt = next_file_anchor(temporal, st)
        n, a = 0, first_record_anchor_from(temporal, st)
        while a < nxt
            n += 1
            a = next_record_anchor(temporal, a)
        end
        (rpf isa Integer && !(rpf isa Bool) && rpf == n) || throw(
            EsmSourceError(
                "esm_source_records_per_file", name,
                "temporal.records_per_file is $(repr(rpf)) but file_period / frequency " *
                    "gives $n records per file"
            )
        )
    end
    return temporal
end

"""
    provider_from_esm(name, entry, cache; variables, substitutions=Dict(),
                      url_override=nothing, records_per_sample=nothing,
                      window=nothing) -> Provider

A [`Provider`](@ref) over one esm `data_sources` entry (a parsed JSON object, as
a document carries it after EarthSciAST loaded it), bound to `cache` and the run
`window` (UTC: `DateTime`s or Unix seconds). See spec/esm-sources.md §3. A
time-varying source's ticks are Unix seconds."""
function provider_from_esm(
        name::AbstractString, entry, cache::Cache;
        variables = String[],
        substitutions::AbstractDict = Dict{String, String}(),
        url_override = nothing, records_per_sample = nothing, window = nothing
    )
    nm = String(name)
    entry isa AbstractDict || throw(EsmSourceError("esm_source_invalid", nm, "the entry must be an object"))
    fmt = _get(_get(entry, "metadata"), "esio_format")
    (fmt isa AbstractString && !isempty(fmt)) || throw(
        EsmSourceError(
            "esm_source_no_format", nm,
            "declares no metadata.esio_format, so the reader is unknown"
        )
    )
    src = _get(entry, "source")
    template = url_override !== nothing ? url_override : _get(src, "url_template")
    template isa AbstractString || throw(
        EsmSourceError("esm_source_url", nm, "has no source.url_template")
    )
    template = String(template)
    ms = url_override !== nothing ? Any[] : something(_get(src, "mirrors"), Any[])
    mirrors = String[String(m) for m in ms]
    for t in (template, mirrors...)
        _esm_check_url(nm, t)
    end
    vars = String[String(v) for v in variables]
    subs = Dict{String, String}(String(k) => string(v) for (k, v) in substitutions)
    needs = Set{String}()
    for t in (template, mirrors...)
        union!(needs, esm_template_names(t))
    end
    if "var" in needs && !haskey(subs, "var") && length(vars) == 1
        subs["var"] = vars[1]
    end
    ropts = something(_get(entry, "reader_options"), Dict{String, Any}())
    ropts isa AbstractDict || throw(
        EsmSourceError("esm_source_invalid", nm, "reader_options must be an object")
    )
    reader_kwargs = Dict{Symbol, Any}(Symbol(k) => v for (k, v) in ropts)
    url = EsmTemplate(template; substitutions = subs, mirrors = mirrors)
    temporal = _esm_temporal(nm, _get(entry, "temporal"), records_per_sample)
    common = (;
        format = String(fmt), variables = isempty(vars) ? nothing : vars,
        reader_kwargs = reader_kwargs, source_loader = nm,
    )
    temporal === nothing && return Provider(cache, url(0.0); common..., mirrors = esm_mirrors(url, 0.0))
    return Provider(cache, url; common..., temporal = temporal, window = window)
end
