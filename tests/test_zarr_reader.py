"""The active, store-backed ``zarr`` reader (Zarr v2 chunked arrays).

Covers the chunk math, orthogonal selection, the partial edge chunk, the
fill-for-absent-chunk rule, the ``fill_value != NaN`` deviation, and — the
load-bearing capability — **laziness**: a runtime index list on a dimension
fetches ONLY the intersecting chunk objects, never the whole array. Laziness is
proven with a "poison" store: chunks that must NOT be read hold undecodable
garbage, so any over-fetch decode-errors instead of silently succeeding.
"""

from __future__ import annotations

import datetime as _dt

import numpy as np
import pytest

numcodecs = pytest.importorskip("numcodecs")

from earthsciio import (
    Cache,
    CSVReader,
    DataLoader,
    FF10Reader,
    Provider,
    cache_key,
    format_registry,
    supports_selection,
)
from earthsciio.backends.local import LocalStore
from earthsciio.backends.zarr import (
    ZarrReader,
    _chunk_key,
    _needed_chunks,
    _parse_axis,
    _resolve_axis_indices,
)
from earthsciio.cachekey import sha256_bytes
from earthsciio.manifest import Manifest

BASE = "s3://earthsci-fixtures/mini.zarr"


def _blosc():
    return numcodecs.Blosc(cname="lz4", clevel=5, shuffle=numcodecs.Blosc.SHUFFLE, blocksize=0)


def _zarray(shape, chunks, dtype="<f4"):
    import json

    return json.dumps({
        "zarr_format": 2, "shape": list(shape), "chunks": list(chunks), "dtype": dtype,
        "compressor": {"id": "blosc", "cname": "lz4", "clevel": 5, "shuffle": 1, "blocksize": 0},
        "fill_value": 0.0, "order": "C", "filters": None, "dimension_separator": None,
    }).encode()


def _encode_chunk(chunk):
    return bytes(_blosc().encode(np.ascontiguousarray(chunk)))


def _populate(root, objects):
    """Write ``{url: bytes}`` as offline cache blobs+manifests keyed by sha256(url)."""
    store = LocalStore(root)
    for url, data in objects.items():
        key = cache_key(url)
        staged = store.staging_path()
        staged.write_bytes(data)
        store.put_blob(key, staged, "")
        store.put_meta(key, Manifest(
            url=url, sha256_content=sha256_bytes(data), bytes=len(data),
            fetched_at="2026-06-26T00:00:00Z",
        ))


# --------------------------------------------------------------------------- #
# Pure chunk math.
# --------------------------------------------------------------------------- #


def test_chunk_key():
    assert _chunk_key((0, 5, 0), ".") == "0.5.0"
    assert _chunk_key((3,), ".") == "3"
    assert _chunk_key((1, 2), "/") == "1/2"


def test_needed_chunks_orthogonal_dedup_and_skip():
    # dim0 chunk_len 1: index 1 -> chunk {1}; dim1 chunk_len 100: [0, 250, 260]
    # -> chunks {0, 2}; dim2 all one chunk {0}.
    got = _needed_chunks([[1], [0, 250, 260], [0]], [1, 100, 1])
    assert got == [(1, 0, 0), (1, 2, 0)]  # chunk 1 (rows 100-199) is skipped


def test_needed_chunks_never_scans_whole_array():
    # 525 dim1 chunks of width 100; a 3-index selection touches <= 3 chunks.
    sel1 = [50, 12345, 52000]
    got = _needed_chunks([[0], sel1, [0]], [1, 100, 52411])
    dim1_chunks = {c[1] for c in got}
    assert dim1_chunks == {0, 123, 520}
    assert len(got) == 3  # never 525


def test_resolve_axis_slice_and_indices():
    assert _resolve_axis_indices(_parse_axis("all"), 4) == [0, 1, 2, 3]
    assert _resolve_axis_indices(_parse_axis({"indices": [3, 0, 1]}), 4) == [3, 0, 1]
    assert _resolve_axis_indices(_parse_axis({"slice": [1, 8, 2]}), 10) == [1, 3, 5, 7]
    with pytest.raises(IndexError):
        _resolve_axis_indices(_parse_axis({"indices": [9]}), 4)


