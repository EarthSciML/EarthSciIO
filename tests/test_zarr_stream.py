"""The streaming writer contract (``spec/writer.md``, RFC U12) in Python.

Unit-level coverage of the §10 conformance cases — region tiling, misaligned
regions, commit gap / overlap, indexed records (reverse order, the rewrite
rule), resume (partial shard, truncation), dtypes, statics and blobs, the v1
manifest — plus the binding-level guarantees: a contributor never writes a
``zarr.json`` or the manifest, whole shards are encoded without reading the
store, the commit-token wire form and the schema fingerprint.
"""

from __future__ import annotations

import hashlib
import json

import numpy as np
import pytest

pytest.importorskip("zarr")

import zarr  # noqa: E402

from earthsciio.backends.zarr_stream import (  # noqa: E402
    COMMIT_TOKEN_SCHEMA,
    OUTPUT_MANIFEST_V1,
    OUTPUT_MANIFEST_V2,
    CommitToken,
    IndexedTime,
    OutputSchema,
    OutputVar,
    ZarrWriter,
    read_blob,
    read_output_manifest,
    read_record,
    read_static,
    schema_fingerprint,
)
from earthsciio.errors import CommitError, EarthSciIOError, RegionNotShardAligned  # noqa: E402

DT = 3600.0
NLAT, NLON = 6, 5
P = [3, 5, 11, 17, 23]
SEEDS = {"temperature": 100, "pressure": 200, "global_mean": 300,
         "dJ_dfield": 500, "dJ_dlon": 600, "step_count": 700}


def schema(*, time_mode="append", statics=(), vars_=None, profile="diagnostic"):
    if vars_ is None:
        vars_ = [
            ("temperature", OutputVar(["time", "lat", "lon"], "float64", {"units": "K"})),
            ("pressure", OutputVar(["time", "lat", "lon"], "float32")),
            ("global_mean", OutputVar(["time"], "float64")),
        ]
    return OutputSchema(
        dims=[("time", 0), ("lat", NLAT), ("lon", NLON)],
        time_dim="time",
        vars=list(vars_),
        chunk_shape={"time": 1, "lat": 1, "lon": 2},
        shard_shape={"time": 2, "lat": 2, "lon": 4},
        coords=[
            ("lat", (np.arange(NLAT) * 10.0, {"units": "degrees_north"})),
            ("lon", (np.arange(NLON) * 1.0, {"units": "degrees_east"})),
            ("time", (np.array([]), {"units": "seconds since 2020-01-01"})),
        ],
        profile=profile,
        attrs={"title": "stream"},
        statics=list(statics),
        time_mode=time_mode,
    )


STATICS = [
    ("dJ_dfield", OutputVar(["lat", "lon"], "float64")),
    ("dJ_dlon", OutputVar(["lon"], "float64")),
    ("step_count", OutputVar(["lat", "lon"], "int64")),
]


def gen(seed, r, dtype, ranges):
    """The §10 generator over global index ranges."""
    shape = tuple(len(rg) for rg in ranges)
    base = np.full(shape, seed + 13 * r, dtype=np.int64)
    for j, rg in enumerate(ranges):
        ax = (np.asarray(rg, dtype=np.int64) + 1) * P[j]
        base = base + ax.reshape([-1 if k == j else 1 for k in range(len(shape))])
    dtype = np.dtype(dtype)
    if dtype == np.uint8:
        return (base % 251).astype(np.uint8)
    if dtype.kind == "i":
        return base.astype(dtype)
    return (base + 0.25).astype(dtype)


def arrays(sch, r, region=None, contributor=False, which=None):
    region = region or {}
    dimlen = sch.dim_lengths
    out = {}
    for nm, ov in which if which is not None else sch.vars:
        sdims = [d for d in ov.dims if d != sch.time_dim]
        part = any(d in region for d in sdims)
        if contributor and not part:
            continue
        ranges = [range(*region[d]) if d in region else range(dimlen[d]) for d in sdims]
        out[nm] = gen(SEEDS.get(nm, 1), r, ov.dtype, ranges)
    return out


def read_all(path, sch):
    """Every committed record of every var, read through the read-back API."""
    m = read_output_manifest(str(path))
    from earthsciio.backends.zarr_stream import committed_records

    idx = committed_records(m)
    return {nm: np.stack([read_record(str(path), nm, i) for i in idx]) for nm, _ in sch.vars}


QUAD = [
    ("owner", dict(n_writers=4), {"lat": (0, 4), "lon": (0, 4)}),
    ("contributor", dict(id=1), {"lat": (0, 4), "lon": (4, 5)}),
    ("contributor", dict(id=2), {"lat": (4, 6), "lon": (0, 4)}),
    ("contributor", dict(id=3), {"lat": (4, 6), "lon": (4, 5)}),
]


def open_quad(base, sch):
    w = ZarrWriter()
    hs = []
    for role, kw, region in QUAD:
        hs.append(w.write_open(base, sch, role=role, region=region, **kw))
    return w, hs


def write_single(base, sch, n, flush_at=()):
    w = ZarrWriter()
    h = w.write_open(base, sch)
    for r in range(n):
        w.write_record(h, DT * r, arrays(sch, r))
        if r in flush_at:
            w.write_flush(h)
    return w.write_close(h)


# --------------------------------------------------------------------------- #
# Regions and the commit protocol.
# --------------------------------------------------------------------------- #


