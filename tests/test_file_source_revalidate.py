"""A ``file://`` entry is rechecked against its source before it is served.

``spec/cache-format.md`` §4.1 rung 0 — the regression suite for
`EarthSciML/EarthSciAST#293 <https://github.com/EarthSciML/EarthSciAST/issues/293>`_,
and the Python half of the same suite the Rust track carries in
``rust/tests/file_source_revalidate.rs``.

The bug's whole signature is that it passes GREEN, so every test here
reproduces the actual failure rather than the happy path: warm the cache from a
local file, replace that file **in place** at the same path (so the resolved
URL, and therefore the cache key, is unchanged), read again, and demand the new
bytes. The report's corpus was 2.8 GB replaced at the same paths; the entry it
caught held 371 bytes while the file on disk was 363.

Two of these are the ones a plausible half-fix fails:

* a replacement of the SAME byte length (a float re-encode, a corrected value) —
  a size-only check waves it through;
* an UNCHANGED file, which must still be served from the warm entry and must NOT
  be re-ingested. A "fix" that simply stops caching passes every other test
  here; this one counts the fetches that reach the transport to prove it did
  not.
"""

from __future__ import annotations

import os
import pathlib

import pytest

from earthsciio import (
    Cache,
    FetchError,
    file_source_is_current,
    sha256_bytes,
    sha256_file,
    transport_registry,
)
from earthsciio.config import env_revalidate_file, resolve_revalidate_file
from earthsciio.manifest import Manifest
from earthsciio.transport import DOWNLOADED, FetchResult


def _file_url(p) -> str:
    return "file://" + str(pathlib.Path(p).resolve())


@pytest.fixture
def fetches(monkeypatch):
    """Count the fetches that actually reach the ``file`` transport.

    A cache hit never gets here, so this counter is the difference between
    "served warm" and "re-ingested" — what the unchanged-file test turns on.
    """
    from earthsciio.backends.file import FileTransport

    seen = {"n": 0}
    real = FileTransport.fetch

    def counting(self, resolved_url, dest, conditional=None, auth=None):
        seen["n"] += 1
        return real(self, resolved_url, dest, conditional, auth)

    monkeypatch.setattr(FileTransport, "fetch", counting)
    return seen


# --------------------------------------------------------------------------- #
# 1. replaced in place, different length.
# --------------------------------------------------------------------------- #


def test_source_replaced_in_place_is_re_ingested(cache_root, tmp_path, fetches):
    """The report's own shape: same path, new contents, a different length.

    Before the fix this returned the OLD corpus for the life of the cache dir.
    """
    src = tmp_path / "characterization.csv"
    url = _file_url(src)
    cache = Cache(root=cache_root)

    old = b"o" * 371  # the report's stale entry
    src.write_bytes(old)
    assert cache.fetch(url).path.read_bytes() == old
    assert fetches["n"] == 1

    # Replace the corpus in place — same path, so the same resolved URL and the
    # same cache key.
    new = b"n" * 363  # the report's file-on-disk
    src.write_bytes(new)

    entry = cache.fetch(url)
    assert entry.path.read_bytes() == new, "a file replaced in place must be re-ingested"
    assert entry.status == "downloaded"
    assert fetches["n"] == 2
    # The manifest is the record the report found lying: it must now describe
    # the file that is actually there.
    assert entry.manifest.bytes == 363
    assert entry.manifest.sha256_content == sha256_file(src)


# --------------------------------------------------------------------------- #
# 2. replaced in place, SAME length.
# --------------------------------------------------------------------------- #


def test_source_replaced_with_same_length_is_re_ingested(cache_root, tmp_path, fetches):
    """The sibling case a size-only check misses.

    The report's corpus happened to change length; a float re-encode or a
    corrected value easily would not.
    """
    src = tmp_path / "emissions.csv"
    url = _file_url(src)
    cache = Cache(root=cache_root)

    old = b"year,pm25\n2016,12.500\n"
    new = b"year,pm25\n2016,12.499\n"
    assert len(old) == len(new) and old != new  # the point of this test

    src.write_bytes(old)
    assert cache.fetch(url).path.read_bytes() == old

    src.write_bytes(new)
    assert cache.fetch(url).path.read_bytes() == new, (
        "an equal-length replacement must still be detected — size alone is not enough"
    )
    assert fetches["n"] == 2


# --------------------------------------------------------------------------- #
# 3. an unchanged file is still CACHED.
# --------------------------------------------------------------------------- #


