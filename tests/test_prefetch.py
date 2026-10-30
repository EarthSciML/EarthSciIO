"""``sample_at``, ``with_window``, ``sample_windows_at`` and ``PrefetchGroup``.

``spec/prefetch.md`` §1–§3, driven where it can be by the committed SCENARIOS
(``conformance/corpus/scenarios/*.json``, ``spec/conformance.md`` §1
"Scenarios"): a decode case pins one dataset, while these pin a SEQUENCE of
calls — that a group's ``take`` sequence is bitwise the synchronous ``refresh``
sequence for every order, depth, worker count and budget with decode latency
randomized; that K windows from one decode equal K separately windowed
providers; and that a decode error surfaces at its own ``take`` and not before.
Each scenario's JSON carries the parameter grid and the expected anchors, so the
grid run here is the one the Julia and Rust suites run against the same corpus.

Every test is offline against the committed corpus (or an empty root, for the
cold-cache case), except the ``fetch_ahead`` test, which needs an online cache
over local ``file://`` copies to have something to download. The peers are
``julia/test/test_prefetch.jl`` and ``rust/tests/prefetch_scenarios.rs``.
"""

from __future__ import annotations

import dataclasses
import datetime as dt
import json
import pathlib
import random
import threading
import time
from concurrent.futures import ThreadPoolExecutor

import numpy as np
import pytest

from earthsciio import (
    CacheMiss,
    Cache,
    DataSource,
    DimWindow,
    EpochSample,
    PrefetchGroup,
    Provider,
    Sample,
    SourceTemporal,
)
from earthsciio.prefetch import _dataset_nbytes

REPO_ROOT = pathlib.Path(__file__).resolve().parent.parent
CORPUS = REPO_ROOT / "conformance" / "corpus"
SCENARIOS = CORPUS / "scenarios"

# Everything here decodes NetCDF; the lean CI leg has no xarray/netCDF4.
pytestmark = pytest.mark.needs_format("netcdf")


def _scenario(name: str) -> dict:
    return json.loads((SCENARIOS / f"{name}.json").read_text())


def _iso(s: str) -> dt.datetime:
    return dt.datetime.fromisoformat(s.replace("Z", "+00:00"))


def _utc(seconds: int) -> dt.datetime:
    return dt.datetime.fromtimestamp(seconds, tz=dt.timezone.utc)


@pytest.fixture
def cache():
    return Cache(root=CORPUS / "cache", offline=True, verify=True)


def _provider(source: dict, rps, cache, name="scenario") -> Provider:
    """A DISCRETE provider over a scenario source (its strftime URL template)."""
    return Provider(
        DataSource(
            name, source["format"], source["url_template"]["strftime"],
            variables=list(source["variables"]),
            temporal=SourceTemporal(
                start=_iso(source["start"]),
                frequency=dt.timedelta(seconds=source["frequency_s"]),
                file_period=dt.timedelta(seconds=source["file_period_s"]),
                end=_iso(source["end"]),
                time_dim=source["time_dim"],
                records_per_sample=rps,
            ),
        ),
        cache,
    )


def assert_bitwise(a, b, label=""):
    """Two NativeDatasets equal BIT FOR BIT: names, dims, attrs, dtypes, shapes
    and the raw bytes (so NaN equals NaN only with the same payload)."""
    assert sorted(a.variables) == sorted(b.variables), label
    assert sorted(a.coords) == sorted(b.coords), label
    for group in ("variables", "coords"):
        for name, fa in getattr(a, group).items():
            fb = getattr(b, group)[name]
            where = f"{label} {group}.{name}"
            assert fa.dims == fb.dims, where
            assert fa.attrs == fb.attrs, where
            da, db = np.asarray(fa.data), np.asarray(fb.data)
            assert da.dtype == db.dtype, where
            assert da.shape == db.shape, where
            assert da.tobytes() == db.tobytes(), where


def _indices(spec, n):
    if spec == "all":
        return list(range(n))
    if isinstance(spec, list):
        return list(spec)
    if "indices" in spec:
        return list(spec["indices"])
    s = spec["slice"]
    return list(range(s[0], s[1], s[2] if len(s) > 2 else 1))


def _sliced_by_name(ds, window):
    """The whole read sliced by name afterwards — the gate's right-hand side."""
    from earthsciio import NativeDataset, NativeField

    dims = window["dims"]

    def cut(f):
        data = np.asarray(f.data)
        for axis, dim in enumerate(f.dims):
            if dim in dims:
                data = np.take(data, _indices(dims[dim], data.shape[axis]), axis=axis)
        return NativeField(data, f.dims, f.attrs)

    return NativeDataset({n: cut(f) for n, f in ds.variables.items()},
                         {n: cut(f) for n, f in ds.coords.items()})


def _assert_equal_values(a, b, label=""):
    for group in ("variables", "coords"):
        assert sorted(getattr(a, group)) == sorted(getattr(b, group)), label
        for name, fa in getattr(a, group).items():
            fb = getattr(b, group)[name]
            assert fa.dims == fb.dims, f"{label} {name}"
            assert np.array_equal(np.asarray(fa.data), np.asarray(fb.data),
                                  equal_nan=True), f"{label} {name}"


# --------------------------------------------------------------------------- #
# §1 sample_at — refresh's data, no buffer touched.
# --------------------------------------------------------------------------- #


