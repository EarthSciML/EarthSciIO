"""The cache validation ladder — is a cached blob a hit, or must we revalidate?

Ports the Rust ``validate::decide``. Given a stored manifest plus the loader's
:class:`Temporal` freshness policy, decide **hit / revalidate / miss** in this
order (first applicable wins, ``spec/cache-format.md`` §4):

1. **content hash** — if a loader-declared checksum exists, compare it to
   ``manifest.sha256_content``. Strongest. (No loader declares one today; this is
   the future ``source.checksums`` hook.)
2. **conditional GET** — if ``etag``/``last_modified`` are stored, revalidate
   over the network (``If-None-Match`` / ``If-Modified-Since``). Validators beat
   heuristic freshness, so this fires **before** TTL.
3. **TTL from ``temporal``** — a closed past period is immutable (infinite TTL);
   a current/incomplete period is fresh only until its TTL elapses; a static
   loader (no ``temporal``) is immutable once fetched.

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
    # 2. conditional GET when validators are stored
    if manifest.etag or manifest.last_modified:
        return REVALIDATE
    # 3. TTL from temporal
    if temporal is None or temporal.immutable:
        return HIT
    return HIT if is_fresh(manifest.fetched_at, temporal.ttl, now) else MISS