def test_region_tiling_equals_single_writer_bitwise(tmp_path):
    sch = schema()
    write_single(str(tmp_path / "single"), sch, 5, flush_at=(2,))

    base = str(tmp_path / "quad")
    w, hs = open_quad(base, sch)
    for r in range(3):
        for h, (role, _, region) in zip(hs, QUAD):
            assert w.write_record(h, DT * r, arrays(sch, r, region, role == "contributor")) == r
    toks = [w.write_flush(h) for h in hs]
    w.write_commit(hs[0], toks)
    for r in range(3, 5):
        for h, (role, _, region) in zip(hs, QUAD):
            w.write_record(h, DT * r, arrays(sch, r, region, role == "contributor"))
    toks = [w.write_flush(h) for h in hs[1:]]
    for h in hs[1:]:
        assert w.write_close(h) is None
    m = w.write_close(hs[0], toks)
    assert m["schema"] == OUTPUT_MANIFEST_V2
    assert m["commit_seq"] == 2
    assert m["records"]["committed"] == [[0, 5]]
    assert m["writers"] == [{"id": i, "region": {d: list(rg) for d, rg in reg.items()}}
                            for i, (_, _, reg) in enumerate(QUAD)]

    a = zarr.open_group(str(tmp_path / "single"), mode="r")
    b = zarr.open_group(base, mode="r")
    for nm in ("temperature", "pressure", "global_mean", "time", "lat", "lon"):
        x, y = a[nm][...], b[nm][...]
        assert x.dtype == y.dtype and x.shape == y.shape
        assert x.tobytes() == y.tobytes(), nm
    got = read_all(base, sch)
    for nm, ov in sch.vars:
        sdims = [d for d in ov.dims if d != "time"]
        want = np.stack([gen(SEEDS[nm], r, ov.dtype, [range(sch.dim_lengths[d]) for d in sdims])
                         for r in range(5)])
        assert got[nm].tobytes() == want.tobytes()


def test_misaligned_region_raises_before_any_io(tmp_path):
    sch = schema()
    w = ZarrWriter()
    bad = tmp_path / "never"
    with pytest.raises(RegionNotShardAligned):
        w.write_open(str(bad), sch, role="owner", n_writers=2, region={"lat": (0, 3)})
    assert not bad.exists()
    base = str(tmp_path / "s")
    w.write_open(base, sch, role="owner", n_writers=2, region={"lat": (0, 2)})
    with pytest.raises(RegionNotShardAligned):
        w.write_open(base, sch, role="contributor", id=1, region={"lat": (1, 6)})
    with pytest.raises(RegionNotShardAligned):
        w.write_open(base, sch, role="contributor", id=1, region={"lon": (2, 5)})
    # stop == len is aligned even off a shard boundary
    w.write_open(base, sch, role="contributor", id=1, region={"lat": (2, 6), "lon": (4, 5)})


@pytest.mark.parametrize("r1,what", [((4, 6), "gap"), ((0, 6), "overlap")])
def test_gap_and_overlap_are_commit_errors(tmp_path, r1, what):
    sch = schema()
    base = str(tmp_path / "s")
    w = ZarrWriter()
    h0 = w.write_open(base, sch, role="owner", n_writers=2, region={"lat": (0, 2)})
    h1 = w.write_open(base, sch, role="contributor", id=1, region={"lat": (2, 6)})
    w.write_record(h0, 0.0, arrays(sch, 0, {"lat": (0, 2)}))
    w.write_record(h1, 0.0, arrays(sch, 0, {"lat": (2, 6)}, True))
    w.write_commit(h0, [w.write_flush(h0), w.write_flush(h1)])
    before = (tmp_path / "s" / "output_manifest.json").read_bytes()
    zj = (tmp_path / "s" / "temperature" / "zarr.json").read_bytes()

    h9 = w.write_open(base, sch, role="contributor", id=1, region={"lat": r1})
    w.write_record(h0, DT, arrays(sch, 1, {"lat": (0, 2)}))
    w.write_record(h9, DT, arrays(sch, 1, {"lat": r1}, True))
    with pytest.raises(CommitError, match=what):
        w.write_commit(h0, [w.write_flush(h0), w.write_flush(h9)])
    assert (tmp_path / "s" / "output_manifest.json").read_bytes() == before
    assert (tmp_path / "s" / "temperature" / "zarr.json").read_bytes() == zj
    got = read_all(base, sch)
    assert got["temperature"].shape == (1, NLAT, NLON)
    assert got["temperature"][0].tobytes() == gen(100, 0, "float64",
                                                  [range(NLAT), range(NLON)]).tobytes()


