"""The streaming ``zarr`` v3 WRITER and its round-trip with the reader.

Writes a small sharded Zarr v3 store with :class:`ZarrWriter` (native zarr-python
sharding + Blosc), then reads it back through the store-backed
:class:`ZarrReader` (over an offline content-addressed cache seeded from the
written store), asserting array + coordinate + attr agreement within tolerance
(RFC §16.6 — decoded agreement, never byte identity). Also covers the durable
``write_flush`` barrier and the output manifest (``earthsciio/output-manifest/v1``).
"""

from __future__ import annotations

import json
import pathlib

import numpy as np
import pytest

pytest.importorskip("zarr")

from earthsciio import Cache
from earthsciio.backends.local import LocalStore
from earthsciio.backends.zarr import ZarrReader
from earthsciio.backends.zarr_write import (
    BLOSC_CHECKPOINT,
    BLOSC_DIAGNOSTIC,
    ZSTD_WASM,
    OUTPUT_MANIFEST_SCHEMA,
    OutputSchema,
    OutputVar,
    ZarrWriter,
)
from earthsciio.cachekey import sha256_bytes
from earthsciio.manifest import Manifest

BASE = "s3://earthsci-fixtures/roundtrip.zarr"


def _seed_cache_from_store(store_dir: pathlib.Path, root: pathlib.Path) -> None:
    """Load every object of the written store into an offline cache keyed by
    ``sha256(BASE/relpath)`` — the exact shape the reader fetches (mirrors the
    corpus)."""
    from earthsciio import cache_key

    store = LocalStore(root)
    for p in store_dir.rglob("*"):
        if not p.is_file():
            continue
        rel = str(p.relative_to(store_dir))
        url = f"{BASE}/{rel}"
        data = p.read_bytes()
        staged = store.staging_path()
        staged.write_bytes(data)
        store.put_blob(cache_key(url), staged, "")
        store.put_meta(
            cache_key(url),
            Manifest(
                url=url,
                sha256_content=sha256_bytes(data),
                bytes=len(data),
                fetched_at="2026-01-01T00:00:00Z",
            ),
        )


def _schema(profile="diagnostic"):
    return OutputSchema(
        dims=[("time", 0), ("y", 3), ("x", 4)],
        time_dim="time",
        vars=[
            (
                "temp",
                OutputVar(
                    ["time", "y", "x"],
                    "float64",
                    {"units": "K", "standard_name": "air_temperature"},
                ),
            )
        ],
        chunk_shape={"time": 2, "y": 3, "x": 4},
        shard_shape={"time": 4, "y": 3, "x": 4},
        coords=[
            ("y", (np.array([0.0, 10.0, 20.0]), {"standard_name": "projection_y_coordinate", "axis": "Y", "units": "m"})),
            ("x", (np.array([0.0, 1.0, 2.0, 3.0]), {"standard_name": "projection_x_coordinate", "axis": "X", "units": "m"})),
            ("time", (np.array([]), {"units": "seconds since 1970-01-01", "standard_name": "time"})),
        ],
        profile=profile,
        attrs={"title": "roundtrip"},
    )


def _write(store_dir, schema, n=5, flush_at=None):
    w = ZarrWriter()
    h = w.write_open(str(store_dir), schema)
    recs = []
    for ti in range(n):
        slab = np.array(
            [[ti * 100 + y * 10 + x for x in range(4)] for y in range(3)], dtype="f8"
        )
        recs.append(slab)
        w.write_record(h, float(ti * 3600), {"temp": slab})
        if flush_at is not None and ti == flush_at:
            w.write_flush(h)
    manifest = w.write_close(h)
    return np.stack(recs, axis=0), manifest


# --------------------------------------------------------------------------- #
# Round-trip: write with the writer, read back with the reader.
# --------------------------------------------------------------------------- #


