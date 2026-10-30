#!/usr/bin/env python3
"""Measure how much of a synchronous ``refresh`` loop a :class:`PrefetchGroup`
hides behind the consumer's compute, and what it costs in peak RSS.

Why this exists
---------------
A solver driving DISCRETE forcing stops at every refresh boundary and calls
:meth:`earthsciio.Provider.refresh`, which decodes while the solver waits.
``spec/prefetch.md`` §3 moves that decode onto background workers: a
:class:`earthsciio.PrefetchGroup` decodes the next epochs while the consumer
integrates the current one, and ``take(k)`` returns bitwise what
``refresh(B[k])`` would have. This script answers the two questions that
promise raises: how much refresh time actually disappears, and how much memory
the epochs held ahead cost.

Method
------
Synthesize a multi-file DISCRETE NetCDF source on local disk — ``--files``
files of ``--records`` hourly records, three zlib-compressed float32 3-D
variables (``lev x lat x lon``), so a file decode is real decompression work —
and warm it into a fresh content-addressed cache. Two members read it, the way
an integrator would: ``UV`` (``records_per_sample = 1``, ``U`` and ``V``) and
``T`` (``records_per_sample = 2``, the interpolation bracket).

* **sync**: for every epoch ``B[k]``, ``refresh(B[k])`` on both members (timed),
  then the synthetic per-epoch compute;
* **prefetch**: ``PrefetchGroup(...).start("forward")``, then for every epoch
  ``take(k)`` (blocked time = ``stats()["waited_s"]``), then the same compute.

The **hidden-refresh fraction** is ``1 - waited_s / sync_refresh_s``: 1.0 means
every second the synchronous loop spent decoding became overlap; 0 means none
did. The first ``take`` always waits for the first decode — nothing can be ahead
of it, and it also pays one-off costs (the first ``open_dataset``; with
``executor="process"``, spawning the workers and importing the decode stack in
them) — so the **steady** fraction repeats the ratio without epoch 0. The decode
is lumpy: a whole file is decoded on its FIRST epoch and the rest are slices of
it, so a file-opening epoch costs about ``records x`` the mean, and only a
compute that covers it within ``depth`` epochs hides it fully. Hence the sweep
over ``--compute-ratio`` (a multiple of the MEAN refresh per epoch).

The compute is ``--compute-kind``: ``sleep`` (a solver in C that releases the
GIL — the overlap a thread executor can win), ``python`` (a GIL-holding busy
loop — what a thread executor cannot hide well, and ``--executor process`` is
for) or ``numpy`` (BLAS, which releases the GIL). Its length defaults to
``--compute-ratio`` times the calibrated mean synchronous refresh per epoch, so
compute exceeds decode and the decode CAN be hidden.

Every case runs in a FRESH subprocess, so peak RSS (``getrusage`` max RSS — bytes
on macOS, KiB on Linux, normalised here) belongs to that case alone;
``rss_base`` is the high-water mark after the providers are built and before
the loop, ``rss_peak`` the mark after it. For ``executor="process"`` that is the
CONSUMER process only: the decode workers' own memory (their two decoded files
per member each, the decode stack) is not in it.

Everything is offline and local (``file://`` source ingested into a temp cache).

Usage::

    python bench/prefetch_overlap.py                     # the default (small) sweep
    python bench/prefetch_overlap.py --size medium
    python bench/prefetch_overlap.py --compute-kind python
    python bench/prefetch_overlap.py --case JSON         # (internal) run one case
"""

from __future__ import annotations

import argparse
import datetime as dt
import json
import os
import pathlib
import resource
import shutil
import subprocess
import sys
import tempfile
import time
from typing import Any, Dict, List

import numpy as np

_HERE = pathlib.Path(__file__).resolve().parent
_ROOT = _HERE.parent
sys.path.insert(0, str(_ROOT))

