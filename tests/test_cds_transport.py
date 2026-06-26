"""The ``cds`` transport: credential/endpoint resolution, the ``cds://`` codec,
and submit→poll→download against a **hermetic** mock CDS server (``esio-9nb.10``).

No external network: the ``cds_server`` fixture binds ``127.0.0.1:0`` and speaks
just enough CDS API v1 for the transport to drive end-to-end (mirroring the
mocked-server strategy of ``test_http.py``). One live smoke test is gated behind
``EARTHSCI_LIVE`` + a real key and is skipped in CI.
"""

from __future__ import annotations

import json

import pytest

from earthsciio import cache_key, decode_cds_url, encode_cds_url
from earthsciio.backends.cds import (
    DEFAULT_CDS_API_URL,
    CdsTransport,
    cds_api_key,
    cds_api_url,
    cds_auth,
)
from earthsciio.config import env_live
from earthsciio.errors import TransportError


# --------------------------------------------------------------------------- #
# Credential + endpoint resolution (port of cds_api.jl `cds_api_key`).
# --------------------------------------------------------------------------- #


def test_api_key_env_wins(monkeypatch):
    monkeypatch.setenv("CDSAPI_KEY", "  env-token  ")
    assert cds_api_key() == "env-token"


def test_api_key_from_cdsapirc(monkeypatch, tmp_path):
    (tmp_path / ".cdsapirc").write_text(
        "url: https://cds.example/api\nkey: rc-token\n"
    )
    monkeypatch.setenv("HOME", str(tmp_path))
    assert cds_api_key() == "rc-token"
    assert cds_api_url() == "https://cds.example/api"


def test_api_key_missing_raises(monkeypatch, tmp_path):
    monkeypatch.setenv("HOME", str(tmp_path))  # no ~/.cdsapirc here
    with pytest.raises(TransportError) as ei:
        cds_api_key()
    assert "CDS API key not found" in str(ei.value)


def test_api_url_precedence(monkeypatch, tmp_path):
    monkeypatch.setenv("HOME", str(tmp_path))
    assert cds_api_url() == DEFAULT_CDS_API_URL  # default when nothing set
    (tmp_path / ".cdsapirc").write_text("url: https://rc.example/api/\n")
    assert cds_api_url() == "https://rc.example/api"  # rc, trailing slash trimmed
    monkeypatch.setenv("CDSAPI_URL", "http://127.0.0.1:9/api/")
    assert cds_api_url() == "http://127.0.0.1:9/api"  # env wins


def test_cds_auth_carries_private_token():
    resolver = cds_auth("secret-token")
    assert resolver.realm() == "cds"
    assert resolver.headers() == [("PRIVATE-TOKEN", "secret-token")]


# --------------------------------------------------------------------------- #
# The cds:// URL codec.
# --------------------------------------------------------------------------- #


def test_url_is_the_shared_raw_canonical_form():
    # The cross-language form is cds://<dataset>?<canonical-json> — the request
    # rides as raw, sorted-key, compact JSON (no request= param, no %-encoding),
    # so the URL string (hence sha256 cache key) is byte-identical to the Rust
    # track for the same request.
    dataset = "reanalysis-era5-pressure-levels"
    request = {"variable": ["temperature"], "area": [47, -102, 38, -79], "year": ["2018"]}
    canonical = json.dumps(request, sort_keys=True, separators=(",", ":"))
    url = encode_cds_url(dataset, request)
    assert url == f"cds://{dataset}?{canonical}"
    ds, req = decode_cds_url(url)
    assert ds == dataset
    assert req == request


def test_url_is_deterministic_regardless_of_key_order():
    a = encode_cds_url("ds", {"b": [2], "a": [1]})
    b = encode_cds_url("ds", {"a": [1], "b": [2]})
    assert a == b  # canonical JSON ⇒ identical URL ⇒ identical cache key


def test_cds_url_cross_language_byte_identity_golden():
    # A FIXED canonical CDS request pinned to its EXACT resolved cds:// URL and
    # sha256 cache key — the cross-language guard for the URL WRAPPER. The SAME
    # request, url string, and key are asserted verbatim in the Julia
    # (julia/test/test_cds.jl) and Rust (rust/src/transport/cds.rs) suites. The
    # request is held fixed (not built via earthsciio.era5) so this golden
    # isolates the wrapper: any track drifting off the spec's raw
    # cds://<dataset>?<canonical-json> form (spec/registries.md §1) breaks the
    # cross-language cache invariant key = sha256(resolved_url) and fails one of
    # the three suites.
    golden_dataset = "reanalysis-era5-pressure-levels"
    golden_request = {
        "variable": ["geopotential", "temperature"],
        "pressure_level": ["1000", "500"],
        "year": ["2018"],
        "month": ["11"],
        "day": ["01", "08"],
        "time": ["00:00", "12:00"],
        "data_format": "netcdf",
        "area": [50, -130, 20, -60],
    }
    golden_url = (
        "cds://reanalysis-era5-pressure-levels?"
        '{"area":[50,-130,20,-60],"data_format":"netcdf",'
        '"day":["01","08"],"month":["11"],'
        '"pressure_level":["1000","500"],"time":["00:00","12:00"],'
        '"variable":["geopotential","temperature"],"year":["2018"]}'
    )
    golden_key = "435456602e5af8b3d0dd1015fc2c2a024229efd19d5081563ea275c37001bb89"

    # encode → the exact golden URL; its sha256 → the exact pinned key
    assert encode_cds_url(golden_dataset, golden_request) == golden_url
    assert cache_key(golden_url) == golden_key
    # parse round-trips; re-encoding the recovered request reproduces the URL
    ds, req = decode_cds_url(golden_url)
    assert ds == golden_dataset
    assert req == golden_request
    assert encode_cds_url(ds, req) == golden_url


