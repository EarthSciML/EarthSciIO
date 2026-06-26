"""The active ``cds`` transport — Copernicus Climate Data Store API v1.

Registered under the URL scheme ``cds`` (``spec/registries.md`` §1). Unlike a
plain ``http`` GET, a CDS fetch is a **three-step async retrieval**: submit a
request, poll the job until it succeeds, then download the produced asset. This
module ports the Julia ``cds_api.jl`` client to plain ``requests`` (no Python
``cdsapi`` SDK dependency) and wraps it behind the standard
:class:`~earthsciio.registry.Transport` interface so the content-addressed cache
dispatches it exactly like every other transport.

**The ``cds://`` URL.** The transport interface receives a single resolved URL,
but a CDS retrieval needs a *dataset* plus a *request Dict*. Both are encoded
into the URL by :func:`encode_cds_url` — the cross-language form
``cds://<dataset>?<canonical-request-json>`` (``spec/registries.md`` §1) — and
recovered by :func:`decode_cds_url`. The JSON is serialized with recursively
sorted keys and compact separators, byte-identical to the Julia/Rust tracks, so
an identical request always yields an identical URL — and therefore an identical
cache key (``sha256(resolved_url)``) **across languages**. That is what makes the
cache's **skip-if-exists** work: re-requesting the same ERA5 month is a cache
hit, never a second CDS job. The loader-side request builders live in
:mod:`earthsciio.era5`.

**Authentication.** The CDS API authenticates with a ``PRIVATE-TOKEN`` header.
The spec-aligned path injects it through the pluggable auth seam: register
:func:`cds_auth` under the ``cds`` realm on the :class:`~earthsciio.cache.Cache`
and fetch with ``auth_realm="cds"`` — credentials reach the request, never the
manifest. For standalone use (and the live smoke test) the transport falls back
to reading the key itself from ``$CDSAPI_KEY`` or ``~/.cdsapirc`` via
:func:`cds_api_key`, mirroring the Julia client.

**Offline mode.** Like every transport, ``cds`` is never even constructed when
``offline=True`` — the cache serves the previously downloaded blob from the
store and never touches the network.
"""

from __future__ import annotations

import json
import os
import pathlib
import time
from typing import Any, Dict, List, Mapping, Optional, Tuple

from ..auth import StaticHeaderAuth
from ..errors import TransportError
from ..transport import DOWNLOADED, FetchResult

__all__ = [
    "CdsTransport",
    "CDS_SCHEME",
    "DEFAULT_CDS_API_URL",
    "cds_api_key",
    "cds_api_url",
    "cds_auth",
    "encode_cds_url",
    "decode_cds_url",
    "cds_submit",
    "cds_wait",
    "cds_download",
]

#: The URL scheme this transport answers to (its ``transport`` registry key).
CDS_SCHEME = "cds"

#: The CDS API v1 root. The NEW endpoint (the legacy ``/v2`` host is retired).
DEFAULT_CDS_API_URL = "https://cds.climate.copernicus.eu/api"

#: Seconds between job-status polls, and the overall give-up budget (ported from
#: ``cds_api.jl``'s ``CDS_POLL_INTERVAL`` / ``CDS_TIMEOUT``).
DEFAULT_POLL_INTERVAL = 5.0
DEFAULT_TIMEOUT = 600.0

#: The auth realm name recorded in the manifest for a CDS fetch (never the key).
CDS_REALM = "cds"

#: The CDS header that carries the API key.
_TOKEN_HEADER = "PRIVATE-TOKEN"

_USER_AGENT = "earthsciio/python"
_CHUNK = 1 << 16
#: (connect, read) timeouts in seconds — a wedged CDS endpoint must not hang.
_HTTP_TIMEOUT = (10, 300)


# --------------------------------------------------------------------------- #
# Credentials + endpoint resolution (port of cds_api.jl `cds_api_key`).
# --------------------------------------------------------------------------- #


def _cdsapirc_path() -> pathlib.Path:
    return pathlib.Path(os.path.expanduser("~")) / ".cdsapirc"


