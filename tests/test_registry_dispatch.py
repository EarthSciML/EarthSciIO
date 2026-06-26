"""Registry-dispatch tests for the S3 + Zarr stubs (``esio-9nb.8``).

These prove the extensibility seam (``spec/registries.md``):

* the cloud **stubs** resolve **by name** through the three registries and are
  interface-conformant, then raise a clean ``Unsupported`` when an operation is
  actually called (the "name-resolution → graceful Unsupported" sequence);
* the Python registry agrees with the **shared spec** (``spec/registries.json``):
  every ``status:"stub"`` entry there is registered here, and vice-versa;
* a brand-new backend **slots in by registration alone** — the dispatch the
  Provider uses (spec §4) is untouched whether the resolved backend is active or
  a stub.

Run offline; no network, no cache, no fixtures required.
"""

from __future__ import annotations

import json
import pathlib

import pytest

import earthsciio
from earthsciio import (
    BackendNotRegistered,
    Reader,
    Registry,
    Store,
    Transport,
    Unsupported,
    format_registry,
    store_registry,
    transport_registry,
)

REPO_ROOT = pathlib.Path(__file__).resolve().parent.parent
SPEC_REGISTRIES = REPO_ROOT / "spec" / "registries.json"


# --------------------------------------------------------------------------- #
# 1. Name resolution → interface-conformant stub instances.
# --------------------------------------------------------------------------- #


def test_s3_transport_resolves_by_scheme():
    impl = transport_registry.create("s3")
    assert isinstance(impl, Transport)
    assert impl.schemes() == ["s3"]


def test_zarr_reader_resolves_by_format():
    impl = format_registry.create("zarr")
    assert isinstance(impl, Reader)
    assert impl.formats() == ["zarr"]
    assert impl.extensions() == ["zarr"]


def test_s3_store_resolves_by_name():
    impl = store_registry.create("s3")
    assert isinstance(impl, Store)
    assert impl.name() == "s3"


# --------------------------------------------------------------------------- #
# 2. Every stub operation raises a clean, informative Unsupported.
# --------------------------------------------------------------------------- #


def test_s3_transport_fetch_is_unsupported():
    impl = transport_registry.create("s3")
    with pytest.raises(Unsupported) as ei:
        impl.fetch("s3://bucket/era5/2018/20181108.nc", "/tmp/x.part")
    err = ei.value
    assert err.backend == "s3"
    assert err.registry == "transport"
    assert err.operation == "fetch"
    assert err.tracking == "esio-cloud"
    # message points operators at the tracking epic + the spec note
    assert "esio-cloud" in str(err)
    assert "cloud-future" in str(err)


def test_zarr_reader_operations_are_unsupported():
    impl = format_registry.create("zarr")
    with pytest.raises(Unsupported):
        impl.open("/tmp/blob.zarr")
    with pytest.raises(Unsupported):
        impl.read_native(object(), ["t2m"])


@pytest.mark.parametrize("op", ["exists", "get_blob", "get_meta", "lock"])
def test_s3_store_read_ops_are_unsupported(op):
    impl = store_registry.create("s3")
    with pytest.raises(Unsupported):
        getattr(impl, op)("11cdcec1deadbeef")


def test_s3_store_write_ops_are_unsupported():
    impl = store_registry.create("s3")
    with pytest.raises(Unsupported):
        impl.put_blob("key", "/tmp/x.part")
    with pytest.raises(Unsupported):
        impl.put_meta("key", {"url": "s3://b/k"})


def test_unsupported_is_also_not_implemented_error():
    """Unsupported is catchable as NotImplementedError and EarthSciIOError."""
    impl = transport_registry.create("s3")
    with pytest.raises(NotImplementedError):
        impl.fetch("s3://b/k", "/tmp/x")
    with pytest.raises(earthsciio.EarthSciIOError):
        impl.fetch("s3://b/k", "/tmp/x")


# --------------------------------------------------------------------------- #
# 3. Status + introspection reflect "stub".
# --------------------------------------------------------------------------- #


def test_stub_status_is_reported():
    assert transport_registry.status("s3") == "stub"
    assert transport_registry.is_stub("s3")
    assert store_registry.is_stub("s3")
    assert format_registry.is_stub("zarr")