def test_commit_checks_writers_fingerprint_and_records(tmp_path):
    sch = schema()
    base = str(tmp_path / "s")
    w = ZarrWriter()
    h0 = w.write_open(base, sch, role="owner", n_writers=2, region={"lat": (0, 2)})
    h1 = w.write_open(base, sch, role="contributor", id=1, region={"lat": (2, 6)})
    w.write_record(h0, 0.0, arrays(sch, 0, {"lat": (0, 2)}))
    w.write_record(h1, 0.0, arrays(sch, 0, {"lat": (2, 6)}, True))
    t0, t1 = w.write_flush(h0), w.write_flush(h1)
    with pytest.raises(CommitError, match="missing"):
        w.write_commit(h0, [t0])
    with pytest.raises(CommitError, match="duplicate"):
        w.write_commit(h0, [t0, t0])
    bad = CommitToken.from_json(t1.to_json())
    bad.fingerprint = "sha256:00"
    with pytest.raises(CommitError, match="fingerprint"):
        w.write_commit(h0, [t0, bad])
    bad = CommitToken.from_json(t1.to_json())
    bad.records = [(0, 1.0)]
    with pytest.raises(CommitError, match="records"):
        w.write_commit(h0, [t0, bad])
    m = w.write_commit(h0, [t0, t1])
    assert m["commit_seq"] == 1 and m["n_records"] == 1
    # an owner's commit always commits, even with empty tokens
    m = w.write_commit(h0, [w.write_flush(h0), w.write_flush(h1)])
    assert m["commit_seq"] == 2 and m["n_records"] == 1


def test_contributor_supplying_replicated_var_errors(tmp_path):
    sch = schema()
    base = str(tmp_path / "s")
    w = ZarrWriter()
    w.write_open(base, sch, role="owner", n_writers=2, region={"lat": (0, 2)})
    h1 = w.write_open(base, sch, role="contributor", id=1, region={"lat": (2, 6)})
    a = arrays(sch, 0, {"lat": (2, 6)})  # includes global_mean (replicated)
    with pytest.raises(EarthSciIOError, match="replicated"):
        w.write_record(h1, 0.0, a)
    with pytest.raises(ValueError, match="region"):
        w.write_record(h1, 0.0, arrays(sch, 0, {"lat": (2, 6)}, True), region={"lat": (0, 2)})
    assert w.write_record(h1, 0.0, arrays(sch, 0, {"lat": (2, 6)}, True),
                          region={"lat": (2, 6)}) == 0


def test_join_requires_v2_manifest_and_matching_schema(tmp_path):
    sch = schema()
    w = ZarrWriter()
    v1 = str(tmp_path / "v1")
    write_single(v1, sch, 1)
    with pytest.raises(EarthSciIOError, match="v2"):
        w.write_open(v1, sch, role="contributor", id=1, region={"lat": (2, 6)})
    with pytest.raises(EarthSciIOError, match="output_manifest"):
        w.write_open(str(tmp_path / "none"), sch, role="contributor", id=1)
    base = str(tmp_path / "s")
    w.write_open(base, sch, role="owner", n_writers=2, region={"lat": (0, 2)})
    other = schema(profile="checkpoint")
    with pytest.raises(EarthSciIOError, match="fingerprint"):
        w.write_open(base, other, role="contributor", id=1, region={"lat": (2, 6)})


# --------------------------------------------------------------------------- #
# Ownership: contributors never write shared objects; whole shards, no reads.
# --------------------------------------------------------------------------- #


@pytest.fixture
def store_log(monkeypatch):
    """Record every GET / SET / DELETE key issued through zarr's LocalStore."""
    from zarr.storage import LocalStore

    log = {"get": [], "set": [], "delete": []}
    orig_get, orig_set, orig_del = LocalStore.get, LocalStore.set, LocalStore.delete
    orig_partial = LocalStore.get_partial_values

    async def get(self, key, *a, **k):
        log["get"].append(key)
        return await orig_get(self, key, *a, **k)

    async def get_partial_values(self, prototype, key_ranges):
        key_ranges = list(key_ranges)
        log["get"].extend(k for k, _ in key_ranges)
        return await orig_partial(self, prototype, key_ranges)

    async def set_(self, key, value):
        log["set"].append(key)
        return await orig_set(self, key, value)

    async def delete(self, key):
        log["delete"].append(key)
        return await orig_del(self, key)

    monkeypatch.setattr(LocalStore, "get", get)
    monkeypatch.setattr(LocalStore, "get_partial_values", get_partial_values)
    monkeypatch.setattr(LocalStore, "set", set_)
    monkeypatch.setattr(LocalStore, "delete", delete)

    def clear():
        for k in ("get", "set", "delete"):
            log[k].clear()

    log["clear"] = clear
    return log


def test_contributor_never_writes_zarr_json_or_manifest(tmp_path, store_log):
    sch = schema(statics=STATICS)
    base = str(tmp_path / "s")
    w = ZarrWriter()
    r0 = {"lat": (0, 4)}
    h0 = w.write_open(base, sch, role="owner", n_writers=2, region=r0)
    for r in range(3):
        w.write_record(h0, DT * r, arrays(sch, r, r0))
    w.write_static(h0, "dJ_dfield", arrays(sch, 0, r0, which=STATICS[:1])["dJ_dfield"])
    store_log["clear"]()
    reg = {"lat": (4, 6)}
    h1 = w.write_open(base, sch, role="contributor", id=1, region=reg)
    for r in range(3):
        w.write_record(h1, DT * r, arrays(sch, r, reg, True))
    w.write_static(h1, "dJ_dfield", arrays(sch, 0, reg, which=STATICS[:1])["dJ_dfield"])
    w.write_blob(h1, ("record", 1), "state", b"\x01\x02")
    tok = w.write_flush(h1)
    assert w.write_close(h1) is None
    written = store_log["set"] + store_log["delete"]
    assert written, "the contributor wrote nothing"
    assert not [k for k in written if k.endswith("zarr.json") or k == "output_manifest.json"]
    # it wrote exactly the objects its token names, all in its own region
    assert sorted(set(written)) == tok.objects
    for k in tok.objects:
        if k.startswith("blobs/"):
            assert k == "blobs/record/1/1/state"
        else:
            name, _, *coords = k.split("/")
            assert name in ("temperature", "pressure", "dJ_dfield")
            lat = coords[1] if name != "dJ_dfield" else coords[0]
            assert lat == "2"  # lat [4, 6) is lat shard 2
    # fresh whole shards are encoded without reading any shard object
    assert not [k for k in store_log["get"] if "/c/" in k]
    w.write_close(h0, [tok])
    assert read_output_manifest(base)["n_records"] == 3