def _buffers(p):
    return (p._current, p._current_anchor, p._current_file,
            p._current_file_anchor, list(p._files.items()))


def _same_buffers(before, after):
    return all(x is y for x, y in zip(before[:4], after[:4])) and \
        [(k, id(v)) for k, v in before[4]] == [(k, id(v)) for k, v in after[4]]


@pytest.mark.parametrize("rps", [1, 2])
def test_sample_at_is_refresh_and_touches_no_buffer(cache, rps):
    src = _scenario("prefetch-equivalence")["sources"]["hourly"]
    stepping = _provider(src, rps, cache)
    peeker = _provider(src, rps, cache)
    peeker.refresh(_utc(src["refresh_times_s"][5]))  # a primed, non-empty buffer
    for s in src["refresh_times_s"]:
        t = _utc(s)
        want = stepping.refresh(t)
        before = _buffers(peeker)
        got = peeker.sample_at(t)
        assert _same_buffers(before, _buffers(peeker)), "sample_at touched a buffer"
        assert_bitwise(got, want, f"rps={rps} t={t}")
        # ...and it depends on t only through the anchor t snaps to
        assert_bitwise(peeker.sample_at(t + dt.timedelta(minutes=59)), want)
    fresh = _provider(src, rps, cache)
    fresh.sample_at(_utc(src["refresh_times_s"][0]))
    assert fresh._current is None and not fresh._files and fresh._current_file is None


def test_sample_at_raises_what_refresh_raises(cache, tmp_path):
    src = _scenario("prefetch-errors")["source"]
    p = _provider(src, 1, cache)
    with pytest.raises(ValueError, match="precedes the loader start"):
        p.sample_at(_iso(src["start"]) - dt.timedelta(hours=1))
    missing = _utc(src["refresh_times_s"][12])
    with pytest.raises(CacheMiss):
        p.sample_at(missing)
    with pytest.raises(CacheMiss):
        p.refresh(missing)
    cold = _provider(src, 1, Cache(root=tmp_path / "empty", offline=True))
    with pytest.raises(CacheMiss):
        cold.sample_at(_iso(src["start"]))


def test_sample_at_is_safe_from_several_threads(cache):
    src = _scenario("prefetch-equivalence")["sources"]["hourly"]
    times = [_utc(s) for s in src["refresh_times_s"]] * 4
    p = _provider(src, 2, cache)
    with ThreadPoolExecutor(max_workers=8) as pool:
        got = list(pool.map(p.sample_at, times))
    ref = _provider(src, 2, cache)
    for t, ds in zip(times, got):
        assert_bitwise(ds, ref.sample_at(t), str(t))
    assert p._current is None and not p._files


def test_const_sample_at_is_materialize(cache):
    url = "https://data.earthsci.dev/geosfp/mini/GEOSFP.20240101.A3mixed.nc"
    p = Provider(DataSource("mixed", "netcdf", url), cache)
    got = p.sample_at(dt.datetime(2030, 1, 1, tzinfo=dt.timezone.utc))
    assert p._current is None
    assert_bitwise(got, p.materialize())


# --------------------------------------------------------------------------- #
# §2.3 with_window — a window bound at construction.
# --------------------------------------------------------------------------- #


class _NoIO:
    """A cache that fails the test on any fetch: proves "before any I/O"."""

    offline = True

    def fetch(self, *a, **kw):  # pragma: no cover - reaching it is the failure
        raise AssertionError("fetched")


def _hourly_on(cache, rps=1):
    return _provider(_scenario("prefetch-equivalence")["sources"]["hourly"], rps, cache)


def test_with_window_is_a_new_provider_sharing_cache_and_reader(cache):
    p = _hourly_on(cache)
    w = DimWindow({"lat": (1, 2), "lon": [2, 0]})
    q = p.with_window(w)
    assert q is not p and q.cache is p.cache and q._reader is p._reader
    assert q.loader.reader_kwargs["select"] == w.select
    assert "select" not in p.loader.reader_kwargs  # the original is unchanged
    t = p.refresh_times()[3]
    assert_bitwise(q.sample_at(t), _sliced_by_name(p.sample_at(t), w.select))
    # the wire dict is accepted wherever a DimWindow is
    assert_bitwise(p.with_window(w.select).sample_at(t), q.sample_at(t))


def test_with_window_errors_are_raised_before_any_io(cache):
    p = _hourly_on(_NoIO())
    with pytest.raises(ValueError, match="already bakes a select"):
        p.with_window(DimWindow({"lat": [0]})).with_window(DimWindow({"lon": [0]}))
    with pytest.raises(ValueError, match="time dimension 'time'"):
        p.with_window(DimWindow({"time": [0]}))
    with pytest.raises(ValueError, match="time dimension 'time'"):
        p.with_window({"dims": {"time": "all"}})
    with pytest.raises(ValueError, match="names its dimensions"):
        p.with_window({"axes": ["all", "all", "all"]})
    with pytest.raises(TypeError):
        p.with_window({"lat": (0, 1)})
    baked = Provider(DataSource("h", "netcdf", "https://x/a.nc",
                                reader_kwargs={"select": {"axes": ["all"]}}), _NoIO())
    with pytest.raises(ValueError, match="already bakes a select"):
        baked.with_window(DimWindow({"lat": [0]}))
    csv = Provider(DataSource("o", "csv", "https://x/a.csv"), _NoIO())
    with pytest.raises(ValueError, match="does not support selection"):
        csv.with_window(DimWindow({"index": [0]}))
    with pytest.raises(ValueError, match="does not support selection"):
        csv.sample_windows_at(dt.datetime(2024, 1, 1), [DimWindow({"index": [0]})])