def _read_cdsapirc() -> Dict[str, str]:
    """Parse ``~/.cdsapirc`` into ``{"url": ..., "key": ...}`` (missing keys absent).

    The file format is the two-line ``url: ...`` / ``key: ...`` used by the CDS
    SDK. Whitespace around the value is stripped; lines without a recognized
    ``name:`` prefix are ignored. A missing file yields an empty mapping.
    """
    rc = _cdsapirc_path()
    out: Dict[str, str] = {}
    try:
        text = rc.read_text()
    except (FileNotFoundError, OSError):
        return out
    for line in text.splitlines():
        name, sep, value = line.partition(":")
        if not sep:
            continue
        name = name.strip().lower()
        if name in ("url", "key") and name not in out:
            out[name] = value.strip()
    return out


def cds_api_key() -> str:
    """The CDS API key from ``$CDSAPI_KEY`` or the ``key:`` line of ``~/.cdsapirc``.

    Precedence matches the Julia client: the environment variable wins, then the
    rc file. Raises :class:`~earthsciio.errors.TransportError` when neither is
    present — a hard, surfaced configuration error, not a silent unauthenticated
    fetch.
    """
    env = os.environ.get("CDSAPI_KEY")
    if env:
        return env.strip()
    rc = _read_cdsapirc()
    if rc.get("key"):
        return rc["key"]
    raise TransportError(
        "CDS API key not found. Set $CDSAPI_KEY or create ~/.cdsapirc with "
        "'key: <your-key>' (see https://cds.climate.copernicus.eu/how-to-api)."
    )


def cds_api_url() -> str:
    """The CDS API root: ``$CDSAPI_URL`` → ``url:`` in ``~/.cdsapirc`` → default.

    Overridable so a fetch can target a test/mock server or a mirror without
    touching the transport. The trailing slash is normalized off so the client
    can join paths unambiguously.
    """
    env = os.environ.get("CDSAPI_URL")
    if env:
        return env.strip().rstrip("/")
    rc = _read_cdsapirc()
    if rc.get("url"):
        return rc["url"].rstrip("/")
    return DEFAULT_CDS_API_URL


def cds_auth(key: Optional[str] = None) -> StaticHeaderAuth:
    """A ``cds``-realm resolver carrying the ``PRIVATE-TOKEN`` header.

    Register it on the cache (``auth={"cds": cds_auth()}``) and fetch with
    ``auth_realm="cds"``. With ``key=None`` the token is read once via
    :func:`cds_api_key`; the caller may also pass a key resolved from its own
    secret source. The realm *name* is all that ever reaches the manifest.
    """
    token = key if key is not None else cds_api_key()
    return StaticHeaderAuth.header(CDS_REALM, _TOKEN_HEADER, token)


# --------------------------------------------------------------------------- #
# The cds:// URL codec — (dataset, request) <-> a resolved, cache-keyable URL.
# --------------------------------------------------------------------------- #


def _canonical_request_json(request: Mapping[str, Any]) -> str:
    """Deterministic JSON for a CDS request: recursively sorted keys, no spaces.

    Byte-identical to the Julia/Rust tracks' canonical encoders (Rust's
    ``serde_json`` over a ``BTreeMap``, Julia's ``_canonical_json``), so the same
    logical request hashes to the same cache key in every language.
    """
    return json.dumps(request, sort_keys=True, separators=(",", ":"))


def encode_cds_url(dataset: str, request: Mapping[str, Any]) -> str:
    """Encode a CDS ``(dataset, request)`` pair into a resolved ``cds://`` URL.

    The shared, cross-language form is ``cds://<dataset>?<canonical-request-json>``
    (``spec/registries.md`` §1): the request is appended as canonical JSON
    verbatim — **no** ``request=`` parameter and **no** percent-encoding — so the
    URL string (and therefore ``sha256(resolved_url)``) is byte-identical to the
    Rust track for the same request, and a repeat is a cross-language cache hit.
    The dataset is the authority and must not contain ``?``.
    """
    if not dataset:
        raise ValueError("CDS dataset id must be non-empty")
    if "?" in dataset:
        raise ValueError(f"CDS dataset id must not contain '?': {dataset!r}")
    return f"{CDS_SCHEME}://{dataset}?{_canonical_request_json(request)}"


