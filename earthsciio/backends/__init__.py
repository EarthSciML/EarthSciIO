"""Backend implementations for the three EarthSciIO registries.

Each backend is a separate module that registers itself into the registry it
belongs to — so adding one is purely additive (the extensibility invariant,
``spec/registries.md`` §4). This package currently ships the cloud **stubs**
(``esio-9nb.8``); the active ``http``/``file``/``netcdf``/``csv``/``local``
backends are contributed by the language-core work (``esio-9nb.2``).

:func:`register_stub_backends` is idempotent and called once from
:mod:`earthsciio` at import time.
"""

from __future__ import annotations

from ..registry import format_registry, store_registry, transport_registry
from .s3 import S3Store, S3Transport
from .zarr import ZarrReader

__all__ = ["S3Transport", "S3Store", "ZarrReader", "register_stub_backends"]

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
