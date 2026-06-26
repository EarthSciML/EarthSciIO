"""EarthSciIO — Python binding of the cross-language data-provider spec.

This module currently exposes the **extensibility seam**: the three registries
(:data:`transport_registry`, :data:`format_registry`, :data:`store_registry`),
their interfaces (:class:`Transport`, :class:`Reader`, :class:`Store`), and the
error types (:class:`Unsupported`, :class:`BackendNotRegistered`). On import it
registers the cloud **stubs** (S3 transport + store, Zarr reader) so the seam is
exercised end-to-end (``esio-9nb.8``).

The active backends and the Provider/cache machinery are contributed by the
language-core work (``esio-9nb.2`` and later beads); they register into these
same registries without changing the Provider API — that is the whole point of
the seam (``spec/registries.md`` §4).
"""

from __future__ import annotations

from .errors import BackendNotRegistered, EarthSciIOError, Unsupported
from .registry import (
    Reader,
    Registry,
    RegistryEntry,
    Store,
    Transport,
    all_registries,
    format_registry,
    store_registry,
    transport_registry,
)

# Register the cloud stubs (S3 transport/store, Zarr reader) on import. Idempotent.
from . import backends

backends.register_stub_backends()

__all__ = [
    # errors
    "EarthSciIOError",
    "BackendNotRegistered",
    "Unsupported",
    # registry mechanism + interfaces
    "Registry",
    "RegistryEntry",
    "Transport",
    "Reader",
    "Store",
    # the three registry singletons
    "transport_registry",
    "format_registry",
    "store_registry",
    "all_registries",
    # backend package
    "backends",
]
