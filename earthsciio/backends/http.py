"""The active ``http``/``https`` transport — GET + conditional GET.

Registered under schemes ``http`` and ``https`` (``spec/registries.md`` §1). A
fetch is a streaming GET into the staging path; when the cache supplies stored
validators it sends ``If-None-Match`` / ``If-Modified-Since`` and treats a
``304 Not Modified`` as a successful revalidation (reuse the cached blob). The
auth resolver, when present, contributes request headers — credentials live only
in the request, never in the cache.

``requests`` is imported lazily inside :meth:`fetch` so that importing
EarthSciIO — and every offline path, where no transport is ever constructed —
has no hard dependency on it.
"""

from __future__ import annotations

from typing import Any, Dict, List, Optional

from ..errors import TransportError
from ..transport import DOWNLOADED, NOT_MODIFIED, FetchResult

__all__ = ["HttpTransport"]

_USER_AGENT = "earthsciio/python"
_CHUNK = 1 << 16
#: (connect, read) timeouts in seconds — a wedged server must not hang a fetch.
_TIMEOUT = (10, 300)


class HttpTransport:
    """Streaming HTTP(S) GET with conditional revalidation."""

    NAME = "http"
    SCHEMES = ("http", "https")

    def schemes(self) -> List[str]:
        return list(self.SCHEMES)

    def fetch(
        self,
        resolved_url: str,
        dest: Any,
        conditional: Optional[Dict[str, Any]] = None,
        auth: Optional[Any] = None,
    ) -> FetchResult:
        import requests  # lazy: offline never reaches here

        headers: Dict[str, str] = {"User-Agent": _USER_AGENT}
        if conditional:
            etag = conditional.get("etag")
            last_modified = conditional.get("last_modified")
            if etag:
                headers["If-None-Match"] = etag
            if last_modified:
                headers["If-Modified-Since"] = last_modified
        if auth is not None:
            for name, value in auth.headers():
                headers[name] = value

        try:
            resp = requests.get(
                resolved_url,
                headers=headers,
                stream=True,
                timeout=_TIMEOUT,
                allow_redirects=True,
            )
        except requests.RequestException as exc:
            raise TransportError(f"http GET failed for {resolved_url}: {exc}") from exc

        try:
            if resp.status_code == 304:
                # Reuse the validators we sent (the response may omit them).
                cond = conditional or {}
                return FetchResult(
                    NOT_MODIFIED,
                    etag=resp.headers.get("ETag") or cond.get("etag"),
                    last_modified=resp.headers.get("Last-Modified")
                    or cond.get("last_modified"),
                    bytes_written=0,
                )
            if resp.status_code != 200:
                raise TransportError(
                    f"http GET returned {resp.status_code} for {resolved_url}"
                )
            # Capture validators from the response *before* consuming the body.
            etag = resp.headers.get("ETag")
            last_modified = resp.headers.get("Last-Modified")
            written = 0
            with open(dest, "wb") as fh:
                for chunk in resp.iter_content(chunk_size=_CHUNK):
                    if chunk:
                        fh.write(chunk)
                        written += len(chunk)
            return FetchResult(
                DOWNLOADED,
                etag=etag,
                last_modified=last_modified,
                bytes_written=written,
            )
        finally:
            resp.close()
