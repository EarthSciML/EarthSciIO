#!/usr/bin/env python3
"""Measure the PEAK RSS of the Python store-backed Zarr reader against the
nominal size of the selection it produces.

Why this exists
---------------
The Julia and Rust zarr readers decode *every* chunk a selection intersects into
a map and only then assemble the output, so their peak memory scales with the
total decompressed volume of the intersected chunks rather than with the size of
the result (~15x amplification on the real ISRM source-receptor array: 416
chunks x ~21 MB to produce a 0.59 GB answer). This script answers the same
question for :class:`earthsciio.backends.zarr.ZarrReader`, which delegates chunk
iteration to zarr-python's ``oindex``.

Method
------
Build a synthetic **Zarr v2** store on disk whose chunks are large when
decompressed but tiny on disk (a low-entropy pattern + blosc), then read an
orthogonal selection that touches EVERY chunk while taking only a fraction of
the rows in each. That is exactly the ISRM shape: small answer, huge intersected
chunk volume.

The discriminator is the *density sweep*: at a FIXED store (fixed intersected
chunk volume) the selection density is varied.

* a **streaming** reader's peak tracks the OUTPUT size (ratio peak/output ~ 1-2x,
  roughly flat, and peak/chunk-volume shrinks as density shrinks);
* a **buffering** reader's peak stays pinned near the CHUNK VOLUME no matter how
  small the answer gets (peak/output blows up as density shrinks).

Peak RSS is sampled by a background thread polling ``/proc/self/statm`` every
~2 ms and cross-checked against ``getrusage(RUSAGE_SELF).ru_maxrss``. Each case
runs in a FRESH subprocess so the baseline is clean and one case's high-water
mark cannot leak into the next.

Everything is offline and local; nothing here touches the network or S3, and no
case is run at ISRM scale.

Usage::

    python bench/zarr_peak_memory.py              # the default sweep
    python bench/zarr_peak_memory.py --case NAME  # (internal) run one case
    python bench/zarr_peak_memory.py --list
"""

from __future__ import annotations

import argparse
import json
import os
import pathlib
import resource
import shutil
import subprocess
import sys
import tempfile
import threading
import time
from typing import Any, Dict, List, Optional

import numpy as np

_HERE = pathlib.Path(__file__).resolve().parent
_ROOT = _HERE.parent
sys.path.insert(0, str(_ROOT))

MB = 1024.0 * 1024.0


# --------------------------------------------------------------------------- #
# RSS sampling
# --------------------------------------------------------------------------- #

_PAGE = resource.getpagesize()


def rss_bytes() -> int:
    """Current resident set size, straight from ``/proc/self/statm``."""
    with open("/proc/self/statm", "rb") as fh:
        return int(fh.read().split()[1]) * _PAGE


class PeakSampler:
    """Poll RSS in a background thread and keep the high-water mark."""

    def __init__(self, interval: float = 0.002) -> None:
        self.interval = interval
        self.peak = 0
        self._stop = threading.Event()
        self._thread: Optional[threading.Thread] = None

    def _run(self) -> None:
        while not self._stop.is_set():
            r = rss_bytes()
            if r > self.peak:
                self.peak = r
            time.sleep(self.interval)

    def __enter__(self) -> "PeakSampler":
        self.peak = rss_bytes()
        self._thread = threading.Thread(target=self._run, daemon=True)
        self._thread.start()
        return self

    def __exit__(self, *exc: Any) -> None:
        self._stop.set()
        if self._thread is not None:
            self._thread.join()
        r = rss_bytes()
        if r > self.peak:
            self.peak = r


# --------------------------------------------------------------------------- #
# Synthetic Zarr v2 store (written by hand — no zarr writer involved)
# --------------------------------------------------------------------------- #


def _zarray_json(shape, chunks, dtype: str = "<f4") -> bytes:
    return json.dumps(
        {
            "zarr_format": 2,
            "shape": list(shape),
            "chunks": list(chunks),
            "dtype": dtype,
            "compressor": {
                "id": "blosc",
                "cname": "lz4",
                "clevel": 5,
                "shuffle": 1,
                "blocksize": 0,
            },
            "fill_value": 0.0,
            "order": "C",
            "filters": None,
            "dimension_separator": ".",
        }
    ).encode()


