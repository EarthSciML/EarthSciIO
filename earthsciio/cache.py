"""The cache fetcher — URL download + content-addressed reuse (the core of (a)).

This is the entry point behind the ESS opener/fetcher seam. A caller hands a
**resolved** URL (optionally with mirror candidates) and gets back a
:class:`CacheEntry` whose ``path`` is a local blob — fetched once and reused,
byte-for-byte, across processes *and* languages (the Julia/Rust tracks read the
same blobs from the same ``$EARTHSCIDATADIR``).

The fetch algorithm is ``spec/cache-format.md`` §6:

1. compute ``key = sha256(resolved_url)``; if the blob is present **and** valid,
   return it — **no lock** (the atomic rename guarantees a reader never sees a
   partial file).
2. otherwise take the per-blob advisory lock, **re-check** (another process may
   have just filled it), download to ``tmp/<uuid>.part``, verify, atomically
   rename into ``blobs/``, then write the manifest.

**Offline mode** (``spec/offline-mode.md``) short-circuits everything: no
transport is ever constructed, the store is consulted directly, and a missing
blob raises :class:`~earthsciio.errors.CacheMiss`.

**Mirror failover** behind the ESS ``open_with_fallback`` seam: pass
``mirrors=[...]`` and the canonical URL is tried first, then each mirror in
order; the manifest and key always record the **canonical** URL, never the
mirror that happened to serve the bytes.
"""

from __future__ import annotations

import os
import pathlib
from dataclasses import dataclass
from typing import Optional, Sequence

from . import validate
from .auth import coerce_auth
from .cachekey import cache_key, sha256_file
from .config import resolve_offline
from .errors import CacheMiss, FetchError, IntegrityError, TransportError
from .manifest import Manifest, utc_now_rfc3339
from .registry import store_registry, transport_registry
from .transport import NOT_MODIFIED, ext_from_url, scheme_of

#: :attr:`CacheEntry.status` values.
HIT = "hit"
DOWNLOADED = "downloaded"
NOT_MODIFIED_STATUS = "not_modified"


@dataclass
class CacheEntry:
    """The result of a fetch: where the blob is + how it got there.

    ``status`` is ``"hit"`` (served from cache, no network), ``"downloaded"`` (a
    fresh GET), or ``"not_modified"`` (a conditional GET returned 304 and the
    cached blob was reused). ``manifest`` may be ``None`` only for an offline hit
    against a blob whose sidecar manifest is absent.
    """

    key: str
    path: pathlib.Path
    manifest: Optional[Manifest]
    status: str