def test_a_baked_window_keeps_one_decode_per_file(cache, monkeypatch):
    """The point of binding a window at construction: `refresh` still decodes
    each windowed file once, not once per tick (a per-call select would not)."""
    p = _hourly_on(cache).with_window(DimWindow({"lon": [2, 0]}))
    calls = []
    real = p._reader.read_native

    def counting(*a, **kw):
        calls.append(kw.get("select"))
        return real(*a, **kw)

    monkeypatch.setattr(p._reader, "read_native", counting)
    for t in p.refresh_times():
        p.refresh(t)
    assert len(calls) == 3  # three files, twelve ticks
    assert all(c == {"dims": {"lon": {"indices": [2, 0]}}} for c in calls)


# --------------------------------------------------------------------------- #
# §2.4 sample_windows_at — the `multi-window` scenario.
# --------------------------------------------------------------------------- #


def test_sample_windows_at_of_nothing_fetches_nothing():
    p = _hourly_on(_NoIO())
    assert p.sample_windows_at(p.refresh_times()[0], []) == []


def test_multi_window_scenario_const(cache):
    sc = _scenario("multi-window")["const"]
    p = Provider(DataSource("mixed", sc["format"], sc["url"],
                            variables=list(sc["variables"])), cache)
    t = dt.datetime(2024, 1, 1, tzinfo=dt.timezone.utc)
    full = p.materialize()
    windows = sc["windows"]
    got = p.sample_windows_at(t, windows)
    assert len(got) == len(windows)
    for i, (w, ds) in enumerate(zip(windows, got)):
        assert_bitwise(ds, p.with_window(w).sample_at(t), f"window {i}")
        _assert_equal_values(ds, _sliced_by_name(full, w), f"window {i} vs sliced")
    # DimWindows and wire dicts mix, and each window keeps its own order
    mixed = p.sample_windows_at(t, [DimWindow.from_select(windows[1]), windows[0]])
    assert_bitwise(mixed[0], got[1])
    assert_bitwise(mixed[1], got[0])


@pytest.mark.parametrize("rps", [1, 2])
def test_multi_window_scenario_discrete(cache, rps):
    sc = _scenario("multi-window")["discrete"]
    assert rps in sc["records_per_sample"]
    p = _provider(sc["source"], rps, cache)
    windowed = [p.with_window(w) for w in sc["windows"]]
    stepping = [_provider(sc["source"], rps, cache).with_window(w) for w in sc["windows"]]
    for s in sc["source"]["refresh_times_s"]:  # file seams included
        t = _utc(s)
        got = p.sample_windows_at(t, sc["windows"])
        whole = p.sample_at(t)
        for i, w in enumerate(sc["windows"]):
            assert_bitwise(got[i], windowed[i].sample_at(t), f"t={t} window {i}")
            assert_bitwise(got[i], stepping[i].refresh(t), f"t={t} window {i} refresh")
            _assert_equal_values(got[i], _sliced_by_name(whole, w), f"t={t} window {i}")


def test_sample_windows_at_bounds_checks_a_window_the_union_widened(cache):
    """Window B leaves `lon` whole, so the union reads all of it: window A's
    out-of-range `lon` index must still be refused, as it would be alone."""
    p = _hourly_on(cache)
    t = p.refresh_times()[0]
    with pytest.raises(IndexError, match="out of range"):
        p.with_window(DimWindow({"lon": [7]})).sample_at(t)
    with pytest.raises(IndexError, match="out of range"):
        p.sample_windows_at(t, [DimWindow({"lon": [7]}), DimWindow({"lat": [0]})])
    with pytest.raises(ValueError, match="no field of this read carries"):
        p.sample_windows_at(t, [DimWindow({"lev": "all"}), DimWindow({"lat": [0]})])


# --------------------------------------------------------------------------- #
# §3 PrefetchGroup — the `prefetch-equivalence` scenario, its full grid.
# --------------------------------------------------------------------------- #


def _members(sc, cache):
    return {m["key"]: _provider(sc["sources"][m["source"]], m["records_per_sample"], cache,
                                name=m["key"])
            for m in sc["members"]}


def _order_list(order, n):
    if order == "forward":
        return list(range(n))
    if order == "reverse":
        return list(range(n - 1, -1, -1))
    return list(order)


def _sync_sequence(sc, cache, order):
    """What a FRESH provider per member returns stepping `refresh` along the
    order — the right-hand side of the equivalence gate."""
    fresh = _members(sc, cache)
    B = [_utc(s) for s in sc["expect"]["boundaries_s"]]
    return {key: [fresh[key].refresh(B[k]) for k in order] for key in fresh}


def _latency():
    return random.uniform(0.0, 0.004)


@pytest.fixture(scope="module")
def equivalence():
    return _scenario("prefetch-equivalence")


