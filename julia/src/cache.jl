# The cache — component (a)'s primitive: resolved URL -> cached blob, offline
# aware and concurrency-safe (spec/cache-format.md, spec/offline-mode.md).

# --- errors -----------------------------------------------------------------

"""Raised in offline mode when the blob for a resolved URL is absent. Carries
both the `url` and its `key` so a failure names exactly which blob is missing."""
struct CacheMiss <: Exception
    url::String
    key::String
end
Base.showerror(io::IO, e::CacheMiss) =
    print(io, "CacheMiss: no cached blob for resolved_url=", e.url, " (key=", e.key, ")")

"""Raised when a blob's bytes do not match its manifest `sha256_content`."""
struct IntegrityError <: Exception
    url::String
    key::String
    expected::String
    got::String
end
Base.showerror(io::IO, e::IntegrityError) = print(io,
    "IntegrityError: blob for ", e.url, " (key=", e.key, ") sha256=", e.got,
    " != manifest sha256_content=", e.expected)

# --- environment ------------------------------------------------------------

"""
    datadir() -> String

Resolve `\$EARTHSCIDATADIR` (spec/cache-format.md §5). The env var always wins;
the fallback default lives on `/scratch.local`, NEVER `/u` (the home inode quota
cannot absorb many small NetCDF slices — a hard rule)."""
function datadir()
    v = get(ENV, "EARTHSCIDATADIR", "")
    isempty(v) || return v
    user = get(ENV, "USER", get(ENV, "LOGNAME", "user"))
    return joinpath("/scratch.local", user, "earthsci-cache")
end

"""True when `EARTHSCI_OFFLINE` is a truthy value (`1`/`true`/`yes`)."""
function env_offline()
    v = lowercase(strip(get(ENV, "EARTHSCI_OFFLINE", "")))
    return v in ("1", "true", "yes")
end

"""True unless `EARTHSCI_REVALIDATE_FILE` is an explicit denial (`0`/`false`/
`no`/`off`).

The `file://` source recheck (spec/cache-format.md §4.1) is ON by default — that
default IS the fix for EarthSciML/EarthSciAST#293 — so unlike `env_offline` this
reads the knob the other way round: anything that is not an explicit denial,
including an unparseable value, leaves the recheck on. A typo in a knob must not
silently restore a silent-staleness bug."""
function env_revalidate_file()
    v = lowercase(strip(get(ENV, "EARTHSCI_REVALIDATE_FILE", "")))
    isempty(v) && return true
    return !(v in ("0", "false", "no", "off"))
end

# --- cache key (spec/cache-format.md §1) ------------------------------------

"""
    cache_key(resolved_url) -> String

`lowercase_hex(sha256(utf8(resolved_url)))`. The URL is hashed exactly as
resolved — UTF-8, no trailing newline, no normalization — so all three language
tracks hash the identical byte string and share the cache."""
cache_key(resolved_url::AbstractString) = bytes2hex(sha256(codeunits(resolved_url)))

"""Sub-range request: `#bytes=<a>-<b>` is appended before hashing, so a
byte-slice is its own cache entry."""
cache_key(resolved_url::AbstractString, byterange::Tuple{Integer,Integer}) =
    cache_key(string(resolved_url, "#bytes=", byterange[1], "-", byterange[2]))

# --- url helpers ------------------------------------------------------------

function url_scheme(url::AbstractString)
    m = match(r"^([A-Za-z][A-Za-z0-9+.\-]*)://", url)
    m === nothing && error("no URL scheme in: $url")
    return lowercase(m.captures[1])
end

# Extension from the URL's last path segment (debuggability only; never used for
# lookup). Query/fragment stripped first.
function url_ext(url::AbstractString)
    u = first(split(url, '?'))
    u = first(split(u, '#'))
    seg = last(split(u, '/'))
    return last(splitext(seg))     # ".nc", ".csv", … or ""
end