def test_roundtrip_arrays_coords_attrs(tmp_path):
    store_dir = tmp_path / "out.zarr"
    expected, _ = _write(store_dir, _schema(), n=5)
    _seed_cache_from_store(store_dir, tmp_path / "cache")

    cache = Cache(root=tmp_path / "cache", offline=True, verify=True)
    nds = ZarrReader().read_store(cache, BASE, ["temp", "time", "y", "x"])

    temp = nds.variables["temp"]
    assert temp.dims == ("time", "y", "x")
    assert temp.shape == (5, 3, 4)
    assert temp.data.dtype == np.float64
    np.testing.assert_allclose(temp.data, expected, rtol=1e-6, atol=0)

    np.testing.assert_allclose(
        nds.variables["time"].data, np.arange(5) * 3600.0, rtol=1e-6
    )
    np.testing.assert_allclose(nds.variables["y"].data, [0.0, 10.0, 20.0])
    np.testing.assert_allclose(nds.variables["x"].data, [0.0, 1.0, 2.0, 3.0])


def test_roundtrip_attrs_and_dimension_names(tmp_path):
    import zarr

    from earthsciio.backends.zarr import _make_cache_store

    store_dir = tmp_path / "out.zarr"
    _write(store_dir, _schema(), n=3)
    _seed_cache_from_store(store_dir, tmp_path / "cache")
    cache = Cache(root=tmp_path / "cache", offline=True, verify=True)

    a = zarr.open_array(store=_make_cache_store(cache, BASE), path="temp", mode="r")
    assert a.metadata.zarr_format == 3
    assert list(a.metadata.dimension_names) == ["time", "y", "x"]
    assert dict(a.attrs) == {"units": "K", "standard_name": "air_temperature"}
    # sharded v3: the top codec is the sharding codec with an inner chunk shape.
    zj = json.loads(
        (store_dir / "temp" / "zarr.json").read_text()
    )
    assert zj["codecs"][0]["name"] == "sharding_indexed"
    assert zj["chunk_grid"]["configuration"]["chunk_shape"] == [4, 3, 4]  # shard
    assert zj["codecs"][0]["configuration"]["chunk_shape"] == [2, 3, 4]  # inner


def test_roundtrip_selection(tmp_path):
    store_dir = tmp_path / "out.zarr"
    expected, _ = _write(store_dir, _schema(), n=5)
    _seed_cache_from_store(store_dir, tmp_path / "cache")
    cache = Cache(root=tmp_path / "cache", offline=True, verify=True)

    # permuted, non-contiguous time selection — order preserved
    nds = ZarrReader().read_store(
        cache, BASE, ["temp"], select={"axes": [{"indices": [4, 1]}, "all", "all"]}
    )
    got = nds.variables["temp"]
    assert got.shape == (2, 3, 4)
    np.testing.assert_allclose(got.data, expected[[4, 1]], rtol=1e-6)


# --------------------------------------------------------------------------- #
# Durable barrier + output manifest.
# --------------------------------------------------------------------------- #


def test_write_flush_is_a_durable_barrier(tmp_path):
    store_dir = tmp_path / "out.zarr"
    # flush after record 2 (mid-shard, shard_time=4): 3 records become durable
    # before the run continues; total 5 records over 2 shards.
    expected, manifest = _write(store_dir, _schema(), n=5, flush_at=2)
    assert manifest["total_records"] == 5
    assert len(manifest["time_shards"]) == 2
    assert manifest["time_shards"][0]["n_records"] == 3  # the flushed partial shard
    assert manifest["last_t"] == 14400.0

    # the store on disk is fully readable after the run
    _seed_cache_from_store(store_dir, tmp_path / "cache")
    cache = Cache(root=tmp_path / "cache", offline=True, verify=True)
    nds = ZarrReader().read_store(cache, BASE, ["temp"])
    np.testing.assert_allclose(nds.variables["temp"].data, expected, rtol=1e-6)