# --------------------------------------------------------------------------- #
# read_store over a small file-backed store (offline cache).
# --------------------------------------------------------------------------- #


def _mini_store(root):
    """field3d [2,5,4] chunks [1,2,4]; value = layer*100 + y*10 + x."""
    import json

    objs = {
        f"{BASE}/field3d/.zarray": _zarray((2, 5, 4), (1, 2, 4), "<f4"),
        f"{BASE}/field3d/.zattrs": json.dumps({"_ARRAY_DIMENSIONS": ["layer", "y", "x"]}).encode(),
    }
    for c0 in range(2):
        for c1 in range(3):
            chunk = np.zeros((1, 2, 4), dtype="<f4")
            for b in range(2):
                for c in range(4):
                    y, x = c1 * 2 + b, c
                    if y < 5:
                        chunk[0, b, c] = c0 * 100 + y * 10 + x
            objs[f"{BASE}/field3d/{c0}.{c1}.0"] = _encode_chunk(chunk)
    _populate(root, objs)


def test_read_store_orthogonal_selection(tmp_path):
    _mini_store(tmp_path)
    cache = Cache(root=tmp_path, offline=True, verify=True)
    nds = ZarrReader().read_store(
        cache, BASE, ["field3d"],
        select={"axes": [{"indices": [1]}, {"indices": [1, 4]}, "all"]},
    )
    f = nds.variables["field3d"]
    assert f.dims == ("layer", "y", "x")
    assert f.shape == (1, 2, 4)
    assert f.data.dtype == np.float64
    np.testing.assert_array_equal(
        f.data, np.array([[[110, 111, 112, 113], [140, 141, 142, 143]]], dtype="f8")
    )


def test_read_store_all_and_partial_edge_chunk(tmp_path):
    _mini_store(tmp_path)
    cache = Cache(root=tmp_path, offline=True, verify=True)
    nds = ZarrReader().read_store(cache, BASE, ["field3d"], select=None)
    f = nds.variables["field3d"]
    assert f.shape == (2, 5, 4)  # full array; edge chunk row 4 present, pad row dropped
    # row 4 (the partial edge chunk) decodes to real values, not pad
    np.testing.assert_array_equal(f.data[1, 4], np.array([140, 141, 142, 143], dtype="f8"))


def test_fill_value_zero_is_not_nan(tmp_path):
    """A stored 0.0 is real data; the reader must NOT map fill_value to NaN."""
    import json

    objs = {
        f"{BASE}/z/.zarray": _zarray((4,), (4,), "<f8"),
        f"{BASE}/z/.zattrs": json.dumps({"_ARRAY_DIMENSIONS": ["c"]}).encode(),
        f"{BASE}/z/0": _encode_chunk(np.array([0.0, 1.0, 0.0, 2.0], dtype="<f8")),
    }
    _populate(tmp_path, objs)
    cache = Cache(root=tmp_path, offline=True, verify=True)
    f = ZarrReader().read_store(cache, BASE, ["z"]).variables["z"]
    assert not np.isnan(f.data).any()
    np.testing.assert_array_equal(f.data, [0.0, 1.0, 0.0, 2.0])


def test_absent_chunk_object_fills_with_fill_value(tmp_path):
    """A missing chunk object fills its region with fill_value (0.0 here)."""
    import json

    objs = {
        f"{BASE}/g/.zarray": _zarray((4,), (2,), "<f8"),  # 2 chunks
        f"{BASE}/g/.zattrs": json.dumps({"_ARRAY_DIMENSIONS": ["c"]}).encode(),
        f"{BASE}/g/0": _encode_chunk(np.array([5.0, 6.0], dtype="<f8")),
        # chunk "1" (cells 2,3) intentionally omitted → filled with 0.0
    }
    _populate(tmp_path, objs)
    cache = Cache(root=tmp_path, offline=True, verify=True)
    f = ZarrReader().read_store(cache, BASE, ["g"]).variables["g"]
    np.testing.assert_array_equal(f.data, [5.0, 6.0, 0.0, 0.0])