# RFC 3339 UTC, second precision (matches the corpus manifests).
rfc3339_utc(t::DateTime = now(UTC)) =
    Dates.format(t, dateformat"yyyy-mm-dd\THH:MM:SS\Z")

function _age_seconds(fetched_at::AbstractString)
    try
        t = DateTime(rstrip(fetched_at, 'Z'), dateformat"yyyy-mm-ddTHH:MM:SS")
        return Dates.value(now(UTC) - t) / 1000      # ms -> s
    catch
        return nothing
    end
end

# --- Cache ------------------------------------------------------------------

"""
    Cache(store::Store; offline=nothing, auth=nothing, verify=false, revalidate_file=nothing)
    Cache(; store="local", root=datadir(), offline=nothing, auth=nothing, verify=false,
          revalidate_file=nothing)

The content-addressed cache. `offline=nothing` reads `EARTHSCI_OFFLINE` from the
environment; an explicit `offline` argument wins. `auth` is a resolver or a
realm→resolver map. `verify=true` re-checks `sha256_content` on every read (off
by default, on for CI/conformance) — that is the CACHED BLOB against its own
manifest, cache-internal consistency, NOT the `file://` source recheck.

`revalidate_file=nothing` reads `EARTHSCI_REVALIDATE_FILE` and leaves the
`file://` source recheck ON unless the knob is an explicit denial (spec §4.1).
Turning it off restores the behaviour of EarthSciML/EarthSciAST#293: a local file
replaced in place is served from the warm entry forever, silently and greenly. Do
it only for a corpus known to be immutable, and only to save the one extra
read."""
struct Cache
    store::Store
    offline::Bool
    auth::Any
    verify::Bool
    revalidate_file::Bool
end

function Cache(store::Store; offline::Union{Bool,Nothing} = nothing,
              auth = nothing, verify::Bool = false,
              revalidate_file::Union{Bool,Nothing} = nothing)
    off = offline === nothing ? env_offline() : offline
    rev = revalidate_file === nothing ? env_revalidate_file() : revalidate_file
    return Cache(store, off, auth, verify, rev)
end

function Cache(; store::AbstractString = "local", root::AbstractString = datadir(),
              kwargs...)
    return Cache(make_store(store; root = root); kwargs...)
end

"""True if this cache is in offline (cache-only) mode."""
is_offline(c::Cache) = c.offline

"""The outcome of [`fetch_blob`]: the resolved blob plus how it was obtained."""
struct CacheEntry
    key::String
    path::String
    manifest::Union{Manifest,Nothing}
    status::Symbol            # :hit | :downloaded | :not_modified
end

# --- fetch ------------------------------------------------------------------

"""
    fetch_blob(cache, resolved_url; source_loader=nothing, auth_realm=nothing,
               ttl=nothing, revalidate=false, store_read=false) -> CacheEntry

Return the cached blob for `resolved_url`, fetching it first if necessary.

  * A valid cache **hit** takes no lock (spec §6).
  * Offline (`cache.offline`): resolve purely against the store; a miss raises
    [`CacheMiss`]; no transport is ever constructed (spec/offline-mode.md).
  * Online miss/stale: acquire the per-blob advisory lock, re-check, download to
    a `tmp/<uuid>.part` staging file, verify, atomically rename into `blobs/`,
    then write the manifest.

`ttl` (seconds) expires a present blob and forces revalidation; `revalidate`
forces it unconditionally. `auth_realm` selects a resolver from a realm→resolver
`auth` map and is recorded (name only) in the manifest. `store_read=true` says
this URL is ONE OBJECT of a store-backed read (a Zarr chunk, fetched once per
object) rather than a whole blob, and is passed to the transport, which bounds
such a fetch more tightly — the caller knows which it is; the transport must not
guess (transport.jl, `_http_store_read_ceiling`)."""
function fetch_blob(c::Cache, resolved_url::AbstractString;
                    source_loader = nothing, auth_realm = nothing,
                    ttl::Union{Real,Nothing} = nothing, revalidate::Bool = false,
                    store_read::Bool = false)
    key = cache_key(resolved_url)
    bp = get_blob(c.store, key)
    present = bp !== nothing

    # Fast path: present + valid, no lock.
    if present && _valid_fast(c, resolved_url, key, bp, ttl, revalidate)
        return CacheEntry(key, bp, get_meta(c.store, key), :hit)
    end

    if c.offline
        throw(CacheMiss(resolved_url, key))
    end

    # If the blob is present here, we are past the fast path because it is stale
    # or `revalidate` was set — so we WANT a conditional GET, not a presence
    # short-circuit. If it is absent, we are filling a miss (and a blob that
    # appears under the lock means a peer filled it: reuse it).
    return _locked_fetch(c, resolved_url, key, source_loader, auth_realm, present,
                         store_read)
