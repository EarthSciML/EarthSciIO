"""The pluggable auth seam (``spec/cache-format.md`` §3).

A realm → resolver map produces request headers; credentials never appear in the
manifest (only the realm name). A fetch that declares an unknown realm is
fail-closed (:class:`AuthError`).
"""

from __future__ import annotations

import pytest

from earthsciio import AuthRegistry, StaticHeaderAuth
from earthsciio.auth import coerce_auth
from earthsciio.errors import AuthError


def test_bearer_resolver_headers():
    r = StaticHeaderAuth.bearer("cds", "secret-token")
    assert r.realm() == "cds"
    assert r.headers() == [("Authorization", "Bearer secret-token")]


def test_custom_header_resolver_firms():
    r = StaticHeaderAuth.header("firms", "X-API-Key", "abc123")
    assert r.headers() == [("X-API-Key", "abc123")]


def test_registry_from_dict_and_resolve():
    reg = AuthRegistry(
        {
            "cds": StaticHeaderAuth.bearer("cds", "t1"),
            "firms": StaticHeaderAuth.header("firms", "X-API-Key", "k"),
        }
    )
    assert reg.realms() == ["cds", "firms"]
    assert reg.resolve("cds").headers() == [("Authorization", "Bearer t1")]
    assert reg.resolve(None) is None  # a fetch with no realm needs no auth


def test_unknown_realm_is_fail_closed():
    reg = AuthRegistry(StaticHeaderAuth.bearer("cds", "t"))
    with pytest.raises(AuthError) as ei:
        reg.resolve("rda")
    assert ei.value.realm == "rda"
    assert "cds" in ei.value.available


def test_coerce_auth_accepts_resolver_iterable_dict_and_none():
    assert coerce_auth(None).realms() == []
    single = coerce_auth(StaticHeaderAuth.bearer("rda", "t"))
    assert single.realms() == ["rda"]
    many = coerce_auth(
        [StaticHeaderAuth.bearer("a", "1"), StaticHeaderAuth.bearer("b", "2")]
    )
    assert many.realms() == ["a", "b"]
    # an AuthRegistry passes through unchanged
    reg = AuthRegistry()
    assert coerce_auth(reg) is reg