def test_synthesized_dims_without_zattrs(tmp_path):
    objs = {f"{BASE}/n/.zarray": _zarray((3,), (3,), "<f8"),
            f"{BASE}/n/0": _encode_chunk(np.array([1.0, 2.0, 3.0], dtype="<f8"))}
    _populate(tmp_path, objs)
    cache = Cache(root=tmp_path, offline=True, verify=True)
    f = ZarrReader().read_store(cache, BASE, ["n"]).variables["n"]
    assert f.dims == ("dim_0",)


# --------------------------------------------------------------------------- #
# Laziness — the load-bearing capability, proven with a poison store.
# --------------------------------------------------------------------------- #


def test_laziness_never_touches_unselected_chunks(tmp_path):
    """Non-selected chunks hold undecodable garbage; a lazy read never touches
    them, so the selection decodes cleanly. An over-fetch would blosc-error."""
    import json

    objs = {
        f"{BASE}/sr/.zarray": _zarray((3, 500, 1), (1, 100, 1), "<f4"),
        f"{BASE}/sr/.zattrs": json.dumps(
            {"_ARRAY_DIMENSIONS": ["layer", "source", "receptor"]}).encode(),
    }
    # 3 layers x 5 source-chunks x 1 = 15 chunks. Only layer 0, source-chunks {0,3}
    # are valid; every other chunk is poison (garbage that fails blosc decode).
    want_layers, want_source_chunks = {0}, {0, 3}
    for c0 in range(3):
        for c1 in range(5):
            key = f"{BASE}/sr/{c0}.{c1}.0"
            if c0 in want_layers and c1 in want_source_chunks:
                chunk = np.full((1, 100, 1), float(c0 * 1000 + c1), dtype="<f4")
                objs[key] = _encode_chunk(chunk)
            else:
                objs[key] = b"\x00POISON-not-a-blosc-container\xff"
    _populate(tmp_path, objs)
    cache = Cache(root=tmp_path, offline=True, verify=True)

    # select layer=[0], source=[5, 12, 305, 340] (chunks {0, 3}), receptor=all.
    nds = ZarrReader().read_store(
        cache, BASE, ["sr"],
        select={"axes": [{"indices": [0]},
                         {"indices": [5, 12, 305, 340]},
                         "all"]},
    )
    f = nds.variables["sr"]
    assert f.shape == (1, 4, 1)
    # sources 5,12 -> chunk 0 (value 0); sources 305,340 -> chunk 3 (value 3)
    np.testing.assert_array_equal(f.data.ravel(), [0.0, 0.0, 3.0, 3.0])


def test_over_selection_would_hit_poison(tmp_path):
    """Control: selecting a source in a POISON chunk DOES decode-error — proving
    the poison is genuinely undecodable, so the lazy test above is meaningful."""
    import json

    objs = {
        f"{BASE}/sr/.zarray": _zarray((1, 500, 1), (1, 100, 1), "<f4"),
        f"{BASE}/sr/.zattrs": json.dumps(
            {"_ARRAY_DIMENSIONS": ["layer", "source", "receptor"]}).encode(),
        f"{BASE}/sr/0.0.0": _encode_chunk(np.zeros((1, 100, 1), dtype="<f4")),
    }
    for c1 in range(1, 5):
        objs[f"{BASE}/sr/0.{c1}.0"] = b"\x00POISON\xff"
    _populate(tmp_path, objs)
    cache = Cache(root=tmp_path, offline=True, verify=True)
    with pytest.raises(Exception):
        ZarrReader().read_store(
            cache, BASE, ["sr"],
            select={"axes": [{"indices": [0]}, {"indices": [150]}, "all"]},  # chunk 1 = poison
        )


# --------------------------------------------------------------------------- #
# Registry dispatch + Provider store-backed seam.
# --------------------------------------------------------------------------- #


def test_zarr_registered_active_and_store_backed():
    assert format_registry.status("zarr") == "active"
    reader = format_registry.create("zarr")
    assert getattr(reader, "store_backed", False) is True