def test_output_manifest_shape(tmp_path):
    store_dir = tmp_path / "out.zarr"
    _, manifest = _write(store_dir, _schema(), n=5)
    # written into the store as a sibling object
    on_disk = json.loads((store_dir / "output_manifest.json").read_text())
    assert on_disk == manifest
    assert manifest["schema"] == OUTPUT_MANIFEST_SCHEMA
    assert manifest["format"] == "zarr"
    assert manifest["zarr_format"] == 3
    assert manifest["profile"] == "diagnostic"
    assert manifest["codec"] == {
        "id": "blosc",
        "cname": "zstd",
        "clevel": 5,
        "shuffle": "shuffle",
    }
    assert manifest["time_dim"] == "time"
    assert manifest["vars"] == [
        {"name": "temp", "dims": ["time", "y", "x"], "dtype": "float64"}
    ]
    assert manifest["chunk_shape"] == {"time": 2, "y": 3, "x": 4}
    assert manifest["shard_shape"] == {"time": 4, "y": 3, "x": 4}


def test_checkpoint_profile_uses_lossless_level(tmp_path):
    store_dir = tmp_path / "ckpt.zarr"
    _, manifest = _write(store_dir, _schema(profile="checkpoint"), n=2)
    assert manifest["profile"] == "checkpoint"
    assert manifest["codec"]["clevel"] == BLOSC_CHECKPOINT.clevel == 7
    assert BLOSC_DIAGNOSTIC.clevel == 5


# --------------------------------------------------------------------------- #
# The `wasm` profile — plain Zarr v3 zstd inner codec, NO Blosc.
#
# Why it exists: a WebAssembly/browser Zarr reader cannot decode the Blosc
# container (the `zarrs` crate's blosc support comes from `blosc-src`, whose
# vendored C sources don't build for wasm32-unknown-unknown), while the standard
# v3 `zstd` codec is pure Rust there. Sharding/crc32c are unchanged — only the
# inner compressor differs — so these tests pin BOTH the emitted codec chain and
# a full value round-trip.
# --------------------------------------------------------------------------- #


def test_wasm_profile_inner_codec_is_plain_zstd_without_blosc(tmp_path):
    store_dir = tmp_path / "wasm.zarr"
    _write(store_dir, _schema(profile="wasm"), n=5)

    for array in ("temp", "time", "y", "x"):
        meta = json.loads((store_dir / array / "zarr.json").read_text())
        # sharding is unchanged: it stays the OUTER codec
        assert [c["name"] for c in meta["codecs"]] == ["sharding_indexed"]
        scfg = meta["codecs"][0]["configuration"]
        inner = scfg["codecs"]
        # inner pipeline is exactly bytes(little-endian) + zstd — no blosc, and
        # no standalone shuffle filter
        assert [c["name"] for c in inner] == ["bytes", "zstd"], array
        assert inner[0]["configuration"]["endian"] == "little"
        assert inner[1]["configuration"] == {"level": 5, "checksum": False}
        assert not any(c["name"] == "blosc" for c in inner), array
        # fill_value stays 0.0 (never NaN)
        assert meta["fill_value"] == 0.0
        # the shard index pipeline is untouched
        assert [c["name"] for c in scfg["index_codecs"]] == ["bytes", "crc32c"]


def test_wasm_profile_roundtrip_arrays_coords_attrs(tmp_path):
    store_dir = tmp_path / "wasm.zarr"
    expected, _ = _write(store_dir, _schema(profile="wasm"), n=5)
    _seed_cache_from_store(store_dir, tmp_path / "cache")

    cache = Cache(root=tmp_path / "cache", offline=True, verify=True)
    nds = ZarrReader().read_store(cache, BASE, ["temp", "time", "y", "x"])

    temp = nds.variables["temp"]
    assert temp.dims == ("time", "y", "x")
    assert temp.shape == (5, 3, 4)
    assert temp.data.dtype == np.float64
    # zstd is lossless, but assert on tolerance per the RFC §16.6 policy
    np.testing.assert_allclose(temp.data, expected, rtol=1e-6, atol=1e-9)

    np.testing.assert_allclose(
        nds.variables["time"].data, np.arange(5) * 3600.0, rtol=1e-6, atol=1e-9
    )
    np.testing.assert_allclose(nds.variables["y"].data, [0.0, 10.0, 20.0])
    np.testing.assert_allclose(nds.variables["x"].data, [0.0, 1.0, 2.0, 3.0])


