"""The cache validation ladder — is a cached blob a hit, or must we revalidate?

Ports the Rust ``validate::decide``. Given a stored manifest plus the loader's
:class:`Temporal` freshness policy, decide **hit / revalidate / miss** in this
order (first applicable wins, ``spec/cache-format.md`` §4):

0. **local source recheck** — a ``file://`` source is compared against the file
   it was ingested from (:func:`file_source_is_current`). Rules 1-4 are all
   about a source that can only be consulted over the network; a local file has
   an exact truth sitting on disk. It is a separate function rather than a rung
   inside :func:`decide` because it touches the filesystem and ``decide`` is
   pure; :class:`~earthsciio.cache.Cache` applies it to the entries ``decide``
   has already called a hit.
1. **content hash** — if a loader-declared checksum exists, compare it to
   ``manifest.sha256_content``. Strongest. (No loader declares one today; this is
   the future ``source.checksums`` hook.)
2. **declared immutability** — a static loader (no ``temporal``) or a closed
   past period cannot change, so it is a hit with no network access at all.
   A *declaration* outranks a conditional GET, whose only possible answer here
   is "unchanged".
3. **conditional GET** — if ``etag``/``last_modified`` are stored, revalidate
   over the network (``If-None-Match`` / ``If-Modified-Since``). Validators beat
   the TTL *heuristic*, so this fires **before** TTL — but not before rule 2.
4. **TTL from ``temporal``** — a current/incomplete period is fresh only until
   its TTL elapses.

Rule 2 used to sit below rule 3, which made it unreachable for any store that
returns an ETag — i.e. all of S3. Every warm hit then paid a round-trip to be
told the blob had not changed; on the ISRM store that was 85.9 ms per chunk
against a 0.078 ms file read, and it dominated the wall clock of a run whose
data was already entirely on local disk.

Offline mode short-circuits all of this (presence + stored hash only); that
short-circuit lives in :mod:`earthsciio.cache`, not here. ``decide`` is pure and
takes an injectable ``now`` so TTL tests are deterministic.
"""

from __future__ import annotations

import datetime as _dt
import os
import stat as _stat
from dataclasses import dataclass
from enum import Enum
from typing import Optional, Union

from .cachekey import sha256_file
from .manifest import Manifest, parse_rfc3339

#: The three possible verdicts (mirrors Rust ``CacheDecision``).
HIT = "hit"
REVALIDATE = "revalidate"
MISS = "miss"


class _Kind(Enum):
    STATIC = "static"
    CLOSED = "closed_period"
    INCOMPLETE = "incomplete"


@dataclass(frozen=True)
class Temporal:
    """The freshness policy derived from a loader's ``temporal`` block.

    Build via the classmethods rather than the raw constructor:

    * :meth:`static` — no ``temporal`` block; immutable once fetched.
    * :meth:`closed_period` — a closed past period (e.g. ``file_period:P1D`` for a
      past date); immutable, infinite TTL.
    * :meth:`incomplete` — a current/incomplete period; fresh only until ``ttl``.
    """

    kind: _Kind
    ttl: Optional[_dt.timedelta] = None

    @classmethod
    def static(cls) -> "Temporal":
        return cls(_Kind.STATIC)

    @classmethod
    def closed_period(cls) -> "Temporal":
        return cls(_Kind.CLOSED)

    @classmethod
    def incomplete(cls, ttl: Union[_dt.timedelta, float, int]) -> "Temporal":
        return cls(_Kind.INCOMPLETE, _as_timedelta(ttl))

    @property
    def immutable(self) -> bool:
        """True for static + closed-period loaders (infinite TTL)."""
        return self.kind in (_Kind.STATIC, _Kind.CLOSED)


def _as_timedelta(ttl: Union[_dt.timedelta, float, int]) -> _dt.timedelta:
    if isinstance(ttl, _dt.timedelta):
        return ttl
    return _dt.timedelta(seconds=float(ttl))