def test_unchanged_source_is_served_from_the_warm_entry(cache_root, tmp_path, fetches):
    """The proof that this is still a cache.

    A fix that "revalidated" by always re-copying the file would pass every
    other test in this file and quietly turn the cache into a no-op.
    """
    src = tmp_path / "static.csv"
    src.write_bytes(b"year,value\n2016,1.0\n")
    url = _file_url(src)
    cache = Cache(root=cache_root)

    first = cache.fetch(url)
    assert first.status == "downloaded"
    assert fetches["n"] == 1

    for _ in range(3):
        again = cache.fetch(url)
        assert again.status == "hit"
        assert again.path == first.path
        assert again.path.read_bytes() == b"year,value\n2016,1.0\n"
        assert again.manifest.fetched_at == first.manifest.fetched_at

    # A fresh Cache over the same root (a new process — the case the report's
    # "warmed at different times" checkouts hit) is still a hit.
    assert Cache(root=cache_root).fetch(url).status == "hit"
    assert fetches["n"] == 1, "an unchanged file must NOT be re-ingested"


# --------------------------------------------------------------------------- #
# 4. a deleted source is an ERROR.
# --------------------------------------------------------------------------- #


def test_deleted_source_behind_a_warm_entry_is_an_error(cache_root, tmp_path):
    """"Pointed at a directory that does not exist ... nothing was read at all."

    Absence must be loud, not a warm stale serve.
    """
    src = tmp_path / "corpus.csv"
    src.write_bytes(b"present")
    url = _file_url(src)
    cache = Cache(root=cache_root)
    assert cache.fetch(url).path.read_bytes() == b"present"

    src.unlink()

    with pytest.raises(FetchError) as excinfo:
        cache.fetch(url)
    assert excinfo.value.not_found, "a deleted source is a definitive absence"


def test_deleted_source_directory_behind_a_warm_entry_is_an_error(cache_root, tmp_path):
    """The same, with the whole corpus directory removed."""
    corpus = tmp_path / "characterization"
    corpus.mkdir()
    src = corpus / "rates.csv"
    src.write_bytes(b"rates")
    url = _file_url(src)
    cache = Cache(root=cache_root)
    assert cache.fetch(url).path.read_bytes() == b"rates"

    src.unlink()
    corpus.rmdir()

    with pytest.raises(FetchError):
        cache.fetch(url)


# --------------------------------------------------------------------------- #
# 5. remote entries are untouched.
# --------------------------------------------------------------------------- #


class MemTransport:
    """A made-up REMOTE scheme: bytes from memory, no local file anywhere.

    Registered under its own name rather than shadowing a built-in, the same way
    ``tests/test_concurrency.py`` registers ``count://``.
    """

    NAME = "mem"
    SCHEMES = ("mem",)
    BODY = b"remote-bytes"
    calls = 0

    def schemes(self):
        return list(self.SCHEMES)

    def fetch(self, resolved_url, dest, conditional=None, auth=None):
        MemTransport.calls += 1
        with open(os.fspath(dest), "wb") as fh:
            fh.write(self.BODY)
        return FetchResult(DOWNLOADED, bytes_written=len(self.BODY))


def test_a_remote_entry_is_not_rechecked_against_the_filesystem(cache_root):
    """Rung 0 is a ``file://`` rung.

    A remote source has no local truth to consult, and its warm entry must keep
    being served without a re-fetch — the immutability declaration (rule 2) is
    what makes an S3-backed store usable at all.
    """
    transport_registry.register("mem", MemTransport, keys=["mem"], status="active")
    MemTransport.calls = 0
    cache = Cache(root=cache_root)
    url = "mem://store/chunk/0.0.0"

    assert cache.fetch(url).path.read_bytes() == MemTransport.BODY
    for _ in range(3):
        assert cache.fetch(url).status == "hit"
    assert MemTransport.calls == 1


# --------------------------------------------------------------------------- #
# 6. the documented opt-out.
# --------------------------------------------------------------------------- #


def test_the_opt_out_restores_the_stale_serve(cache_root, tmp_path, fetches):
    """``revalidate_file=False`` restores the old behaviour exactly.

    Which is why it is off the default path.
    """
    src = tmp_path / "immutable.csv"
    url = _file_url(src)
    cache = Cache(root=cache_root, revalidate_file=False)

    src.write_bytes(b"old")
    assert cache.fetch(url).path.read_bytes() == b"old"
    src.write_bytes(b"new")
    assert cache.fetch(url).path.read_bytes() == b"old", "opted out: the warm entry wins"
    assert fetches["n"] == 1


