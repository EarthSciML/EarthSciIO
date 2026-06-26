"""``$EARTHSCIDATADIR`` resolution + offline detection + ``${...}`` expansion.

Pins ``spec/cache-format.md`` §5 (env wins; default on ``/scratch.local``, never
``/u`` — Risk R6) and ``spec/offline-mode.md`` §1 (explicit arg wins over env).
"""

from __future__ import annotations

import pathlib

from earthsciio import config


def test_default_root_on_scratch_never_home(monkeypatch):
    monkeypatch.delenv("EARTHSCIDATADIR", raising=False)
    monkeypatch.setenv("USER", "alice")
    root = config.default_cache_root()
    assert root == pathlib.Path("/scratch.local/alice/earthsci-cache")
    # Hard rule R6: the home inode quota must never be the default.
    assert str(root).startswith("/scratch.local/")
    assert not str(root).startswith("/u/")


def test_env_wins_over_default(monkeypatch):
    monkeypatch.setenv("EARTHSCIDATADIR", "/data/cache")
    assert config.resolve_cache_root() == pathlib.Path("/data/cache")


def test_explicit_arg_overrides_env(monkeypatch):
    monkeypatch.setenv("EARTHSCIDATADIR", "/data/cache")
    assert config.resolve_cache_root("/override") == pathlib.Path("/override")


def test_empty_env_falls_back_to_default(monkeypatch):
    monkeypatch.setenv("EARTHSCIDATADIR", "")
    monkeypatch.setenv("USER", "bob")
    assert config.resolve_cache_root() == pathlib.Path(
        "/scratch.local/bob/earthsci-cache"
    )


def test_env_offline_truthy_and_falsy(monkeypatch):
    for truthy in ("1", "true", "TRUE", "yes", "On"):
        monkeypatch.setenv("EARTHSCI_OFFLINE", truthy)
        assert config.env_offline() is True, truthy
    for falsy in ("0", "false", "no", ""):
        monkeypatch.setenv("EARTHSCI_OFFLINE", falsy)
        assert config.env_offline() is False, falsy


def test_resolve_offline_explicit_wins_over_env(monkeypatch):
    monkeypatch.setenv("EARTHSCI_OFFLINE", "1")
    assert config.resolve_offline(False) is False  # explicit False beats env
    assert config.resolve_offline(True) is True
    assert config.resolve_offline(None) is True  # None ⇒ consult env


def test_expand_datadir_in_file_template():
    root = "/scratch.local/u/earthsci-cache"
    assert (
        config.expand_datadir("file://${EARTHSCIDATADIR}/nei2016/x.nc", root)
        == "file:///scratch.local/u/earthsci-cache/nei2016/x.nc"
    )
    assert (
        config.expand_datadir("$EARTHSCIDATADIR/mirror", root)
        == "/scratch.local/u/earthsci-cache/mirror"
    )