MB = 1024.0 * 1024.0
START = dt.datetime(2024, 1, 1, tzinfo=dt.timezone.utc)
HOUR = dt.timedelta(hours=1)

SIZES = {
    # files, records per file, lev, lat, lon
    "small": dict(files=4, records=6, lev=12, lat=91, lon=144),
    "medium": dict(files=6, records=8, lev=36, lat=181, lon=288),
}


# --------------------------------------------------------------------------- #
# Source synthesis
# --------------------------------------------------------------------------- #


def build_source(src: pathlib.Path, files: int, records: int, lev: int, lat: int, lon: int) -> str:
    """Write the NetCDF files; return the strftime ``file://`` URL template."""
    import netCDF4

    src.mkdir(parents=True, exist_ok=True)
    period = records * HOUR
    z, y, x = np.meshgrid(np.arange(lev), np.arange(lat), np.arange(lon), indexing="ij")
    base = (np.sin(x / 7.0) + np.cos(y / 5.0) + z / 10.0).astype("f4")
    for f in range(files):
        anchor = START + f * period
        path = src / anchor.strftime("bench.%Y%m%d_%H%M.nc")
        with netCDF4.Dataset(str(path), "w") as ds:
            ds.createDimension("time", records)
            ds.createDimension("lev", lev)
            ds.createDimension("lat", lat)
            ds.createDimension("lon", lon)
            tv = ds.createVariable("time", "i4", ("time",))
            tv.units = "seconds since 1970-01-01 00:00:00"
            tv[:] = [int((anchor + r * HOUR).timestamp()) for r in range(records)]
            ds.createVariable("lev", "f8", ("lev",))[:] = np.arange(lev, dtype="f8")
            ds.createVariable("lat", "f8", ("lat",))[:] = np.linspace(-90, 90, lat)
            ds.createVariable("lon", "f8", ("lon",))[:] = np.linspace(0, 360, lon, endpoint=False)
            for i, name in enumerate(("U", "V", "T")):
                v = ds.createVariable(
                    name,
                    "f4",
                    ("time", "lev", "lat", "lon"),
                    zlib=True,
                    complevel=1,
                    chunksizes=(1, lev, lat, lon),
                )
                for r in range(records):
                    v[r] = base * (1 + i) + (f * records + r)
    return "file://" + str(src / "bench.%Y%m%d_%H%M.nc")


def _providers(url: str, cache_root: str, files: int, records: int):
    from earthsciio import Cache, DataSource, Provider, SourceTemporal

    cache = Cache(root=cache_root)
    end = START + files * records * HOUR

    def member(name, variables, rps):
        temporal = SourceTemporal(
            START, HOUR, records * HOUR, end=end, time_dim="time", records_per_sample=rps
        )
        return Provider(
            DataSource(name, "netcdf", url, variables=variables, temporal=temporal), cache
        )

    return {"UV": member("UV", ["U", "V"], 1), "T": member("T", ["T"], 2)}


# --------------------------------------------------------------------------- #
# Synthetic compute
# --------------------------------------------------------------------------- #


def compute(kind: str, seconds: float) -> None:
    end = time.perf_counter() + seconds
    if kind == "sleep":
        time.sleep(seconds)
    elif kind == "python":
        x = 0
        while time.perf_counter() < end:
            for _ in range(1000):
                x += 1
    elif kind == "numpy":
        a = np.random.default_rng(0).random((256, 256))
        while time.perf_counter() < end:
            a = np.tanh(a @ a.T / 256.0)
    else:
        raise ValueError(kind)


# --------------------------------------------------------------------------- #
# One case (runs in its own subprocess)
# --------------------------------------------------------------------------- #


def _maxrss_bytes() -> int:
    r = resource.getrusage(resource.RUSAGE_SELF).ru_maxrss
    return int(r if sys.platform == "darwin" else r * 1024)


