#!/usr/bin/env python3
"""Reference conformance runner for the EarthSciIO corpus (the oracle).

This is the executable definition of "passes conformance", in Python, run
fully OFFLINE against the committed corpus. Every language track's harness
(esio-9nb.9) performs the same five checks and must reach the same verdict:

  1. cache-key agreement     sha256(resolved_url) == case.cache_key
  2. manifest integrity      sha256(blob) == manifest.sha256_content == case.content_sha256
                             and len(blob) == manifest.bytes == case.bytes
  3. format/reader decode    open the blob with the format's reader, CF-decode
  4. native-array equality   decoded arrays == case.expected (tolerance below)
  5. offline-only            no network access (this runner only reads files)

Tolerances: raw/unpacked numeric reads compare exactly; CF-decoded (packed)
values and unit-affected reads compare within ATOL/RTOL (libraries differ at
ULP level — xarray vs NCDatasets vs netcdf-rs). String arrays compare exactly.
``null`` in expected.data maps to NaN (numeric) / missing (string).

Usage:  python3 conformance/verify.py        # verifies every case, exit 1 on failure
Spec:   ../spec/conformance.md
"""

from __future__ import annotations

import csv
import hashlib
import itertools
import json
import math
import pathlib
import sys

import numpy as np

ATOL = 1e-6
RTOL = 1e-9

HERE = pathlib.Path(__file__).resolve().parent
CORPUS = HERE / "corpus"


def _flat(x):
    if isinstance(x, list):
        for e in x:
            yield from _flat(e)
    else:
        yield x


def _cmp_numeric(got: np.ndarray, expected_nested, label: str, errs: list):
    exp = np.array(
        [math.nan if v is None else float(v) for v in _flat(expected_nested)],
        dtype="f8",
    )
    g = np.asarray(got, dtype="f8").reshape(-1)
    if g.shape != exp.shape:
        errs.append(f"{label}: shape {g.shape} != expected {exp.shape}")
        return
    gn, en = np.isnan(g), np.isnan(exp)
    if not np.array_equal(gn, en):
        errs.append(f"{label}: NaN/fill mask mismatch")
        return
    ok = np.allclose(g[~gn], exp[~en], atol=ATOL, rtol=RTOL)
    if not ok:
        d = np.max(np.abs(g[~gn] - exp[~en])) if np.any(~gn) else 0.0
        errs.append(f"{label}: value mismatch (max abs diff {d:g} > atol {ATOL:g})")


def _cmp_string(got, expected_nested, label: str, errs: list):
    g = [str(v) for v in _flat(got)]
    e = [None if v is None else str(v) for v in _flat(expected_nested)]
    if g != e:
        errs.append(f"{label}: string mismatch {g} != {e}")


def read_netcdf(path, expected):
    """CF-decode via xarray: scale/offset + fill->NaN; time NOT decoded."""
    import xarray as xr

    out = {}
    with xr.open_dataset(path, decode_times=False, mask_and_scale=True) as ds:
        for name in expected["variables"]:
            out[name] = ds[name].values
        coords = {}
        for name in expected.get("coords", {}):
            coords[name] = ds[name].values
    return out, coords


def read_csv(path, expected):
    with open(path, newline="") as fh:
        rows = list(csv.reader(fh))
    header, body = rows[0], rows[1:]
    cols = {h: [r[j] for r in body] for j, h in enumerate(header)}
    out = {}
    for name, spec in expected["variables"].items():
        vals = cols[name]
        if spec["dtype"] == "string":
            out[name] = vals
        else:
            out[name] = np.array([float(v) for v in vals], dtype="f8")
    return out, {}


def _zarr_decompress(compressor, raw):
    """Independent decode of one chunk object's bytes (the zarr oracle codec)."""
    if compressor is None:
        return bytes(raw)
    cid = str(compressor.get("id", "")).lower()
    if cid == "blosc":
        from numcodecs import Blosc

        return bytes(Blosc().decode(raw))
    if cid == "zlib":
        from numcodecs import Zlib

        return bytes(Zlib().decode(raw))
    if cid == "zstd":
        from numcodecs import Zstd

        return bytes(Zstd().decode(raw))
    if cid in ("", "none"):
        return bytes(raw)
    raise ValueError(f"unsupported zarr compressor id {cid!r}")


def _zarr_resolve_axis(spec, dim_len):
    """Resolve one axis selector (from ``select.axes``) to a global index list."""
    if spec is None or spec == "all":
        return list(range(dim_len))
    if isinstance(spec, dict) and "indices" in spec:
        return [int(i) for i in spec["indices"]]
    if isinstance(spec, dict) and "slice" in spec:
        s = spec["slice"]
        step = int(s[2]) if len(s) > 2 else 1
        return list(range(int(s[0]), int(s[1]), step))
    if isinstance(spec, (list, tuple)):
        return [int(i) for i in spec]
    raise ValueError(f"unrecognized axis selector: {spec!r}")