def test_the_env_knob_only_switches_off_on_an_explicit_denial(
    cache_root, tmp_path, monkeypatch
):
    """``EARTHSCI_REVALIDATE_FILE`` is honoured, and a typo leaves it ON."""
    assert env_revalidate_file() is True  # unset ⇒ on
    for off in ("0", "false", "NO", " off "):
        monkeypatch.setenv("EARTHSCI_REVALIDATE_FILE", off)
        assert env_revalidate_file() is False, off
    for on in ("1", "true", "yes", "", "offf", "maybe"):
        monkeypatch.setenv("EARTHSCI_REVALIDATE_FILE", on)
        assert env_revalidate_file() is True, on
    # The explicit argument wins over the environment, both ways.
    monkeypatch.setenv("EARTHSCI_REVALIDATE_FILE", "0")
    assert resolve_revalidate_file(True) is True
    assert Cache(root=cache_root).revalidate_file is False
    monkeypatch.delenv("EARTHSCI_REVALIDATE_FILE")
    assert Cache(root=cache_root, revalidate_file=False).revalidate_file is False

    # ... and the env knob reaches the actual serve path.
    src = tmp_path / "x.csv"
    src.write_bytes(b"old")
    url = _file_url(src)
    monkeypatch.setenv("EARTHSCI_REVALIDATE_FILE", "0")
    cache = Cache(root=cache_root)
    assert cache.fetch(url).path.read_bytes() == b"old"
    src.write_bytes(b"new")
    assert cache.fetch(url).path.read_bytes() == b"old"


# --------------------------------------------------------------------------- #
# The existing blob-integrity check is a different question.
# --------------------------------------------------------------------------- #


def test_blob_verification_alone_never_sees_a_replaced_source(cache_root, tmp_path):
    """Why ``verify=True`` did not catch any of this.

    It hashes the CACHED BLOB against the manifest, i.e. it compares the copy
    with the record of the copy. With the source replaced it is perfectly
    consistent and perfectly wrong.
    """
    src = tmp_path / "x.csv"
    src.write_bytes(b"old")
    url = _file_url(src)

    stale = Cache(root=cache_root, verify=True, revalidate_file=False)
    assert stale.fetch(url).path.read_bytes() == b"old"
    src.write_bytes(b"new-and-longer")
    # verify passes: blob and manifest agree with each other.
    assert stale.fetch(url).path.read_bytes() == b"old"

    # Rung 0 is the check that asks the source. Same cache root, same key.
    fixed = Cache(root=cache_root, verify=True)
    assert fixed.fetch(url).path.read_bytes() == b"new-and-longer"


# --------------------------------------------------------------------------- #
# The predicate itself, row by row of the §4.1 table.
# --------------------------------------------------------------------------- #


def _manifest_for(body: bytes) -> Manifest:
    return Manifest(
        url="file:///corpus/x.csv",
        sha256_content=sha256_bytes(body),
        bytes=len(body),
        fetched_at="2026-01-01T00:00:00Z",
    )


def test_file_source_is_current_table(tmp_path):
    src = tmp_path / "x.csv"

    # unchanged ⇒ current
    src.write_bytes(b"year,value\n2016,1.0\n")
    assert file_source_is_current(src, _manifest_for(b"year,value\n2016,1.0\n"))

    # different length ⇒ not current
    assert not file_source_is_current(src, _manifest_for(b"371-bytes-worth-of-corpus"))

    # SAME length, different bytes ⇒ not current (the half a size check misses)
    same_len = _manifest_for(b"year,value\n2016,9.0\n")
    assert src.stat().st_size == same_len.bytes
    assert not file_source_is_current(src, same_len)

    # deleted ⇒ not current
    assert not file_source_is_current(tmp_path / "gone.csv", _manifest_for(b"whatever"))

    # a directory in place of the source ⇒ not current
    sub = tmp_path / "subdir"
    sub.mkdir()
    assert not file_source_is_current(sub, _manifest_for(b"whatever"))


# --------------------------------------------------------------------------- #
# 7. a source this host cannot READ is not a source that changed.
# --------------------------------------------------------------------------- #