def test_prefetch_boundaries_and_forward_anchors_match_the_scenario(cache, equivalence):
    sc = equivalence
    with PrefetchGroup(_members(sc, cache)) as g:
        B = g.boundaries()
        assert [int(b.timestamp()) for b in B] == sc["expect"]["boundaries_s"]
        g.start("forward")
        anchors = {k: [] for k in sc["expect"]["anchors_s"]}
        changed = {k: [] for k in sc["expect"]["anchors_s"]}
        for k in range(len(B)):
            ep = g.take(k)
            assert isinstance(ep, EpochSample) and ep.epoch == k and ep.time == B[k]
            for key in anchors:
                assert isinstance(ep[key], Sample)
                anchors[key].append(int(ep[key].anchor.timestamp()))
                changed[key].append(ep[key].changed)
    assert anchors == sc["expect"]["anchors_s"]
    assert changed == sc["expect"]["changed_forward"]


_GRID = [
    (oi, depth, workers, max_bytes)
    for oi in range(3)
    for depth in (1, 2, 3)
    for workers in (1, 4)
    for max_bytes in (None, 1)
]


@pytest.mark.parametrize("oi,depth,workers,max_bytes", _GRID)
def test_prefetch_equivalence_grid(cache, equivalence, oi, depth, workers, max_bytes):
    sc = equivalence
    assert depth in sc["depth"] and workers in sc["workers"] and max_bytes in sc["max_bytes"]
    order_spec = sc["orders"][oi]
    n = len(sc["expect"]["boundaries_s"])
    order = _order_list(order_spec, n)
    want = _sync_sequence(sc, cache, order)
    members = _members(sc, cache)
    with PrefetchGroup(members, depth=depth, workers=workers, max_bytes=max_bytes,
                       _decode_latency=_latency) as g:
        g.start(order_spec)
        prev = {}
        for i, k in enumerate(order):
            ep = g.take(k)
            for key in members:
                s = ep[key]
                assert_bitwise(s.data, want[key][i], f"{order_spec!r} pos {i} {key}")
                assert_bitwise(s.data, members[key].sample_at(g.boundaries()[k]))
                assert s.anchor == members[key]._anchor(g.boundaries()[k])
                assert s.changed == (i == 0 or prev[key] != s.anchor)
                prev[key] = s.anchor
        st = g.stats()
        with pytest.raises(RuntimeError, match="exhausted"):
            g.take(order[0])
    assert st["epochs_decoded"] == len(order)
    assert st["samples_decoded"] + st["samples_reused"] == len(order) * len(members)
    assert st["overlap"] is True
    if max_bytes is not None:
        # every slot is bigger than one byte: each was decoded alone, oversize
        assert st["oversize_epochs"] == len(order)
        biggest = max(sum(_dataset_nbytes(want[key][i]) for key in members)
                      for i in range(len(order)))
        assert 0 < st["peak_bytes"] <= biggest
    else:
        assert st["oversize_epochs"] == 0


def test_prefetch_equivalence_with_windows(cache, equivalence):
    sc = equivalence
    windows = sc["windows"]
    members = _members(sc, cache)
    B = [_utc(s) for s in sc["expect"]["boundaries_s"]]
    with PrefetchGroup(members, depth=2, workers=4, windows=windows,
                       _decode_latency=_latency) as g:
        g.start("forward")
        for k in range(len(B)):
            ep = g.take(k)
            for key, p in members.items():
                assert isinstance(ep[key].data, list) and len(ep[key].data) == len(windows)
                for i, w in enumerate(windows):
                    assert_bitwise(ep[key].data[i], p.with_window(w).sample_at(B[k]),
                                   f"epoch {k} {key} window {i}")


def test_prefetch_a_coarse_member_is_decoded_once_per_anchor(cache, equivalence):
    """Even at depth 1 — never two slots held at once — the two-hourly PS is
    decoded once per anchor across the hourly union: the sample last handed to
    the consumer is reused when the next slot snaps to the same anchor."""
    sc = equivalence
    m = _members(sc, cache)
    with PrefetchGroup({"PS": m["PS"], "Q1": m["Q1"]}, depth=1) as g:
        g.start("forward")
        for k in range(len(g.boundaries())):
            ep = g.take(k)
            if not ep["PS"].changed:
                assert ep["PS"].data is last  # the same, shared, read-only object
            last = ep["PS"].data
        st = g.stats()
    assert st["samples_decoded"] == 12 + 6 and st["samples_reused"] == 6


def test_prefetch_data_is_read_only(cache, equivalence):
    with PrefetchGroup(_members(equivalence, cache)) as g:
        g.start("forward")
        ep = g.take(0)
    with pytest.raises(ValueError, match="read-only"):
        ep["Q1"].data.variables["Q"].data[...] = 0.0


def test_prefetch_transform_runs_on_the_decode_and_its_errors_are_deferred(cache, equivalence):
    sc = equivalence
    members = _members(sc, cache)
    B = [_utc(s) for s in sc["expect"]["boundaries_s"]]

    def total(key, ds):
        if key == "Q2" and ds.coords["time"].data[0] >= B[3].timestamp():
            raise ArithmeticError("transform failed")
        return float(np.nansum(ds.variables[next(iter(ds.variables))].data))

    with PrefetchGroup(members, depth=3, workers=2, transform=total) as g:
        g.start("forward")
        for k in range(3):
            ep = g.take(k)
            for key, p in members.items():
                ds = p.sample_at(B[k])
                assert ep[key].data == float(np.nansum(ds.variables[next(iter(ds.variables))].data))
        with pytest.raises(ArithmeticError, match="transform failed"):
            g.take(3)