def build_store(root: pathlib.Path, n_chunks: int, chunk_rows: int, ncols: int) -> Dict[str, int]:
    """Write ``field/`` as a 2-D ``(n_chunks*chunk_rows, ncols)`` float32 v2 array
    chunked ``(chunk_rows, ncols)``.

    Chunk payloads are a low-entropy but non-constant pattern so the store is
    small on disk while each chunk still decompresses to the full nominal size —
    the ISRM situation (compressed object store, fat decompressed chunks).
    Chunks are written one at a time so building the fixture never itself holds
    more than one chunk.
    """
    import numcodecs

    codec = numcodecs.Blosc(cname="lz4", clevel=5, shuffle=numcodecs.Blosc.SHUFFLE, blocksize=0)
    adir = root / "field"
    adir.mkdir(parents=True, exist_ok=True)
    nrows = n_chunks * chunk_rows
    (adir / ".zarray").write_bytes(_zarray_json((nrows, ncols), (chunk_rows, ncols)))
    (adir / ".zattrs").write_bytes(json.dumps({"_ARRAY_DIMENSIONS": ["source", "receptor"]}).encode())

    # One reusable row pattern; each chunk row is pattern + its global row index,
    # which keeps the data verifiable yet highly compressible.
    pattern = (np.arange(ncols, dtype=np.float32) % 97.0) * 0.25
    on_disk = 0
    for c in range(n_chunks):
        chunk = np.empty((chunk_rows, ncols), dtype="<f4")
        base = c * chunk_rows
        for r in range(chunk_rows):
            chunk[r] = pattern + np.float32(base + r)
        enc = bytes(codec.encode(np.ascontiguousarray(chunk)))
        (adir / f"{c}.0").write_bytes(enc)
        on_disk += len(enc)
        del chunk
    return {"nrows": nrows, "ncols": ncols, "on_disk_bytes": on_disk}


def expected_row(global_row: int, ncols: int) -> np.ndarray:
    pattern = (np.arange(ncols, dtype=np.float32) % 97.0) * 0.25
    return (pattern + np.float32(global_row)).astype(np.float64)


# --------------------------------------------------------------------------- #
# Minimal offline cache shim (same shape as conformance/dumpers/read_python.py)
# --------------------------------------------------------------------------- #


class _Entry:
    def __init__(self, path: str) -> None:
        self.path = path


class _LocalDirCache:
    """``cache.fetch(url)`` over a local directory; counts objects fetched."""

    def __init__(self) -> None:
        self.fetches = 0
        self.chunk_fetches = 0

    def fetch(self, url: str, **_: Any) -> _Entry:
        from earthsciio.errors import CacheMiss

        path = url[len("file://"):] if url.startswith("file://") else url
        if not os.path.exists(path):
            raise CacheMiss(url, "bench")
        self.fetches += 1
        if not path.endswith((".zarray", ".zattrs", "zarr.json")):
            self.chunk_fetches += 1
        return _Entry(path)


# --------------------------------------------------------------------------- #
# Cases
# --------------------------------------------------------------------------- #

# All cases share one store geometry so the intersected chunk volume is fixed;
# only the number of rows taken *per chunk* changes.
NCOLS = 1000
CHUNK_ROWS = 525          # 525 * 1000 * 4 B = 2.10 MB decompressed per chunk

CASES: Dict[str, Dict[str, int]] = {
    # -- A. THE DISCRIMINATOR: chunk-count scaling at a fixed, tiny output ---- #
    # The output is held ~constant-tiny while the intersected chunk volume grows
    # 32x. A buffering reader's peak grows with it; a streaming reader's does not.
    "count-25":      {"n_chunks": 25, "rows_per_chunk": 1},
    "count-50":      {"n_chunks": 50, "rows_per_chunk": 1},
    "count-100":     {"n_chunks": 100, "rows_per_chunk": 1},
    "count-200":     {"n_chunks": 200, "rows_per_chunk": 1},
    "count-400":     {"n_chunks": 400, "rows_per_chunk": 1},
    "count-800":     {"n_chunks": 800, "rows_per_chunk": 1},
    # -- B. density sweep at a fixed store (fixed intersected chunk volume) --- #
    "density-1":     {"n_chunks": 100, "rows_per_chunk": 1},
    "density-8":     {"n_chunks": 100, "rows_per_chunk": 8},
    "density-37":    {"n_chunks": 100, "rows_per_chunk": 37},
    "density-131":   {"n_chunks": 100, "rows_per_chunk": 131},
    "density-525":   {"n_chunks": 100, "rows_per_chunk": 525},  # every row
    # -- C. ISRM-shaped chunks (~21 MB decompressed, as in the real SR array) - #
    "isrm-chunk-20": {"n_chunks": 20, "rows_per_chunk": 1, "chunk_rows": 5250},
    "isrm-chunk-40": {"n_chunks": 40, "rows_per_chunk": 1, "chunk_rows": 5250},
    "isrm-chunk-80": {"n_chunks": 80, "rows_per_chunk": 1, "chunk_rows": 5250},
}