def is_fresh(
    fetched_at: str,
    ttl: Union[_dt.timedelta, float, int],
    now: Optional[_dt.datetime] = None,
) -> bool:
    """Whether a blob fetched at ``fetched_at`` is still within ``ttl``.

    An **unparseable** ``fetched_at`` returns ``False`` (force revalidation — we
    cannot prove freshness). A ``fetched_at`` in the future (clock skew) is
    treated as just-fetched (``True``), matching the Rust ``is_fresh``.
    """
    try:
        fetched = parse_rfc3339(fetched_at)
    except (ValueError, TypeError):
        return False
    if now is None:
        now = _dt.datetime.now(_dt.timezone.utc)
    age = now - fetched
    if age.total_seconds() < 0:
        return True
    return age <= _as_timedelta(ttl)


def decide(
    manifest: Manifest,
    temporal: Optional[Temporal] = None,
    expected_checksum: Optional[str] = None,
    *,
    now: Optional[_dt.datetime] = None,
) -> str:
    """Return :data:`HIT`, :data:`REVALIDATE`, or :data:`MISS` for ``manifest``.

    See the module docstring for the (first-wins) ladder. ``REVALIDATE`` tells
    the cache to issue a conditional GET using the stored validators; ``MISS``
    tells it to re-download.
    """
    # 1. content hash (strongest; future source.checksums hook)
    if expected_checksum:
        stored = (manifest.sha256_content or "").lower()
        return HIT if stored == expected_checksum.lower() else MISS
    # 2. declared immutability. A static or closed-period source cannot change,
    #    so a conditional GET can only ever answer "unchanged" -- a network
    #    round-trip whose result is known in advance. This MUST stay above the
    #    validator rule: S3 returns an ETag on EVERY object, so with the
    #    validators first this branch is unreachable for any S3-backed store and
    #    every warm cache hit pays a round-trip to be told nothing. Measured on
    #    the ISRM store: 85.9 ms/chunk before, 0.078 ms/chunk after, the latter
    #    being the raw file-read floor.
    if temporal is None or temporal.immutable:
        return HIT
    # 3. conditional GET when validators are stored. Validators beat the TTL
    #    HEURISTIC below, but not the DECLARATION above.
    if manifest.etag or manifest.last_modified:
        return REVALIDATE
    # 4. TTL from temporal (incomplete period, no validators)
    return HIT if is_fresh(manifest.fetched_at, temporal.ttl, now) else MISS


def file_source_is_current(source: os.PathLike, manifest: Manifest) -> bool:
    """Rung 0: is the ``file://`` source behind a cached entry still that file?

    The cache is keyed by the resolved URL, so a local file replaced **in
    place** keeps the same key and every later read is served the bytes of the
    file that used to be there. Nothing in :func:`decide` catches it: a
    ``file://`` source has no ETag and no ``Last-Modified``, and a source with
    no ``temporal`` is *declared* immutable by rule 2, so ``decide`` answers
    :data:`HIT` forever. That is `EarthSciML/EarthSciAST#293
    <https://github.com/EarthSciML/EarthSciAST/issues/293>`_, where a snapshot
    corpus was replaced at the same paths and a test suite stayed green against
    a corpus that no longer existed — the manifest recorded 371 bytes while the
    file on disk was 363.

    Note what this is NOT. ``Cache(verify=True)`` hashes the **cached blob**
    against the manifest: it compares the copy with the record of the copy, so
    it passes with flying colours while the file the copy was made from has been
    replaced. This compares the **source** with that record.

    The manifest already carries everything needed, so this is not a
    cache-format change:

    * source missing, unreadable, or not a regular file → not current (the
      caller re-ingests, and the transport then raises the real error instead of
      the cache serving a ghost);
    * on-disk length != ``manifest.bytes`` → not current, **without hashing**;
    * ``sha256(source)`` != ``manifest.sha256_content`` → not current. Size alone
      would not do: a float re-encode, a different scenario year, or any edit
      that preserves the length is exactly what a size check waves through;
    * otherwise current — serve the cached blob, re-ingest nothing.
    """
    try:
        st = os.stat(source)
    except OSError:
        # Gone, or unreadable. Either way this entry must not be served: the
        # report's sharpest case is a corpus DELETED outright while the suite
        # kept passing.
        return False
    if not _stat.S_ISREG(st.st_mode):
        return False
    if st.st_size != manifest.bytes:
        return False
    try:
        digest = sha256_file(source)
    except OSError:
        # Unreadable half-way through ⇒ re-ingest and let the transport speak.
        return False
    return digest.lower() == (manifest.sha256_content or "").lower()
