"""A MISSING key must read as absence, not as an error — the zarr Store contract.

zarr-python 3 opens an array by probing ``zarr.json`` (the v3 metadata) FIRST and
falling back to ``.zarray`` (v2) when the store answers "no such key". The
cache-backed store adapter only mapped :class:`CacheMiss` to ``None``; a real
fetch that returned HTTP 404 raised :class:`FetchError` straight through the
probe, so **every Zarr v2 store reachable over http/s3 was unreadable** — the
first thing zarr asked for was the one key that does not exist.

The existing zarr tests all run against a pre-seeded OFFLINE cache, where a
missing key raises ``CacheMiss`` and was already handled correctly. That is
precisely why this went unnoticed: the offline path is fine and the online path
is the broken one.

The distinction that matters is DEFINITIVE ABSENCE (404/410, a missing local
file) versus UNKNOWN (timeout, 5xx, 403). Only the former may become ``None``:
reporting "absent" for a transient fault would silently present a live store as
empty instead of failing loudly.
"""

from __future__ import annotations

import json

import numpy as np
import pytest

numcodecs = pytest.importorskip("numcodecs")
pytest.importorskip("zarr")

from earthsciio import Cache, DataSource, Provider
from earthsciio.errors import FetchError, TransportError


def _blosc():
    return numcodecs.Blosc(cname="lz4", clevel=5, shuffle=numcodecs.Blosc.SHUFFLE, blocksize=0)


# --------------------------------------------------------------------------- #
# The error-classification contract.
# --------------------------------------------------------------------------- #


def test_transport_error_carries_status_and_not_found():
    e = TransportError("http GET returned 404 for x", status=404, not_found=True)
    assert e.status == 404 and e.not_found is True
    # A plain message keeps the old positional construction working.
    plain = TransportError("boom")
    assert plain.status is None and plain.not_found is False


def test_fetch_error_not_found_requires_every_candidate_absent():
    absent = TransportError("404", status=404, not_found=True)
    transient = TransportError("timeout")

    assert FetchError("u", causes=[absent]).not_found is True
    assert FetchError("u", causes=[absent, absent]).not_found is True
    # ONE unknown outcome makes the whole thing unknown — a mirror that timed out
    # might well have had the object.
    assert FetchError("u", causes=[absent, transient]).not_found is False
    assert FetchError("u", causes=[]).not_found is False
    # Legacy single-cause construction still classifies.
    assert FetchError("u", cause=absent).not_found is True


# --------------------------------------------------------------------------- #
# End-to-end: a v2 store with NO zarr.json opens and reads.
# --------------------------------------------------------------------------- #


def _write_v2_store(root) -> str:
    """A real on-disk Zarr **v2** array (``.zarray``/``.zattrs``/chunks, and
    deliberately NO ``zarr.json``), served over the ``file`` transport so the
    v3 probe takes the definitive-absence path rather than CacheMiss."""
    arr_dir = root / "mini.zarr" / "field"
    arr_dir.mkdir(parents=True)
    shape, chunks = (6,), (3,)
    (arr_dir / ".zarray").write_text(json.dumps({
        "zarr_format": 2, "shape": list(shape), "chunks": list(chunks), "dtype": "<f8",
        "compressor": {"id": "blosc", "cname": "lz4", "clevel": 5, "shuffle": 1,
                       "blocksize": 0},
        "fill_value": 0.0, "order": "C", "filters": None, "dimension_separator": None,
    }))
    (arr_dir / ".zattrs").write_text(json.dumps({"_ARRAY_DIMENSIONS": ["i"]}))
    data = np.arange(6, dtype="<f8")
    for c in range(2):
        block = np.ascontiguousarray(data[c * 3 : (c + 1) * 3])
        (arr_dir / str(c)).write_bytes(bytes(_blosc().encode(block)))
    assert not (arr_dir / "zarr.json").exists(), "the fixture must be v2-only"
    return f"file://{root / 'mini.zarr'}"


def test_v2_store_without_zarr_json_is_readable(tmp_path):
    base = _write_v2_store(tmp_path / "store")
    cache = Cache(root=str(tmp_path / "cache"))
    loader = DataSource(name="mini", format="zarr", url=base, variables=["field"])

    ds = Provider(loader, cache).materialize()

    np.testing.assert_array_equal(
        np.asarray(ds["field"].data, dtype=float), np.arange(6, dtype=float)
    )


def test_v2_store_selection_still_pushes_down(tmp_path):
    """The absence fix must not disturb the projection pushdown."""
    base = _write_v2_store(tmp_path / "store")
    cache = Cache(root=str(tmp_path / "cache"))
    loader = DataSource(name="mini", format="zarr", url=base, variables=["field"])

    ds = Provider(loader, cache).materialize(select={"axes": [{"indices": [1, 4, 5]}]})

    np.testing.assert_array_equal(
        np.asarray(ds["field"].data, dtype=float), np.array([1.0, 4.0, 5.0])
    )


def test_a_transient_failure_is_not_reported_as_absence(tmp_path, monkeypatch):
    """A timeout during the probe must RAISE, not be swallowed into "no such
    key" — otherwise a live store reads as empty."""
    from earthsciio.backends import zarr as zarr_backend

    base = _write_v2_store(tmp_path / "store")
    cache = Cache(root=str(tmp_path / "cache"))

    real_fetch = Cache.fetch

    def flaky(self, url, *a, **kw):
        if url.endswith("zarr.json"):
            raise FetchError(url, causes=[TransportError("connection timed out")])
        return real_fetch(self, url, *a, **kw)

    monkeypatch.setattr(Cache, "fetch", flaky)
    store = zarr_backend._cache_store(cache, base) if hasattr(
        zarr_backend, "_cache_store"
    ) else None
    if store is None:
        loader = DataSource(name="mini", format="zarr", url=base, variables=["field"])
        with pytest.raises(FetchError):
            Provider(loader, cache).materialize()
    else:  # pragma: no cover - alternate constructor name
        with pytest.raises(FetchError):
            store._raw("zarr.json")
