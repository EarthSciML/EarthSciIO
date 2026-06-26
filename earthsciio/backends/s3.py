"""S3 stubs: an object-store transport and an object-store cache backend.

Both are ``status:"stub"`` in ``spec/registries.json`` — registered now so the
extensibility seam is *exercised* (``esio-9nb.8``), implemented later by the
``esio-cloud`` epic (``spec/cloud-future.md``). Each is interface-conformant
(:class:`earthsciio.registry.Transport` / :class:`~earthsciio.registry.Store`),
so the Provider resolves and constructs it by name unchanged; every real
operation raises :class:`~earthsciio.errors.Unsupported`.

The point is the seam, not the cloud: when the real S3 path lands it replaces
these bodies with **zero** change to Provider code or the language cores.
"""

from __future__ import annotations

from typing import Any, Dict, List, Optional

from ..errors import Unsupported

__all__ = ["S3Transport", "S3Store"]

_TRACKING = "esio-cloud"


class S3Transport:
    """Stub ``s3://`` transport (object-store GET; the future cloud fetch path).

    Real implementation (``esio-cloud``) must do an authenticated/anonymous
    object GET with conditional revalidation (``If-None-Match``/ETag), byte-range
    requests (``#bytes=a-b``), and mirror failover — see ``spec/cloud-future.md``.
    """

    #: Registry name + the URL scheme(s) this transport answers to.
    NAME = "s3"
    SCHEMES = ("s3",)

    def schemes(self) -> List[str]:
        return list(self.SCHEMES)

    def fetch(
        self,
        resolved_url: str,
        dest: Any,
        conditional: Optional[Dict[str, Any]] = None,
        auth: Optional[Any] = None,
    ) -> Any:
        raise Unsupported(self.NAME, registry="transport", operation="fetch", tracking=_TRACKING)


class S3Store:
    """Stub ``s3`` store backend (object store as the content-addressed cache).

    Real implementation (``esio-cloud``) must map ``key → object`` under the
    cache layout, use conditional PUT / ``If-None-Match`` as the lock analog, and
    read manifests as sidecar objects — see ``spec/cloud-future.md``. The cache
    key (``sha256(resolved_url)``) is store-independent, so blobs cached locally
    are addressable here unchanged.
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
