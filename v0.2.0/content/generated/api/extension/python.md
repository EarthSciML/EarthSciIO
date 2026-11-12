---
title: "Python extension seams"
description: "Python's extension-tier names: documented, reachable, not portable."
type: "api"
---


Extension-tier names are part of this binding's documented surface but are not harmonized across bindings, and may change in a minor release.

6 names.

### `cds_api_key` {#x-cds_api_key}

*function* · canonical name `cds_api_key`

```python
cds_api_key() -> str
```

The CDS API key from `$CDSAPI_KEY` or the `key:` line of `~/.cdsapirc`.

Precedence matches the Julia client: the environment variable wins, then the
rc file. Raises `TransportError` when neither is
present — a hard, surfaced configuration error, not a silent unauthenticated
fetch.

[Source](https://github.com/EarthSciML/EarthSciIO/blob/a49538bbd6ec318dde369d3496f6e8880586a73f/earthsciio/backends/cds.py#L118)

### `CdsTransport` {#x-cdstransport}

*type* · canonical name `cds_transport`

```python
class CdsTransport
CdsTransport(api_url: str | None = None, *, poll_interval: float = DEFAULT_POLL_INTERVAL, timeout: float = DEFAULT_TIMEOUT, sleep=time.sleep)
CdsTransport.schemes() -> list[str]
CdsTransport.fetch(resolved_url: str, dest: Any, conditional: dict[str, Any] | None = None, auth: Any | None = None) -> FetchResult
```

CDS API v1 transport: submit → poll → download behind the fetch seam.

Constructed with no arguments by the cache (`transport_registry.create("cds")`);
the API root, poll cadence and timeout default to the module constants and
can be overridden for tests or a mirror. `conditional` (ETag/If-Modified)
has no CDS analog and is ignored — re-use is keyed on the deterministic
`cds://` URL, so an unchanged request is a cache hit and this transport is
never re-run for it.

[Source](https://github.com/EarthSciML/EarthSciIO/blob/a49538bbd6ec318dde369d3496f6e8880586a73f/earthsciio/backends/cds.py#L349)

### `Reader` {#x-reader}

*type* · canonical name `reader`

```python
class Reader(Protocol)
Reader.formats() -> list[str]
Reader.extensions() -> list[str]
Reader.open(blob_path: Any) -> Any
Reader.read_native(handle: Any, variables: list[str], select: Any | None = None) -> dict[str, Any]
```

Opens a cached blob and returns CF-decoded native-grid arrays.

Keyed by format name. Returns arrays keyed by the on-disk `file_variable`
name; does **not** remap variable names or apply `unit_conversion` (those
are ESS contract semantics — spec §2, Risk R3).

**Members**

- `formats` — Format names this reader handles, e.g. `["netcdf"]`.
- `extensions` — Filename-extension sniff hints, e.g. `["nc", "nc4"]`.
- `open` — Open the cached blob, returning an opaque handle.
- `read_native` — Read `variables` (file_variable names) into native fields + coords.

[Source](https://github.com/EarthSciML/EarthSciIO/blob/a49538bbd6ec318dde369d3496f6e8880586a73f/earthsciio/registry.py#L137)

### `StaticHeaderAuth` {#x-staticheaderauth}

*type* · canonical name `static_header_auth`

```python
class StaticHeaderAuth(AuthResolver)
StaticHeaderAuth(realm: str, headers: Iterable[Header])
StaticHeaderAuth.realm() -> str
StaticHeaderAuth.headers() -> list[Header]
@classmethod StaticHeaderAuth.bearer(realm: str, token: str) -> StaticHeaderAuth
@classmethod StaticHeaderAuth.header(realm: str, name: str, value: str) -> StaticHeaderAuth
```

A resolver that always returns the same fixed header(s).

Covers the common realms with two constructors:

* `bearer` — `Authorization: Bearer <token>` (CDS / RDA / OpenAQ-style
  bearer tokens).
* `header` — an arbitrary custom header, e.g. FIRMS' `X-API-Key`.

The caller supplies the secret; this object only carries it as a header and
never logs or persists it.

[Source](https://github.com/EarthSciML/EarthSciIO/blob/a49538bbd6ec318dde369d3496f6e8880586a73f/earthsciio/auth.py#L36)

### `Store` {#x-store}

*type* · canonical name `store`

```python
class Store(Protocol)
Store.name() -> str
Store.exists(key: str) -> bool
Store.get_blob(key: str) -> Any
Store.put_blob(key: str, staged: Any) -> None
Store.get_meta(key: str) -> Any
Store.put_meta(key: str, manifest: Any) -> None
Store.lock(key: str) -> Any
```

The physical home of the content-addressed cache. Keyed by store name.

Realizes the cache layout in `spec/cache-format.md`; the cache **key**
(`sha256(resolved_url)`) is store-independent.

**Members**

- `name` — The store's registry name, e.g. `"local"`.
- `exists` — Whether a valid blob is present for `key`.
- `get_blob` — Return the blob (path/bytes) for `key`, or `None` on a miss.
- `put_blob` — Atomically commit a staged blob into the cache under `key`.
- `get_meta` — Return the manifest for `key`, or `None` if absent.
- `put_meta` — Persist the manifest for `key`.
- `lock` — Acquire the per-blob advisory lock (scope = one blob fetch).

[Source](https://github.com/EarthSciML/EarthSciIO/blob/a49538bbd6ec318dde369d3496f6e8880586a73f/earthsciio/registry.py#L185)

### `Transport` {#x-transport}

*type* · canonical name `transport`

```python
class Transport(Protocol)
Transport.schemes() -> list[str]
Transport.fetch(resolved_url: str, dest: Any, conditional: dict[str, Any] | None = None, auth: Any | None = None) -> Any
```

Fetches a resolved URL's bytes into the cache. Keyed by URL scheme.

Never constructed in offline mode (the transport is bypassed entirely).

**Members**

- `schemes` — URL schemes this transport serves, e.g. `["http", "https"]`.
- `fetch` — Download `resolved_url` to the staging path `dest`.

[Source](https://github.com/EarthSciML/EarthSciIO/blob/a49538bbd6ec318dde369d3496f6e8880586a73f/earthsciio/registry.py#L110)
