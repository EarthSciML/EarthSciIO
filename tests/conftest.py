"""Shared fixtures for the cache-core test suite (``esio-9nb.2``).

The whole suite is **hermetic**: the autouse fixture clears the ambient
environment knobs (``EARTHSCIDATADIR`` / ``EARTHSCI_OFFLINE`` / ``EARTHSCI_LIVE``
plus the CDS knobs ``CDSAPI_KEY`` / ``CDSAPI_URL``) so a test's behavior never
depends on the refinery's environment, and offline tests never accidentally see
a leaked datadir or credential. Tests that exercise those knobs set them
explicitly via ``monkeypatch``.
"""

from __future__ import annotations

import http.server
import json
import pathlib
import threading
from urllib.parse import urlsplit

import pytest

REPO_ROOT = pathlib.Path(__file__).resolve().parent.parent
CORPUS_CACHE = REPO_ROOT / "conformance" / "corpus" / "cache"
CORPUS_DIR = REPO_ROOT / "conformance" / "corpus"


@pytest.fixture(autouse=True)
def _hermetic_env(monkeypatch):
    for var in (
        "EARTHSCIDATADIR",
        "EARTHSCI_OFFLINE",
        "EARTHSCI_LIVE",
        "CDSAPI_KEY",
        "CDSAPI_URL",
    ):
        monkeypatch.delenv(var, raising=False)


@pytest.fixture
def cache_root(tmp_path):
    """A fresh, empty ``$EARTHSCIDATADIR`` for one test."""
    root = tmp_path / "cache"
    root.mkdir()
    return root


# --------------------------------------------------------------------------- #
# A hermetic mock CDS API v1 server (esio-9nb.10).
#
# Implements just enough of the CDS retrieve protocol for the ``cds`` transport
# to drive end-to-end offline: submit -> poll -> results -> asset download, all
# on 127.0.0.1 so CI never opens an external socket. Behaviour is tweakable per
# test via attributes set before the fetch (``poll_states`` / ``payload`` /
# ``fail_submit``).
# --------------------------------------------------------------------------- #

CDS_PAYLOAD = b"\x89NETCDF-ish\x00ERA5-bytes\x01\x02\x03" * 4


class _CdsHandler(http.server.BaseHTTPRequestHandler):
    def log_message(self, *args):  # silence default stderr logging
        pass

    def _json(self, code, obj):
        body = json.dumps(obj).encode()
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_POST(self):
        srv = self.server
        path = urlsplit(self.path).path
        srv.tokens.append(self.headers.get("PRIVATE-TOKEN"))
        # /retrieve/v1/processes/{dataset}/execution
        parts = path.strip("/").split("/")
        srv.submits += 1
        srv.last_dataset = parts[3] if len(parts) > 3 else None
        length = int(self.headers.get("Content-Length", 0))
        body = json.loads(self.rfile.read(length) or b"{}")
        srv.last_inputs = body.get("inputs")
        if srv.fail_submit:
            self._json(400, {"message": "bad request"})
            return
        self._json(200, {"jobID": srv.job_id, "status": "accepted"})

    def do_GET(self):
        srv = self.server
        path = urlsplit(self.path).path
        if path.startswith("/download/"):
            srv.downloads += 1
            self.send_response(200)
            self.send_header("Content-Length", str(len(srv.payload)))
            self.end_headers()
            self.wfile.write(srv.payload)
            return
        srv.tokens.append(self.headers.get("PRIVATE-TOKEN"))
        if path.endswith("/results"):
            href = f"{srv.api_url}/download/era5.nc"
            self._json(200, {"asset": {"value": {"href": href}}})
            return
        # job-status poll: walk the configured state sequence, clamping at the end
        idx = min(srv.polls, len(srv.poll_states) - 1)
        srv.polls += 1
        self._json(200, {"status": srv.poll_states[idx]})


@pytest.fixture
def cds_server():
    """A started, hermetic mock CDS API server (torn down after the test)."""
    srv = http.server.HTTPServer(("127.0.0.1", 0), _CdsHandler)
    host, port = srv.server_address
    srv.api_url = f"http://{host}:{port}"
    srv.job_id = "job-abc123"
    srv.poll_states = ["successful"]
    srv.payload = CDS_PAYLOAD
    srv.fail_submit = False
    srv.submits = srv.polls = srv.downloads = 0
    srv.tokens = []
    srv.last_inputs = None
    srv.last_dataset = None
    thread = threading.Thread(target=srv.serve_forever, daemon=True)
    thread.start()
    try:
        yield srv
    finally:
        srv.shutdown()
        srv.server_close()
