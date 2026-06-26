"""The pluggable auth seam — realm → resolver, resolver → request headers.

``spec/cache-format.md`` §3: credentials are **never** written to the manifest —
only the realm *name*. The auth layer is a map from realm
(``cds``/``firms``/``openaq``/``rda``/``bearer``/…) to a resolver that returns
HTTP request headers; it is injected into the :class:`~earthsciio.cache.Cache`
and passed to the transport at fetch time, never baked into a transport.

No realm-*specific* resolver lives here (matching the Rust/Julia tracks): a
caller constructs a generic :class:`StaticHeaderAuth` per realm from its own
secret source (env var, secrets file, keyring) and registers it. That keeps
secrets out of the library entirely — EarthSciIO only ever sees a header value
it forwards, and a realm name it records.
"""

from __future__ import annotations

from typing import Dict, Iterable, List, Optional, Tuple, Union

from .errors import AuthError

#: One HTTP request header as a (name, value) pair.
Header = Tuple[str, str]


class AuthResolver:
    """Base interface: a realm name + the request headers that authenticate it."""

    def realm(self) -> str:  # pragma: no cover - abstract
        raise NotImplementedError

    def headers(self) -> List[Header]:  # pragma: no cover - abstract
        raise NotImplementedError


class StaticHeaderAuth(AuthResolver):
    """A resolver that always returns the same fixed header(s).

    Covers the common realms with two constructors:

    * :meth:`bearer` — ``Authorization: Bearer <token>`` (CDS / RDA / OpenAQ-style
      bearer tokens).
    * :meth:`header` — an arbitrary custom header, e.g. FIRMS' ``X-API-Key``.

    The caller supplies the secret; this object only carries it as a header and
    never logs or persists it.
    """

    def __init__(self, realm: str, headers: Iterable[Header]) -> None:
        self._realm = realm
        self._headers: List[Header] = list(headers)

    def realm(self) -> str:
        return self._realm

    def headers(self) -> List[Header]:
        return list(self._headers)

    @classmethod
    def bearer(cls, realm: str, token: str) -> "StaticHeaderAuth":
        return cls(realm, [("Authorization", f"Bearer {token}")])

    @classmethod
    def header(cls, realm: str, name: str, value: str) -> "StaticHeaderAuth":
        return cls(realm, [(name, value)])


class AuthRegistry:
    """A realm → resolver map injected into the cache.

    Constructed from ``None`` (no auth), a single :class:`AuthResolver`, an
    iterable of resolvers, or a ``{realm: resolver}`` dict. The cache resolves a
    fetch's ``auth_realm`` through :meth:`resolve`; a fetch that *declares* a
    realm with no registered resolver raises :class:`~earthsciio.errors.AuthError`
    (fail-closed — we do not silently fetch unauthenticated). A fetch with no
    realm needs no auth and resolves to ``None``.
    """

    def __init__(
        self,
        resolvers: Optional[
            Union[AuthResolver, Iterable[AuthResolver], Dict[str, AuthResolver]]
        ] = None,
    ) -> None:
        self._by_realm: Dict[str, AuthResolver] = {}
        if resolvers is None:
            return
        if isinstance(resolvers, AuthResolver):
            self.register(resolvers)
        elif isinstance(resolvers, dict):
            for realm, resolver in resolvers.items():
                self._by_realm[realm] = resolver
        else:
            for resolver in resolvers:
                self.register(resolver)

    def register(self, resolver: AuthResolver) -> None:
        self._by_realm[resolver.realm()] = resolver

    def resolve(self, realm: Optional[str]) -> Optional[AuthResolver]:
        """Resolver for ``realm``; ``None`` when no realm; raises on unknown realm."""
        if realm is None:
            return None
        try:
            return self._by_realm[realm]
        except KeyError:
            raise AuthError(realm, available=self._by_realm.keys()) from None

    def realms(self) -> List[str]:
        return sorted(self._by_realm)


def coerce_auth(auth) -> AuthRegistry:
    """Normalize the cache ``auth=`` argument to an :class:`AuthRegistry`."""
    if isinstance(auth, AuthRegistry):
        return auth
    return AuthRegistry(auth)