def decode_cds_url(url: str) -> Tuple[str, Dict[str, Any]]:
    """Recover ``(dataset, request)`` from a ``cds://`` URL (inverse of encode).

    String-split (not ``urlsplit``) so the raw JSON in the query is preserved
    verbatim, mirroring the Rust ``parse_cds_url``. Raises :class:`ValueError`
    for a non-``cds`` URL, a missing/empty request, or a request that is not a
    JSON object — a transport must never guess at a corrupt resolved URL.
    """
    prefix = f"{CDS_SCHEME}://"
    if not url.startswith(prefix):
        raise ValueError(f"not a cds:// URL: {url!r}")
    dataset, sep, payload = url[len(prefix):].partition("?")
    if not sep:
        raise ValueError(f"cds:// URL missing '?<request-json>': {url!r}")
    if not dataset:
        raise ValueError(f"cds:// URL has an empty dataset: {url!r}")
    if not payload:
        raise ValueError(f"cds:// URL has an empty request: {url!r}")
    try:
        request = json.loads(payload)
    except json.JSONDecodeError as exc:
        raise ValueError(f"cds:// request is not valid JSON: {url!r}") from exc
    if not isinstance(request, dict):
        raise ValueError(f"cds:// request must be a JSON object: {url!r}")
    return dataset, request


# --------------------------------------------------------------------------- #
# The low-level client (port of cds_api.jl: submit -> wait -> download).
# --------------------------------------------------------------------------- #


def cds_submit(
    api_url: str,
    dataset: str,
    request: Mapping[str, Any],
    headers: Mapping[str, str],
) -> str:
    """POST a retrieve request and return the job id (``cds_submit`` in Julia).

    Endpoint: ``{api_url}/retrieve/v1/processes/{dataset}/execution`` with body
    ``{"inputs": request}``. A queued/running/finished job all carry a ``jobID``;
    anything else is a submission failure.
    """
    import requests  # lazy: never imported on the offline path

    url = f"{api_url}/retrieve/v1/processes/{dataset}/execution"
    send = {"User-Agent": _USER_AGENT, "Content-Type": "application/json"}
    send.update(headers)
    try:
        resp = requests.post(
            url, json={"inputs": dict(request)}, headers=send, timeout=_HTTP_TIMEOUT
        )
    except requests.RequestException as exc:
        raise TransportError(f"CDS submit failed for {dataset}: {exc}") from exc
    if resp.status_code >= 400:
        raise TransportError(
            f"CDS submit returned {resp.status_code} for {dataset}: {resp.text}"
        )
    data = resp.json()
    if data.get("status") in ("accepted", "running", "successful") and data.get("jobID"):
        return str(data["jobID"])
    raise TransportError(f"CDS submit did not return a job id for {dataset}: {data!r}")


def cds_wait(
    api_url: str,
    job_id: str,
    headers: Mapping[str, str],
    *,
    poll_interval: float = DEFAULT_POLL_INTERVAL,
    timeout: float = DEFAULT_TIMEOUT,
    sleep=time.sleep,
    monotonic=time.monotonic,
) -> str:
    """Poll a job until ``successful`` and return the asset download href.

    Mirrors ``cds_wait``: on ``successful`` it GETs ``{job}/results`` and returns
    ``asset.value.href``; ``failed`` raises; exceeding ``timeout`` raises. ``sleep``
    and ``monotonic`` are injectable so tests drive the loop without real delay.
    """
    import requests  # lazy

    job_url = f"{api_url}/retrieve/v1/jobs/{job_id}"
    send = {"User-Agent": _USER_AGENT}
    send.update(headers)
    start = monotonic()
    while True:
        try:
            resp = requests.get(job_url, headers=send, timeout=_HTTP_TIMEOUT)
        except requests.RequestException as exc:
            raise TransportError(f"CDS poll failed for job {job_id}: {exc}") from exc
        if resp.status_code >= 400:
            raise TransportError(
                f"CDS poll returned {resp.status_code} for job {job_id}: {resp.text}"
            )
        status = resp.json().get("status", "")
        if status == "successful":
            try:
                results = requests.get(
                    f"{job_url}/results", headers=send, timeout=_HTTP_TIMEOUT
                )
            except requests.RequestException as exc:
                raise TransportError(
                    f"CDS results fetch failed for job {job_id}: {exc}"
                ) from exc
            if results.status_code >= 400:
                raise TransportError(
                    f"CDS results returned {results.status_code} for job {job_id}: "
                    f"{results.text}"
                )
            try:
                return results.json()["asset"]["value"]["href"]
            except (KeyError, TypeError) as exc:
                raise TransportError(
                    f"CDS results for job {job_id} missing asset href: {results.text}"
                ) from exc
        if status == "failed":
            raise TransportError(f"CDS job {job_id} failed: {resp.text}")
        if monotonic() - start > timeout:
            raise TransportError(
                f"CDS job {job_id} timed out after {timeout}s (last status: {status!r})"
            )
        sleep(poll_interval)


