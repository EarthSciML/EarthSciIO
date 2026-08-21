"""The cache validation ladder — is a cached blob a hit, or must we revalidate?

Ports the Rust ``validate::decide``. Given a stored manifest plus the loader's
:class:`Temporal` freshness policy, decide **hit / revalidate / miss** in this
order (first applicable wins, ``spec/cache-format.md`` §4):

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
from dataclasses import dataclass
from enum import Enum
from typing import Optional, Union

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