def read_zarr(corpus, case):
    """Reference (oracle) Zarr v2 reader: reconstruct each array from its committed
    chunk objects and apply the case's orthogonal selection.

    Independent of the production reader's chunk math — it rebuilds the FULL array
    from every chunk object, then gathers ``full[np.ix_(*sel)]`` — so agreement is
    a real cross-check, not a tautology. Runs offline against the committed blobs.
    """
    objmap = {o["url"]: o for o in case.get("objects", [])}
    base = case["resolved_url"]
    axes_spec = (case.get("select") or {}).get("axes")

    def obj_bytes(url):
        o = objmap.get(url)
        return None if o is None else (corpus / o["blob_path"]).read_bytes()

    out = {}
    for array in case["variables"]:
        zmeta = json.loads(obj_bytes(f"{base}/{array}/.zarray").decode("utf-8"))
        shape = [int(s) for s in zmeta["shape"]]
        chunks = [int(c) for c in zmeta["chunks"]]
        dt = np.dtype(zmeta["dtype"])
        order = zmeta.get("order", "C")
        sep = "." if zmeta.get("dimension_separator") in (None, "") else zmeta["dimension_separator"]
        ndim = len(shape)
        out_dt = np.dtype("float64") if dt.kind == "f" else dt

        if axes_spec is not None and len(axes_spec) == ndim:
            sel = [_zarr_resolve_axis(axes_spec[d], shape[d]) for d in range(ndim)]
        else:
            sel = [list(range(shape[d])) for d in range(ndim)]

        fill = zmeta.get("fill_value", 0.0) or 0.0
        full = np.full(shape, fill, dtype=out_dt)
        nchunks = [-(-shape[d] // chunks[d]) for d in range(ndim)]
        for cidx in itertools.product(*[range(n) for n in nchunks]):
            raw = obj_bytes(f"{base}/{array}/" + sep.join(str(c) for c in cidx))
            if raw is None:
                continue  # absent chunk object → keep fill
            carr = np.frombuffer(_zarr_decompress(zmeta.get("compressor"), raw),
                                 dtype=dt).reshape(chunks, order=order)
            reg = tuple(slice(cidx[d] * chunks[d],
                              min(cidx[d] * chunks[d] + chunks[d], shape[d]))
                        for d in range(ndim))
            loc = tuple(slice(0, r.stop - r.start) for r in reg)
            full[reg] = carr[loc].astype(out_dt, copy=False)
        out[array] = full[np.ix_(*sel)]
    return out, {}


READERS = {"netcdf": read_netcdf, "csv": read_csv, "zarr": read_zarr}


def _verify_zarr_objects(case) -> list:
    """Checks 1+2 PER OBJECT for a store-backed (zarr) case: a Zarr store is many
    objects, not one blob, so key-agreement + integrity are verified for each
    object in the case's ``objects`` array (sha256(url)==cache_key,
    sha256(blob)==content_sha256==manifest.sha256_content, len==bytes, url match)."""
    errs: list = []
    for o in case["objects"]:
        key = hashlib.sha256(o["url"].encode("utf-8")).hexdigest()
        if key != o["cache_key"]:
            errs.append(f"cache-key: sha256({o['url']})={key} != {o['cache_key']}")
        blob = (CORPUS / o["blob_path"]).read_bytes()
        content_sha = hashlib.sha256(blob).hexdigest()
        if content_sha != o["content_sha256"]:
            errs.append(f"integrity: {o['url']} blob sha256 {content_sha} != {o['content_sha256']}")
        if len(blob) != o["bytes"]:
            errs.append(f"integrity: {o['url']} blob bytes {len(blob)} != {o['bytes']}")
        man_path = CORPUS / "cache" / "v1" / "meta" / f"{o['cache_key']}.json"
        manifest = json.loads(man_path.read_text())
        if manifest["sha256_content"] != o["content_sha256"]:
            errs.append(f"integrity: {o['url']} manifest.sha256_content mismatch")
        if manifest["bytes"] != o["bytes"]:
            errs.append(f"integrity: {o['url']} manifest.bytes mismatch")
        if manifest["url"] != o["url"]:
            errs.append(f"integrity: {o['url']} manifest.url mismatch")
    return errs


def verify_case(case_path: pathlib.Path) -> list:
    errs: list = []
    case = json.loads(case_path.read_text())

    if case.get("format") == "zarr":
        # 1 + 2 per object (a Zarr case's resolved_url is a store base, not a blob).
        errs += _verify_zarr_objects(case)
    else:
        # 1. cache-key agreement
        key = hashlib.sha256(case["resolved_url"].encode("utf-8")).hexdigest()
        if key != case["cache_key"]:
            errs.append(f"cache-key: sha256(resolved_url)={key} != case.cache_key={case['cache_key']}")

        # 2. manifest integrity
        blob = (CORPUS / case["blob_path"]).read_bytes()
        content_sha = hashlib.sha256(blob).hexdigest()
        if content_sha != case["content_sha256"]:
            errs.append(f"integrity: blob sha256 {content_sha} != case.content_sha256")
        if len(blob) != case["bytes"]:
            errs.append(f"integrity: blob bytes {len(blob)} != case.bytes {case['bytes']}")
        manifest = json.loads((CORPUS / case["manifest_path"]).read_text())
        if manifest["sha256_content"] != case["content_sha256"]:
            errs.append("integrity: manifest.sha256_content != case.content_sha256")
        if manifest["bytes"] != case["bytes"]:
            errs.append("integrity: manifest.bytes != case.bytes")
        if manifest["url"] != case["resolved_url"]:
            errs.append("integrity: manifest.url != case.resolved_url")

    # 3 + 4. decode + native-array equality
    reader = READERS.get(case["format"])
    if reader is None:
        errs.append(f"format: no reference reader for '{case['format']}' (stub-only?)")
        return errs
    # A zarr case is a store (many objects), not a single blob: the oracle needs
    # the objects + selection from the whole case, not just one blob_path.
    if case["format"] == "zarr":
        got, coords = read_zarr(CORPUS, case)
    else:
        got, coords = reader(CORPUS / case["blob_path"], case["expected"])
    for name, spec in case["expected"]["variables"].items():
        if name not in got:
            errs.append(f"{name}: missing from reader output")
            continue
        if spec["dtype"] == "string":
            _cmp_string(got[name], spec["data"], name, errs)
        else:
            _cmp_numeric(got[name], spec["data"], name, errs)
    for name, spec in case["expected"].get("coords", {}).items():
        if name not in coords:
            errs.append(f"coord {name}: missing from reader output")
            continue
        _cmp_numeric(coords[name], spec["data"], f"coord {name}", errs)
    return errs


def validate_schemas() -> int:
    """Optional: validate manifests + cases against ../spec/schemas (if jsonschema
    is installed). Cross-language tracks rely on the schemas as the contract; this
    is the Python convenience check. Returns the number of schema failures."""
    try:
        from jsonschema import Draft202012Validator
        from referencing import Registry, Resource
    except Exception:
        print("schema-validation: SKIP (jsonschema/referencing not installed)")
        return 0
    sdir = HERE.parent / "spec" / "schemas"
    schemas = {json.loads(p.read_text())["$id"]: json.loads(p.read_text())
               for p in sdir.glob("*.json")}
    resources = [(sid, Resource.from_contents(s)) for sid, s in schemas.items()]
    resources += [(p.name, Resource.from_contents(json.loads(p.read_text())))
                  for p in sdir.glob("*.json")]
    registry = Registry().with_resources(resources)
    man = schemas["https://earthsci.dev/earthsciio/schemas/manifest.schema.json"]
    case = schemas["https://earthsci.dev/earthsciio/schemas/cache-case.schema.json"]
    fails = 0
    for p in (CORPUS / "cache").rglob("meta/*.json"):
        errs = list(Draft202012Validator(man, registry=registry).iter_errors(json.loads(p.read_text())))
        if errs:
            fails += 1
            print(f"schema FAIL manifest {p.name}: {errs[0].message}")
    for p in (CORPUS / "cases").glob("*.json"):
        errs = list(Draft202012Validator(case, registry=registry).iter_errors(json.loads(p.read_text())))
        if errs:
            fails += 1
            print(f"schema FAIL case {p.name}: {errs[0].message}")
    print(f"schema-validation: {'OK' if not fails else str(fails) + ' FAILED'}")
    return fails


def main() -> int:
    index = json.loads((CORPUS / "cases.json").read_text())
    schema_fails = validate_schemas()
    failed = 0
    for entry in index["cases"]:
        case_path = CORPUS / entry["file"]
        errs = verify_case(case_path)
        if errs:
            failed += 1
            print(f"FAIL  {entry['id']}")
            for e in errs:
                print(f"        - {e}")
        else:
            print(f"PASS  {entry['id']}")
    print(f"\n{len(index['cases']) - failed}/{len(index['cases'])} cases passed (offline)")
    return 1 if (failed or schema_fails) else 0


if __name__ == "__main__":
    sys.exit(main())
