"""The active ``local`` store — the content-addressed cache on a filesystem.

Realizes the on-disk layout from ``spec/cache-format.md`` §2 on a
``$EARTHSCIDATADIR``/``v1`` tree::

    v1/blobs/<key[:2]>/<key>.<ext>   the downloaded file
    v1/meta/<key>.json               the manifest
    v1/locks/<key>.lock              per-blob advisory lock
    v1/tmp/<uuid>.part               atomic-rename staging

Two guarantees make a shared ``/scratch.local`` cache safe for many processes
(``spec/cache-format.md`` §6):

* **Atomic rename** — a blob is staged under ``tmp/`` and ``os.replace``-d into
  ``blobs/`` (same filesystem ⇒ atomic). A reader never sees a partial file,
  even without taking the lock.
* **Advisory lock** — a per-blob ``flock`` prevents *redundant* concurrent
  downloads; the winner downloads, the rest re-check and reuse.

Lookups are by ``<key>`` (glob ``<key>*``), **never** by extension — the suffix
is for human debuggability only; a reader is chosen by the ``format`` registry.
``LocalStore`` conforms structurally to :class:`earthsciio.registry.Store` and is
registered as the active ``local`` backend (``s3`` ships as a stub in
:mod:`earthsciio.backends.s3`).
"""

from __future__ import annotations

import os
import pathlib
import uuid
import warnings
from typing import Optional

from ..config import CACHE_FORMAT_VERSION, resolve_cache_root
from ..manifest import Manifest

try:
    import fcntl  # POSIX advisory locks
    _HAVE_FCNTL = True
except ImportError:  # pragma: no cover - non-POSIX fallback
    _HAVE_FCNTL = False

__all__ = ["LocalStore"]


class _FlockLock:
    """Context manager wrapping a POSIX ``flock`` on a per-blob lock file.

    A fresh ``flock(LOCK_EX)`` blocks until the lock is free, so two processes
    (or two threads, each opening its own fd) racing the same key serialize here
    — exactly the "prevent redundant concurrent downloads" guarantee of spec §6.
    Where ``fcntl`` is unavailable the lock degrades to a no-op (single-process
    only) with a warning.
    """

    def __init__(self, lock_path: pathlib.Path) -> None:
        self._lock_path = lock_path
        self._fd: Optional[int] = None

    def __enter__(self) -> "_FlockLock":
        self._lock_path.parent.mkdir(parents=True, exist_ok=True)
        self._fd = os.open(str(self._lock_path), os.O_CREAT | os.O_RDWR, 0o644)
        if _HAVE_FCNTL:
            fcntl.flock(self._fd, fcntl.LOCK_EX)  # blocks until acquired
        else:  # pragma: no cover - exercised only on non-POSIX
            warnings.warn(
                "fcntl unavailable; per-blob locking is a no-op (single-process only)",
                RuntimeWarning,
            )
        return self

    def __exit__(self, *exc) -> None:
        if self._fd is not None:
            try:
                if _HAVE_FCNTL:
                    fcntl.flock(self._fd, fcntl.LOCK_UN)
            finally:
                os.close(self._fd)
                self._fd = None


class LocalStore:
    """Filesystem store rooted at a ``$EARTHSCIDATADIR``/``v1`` directory.

    Directories are created **lazily** on first write so an offline read against
    a read-only corpus never needs write access; reads tolerate missing dirs and
    return ``None`` (a clean miss).
    """

    def __init__(
        self,
        root: Optional[os.PathLike] = None,
        version: str = CACHE_FORMAT_VERSION,
    ) -> None:
        self.root = resolve_cache_root(root)
        self.version = version
        self.version_dir = self.root / version
        self.blobs_dir = self.version_dir / "blobs"
        self.meta_dir = self.version_dir / "meta"
        self.locks_dir = self.version_dir / "locks"
        self.tmp_dir = self.version_dir / "tmp"

    # -- naming -------------------------------------------------------------
    def name(self) -> str:
        return "local"

    def _blob_dir(self, key: str) -> pathlib.Path:
        return self.blobs_dir / key[:2]

    def _meta_path(self, key: str) -> pathlib.Path:
        return self.meta_dir / f"{key}.json"

    def _lock_path(self, key: str) -> pathlib.Path:
        return self.locks_dir / f"{key}.lock"

    # -- blob ---------------------------------------------------------------
    def get_blob(self, key: str) -> Optional[pathlib.Path]:
        """Path to the cached blob, or ``None`` on a miss.

        Looks up by ``<key>`` (glob ``<key>*``), so a blob stored under any
        extension — or under a bare key, no extension — is found.
        """
        d = self._blob_dir(key)
        if not d.is_dir():
            return None
        matches = sorted(d.glob(f"{key}*"))
        return matches[0] if matches else None

    def exists(self, key: str) -> bool:
        return self.get_blob(key) is not None

    def put_blob(
        self, key: str, staged: os.PathLike, ext: str = "bin"
    ) -> pathlib.Path:
        """Atomically commit a staged file as the blob for ``key``; return its path.

        ``ext`` is the human-debug suffix (``""`` ⇒ store under a bare key, no
        trailing dot). Any prior blob for the key — under any extension — is
        dropped first so the glob-by-key lookup stays unambiguous.
        """
        d = self._blob_dir(key)
        d.mkdir(parents=True, exist_ok=True)
        for old in d.glob(f"{key}*"):
            try:
                old.unlink()
            except FileNotFoundError:  # pragma: no cover - race with another writer
                pass
        suffix = ext.lstrip(".").lower() if ext else ""
        target = d / (f"{key}.{suffix}" if suffix else key)
        os.replace(os.fspath(staged), os.fspath(target))  # atomic same-fs rename
        return target

    # -- manifest -----------------------------------------------------------
    def get_meta(self, key: str) -> Optional[Manifest]:
        path = self._meta_path(key)
        if not path.is_file():
            return None
        return Manifest.from_json(path.read_text())

    def put_meta(self, key: str, manifest: Manifest) -> None:
        self.meta_dir.mkdir(parents=True, exist_ok=True)
        path = self._meta_path(key)
        # Atomic manifest write: stage in tmp, then replace, so a concurrent
        # reader never sees a half-written JSON document.
        staged = self.staging_path("json")
        staged.write_text(manifest.to_json())
        os.replace(os.fspath(staged), os.fspath(path))

    # -- staging + lock -----------------------------------------------------
    def staging_path(self, ext: str = "part") -> pathlib.Path:
        """A fresh ``tmp/<uuid>`` path for a transport to download into."""
        self.tmp_dir.mkdir(parents=True, exist_ok=True)
        return self.tmp_dir / f"{uuid.uuid4().hex}.{ext.lstrip('.')}"

    def lock(self, key: str) -> _FlockLock:
        """A context manager for the per-blob advisory lock (scope = one fetch)."""
        return _FlockLock(self._lock_path(key))
