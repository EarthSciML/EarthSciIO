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

# --- rung 0 (spec/cache-format.md §4.1) --------------------------------------

# What rung 0 learned about the `file://` source behind a cached entry. A Bool
# cannot carry this: "the corpus was deleted", "the corpus was replaced" and
# "this host cannot read the corpus" are three different facts, and only the
# first two are grounds to re-ingest.
const SOURCE_CURRENT  = :current    # byte-for-byte the file the manifest records
const SOURCE_REPLACED = :replaced   # present, and a different file
const SOURCE_MISSING  = :missing    # nothing there, or not a regular file
const SOURCE_UNKNOWN  = :unknown    # could not be read at all; nothing learned

"""`stat` the source and classify what came back.

The split that matters is "no such file" (the corpus is gone — the report's
sharpest case, and it must stay a loud error) against every other failure (no
permission, a path component that is not a directory, an I/O error — this host
cannot see the corpus, which is not evidence that it changed). Warming a cache
where `/corpus` is mounted and reading it where it is not must not be a hard
failure. Returns `(stat, nothing)` or `(nothing, verdict)`."""
function _stat_source(source::AbstractString)
    st = try
        stat(source)
    catch e
        # `stat` throws only on a path it cannot walk at all (ENOTDIR, EACCES on
        # a parent); a plain absence comes back as a zero `StatStruct`.
        return nothing, (e isa Base.IOError && e.code == Base.UV_ENOENT) ?
                        SOURCE_MISSING : SOURCE_UNKNOWN
    end
    if !ispath(st)
        # Julia's `stat` reports "not there" for a genuine absence AND for a
        # path it could not walk (ENOTDIR), swallowing the errno that separates
        # them — so ask the parent, which is what the errno would have told us.
        # A parent that is not a directory means the path is broken (this host
        # may have the wrong mount): nothing was learned. A parent that is a
        # directory, or is itself absent, means the file really is gone — the
        # report's "pointed at a data path that does not exist" — and that must
        # stay a loud error.
        parent = dirname(source)
        broken = try
            !isempty(parent) && ispath(parent) && !isdir(parent)
        catch
            false
        end
        return nothing, broken ? SOURCE_UNKNOWN : SOURCE_MISSING
    end
    # A directory standing where the corpus used to be is the report's "pointed
    # at a path that is not there any more" shape.
    isfile(st) || return nothing, SOURCE_MISSING
    return st, nothing
end

"""Rung 0's `(size, mtime)` memo.

Hashing the source on every `fetch_blob` is what rung 0 costs, and this track
calls `fetch_blob` **per tick**: `Provider` keeps no decoded-file buffer (see
`_file_for`), so a 2.8 GB corpus would be re-hashed for every tick of a run.

A digest is therefore remembered against the `(size, mtime)` the source had when
it was computed, and reused while both are unchanged — the same bargain `make`,
`ninja` and `rsync` strike. The memo lives on the [`Cache`] and dies with it, so
a fresh process always pays one real read per file; what it removes is the
*repeat* read within a run.

What it can miss: a replacement preserving both the size and the mtime, in
process, after the file was already read once. The window is one `Cache`
lifetime."""
mutable struct SourceRevalidator
    seen::Dict{String,Tuple{Int,Float64,String}}   # path => (size, mtime, sha)
    lock::ReentrantLock
    full_reads::Int
end
SourceRevalidator() = SourceRevalidator(Dict{String,Tuple{Int,Float64,String}}(),
                                        ReentrantLock(), 0)

# How many fingerprints one revalidator remembers before dropping the lot. A
# read loop touches a handful of files; this only stops an unbounded walk from
# growing the map without limit.
const SOURCE_MEMO_CAP = 512

"""
    source_state(rev, source, manifest) -> Symbol

Rung 0: the state of `source` relative to `manifest`, reusing a remembered
digest when the source's `(size, mtime)` is unchanged."""
function source_state(rev::SourceRevalidator, source::AbstractString, m::Manifest)
    st, verdict = _stat_source(source)
    verdict === nothing || return verdict
    # A different size is a different file, and no read is needed to say so.
    # This is the cheap half of the check and every track keeps it.
    st.size == m.bytes || return SOURCE_REPLACED
    fingerprint = (Int(st.size), Float64(st.mtime))
    remembered = lock(rev.lock) do
        get(rev.seen, String(source), nothing)
    end
    if remembered !== nothing && remembered[1] == fingerprint[1] &&
       remembered[2] == fingerprint[2]
        return _source_verdict(remembered[3], m)
    end
    got = try
        bytes2hex(open(sha256, source))
    catch
        # It survived `stat` but not `open`: still "cannot tell".
        return SOURCE_UNKNOWN
    end
    lock(rev.lock) do
        rev.full_reads += 1
        # Crude but sufficient: the memo is an optimisation, so dropping all of
        # it costs one re-read per live file rather than needing an LRU.
        length(rev.seen) >= SOURCE_MEMO_CAP && empty!(rev.seen)
        rev.seen[String(source)] = (fingerprint[1], fingerprint[2], got)
    end
    return _source_verdict(got, m)
end

_source_verdict(sha, m::Manifest) =
    lowercase(sha) == lowercase(m.sha256_content) ? SOURCE_CURRENT : SOURCE_REPLACED

