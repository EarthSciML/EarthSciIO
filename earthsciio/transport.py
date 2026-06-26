"""Transport-layer value types + pure URL helpers shared by every transport.

The ``transport`` registry (``spec/registries.md`` §1) is keyed by URL scheme;
each transport fetches a resolved URL's bytes into a staging file and returns a
:class:`FetchResult`. This module holds the *types and pure helpers* every
transport and the cache orchestrator share — the concrete ``http``/``file``
transports live in :mod:`earthsciio.backends` alongside the ``s3`` stub, exactly
as the Rust track splits ``transport/mod.rs`` (types) from
``transport/{http,file}.rs`` (impls).

Nothing here opens a socket or touches the filesystem beyond pure path math, so
it is import-safe and usable in offline mode.
"""

from __future__ import annotations

import string
from dataclasses import dataclass
from typing import Optional
from urllib.parse import urlsplit

from .config import expand_datadir, resolve_cache_root

#: Transport ``FetchResult.status`` values.
DOWNLOADED = "downloaded"
NOT_MODIFIED = "not_modified"  # a 304 — the cached blob is still valid, reuse it


@dataclass
class FetchResult:
    """What a transport returns after a fetch attempt (``spec`` §1).

    ``status`` is :data:`DOWNLOADED` (bytes were written to the staging path) or
    :data:`NOT_MODIFIED` (a conditional GET returned 304 — nothing written,
    reuse the cached blob). ``etag``/``last_modified`` are the validators to
    persist into the manifest; ``bytes_written`` is ``0`` for a 304.
    """

    status: str
    etag: Optional[str] = None
    last_modified: Optional[str] = None
    bytes_written: int = 0

    @property
    def not_modified(self) -> bool:
        return self.status == NOT_MODIFIED


def scheme_of(url: str) -> str:
    """The lowercase URL scheme — the ``transport`` registry lookup key.

    An unknown scheme is a *registration gap* surfaced by the registry, not here;
    a URL with no scheme at all is a caller error (the URL must be resolved).
    """
    scheme = urlsplit(url).scheme.lower()
    if not scheme:
        raise ValueError(f"resolved URL has no scheme: {url!r}")
    return scheme


_EXT_OK = frozenset(string.ascii_lowercase + string.digits)


def ext_from_url(url: str) -> str:
    """A short, sanitized blob-file extension — for **human debuggability only**.

    Mirrors the Rust ``ext_from_url`` so the three tracks pick the same suffix:
    strip query/fragment, take the last path segment, split on its last ``.``,
    and accept the extension only when the stem is non-empty, the ext is 1..8
    chars, and every char is ASCII ``[a-z0-9]`` (lower-cased). Otherwise return
    ``""`` — the blob is then stored under a **bare key** (no trailing dot).

    Lookups are always by ``<key>`` (glob ``<key>*``), never by extension, so a
    suffix mismatch never breaks the shared cache; this only keeps on-disk files
    glance-able.
    """
    path = urlsplit(url).path
    seg = path.rsplit("/", 1)[-1]
    stem, dot, ext = seg.rpartition(".")
    if not dot:
        return ""
    ext = ext.lower()
    if not stem or not ext or len(ext) > 8:
        return ""
    if not all(c in _EXT_OK for c in ext):
        return ""
    return ext


def file_url_to_path(url: str, *, cache_root=None) -> str:
    """Resolve a ``file://`` URL (or a bare local path) to a filesystem path.

    Expands ``${EARTHSCIDATADIR}`` / ``$EARTHSCIDATADIR`` first — the ``nei2016``
    mirror pattern (``spec/cache-format.md`` §5) — against the effective cache
    root, then strips the ``file://`` prefix. Handles ``file:///abs`` (empty
    authority, keep the absolute path) and ``file://localhost/abs`` (drop the
    authority). A bare path (no scheme) is returned expanded. No percent-decoding
    is applied, matching the Rust transport (the cache key hashes the URL
    verbatim regardless).
    """
    root = resolve_cache_root(cache_root)
    expanded = expand_datadir(url, root)
    split = urlsplit(expanded)
    if not split.scheme:
        return expanded  # already a bare local path
    if split.scheme.lower() != "file":
        raise ValueError(f"not a file:// URL: {url!r}")
    netloc = split.netloc
    if netloc and netloc.lower() != "localhost":
        # file://host/abs — uncommon; the path is absolute, the authority ignored.
        return split.path
    return split.path