end

# Validity for the lock-free fast path. Offline: presence (+ optional integrity).
# Online: present blobs are immutable by default (closed past period / static
# loader); a finite TTL or `revalidate` sends us to the lock path to conditional-
# GET. A corrupt present blob raises IntegrityError (it is not a silent miss).
#
# Rung 0 (spec/cache-format.md §4.1) is the one exception to "immutable by
# default": a `file://` source is sitting right there, so ask it instead of
# guessing. "Immutable by default" is exactly how a corpus replaced in place kept
# being served from a warm entry (EarthSciML/EarthSciAST#293). Offline keeps the
# old behaviour on purpose — it trades freshness for hermeticity by design
# (spec/offline-mode.md §3) and has no transport left to re-ingest with.
function _valid_fast(c::Cache, url, key, bp, ttl, revalidate)
    if c.offline
        c.verify && _verify_integrity(c, url, key, bp)
        return true
    end
    revalidate && return false
    if c.revalidate_file
        src = _local_source_path(url)
        if src !== nothing
            meta = get_meta(c.store, key)
            # A manifest-less blob ABSTAINS rather than re-ingesting: there is
            # nothing to judge the source against, and that state is the commit
            # race (put_blob! lands before put_meta!, so a peer legitimately sees
            # a blob with no manifest for an instant). Forcing a download there
            # would break the "N racing fetchers ⇒ exactly ONE download"
            # contract of spec §6 — measured: 3 of 4 processes re-downloaded.
            # A source that is GONE still lands below and fails in the
            # transport: absent must not read as a stale hit.
            meta !== nothing && !file_source_is_current(src, meta) && return false
        end
    end
    if ttl !== nothing
        meta = get_meta(c.store, key)
        if meta !== nothing
            age = _age_seconds(meta.fetched_at)
            age !== nothing && age > ttl && return false
        end
    end
    c.verify && _verify_integrity(c, url, key, bp)
    return true
end

"""The local path a `file://` URL names, or `nothing` for every other scheme.

Goes through the transport's own [`file_url_to_path`] so the file the cache
rechecks is exactly the file the transport would re-read; two spellings of that
mapping would be a bug generator. It is the CANONICAL url — the one the manifest
records (spec §3) — so the entry is judged against the source it claims to be."""
function _local_source_path(url::AbstractString)
    startswith(url, "file://") || return nothing
    try
        return file_url_to_path(url)
    catch
        return nothing
    end
end

