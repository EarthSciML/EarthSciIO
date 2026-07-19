"""The ``s3`` transport (active) + the ``s3`` object-store backend (stub).

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
edit.

**S3 store** — a registered *stub* (the object-store cache backend is tracked by
``esio-cloud`` / ``spec/cloud-future.md`` §2 and is **not** required to read the
ISRM zarr: for the read goal the cache stays ``local`` and only the transport is
active). Every store operation raises :class:`~earthsciio.errors.Unsupported`.
"""

from __future__ import annotations

import os
from typing import Any, Dict, List, Optional

from ..errors import Unsupported

__all__ = ["S3Transport", "S3Store", "s3_https_url", "resolve_region"]

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