# --------------------------------------------------------------------------- #
# §3.7 errors — the `prefetch-errors` scenario.
# --------------------------------------------------------------------------- #


@pytest.fixture(scope="module")
def errors_scenario():
    return _scenario("prefetch-errors")


def _wait_until(cond, timeout=5.0):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if cond():
            return True
        time.sleep(0.005)
    return False


@pytest.mark.parametrize("depth", [1, 3])
@pytest.mark.parametrize("workers", [1, 4])
def test_prefetch_errors_cachemiss_at_its_take_and_not_before(cache, errors_scenario,
                                                              depth, workers):
    sc = errors_scenario
    assert depth in sc["depth"] and workers in sc["workers"]
    miss = sc["missing_from"]
    member = _provider(sc["source"], sc["records_per_sample"], cache)
    ref = _provider(sc["source"], sc["records_per_sample"], cache)
    with PrefetchGroup({"Q": member}, depth=depth, workers=workers,
                       _decode_latency=_latency) as g:
        B = g.boundaries()
        assert [int(b.timestamp()) for b in B] == sc["source"]["refresh_times_s"]
        g.start("forward")
        for k in range(miss):
            if k == miss - 2 and depth > 2:
                # the workers reach the missing file EARLY: its slot completes
                # (failed) while two good epochs are still ahead of it...
                assert _wait_until(lambda: g._slot_done(miss))
            assert_bitwise(g.take(k)["Q"].data, ref.refresh(B[k]), f"epoch {k}")
        # ...and its error is still raised by its own take, as a CacheMiss
        with pytest.raises(CacheMiss) as info:
            g.take(miss)
        assert type(info.value) is CacheMiss
        assert info.value.resolved_url == sc["source"]["files"][3]["url"]
        with pytest.raises(RuntimeError, match="start"):
            g.take(miss + 1)
        assert g.ready(miss + 1) is False
        # start(order, from_=k+1) resumes past the failure... into the same file
        g.start("forward", from_=miss + 1)
        with pytest.raises(CacheMiss):
            g.take(miss + 1)


def test_prefetch_errors_cancel_then_start_from_reproduces_the_sequence(cache, errors_scenario):
    sc = errors_scenario
    member = _provider(sc["source"], sc["records_per_sample"], cache)
    ref = _provider(sc["source"], sc["records_per_sample"], cache)
    cut, miss = sc["cancel_after"], sc["missing_from"]
    with PrefetchGroup({"Q": member}, depth=3, workers=4, _decode_latency=_latency) as g:
        B = g.boundaries()
        g.start("forward")
        for k in range(cut):
            assert_bitwise(g.take(k)["Q"].data, ref.refresh(B[k]))
        g.cancel()
        with pytest.raises(RuntimeError, match="after cancel"):
            g.take(cut)
        assert g.ready(cut) is False
        g.start("forward", from_=cut)
        first = True
        for k in range(cut, miss):
            s = g.take(k)["Q"]
            assert s.changed  # hourly: every anchor is new, the first after start too
            assert first or s.anchor > prev
            first, prev = False, s.anchor
            assert_bitwise(s.data, ref.refresh(B[k]), f"epoch {k}")
        with pytest.raises(CacheMiss):
            g.take(miss)


def test_prefetch_errors_cold_cache_raises_at_take(tmp_path, errors_scenario):
    sc = errors_scenario
    cold = Cache(root=tmp_path / "empty", offline=True)
    member = _provider(sc["source"], sc["records_per_sample"], cold)
    with PrefetchGroup({"Q": member}, depth=3, workers=2) as g:
        g.start("forward")  # never raises: the error is stored...
        assert _wait_until(lambda: g.ready(0))
        with pytest.raises(CacheMiss):  # ...and raised by take
            g.take(0)


def test_prefetch_multi_member_failure_raises_the_first_by_key(cache, errors_scenario, tmp_path):
    sc = errors_scenario
    cold = Cache(root=tmp_path / "empty", offline=True)
    good = _provider(sc["source"], 1, cache)
    bad_b = _provider(sc["source"], 1, cold, name="b")
    bad_a = Provider(DataSource("a", "netcdf", "https://data.earthsci.dev/none/%H.nc",
                                temporal=good.loader.temporal), cold)
    with PrefetchGroup({"z": good, "b": bad_b, "a": bad_a}, depth=2, workers=3) as g:
        g.start("forward")
        with pytest.raises(CacheMiss) as info:
            g.take(0)
    assert "data.earthsci.dev/none/" in info.value.resolved_url