@pytest.mark.skipif(os.geteuid() == 0, reason="root traverses a 0o000 directory")
def test_unreadable_source_abstains_and_serves_the_warm_entry(cache_root, tmp_path):
    """Rung 0 answers UNKNOWN, not "stale", when the path cannot be consulted.

    Warming a cache where ``/corpus`` is mounted and reading it where it is not
    is an ordinary HPC shape, and it used to work: the entry was served warm.
    The first cut of this rung turned every ``stat`` failure into a re-ingest,
    which turned that shape into a hard :class:`FetchError`. Only a genuine
    "no such file" is evidence of a deletion; "permission denied" is evidence of
    nothing at all.
    """
    corpus = tmp_path / "corpus"
    corpus.mkdir()
    src = corpus / "x.nc"
    src.write_bytes(b"warm-corpus")
    url = _file_url(src)

    c = Cache(root=cache_root)
    assert c.fetch(url).status == "downloaded"

    corpus.chmod(0o000)  # the closest a test gets to "that mount is not here"
    try:
        entry = c.fetch(url)
        assert entry.status == "hit"
        assert entry.path.read_bytes() == b"warm-corpus"
    finally:
        corpus.chmod(0o755)


@pytest.mark.skipif(os.geteuid() == 0, reason="root traverses a 0o000 directory")
def test_only_a_genuine_absence_reads_as_missing(tmp_path):
    """The classification itself, on the three error shapes that matter."""
    from earthsciio.validate import CURRENT, MISSING, UNKNOWN, file_source_state

    body = b"corpus"
    present = tmp_path / "present.nc"
    present.write_bytes(body)
    m = _manifest_for(body)

    assert file_source_state(present, m) == CURRENT
    assert file_source_state(tmp_path / "gone.nc", m) == MISSING

    # A path *under a file* (ENOTDIR) is a broken path, not a deletion: this
    # host may simply have the wrong mount.
    assert file_source_state(present / "deeper.nc", m) == UNKNOWN

    locked = tmp_path / "locked"
    locked.mkdir()
    hidden = locked / "x.nc"
    hidden.write_bytes(body)
    locked.chmod(0o000)
    try:
        assert file_source_state(hidden, m) == UNKNOWN
    finally:
        locked.chmod(0o755)


# --------------------------------------------------------------------------- #
# 8. a missing canonical with mirrors is not a per-read re-download.
# --------------------------------------------------------------------------- #


@pytest.fixture
def mem_mirror():
    """The ``mem://`` transport above, registered and its counter zeroed.

    It has no local file anywhere, which is the point: it stands in for the
    remote mirror that actually served a blob whose canonical URL is ``file://``.
    """
    transport_registry.register("mem", MemTransport, keys=["mem"], status="active")
    MemTransport.calls = 0
    return MemTransport


def test_missing_canonical_with_mirrors_still_hits(cache_root, tmp_path, mem_mirror):
    """A ``file://`` canonical plus a working mirror must still produce HITS.

    The manifest records the canonical URL whichever candidate served the bytes
    (``Cache._commit``), so rung 0 stats a canonical that was never there, finds
    nothing, and — if it treated that as stale — would re-download from the
    mirror on every single read, for ever, never once hitting. Measured before
    the fix: 3 fetches for 3 reads.
    """
    canonical = _file_url(tmp_path / "never-mounted.nc")
    c = Cache(root=cache_root)
    for i in range(3):
        entry = c.fetch(canonical, mirrors=["mem://mirror/x.nc"])
        assert entry.path.read_bytes() == mem_mirror.BODY, f"read {i}"
    assert mem_mirror.calls == 1, "the mirror is downloaded ONCE, not once per read"


def test_mirrors_do_not_excuse_a_replaced_canonical(cache_root, tmp_path, mem_mirror):
    """The abstention above is scoped to *absence*.

    A canonical that is really there and really changed is still caught, mirrors
    or no mirrors — otherwise configuring a mirror would silently switch the fix
    off.
    """
    src = tmp_path / "x.csv"
    src.write_bytes(b"year,value\n2016,1.0\n")
    url = _file_url(src)
    c = Cache(root=cache_root)

    assert c.fetch(url, mirrors=["mem://mirror/x.nc"]).path.read_bytes() == (
        b"year,value\n2016,1.0\n"
    )
    src.write_bytes(b"year,value\n2016,9.0\n")  # same length
    assert c.fetch(url, mirrors=["mem://mirror/x.nc"]).path.read_bytes() == (
        b"year,value\n2016,9.0\n"
    )
    assert mem_mirror.calls == 0, "the canonical served both reads"


# --------------------------------------------------------------------------- #
# 9. the fingerprint memo.
# --------------------------------------------------------------------------- #


