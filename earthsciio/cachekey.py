"""The shared cache key — ``key = sha256(resolved_url)``.

This is the single most load-bearing function in EarthSciIO: it is what makes
the cache **shared across languages**. A file the Python track fetches must be
reused, byte-for-byte and without re-fetch, by the Julia and Rust tracks, which
hash the *identical* byte string. See ``spec/cache-format.md`` §1.

Rules (normative, do not "improve"):

* The URL is encoded **UTF-8, no trailing newline**, exactly as resolved — no
  normalization, no percent-encoding changes, no case folding.
* A sub-range / byte-slice request appends ``#bytes=<a>-<b>`` to the URL
  **before** hashing, so a sub-slice is its own cache entry.
"""

from __future__ import annotations

import hashlib
from typing import Optional, Tuple


def cache_key(resolved_url: str, byte_range: Optional[Tuple[int, int]] = None) -> str:
    """Return the lowercase-hex sha256 of the resolved URL.

    ``byte_range=(a, b)`` appends ``#bytes=a-b`` before hashing so a byte-slice
    addresses its own blob, exactly as the spec requires. The base URL is hashed
    verbatim; callers pass the URL already resolved (time anchor + parameters
    expanded). URL resolution itself is pure and lives in the cores — this layer
    starts from the resolved URL.
    """
    keyed = range_keyed_url(resolved_url, byte_range)
    return hashlib.sha256(keyed.encode("utf-8")).hexdigest()


def range_keyed_url(resolved_url: str, byte_range: Optional[Tuple[int, int]] = None) -> str:
    """The exact string that gets hashed (URL plus any ``#bytes=`` fragment)."""
    if byte_range is None:
        return resolved_url
    start, end = byte_range
    return f"{resolved_url}#bytes={start}-{end}"


def sha256_bytes(data: bytes) -> str:
    """sha256 of a byte string, lowercase hex (manifest ``sha256_content``)."""
    return hashlib.sha256(data).hexdigest()


def sha256_file(path, chunk_size: int = 1 << 20) -> str:
    """sha256 of a file's contents, streamed so large blobs don't load to RAM."""
    h = hashlib.sha256()
    with open(path, "rb") as fh:
        for chunk in iter(lambda: fh.read(chunk_size), b""):
            h.update(chunk)
    return h.hexdigest()