def test_prefetch_an_epoch_before_a_member_starts_raises_at_its_take(cache, equivalence):
    """§3.2: the union of schedules can begin before a member's own first anchor;
    that member's sample there raises what its refresh would — at take, only."""
    sc = equivalence
    q1 = _members(sc, cache)["Q1"]
    late_t = dataclasses.replace(q1.loader.temporal, start=_utc(sc["expect"]["boundaries_s"][1]))
    late = Provider(dataclasses.replace(q1.loader, temporal=late_t), cache)
    with pytest.raises(ValueError, match="precedes the loader start"):
        late.refresh(_utc(sc["expect"]["boundaries_s"][0]))
    with PrefetchGroup({"A": q1, "B": late}, depth=3) as g:
        g.start("forward")
        assert _wait_until(lambda: g.ready(0))
        with pytest.raises(ValueError, match="precedes the loader start"):
            g.take(0)


# --------------------------------------------------------------------------- #
# §3.3 / §3.8 the API's own errors, cancellation and lifetime.
# --------------------------------------------------------------------------- #


def test_prefetch_take_errors(cache, equivalence):
    with PrefetchGroup(_members(equivalence, cache), depth=2) as g:
        with pytest.raises(RuntimeError, match="before start"):
            g.take(0)
        assert g.ready(0) is False
        g.start([3, 1, 4])
        with pytest.raises(ValueError, match="next epoch of the current order is 3"):
            g.take(0)
        assert g.take(3).epoch == 3  # the wrong take consumed nothing
        with pytest.raises(ValueError, match="is 1 "):
            g.take(4)
        g.take(1)
        g.take(4)
        with pytest.raises(RuntimeError, match="exhausted"):
            g.take(4)
        g.start("reverse")
        assert g.take(11).epoch == 11
        g.cancel(wait=True)
        with pytest.raises(RuntimeError, match="after cancel"):
            g.take(10)
        g.start("reverse", from_=10)  # reusable after start
        assert g.take(10).epoch == 10
    with pytest.raises(RuntimeError, match="closed"):
        g.start("forward")
    with pytest.raises(RuntimeError, match="closed"):
        g.take(0)
    g.close()  # idempotent


@pytest.mark.parametrize("order,from_,exc,msg", [
    ([], None, ValueError, "at least one epoch"),
    ([0, 12], None, ValueError, r"\[12\] are out of range"),
    ([-1], None, ValueError, "out of range"),
    ("sideways", None, ValueError, "forward"),
    ([0.5], None, TypeError, "epoch"),
    ([True], None, TypeError, "epoch"),
    ("forward", 12, ValueError, "from_=12 is not an epoch"),
    ([3, 1, 4], 2, ValueError, "from_=2 is not an epoch"),
])
def test_prefetch_bad_orders_are_refused_at_start(cache, equivalence, order, from_, exc, msg):
    with PrefetchGroup(_members(equivalence, cache)) as g:
        g.start([5, 6])
        with pytest.raises(exc, match=msg):
            g.start(order, from_=from_)
        assert g.take(5).epoch == 5  # a refused start leaves the order as it was


def test_prefetch_start_from_trims_to_the_first_occurrence(cache, equivalence):
    with PrefetchGroup(_members(equivalence, cache)) as g:
        g.start([3, 1, 4, 1, 5], from_=1)
        assert [g.take(k).epoch for k in (1, 4, 1, 5)] == [1, 4, 1, 5]


def test_prefetch_start_cancels_work_in_flight(cache, equivalence):
    with PrefetchGroup(_members(equivalence, cache), depth=3, workers=2,
                       _decode_latency=lambda: 0.02) as g:
        g.start("forward")
        g.start("reverse")  # while the forward decodes are queued or running
        assert g.take(11).epoch == 11
        g.cancel(wait=True)
        assert not g._futures or all(f.done() for f in list(g._futures))


@pytest.mark.parametrize("bad,exc,msg", [
    ({}, ValueError, "non-empty"),
    ({"c": "const"}, ValueError, "CONST"),
    ({"u": "unbounded"}, ValueError, "no enumerable schedule"),
    ({"q": "provider", "_": {"depth": 0}}, ValueError, "depth must be >= 1"),
    ({"q": "provider", "_": {"workers": 0}}, ValueError, "workers must be >= 1"),
    ({"q": "provider", "_": {"max_bytes": -1}}, ValueError, "max_bytes"),
    ({"q": "provider", "_": {"fetch_ahead": -1}}, ValueError, "fetch_ahead"),
    ({"q": "provider", "_": {"executor": "gpu"}}, ValueError, "executor"),
    ({"q": "provider", "_": {"windows": [DimWindow({"time": [0]})]}}, ValueError, "time"),
    ({"q": "windowed", "_": {"windows": [DimWindow({"lat": [0]})]}}, ValueError, "already bakes"),
    ({"q": "lambda-url", "_": {"executor": "process"}}, TypeError, "must pickle"),
    ({"q": "provider", "_": {"executor": "process", "transform": lambda k, d: d}},
     TypeError, "must pickle"),
])
def test_prefetch_construction_errors(cache, bad, exc, msg):
    src = _scenario("prefetch-equivalence")["sources"]["hourly"]
    kwargs = dict(bad.pop("_", {}))
    members = {}
    for key, what in bad.items():
        if what == "const":
            members[key] = Provider(DataSource(key, "netcdf", "https://x/a.nc"), cache)
        elif what == "unbounded":
            t = SourceTemporal(_iso(src["start"]), dt.timedelta(hours=1), dt.timedelta(hours=4))
            members[key] = Provider(DataSource(key, "netcdf", "https://x/%H.nc", temporal=t), cache)
        elif what == "windowed":
            members[key] = _provider(src, 1, cache).with_window(DimWindow({"lon": [0]}))
        elif what == "lambda-url":
            p = _provider(src, 1, cache)
            p.loader.url = lambda anchor: anchor.strftime(src["url_template"]["strftime"])
            members[key] = p
        else:
            members[key] = _provider(src, 1, cache)
    with pytest.raises(exc, match=msg):
        PrefetchGroup(members, **kwargs)


