"""The active ``file`` transport — a local copy into the cache.

Registered under scheme ``file`` (``spec/registries.md`` §1). It resolves a
``file://`` URL to a local path — expanding ``${EARTHSCIDATADIR}`` first (the
``nei2016`` mirror pattern, ``spec/cache-format.md`` §5) — and copies it into the
staging path. ``file://`` has no conditional-GET or auth semantics, so those
arguments are accepted (for interface conformance) and ignored; the result is
always :data:`~earthsciio.transport.DOWNLOADED` with no validators.

This is what lets a pre-populated local mirror — or the conformance corpus —
feed the cache with exactly the same code path as a network fetch.
"""

from __future__ import annotations

import os
import shutil
from typing import Any, Dict, List, Optional

from ..errors import TransportError
from ..transport import DOWNLOADED, FetchResult, file_url_to_path

__all__ = ["FileTransport"]


class FileTransport:
    """Copy a ``file://`` source (or bare path) into the staging file."""

    NAME = "file"
    SCHEMES = ("file",)

    def schemes(self) -> List[str]:
        return list(self.SCHEMES)

    def fetch(
        self,
        resolved_url: str,
        dest: Any,
        conditional: Optional[Dict[str, Any]] = None,
        auth: Optional[Any] = None,
    ) -> FetchResult:
        src = file_url_to_path(resolved_url)
        if not os.path.isfile(src):
            raise TransportError(
                f"file transport: source not found: {src} (from {resolved_url})"
            )
        try:
            shutil.copyfile(src, os.fspath(dest))
        except OSError as exc:
            raise TransportError(
                f"file copy failed {src} -> {dest}: {exc}"
            ) from exc
        return FetchResult(
            DOWNLOADED,
            etag=None,
            last_modified=None,
            bytes_written=os.path.getsize(dest),
        )