def run_case(spec: Dict[str, Any]) -> Dict[str, Any]:
    from earthsciio import PrefetchGroup

    members = _providers(spec["url"], spec["cache_root"], spec["files"], spec["records"])
    B = members["UV"].refresh_times()
    # Warm the decode stack (imports, xarray's backend discovery, the first
    # open_dataset) on a throwaway read: a one-off process cost, not refresh
    # time, and it would otherwise land on epoch 0 of whichever mode runs. The
    # providers' own buffers are untouched. Process workers still pay it
    # themselves — that IS executor="process"'s start-up cost, and stays in.
    from earthsciio import NetCDFReader

    NetCDFReader().read_native(B[0].strftime(spec["url"][len("file://") :]), ["U"])

    rss_base = _maxrss_bytes()
    out: Dict[str, Any] = dict(spec, epochs=len(B), rss_base=rss_base)
    t_wall = time.perf_counter()
    per_epoch: List[float] = []
    if spec["mode"] == "sync":
        for t in B:
            t0 = time.perf_counter()
            for p in members.values():
                p.refresh(t)
            per_epoch.append(time.perf_counter() - t0)
            compute(spec["compute_kind"], spec["compute_s"])
        out["refresh_s"] = sum(per_epoch)
    else:
        with PrefetchGroup(
            members, depth=spec["depth"], workers=spec["workers"], executor=spec["executor"]
        ) as g:
            g.start("forward")
            for k in range(len(B)):
                before = g.stats()["waited_s"]
                g.take(k)
                per_epoch.append(g.stats()["waited_s"] - before)
                compute(spec["compute_kind"], spec["compute_s"])
            out.update(g.stats())
    out["per_epoch_s"] = per_epoch
    out["wall_s"] = time.perf_counter() - t_wall
    out["rss_peak"] = _maxrss_bytes()
    return out


def _spawn(spec: Dict[str, Any]) -> Dict[str, Any]:
    proc = subprocess.run(
        [sys.executable, str(pathlib.Path(__file__).resolve()), "--case", json.dumps(spec)],
        capture_output=True,
        text=True,
    )
    if proc.returncode != 0:
        raise RuntimeError(f"case {spec['mode']} failed:\n{proc.stderr}")
    return json.loads(proc.stdout.strip().splitlines()[-1])


# --------------------------------------------------------------------------- #
# Driver
# --------------------------------------------------------------------------- #


def _hidden(blocked: float, sync: float) -> float:
    return 1.0 - blocked / sync if sync > 0 else float("nan")


def _fmt(row: Dict[str, Any], sync: Dict[str, Any]) -> str:
    rss = (
        f"rss_peak={row['rss_peak'] / MB:6.1f} MB "
        f"(+{(row['rss_peak'] - row['rss_base']) / MB:5.1f})"
    )
    if row["mode"] == "sync":
        return (
            f"{'sync refresh loop':30s} blocked={row['refresh_s']:6.3f} s "
            f"(epoch 0: {row['per_epoch_s'][0]:5.3f})                         "
            f"wall={row['wall_s']:6.3f} s  {rss}"
        )
    steady = _hidden(sum(row["per_epoch_s"][1:]), sum(sync["per_epoch_s"][1:]))
    label = f"{row['executor']} depth={row['depth']} workers={row['workers']}"
    return (
        f"{label:30s} blocked={row['waited_s']:6.3f} s (epoch 0: {row['per_epoch_s'][0]:5.3f})  "
        f"hidden={_hidden(row['waited_s'], sync['refresh_s']):6.1%} "
        f"steady={steady:6.1%}  wall={row['wall_s']:6.3f} s  {rss}  "
        f"slots<={row['peak_bytes'] / MB:4.1f} MB"
    )


