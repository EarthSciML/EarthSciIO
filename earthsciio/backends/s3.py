"""The ``s3`` transport (active) + ``s3`` object stores.

**S3 transport** — an anonymous URL rewriter over the ``http`` transport. The
canonical resolved URL stays ``s3://<bucket>/<key…>`` (kept verbatim in the cache
key + ``manifest.url``, exactly like ``cds://`` is canonical while the CDS
transport rewrites internally). :meth:`S3Transport.fetch` rewrites it to a
**regional virtual-hosted HTTPS** URL — ``https://<bucket>.s3.<region>.amazonaws.com/<key>``
— and delegates to a held :class:`~earthsciio.backends.http.HttpTransport`. A
public bucket needs **no AWS SDK, no SigV4, no credentials**; streaming,
conditional GET (S3 returns ETags), redirect following, and mirror failover all
come for free from the HTTP delegate. Region defaults to ``us-east-2`` (the
pinned InMAP ISRM bucket), overridable via ``$EARTHSCI_S3_REGION`` (fallback
``$AWS_REGION``) or a construction arg. The ``auth`` resolver is threaded through
unchanged so a future SigV4/requester-pays resolver plugs in with no transport
edit. (The anonymous read path stays deliberately SDK-free; it is orthogonal to
the s3fs write path below.)

**S3ObjectStore** — the real object-store I/O the streaming-output-sinks writer
needs, built on **s3fs / fsspec** (the write mirror of what the ZarrReader gets
from the cache). It reads and writes arbitrary object keys under an
``s3://bucket/prefix`` root through an ``fsspec`` filesystem (``s3fs`` by default;
any ``fsspec`` filesystem — e.g. an in-memory one — may be injected for tests).
This is the mature-library replacement for hand-rolled S3 GET/PUT: ``s3fs`` owns
credentials/SigV4, multipart upload (the ``CompleteMultipartUpload`` atomic reveal
of RFC §5.2), retries, and anonymous access. ``s3fs``/``fsspec`` are imported
**lazily** (the optional ``s3`` extra) so a base install stays lean. The Zarr v3
writer targets ``s3://`` output through zarr-python's own ``FsspecStore`` (also
s3fs) — see :mod:`earthsciio.backends.zarr_write`.

**S3Store** — the object store *as the content-addressed cache backend* remains a
registered **stub**. This is a deliberate cross-language coordination point: the
Julia (``julia/src/store.jl``) and Rust tracks also keep it a stub in Wave 1, and
the shared ``spec/registries.json`` pins it ``status:"stub"`` (RFC §13 step 6 /
§16.11 sequence the content-addressed object-store cache after the writers). The
s3fs machinery for it lives in :class:`S3ObjectStore` above, ready to back it when
that coordinated wave lands. Every :class:`S3Store` operation still raises
:class:`~earthsciio.errors.Unsupported`.
"""

from __future__ import annotations

import os
import posixpath
from typing import Any, Dict, List, Optional

from ..errors import Unsupported

__all__ = [
    "S3Transport",
    "S3Store",
    "S3ObjectStore",
    "s3_https_url",
    "resolve_region",
    "parse_s3_url",
]

_TRACKING = "esio-cloud"

#: Default region — the pinned InMAP ISRM bucket lives in us-east-2.
DEFAULT_REGION = "us-east-2"


def resolve_region(region: Optional[str] = None) -> str:
    """Resolve the S3 region: explicit arg → ``$EARTHSCI_S3_REGION`` →
    ``$AWS_REGION`` → :data:`DEFAULT_REGION`."""
    if region:
        return region
    return (
        os.environ.get("EARTHSCI_S3_REGION")
        or os.environ.get("AWS_REGION")
        or DEFAULT_REGION
    )


def s3_https_url(s3_url: str, region: Optional[str] = None) -> str:
    """Rewrite ``s3://<bucket>/<key…>`` to regional virtual-hosted HTTPS.

    ``s3://inmap-model/isrm_v1.2.1.zarr/PrimaryPM25/.zarray`` →
    ``https://inmap-model.s3.us-east-2.amazonaws.com/isrm_v1.2.1.zarr/PrimaryPM25/.zarray``.
    """
    if not s3_url.startswith("s3://"):
        raise ValueError(f"not an s3:// URL: {s3_url!r}")
    rest = s3_url[len("s3://"):]
    bucket, sep, key = rest.partition("/")
    if not bucket:
        raise ValueError(f"s3:// URL has an empty bucket: {s3_url!r}")
    if not sep:
        raise ValueError(f"s3:// URL has no object key: {s3_url!r}")
    return f"https://{bucket}.s3.{resolve_region(region)}.amazonaws.com/{key}"


def parse_s3_url(s3_url: str) -> tuple:
    """Split ``s3://<bucket>/<key…>`` into ``(bucket, key)`` (key may be empty)."""
    if not s3_url.startswith("s3://"):
        raise ValueError(f"not an s3:// URL: {s3_url!r}")
    rest = s3_url[len("s3://") :]
    bucket, _, key = rest.partition("/")
    if not bucket:
        raise ValueError(f"s3:// URL has an empty bucket: {s3_url!r}")
    return bucket, key