def test_whole_shard_writes_never_read_the_store(tmp_path, store_log):
    """The RFC assumption: zarr-python encodes a fully covered shard without
    reading it back — including edge shards (lon [4, 5) of a 4-long shard),
    via the rounded-up in-memory shape."""
    sch = schema()
    base = str(tmp_path / "s")
    w = ZarrWriter()
    h = w.write_open(base, sch, role="owner", n_writers=1)
    store_log["clear"]()
    for r in range(5):
        w.write_record(h, DT * r, arrays(sch, r))
        if r == 2:  # partial time shard padded, then rewritten whole
            w.write_commit(h, [w.write_flush(h)])
    w.write_close(h)
    assert store_log["set"]
    assert not [k for k in store_log["get"] if "/c/" in k], store_log["get"]


def test_owner_writes_shapes_only_at_commit(tmp_path):
    sch = schema()
    base = tmp_path / "s"
    w = ZarrWriter()
    h = w.write_open(str(base), sch, role="owner", n_writers=1)
    for r in range(3):
        w.write_record(h, DT * r, arrays(sch, r))
    w.write_flush(h)  # owner flush: data PUT, nothing committed
    assert json.loads((base / "temperature" / "zarr.json").read_text())["shape"][0] == 0
    assert read_output_manifest(str(base))["n_records"] == 0
    with pytest.raises(EarthSciIOError, match="not committed"):
        read_record(str(base), "temperature", 0)


# --------------------------------------------------------------------------- #
# Indexed time mode.
# --------------------------------------------------------------------------- #


def test_indexed_reverse_equals_forward_append(tmp_path):
    n = 5
    times = [DT * i for i in range(n)]
    fwd = str(tmp_path / "fwd")
    write_single(fwd, schema(), n)
    sch = schema(time_mode=IndexedTime(times))
    assert sch.dim_lengths["time"] == n
    base = str(tmp_path / "rev")
    w = ZarrWriter()
    h = w.write_open(base, sch)
    for i in reversed(range(n)):
        w.write_record_at(h, i, times[i], arrays(sch, i))
    m = w.write_close(h)
    assert m["schema"] == OUTPUT_MANIFEST_V2 and m["commit_seq"] == 1
    assert m["time_mode"] == {"kind": "indexed", "n_records": n}
    assert [e["generation"] for e in m["records"]["entries"]] == [1] * n
    assert [s["n"] for s in m["time_shards"]] == [2, 2, 1]
    a, b = zarr.open_group(fwd, mode="r"), zarr.open_group(base, mode="r")
    for nm in ("temperature", "pressure", "global_mean", "time"):
        assert a[nm][...].tobytes() == b[nm][...].tobytes(), nm
    with pytest.raises(EarthSciIOError):
        w.write_record(w.write_resume(base), 0.0, arrays(sch, 0))


def test_indexed_rewrite_needs_invalidate(tmp_path):
    sch = schema(time_mode=IndexedTime([0.0, DT]))
    base = str(tmp_path / "s")
    w = ZarrWriter()
    h = w.write_open(base, sch)
    w.write_record_at(h, 0, 0.0, arrays(sch, 0))
    w.write_record_at(h, 1, DT, arrays(sch, 1))
    w.write_flush(h)
    with pytest.raises(EarthSciIOError, match="invalidate"):
        w.write_record_at(h, 0, 0.0, arrays(sch, 7))
    with pytest.raises(ValueError, match="times"):
        w.write_record_at(h, 0, 1.0, arrays(sch, 7))
    w.write_invalidate(h, [0])
    assert read_output_manifest(base)["commit_seq"] == 2
    with pytest.raises(EarthSciIOError):
        read_record(base, "temperature", 0)
    w.write_record_at(h, 0, 0.0, arrays(sch, 7))
    w.write_flush(h)
    m = w.write_close(h)
    assert m["commit_seq"] == 3
    assert [e["generation"] for e in m["records"]["entries"]] == [2, 1]
    want7 = gen(100, 7, "float64", [range(NLAT), range(NLON)])
    want1 = gen(100, 1, "float64", [range(NLAT), range(NLON)])
    assert read_record(base, "temperature", 0).tobytes() == want7.tobytes()
    assert read_record(base, "temperature", 1).tobytes() == want1.tobytes()


