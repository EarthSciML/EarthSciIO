"""Cache-key parity + helpers (``spec/cache-format.md`` §1).

The cache key is the single most load-bearing cross-language contract: a file
the Python track fetches must be reused, byte-for-byte, by Julia/Rust, which hash
the *identical* byte string. These pinned vectors come straight from the spec +
the conformance corpus — if they change, the shared cache silently splits.
"""

from __future__ import annotations

import hashlib

from earthsciio import cache_key, range_keyed_url, sha256_bytes, sha256_file
from earthsciio.transport import ext_from_url

# Pinned vectors (spec §1 worked example + the two corpus blobs).
ERA5_URL = "https://data.earthsci.dev/era5/2018/11/20181108.nc"
ERA5_KEY = "11cdcec111409f586e6afc432e1a6da47e6f97ccf3715e5db8554632b00671c1"
OPENAQ_URL = (
    "https://openaq-data-archive.s3.amazonaws.com/records/openaq/"
    "locationid=1/2018-11-08.csv"
)
OPENAQ_KEY = "69dd26b950e43cb2182e3b4d02e89e09bfb798b13469183ca2dad15c5794379a"


def test_pinned_era5_key():
    assert cache_key(ERA5_URL) == ERA5_KEY


def test_pinned_openaq_key():
    assert cache_key(OPENAQ_URL) == OPENAQ_KEY


def test_key_is_utf8_sha256_no_normalization():
    url = "https://example.org/a/b?x=1&y=2"
    assert cache_key(url) == hashlib.sha256(url.encode("utf-8")).hexdigest()


def test_byte_range_is_its_own_entry():
    base = cache_key(ERA5_URL)
    ranged = cache_key(ERA5_URL, byte_range=(0, 1023))
    assert ranged != base
    # The hashed string is the URL plus the #bytes fragment, appended verbatim.
    assert range_keyed_url(ERA5_URL, (0, 1023)) == ERA5_URL + "#bytes=0-1023"
    assert ranged == hashlib.sha256(
        (ERA5_URL + "#bytes=0-1023").encode("utf-8")
    ).hexdigest()


def test_sha256_helpers(tmp_path):
    data = b"earthsci\n"
    assert sha256_bytes(data) == hashlib.sha256(data).hexdigest()
    p = tmp_path / "blob.bin"
    p.write_bytes(data)
    assert sha256_file(p) == hashlib.sha256(data).hexdigest()


def test_ext_from_url_matches_rust_rules():
    assert ext_from_url(ERA5_URL) == "nc"
    assert ext_from_url(OPENAQ_URL) == "csv"
    assert ext_from_url("https://h/p/file.NC4") == "nc4"  # lower-cased
    assert ext_from_url("https://h/p/data.nc?token=abc#frag") == "nc"  # strip q/frag
    # No clean extension ⇒ empty (blob stored under a bare key, no trailing dot).
    assert ext_from_url("https://h/p/noext") == ""
    assert ext_from_url("https://h/p/.hidden") == ""  # empty stem
    assert ext_from_url("https://h/p/x.toolongext") == ""  # ext > 8 chars
    assert ext_from_url("https://h/p/x.tar.gz") == "gz"