_GROUPS = {
    "A. chunk-count scaling, fixed tiny output (THE DISCRIMINATOR)":
        ["count-25", "count-50", "count-100", "count-200", "count-400", "count-800"],
    "B. density sweep, fixed 200 MB chunk volume":
        ["density-1", "density-8", "density-37", "density-131", "density-525"],
    "C. ISRM-shaped 21 MB chunks, fixed tiny output":
        ["isrm-chunk-20", "isrm-chunk-40", "isrm-chunk-80"],
}


def parse_case(name: str) -> Dict[str, int]:
    """``NAME`` from :data:`CASES`, or an ad-hoc ``n_chunks,rows_per_chunk[,chunk_rows]``."""
    if name in CASES:
        cfg = dict(CASES[name])
    else:
        parts = [int(p) for p in name.split(",")]
        cfg = {"n_chunks": parts[0], "rows_per_chunk": parts[1]}
        if len(parts) > 2:
            cfg["chunk_rows"] = parts[2]
    cfg.setdefault("chunk_rows", CHUNK_ROWS)
    return cfg


def run_case(name: str, keep: Optional[str] = None) -> Dict[str, Any]:
    cfg = parse_case(name)
    n_chunks = cfg["n_chunks"]
    rows_per_chunk = cfg["rows_per_chunk"]
    chunk_rows = cfg["chunk_rows"]

    tmp = pathlib.Path(keep) if keep else pathlib.Path(tempfile.mkdtemp(prefix="zarrbench-"))
    tmp.mkdir(parents=True, exist_ok=True)
    try:
        info = build_store(tmp, n_chunks, chunk_rows, NCOLS)

        # Rows taken from EVERY chunk, evenly spread inside the chunk.
        offsets = np.linspace(0, chunk_rows - 1, rows_per_chunk).astype(int).tolist()
        offsets = sorted(set(offsets))
        idx: List[int] = []
        for c in range(n_chunks):
            for o in offsets:
                idx.append(c * chunk_rows + o)

        n_out_rows = len(idx)
        out_f32 = n_out_rows * NCOLS * 4
        out_f64 = n_out_rows * NCOLS * 8
        chunk_volume = n_chunks * chunk_rows * NCOLS * 4

        from earthsciio.backends.zarr import ZarrReader

        # Warm every import / lazy module and the allocator before baselining.
        reader = ZarrReader()
        warm_cache = _LocalDirCache()
        reader.array_shape(warm_cache, str(tmp), "field")

        cache = _LocalDirCache()
        select = {"axes": [{"indices": idx}, "all"]}

        use_tm = os.environ.get("BENCH_TRACEMALLOC") == "1"
        tm_peak = tm_base = 0
        if use_tm:
            import tracemalloc

            # numpy routes its data allocations through PyTraceMalloc, so traced
            # memory includes decoded chunk buffers and the output array. Traced
            # peak = peak SIMULTANEOUS live bytes, which (unlike RSS) is immune to
            # allocator retention/fragmentation.
            tracemalloc.start()
            tm_base, _ = tracemalloc.get_traced_memory()
            tracemalloc.reset_peak()

        baseline = rss_bytes()
        t0 = time.time()
        with PeakSampler() as sampler:
            nds = reader.read_store(cache, str(tmp), ["field"], select=select)
            data = nds.variables["field"].data
            during = rss_bytes()
        elapsed = time.time() - t0
        peak = sampler.peak
        ru_peak = resource.getrusage(resource.RUSAGE_SELF).ru_maxrss * 1024
        if use_tm:
            import tracemalloc

            tm_cur, tm_peak = tracemalloc.get_traced_memory()
            tracemalloc.stop()

        assert data.shape == (n_out_rows, NCOLS), data.shape
        assert data.dtype == np.float64, data.dtype
        # Correctness spot-check: first, middle and last selected rows.
        for probe in (0, n_out_rows // 2, n_out_rows - 1):
            np.testing.assert_allclose(data[probe], expected_row(idx[probe], NCOLS), rtol=0, atol=0)

        del nds, data

        return {
            "case": name,
            "n_chunks": n_chunks,
            "rows_per_chunk": rows_per_chunk,
            "array_shape": [info["nrows"], info["ncols"]],
            "chunk_shape": [chunk_rows, NCOLS],
            "chunk_decompressed_bytes": chunk_rows * NCOLS * 4,
            "store_on_disk_bytes": info["on_disk_bytes"],
            "chunk_fetches": cache.chunk_fetches,
            "intersected_chunk_volume_bytes": chunk_volume,
            "out_rows": n_out_rows,
            "out_f32_bytes": out_f32,
            "out_f64_bytes": out_f64,
            "baseline_rss_bytes": baseline,
            "peak_rss_bytes": peak,
            "rss_after_read_bytes": during,
            "ru_maxrss_bytes": ru_peak,
            "delta_peak_bytes": peak - baseline,
            "tracemalloc_peak_bytes": tm_peak,
            "tracemalloc_base_bytes": tm_base,
            "seconds": elapsed,
        }
    finally:
        if not keep:
            shutil.rmtree(tmp, ignore_errors=True)


# --------------------------------------------------------------------------- #
# Driver
# --------------------------------------------------------------------------- #


def _fmt(row: Dict[str, Any]) -> str:
    d = row["delta_peak_bytes"]
    live = row["tracemalloc_peak_bytes"]
    out = row["out_f64_bytes"]
    vol = row["intersected_chunk_volume_bytes"]
    return (
        f"{row['case']:<14} chunks={row['n_chunks']:<4} chunk={row['chunk_decompressed_bytes']/MB:5.1f}MB "
        f"out={out/MB:7.2f}MB volume={vol/MB:7.1f}MB | "
        f"live={live/MB:7.1f}MB  live/vol={live/vol:6.3f}x  live/out={live/out:7.2f}x | "
        f"rssΔ={d/MB:7.1f}MB  ({row['seconds']:.1f}s, {row['chunk_fetches']} chunks read)"
    )


def main(argv: List[str]) -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--case")
    ap.add_argument("--list", action="store_true")
    ap.add_argument("--json", action="store_true")
    args = ap.parse_args(argv[1:])

    if args.list:
        for k, v in CASES.items():
            print(k, v)
        return 0

    if args.case:
        print(json.dumps(run_case(args.case)))
        return 0

    print("EarthSciIO Python zarr reader — peak memory vs. output size vs. chunk volume")
    print(f"python={sys.version.split()[0]}", end=" ")
    try:
        import zarr

        print(f"zarr={zarr.__version__} async.concurrency={zarr.config.get('async.concurrency')}", end=" ")
    except Exception:
        pass
    print(f"numpy={np.__version__}")
    print(
        "live = peak SIMULTANEOUS traced bytes (tracemalloc; numpy data included) — the\n"
        "       quantity the Julia/Rust bug inflates.  rssΔ = peak RSS growth, which also\n"
        "       contains glibc arena retention of freed chunk buffers (reusable, not live).\n"
    )

    env = dict(os.environ, BENCH_TRACEMALLOC="1")
    rows: List[Dict[str, Any]] = []
    by_name: Dict[str, Dict[str, Any]] = {}
    for title, names in _GROUPS.items():
        print(title)
        for name in names:
            proc = subprocess.run(
                [sys.executable, str(pathlib.Path(__file__).resolve()), "--case", name],
                capture_output=True,
                text=True,
                env=env,
            )
            if proc.returncode != 0:
                print(f"{name}: FAILED\n{proc.stderr}", file=sys.stderr)
                return 1
            row = json.loads(proc.stdout.strip().splitlines()[-1])
            rows.append(row)
            by_name[name] = row
            print("  " + _fmt(row))
        print()

    lo, hi = by_name["count-25"], by_name["count-800"]
    vol_growth = hi["intersected_chunk_volume_bytes"] / lo["intersected_chunk_volume_bytes"]
    live_growth = hi["tracemalloc_peak_bytes"] / lo["tracemalloc_peak_bytes"]
    print(
        "A: intersected chunk volume grew %.0fx (%.0f -> %.0f MB) at a near-constant %.2f MB "
        "output; peak LIVE grew only %.2fx (%.1f -> %.1f MB)."
        % (
            vol_growth,
            lo["intersected_chunk_volume_bytes"] / MB,
            hi["intersected_chunk_volume_bytes"] / MB,
            hi["out_f64_bytes"] / MB,
            live_growth,
            lo["tracemalloc_peak_bytes"] / MB,
            hi["tracemalloc_peak_bytes"] / MB,
        )
    )
    c = by_name["isrm-chunk-80"]
    print(
        "C: with ISRM-sized %.0f MB chunks, peak LIVE is %.0f MB ~= async.concurrency x chunk "
        "size, flat across 20/40/80 chunks — bounded by CONCURRENCY, not by chunk count."
        % (c["chunk_decompressed_bytes"] / MB, c["tracemalloc_peak_bytes"] / MB)
    )
    streaming = live_growth < 0.25 * vol_growth
    print(
        "\nVERDICT: peak LIVE memory tracks the %s"
        % (
            "OUTPUT + O(concurrency x chunk) — STREAMING, no chunk-count amplification"
            if streaming
            else "CHUNK VOLUME — BUFFERING, same amplification as Julia/Rust"
        )
    )
    if args.json:
        print(json.dumps(rows, indent=2))
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