def test_membership_checks():
    assert "s3" in transport_registry
    assert "s3" in store_registry
    assert "zarr" in format_registry
    assert "ftp" not in transport_registry


# --------------------------------------------------------------------------- #
# 4. Unknown names raise BackendNotRegistered (a clean registration gap).
# --------------------------------------------------------------------------- #


@pytest.mark.parametrize(
    "registry,key",
    [
        (transport_registry, "ftp"),
        (format_registry, "grib"),
        (store_registry, "gcs"),
    ],
)
def test_unknown_name_raises_backend_not_registered(registry, key):
    with pytest.raises(BackendNotRegistered) as ei:
        registry.create(key)
    err = ei.value
    assert err.key == key
    assert err.registry_kind == registry.kind
    # available keys are surfaced to make the gap obvious
    assert isinstance(err.available, list)
    assert str(key) in str(err)


def test_backend_not_registered_is_lookup_error():
    with pytest.raises(LookupError):
        transport_registry.create("nope")


# --------------------------------------------------------------------------- #
# 5. The Python registry agrees with the shared spec (registries.json).
# --------------------------------------------------------------------------- #


def _spec_entries():
    spec = json.loads(SPEC_REGISTRIES.read_text())
    return spec["registries"]


def test_spec_file_present_and_versioned():
    spec = json.loads(SPEC_REGISTRIES.read_text())
    assert spec["schema"] == "earthsciio/registries/v1"
    assert set(spec["registries"]) == {"transport", "format", "store"}


def test_registry_keyed_by_matches_spec():
    spec = _spec_entries()
    for kind, reg in earthsciio.all_registries().items():
        assert reg.keyed_by == spec[kind]["keyed_by"], kind


def test_every_spec_stub_is_registered_as_a_stub():
    """Each status:"stub" entry in the spec must be a stub in the Python registry."""
    spec = _spec_entries()
    registries = earthsciio.all_registries()
    found = []
    for kind, reg in registries.items():
        for entry in spec[kind]["entries"]:
            if entry.get("status") != "stub":
                continue
            name = entry["name"]
            found.append((kind, name))
            assert name in reg.names(), f"{kind} stub {name!r} not registered"
            assert reg.is_stub(name), f"{kind} {name!r} should be a stub"
    # the spec declares exactly the S3 (transport+store) and Zarr (format) stubs
    assert set(found) == {
        ("transport", "s3"),
        ("store", "s3"),
        ("format", "zarr"),
    }


def test_registered_stubs_appear_in_spec():
    """Conversely: no Python stub is registered that the spec doesn't declare."""
    spec = _spec_entries()
    for kind, reg in earthsciio.all_registries().items():
        spec_stub_names = {
            e["name"] for e in spec[kind]["entries"] if e.get("status") == "stub"
        }
        for name in reg.names():
            if reg.is_stub(name):
                assert name in spec_stub_names, f"{kind} stub {name!r} absent from spec"


def test_stub_lookup_keys_match_spec():
    """Transport/format stub lookup keys (schemes/extensions) match the spec."""
    spec = _spec_entries()
    t_s3 = next(e for e in spec["transport"]["entries"] if e["name"] == "s3")
    assert transport_registry.entry("s3").keys == tuple(t_s3["schemes"])
    f_zarr = next(e for e in spec["format"]["entries"] if e["name"] == "zarr")
    assert format_registry.entry("zarr").keys == tuple(f_zarr["extensions"])


# --------------------------------------------------------------------------- #
# 6. The §4 invariant: a new backend slots in by registration alone — the
#    dispatch the Provider uses never changes shape for active vs. stub.
# --------------------------------------------------------------------------- #


def provider_dispatch(transport_reg, store_reg, format_reg, *, scheme, store, fmt):
    """A faithful mirror of spec/registries.md §4 Provider dispatch.

    Resolves the registry *triple* by name only. It depends solely on the three
    interfaces; it has no knowledge of which concrete backend (active or stub)
    it will get. This stand-in stays byte-for-byte identical whether we resolve
    the active or the stub backends — that is the invariant under test.
    """
    return (
        transport_reg.create(scheme),
        store_reg.create(store),
        format_reg.create(fmt),
    )