"""
    file_source_is_current(source, manifest) -> Bool

Rung 0 (spec/cache-format.md §4.1): is the `file://` source behind a cached entry
still the file that was ingested into it?

The cache is keyed by the resolved URL, so a local file replaced IN PLACE keeps
the same key and every later read is served the bytes of the file that used to be
there. Nothing else catches it: a `file://` source has no ETag and no
`Last-Modified`, and a present blob is immutable by default, so the entry
outlives the file it was copied from. That is EarthSciML/EarthSciAST#293, where a
snapshot corpus was replaced at the same paths and a test suite stayed green
against a corpus that no longer existed — the manifest recorded 371 bytes while
the file on disk was 363.

Note what this is NOT: `verify=true` hashes the CACHED BLOB against the manifest,
comparing the copy with the record of the copy, so it passes with flying colours
while the file the copy was made from has been replaced. This compares the
SOURCE with that record.

The manifest already carries everything needed, so this is not a cache-format
change:

  * source missing, unreadable, or not a regular file ⇒ not current (the caller
    re-ingests, and the transport then raises the real error);
  * on-disk size != `manifest.bytes` ⇒ not current, WITHOUT hashing;
  * `sha256(source)` != `manifest.sha256_content` ⇒ not current. Size alone would
    not do: a float re-encode, a different scenario year, or any edit that
    preserves the length is exactly what a size check waves through;
  * otherwise current — serve the cached blob, re-ingest nothing.
"""
function file_source_is_current(source::AbstractString, m::Manifest)
    st = try
        stat(source)
    catch
        return false
    end
    (isfile(st) && st.size == m.bytes) || return false
    got = try
        bytes2hex(open(sha256, source))
    catch
        # Unreadable half-way through ⇒ re-ingest and let the transport speak.
        return false
    end
    return lowercase(got) == lowercase(m.sha256_content)
end

function _verify_integrity(c::Cache, url, key, bp)
    meta = get_meta(c.store, key)
    meta === nothing && return nothing
    got = bytes2hex(open(sha256, bp))
    got == meta.sha256_content ||
        throw(IntegrityError(String(url), String(key), meta.sha256_content, got))
    return nothing
end

function _locked_fetch(c::Cache, url, key, source_loader, auth_realm, want_revalidate,
                       store_read::Bool = false)
    return lock_key(c.store, key) do
        # Re-check under the lock. When filling a miss, a blob that appeared
        # means a peer process just filled it — reuse it, take no download. When
        # revalidating, presence is expected and we proceed to the conditional GET.
        bp = get_blob(c.store, key)
        if bp !== nothing && !want_revalidate
            c.verify && _verify_integrity(c, url, key, bp)
            return CacheEntry(key, bp, get_meta(c.store, key), :hit)
        end

        transport = TRANSPORT_REGISTRY[url_scheme(url)]
        meta = get_meta(c.store, key)
        # Only send conditional-GET headers when there is a cached blob to fall
        # back on; otherwise force a full GET.
        conditional = (bp !== nothing && meta !== nothing) ?
            (etag = meta.etag, last_modified = meta.last_modified) : NamedTuple()
        staged = staging_path(c.store)
        try
            res = fetch!(transport, url, staged;
                         conditional = conditional,
                         auth = resolve_auth(c.auth, auth_realm),
                         store_read = store_read)

            if res.status == :not_modified
                bp2 = get_blob(c.store, key)
                bp2 === nothing &&
                    error("transport reported 304 but no cached blob present for $url")
                newmeta = _touch_fetched_at(meta)
                newmeta === nothing || put_meta!(c.store, key, newmeta)
                return CacheEntry(key, bp2, newmeta, :not_modified)
            end

            sha = bytes2hex(open(sha256, staged))
            nbytes = filesize(staged)
            committed = put_blob!(c.store, key, staged; ext = url_ext(url))
            manifest = Manifest(String(url), res.etag, res.last_modified, sha,
                                nbytes, rfc3339_utc(),
                                _strornothing(source_loader), _strornothing(auth_realm))
            put_meta!(c.store, key, manifest)
            return CacheEntry(key, committed, manifest, :downloaded)
        finally
            isfile(staged) && rm(staged; force = true)
        end
    end
end

_touch_fetched_at(::Nothing) = nothing
_touch_fetched_at(m::Manifest) = Manifest(m.url, m.etag, m.last_modified,
    m.sha256_content, m.bytes, rfc3339_utc(), m.source_loader, m.auth_realm)
