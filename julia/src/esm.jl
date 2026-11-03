# esm data-source URL templates (spec/esm-sources.md §1).
#
# An esm `source.url_template` (and every `source.mirrors` entry) is literal text
# with `{date:<format>}` and `{name}` tokens (esm-spec §8.2). This file expands
# one exactly as the Python and Rust tracks do: the date directives are
# implemented here, not handed to `Dates.format`, whose directive letters differ.

"""
    EsmTemplateError(code, template, detail)

An esm URL template could not be expanded. `code` is one of the
`esm_template_*` codes of spec/esm-sources.md §1.3; the message contains it."""
struct EsmTemplateError <: Exception
    code::String
    template::String
    detail::String
end

Base.showerror(io::IO, e::EsmTemplateError) =
    print(io, e.code, ": ", e.detail, " in esm URL template ", repr(e.template))

const _ESM_TOKEN = r"\{([A-Za-z_][A-Za-z0-9_]*)(?::([^{}]*))?\}"

_esm_anchor(::Nothing) = nothing
_esm_anchor(t::DateTime) = t
# Unix seconds. Fractional seconds are dropped toward -Inf, never rounded, so
# 23:59:59.999 stays in its second (spec §1.2 `%S`).
_esm_anchor(t::Real) = unix2datetime(0) + Second(floor(Int64, t))

function _esm_format_date(template::String, fmt::AbstractString, t::DateTime)
    io = IOBuffer()
    chars = collect(fmt)
    i = 1
    while i <= length(chars)
        c = chars[i]
        if c != '%'
            print(io, c)
            i += 1
            continue
        end
        i == length(chars) && throw(
            EsmTemplateError(
                "esm_template_directive", template,
                "a '%' ends the date format $(repr(String(fmt)))"
            )
        )
        d = chars[i + 1]
        if d == 'Y'
            y = year(t)
            0 <= y <= 9999 || throw(
                EsmTemplateError(
                    "esm_template_year_range", template,
                    "%Y of year $y is outside 0 to 9999"
                )
            )
            print(io, lpad(y, 4, '0'))
        elseif d == 'y'
            print(io, lpad(mod(year(t), 100), 2, '0'))
        elseif d == 'm'
            print(io, lpad(month(t), 2, '0'))
        elseif d == 'd'
            print(io, lpad(day(t), 2, '0'))
        elseif d == 'j'
            print(io, lpad(dayofyear(t), 3, '0'))
        elseif d == 'H'
            print(io, lpad(hour(t), 2, '0'))
        elseif d == 'M'
            print(io, lpad(minute(t), 2, '0'))
        elseif d == 'S'
            print(io, lpad(second(t), 2, '0'))
        elseif d == '%'
            print(io, '%')
        else
            throw(
                EsmTemplateError(
                    "esm_template_directive", template,
                    "unsupported date directive %$d (supported: %Y %y %m %d %j %H %M %S %%)"
                )
            )
        end
        i += 2
    end
    return String(take!(io))
end

"""
    expand_esm_template(template, anchor=nothing; substitutions=Dict()) -> String

Expand an esm URL template (spec/esm-sources.md §1) at `anchor` — a UTC
`DateTime`, Unix seconds, or `nothing` — with `substitutions` (name => string).
Errors are [`EsmTemplateError`](@ref)s carrying a §1.3 code."""
function expand_esm_template(
        template::AbstractString, anchor = nothing;
        substitutions::AbstractDict = Dict{String, String}()
    )
    tmpl = String(template)
    subs = Dict{String, String}(String(k) => string(v) for (k, v) in substitutions)
    haskey(subs, "date") && throw(
        EsmTemplateError(
            "esm_template_reserved", tmpl,
            "the substitution name 'date' is reserved for {date:<format>}"
        )
    )
    when = _esm_anchor(anchor)
    io = IOBuffer()
    last = 1
    for m in eachmatch(_ESM_TOKEN, tmpl)
        print(io, SubString(tmpl, last, prevind(tmpl, m.offset)))
        name = m.captures[1]
        fmt = m.captures[2]
        if name == "date"
            (fmt === nothing || isempty(fmt)) && throw(
                EsmTemplateError(
                    "esm_template_directive", tmpl,
                    "{date} needs a format, e.g. {date:%Y%m%d}"
                )
            )
            when === nothing && throw(
                EsmTemplateError(
                    "esm_template_no_anchor", tmpl,
                    "{date:...} needs a time and none was given"
                )
            )
            print(io, _esm_format_date(tmpl, fmt, when))
        else
            fmt === nothing || throw(
                EsmTemplateError(
                    "esm_template_directive", tmpl,
                    "the substitution {$name} takes no format (got {$name:$fmt})"
                )
            )
            haskey(subs, name) || throw(
                EsmTemplateError(
                    "esm_template_unbound", tmpl,
                    "the substitution {$name} is not bound"
                )
            )
            print(io, subs[name])
        end
        last = m.offset + ncodeunits(m.match)
    end
    print(io, SubString(tmpl, last))
    return String(take!(io))
end

"""
    esm_template_names(template) -> Vector{String}

The substitution names `template` needs, in order of first appearance, without
`date`."""
function esm_template_names(template::AbstractString)
    out = String[]
    for m in eachmatch(_ESM_TOKEN, String(template))
        name = String(m.captures[1])
        name == "date" || name in out || push!(out, name)
    end
    return out
end

"""
    EsmTemplate(template; substitutions=Dict(), mirrors=String[])

A provider URL that is an esm template (spec/esm-sources.md §1.5). Pass it as
the `url` of a [`Provider`](@ref): it is called with a Unix-seconds file anchor
and returns the expanded URL. Its `mirrors` are esm templates expanded at the
same anchor with the same substitutions and tried in order after the primary
URL. An unbound substitution is refused here, not at the first fetch."""
struct EsmTemplate <: Function
    template::String
    substitutions::Dict{String, String}
    mirrors::Vector{String}
    function EsmTemplate(
            template::AbstractString;
            substitutions::AbstractDict = Dict{String, String}(),
            mirrors = String[]
        )
        subs = Dict{String, String}(String(k) => string(v) for (k, v) in substitutions)
        e = new(String(template), subs, String[String(m) for m in mirrors])
        for t in (e.template, e.mirrors...)
            # Every non-date token checked now; the date tokens need an anchor.
            expand_esm_template(
                replace(t, r"\{date(?::[^{}]*)?\}" => ""); substitutions = subs
            )
        end
        return e
    end
end

(e::EsmTemplate)(t) = expand_esm_template(e.template, t; substitutions = e.substitutions)

"""
    esm_mirrors(e::EsmTemplate, t) -> Vector{String}

Each mirror of `e` expanded at `t`."""
esm_mirrors(e::EsmTemplate, t) =
    String[expand_esm_template(m, t; substitutions = e.substitutions) for m in e.mirrors]
