"""Shared fixtures for the cache-core test suite (``esio-9nb.2``).

The whole suite is **hermetic**: the autouse fixture clears the three ambient
environment knobs (``EARTHSCIDATADIR`` / ``EARTHSCI_OFFLINE`` / ``EARTHSCI_LIVE``)
so a test's behavior never depends on the refinery's environment, and offline
tests never accidentally see a leaked datadir. Tests that exercise those knobs
set them explicitly via ``monkeypatch``.
"""

from __future__ import annotations

import pathlib

import pytest

REPO_ROOT = pathlib.Path(__file__).resolve().parent.parent
CORPUS_CACHE = REPO_ROOT / "conformance" / "corpus" / "cache"
CORPUS_DIR = REPO_ROOT / "conformance" / "corpus"


@pytest.fixture(autouse=True)
def _hermetic_env(monkeypatch):
    for var in ("EARTHSCIDATADIR", "EARTHSCI_OFFLINE", "EARTHSCI_LIVE"):
        monkeypatch.delenv(var, raising=False)


@pytest.fixture
def cache_root(tmp_path):
    """A fresh, empty ``$EARTHSCIDATADIR`` for one test."""
    root = tmp_path / "cache"
    root.mkdir()
    return root
