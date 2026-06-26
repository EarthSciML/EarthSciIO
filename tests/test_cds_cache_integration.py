"""End-to-end: the content-addressed cache dispatches the ``cds`` transport
(``esio-9nb.10``) over the hermetic mock CDS server.

Proves the wiring the bead asks for: ``Cache.fetch(cds://…)`` resolves the
``cds`` transport by scheme, runs submit→poll→download, content-addresses the
asset, records the ``cds`` realm + loader in the manifest (never the key), honors
**skip-if-exists** (a repeat request is a hit — no second CDS job), reuses the
blob **offline** with no socket, and is **fail-closed** when the declared realm
has no resolver. ``CDSAPI_URL`` points the cache-constructed transport at the
mock; the default first-poll-successful keeps it fast.
"""

from __future__ import annotations

import pytest

from earthsciio import AuthError, Cache, cache_key, cds_auth, era5


def _era5_url():
    return era5.era5_cds_url(
        2018, 11, [8], ["temperature"], [1000, 500],
        era5.era5_area_from_bbox(-100.5, 39.0, -80.2, 45.7),
    )


@pytest.fixture
def cds_cache(cds_server, cache_root, monkeypatch):
    """A cache whose ``cds`` transport is pointed at the mock + given a token."""
    monkeypatch.setenv("CDSAPI_URL", cds_server.api_url)
    return Cache(root=cache_root, auth={"cds": cds_auth("tok-int")})


def test_cache_dispatches_cds_and_records_manifest(cds_server, cds_cache):
    entry = cds_cache.fetch(_era5_url(), source_loader="era5", auth_realm="cds")
    assert entry.status == "downloaded"
    assert entry.path.read_bytes() == cds_server.payload
    assert entry.key == cache_key(_era5_url())
    # provenance recorded; the realm name is stored, never the token
    assert entry.manifest.source_loader == "era5"
    assert entry.manifest.auth_realm == "cds"
    assert "tok-int" not in (entry.manifest.url or "")
    assert cds_server.submits == 1 and cds_server.downloads == 1


def test_skip_if_exists_no_second_job(cds_server, cds_cache):
    url = _era5_url()
    assert cds_cache.fetch(url, auth_realm="cds").status == "downloaded"
    # identical request ⇒ identical cds:// URL ⇒ identical key ⇒ cache hit
    second = cds_cache.fetch(url, auth_realm="cds")
    assert second.status == "hit"
    assert second.path.read_bytes() == cds_server.payload
    assert cds_server.submits == 1  # the CDS job ran exactly once
    assert cds_server.downloads == 1


def test_offline_after_cds_is_a_hit_with_no_socket(cds_server, cds_cache, cache_root):
    url = _era5_url()
    cds_cache.fetch(url, auth_realm="cds")  # populate online
    submits_before, downloads_before = cds_server.submits, cds_server.downloads
    offline = Cache(root=cache_root, offline=True)  # new cache, same root
    entry = offline.fetch(url)
    assert entry.status == "hit"
    assert entry.path.read_bytes() == cds_server.payload
    # offline consulted no transport: the mock saw no new traffic
    assert cds_server.submits == submits_before
    assert cds_server.downloads == downloads_before


def test_declared_realm_without_resolver_is_fail_closed(cds_server, cache_root, monkeypatch):
    monkeypatch.setenv("CDSAPI_URL", cds_server.api_url)
    cache = Cache(root=cache_root)  # no auth registered at all
    with pytest.raises(AuthError):
        cache.fetch(_era5_url(), auth_realm="cds")
    assert cds_server.submits == 0  # never reached the network