def test_provider_routes_store_backed(tmp_path):
    _mini_store(tmp_path)
    cache = Cache(root=tmp_path, offline=True, verify=True)
    loader = DataLoader(
        name="isrm", format="zarr", url=BASE, variables=["field3d"],
        reader_kwargs={"select": {"axes": [{"indices": [1]}, {"indices": [1, 4]}, "all"]}},
    )
    nds = Provider(loader, cache).materialize()
    assert nds.variables["field3d"].shape == (1, 2, 4)


def test_read_native_is_store_backed_error():
    reader = ZarrReader()
    from earthsciio.errors import Unsupported

    with pytest.raises(Unsupported):
        reader.open("/tmp/x")
    with pytest.raises(Unsupported):
        reader.read_native(object(), ["field3d"])


def test_variables_required():
    reader = ZarrReader()
    with pytest.raises(ValueError):
        reader.read_store(object(), BASE, None)


# --------------------------------------------------------------------------- #
# Phase 1: per-call `select` pushdown, supports_selection, array_shape.
# (mirrors julia/test/test_zarr.jl's Phase-1a tests.)
# --------------------------------------------------------------------------- #


class _CountingStore:
    """Wraps a :class:`LocalStore` and records every ``get_blob`` KEY, so a test can
    prove the reader fetched ONLY the objects it needed (each on-demand object fetch
    is exactly one ``get_blob`` on the offline path). Mirrors the Julia
    ``CountingStore``; everything else forwards to the wrapped store."""

    def __init__(self, inner: LocalStore) -> None:
        self.inner = inner
        self.gets: list = []

    def name(self):  # pragma: no cover - trivial delegation
        return self.inner.name()

    def get_blob(self, key):
        self.gets.append(key)
        return self.inner.get_blob(key)

    def exists(self, key):  # pragma: no cover - trivial delegation
        return self.inner.exists(key)

    def get_meta(self, key):
        return self.inner.get_meta(key)

    def put_blob(self, key, staged, ext=""):  # pragma: no cover - not used offline
        return self.inner.put_blob(key, staged, ext)

    def put_meta(self, key, manifest):  # pragma: no cover - not used offline
        return self.inner.put_meta(key, manifest)

    def staging_path(self, ext="part"):  # pragma: no cover - not used offline
        return self.inner.staging_path(ext)

    def lock(self, key):  # pragma: no cover - not used offline
        return self.inner.lock(key)


ZSR = "s3://earthsci-fixtures/sr-mini.zarr"


def _sr_store(root):
    """A VALID `sr` store: shape (3,500,1), chunks (1,100,1). Element at global
    (layer, source, 0) encodes its indices: value = layer*1_000_000 + source (exact
    in float32 for these ranges), so a selection's values are self-checking."""
    import json

    objs = {
        f"{ZSR}/sr/.zarray": _zarray((3, 500, 1), (1, 100, 1), "<f4"),
        f"{ZSR}/sr/.zattrs": json.dumps(
            {"_ARRAY_DIMENSIONS": ["layer", "source", "receptor"]}
        ).encode(),
    }
    for c0 in range(3):
        for c1 in range(5):
            chunk = np.zeros((1, 100, 1), dtype="<f4")
            for j in range(100):
                chunk[0, j, 0] = float(c0 * 1_000_000 + (c1 * 100 + j))
            objs[f"{ZSR}/sr/{c0}.{c1}.0"] = _encode_chunk(chunk)
    _populate(root, objs)


def test_per_call_select_pushes_down_and_fetches_only_needed_chunks(tmp_path):
    _sr_store(tmp_path)
    store = _CountingStore(LocalStore(tmp_path))
    cache = Cache(store, offline=True, verify=False)
    p = Provider(DataLoader(name="isrm", format="zarr", url=ZSR, variables=["sr"]), cache)

    # layer 1, sources {5,12}∈chunk0 and {305,340}∈chunk3, all receptors.
    sel = {"axes": [{"indices": [1]}, {"indices": [5, 12, 305, 340]}, "all"]}
    nds = p.materialize(select=sel)
    f = nds.variables["sr"]
    assert f.dims == ("layer", "source", "receptor")
    assert f.shape == (1, 4, 1)
    np.testing.assert_array_equal(
        f.data.ravel(), [1_000_005, 1_000_012, 1_000_305, 1_000_340]
    )

    # Laziness: fetched ONLY .zarray + .zattrs + chunks (1,0,0) and (1,3,0) — the
    # 13 other chunks (layers 0/2, source-chunks 1/2/4) were never touched.
    expected = {
        cache_key(f"{ZSR}/sr/{k}")
        for k in (".zarray", ".zattrs", "1.0.0", "1.3.0")
    }
    assert set(store.gets) == expected
    assert len(store.gets) == 4


