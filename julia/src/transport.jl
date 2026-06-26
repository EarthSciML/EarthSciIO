# Transport backends (spec/registries.md §1).
#
# A transport fetches a resolved URL's bytes into a staging file. Transports are
# constructed and called ONLY when offline=false (offline mode bypasses the
# transport registry entirely — see cache.jl / spec/offline-mode.md §2).

"""Result of a transport fetch. `:not_modified` means a 304 — reuse the cache."""
struct FetchResult
    status::Symbol            # :downloaded | :not_modified
    etag::Union{String,Nothing}
    last_modified::Union{String,Nothing}
    bytes_written::Int
end

# --- auth seam --------------------------------------------------------------
# Pluggable per-realm credential resolver, injected into the transport. The
# realm name (cds/firms/openaq/rda) reaches the manifest; the credential never
# does. New realms are new resolvers — not a transport edit.

"""Resolves request auth headers for a realm. Never stored in the manifest."""
abstract type AuthResolver end

"""No authentication (the default)."""
struct NoAuth <: AuthResolver end

"""Bearer/token auth (CDS / FIRMS / OpenAQ / RDA tokens, generic bearer)."""
struct BearerAuth <: AuthResolver
    token::String
end

auth_headers(::NoAuth, ::AbstractString) = Pair{String,String}[]
auth_headers(a::BearerAuth, ::AbstractString) = ["Authorization" => string("Bearer ", a.token)]

# Resolve the auth for a realm from whatever the caller supplied: nothing → no
# auth; a single resolver → used as-is; a realm→resolver map → looked up.
resolve_auth(::Nothing, ::Any) = NoAuth()
resolve_auth(a::AuthResolver, ::Any) = a
resolve_auth(m::AbstractDict, realm) =
    realm === nothing ? NoAuth() : get(m, realm, NoAuth())

# --- http(s) transport (Downloads / libcurl) --------------------------------

"""HTTP/HTTPS transport: GET with conditional-GET revalidation. Mirror failover
is handled at the call site (the cache tries mirror URLs in order)."""
struct HttpTransport <: Transport end
schemes(::HttpTransport) = ["http", "https"]

function fetch!(::HttpTransport, url::AbstractString, dest::AbstractString;
                conditional = NamedTuple(), auth::AuthResolver = NoAuth())
    headers = Pair{String,String}[]
    append!(headers, auth_headers(auth, url))
    et = get(conditional, :etag, nothing)
    lm = get(conditional, :last_modified, nothing)
    et === nothing || push!(headers, "If-None-Match" => et)
    lm === nothing || push!(headers, "If-Modified-Since" => lm)

    resp = Downloads.request(url; method = "GET", output = dest,
                             headers = headers, throw = false)
    if resp.status == 304
        return FetchResult(:not_modified, et, lm, 0)
    elseif 200 <= resp.status < 300
        return FetchResult(:downloaded, _header(resp, "etag"),
                           _header(resp, "last-modified"), filesize(dest))
    else
        error("http transport: GET $url returned HTTP status $(resp.status)")
    end
end

function _header(resp::Downloads.Response, name::AbstractString)
    for (k, v) in resp.headers
        lowercase(String(k)) == name && return String(v)
    end
    return nothing
end

# --- file transport (local copy) --------------------------------------------

"""`file://` transport: copy a local file into the cache. Expands
`\${EARTHSCIDATADIR}` (and other `\$VAR`) inside `file://` templates so a
pre-populated local mirror (the `nei2016` pattern) is found."""
struct FileTransport <: Transport end
schemes(::FileTransport) = ["file"]

function fetch!(::FileTransport, url::AbstractString, dest::AbstractString;
                conditional = NamedTuple(), auth::AuthResolver = NoAuth())
    src = file_url_to_path(url)
    isfile(src) || error("file transport: source not found: $src (from $url)")
    cp(src, dest; force = true)
    return FetchResult(:downloaded, nothing, nothing, filesize(dest))
end

"""Expand `\${VAR}` and `\$VAR` from the environment (empty if unset)."""
function expand_env(s::AbstractString)
    s = replace(s, r"\$\{([A-Za-z_][A-Za-z0-9_]*)\}" => m -> get(ENV, m[3:prevind(m, lastindex(m))], ""))
    s = replace(s, r"\$([A-Za-z_][A-Za-z0-9_]*)" => m -> get(ENV, m[2:end], ""))
    return s
end

"""Map a `file://` URL to a local filesystem path, expanding env templates."""
function file_url_to_path(url::AbstractString)
    startswith(url, "file://") || error("not a file:// URL: $url")
    rest = expand_env(url[8:end])              # strip "file://"
    startswith(rest, "localhost/") && (rest = rest[10:end])
    return startswith(rest, "/") ? rest : string("/", rest)
end

# --- s3 transport (STUB; esio-9nb.8) ----------------------------------------

"""Registered stub for the future S3-proxy / object-store GET path. Registered
now so the transport dispatch is complete; no network impl yet."""
struct S3Transport <: Transport end
schemes(::S3Transport) = ["s3"]
fetch!(::S3Transport, url::AbstractString, ::AbstractString; kwargs...) =
    error("s3 transport is a registered STUB (esio-9nb.8); no network impl yet for $url")
