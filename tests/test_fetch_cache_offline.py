"""Fetch → cache → offline reuse — the core acceptance for ``esio-9nb.2``.

Hermetic: uses the ``file`` transport (no sockets). Proves a fetch caches the
blob + writes its manifest; a second fetch is a hit; a fresh **offline** cache
re-reads the same bytes from the same root; an offline miss raises
:class:`CacheMiss`; ``verify`` catches on-disk corruption; ``expected_checksum``
and TTL are honored; mirror failover tries candidates in order and records the
canonical URL; and the conformance corpus resolves entirely offline.
"""

from __future__ import annotations

import json
import os
import pathlib

import pytest

from earthsciio import (
    Cache,
    CacheMiss,
    FetchError,
    IntegrityError,
    cache_key,
    sha256_file,
)
from earthsciio.validate import Temporal

REPO_ROOT = pathlib.Path(__file__).resolve().parent.parent
CORPUS = REPO_ROOT / "conformance" / "corpus"

DATA = b"\x89NETCDF-ish\x00bytes\x01\x02\x03" * 8


def _file_url(p) -> str:
    return "file://" + str(pathlib.Path(p).resolve())


def _src(tmp_path, name="era5.nc", data=DATA):
    p = tmp_path / name
    p.write_bytes(data)
    return p, data


# --------------------------------------------------------------------------- #
# Fetch + cache + manifest.
# --------------------------------------------------------------------------- #


def test_fetch_caches_and_writes_manifest(cache_root, tmp_path):
    src, data = _src(tmp_path)
    url = _file_url(src)
    entry = Cache(root=cache_root).fetch(url, source_loader="era5")
    assert entry.status == "downloaded"
    assert entry.key == cache_key(url)
    assert entry.path.read_bytes() == data
    # manifest: integrity + provenance (no credentials, canonical url)
    assert entry.manifest.url == url
    assert entry.manifest.bytes == len(data)
    assert entry.manifest.sha256_content == sha256_file(src)
    assert entry.manifest.source_loader == "era5"
    assert entry.manifest.auth_realm is None
    # on-disk layout: blobs/<key[:2]>/<key>.nc
    assert entry.path.parent.name == entry.key[:2]
    assert entry.path.suffix == ".nc"


def test_second_fetch_is_a_hit_without_redownload(cache_root, tmp_path):
    src, data = _src(tmp_path)
    url = _file_url(src)
    c = Cache(root=cache_root)
    assert c.fetch(url).status == "downloaded"
    src.write_bytes(b"MUTATED-SOURCE")  # a static-loader hit must not re-copy
    entry = c.fetch(url)
    assert entry.status == "hit"
    assert entry.path.read_bytes() == data  # original bytes, not the mutation


# --------------------------------------------------------------------------- #
# Offline mode.
# --------------------------------------------------------------------------- #


def test_offline_reuse_same_bytes(cache_root, tmp_path):
    src, data = _src(tmp_path)
    url = _file_url(src)
    Cache(root=cache_root).fetch(url)  # populate online
    offline = Cache(root=cache_root, offline=True)  # new cache, same root
    entry = offline.fetch(url)
    assert entry.status == "hit"
    assert entry.path.read_bytes() == data


def test_offline_miss_raises_cachemiss(cache_root):
    url = "https://data.earthsci.dev/era5/2099/01/nope.nc"
    with pytest.raises(CacheMiss) as ei:
        Cache(root=cache_root, offline=True).fetch(url)
    assert ei.value.resolved_url == url
    assert ei.value.key == cache_key(url)


def test_offline_enabled_by_env(cache_root, tmp_path, monkeypatch):
    src, _ = _src(tmp_path)
    url = _file_url(src)
    Cache(root=cache_root).fetch(url)  # populate while online
    monkeypatch.setenv("EARTHSCI_OFFLINE", "1")
    c = Cache(root=cache_root)  # offline=None ⇒ consult env
    assert c.offline is True
    assert c.fetch(url).status == "hit"


# --------------------------------------------------------------------------- #
# Integrity: verify-on-read + expected_checksum.
# --------------------------------------------------------------------------- #