def test_per_call_select_preserves_permuted_order(tmp_path):
    """A NON-CONTIGUOUS PERMUTED index list returns rows in the GIVEN order, not
    sorted — the load-bearing ordering contract for the 3-way conformance case."""
    _sr_store(tmp_path)
    cache = Cache(LocalStore(tmp_path), offline=True, verify=False)
    p = Provider(DataLoader(name="isrm", format="zarr", url=ZSR, variables=["sr"]), cache)
    sel = {"axes": [{"indices": [0]}, {"indices": [340, 5, 305, 12]}, "all"]}
    f = p.materialize(select=sel).variables["sr"]
    # Order preserved exactly (a reader that sorted would give 5,12,305,340).
    np.testing.assert_array_equal(f.data.ravel(), [340, 5, 305, 12])


def test_per_call_select_overrides_baked(tmp_path):
    _sr_store(tmp_path)
    cache = Cache(LocalStore(tmp_path), offline=True, verify=False)
    baked = {"axes": [{"indices": [0]}, {"indices": [7]}, "all"]}
    p = Provider(
        DataLoader(name="isrm", format="zarr", url=ZSR, variables=["sr"],
                   reader_kwargs={"select": baked}),
        cache,
    )
    # No per-call select ⇒ the baked select still applies (regression).
    np.testing.assert_array_equal(p.materialize().variables["sr"].data.ravel(), [7])
    # A per-call select OVERRIDES the baked one for this call only.
    over = {"axes": [{"indices": [2]}, {"indices": [7]}, "all"]}
    np.testing.assert_array_equal(
        p.materialize(select=over).variables["sr"].data.ravel(), [2_000_007]
    )
    # ... and the baked default is untouched afterwards.
    np.testing.assert_array_equal(p.materialize().variables["sr"].data.ravel(), [7])


def test_array_shape_reads_only_zarray(tmp_path):
    _sr_store(tmp_path)
    store = _CountingStore(LocalStore(tmp_path))
    cache = Cache(store, offline=True, verify=False)
    p = Provider(DataLoader(name="isrm", format="zarr", url=ZSR, variables=["sr"]), cache)

    assert p.array_shape("sr") == (3, 500, 1)
    assert store.gets == [cache_key(f"{ZSR}/sr/.zarray")]  # ONLY .zarray, never a chunk


def test_supports_selection_and_array_shape_capability_surface(tmp_path):
    _sr_store(tmp_path)
    cache = Cache(LocalStore(tmp_path), offline=True, verify=False)

    # store-backed zarr provider CAN push down
    pz = Provider(DataLoader(name="isrm", format="zarr", url=ZSR, variables=["sr"]), cache)
    assert supports_selection(ZarrReader()) is True
    assert pz.supports_selection is True

    # whole-file readers cannot; array_shape is None (shape unknown without a read)
    for fmt in ("csv", "ff10", "netcdf"):
        pw = Provider(DataLoader(name="x", format=fmt, url="file:///dev/null"), cache)
        assert pw.supports_selection is False
        assert pw.array_shape("anything") is None
    assert supports_selection(CSVReader()) is False
    assert supports_selection(FF10Reader()) is False


def test_per_call_select_on_non_store_reader_raises(tmp_path):
    cache = Cache(LocalStore(tmp_path), offline=True, verify=False)
    pw = Provider(DataLoader(name="x", format="csv", url="file:///dev/null"), cache)
    # raised before any fetch — the reader can't honour a projection pushdown
    with pytest.raises(ValueError):
        pw.materialize(select={"axes": ["all"]})
    with pytest.raises(ValueError):
        pw.refresh(_dt.datetime(2020, 1, 1), select={"axes": ["all"]})
