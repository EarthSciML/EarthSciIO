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
    "LocalStore",
    "register_active_backends",
]

#: Epic that tracks the real implementations of the stubs registered here.
STUB_TRACKING_EPIC = "esio-cloud"


def register_stub_backends() -> None:
    """Register the S3 (transport + store) and Zarr (reader) stubs.

    Idempotent: the underlying :meth:`Registry.register` is a no-op when the same
    factory is re-registered, so calling this twice (or importing the package
    twice) is safe.
    """
    transport_registry.register(
        S3Transport.NAME,
        S3Transport,
        keys=list(S3Transport.SCHEMES),
        status="stub",
        tracking=STUB_TRACKING_EPIC,
        notes="Object-store GET; the future S3-proxy/cloud path.",
    )
    store_registry.register(
        S3Store.NAME,
        S3Store,
        status="stub",
        tracking=STUB_TRACKING_EPIC,
        notes="Object-store cache backend; conditional PUT / If-None-Match as the lock analog.",
    )
    format_registry.register(
        ZarrReader.NAME,
        ZarrReader,
        keys=list(ZarrReader.FORMATS),
        status="stub",
        extensions=list(ZarrReader.EXTENSIONS),
        tracking=STUB_TRACKING_EPIC,
        notes="Chunked array store; the future NetCDF->Zarr cloud path.",
    )


def register_active_backends() -> None:
    """Register the active ``http``/``file`` transports and the ``local`` store.

    Idempotent (the same factory re-registers as a no-op), so calling this twice
    — or importing the package twice — is safe. Orthogonal to the stubs: the
    active backends take new names/keys and never collide with ``s3``/``zarr``.
    """
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
    store_registry.register(
        "local",
        LocalStore,
        status="active",
        notes="$EARTHSCIDATADIR filesystem; flock + atomic rename.",
    )