def test_contributor_rewrite_rereads_manifest(tmp_path):
    sch = schema(time_mode=IndexedTime([0.0, DT]))
    base = str(tmp_path / "s")
    w = ZarrWriter()
    r0, r1 = {"lat": (0, 4)}, {"lat": (4, 6)}
    h0 = w.write_open(base, sch, role="owner", n_writers=2, region=r0)
    h1 = w.write_open(base, sch, role="contributor", id=1, region=r1)
    w.write_record_at(h0, 0, 0.0, arrays(sch, 0, r0))
    w.write_record_at(h1, 0, 0.0, arrays(sch, 0, r1, True))
    t1 = w.write_flush(h1)
    # tokened but not yet committed: the contributor re-reads, may rewrite
    w.write_record_at(h1, 0, 0.0, arrays(sch, 0, r1, True))
    t1 = w.write_flush(h1)
    w.write_commit(h0, [w.write_flush(h0), t1])
    with pytest.raises(EarthSciIOError, match="committed"):
        w.write_record_at(h1, 0, 0.0, arrays(sch, 0, r1, True))
    w.write_invalidate(h0, [0])
    w.write_record_at(h1, 0, 0.0, arrays(sch, 3, r1, True))


# --------------------------------------------------------------------------- #
# Resume.
# --------------------------------------------------------------------------- #


def test_partial_shard_resume_equals_uninterrupted(tmp_path):
    sch = schema()
    ref = str(tmp_path / "ref")
    write_single(ref, sch, 5)
    base = str(tmp_path / "s")
    w = ZarrWriter()
    h = w.write_open(base, sch)
    for r in range(3):
        w.write_record(h, DT * r, arrays(sch, r))
    w.write_flush(h)
    w.write_record(h, DT * 3, arrays(sch, 99))  # shard [2, 4) filled: PUT, uncommitted
    del h  # dropped
    h = w.write_resume(base)
    assert read_output_manifest(base)["total_records"] == 3
    for r in range(3, 5):
        assert w.write_record(h, DT * r, arrays(sch, r)) == r
    m = w.write_close(h)
    assert m["schema"] == OUTPUT_MANIFEST_V1 and m["total_records"] == 5
    a, b = zarr.open_group(ref, mode="r"), zarr.open_group(base, mode="r")
    for nm in ("temperature", "pressure", "global_mean", "time"):
        assert a[nm][...].tobytes() == b[nm][...].tobytes(), nm


def test_partial_shard_resume_regions(tmp_path):
    sch = schema()
    base = str(tmp_path / "s")
    w = ZarrWriter()
    r0, r1 = {"lat": (0, 4)}, {"lat": (4, 6)}
    h0 = w.write_open(base, sch, role="owner", n_writers=2, region=r0)
    h1 = w.write_open(base, sch, role="contributor", id=1, region=r1)
    for r in range(3):
        w.write_record(h0, DT * r, arrays(sch, r, r0))
        w.write_record(h1, DT * r, arrays(sch, r, r1, True))
    w.write_commit(h0, [w.write_flush(h0), w.write_flush(h1)])
    w.write_record(h0, DT * 3, arrays(sch, 50, r0))
    w.write_record(h1, DT * 3, arrays(sch, 50, r1, True))
    w.write_flush(h0), w.write_flush(h1)
    del h0, h1
    v0 = w.write_resume(base, role="owner", n_writers=2, region=r0)
    v1 = w.write_resume(base, role="contributor", id=1, region=r1)
    assert read_output_manifest(base)["commit_seq"] == 1  # nothing truncated
    for r in range(3, 5):
        w.write_record(v0, DT * r, arrays(sch, r, r0))
        w.write_record(v1, DT * r, arrays(sch, r, r1, True))
    tok = w.write_flush(v1)
    w.write_close(v1)
    m = w.write_close(v0, [tok])
    assert m["commit_seq"] == 2
    got = read_all(base, sch)
    for nm, ov in sch.vars:
        sdims = [d for d in ov.dims if d != "time"]
        want = np.stack([gen(SEEDS[nm], r, ov.dtype, [range(sch.dim_lengths[d]) for d in sdims])
                         for r in range(5)])
        assert got[nm].tobytes() == want.tobytes(), nm


def test_truncate_on_resume(tmp_path):
    sch = schema()
    base = str(tmp_path / "s")
    write_single(base, sch, 5)
    w = ZarrWriter()
    h = w.write_resume(base, at=2 * DT)
    m = read_output_manifest(base)
    assert m["total_records"] == 3 and m["last_t"] == 2 * DT
    assert [e["n_records"] for e in m["time_shards"]] == [2, 1]
    assert json.loads((tmp_path / "s" / "temperature" / "zarr.json").read_text())["shape"][0] == 3
    with pytest.raises(ValueError, match="increasing"):
        w.write_record(h, 2 * DT, arrays(sch, 2))
    assert w.write_record(h, 3 * DT, arrays(sch, 13)) == 3
    w.write_close(h)
    got = read_all(base, sch)["temperature"]
    for i, r in enumerate([0, 1, 2, 13]):
        assert got[i].tobytes() == gen(100, r, "float64", [range(NLAT), range(NLON)]).tobytes()


def test_truncate_on_resume_v2_bumps_commit_seq_and_drops_blobs(tmp_path):
    sch = schema()
    base = str(tmp_path / "s")
    w = ZarrWriter()
    h = w.write_open(base, sch, role="owner", n_writers=1)
    for r in range(4):
        w.write_record(h, DT * r, arrays(sch, r))
    w.write_blob(h, ("record", 3), "k", b"x")
    w.write_blob(h, ("record", 1), "k", b"y")
    w.write_close(h, [])
    assert read_output_manifest(base)["commit_seq"] == 1
    w.write_resume(base, role="owner", n_writers=1, at=DT)
    m = read_output_manifest(base)
    assert m["commit_seq"] == 2 and m["n_records"] == 2
    assert m["blobs"] == [{"scope": "record", "record": 1, "writer": 0, "key": "k"}]
    w.write_resume(base, role="owner", n_writers=1, at=None)
    assert read_output_manifest(base)["commit_seq"] == 2  # no truncation, no write


