# Fetch a file into the content-addressed cache, then read it back offline.
using EarthSciIO

const FIXTURES = joinpath(@__DIR__, "..", "fixtures")
const URL = "file://" * abspath(joinpath(FIXTURES, "stations.csv"))
const CACHE_ROOT = mktempdir() # hide

# region: fetch ops=cache,cache_entry,cache_key
cache = Cache(; root = CACHE_ROOT, offline = false)
entry = fetch_blob(cache, URL; source_loader = "stations")
@assert entry isa CacheEntry
@assert entry.status == :downloaded
@assert read(entry.path) == read(joinpath(FIXTURES, "stations.csv"))
# The blob is addressed by the SHA-256 of the resolved URL.
@assert entry.key == cache_key(URL)

# Asking again is a cache hit: no transfer happens.
again = fetch_blob(cache, URL; source_loader = "stations")
@assert again.status == :hit && again.path == entry.path
# endregion

# region: offline ops=is_offline,cache_miss
offline = Cache(; root = CACHE_ROOT, offline = true)
@assert is_offline(offline)
@assert fetch_blob(offline, URL).path == entry.path  # served from disk only

missed = try
    fetch_blob(offline, "file://" * abspath(joinpath(FIXTURES, "not-cached.csv")))
    false
catch err
    err isa CacheMiss  # offline mode never touches the network
end
@assert missed
# endregion

# region: data_dir ops=data_dir
@assert datadir() isa String
# endregion
