"""EarthSciIO error types.

Defines the small, dedicated exception hierarchy the extensibility registries
raise. Two errors matter for the registry seam (``esio-9nb.8``):

* :class:`BackendNotRegistered` — a name was looked up in a registry but no
  implementation is registered for it. This is a *registration gap*, never a
  Provider bug: adding the backend is a registration, per
  ``spec/registries.md`` §4.
* :class:`Unsupported` — a *registered stub* backend (``status:"stub"`` in
  ``spec/registries.json``) was resolved by name and constructed fine, but one
  of its operations is not implemented yet. S3 (transport + store) and Zarr
  (reader) ship as stubs now; their real bodies land with the future
  ``esio-cloud`` epic (see ``spec/cloud-future.md``) — with *zero* change to
  Provider code.

The base :class:`EarthSciIOError` exists so the language-track cores
(``esio-9nb.2`` and friends) can hang their own runtime errors (``CacheMiss``,
integrity failures, …) off a shared root without this module presuming their
shape.
"""

from __future__ import annotations

from typing import Iterable, List, Optional

__all__ = ["EarthSciIOError", "BackendNotRegistered", "Unsupported"]


class EarthSciIOError(Exception):
    """Base class for every EarthSciIO error."""


class BackendNotRegistered(EarthSciIOError, LookupError):
    """No implementation is registered under a registry lookup key.

    Raised by :meth:`earthsciio.registry.Registry.entry` (and the methods that
    build on it) when a scheme / format / store name has no registered backend.
    The message names the registry, the lookup key, and what *is* available, so
    a missing backend reads as a registration gap rather than an opaque
    ``KeyError`` deep inside dispatch.
    """

    def __init__(
        self,
        registry_kind: str,
        keyed_by: str,
        key: str,
        available: Optional[Iterable[str]] = None,
    ) -> None:
        self.registry_kind = registry_kind
        self.keyed_by = keyed_by
        self.key = key
        self.available: List[str] = sorted(available) if available is not None else []
        super().__init__(
            f"no {registry_kind} backend registered for {keyed_by}={key!r}; "
            f"registered keys: {self.available}. Register one with "
            f"{registry_kind}_registry.register(...) — no Provider change needed "
            f"(spec/registries.md §4)."
        )


class Unsupported(EarthSciIOError, NotImplementedError):
    """A registered *stub* backend's operation is not implemented yet.

    The stub resolves by name and is interface-conformant (so the Provider can
    construct it through the registry unchanged), but calling a real operation
    raises this. It subclasses :class:`NotImplementedError` so callers that
    already special-case "not implemented" catch it, and
    :class:`EarthSciIOError` so EarthSciIO callers can catch the whole family.

    The message points at the tracking epic and the spec note that enumerates
    what the real implementation must deliver.
    """

    def __init__(
        self,
        backend: str,
        registry: str,
        operation: Optional[str] = None,
        *,
        tracking: str = "esio-cloud",
        doc: str = "spec/cloud-future.md",
    ) -> None:
        self.backend = backend
        self.registry = registry
        self.operation = operation
        self.tracking = tracking
        self.doc = doc
        op = operation or "this operation"
        super().__init__(
            f"{registry} backend {backend!r} is a registered stub: {op} is not "
            f"implemented yet. The real implementation is tracked by the future "
            f"{tracking!r} epic (see {doc})."
        )