# --------------------------------------------------------------------------- #
# §3.9 waited_s falls to about zero when compute exceeds decode.
# --------------------------------------------------------------------------- #


def test_prefetch_hides_decode_behind_compute(cache, equivalence):
    """Each epoch's decode (an injected 15 ms per member sample plus the real
    decode) is shorter than the consumer's synthetic 80 ms of compute, so after
    the first epoch every take finds its slot ready. The same loop with no
    compute must wait for nearly every decode — the contrast shows the overlap
    is real rather than the decode being free."""
    compute, latency = 0.08, 0.015
    members = _members(equivalence, cache)
    n = len(equivalence["expect"]["boundaries_s"])
    with PrefetchGroup(members, depth=2, workers=1,
                       _decode_latency=lambda: latency) as g:
        g.start("forward")
        assert _wait_until(lambda: g.ready(0))
        for k in range(n):
            g.take(k)
            time.sleep(compute)
        hidden = g.stats()
    with PrefetchGroup(_members(equivalence, cache), depth=2, workers=1,
                       _decode_latency=lambda: latency) as g:
        g.start("forward")
        for k in range(n):
            g.take(k)
        exposed = g.stats()
    assert hidden["waited_s"] < 0.1, hidden
    assert exposed["waited_s"] > 10 * hidden["waited_s"] + 0.1, (hidden, exposed)
    assert hidden["decode_s"] > 0 and hidden["overlap"] is True


# --------------------------------------------------------------------------- #
# §3.10 executor="process".
# --------------------------------------------------------------------------- #


def test_prefetch_process_executor_smoke(cache, errors_scenario):
    """Decoding in worker processes: the same bytes as the synchronous refresh,
    and a worker's CacheMiss reaches take() as a CacheMiss — the provider, its
    cache and the error all cross the process boundary intact."""
    sc = errors_scenario
    member = _provider(sc["source"], 2, cache)
    ref = _provider(sc["source"], 2, cache)
    miss = sc["missing_from"]
    with PrefetchGroup({"Q": member}, depth=3, workers=2, executor="process") as g:
        B = g.boundaries()
        g.start("forward", from_=miss - 3)
        for k in range(miss - 3, miss):
            s = g.take(k)["Q"]
            assert_bitwise(s.data, ref.refresh(B[k]), f"epoch {k}")
            assert not s.data.variables["Q"].data.flags.writeable
        with pytest.raises(CacheMiss) as info:
            g.take(miss)
        assert info.value.key and info.value.resolved_url.endswith("_1200.nc")
        st = g.stats()
    assert st["samples_decoded"] == 3 and st["decode_s"] > 0


# --------------------------------------------------------------------------- #
# §3.6 the budget: a finite max_bytes, and fetch_ahead.
# --------------------------------------------------------------------------- #


def test_prefetch_max_bytes_bounds_the_held_slots(cache, equivalence):
    """With room for about two and a half slots, a depth-3 group never holds
    more than the budget, yet still runs ahead (it is not degraded to one slot
    at a time the way a sub-slot budget is)."""
    sc = equivalence
    B = [_utc(s) for s in sc["expect"]["boundaries_s"]]
    probe = _members(sc, cache)
    slot = sum(_dataset_nbytes(p.sample_at(B[0])) for p in probe.values())
    budget = int(2.5 * slot)
    with PrefetchGroup(_members(sc, cache), depth=3, workers=2, max_bytes=budget,
                       _decode_latency=_latency) as g:
        g.start("forward")
        for k in range(len(B)):
            ep = g.take(k)
            for key, p in probe.items():
                assert_bitwise(ep[key].data, p.sample_at(B[k]))
        st = g.stats()
    assert st["oversize_epochs"] == 0
    assert slot < st["peak_bytes"] <= budget


def _file_source(tmp_path, end="2024-01-01T16:00:00Z"):
    """The hourly scenario source served from local files (`file://`), so an
    ONLINE cache has something to download: three corpus blobs copied out,
    declared one file longer than that, like `prefetch-errors`."""
    src = _scenario("prefetch-errors")["source"]
    d = tmp_path / "src"
    d.mkdir()
    for f in src["files"]:
        if f["in_corpus"]:
            blob = next(b for b in _scenario("prefetch-errors")["blobs"] if b["url"] == f["url"])
            (d / f["url"].rsplit("/", 1)[1]).write_bytes((CORPUS / blob["blob_path"]).read_bytes())
    return dict(src, end=end,
                url_template={"strftime": f"file://{d}/geosfp-hourly.%Y%m%d_%H%M.nc"})