def test_an_unchanged_source_is_read_once_not_once_per_call(tmp_path):
    """Rung 0 reads each unchanged source ONCE per process, not once per call.

    ``fetch`` is called per TICK on the record-selective read paths
    (``Provider._file_for`` skips the decoded-file buffer whenever a ``select``
    is passed), so hashing on every call made the rung cost the whole corpus per
    tick. A digest is kept against the ``(size, mtime)`` it was computed for and
    reused while both hold.
    """
    from earthsciio.validate import CURRENT, SourceRevalidator

    src = tmp_path / "x.nc"
    body = b"a" * 4096
    src.write_bytes(body)
    # Safely in the past: a just-written file is "racy" and is never memoised.
    _set_mtime(src, _whole_second_now() - 60)
    m = _manifest_for(body)

    rev = SourceRevalidator()
    for _ in range(25):
        assert rev.state(src, m) == CURRENT
    assert rev.full_reads == 1, "24 of the 25 calls came from the memo"


def test_the_memo_is_dropped_when_the_source_changes(tmp_path):
    """A replacement changes the mtime, so the digest is recomputed."""
    import time

    from earthsciio.validate import CURRENT, REPLACED, SourceRevalidator

    src = tmp_path / "x.csv"
    body = b"year,value\n2016,1.0\n"
    src.write_bytes(body)
    m = _manifest_for(body)

    rev = SourceRevalidator()
    assert rev.state(src, m) == CURRENT

    # Same length, different bytes — only the mtime betrays it, which is exactly
    # the case the memo has to get right.
    time.sleep(0.01)
    src.write_bytes(b"year,value\n2016,9.0\n")
    assert rev.state(src, m) == REPLACED
    assert rev.full_reads == 2


def _whole_second_now() -> int:
    import time

    return int(time.time())


def _set_mtime(path, seconds: int) -> None:
    ns = seconds * 1_000_000_000
    os.utime(path, ns=(ns, ns))


def test_a_same_size_replacement_within_one_timestamp_tick_is_caught(tmp_path):
    """A filesystem with whole-second mtimes (Lustre, ext3, HFS+; FAT keeps two
    seconds) gives a same-size replacement in the same second the SAME
    ``(size, mtime)``. The memo must not vouch for a digest taken that close to
    the mtime. Simulated by pinning both mtimes to one whole second, so it does
    not depend on the filesystem the test runs on."""
    from earthsciio.validate import CURRENT, REPLACED, SourceRevalidator

    src = tmp_path / "x.csv"
    body = b"year,value\n2016,1.0\n"
    tick = _whole_second_now()
    src.write_bytes(body)
    _set_mtime(src, tick)
    m = _manifest_for(body)

    rev = SourceRevalidator()
    assert rev.state(src, m) == CURRENT

    src.write_bytes(b"year,value\n2016,9.0\n")
    _set_mtime(src, tick)
    assert rev.state(src, m) == REPLACED


def test_a_recent_source_is_rehashed_until_its_mtime_is_safely_past(tmp_path):
    """The racy rule: a digest taken within the margin of the mtime is re-taken on
    the next check, and a check that sees the mtime safely in the past is
    memoised and trusted again."""
    from earthsciio.validate import CURRENT, SourceRevalidator

    src = tmp_path / "x.nc"
    body = b"a" * 4096
    src.write_bytes(body)
    _set_mtime(src, _whole_second_now())
    m = _manifest_for(body)

    rev = SourceRevalidator()
    for _ in range(3):
        assert rev.state(src, m) == CURRENT
    assert rev.full_reads == 3, "a racy digest is never reused"

    _set_mtime(src, _whole_second_now() - 60)
    for _ in range(3):
        assert rev.state(src, m) == CURRENT
    assert rev.full_reads == 4, "one re-hash, then the memo is trusted again"


def test_the_memo_does_not_keep_a_stale_entry_alive_across_a_re_ingest(
    cache_root, tmp_path, fetches
):
    """The memo remembers the SOURCE digest, not the manifest's.

    So it stays right after a re-ingest rewrites the manifest: the next read
    compares the remembered digest against the NEW manifest and says "current"
    without a second read, rather than re-ingesting for ever.
    """
    src = tmp_path / "x.csv"
    src.write_bytes(b"year,value\n2016,1.0\n")
    url = _file_url(src)
    c = Cache(root=cache_root)

    assert c.fetch(url).status == "downloaded"
    src.write_bytes(b"year,value\n2016,9.0\n")  # same length, different bytes
    assert c.fetch(url).status == "downloaded"  # caught, re-ingested
    before = fetches["n"]
    assert c.fetch(url).status == "hit"  # and settled
    assert c.fetch(url).status == "hit"
    assert fetches["n"] == before, "settled means settled: no further re-ingest"
