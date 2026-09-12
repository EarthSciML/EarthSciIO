"""The cache validation ladder — is a cached blob a hit, or must we revalidate?

Ports the Rust ``validate::decide``. Given a stored manifest plus the loader's
:class:`Temporal` freshness policy, decide **hit / revalidate / miss** in this
order (first applicable wins, ``spec/cache-format.md`` §4):

0. **local source recheck** — a ``file://`` source is compared against the file
   it was ingested from (:func:`file_source_state`). Rules 1-4 are all about a
   source that can only be consulted over the network; a local file has an exact
   truth sitting on disk. It answers with four states, not a boolean: the
   difference between *gone*, *changed* and *cannot tell from here* is what the
   caller acts on. It is a separate function rather than a rung inside
   :func:`decide` because it touches the filesystem and ``decide`` is pure;
   :class:`~earthsciio.cache.Cache` applies it — via
   :class:`SourceRevalidator`, which memoises the hash — to the entries
   ``decide`` has already called a hit.
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
import threading
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


#: Rung 0 verdicts. A boolean cannot carry these: "the corpus was deleted",
#: "the corpus was replaced" and "this host cannot read the corpus" are three
#: different facts and :class:`~earthsciio.cache.Cache` acts differently on each.
CURRENT = "current"
REPLACED = "replaced"
MISSING = "missing"
UNKNOWN = "unknown"


def _stat_source(source):
    """``os.stat`` the source and classify what came back.

    The split that matters is :data:`errno.ENOENT` (the file is gone — the
    report's sharpest case, and it must stay a loud error) against every other
    error (``EACCES``, ``ENOTDIR``, an I/O failure — this host cannot see the
    corpus, which is not evidence that it changed). Warming a cache where
    ``/corpus`` is mounted and reading it where it is not must not be a hard
    failure.

    Returns ``(stat_result, None)`` or ``(None, verdict)``.
    """
    try:
        st = os.stat(source)
    except FileNotFoundError:
        return None, MISSING
    except OSError:
        return None, UNKNOWN
    if not _stat.S_ISREG(st.st_mode):
        # A directory standing where the corpus used to be is the report's
        # "pointed at a path that is not there any more" shape.
        return None, MISSING
    return st, None


class SourceRevalidator:
    """Rung 0 plus the fingerprint memo that makes it affordable per read.

    Hashing the source on every ``fetch`` is what rung 0 costs, and on the
    record-selective read paths ``fetch`` is called **per tick**, not per file:
    :meth:`earthsciio.provider.Provider._file_for` bypasses the decoded-file
    buffer whenever a ``select`` is passed, and the Julia provider keeps no such
    buffer at all. A 2.8 GB corpus would be re-hashed for every tick of a run.

    So a digest is remembered against the ``(st_size, st_mtime_ns)`` the source
    had when it was computed, and reused while both are unchanged — the same
    bargain ``make``, ``ninja`` and ``rsync`` strike. The memo lives on the
    :class:`~earthsciio.cache.Cache` and dies with it, so a fresh process always
    pays one real read per file; what it removes is the *repeat* read within a
    run.

    What it can miss: a replacement preserving both the length and the
    nanosecond mtime, in-process, after the file was already read once
    (``cp --preserve=timestamps`` over a same-length file can do it). The window
    is one process lifetime.

    Instances are safe to share between threads.
    """

    #: How many fingerprints one revalidator remembers before dropping the lot.
    #: A read loop touches a handful of files; this only stops an unbounded walk
    #: from growing the map without limit.
    MEMO_CAP = 512

    def __init__(self) -> None:
        self._seen: dict = {}
        self._lock = threading.Lock()
        #: Times a source was actually read end to end. Tests assert the memo
        #: saves the repeat reads; nothing else should depend on it.
        self.full_reads = 0

    def state(self, source: os.PathLike, manifest: Manifest) -> str:
        """Rung 0: the state of ``source`` relative to ``manifest``."""
        st, verdict = _stat_source(source)
        if verdict is not None:
            return verdict
        # A different length is a different file, and no read is needed to say
        # so. This is the cheap half of the check and every track keeps it.
        if st.st_size != manifest.bytes:
            return REPLACED
        key = os.fspath(source)
        fingerprint = (st.st_size, st.st_mtime_ns)
        with self._lock:
            seen = self._seen.get(key)
        if seen is not None and seen[0] == fingerprint:
            return _verdict(seen[1], manifest)
        try:
            digest = sha256_file(source)
        except FileNotFoundError:
            return MISSING
        except OSError:
            # It survived ``stat`` but not ``open``: still "cannot tell".
            return UNKNOWN
        with self._lock:
            self.full_reads += 1
            # Crude but sufficient: the memo is an optimisation, so dropping all
            # of it costs one re-read per live file rather than needing an LRU.
            if len(self._seen) >= self.MEMO_CAP:
                self._seen.clear()
            self._seen[key] = (fingerprint, digest)
        return _verdict(digest, manifest)


def _verdict(digest: str, manifest: Manifest) -> str:
    return (
        CURRENT
        if digest.lower() == (manifest.sha256_content or "").lower()
        else REPLACED
    )


def file_source_state(source: os.PathLike, manifest: Manifest) -> str:
    """Rung 0 without a memo: what is the ``file://`` source behind this entry?

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

    * nothing at the path, or not a regular file → :data:`MISSING`;
    * the path cannot be read at all → :data:`UNKNOWN`, and rung 0 abstains
      rather than claim a change it did not observe;
    * on-disk length != ``manifest.bytes`` → :data:`REPLACED`, **without
      hashing**;
    * ``sha256(source)`` != ``manifest.sha256_content`` → :data:`REPLACED`. Size
      alone would not do: a float re-encode, a different scenario year, or any
      edit that preserves the length is exactly what a size check waves through;
    * otherwise :data:`CURRENT` — serve the cached blob, re-ingest nothing.

    A :class:`~earthsciio.cache.Cache` calls :meth:`SourceRevalidator.state`
    instead, which is this with a ``(size, mtime)`` memo in front of the read;
    use this when there is nothing to amortise over.
    """
    return SourceRevalidator().state(source, manifest)


def file_source_is_current(source: os.PathLike, manifest: Manifest) -> bool:
    """Rung 0 as a yes/no: is the source **provably** the file that was ingested?

    Only :data:`CURRENT` is ``True``. The cache does not use this — the
    difference between :data:`MISSING`, :data:`REPLACED` and :data:`UNKNOWN` is
    exactly what it acts on — but it is the honest predicate for a caller that
    just wants the question answered.
    """
    return file_source_state(source, manifest) == CURRENT