def test_decode_rejects_non_cds_and_malformed():
    with pytest.raises(ValueError):
        decode_cds_url("https://example/x.nc")  # wrong scheme
    with pytest.raises(ValueError):
        decode_cds_url("cds://ds?not-json")  # query is not JSON
    with pytest.raises(ValueError):
        decode_cds_url("cds://ds?[1,2]")  # JSON, but not an object
    with pytest.raises(ValueError):
        decode_cds_url("cds://ds")  # no '?<request>' at all


def test_encode_rejects_empty_dataset():
    with pytest.raises(ValueError):
        encode_cds_url("", {"x": 1})


# --------------------------------------------------------------------------- #
# Transport fetch over the mock server: submit -> poll -> download.
# --------------------------------------------------------------------------- #


def _transport(server, **kw):
    kw.setdefault("poll_interval", 0.0)
    kw.setdefault("sleep", lambda _s: None)
    return CdsTransport(api_url=server.api_url, **kw)


def _url():
    return encode_cds_url(
        "reanalysis-era5-pressure-levels",
        {"variable": ["temperature"], "year": ["2018"], "month": ["11"]},
    )


def test_fetch_submits_polls_and_downloads(cds_server, tmp_path):
    cds_server.poll_states = ["accepted", "running", "successful"]
    dest = tmp_path / "out.part"
    result = _transport(cds_server).fetch(
        _url(), str(dest), auth=cds_auth("tok-123")
    )
    assert result.status == "downloaded"
    assert result.bytes_written == len(cds_server.payload)
    assert dest.read_bytes() == cds_server.payload
    # one submit, three polls (accepted→running→successful), one asset download
    assert cds_server.submits == 1
    assert cds_server.polls == 3
    assert cds_server.downloads == 1
    # the dataset + request reached the server intact
    assert cds_server.last_dataset == "reanalysis-era5-pressure-levels"
    assert cds_server.last_inputs == {
        "variable": ["temperature"], "year": ["2018"], "month": ["11"]
    }
    # PRIVATE-TOKEN went on every API call (submit/poll/results), never blank
    assert cds_server.tokens and all(t == "tok-123" for t in cds_server.tokens)


def test_fetch_uses_injected_auth_over_ambient_key(cds_server, tmp_path, monkeypatch):
    # An ambient key exists, but the injected resolver must take precedence.
    monkeypatch.setenv("CDSAPI_KEY", "ambient-should-not-be-used")
    _transport(cds_server).fetch(
        _url(), str(tmp_path / "o.part"), auth=cds_auth("injected")
    )
    assert all(t == "injected" for t in cds_server.tokens)


def test_fetch_falls_back_to_ambient_key_without_auth(cds_server, tmp_path, monkeypatch):
    monkeypatch.setenv("CDSAPI_KEY", "ambient-key")
    _transport(cds_server).fetch(_url(), str(tmp_path / "o.part"))  # no auth=
    assert all(t == "ambient-key" for t in cds_server.tokens)


def test_fetch_submit_failure_raises_transport_error(cds_server, tmp_path):
    cds_server.fail_submit = True
    with pytest.raises(TransportError) as ei:
        _transport(cds_server).fetch(_url(), str(tmp_path / "o.part"), auth=cds_auth("t"))
    assert "CDS submit returned 400" in str(ei.value)
    assert cds_server.downloads == 0


def test_fetch_job_failed_raises_transport_error(cds_server, tmp_path):
    cds_server.poll_states = ["failed"]
    with pytest.raises(TransportError) as ei:
        _transport(cds_server).fetch(_url(), str(tmp_path / "o.part"), auth=cds_auth("t"))
    assert "failed" in str(ei.value)
    assert cds_server.downloads == 0


def test_fetch_timeout_raises_transport_error(cds_server, tmp_path):
    cds_server.poll_states = ["running"]  # never succeeds
    with pytest.raises(TransportError) as ei:
        _transport(cds_server, timeout=-1).fetch(
            _url(), str(tmp_path / "o.part"), auth=cds_auth("t")
        )
    assert "timed out" in str(ei.value)
    assert cds_server.downloads == 0


# --------------------------------------------------------------------------- #
# Live smoke test — MANUAL, opt-in. Skipped in CI (hermetic env clears the knobs).
#
# Run with a real key + accepted ERA5 licence:
#   EARTHSCI_LIVE=1 CDSAPI_KEY=<key> pytest -q -k live_cds
# It pulls one variable / one level / one hour for a tiny area — enough to prove
# the real submit→poll→download path end-to-end against the production endpoint.
# --------------------------------------------------------------------------- #


@pytest.mark.skipif(
    not env_live(), reason="live CDS pull is opt-in (EARTHSCI_LIVE=1) + needs a real key"
)
def test_live_cds_era5_smoke(tmp_path):
    from earthsciio import era5

    try:
        key = cds_api_key()
    except TransportError:
        pytest.skip("no CDS key available for the live smoke test")
    url = era5.era5_cds_url(
        2018, 11, [8], ["temperature"], [1000], era5.era5_area_from_bbox(0, 0, 1, 1)
    )
    dest = tmp_path / "era5_live.nc"
    result = CdsTransport().fetch(url, str(dest), auth=cds_auth(key))
    assert result.status == "downloaded"
    assert dest.stat().st_size > 0
