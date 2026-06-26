"""Manifest byte-format parity (``spec/cache-format.md`` §3).

The on-disk manifest must be **byte-identical** across languages: 2-space indent,
keys alphabetically sorted, a trailing newline, and all nine fields always
present (optional ones as JSON ``null``). A drift here makes the Julia/Rust/Python
tracks disagree on the corpus. The corpus manifests are the ground truth, so we
round-trip a real one and require byte-equality.
"""

from __future__ import annotations

import json
import pathlib

from earthsciio import Manifest
from earthsciio.manifest import MANIFEST_SCHEMA_TAG, parse_rfc3339, utc_now_rfc3339

CORPUS_CACHE = pathlib.Path(__file__).resolve().parent.parent / "conformance" / "corpus" / "cache"

ERA5_META = (
    CORPUS_CACHE
    / "v1" / "meta"
    / "11cdcec111409f586e6afc432e1a6da47e6f97ccf3715e5db8554632b00671c1.json"
)

NINE_FIELDS = {
    "schema", "url", "etag", "last_modified", "sha256_content",
    "bytes", "fetched_at", "source_loader", "auth_realm",
}


def test_corpus_manifest_roundtrips_byte_identical():
    raw = ERA5_META.read_text()
    manifest = Manifest.from_json(raw)
    assert manifest.to_json() == raw  # byte-for-byte, including trailing newline


def test_to_json_is_sorted_indented_newline():
    m = Manifest(
        url="https://h/x.nc", sha256_content="ab", bytes=3,
        fetched_at="2026-06-26T00:00:00Z", source_loader="era5",
    )
    text = m.to_json()
    assert text.endswith("\n")
    parsed = json.loads(text)
    assert set(parsed) == NINE_FIELDS
    # all nine present, optionals serialized as null (never omitted)
    assert parsed["etag"] is None
    assert parsed["last_modified"] is None
    assert parsed["auth_realm"] is None
    assert parsed["schema"] == MANIFEST_SCHEMA_TAG
    # keys sorted + 2-space indent (compare against canonical dump)
    assert text == json.dumps(parsed, indent=2, sort_keys=True) + "\n"


def test_from_dict_tolerates_missing_optionals():
    m = Manifest.from_dict(
        {
            "url": "https://h/x.nc",
            "sha256_content": "ab",
            "bytes": 3,
            "fetched_at": "2026-06-26T00:00:00Z",
        }
    )
    assert m.etag is None and m.auth_realm is None
    assert m.schema == MANIFEST_SCHEMA_TAG


def test_rfc3339_helpers_roundtrip():
    stamp = utc_now_rfc3339()
    assert stamp.endswith("Z") and "T" in stamp
    dt = parse_rfc3339(stamp)
    assert dt.tzinfo is not None
    # tolerate fractional seconds + Z
    assert parse_rfc3339("2026-06-26T12:00:00.500Z").year == 2026
