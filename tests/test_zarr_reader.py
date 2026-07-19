"""The active, store-backed ``zarr`` reader (Zarr v2 chunked arrays).

Covers the chunk math, orthogonal selection, the partial edge chunk, the
fill-for-absent-chunk rule, the ``fill_value != NaN`` deviation, and — the
load-bearing capability — **laziness**: a runtime index list on a dimension
fetches ONLY the intersecting chunk objects, never the whole array. Laziness is
proven with a "poison" store: chunks that must NOT be read hold undecodable
garbage, so any over-fetch decode-errors instead of silently succeeding.
"""

from __future__ import annotations

import numpy as np
import pytest

numcodecs = pytest.importorskip("numcodecs")

from earthsciio import Cache, DataLoader, Provider, cache_key, format_registry
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