def test_verify_on_read_detects_corruption(cache_root, tmp_path):
    src, _ = _src(tmp_path)
    url = _file_url(src)
    entry = Cache(root=cache_root).fetch(url)
    entry.path.write_bytes(b"corrupt")  # tamper with the cached blob
    with pytest.raises(IntegrityError):
        Cache(root=cache_root, offline=True, verify=True).fetch(url)


def test_expected_checksum_mismatch_raises_and_commits_nothing(cache_root, tmp_path):
    src, _ = _src(tmp_path)
    url = _file_url(src)
    with pytest.raises(IntegrityError):
        Cache(root=cache_root).fetch(url, expected_checksum="0" * 64)
    assert not Cache(root=cache_root, offline=True).store.exists(cache_key(url))


def test_expected_checksum_match_caches(cache_root, tmp_path):
    src, _ = _src(tmp_path)
    url = _file_url(src)
    good = sha256_file(src)
    entry = Cache(root=cache_root).fetch(url, expected_checksum=good)
    assert entry.status == "downloaded"
    assert entry.manifest.sha256_content == good


# --------------------------------------------------------------------------- #
# TTL (validation ladder, integration).
# --------------------------------------------------------------------------- #


def test_ttl_stale_incomplete_period_refetches(cache_root, tmp_path):
    src, _ = _src(tmp_path)
    url = _file_url(src)
    c = Cache(root=cache_root)
    assert c.fetch(url, temporal=Temporal.incomplete(0)).status == "downloaded"
    src.write_bytes(b"NEWDATA-NEWDATA-NEWDATA")  # ttl=0 ⇒ always stale ⇒ re-copy
    entry = c.fetch(url, temporal=Temporal.incomplete(0))
    assert entry.status == "downloaded"
    assert entry.path.read_bytes() == b"NEWDATA-NEWDATA-NEWDATA"


def test_ttl_fresh_incomplete_period_is_a_hit(cache_root, tmp_path):
    src, data = _src(tmp_path)
    url = _file_url(src)
    c = Cache(root=cache_root)
    c.fetch(url, temporal=Temporal.incomplete(3600))
    src.write_bytes(b"changed")
    entry = c.fetch(url, temporal=Temporal.incomplete(3600))
    assert entry.status == "hit"
    assert entry.path.read_bytes() == data


# --------------------------------------------------------------------------- #
# Mirror failover.
# --------------------------------------------------------------------------- #


def test_mirror_failover_uses_first_working_candidate(cache_root, tmp_path):
    src, data = _src(tmp_path)
    good = _file_url(src)
    dead = _file_url(tmp_path / "missing-primary.nc")  # primary does not exist
    entry = Cache(root=cache_root).fetch(dead, mirrors=[good])
    assert entry.status == "downloaded"
    assert entry.path.read_bytes() == data
    # the key + manifest record the CANONICAL url, never the mirror that served it
    assert entry.key == cache_key(dead)
    assert entry.manifest.url == dead


def test_all_mirrors_failing_raises_fetcherror(cache_root, tmp_path):
    dead1 = _file_url(tmp_path / "no1.nc")
    dead2 = _file_url(tmp_path / "no2.nc")
    with pytest.raises(FetchError) as ei:
        Cache(root=cache_root).fetch(dead1, mirrors=[dead2])
    assert dead1 in ei.value.attempts
    assert dead2 in ei.value.attempts


# --------------------------------------------------------------------------- #
# The conformance corpus resolves offline (the campfire-C2 hermetic guarantee).
# --------------------------------------------------------------------------- #


def test_conformance_corpus_resolves_offline():
    index = json.loads((CORPUS / "cases.json").read_text())
    offline = Cache(root=CORPUS / "cache", offline=True, verify=True)
    assert index["cases"], "corpus index is empty"
    for entry_idx in index["cases"]:
        case = json.loads((CORPUS / entry_idx["file"]).read_text())
        result = offline.fetch(case["resolved_url"])
        assert result.status == "hit", case["id"]
        assert result.key == case["cache_key"], case["id"]
        assert sha256_file(result.path) == case["content_sha256"], case["id"]
        assert os.path.getsize(result.path) == case["bytes"], case["id"]
