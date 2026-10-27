"""Fetch a file into the content-addressed cache, then read it back offline."""

import tempfile
from pathlib import Path

import earthsciio
from earthsciio import Cache, CacheMiss

FIXTURES = Path(__file__).resolve().parent.parent / "fixtures"
URL = (FIXTURES / "stations.csv").as_uri()
cache_root = Path(tempfile.mkdtemp(prefix="earthsciio-example-"))  # hide


# region: fetch ops=cache,cache_entry,cache_key
cache = Cache(root=cache_root, offline=False)
entry = cache.fetch(URL, source_loader="stations")
assert entry.status == "downloaded"
assert entry.path.read_bytes() == (FIXTURES / "stations.csv").read_bytes()
# The blob is addressed by the SHA-256 of the resolved URL.
assert entry.key == earthsciio.cache_key(URL)

# Asking again is a cache hit: no transfer happens.
again = cache.fetch(URL, source_loader="stations")
assert again.status == "hit" and again.path == entry.path
# endregion


# region: offline ops=is_offline,cache_miss
offline = Cache(root=cache_root, offline=True)
assert offline.fetch(URL).path == entry.path  # served from disk only

try:
    offline.fetch((FIXTURES / "not-cached.csv").as_uri())
    raise AssertionError("an offline miss must raise")
except CacheMiss:
    pass  # offline mode never touches the network
# endregion


# region: data_dir ops=data_dir
root = earthsciio.resolve_cache_root(cache_root)
assert Path(root) == cache_root
# endregion
