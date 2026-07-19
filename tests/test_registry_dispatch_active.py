"""Registry dispatch for the **active** ``http``/``file``/``local`` backends.

Companion to ``test_registry_dispatch.py`` (the stubs): the same name-resolution
seam (``spec/registries.md`` §4) now resolves the active core backends
(``esio-9nb.2``), which are interface-conformant and report ``status:"active"`` —
coexisting with the ``s3``/``zarr`` stubs without collision.
"""

from __future__ import annotations

import pytest

from earthsciio import (
    BackendNotRegistered,
    Store,
    Transport,
    store_registry,
    transport_registry,
)


def test_http_resolves_by_scheme():
    for scheme in ("http", "https"):
        impl = transport_registry.create(scheme)
        assert isinstance(impl, Transport)
        assert set(impl.schemes()) == {"http", "https"}
    assert transport_registry.status("http") == "active"
    assert not transport_registry.is_stub("https")


def test_file_resolves_by_scheme():
    impl = transport_registry.create("file")
    assert isinstance(impl, Transport)
    assert impl.schemes() == ["file"]
    assert transport_registry.status("file") == "active"


def test_local_store_resolves_by_name(cache_root):
    impl = store_registry.create("local", root=cache_root)
    assert isinstance(impl, Store)
    assert impl.name() == "local"
    assert store_registry.status("local") == "active"


def test_active_and_stub_backends_coexist():
    # The s3 transport is now active (anonymous rewrite -> HTTPS); the s3 STORE
    # remains the stub, coexisting with the active core backends.
    assert store_registry.is_stub("s3")
    assert not transport_registry.is_stub("s3")
    assert not transport_registry.is_stub("http")
    assert set(transport_registry.keys()) >= {"http", "https", "file", "s3"}
    assert set(store_registry.names()) >= {"local", "s3"}


def test_unknown_scheme_is_a_registration_gap():
    with pytest.raises(BackendNotRegistered):
        transport_registry.create("ftp")