"""
    file_source_state(source, manifest) -> Symbol

Rung 0 without a memo (spec/cache-format.md §4.1): is the `file://` source behind
a cached entry still the file that was ingested into it?

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

  * nothing at the path, or not a regular file ⇒ [`SOURCE_MISSING`];
  * the path cannot be read at all ⇒ [`SOURCE_UNKNOWN`], and rung 0 abstains
    rather than claim a change it did not observe;
  * on-disk size != `manifest.bytes` ⇒ [`SOURCE_REPLACED`], WITHOUT hashing;
  * `sha256(source)` != `manifest.sha256_content` ⇒ [`SOURCE_REPLACED`]. Size
    alone would not do: a float re-encode, a different scenario year, or any edit
    that preserves the size is exactly what a size check waves through;
  * otherwise [`SOURCE_CURRENT`] — serve the cached blob, re-ingest nothing.

A [`Cache`] calls [`source_state`] instead, which is this with a `(size, mtime)`
memo in front of the read."""
file_source_state(source::AbstractString, m::Manifest) =
    source_state(SourceRevalidator(), source, m)

"""
    file_source_is_current(source, manifest) -> Bool

Rung 0 as a yes/no: is the source PROVABLY the file that was ingested? Only
[`SOURCE_CURRENT`] is `true`. The cache does not use this — the difference
between [`SOURCE_MISSING`], [`SOURCE_REPLACED`] and [`SOURCE_UNKNOWN`] is exactly
what it acts on — but it is the honest predicate for a caller that just wants the
question answered."""
file_source_is_current(source::AbstractString, m::Manifest) =
    file_source_state(source, m) === SOURCE_CURRENT

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
    # Rung 0's (size, mtime) memo, so a per-tick read loop reads each unchanged
    # source once per Cache rather than once per fetch_blob.
    revalidator::SourceRevalidator
end

function Cache(store::Store; offline::Union{Bool,Nothing} = nothing,
              auth = nothing, verify::Bool = false,
              revalidate_file::Union{Bool,Nothing} = nothing)
    off = offline === nothing ? env_offline() : offline
    rev = revalidate_file === nothing ? env_revalidate_file() : revalidate_file
    return Cache(store, off, auth, verify, rev, SourceRevalidator())
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
                         store_read, ttl, revalidate)
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
    # One read of the manifest serves both rungs below.
    #
    # A manifest-less blob ABSTAINS from both rather than re-ingesting: there is
    # nothing to judge the source against, and that state is the commit race
    # (put_blob! lands before put_meta!, so a peer legitimately sees a blob with
    # no manifest for an instant). Forcing a download there would break the
    # "N racing fetchers ⇒ exactly ONE download" contract of spec §6 — measured:
    # 3 of 4 processes re-downloaded. Rust and Python treat that state as a miss
    # instead, which is their own pre-existing behaviour and equally untouched;
    # it is why spec §4 says to delete the BLOB as well as the manifest when
    # invalidating one entry by hand.
    meta = (c.revalidate_file || ttl !== nothing) ? get_meta(c.store, key) : nothing
    if c.revalidate_file && meta !== nothing
        src = _local_source_path(url)
        if src !== nothing
            st = source_state(c.revalidator, src, meta)
            # REPLACED: a different file at the same path — re-ingest, which is
            # the whole point of the rung. MISSING: nothing there — re-ingest so
            # the transport raises the real absence; absent must not read as a
            # stale hit. (This track's `fetch_blob` takes no `mirrors`, so a
            # missing source is always this entry's own source. Rust and Python
            # abstain on MISSING when mirrors were supplied, because there the
            # blob may have come from one.) UNKNOWN: the path could not be read
            # at all — no permission, an unmounted filesystem — which is not
            # evidence the bytes changed, so rung 0 abstains.
            (st === SOURCE_REPLACED || st === SOURCE_MISSING) && return false
        end
    end
    if ttl !== nothing && meta !== nothing
        age = _age_seconds(meta.fetched_at)
        age !== nothing && age > ttl && return false
    end
    c.verify && _verify_integrity(c, url, key, bp)
    return true
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
                       store_read::Bool = false, ttl = nothing,
                       revalidate::Bool = false)
    return lock_key(c.store, key) do
        # Re-check under the lock. When filling a miss, a blob that appeared
        # means a peer process just filled it — reuse it, take no download.
        bp = get_blob(c.store, key)
        if bp !== nothing && !want_revalidate
            c.verify && _verify_integrity(c, url, key, bp)
            return CacheEntry(key, bp, get_meta(c.store, key), :hit)
        end
        # When revalidating, presence alone is NOT enough (that is what we are
        # revalidating), but a peer may have refreshed the entry while we queued
        # for the lock — so ask the fast path again rather than assuming. This is
        # what makes N racers over a replaced `file://` source produce exactly
        # ONE re-ingest instead of N (spec §6): without it, all N fail rung 0
        # before any of them takes the lock and all N then re-copy. Measured on
        # 4 processes: 4 downloads before, 1 after. Rust and Python get this for
        # free by re-running their whole `try_hit` under the lock.
        #
        # A forced `revalidate` still falls through (the fast path refuses it
        # outright), and a TTL that is genuinely expired still falls through to
        # the conditional GET. `_valid_fast` verifies integrity itself when it
        # says yes, so there is no second verify here.
        if bp !== nothing && _valid_fast(c, url, key, bp, ttl, revalidate)
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