class S3ObjectStore:
    """A real object store over ``s3fs``/``fsspec`` — get/put/exists/delete of
    arbitrary object keys under an ``s3://bucket/prefix`` root.

    The mature-library replacement for hand-rolled S3 GET/PUT (the write mirror of
    the reader's cache-backed store). ``s3fs`` handles credentials/SigV4, anonymous
    access, retries and multipart upload; keys are joined onto the root prefix and
    resolved to ``bucket/key`` fsspec paths.

    Parameters
    ----------
    base_url:
        The ``s3://bucket/prefix`` root all keys are relative to.
    fs:
        An explicit ``fsspec`` filesystem (inject an in-memory one for tests).
        When omitted, an ``s3fs.S3FileSystem`` is built lazily (needs the ``s3``
        extra), ``anon`` per ``anon``/the environment.
    anon:
        Force anonymous access (public buckets); ``None`` lets ``s3fs`` decide
        from the environment/credentials.
    """

    def __init__(
        self,
        base_url: str,
        *,
        fs: Optional[Any] = None,
        anon: Optional[bool] = None,
    ) -> None:
        self.bucket, self.prefix = parse_s3_url(base_url)
        self.base_url = base_url.rstrip("/")
        self._fs = fs
        self._anon = anon

    def name(self) -> str:
        return "s3-object"

    @property
    def fs(self) -> Any:
        """The bound fsspec filesystem, building an ``s3fs.S3FileSystem`` lazily."""
        if self._fs is None:
            import s3fs  # lazy: optional ``s3`` extra

            kwargs: Dict[str, Any] = {}
            if self._anon is not None:
                kwargs["anon"] = self._anon
            self._fs = s3fs.S3FileSystem(**kwargs)
        return self._fs

    def _path(self, key: str) -> str:
        """Resolve an object ``key`` to its ``bucket/prefix/key`` fsspec path."""
        rel = posixpath.join(self.prefix, key) if self.prefix else key
        return f"{self.bucket}/{rel}" if rel else self.bucket

    def exists(self, key: str) -> bool:
        return bool(self.fs.exists(self._path(key)))

    def get_bytes(self, key: str) -> Optional[bytes]:
        """The object's bytes, or ``None`` if it is absent (a clean miss)."""
        path = self._path(key)
        if not self.fs.exists(path):
            return None
        return bytes(self.fs.cat_file(path))

    def put_bytes(self, key: str, data: bytes) -> None:
        """Write ``data`` to ``key`` (s3fs handles multipart for large objects)."""
        path = self._path(key)
        parent = posixpath.dirname(path)
        try:
            self.fs.makedirs(parent, exist_ok=True)
        except Exception:  # pragma: no cover - real S3 has no directories
            pass
        self.fs.pipe_file(path, bytes(data))

    def delete(self, key: str) -> None:
        path = self._path(key)
        if self.fs.exists(path):
            self.fs.rm_file(path) if hasattr(self.fs, "rm_file") else self.fs.rm(path)


class S3Transport:
    """Anonymous ``s3://`` transport: rewrite to regional HTTPS, delegate to HTTP."""

    #: Registry name + the URL scheme(s) this transport answers to.
    NAME = "s3"
    SCHEMES = ("s3",)

    def __init__(self, region: Optional[str] = None, http: Optional[Any] = None) -> None:
        # The region is resolved lazily at fetch time (so an env var set after
        # construction still applies) unless pinned here.
        self._region = region
        self._http = http

    def _delegate(self) -> Any:
        if self._http is None:
            from .http import HttpTransport

            self._http = HttpTransport()
        return self._http

    def schemes(self) -> List[str]:
        return list(self.SCHEMES)

    def fetch(
        self,
        resolved_url: str,
        dest: Any,
        conditional: Optional[Dict[str, Any]] = None,
        auth: Optional[Any] = None,
    ) -> Any:
        """Rewrite ``resolved_url`` to regional HTTPS and delegate the GET."""
        https_url = s3_https_url(resolved_url, self._region)
        return self._delegate().fetch(https_url, dest, conditional, auth)


class S3Store:
    """Stub ``s3`` store backend (object store as the content-addressed cache).

    Out of scope for the ISRM zarr read (the cache stays ``local``); the real
    implementation is tracked by the ``esio-cloud`` epic
    (``spec/cloud-future.md`` §2). Every operation raises
    :class:`~earthsciio.errors.Unsupported`.
    """

    #: Registry name (store registry is keyed by store name).
    NAME = "s3"

    def name(self) -> str:
        return self.NAME

    def exists(self, key: str) -> bool:
        raise Unsupported(self.NAME, registry="store", operation="exists", tracking=_TRACKING)

    def get_blob(self, key: str) -> Any:
        raise Unsupported(self.NAME, registry="store", operation="get_blob", tracking=_TRACKING)

    def put_blob(self, key: str, staged: Any) -> None:
        raise Unsupported(self.NAME, registry="store", operation="put_blob", tracking=_TRACKING)

    def get_meta(self, key: str) -> Any:
        raise Unsupported(self.NAME, registry="store", operation="get_meta", tracking=_TRACKING)

    def put_meta(self, key: str, manifest: Any) -> None:
        raise Unsupported(self.NAME, registry="store", operation="put_meta", tracking=_TRACKING)

    def lock(self, key: str) -> Any:
        raise Unsupported(self.NAME, registry="store", operation="lock", tracking=_TRACKING)
