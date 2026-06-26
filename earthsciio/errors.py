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

__all__ = [
    # registry-seam errors (esio-9nb.8)
    "EarthSciIOError",
    "BackendNotRegistered",
    "Unsupported",
    # cache/transport-core runtime errors (esio-9nb.2)
    "CacheMiss",
    "IntegrityError",
    "TransportError",
    "FetchError",
    "OfflineError",
    "AuthError",
]


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


# --------------------------------------------------------------------------- #
# Cache / transport runtime errors (the language-core family, esio-9nb.2).
#
# These hang off :class:`EarthSciIOError` exactly as the module docstring above
# anticipates. They are the runtime counterparts to the registry-seam errors:
# raised while fetching/validating/serving cached blobs, not while resolving a
# backend by name.
# --------------------------------------------------------------------------- #


class CacheMiss(EarthSciIOError):
    """Raised in **offline mode** when the blob for a resolved URL is absent.

    Per ``spec/offline-mode.md`` §2 this is *never* a silent empty result and
    *never* a fallback fetch — offline is cache-only. The error carries the
    ``resolved_url`` and the derived ``key`` (``sha256(resolved_url)``) so the
    caller can point at the exact blob the corpus/cache is missing. Carrying both
    is a cross-language requirement (the Julia/Rust ``CacheMiss`` do the same).
    """

    def __init__(self, resolved_url: str, key: str) -> None:
        self.resolved_url = resolved_url
        self.key = key
        super().__init__(
            f"offline cache miss: no blob for key={key} (url={resolved_url})"
        )


class IntegrityError(EarthSciIOError):
    """A cached or freshly downloaded blob failed its checksum / size check.

    Raised when ``sha256(blob)`` disagrees with the expected/stored
    ``sha256_content`` (or the on-disk length disagrees with ``manifest.bytes``).
    A self-pinned integrity failure means the blob is corrupt and must not be
    served; the cache deletes it and re-fetches (online) or surfaces this error.
    """

    def __init__(
        self,
        message: str,
        *,
        key: Optional[str] = None,
        expected: Optional[str] = None,
        actual: Optional[str] = None,
    ) -> None:
        self.key = key
        self.expected = expected
        self.actual = actual
        super().__init__(message)


class TransportError(EarthSciIOError):
    """A single transport attempt failed (network error, HTTP 4xx/5xx, …).

    The fetch layer catches this **per mirror candidate** and tries the next
    one; only when every candidate fails does it surface as a :class:`FetchError`.
    A ``304 Not Modified`` is **not** an error — it is a successful revalidation.
    """


class FetchError(EarthSciIOError):
    """Every mirror candidate failed to download a resolved URL.

    Carries the canonical ``resolved_url`` and the list of candidate URLs tried
    (canonical first, then mirrors) so an all-mirrors-failed condition names what
    was attempted.
    """

    def __init__(
        self,
        resolved_url: str,
        *,
        attempts: Optional[Iterable[str]] = None,
        cause: Optional[BaseException] = None,
    ) -> None:
        self.resolved_url = resolved_url
        self.attempts: List[str] = list(attempts) if attempts is not None else []
        self.cause = cause
        tried = ", ".join(self.attempts) if self.attempts else resolved_url
        suffix = f": {cause}" if cause is not None else ""
        super().__init__(
            f"all mirror candidates failed to fetch {resolved_url} "
            f"(tried: {tried}){suffix}"
        )


class OfflineError(EarthSciIOError):
    """An operation that needs the network was attempted while offline.

    Distinct from :class:`CacheMiss` (a present-or-absent blob question): this
    names a *contract* violation — e.g. asking a transport to run under
    ``offline=True``, which ``spec/offline-mode.md`` §2 forbids.
    """


class AuthError(EarthSciIOError):
    """Credentials for an auth realm could not be resolved.

    Raised when a fetch declares an ``auth_realm`` for which no resolver is
    registered on the cache's auth map. The realm **name** is the only auth
    detail that ever appears anywhere — credentials are never logged or stored
    (``spec/cache-format.md`` §3).
    """

    def __init__(self, realm: str, *, available: Optional[Iterable[str]] = None) -> None:
        self.realm = realm
        self.available: List[str] = sorted(available) if available is not None else []
        super().__init__(
            f"no auth resolver registered for realm {realm!r}; "
            f"registered realms: {self.available}"
        )