def test_prefetch_fetch_ahead_downloads_without_decoding_and_drops_errors(tmp_path):
    from earthsciio import FetchError, cache_key

    src = _file_source(tmp_path)
    online = Cache(root=tmp_path / "cache")
    member = _provider(src, 1, online)
    urls = {k: member._sample_urls(_utc(src["refresh_times_s"][k]))[0] for k in (0, 4, 8, 12)}
    decoded = []
    real = member._reader.read_native

    def recording(handle, *a, **kw):
        decoded.append(str(handle))
        return real(handle, *a, **kw)

    member._reader.read_native = recording
    # depth 1 and a slow head decode: positions 1-2 (epochs 4 and 8, two other
    # files) are past the decode window, inside fetch_ahead=3; position 3
    # (epoch 12) names a file that does not exist.
    with PrefetchGroup({"Q": member}, depth=1, fetch_ahead=3, workers=1,
                       _decode_latency=lambda: 0.3) as g:
        g.start([0, 4, 8, 12])
        for k in (4, 8):
            assert _wait_until(lambda: online.store.get_blob(cache_key(urls[k])) is not None)
        assert not decoded  # warmed while the head decode is still sleeping
        assert g.take(0)["Q"].anchor == _utc(src["refresh_times_s"][0])
        assert g.take(4) and g.take(8)
        with pytest.raises(FetchError):  # the warm's failure was dropped; take raises
            g.take(12)
    assert online.store.get_blob(cache_key(urls[12])) is None


# --------------------------------------------------------------------------- #
# §3.10 one process-wide NetCDF decode lock.
# --------------------------------------------------------------------------- #


def test_netcdf_decodes_take_the_process_wide_lock(cache):
    from earthsciio import NetCDFReader
    from earthsciio.readers import _NETCDF_LOCK

    path = cache.fetch("https://data.earthsci.dev/geosfp/mini/GEOSFP.20240101.A3mixed.nc").path
    reader = NetCDFReader()
    for call in (lambda: reader.read_native(path), lambda: reader.dim_length(path, "lat")):
        done = threading.Event()
        worker = threading.Thread(target=lambda: (call(), done.set()))
        with _NETCDF_LOCK:
            worker.start()
            assert not done.wait(0.2), "decoded while another thread held the lock"
        assert done.wait(5.0)
        worker.join()


# --------------------------------------------------------------------------- #
# Regressions: no take() may hang, and the tracks agree.
# --------------------------------------------------------------------------- #


def test_prefetch_fetch_ahead_url_errors_are_dropped(tmp_path):
    """A url callable that raises during the warm-up skips that warm: it never
    reaches start/take and never strands an admitted decode."""
    src = _file_source(tmp_path, end="2024-01-01T12:00:00Z")
    member = _provider(src, 1, Cache(root=tmp_path / "cache"))

    def boom(t):
        raise KeyError("no url for this anchor")

    member._sample_urls = boom  # the warm path only; decodes resolve through the loader
    ref = _provider(src, 1, Cache(root=tmp_path / "cache"))
    with PrefetchGroup({"Q": member}, depth=1, fetch_ahead=3, workers=1) as g:
        B = g.boundaries()
        g.start("forward")
        for k in range(len(B)):
            assert_bitwise(g.take(k)["Q"].data, ref.refresh(B[k]), f"epoch {k}")


def test_prefetch_a_worker_exception_reaches_take(cache, equivalence):
    """Whatever escapes a thread worker — even a BaseException from the test
    latency hook — is stored and raised by take, never left to hang it."""
    class Stop(BaseException):
        pass

    def hook():
        raise Stop("hook")

    with PrefetchGroup(_members(equivalence, cache), workers=2, _decode_latency=hook) as g:
        g.start("forward")
        with pytest.raises(Stop):
            g.take(0)


def test_prefetch_transform_results_are_read_only(cache, equivalence):
    def double(key, ds):
        from earthsciio import NativeDataset, NativeField
        return NativeDataset(
            {n: NativeField(np.asarray(f.data) * 2, f.dims, f.attrs) for n, f in ds.variables.items()},
            dict(ds.coords),
        )

    with PrefetchGroup(_members(equivalence, cache), transform=double) as g:
        g.start("forward")
        ep = g.take(0)
    with pytest.raises(ValueError, match="read-only"):
        ep["Q1"].data.variables["Q"].data[...] = 0.0


def test_prefetch_ready_reports_only_the_next_entry(cache, equivalence):
    with PrefetchGroup(_members(equivalence, cache), depth=3, workers=2) as g:
        g.start("forward")
        assert _wait_until(lambda: g._slot_done(1))
        assert g.ready(1) is False  # decoded, but take(1) would raise: 0 is next
        assert _wait_until(lambda: g.ready(0))


def test_window_union_refuses_a_time_subset_it_read_whole():
    """Window B leaves `time` unnamed, so the union reads it whole; window A's
    subset of it is still refused, as A's own read would refuse it."""
    from earthsciio import NativeDataset, NativeField
    from earthsciio.selection import _WindowUnion

    u = _WindowUnion([DimWindow({"time": [0]}), DimWindow({"lat": [1]})])
    assert u.select["dims"]["time"] == "all"
    ds = NativeDataset(
        {"T": NativeField(np.zeros((2, 3)), ("time", "lat"), {})},
        {"time": NativeField(np.arange(2.0), ("time",), {"units": "hours since 2024-01-01"})},
    )
    with pytest.raises(ValueError, match="subset of the time dimension"):
        u.split(ds)
