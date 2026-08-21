"""The validation ladder (``spec/cache-format.md`` §4), ported from Rust.

Order (first wins): content-hash → conditional-GET (etag/last-modified present)
→ TTL from ``temporal`` (static/closed = immutable; incomplete = TTL-bounded).
"""

from __future__ import annotations

import datetime as _dt

from earthsciio import Manifest
from earthsciio.validate import HIT, MISS, REVALIDATE, Temporal, decide, is_fresh

NOW = _dt.datetime(2026, 6, 26, 12, 0, 0, tzinfo=_dt.timezone.utc)


def _manifest(*, etag=None, last_modified=None, sha="abc123",
              fetched_at="2026-06-26T11:00:00Z"):
    return Manifest(
        url="https://h/x.nc", sha256_content=sha, bytes=1,
        fetched_at=fetched_at, etag=etag, last_modified=last_modified,
    )


def test_content_hash_wins_first():
    m = _manifest(etag='"v1"')  # etag present, but checksum check is checked first
    assert decide(m, expected_checksum="ABC123") == HIT  # case-insensitive
    assert decide(m, expected_checksum="deadbeef") == MISS


def test_an_immutable_source_is_never_revalidated_even_with_validators():
    """Rule 2 beats rule 3 — the regression guard for the ordering fix.

    This is the case that matters in production and the one the old order got
    wrong: S3 returns an ETag on EVERY object, so if validators were checked
    first, no S3-backed store could ever take the immutable path and every warm
    cache hit paid a network round-trip to be told "unchanged". Measured on the
    ISRM zarr store that was 85.9 ms per chunk against a 0.078 ms local read.
    """
    for m in (
        _manifest(etag='"v1"'),
        _manifest(last_modified="Wed, 08 Nov 2018 00:00:00 GMT"),
        _manifest(etag='"v1"', last_modified="Wed, 08 Nov 2018 00:00:00 GMT"),
    ):
        assert decide(m, None) == HIT
        assert decide(m, Temporal.static()) == HIT
        assert decide(m, Temporal.closed_period()) == HIT


def test_validators_still_beat_the_ttl_heuristic():
    """Rule 3 beats rule 4. Only the DECLARATION outranks a conditional GET."""
    stale = Temporal.incomplete(_dt.timedelta(minutes=30))
    fresh = Temporal.incomplete(_dt.timedelta(hours=2))
    # 1 h old: stale on TTL, fresh on TTL — validators decide either way.
    m = _manifest(fetched_at="2026-06-26T11:00:00Z", etag='"v1"')
    assert decide(m, stale, now=NOW) == REVALIDATE
    assert decide(m, fresh, now=NOW) == REVALIDATE
    m = _manifest(
        fetched_at="2026-06-26T11:00:00Z",
        last_modified="Wed, 08 Nov 2018 00:00:00 GMT",
    )
    assert decide(m, stale, now=NOW) == REVALIDATE


def test_static_and_closed_period_are_immutable_hits():
    m = _manifest()  # no validators
    assert decide(m, None) == HIT
    assert decide(m, Temporal.static()) == HIT
    assert decide(m, Temporal.closed_period()) == HIT


def test_incomplete_period_respects_ttl():
    m = _manifest(fetched_at="2026-06-26T11:00:00Z")  # 1h before NOW
    fresh = Temporal.incomplete(_dt.timedelta(hours=2))
    stale = Temporal.incomplete(_dt.timedelta(minutes=30))
    assert decide(m, fresh, now=NOW) == HIT
    assert decide(m, stale, now=NOW) == MISS


def test_is_fresh_edge_cases():
    # unparseable timestamp ⇒ not fresh (force revalidation)
    assert is_fresh("not-a-date", 3600, now=NOW) is False
    # future fetched_at (clock skew) ⇒ treated as just-fetched
    future = "2026-06-26T13:00:00Z"
    assert is_fresh(future, 60, now=NOW) is True
    # plain seconds ttl accepted
    assert is_fresh("2026-06-26T11:59:30Z", 60, now=NOW) is True
    assert is_fresh("2026-06-26T11:58:00Z", 60, now=NOW) is False
