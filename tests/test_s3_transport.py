"""The active ``s3`` transport — anonymous ``s3://`` → regional HTTPS rewrite.

The S3 transport is a thin URL rewriter over the ``http`` transport: it keeps the
canonical ``s3://<bucket>/<key>`` URL (so the cache key + ``manifest.url`` stay
scheme-stable, like ``cds://``) and delegates a plain anonymous GET to a held
``HttpTransport`` against the regional virtual-hosted host. These tests pin the
rewrite (region resolution + URL shape) and the delegation, mocking the HTTP
delegate so no socket is opened.
"""

from __future__ import annotations

import pytest

from earthsciio import transport_registry
from earthsciio.backends.s3 import (
    DEFAULT_REGION,
    S3ObjectStore,
    S3Transport,
    parse_s3_url,
    resolve_region,
    s3_https_url,
)


def test_registered_active():
    assert transport_registry.status("s3") == "active"
    assert not transport_registry.is_stub("s3")
    impl = transport_registry.create("s3")
    assert impl.schemes() == ["s3"]


def test_rewrite_default_region():
    url = "s3://inmap-model/isrm_v1.2.1.zarr/PrimaryPM25/.zarray"
    assert s3_https_url(url) == (
        "https://inmap-model.s3.us-east-2.amazonaws.com/"
        "isrm_v1.2.1.zarr/PrimaryPM25/.zarray"
    )


def test_rewrite_chunk_key_with_dots():
    url = "s3://inmap-model/isrm_v1.2.1.zarr/PrimaryPM25/0.5.0"
    assert s3_https_url(url) == (
        "https://inmap-model.s3.us-east-2.amazonaws.com/isrm_v1.2.1.zarr/PrimaryPM25/0.5.0"
    )


def test_rewrite_explicit_region():
    assert s3_https_url("s3://b/k/o", region="eu-west-1") == (
        "https://b.s3.eu-west-1.amazonaws.com/k/o"
    )


def test_region_resolution_precedence(monkeypatch):
    monkeypatch.delenv("EARTHSCI_S3_REGION", raising=False)
    monkeypatch.delenv("AWS_REGION", raising=False)
    assert resolve_region() == DEFAULT_REGION == "us-east-2"
    monkeypatch.setenv("AWS_REGION", "ap-south-1")
    assert resolve_region() == "ap-south-1"
    monkeypatch.setenv("EARTHSCI_S3_REGION", "us-west-2")
    assert resolve_region() == "us-west-2"  # EARTHSCI_S3_REGION wins
    assert resolve_region("me-central-1") == "me-central-1"  # explicit wins over all


def test_rewrite_rejects_bad_urls():
    with pytest.raises(ValueError):
        s3_https_url("https://not-s3/x")
    with pytest.raises(ValueError):
        s3_https_url("s3://bucket-only")  # no key
    with pytest.raises(ValueError):
        s3_https_url("s3:///key")  # empty bucket


def test_fetch_delegates_to_http_with_rewritten_url():
    calls = {}

    class _FakeHttp:
        def fetch(self, url, dest, conditional=None, auth=None):
            calls["url"] = url
            calls["dest"] = dest
            calls["conditional"] = conditional
            calls["auth"] = auth
            return "FETCH_RESULT"

    t = S3Transport(http=_FakeHttp())
    out = t.fetch(
        "s3://inmap-model/isrm_v1.2.1.zarr/pNO3/.zattrs",
        "/tmp/x.part",
        conditional={"etag": '"abc"'},
        auth="AUTH",
    )
    assert out == "FETCH_RESULT"
    assert calls["url"] == (
        "https://inmap-model.s3.us-east-2.amazonaws.com/isrm_v1.2.1.zarr/pNO3/.zattrs"
    )
    assert calls["dest"] == "/tmp/x.part"
    assert calls["conditional"] == {"etag": '"abc"'}  # threaded through unchanged
    assert calls["auth"] == "AUTH"  # auth resolver threaded through unchanged


# --------------------------------------------------------------------------- #
# S3ObjectStore — the real s3fs/fsspec-backed object store (write mirror of the
# reader). Exercised offline against an fsspec in-memory filesystem so no socket
# or AWS credential is touched; s3fs is used unchanged in production.
# --------------------------------------------------------------------------- #


def _mem_fs():
    fsspec = pytest.importorskip("fsspec")
    fs = fsspec.filesystem("memory")
    # a fresh in-memory fs per test (MemoryFileSystem shares a class-level store)
    fs.store.clear()
    try:
        fs.pseudo_dirs[:] = [""]
    except Exception:
        pass
    return fs


def test_parse_s3_url():
    assert parse_s3_url("s3://bucket/prefix/x.zarr") == ("bucket", "prefix/x.zarr")
    assert parse_s3_url("s3://bucket") == ("bucket", "")
    with pytest.raises(ValueError):
        parse_s3_url("https://not-s3/x")
    with pytest.raises(ValueError):
        parse_s3_url("s3:///key")  # empty bucket


def test_object_store_key_path_mapping():
    s = S3ObjectStore("s3://bucket/prefix/x.zarr", fs=_mem_fs())
    assert s.bucket == "bucket"
    assert s.prefix == "prefix/x.zarr"
    assert s._path("temp/c/0/0") == "bucket/prefix/x.zarr/temp/c/0/0"
    # no prefix → key sits directly under the bucket
    s2 = S3ObjectStore("s3://bucket", fs=_mem_fs())
    assert s2._path("zarr.json") == "bucket/zarr.json"


def test_object_store_put_get_exists_delete():
    s = S3ObjectStore("s3://bucket/x.zarr", fs=_mem_fs())
    assert s.get_bytes("temp/zarr.json") is None  # clean miss
    assert s.exists("temp/zarr.json") is False
    s.put_bytes("temp/zarr.json", b'{"zarr_format": 3}')
    s.put_bytes("temp/c/0/0", b"\x01\x02\x03chunk")
    assert s.exists("temp/zarr.json") is True
    assert s.get_bytes("temp/zarr.json") == b'{"zarr_format": 3}'
    assert s.get_bytes("temp/c/0/0") == b"\x01\x02\x03chunk"
    s.delete("temp/zarr.json")
    assert s.exists("temp/zarr.json") is False
    assert s.name() == "s3-object"