# --------------------------------------------------------------------------- #
# dtypes, statics, blobs.
# --------------------------------------------------------------------------- #


def test_dtypes_round_trip_bitwise(tmp_path):
    dts = ["float64", "float32", "int32", "int64", "uint8"]
    vars_ = [(f"v_{dt}", OutputVar(["time", "lat", "lon"], dt)) for dt in dts]
    sch = schema(vars_=vars_)
    base = str(tmp_path / "s")
    m = write_single(base, sch, 3)
    assert [v["dtype"] for v in m["vars"]] == dts
    for i in range(3):
        for nm, ov in vars_:
            got = read_record(base, nm, i)
            want = gen(1, i, ov.dtype, [range(NLAT), range(NLON)])
            assert got.dtype == want.dtype
            assert got.tobytes() == want.tobytes()


def test_statics_and_blobs(tmp_path):
    sch = schema(statics=STATICS)
    base = str(tmp_path / "s")
    w = ZarrWriter()
    r0, r1 = {"lat": (0, 4)}, {"lat": (4, 6)}
    h0 = w.write_open(base, sch, role="owner", n_writers=2, region=r0)
    h1 = w.write_open(base, sch, role="contributor", id=1, region=r1)
    st = lambda reg, nm: arrays(sch, 0, reg, which=[s for s in STATICS if s[0] == nm])[nm]  # noqa: E731
    with pytest.raises(EarthSciIOError, match="replicated"):
        w.write_static(h1, "dJ_dlon", st({}, "dJ_dlon"))
    for r in range(2):
        w.write_record(h0, DT * r, arrays(sch, r, r0))
        w.write_record(h1, DT * r, arrays(sch, r, r1, True))
    for nm in ("dJ_dfield", "step_count"):
        w.write_static(h0, nm, st(r0, nm))
        w.write_static(h1, nm, st(r1, nm))
    w.write_static(h0, "dJ_dlon", st({}, "dJ_dlon"))
    w.write_blob(h0, "static", "rng", b"\x00\x01\x02\x03")
    w.write_blob(h1, "static", "rng", b"\xff\x10")
    w.write_blob(h1, ("record", 1), "ctrl.state", b"\xde\xad\xbe\xef")
    with pytest.raises(ValueError, match="key"):
        w.write_blob(h1, "static", "bad/key", b"")
    tok = w.write_flush(h1)
    with pytest.raises(EarthSciIOError, match="not committed"):
        read_blob(base, "static", 1, "rng")
    w.write_close(h1)
    m = w.write_close(h0, [tok])
    assert m["commit_seq"] == 1
    assert [(s["name"], s["committed"], s["generation"]) for s in m["statics"]] == [
        ("dJ_dfield", True, 1), ("dJ_dlon", True, 1), ("step_count", True, 1)]
    for nm, ov in STATICS:
        want = gen(SEEDS[nm], 0, ov.dtype, [range(sch.dim_lengths[d]) for d in ov.dims])
        assert read_static(base, nm).tobytes() == want.tobytes()
    assert read_static(base, "dJ_dfield", region={"lat": (4, 6)}).tobytes() == st(r1, "dJ_dfield").tobytes()
    assert read_blob(base, "static", 0, "rng") == b"\x00\x01\x02\x03"
    assert read_blob(base, "static", 1, "rng") == b"\xff\x10"
    assert read_blob(base, ("record", 1), 1, "ctrl.state") == b"\xde\xad\xbe\xef"

    # rewriting a committed static needs invalidate_statics
    h = w.write_resume(base, role="owner", n_writers=1)
    with pytest.raises(EarthSciIOError, match="invalidate_statics"):
        w.write_static(h, "dJ_dlon", st({}, "dJ_dlon"))


def test_partitioned_static_must_come_from_every_writer(tmp_path):
    sch = schema(statics=STATICS)
    base = str(tmp_path / "s")
    w = ZarrWriter()
    r0, r1 = {"lat": (0, 4)}, {"lat": (4, 6)}
    h0 = w.write_open(base, sch, role="owner", n_writers=2, region=r0)
    h1 = w.write_open(base, sch, role="contributor", id=1, region=r1)
    w.write_static(h0, "dJ_dfield", arrays(sch, 0, r0, which=STATICS[:1])["dJ_dfield"])
    with pytest.raises(CommitError, match="partitioned static"):
        w.write_commit(h0, [w.write_flush(h0), w.write_flush(h1)])


def test_blob_on_v1_store_raises(tmp_path):
    w = ZarrWriter()
    h = w.write_open(str(tmp_path / "s"), schema())
    with pytest.raises(EarthSciIOError, match="v2"):
        w.write_blob(h, "static", "k", b"x")


# --------------------------------------------------------------------------- #
# Manifest versions, tokens, fingerprint.
# --------------------------------------------------------------------------- #