class _FakeActiveTransport:
    """A throwaway 'active' transport used only to prove the seam accepts new
    backends with no dispatch change."""

    NAME = "memdummy"
    SCHEMES = ("memdummy",)

    def schemes(self):
        return list(self.SCHEMES)

    def fetch(self, resolved_url, dest, conditional=None, auth=None):
        return {"status": "downloaded", "bytes_written": 0, "url": resolved_url}


def test_new_active_backend_slots_into_a_fresh_registry():
    reg = Registry("transport", keyed_by="url_scheme")
    reg.register("memdummy", _FakeActiveTransport, keys=["memdummy"])
    impl = reg.create("memdummy")
    assert isinstance(impl, Transport)
    # same dispatch shape, but this one *works* (active, not a stub)
    result = impl.fetch("memdummy://x", "/tmp/x.part")
    assert result["status"] == "downloaded"
    assert reg.status("memdummy") == "active"


def test_provider_dispatch_is_identical_for_active_and_stub():
    """One dispatch path resolves an active triple and a stub triple alike."""
    # An isolated set of registries standing in for a fully-wired Provider.
    t = Registry("transport", keyed_by="url_scheme")
    s = Registry("store", keyed_by="store_name")
    f = Registry("format", keyed_by="format_name")

    # Register an active transport + reuse the real stub classes for the rest.
    from earthsciio.backends.s3 import S3Store, S3Transport
    from earthsciio.backends.zarr import ZarrReader

    t.register("memdummy", _FakeActiveTransport, keys=["memdummy"])
    t.register("s3", S3Transport, keys=["s3"], status="stub")
    s.register("s3", S3Store, status="stub")
    f.register("zarr", ZarrReader, keys=["zarr"], status="stub")

    # Resolve an ACTIVE-transport triple and a STUB-transport triple with the
    # *same* dispatch function — no branching on backend identity.
    active = provider_dispatch(t, s, f, scheme="memdummy", store="s3", fmt="zarr")
    stub = provider_dispatch(t, s, f, scheme="s3", store="s3", fmt="zarr")

    assert isinstance(active[0], Transport)
    assert active[0].fetch("memdummy://x", "/tmp/x")["status"] == "downloaded"
    # the stub transport resolved through the identical path; it only fails on use
    assert isinstance(stub[0], Transport)
    with pytest.raises(Unsupported):
        stub[0].fetch("s3://b/k", "/tmp/x")


def test_global_registry_accepts_and_releases_a_new_backend():
    """The real singletons accept a newly-registered backend (then we clean up)."""
    assert "memdummy" not in transport_registry
    transport_registry.register("memdummy", _FakeActiveTransport, keys=["memdummy"])
    try:
        impl = transport_registry.create("memdummy")
        assert impl.fetch("memdummy://x", "/tmp/x")["status"] == "downloaded"
        # the s3 stub is unaffected — orthogonal registration
        assert transport_registry.is_stub("s3")
    finally:
        transport_registry.unregister("memdummy")
    assert "memdummy" not in transport_registry


# --------------------------------------------------------------------------- #
# 7. Registration guards: fail loud on conflicting re-registration; idempotent
#    on identical re-registration.
# --------------------------------------------------------------------------- #


def test_idempotent_reregistration_is_a_noop():
    before = transport_registry.describe()
    earthsciio.backends.register_stub_backends()
    earthsciio.backends.register_stub_backends()
    assert transport_registry.describe() == before


def test_conflicting_registration_raises():
    reg = Registry("transport", keyed_by="url_scheme")
    reg.register("s3", _FakeActiveTransport, keys=["s3"])

    class _OtherImpl:
        def schemes(self):
            return ["s3"]

        def fetch(self, *a, **k):
            return None

    with pytest.raises(ValueError):
        reg.register("s3", _OtherImpl, keys=["s3"])  # same name, different impl
    with pytest.raises(ValueError):
        reg.register("s3b", _OtherImpl, keys=["s3"])  # same key, different impl


def test_invalid_status_rejected():
    reg = Registry("store", keyed_by="store_name")
    with pytest.raises(ValueError):
        reg.register("x", _FakeActiveTransport, status="experimental")
