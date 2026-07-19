"""Backend implementations for the three EarthSciIO registries.

Each backend is a separate module registered into the registry it belongs to —
so adding one is purely additive (the extensibility invariant,
``spec/registries.md`` §4). Two families live here:

* the cloud **stubs** — S3 transport+store, Zarr reader (``esio-9nb.8``),
  registered by :func:`register_stub_backends`;
* the **active** core backends — ``http``/``file`` transports and the ``local``
  store (``esio-9nb.2``), registered by :func:`register_active_backends`.

Both registrars are idempotent and called once from :mod:`earthsciio` at import
time. (The ``netcdf``/``csv`` format readers are a separate bead and register
themselves the same way when they land — no Provider change either way.)
"""

from __future__ import annotations

from ..registry import format_registry, store_registry, transport_registry
from .cds import CdsTransport
from .file import FileTransport
from .http import HttpTransport
from .local import LocalStore
from .s3 import S3Store, S3Transport
from .zarr import ZarrReader

__all__ = [
    # stubs
    "S3Transport",
    "S3Store",
    "ZarrReader",
    "register_stub_backends",
    # active core backends
    "HttpTransport",
    "FileTransport",
    "CdsTransport",
    "LocalStore",
    "register_active_backends",
]

#: Epic that tracks the real implementations of the stubs registered here.
STUB_TRACKING_EPIC = "esio-cloud"


def register_stub_backends() -> None:
    """Register the remaining cloud **stub**: the S3 object-store backend.

    The S3 *transport* and the Zarr *reader* are now **active** (registered by
    :func:`register_active_backends`); the S3 *store* (the object store as the
    content-addressed cache) stays a stub — it is not required to read the ISRM
    zarr (the cache stays ``local``) and is tracked by ``esio-cloud``
    (``spec/cloud-future.md`` §2).

    Idempotent: the underlying :meth:`Registry.register` is a no-op when the same
    factory is re-registered, so calling this twice (or importing the package
    twice) is safe.
    """
    store_registry.register(
        S3Store.NAME,
        S3Store,
        status="stub",
        tracking=STUB_TRACKING_EPIC,
        notes="Object-store cache backend; conditional PUT / If-None-Match as the lock analog.",
    )


def register_active_backends() -> None:
    """Register the active ``http``/``file``/``cds``/``s3`` transports, the
    ``local`` store, and (via :func:`register_format_readers`) the format
    readers. The ``s3`` transport (anonymous rewrite → HTTPS) and the ``zarr``
    store-backed reader land here.

    Idempotent (the same factory re-registers as a no-op), so calling this twice
    — or importing the package twice — is safe.
    """
    transport_registry.register(
        S3Transport.NAME,
        S3Transport,
        keys=list(S3Transport.SCHEMES),
        status="active",
        notes="Anonymous s3:// -> regional virtual-hosted HTTPS; delegates to the http transport.",
    )
    format_registry.register(
        ZarrReader.NAME,
        ZarrReader,
        keys=list(ZarrReader.FORMATS),
        status="active",
        extensions=list(ZarrReader.EXTENSIONS),
        notes="Store-backed Zarr v2 reader; lazy orthogonal chunk selection, blosc decode.",
    )
    transport_registry.register(
        HttpTransport.NAME,
        HttpTransport,
        keys=list(HttpTransport.SCHEMES),
        status="active",
        notes="GET + conditional GET; mirror failover at the call site.",
    )
    transport_registry.register(
        FileTransport.NAME,
        FileTransport,
        keys=list(FileTransport.SCHEMES),
        status="active",
        notes="Local copy; expands ${EARTHSCIDATADIR} in file:// templates.",
    )
    transport_registry.register(
        CdsTransport.NAME,
        CdsTransport,
        keys=list(CdsTransport.SCHEMES),
        status="active",
        notes="CDS API v1 submit/poll/download (ERA5 etc.); PRIVATE-TOKEN auth, mocked in CI.",
    )
    store_registry.register(
        "local",
        LocalStore,
        status="active",
        notes="$EARTHSCIDATADIR filesystem; flock + atomic rename.",
    )