class Cache:
    """A content-addressed fetch cache over a pluggable :class:`Store`.

    Parameters
    ----------
    store:
        An explicit store instance. When omitted, the ``store_name`` backend is
        built through the ``store`` registry (default ``local``) rooted at
        ``root`` (else ``$EARTHSCIDATADIR`` / the ``/scratch.local`` default).
    offline:
        ``True``/``False`` forces offline/online; ``None`` (default) consults
        ``$EARTHSCI_OFFLINE``. The explicit argument wins over the environment.
    auth:
        An :class:`~earthsciio.auth.AuthRegistry`, a single resolver, an iterable
        of resolvers, or a ``{realm: resolver}`` dict; ``None`` means no auth.
    verify:
        Re-verify ``sha256`` + byte-length on every read (off by default, on for
        CI/conformance per ``spec/cache-format.md`` §4).
    """

    def __init__(
        self,
        store=None,
        *,
        root: Optional[os.PathLike] = None,
        store_name: str = "local",
        offline: Optional[bool] = None,
        auth=None,
        verify: bool = False,
    ) -> None:
        if store is None:
            store = store_registry.create(store_name, root=root)
        self.store = store
        self.offline = resolve_offline(offline)
        self.auth = coerce_auth(auth)
        self.verify = verify

    # ----------------------------------------------------------------- fetch
    def fetch(
        self,
        resolved_url: str,
        *,
        source_loader: Optional[str] = None,
        auth_realm: Optional[str] = None,
        temporal: Optional[validate.Temporal] = None,
        expected_checksum: Optional[str] = None,
        mirrors: Sequence[str] = (),
    ) -> CacheEntry:
        """Return a :class:`CacheEntry` for ``resolved_url``, fetching if needed.

        ``mirrors`` are additional candidate URLs tried in order after the
        canonical one (the ESS ``open_with_fallback`` failover). ``temporal`` /
        ``expected_checksum`` drive the validation ladder
        (:mod:`earthsciio.validate`). Raises :class:`CacheMiss` offline on a
        miss, :class:`FetchError` when every candidate fails, or
        :class:`IntegrityError` on a checksum/size mismatch.
        """
        key = cache_key(resolved_url)

        if self.offline:
            return self._read_offline(resolved_url, key)

        # 1. Hit without a lock (atomic rename makes this safe).
        hit = self._try_hit(key, temporal, expected_checksum)
        if hit is not None:
            return hit

        # 2. Lock, re-check, download.
        with self.store.lock(key):
            hit = self._try_hit(key, temporal, expected_checksum)
            if hit is not None:
                return hit
            return self._download(
                resolved_url, key, source_loader, auth_realm,
                expected_checksum, list(mirrors),
            )

    # --------------------------------------------------------------- offline
    def _read_offline(self, resolved_url: str, key: str) -> CacheEntry:
        blob = self.store.get_blob(key)
        if blob is None:
            raise CacheMiss(resolved_url, key)
        manifest = self.store.get_meta(key)
        if self.verify and manifest is not None:
            self._verify_blob(blob, manifest, key)
        return CacheEntry(key, blob, manifest, HIT)

    # ------------------------------------------------------------- hit check
    def _try_hit(self, key, temporal, expected_checksum) -> Optional[CacheEntry]:
        blob = self.store.get_blob(key)
        if blob is None:
            return None
        manifest = self.store.get_meta(key)
        if manifest is None:
            return None  # blob without manifest ⇒ treat as miss, re-fetch
        if validate.decide(manifest, temporal, expected_checksum) != validate.HIT:
            return None  # revalidate / miss ⇒ fall through to the download path
        if self.verify:
            self._verify_blob(blob, manifest, key)
        return CacheEntry(key, blob, manifest, HIT)

    # -------------------------------------------------------------- download
    def _download(
        self, resolved_url, key, source_loader, auth_realm, expected_checksum, mirrors
    ) -> CacheEntry:
        prior = self.store.get_meta(key)
        conditional = None
        if prior is not None and (prior.etag or prior.last_modified):
            conditional = {"etag": prior.etag, "last_modified": prior.last_modified}
        # Resolve auth up front: a declared-but-unknown realm is fail-closed.
        resolver = self.auth.resolve(auth_realm)

        candidates = [resolved_url, *mirrors]
        last_err: Optional[BaseException] = None
        for candidate in candidates:
            try:
                transport = transport_registry.create(scheme_of(candidate))
            except Exception as exc:  # unknown scheme / registration gap
                last_err = exc
                continue
            staged = self.store.staging_path()
            try:
                result = transport.fetch(
                    candidate, os.fspath(staged), conditional, resolver
                )
            except TransportError as exc:
                last_err = exc
                _safe_unlink(staged)
                continue
            except Exception as exc:  # defensive: a transport bug is a failed mirror
                last_err = exc
                _safe_unlink(staged)
                continue
            return self._commit(
                resolved_url, key, result, staged,
                source_loader, auth_realm, expected_checksum,
            )

        raise FetchError(resolved_url, attempts=candidates, cause=last_err)

    def _commit(
        self, resolved_url, key, result, staged,
        source_loader, auth_realm, expected_checksum,
    ) -> CacheEntry:
        if result.status == NOT_MODIFIED:
            _safe_unlink(staged)
            blob = self.store.get_blob(key)
            prior = self.store.get_meta(key)
            if blob is None or prior is None:
                raise FetchError(
                    resolved_url, attempts=[resolved_url],
                    cause=RuntimeError("304 Not Modified with no cached blob"),
                )
            # Refresh fetched_at (and any echoed validators); blob/hash unchanged.
            updated = Manifest(
                url=prior.url,
                sha256_content=prior.sha256_content,
                bytes=prior.bytes,
                fetched_at=utc_now_rfc3339(),
                etag=result.etag or prior.etag,
                last_modified=result.last_modified or prior.last_modified,
                source_loader=prior.source_loader if source_loader is None else source_loader,
                auth_realm=prior.auth_realm if auth_realm is None else auth_realm,
            )
            self.store.put_meta(key, updated)
            return CacheEntry(key, blob, updated, NOT_MODIFIED_STATUS)

        # Downloaded: stat, hash, optional checksum check, atomic commit, manifest.
        size = os.path.getsize(staged)
        digest = sha256_file(staged)
        if expected_checksum and digest.lower() != expected_checksum.lower():
            _safe_unlink(staged)
            raise IntegrityError(
                f"checksum mismatch for {resolved_url}",
                key=key, expected=expected_checksum, actual=digest,
            )
        ext = ext_from_url(resolved_url)
        blob = self.store.put_blob(key, staged, ext)
        manifest = Manifest(
            url=resolved_url,  # canonical URL, never a mirror
            sha256_content=digest,
            bytes=size,
            fetched_at=utc_now_rfc3339(),
            etag=result.etag,
            last_modified=result.last_modified,
            source_loader=source_loader,
            auth_realm=auth_realm,
        )
        self.store.put_meta(key, manifest)
        return CacheEntry(key, blob, manifest, DOWNLOADED)

    # ------------------------------------------------------------- integrity
    def _verify_blob(self, blob, manifest: Manifest, key: str) -> None:
        size = os.path.getsize(blob)
        if size != manifest.bytes:
            raise IntegrityError(
                f"byte-size mismatch for cached blob (key={key}): "
                f"{size} != manifest {manifest.bytes}",
                key=key, expected=str(manifest.bytes), actual=str(size),
            )
        digest = sha256_file(blob)
        if digest.lower() != (manifest.sha256_content or "").lower():
            raise IntegrityError(
                f"sha256 mismatch for cached blob (key={key})",
                key=key, expected=manifest.sha256_content, actual=digest,
            )


def _safe_unlink(path) -> None:
    try:
        os.unlink(os.fspath(path))
    except FileNotFoundError:
        pass