def test_v1_manifest_is_the_pre_contract_format(tmp_path):
    base = tmp_path / "s"
    w = ZarrWriter()
    h = w.write_open(str(base), schema())
    assert not (base / "output_manifest.json").exists()  # v1: no manifest at create
    for r in range(3):
        w.write_record(h, DT * r, arrays(schema(), r))
    # legacy behaviour: a shard's worth of pending records commits
    assert read_output_manifest(str(base))["total_records"] == 2
    m = w.write_close(h)
    assert set(m) == {"schema", "base_url", "format", "zarr_format", "profile", "codec",
                      "time_dim", "dims", "vars", "chunk_shape", "shard_shape",
                      "time_shards", "last_t", "total_records", "written_at"}
    assert m["schema"] == OUTPUT_MANIFEST_V1
    assert m["dims"] == [["time", 0], ["lat", NLAT], ["lon", NLON]]
    assert m["time_shards"] == [
        {"index": 0, "t_start": 0.0, "t_end": DT, "n_records": 2},
        {"index": 1, "t_start": 2 * DT, "t_end": 2 * DT, "n_records": 1},
    ]
    text = (base / "output_manifest.json").read_text()
    assert text == json.dumps(m, indent=2, sort_keys=True) + "\n"


def test_v2_when_not_single_append_without_statics(tmp_path):
    w = ZarrWriter()
    for i, kw in enumerate([dict(role="owner", n_writers=1),
                            dict(sch=schema(statics=STATICS)),
                            dict(sch=schema(time_mode=IndexedTime([0.0])))]):
        sch = kw.pop("sch", schema())
        base = tmp_path / f"s{i}"
        w.write_open(str(base), sch, **kw)
        m = read_output_manifest(str(base))
        assert m["schema"] == OUTPUT_MANIFEST_V2 and m["commit_seq"] == 0
        assert m["schema_fingerprint"] == schema_fingerprint(sch)
        assert m["writers"] == [{"id": 0, "region": {}}]


def test_commit_token_wire_form():
    tok = CommitToken(writer=1, region={"lon": (4, 5), "lat": (2, 4)}, fingerprint="sha256:ab",
                      records=[(0, 0.0), (1, 0.1 + 0.2)], statics=["dJ_du0"],
                      blobs=[("record", 1, "rng"), ("static", None, "k")],
                      objects=["temperature/c/0/1/0"])
    d = json.loads(tok.to_json())
    assert list(d) == ["schema", "writer", "region", "fingerprint", "records", "statics",
                       "blobs", "objects"]
    assert d["schema"] == COMMIT_TOKEN_SCHEMA
    assert d["region"] == {"lat": [2, 4], "lon": [4, 5]}
    assert list(d["region"]) == ["lat", "lon"]
    assert d["records"] == [{"index": 0, "t": 0.0}, {"index": 1, "t": 0.30000000000000004}]
    assert d["blobs"] == [{"scope": "record", "record": 1, "key": "rng"},
                          {"scope": "static", "record": None, "key": "k"}]
    back = CommitToken.from_json(tok.to_json())
    assert back.records[1][1] == 0.1 + 0.2
    assert back.to_json() == tok.to_json()


def test_schema_fingerprint_canonical_json():
    sch = schema(statics=STATICS[:1])
    F = ('{"chunk_shape":{"lat":1,"lon":2,"time":1},"dims":[["time",0],["lat",6],["lon",5]],'
         '"profile":"diagnostic","shard_shape":{"lat":2,"lon":4,"time":2},'
         '"statics":[["dJ_dfield",["lat","lon"],"float64"]],"time_dim":"time",'
         '"time_mode":"append","vars":[["temperature",["time","lat","lon"],"float64"],'
         '["pressure",["time","lat","lon"],"float32"],["global_mean",["time"],"float64"]]}')
    assert schema_fingerprint(sch) == "sha256:" + hashlib.sha256(F.encode()).hexdigest()
    idx = schema(time_mode=IndexedTime([0.0, 1.0, 2.0]))
    assert schema_fingerprint(idx) != schema_fingerprint(schema())


def test_mode_mismatch_errors(tmp_path):
    w = ZarrWriter()
    h = w.write_open(str(tmp_path / "a"), schema())
    with pytest.raises(EarthSciIOError, match="append mode"):
        w.write_record_at(h, 0, 0.0, arrays(schema(), 0))
    sch = schema(time_mode=IndexedTime([0.0]))
    h = w.write_open(str(tmp_path / "b"), sch)
    with pytest.raises(EarthSciIOError, match="indexed mode"):
        w.write_record(h, 0.0, arrays(sch, 0))
    with pytest.raises(ValueError, match="at=None"):
        w.write_resume(str(tmp_path / "b"), at=1.0)


# --------------------------------------------------------------------------- #
# fsspec ``memory://`` (object-store code path).
# --------------------------------------------------------------------------- #


def test_memory_store_multi_writer(tmp_path):
    pytest.importorskip("fsspec")
    base = f"memory://esio-stream-{tmp_path.name}/out.zarr"
    sch = schema(statics=STATICS)
    w = ZarrWriter()
    r0, r1 = {"lat": (0, 4)}, {"lat": (4, 6)}
    h0 = w.write_open(base, sch, role="owner", n_writers=2, region=r0)
    h1 = w.write_open(base, sch, role="contributor", id=1, region=r1)
    for r in range(3):
        w.write_record(h0, DT * r, arrays(sch, r, r0))
        w.write_record(h1, DT * r, arrays(sch, r, r1, True))
    w.write_blob(h1, "static", "rng", b"abc")
    tok = w.write_flush(h1)
    w.write_close(h1)
    m = w.write_close(h0, [tok])
    assert m["n_records"] == 3
    full = gen(100, 2, "float64", [range(NLAT), range(NLON)])
    assert read_record(base, "temperature", 2).tobytes() == full.tobytes()
    assert read_blob(base, "static", 1, "rng") == b"abc"
    h = w.write_resume(base, role="owner", n_writers=1, at=DT)
    assert read_output_manifest(base)["n_records"] == 2
    assert h.n_committed == 2