def cds_download(href: str, dest: Any) -> int:
    """Stream the produced asset to ``dest``; return the byte count.

    The asset href is a pre-signed object-store URL, so no auth header is sent
    (the Julia client downloads it the same way). Streams in chunks so a large
    NetCDF never lands wholly in memory.
    """
    import requests  # lazy

    try:
        resp = requests.get(
            href, stream=True, timeout=_HTTP_TIMEOUT, allow_redirects=True
        )
    except requests.RequestException as exc:
        raise TransportError(f"CDS asset download failed for {href}: {exc}") from exc
    try:
        if resp.status_code != 200:
            raise TransportError(
                f"CDS asset download returned {resp.status_code} for {href}"
            )
        written = 0
        with open(dest, "wb") as fh:
            for chunk in resp.iter_content(chunk_size=_CHUNK):
                if chunk:
                    fh.write(chunk)
                    written += len(chunk)
        return written
    finally:
        resp.close()


# --------------------------------------------------------------------------- #
# The Transport implementation.
# --------------------------------------------------------------------------- #


class CdsTransport:
    """CDS API v1 transport: submit → poll → download behind the fetch seam.

    Constructed with no arguments by the cache (``transport_registry.create("cds")``);
    the API root, poll cadence and timeout default to the module constants and
    can be overridden for tests or a mirror. ``conditional`` (ETag/If-Modified)
    has no CDS analog and is ignored — re-use is keyed on the deterministic
    ``cds://`` URL, so an unchanged request is a cache hit and this transport is
    never re-run for it.
    """

    NAME = "cds"
    SCHEMES = (CDS_SCHEME,)

    def __init__(
        self,
        api_url: Optional[str] = None,
        *,
        poll_interval: float = DEFAULT_POLL_INTERVAL,
        timeout: float = DEFAULT_TIMEOUT,
        sleep=time.sleep,
    ) -> None:
        self._api_url = api_url
        self.poll_interval = poll_interval
        self.timeout = timeout
        self._sleep = sleep

    def schemes(self) -> List[str]:
        return list(self.SCHEMES)

    def _token_headers(self, auth: Optional[Any]) -> Dict[str, str]:
        """The auth headers for the CDS API calls.

        Prefers the injected resolver (the spec-aligned path — credentials never
        baked into the transport); falls back to reading the key directly for
        standalone/live use.
        """
        if auth is not None:
            return {name: value for name, value in auth.headers()}
        return {_TOKEN_HEADER: cds_api_key()}

    def fetch(
        self,
        resolved_url: str,
        dest: Any,
        conditional: Optional[Dict[str, Any]] = None,
        auth: Optional[Any] = None,
    ) -> FetchResult:
        dataset, request = decode_cds_url(resolved_url)
        api_url = (self._api_url or cds_api_url()).rstrip("/")
        headers = self._token_headers(auth)

        job_id = cds_submit(api_url, dataset, request, headers)
        href = cds_wait(
            api_url,
            job_id,
            headers,
            poll_interval=self.poll_interval,
            timeout=self.timeout,
            sleep=self._sleep,
        )
        written = cds_download(href, dest)
        return FetchResult(DOWNLOADED, bytes_written=written)
