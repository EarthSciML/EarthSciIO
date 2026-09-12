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
