"""Concurrency — the per-blob lock + atomic rename keep a shared cache intact.

Two levels, mirroring the Julia/Rust siblings:

* **threads** sharing one :class:`Cache` (intra-process), and
* **separate processes** sharing one ``$EARTHSCIDATADIR`` — the real
  ``/scratch.local`` contract (a Julia and a Python process racing the same URL
  must result in exactly one download, no corruption).

A counting transport sleeps mid-fetch to widen the race and records each *real*
download as one byte appended to a marker file (atomic ``O_APPEND``), so the test
asserts "exactly one download" across processes without shared memory.
"""

from __future__ import annotations

import multiprocessing
import os
import time
from concurrent.futures import ThreadPoolExecutor
from urllib.parse import parse_qs, urlsplit

from earthsciio import Cache, sha256_file, transport_registry
from earthsciio.transport import DOWNLOADED, FetchResult

PAYLOAD = b"shared-cache-payload-" * 64
DELAY = 0.15  # held while "downloading" — widens the lock-contention window


class CountingFileTransport:
    """A ``count://`` transport: append a marker byte per real download, then copy.

    The source + counter paths ride in the URL query so no shared memory is
    needed — the counter file's byte-length *is* the cross-process download
    count.
    """

    NAME = "count"
    SCHEMES = ("count",)

    def schemes(self):
        return ["count"]

    def fetch(self, resolved_url, dest, conditional=None, auth=None):
        query = parse_qs(urlsplit(resolved_url).query)
        src = query["src"][0]
        counter = query["counter"][0]
        fd = os.open(counter, os.O_CREAT | os.O_WRONLY | os.O_APPEND, 0o644)
        try:
            os.write(fd, b"x")  # atomic append: one byte == one real download
        finally:
            os.close(fd)
        time.sleep(DELAY)
        with open(src, "rb") as r, open(dest, "wb") as w:
            w.write(r.read())
        return FetchResult(DOWNLOADED, bytes_written=os.path.getsize(dest))


def _register():
    transport_registry.register(
        "count", CountingFileTransport, keys=["count"], status="active"
    )


def _count_url(src, counter):
    return f"count://download?src={src}&counter={counter}"


def _worker(root, url, start_at):
    """Process worker: own Cache on the shared root; returns (status, blob_sha)."""
    _register()
    time.sleep(max(0.0, start_at - time.time()))  # barrier: maximize contention
    entry = Cache(root=root).fetch(url)
    return entry.status, sha256_file(entry.path)


def test_threads_share_one_cache_exactly_one_download(cache_root, tmp_path):
    _register()
    src = tmp_path / "src.bin"
    src.write_bytes(PAYLOAD)
    counter = tmp_path / "downloads.count"
    url = _count_url(src, counter)
    cache = Cache(root=cache_root)

    n = 8
    with ThreadPoolExecutor(max_workers=n) as pool:
        results = list(pool.map(lambda _: cache.fetch(url), range(n)))

    statuses = [r.status for r in results]
    assert statuses.count("downloaded") == 1
    assert statuses.count("hit") == n - 1
    assert counter.stat().st_size == 1  # exactly one real download
    for r in results:  # every caller saw the complete, identical blob
        assert r.path.read_bytes() == PAYLOAD


def test_processes_share_one_root_exactly_one_download(cache_root, tmp_path):
    src = tmp_path / "src.bin"
    src.write_bytes(PAYLOAD)
    counter = tmp_path / "downloads.count"
    url = _count_url(src, counter)

    n = 4
    start_at = time.time() + 0.5
    ctx = multiprocessing.get_context("fork")
    with ctx.Pool(processes=n) as pool:
        results = pool.starmap(_worker, [(str(cache_root), url, start_at)] * n)

    statuses = [s for s, _ in results]
    shas = {sha for _, sha in results}
    assert statuses.count("downloaded") == 1
    assert counter.stat().st_size == 1  # one download across ALL processes
    assert len(shas) == 1  # every process saw identical bytes (no torn read)
    assert shas == {sha256_file(src)}
