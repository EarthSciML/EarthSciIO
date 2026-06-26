"""The per-blob manifest (``meta/<key>.json``).

Schema: ``spec/schemas/manifest.schema.json``. Every cached blob has a sibling
manifest carrying its validation + provenance state. The on-disk form is written
**sorted-keys, indent 2, trailing newline** — byte-for-byte the same convention
the conformance generator uses — so a manifest written by the Python track is
identical to one the Julia/Rust tracks would write, and regenerating the corpus
never churns.

Credentials are **never** written here — only the ``auth_realm`` name.
"""

from __future__ import annotations

import datetime as _dt
import json
from dataclasses import dataclass
from typing import Optional

MANIFEST_SCHEMA_TAG = "earthsciio/manifest/v1"


@dataclass
class Manifest:
    """In-memory view of ``meta/<key>.json`` (see the schema for field meanings)."""

    url: str
    sha256_content: str
    bytes: int
    fetched_at: str
    etag: Optional[str] = None
    last_modified: Optional[str] = None
    source_loader: Optional[str] = None
    auth_realm: Optional[str] = None
    schema: str = MANIFEST_SCHEMA_TAG

    def to_dict(self) -> dict:
        """All nine fields, always present (the key carries ``null`` when N/A)."""
        return {
            "schema": self.schema,
            "url": self.url,
            "etag": self.etag,
            "last_modified": self.last_modified,
            "sha256_content": self.sha256_content,
            "bytes": self.bytes,
            "fetched_at": self.fetched_at,
            "source_loader": self.source_loader,
            "auth_realm": self.auth_realm,
        }

    def to_json(self) -> str:
        """Serialize exactly as the corpus does: sorted keys, indent 2, newline."""
        return json.dumps(self.to_dict(), indent=2, sort_keys=True) + "\n"

    @classmethod
    def from_dict(cls, obj: dict) -> "Manifest":
        return cls(
            url=obj["url"],
            sha256_content=obj["sha256_content"],
            bytes=obj["bytes"],
            fetched_at=obj["fetched_at"],
            etag=obj.get("etag"),
            last_modified=obj.get("last_modified"),
            source_loader=obj.get("source_loader"),
            auth_realm=obj.get("auth_realm"),
            schema=obj.get("schema", MANIFEST_SCHEMA_TAG),
        )

    @classmethod
    def from_json(cls, text: str) -> "Manifest":
        return cls.from_dict(json.loads(text))


def utc_now_rfc3339() -> str:
    """Current UTC instant as ``YYYY-MM-DDTHH:MM:SSZ`` (manifest ``fetched_at``)."""
    now = _dt.datetime.now(_dt.timezone.utc).replace(microsecond=0)
    return now.strftime("%Y-%m-%dT%H:%M:%SZ")


def parse_rfc3339(value: str) -> _dt.datetime:
    """Parse an RFC 3339 / ISO 8601 timestamp to an aware UTC ``datetime``.

    Tolerates the trailing ``Z`` (Python 3.9's ``fromisoformat`` does not accept
    it natively) and fractional seconds. A naive input is assumed UTC.
    """
    text = value.strip()
    if text.endswith("Z") or text.endswith("z"):
        text = text[:-1] + "+00:00"
    dt = _dt.datetime.fromisoformat(text)
    if dt.tzinfo is None:
        dt = dt.replace(tzinfo=_dt.timezone.utc)
    return dt.astimezone(_dt.timezone.utc)