# --------------------------------------------------------------------------- #
# Review fixes.
# --------------------------------------------------------------------------- #


def test_owner_off_the_origin_writes_replicated_vars_and_statics(tmp_path):
    # The owner holds lat [4, 6): no writer's region starts at 0 on lat, but
    # the replicated var, the time coordinate and the replicated static are
    # still the owner's to write.
    sch = schema(statics=STATICS)
    base = str(tmp_path / "off")
    w = ZarrWriter()
    r0, r1 = {"lat": (4, 6)}, {"lat": (0, 4)}
    h0 = w.write_open(base, sch, role="owner", n_writers=2, region=r0)
    h1 = w.write_open(base, sch, role="contributor", id=1, region=r1)
    st = lambda reg, nm: arrays(sch, 0, reg, which=[s for s in STATICS if s[0] == nm])[nm]  # noqa: E731
    for r in range(3):
        w.write_record(h0, DT * (r + 1), arrays(sch, r, r0))
        w.write_record(h1, DT * (r + 1), arrays(sch, r, r1, True))
    for nm in ("dJ_dfield", "step_count"):
        w.write_static(h0, nm, st(r0, nm))
        w.write_static(h1, nm, st(r1, nm))
    w.write_static(h0, "dJ_dlon", st({}, "dJ_dlon"))
    tok = w.write_flush(h1)
    w.write_close(h1)
    w.write_close(h0, [tok])
    got = read_all(base, sch)
    want = np.stack([gen(SEEDS["global_mean"], r, "float64", []) for r in range(3)])
    assert got["global_mean"].tobytes() == want.tobytes()
    assert list(zarr.open_group(base, mode="r")["time"][...]) == [DT, 2 * DT, 3 * DT]
    assert read_static(base, "dJ_dlon").tobytes() == st({}, "dJ_dlon").tobytes()


def test_failed_commit_leaves_state_unchanged_and_retries(tmp_path, monkeypatch):
    from earthsciio.backends import zarr_stream as zs

    real = zs._put_bytes
    fail = {"on": False}

    def flaky(store, key, data):
        if fail["on"] and key == zs.MANIFEST_KEY:
            raise OSError("injected manifest failure")
        return real(store, key, data)

    monkeypatch.setattr(zs, "_put_bytes", flaky)
    sch = schema()
    w = ZarrWriter()
    h = w.write_open(str(tmp_path / "o"), sch, role="owner", n_writers=1)
    for r in range(3):
        w.write_record(h, DT * r, arrays(sch, r))
    tok = w.write_flush(h)
    fail["on"] = True
    with pytest.raises(OSError):
        w.write_commit(h, [tok])
    fail["on"] = False
    m = w.write_commit(h, [tok])
    assert (m["commit_seq"], m["n_records"]) == (1, 3)

    # a single writer's failed flush keeps its writes pending
    h = w.write_open(str(tmp_path / "s"), sch)
    w.write_record(h, 0.0, arrays(sch, 0))
    fail["on"] = True
    with pytest.raises(OSError):
        w.write_flush(h)
    fail["on"] = False
    assert len(w.write_flush(h).records) == 1
    assert w.write_close(h)["total_records"] == 1


def test_blob_keys_dot_and_dotdot_are_refused(tmp_path):
    sch = schema(statics=STATICS)
    w = ZarrWriter()
    h = w.write_open(str(tmp_path / "b"), sch, role="owner", n_writers=1)
    for key in (".", ".."):
        with pytest.raises(ValueError, match="key"):
            w.write_blob(h, "static", key, b"x")


def test_resume_a_store_without_a_time_coordinate(tmp_path):
    # A Rust store has a time-coordinate array only when the schema's coords
    # carry one; resuming such a store continues from the manifest.
    import shutil

    sch = schema(statics=STATICS)  # statics: a v2 store
    base = str(tmp_path / "nt")
    w = ZarrWriter()
    h = w.write_open(base, sch, role="owner", n_writers=1)
    for r in range(3):
        w.write_record(h, DT * r, arrays(sch, r))
    w.write_close(h, [])
    shutil.rmtree(tmp_path / "nt" / "time")
    with pytest.raises(EarthSciIOError, match="time coordinate"):
        w.write_resume(base, role="owner", n_writers=1, at=DT)
    h = w.write_resume(base, role="owner", n_writers=1)
    w.write_record(h, DT * 3, arrays(sch, 3))
    m = w.write_close(h, [])
    assert m["n_records"] == 4 and m["records"]["committed"] == [[0, 4]]
    assert m["time_shards"] == [
        {"index": 0, "t_start": 0.0, "t_end": DT, "n": 2},
        {"index": 1, "t_start": 2 * DT, "t_end": 3 * DT, "n": 2},
    ]
    assert read_record(base, "temperature", 3).tobytes() == arrays(sch, 3)["temperature"].tobytes()
