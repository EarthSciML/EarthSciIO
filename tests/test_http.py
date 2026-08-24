"""HTTP transport over a **hermetic localhost** server: GET + conditional GET.

The server binds ``127.0.0.1:0`` (ephemeral) and only ever talks to this test —
no external network, CI-safe (mirrors the Julia/Rust online tests). It serves a
fixed payload with an ``ETag`` and answers ``304`` to a matching
``If-None-Match``. Proves the download path stores the etag, a second online
fetch of a *refreshable* source revalidates (304 ⇒ reuse), a source declared
immutable is served without any round-trip at all, and offline never opens a
socket.
"""

from __future__ import annotations

import http.server
import threading

import pytest

from earthsciio import Cache, Temporal

PAYLOAD = b"t2m,sp\n282.5,100000.0\n282.6,100050.0\n"
ETAG = '"era5-v1"'


class _Handler(http.server.BaseHTTPRequestHandler):
    def log_message(self, *args):  # silence the default stderr logging
        pass

    def do_GET(self):
        self.server.hits += 1
        if self.headers.get("If-None-Match") == ETAG:
            self.server.not_modified += 1
            self.send_response(304)
            self.send_header("ETag", ETAG)
            self.end_headers()
            return
        self.send_response(200)
        self.send_header("ETag", ETAG)
        self.send_header("Content-Length", str(len(PAYLOAD)))
        self.end_headers()
        self.wfile.write(PAYLOAD)


@pytest.fixture
def server():
    srv = http.server.HTTPServer(("127.0.0.1", 0), _Handler)
    srv.hits = 0
    srv.not_modified = 0
    thread = threading.Thread(target=srv.serve_forever, daemon=True)
    thread.start()
    try:
        yield srv
    finally:
        srv.shutdown()
        srv.server_close()


def _url(srv, path="/era5/2018/11/20181108.nc"):
    host, port = srv.server_address
    return f"http://{host}:{port}{path}"


def test_http_download_caches_with_etag(server, cache_root):
    url = _url(server)
    entry = Cache(root=cache_root).fetch(url)
    assert entry.status == "downloaded"
    assert entry.path.read_bytes() == PAYLOAD
    assert entry.manifest.etag == ETAG
    assert entry.manifest.bytes == len(PAYLOAD)
    assert server.hits == 1


def test_conditional_get_304_reuses_blob(server, cache_root):
    url = _url(server)
    c = Cache(root=cache_root)
    # ERA5's current period is still filling in, so the loader declares it
    # incomplete. That declaration is what puts revalidation in play at all: a
    # source with NO temporal is a declared-immutable one, and an immutable
    # source is a hit on the spot rather than a conditional GET (validate.decide
    # rule 2 above rule 3). test_an_immutable_source_never_revalidates covers it.
    refreshable = Temporal.incomplete(3600)
    assert c.fetch(url, temporal=refreshable).status == "downloaded"
    # etag stored + a refreshable source ⇒ revalidate ⇒ conditional GET ⇒ 304.
    # The blob is still inside its TTL, so this also pins the half of the
    # ordering that did NOT change: a validator outranks the TTL heuristic.
    second = c.fetch(url, temporal=refreshable)
    assert second.status == "not_modified"
    assert second.path.read_bytes() == PAYLOAD
    assert server.not_modified == 1
    assert server.hits == 2  # one 200 + one 304


def test_an_immutable_source_never_revalidates(server, cache_root):
    """A declared-immutable source is served from disk, not conditionally GET.

    The manifest holds an ETag here, which is exactly the case that used to force
    a round-trip on every warm hit: S3 returns an ETag on every object, so with
    validators checked first the immutable arm was unreachable for any S3-backed
    store. The peer of the Rust `http_fetch_conditional_get_and_offline_reuse`
    step 3 and of test_validate's ladder-ordering guard.
    """
    url = _url(server)
    c = Cache(root=cache_root)
    first = c.fetch(url)  # no temporal ⇒ declared immutable
    assert first.status == "downloaded"
    assert server.hits == 1

    second = c.fetch(url)
    assert second.status == "hit"
    assert second.path.read_bytes() == PAYLOAD
    assert server.hits == 1, "an immutable source must not re-download"
    assert server.not_modified == 0, "nor even revalidate"


def test_offline_after_http_opens_no_socket(server, cache_root):
    url = _url(server)
    Cache(root=cache_root).fetch(url)
    hits_before = server.hits
    entry = Cache(root=cache_root, offline=True).fetch(url)
    assert entry.status == "hit"
    assert entry.path.read_bytes() == PAYLOAD
    assert server.hits == hits_before  # offline consulted no transport