def test_wasm_profile_preserves_dimension_names_and_cf_attrs(tmp_path):
    store_dir = tmp_path / "wasm.zarr"
    _write(store_dir, _schema(profile="wasm"), n=5)

    meta = json.loads((store_dir / "temp" / "zarr.json").read_text())
    assert meta["dimension_names"] == ["time", "y", "x"]
    assert meta["data_type"] == "float64"
    assert meta["attributes"]["units"] == "K"
    assert meta["attributes"]["standard_name"] == "air_temperature"
    ymeta = json.loads((store_dir / "y" / "zarr.json").read_text())
    assert ymeta["attributes"]["axis"] == "Y"
    assert ymeta["attributes"]["units"] == "m"
    group = json.loads((store_dir / "zarr.json").read_text())
    assert group["attributes"]["title"] == "roundtrip"


def test_wasm_profile_manifest_records_the_zstd_codec(tmp_path):
    store_dir = tmp_path / "wasm.zarr"
    _, manifest = _write(store_dir, _schema(profile="wasm"), n=5)
    assert manifest["profile"] == "wasm"
    assert manifest["codec"] == {"id": "zstd", "level": 5, "checksum": False}
    assert manifest["codec"] == {
        "id": "zstd",
        "level": ZSTD_WASM.level,
        "checksum": ZSTD_WASM.checksum,
    }
    # the wasm profile changes ONLY the codec — the rest of the manifest is the
    # same shape the Blosc profiles produce
    assert manifest["zarr_format"] == 3
    assert manifest["chunk_shape"] == {"time": 2, "y": 3, "x": 4}
    assert manifest["shard_shape"] == {"time": 4, "y": 3, "x": 4}


def test_wasm_profile_values_match_the_blosc_profile_exactly(tmp_path):
    """Both profiles are lossless, so the DECODED arrays must be identical —
    the codec swap must not perturb a single value."""
    diag_dir = tmp_path / "diag.zarr"
    wasm_dir = tmp_path / "wasm.zarr"
    exp_diag, _ = _write(diag_dir, _schema(profile="diagnostic"), n=5)
    exp_wasm, _ = _write(wasm_dir, _schema(profile="wasm"), n=5)
    np.testing.assert_array_equal(exp_diag, exp_wasm)

    import zarr

    a = zarr.open_group(str(diag_dir), mode="r")["temp"][...]
    b = zarr.open_group(str(wasm_dir), mode="r")["temp"][...]
    np.testing.assert_allclose(a, b, rtol=1e-6, atol=1e-9)
    np.testing.assert_array_equal(a, b)  # lossless: bit-exact in practice


def test_unknown_profile_rejected(tmp_path):
    with pytest.raises(ValueError, match="unknown codec profile"):
        ZarrWriter().write_open(
            str(tmp_path / "bad.zarr"), _schema(profile="not-a-profile")
        )


# --------------------------------------------------------------------------- #
# Schema validation guards.
# --------------------------------------------------------------------------- #


def test_shard_must_be_multiple_of_chunk(tmp_path):
    schema = _schema()
    schema.shard_shape["time"] = 3  # 3 % 2 != 0
    with pytest.raises(ValueError):
        ZarrWriter().write_open(str(tmp_path / "bad.zarr"), schema)


def test_var_must_include_time_dim(tmp_path):
    schema = OutputSchema(
        dims=[("time", 0), ("y", 3)],
        time_dim="time",
        vars=[("static", OutputVar(["y"], "float64"))],  # no time dim
        chunk_shape={"time": 2, "y": 3},
        shard_shape={"time": 2, "y": 3},
    )
    with pytest.raises(ValueError):
        ZarrWriter().write_open(str(tmp_path / "bad.zarr"), schema)


def test_record_slab_shape_checked(tmp_path):
    schema = _schema()
    w = ZarrWriter()
    h = w.write_open(str(tmp_path / "out.zarr"), schema)
    with pytest.raises(ValueError):
        w.write_record(h, 0.0, {"temp": np.zeros((2, 2))})  # wrong (y,x)