def main(argv: List[str]) -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--case")
    ap.add_argument("--size", choices=sorted(SIZES), default="small")
    ap.add_argument("--compute-kind", choices=["sleep", "python", "numpy"], default="sleep")
    ap.add_argument(
        "--compute-ms",
        type=float,
        default=None,
        help="per-epoch compute; default: --compute-ratio x calibrated refresh",
    )
    ap.add_argument("--compute-ratio", type=float, nargs="+", default=[1.5, 4.0])
    ap.add_argument(
        "--process", action="store_true", help="also run executor='process' (slow to start: spawn)"
    )
    ap.add_argument("--keep", help="build the source/cache here and keep it")
    args = ap.parse_args(argv[1:])

    if args.case:
        print(json.dumps(run_case(json.loads(args.case))))
        return 0

    size = SIZES[args.size]
    work = pathlib.Path(args.keep or tempfile.mkdtemp(prefix="esio-prefetch-bench-"))
    try:
        url = build_source(work / "src", **size)
        cache_root = str(work / "cache")
        for p in _providers(url, cache_root, size["files"], size["records"]).values():
            p.prefetch()  # ingest once: every case decodes from a warm disk cache
        disk = sum(f.stat().st_size for f in (work / "src").iterdir())
        base = dict(
            url=url,
            cache_root=cache_root,
            files=size["files"],
            records=size["records"],
            compute_kind=args.compute_kind,
        )

        print("EarthSciIO Python PrefetchGroup — refresh hidden behind compute, and its RSS cost")
        import netCDF4
        import xarray

        print(
            f"python={sys.version.split()[0]} numpy={np.__version__} "
            f"xarray={xarray.__version__} netCDF4={netCDF4.__version__} cpus={os.cpu_count()}"
        )
        n_ep = size["files"] * size["records"]
        print(
            f"source: {size['files']} files x {size['records']} hourly records, U/V/T float32 "
            f"[{size['lev']}x{size['lat']}x{size['lon']}] zlib; {disk / MB:.1f} MB on disk; "
            f"{n_ep} epochs; members UV (rps 1) + T (rps 2)"
        )

        calib = _spawn(dict(base, mode="sync", compute_s=0.0))
        mean = calib["refresh_s"] / calib["epochs"]
        print(
            f"calibration (no compute): refresh {calib['refresh_s']:.3f} s = "
            f"{mean * 1e3:.1f} ms/epoch mean, {max(calib['per_epoch_s']) * 1e3:.1f} ms max "
            f"(a file-opening epoch)"
        )
        levels = (
            [(args.compute_ms / 1e3, "--compute-ms")]
            if args.compute_ms is not None
            else [(r * mean, f"{r:g} x mean refresh") for r in args.compute_ratio]
        )
        configs = [("thread", 1, 1), ("thread", 2, 1), ("thread", 2, 2), ("thread", 3, 2)]
        if args.process:
            configs += [("process", 2, 2)]
        for compute_s, how in levels:
            print(f"\ncompute: {args.compute_kind}, {compute_s * 1e3:.1f} ms/epoch ({how})")
            spec = dict(base, compute_s=compute_s)
            sync = _spawn(dict(spec, mode="sync"))
            print("  " + _fmt(sync, sync))
            rows = []
            for executor, depth, workers in configs:
                row = _spawn(
                    dict(spec, mode="prefetch", executor=executor, depth=depth, workers=workers)
                )
                rows.append(row)
                print("  " + _fmt(row, sync))
            best = min(rows, key=lambda r: r["waited_s"])
            print(
                f"  best: {best['executor']} depth={best['depth']} workers={best['workers']} "
                f"hid {_hidden(best['waited_s'], sync['refresh_s']):.1%} of the sync refresh "
                f"time ({sync['refresh_s']:.3f} s -> {best['waited_s']:.3f} s blocked; "
                f"{_hidden(sum(best['per_epoch_s'][1:]), sum(sync['per_epoch_s'][1:])):.1%} "
                f"past epoch 0) for {(best['rss_peak'] - sync['rss_peak']) / MB:+.1f} MB peak "
                "RSS vs the sync loop"
            )
    finally:
        if not args.keep:
            shutil.rmtree(work, ignore_errors=True)
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
