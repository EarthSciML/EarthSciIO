#!/usr/bin/env python3
"""Cross-language **write**-conformance comparator (streaming-output-sinks Wave 5).

The write-side mirror of :mod:`conformance.crosscheck`. Where that gate proves the
three READERS decode a shared corpus to equal arrays, this one proves the three
Zarr v3 sharded WRITERS emit stores that, once decoded, agree — and whose
structural metadata agrees — for the shared input spec ``conformance/write_spec.json``.

Two independent checks, both **tolerance-based, never byte-identity** (RFC §16.6 —
Julia Blosc.jl / Python numcodecs / Rust zarrs are independent codec builds, so
compressed bytes legitimately differ across languages):

  1. **Decoded-array agreement** — each readback dump
     (``earthsciio/write-native-dump/v1`` from ``read_python.py`` / ``read_julia.jl``,
     one per (writer, reader) pair) is checked (a) against the spec ORACLE and
     (b) pairwise against every other dump, within a numeric tolerance
     (``rtol``/``atol`` for float64), with exact structural agreement (dtype, dims,
     dim order, shape). A store read by two readers, or two writers' stores read
     by one reader, must all land on the same arrays.

  2. **Structural / CF-metadata agreement** — each writer's store ``zarr.json``
     objects are read DIRECTLY from disk (language-neutral JSON, no reader
     involved) and compared pairwise: ``data_type``, ``shape``, ``dimension_names``,
     the shard grid (``chunk_grid.chunk_shape``), the sharding inner
     ``chunk_shape``, the Blosc codec params, ``fill_value``, and the key CF
     attributes (``units``/``standard_name``/``axis``/``coordinates``/``calendar``).
     This proves dim order, shape, coord metadata and CF attrs agree even for a
     store no local reader could open (e.g. a Rust store produced in CI).

Tolerance (declared here, matching the RFC's per-entry policy for float64 output):
``|a-b| <= atol + rtol*|b|`` with ``atol=1e-9``, ``rtol=1e-6``. Fill/NaN cells
(``null``) must match element-for-element. ``fill_value`` (0.0) is NOT mapped to NaN.

Usage:
  crosscheck_write.py READDUMP.json [READDUMP.json ...] [--store LABEL=DIR ...]

Exit 0 ⇔ every readback agrees with the oracle and pairwise within tolerance AND
every pair of stores agrees structurally. ≥1 readback dump is required for the
oracle check; pairwise/structural checks activate when ≥2 are present.
"""

from __future__ import annotations

import json
import math
import pathlib
import sys
from typing import Any, Dict, List, Optional, Tuple

ATOL = 1e-9
RTOL = 1e-6

HERE = pathlib.Path(__file__).resolve().parent
SPEC_PATH = HERE / "write_spec.json"

CF_ATTR_KEYS = ("units", "standard_name", "axis", "coordinates", "calendar")


# --------------------------------------------------------------------------- #
# The oracle: expected decoded arrays derived from the shared spec.
# --------------------------------------------------------------------------- #


def build_oracle(spec: Dict[str, Any]) -> Dict[str, Dict[str, Any]]:
    """``{field_name: {dtype, dims, shape, data(flat row-major)}}`` — the exact
    arrays every writer's store must decode back to."""
    dim_len = {d: int(n) for d, n in spec["dims"]}
    time_dim = spec["time_dim"]
    records = spec["records"]
    dim_len[time_dim] = len(records)

    fields: Dict[str, Dict[str, Any]] = {}

    # coordinate variables (1-D over their own dim)
    for co in spec["coords"]:
        name = co["name"]
        if name == time_dim:
            data = [float(r["t"]) for r in records]
        else:
            data = [float(x) for x in co["values"]]
        fields[name] = {
            "dtype": "float64",
            "dims": [name],
            "shape": [len(data)],
            "data": data,
        }

    # streaming variables, assembled record-by-record in file (dims) order
    for v in spec["vars"]:
        name, dims = v["name"], v["dims"]
        shape = [dim_len[d] for d in dims]
        # only (time, lat, lon)-shaped vars are in this spec; assemble row-major
        assert dims == [time_dim, "lat", "lon"], f"oracle assumes time,lat,lon; got {dims}"
        flat: List[float] = []
        for r in records:
            block = r["vars"][name]  # [lat][lon]
            for row in block:
                for x in row:
                    flat.append(float(x))
        fields[name] = {"dtype": "float64", "dims": list(dims), "shape": shape, "data": flat}

    return fields


# --------------------------------------------------------------------------- #
# Value comparison (tolerance for floats; null ↔ null exact).
# --------------------------------------------------------------------------- #


def _values_equal(got: List[Any], exp: List[Any]) -> Optional[str]:
    if len(got) != len(exp):
        return f"length {len(got)} != {len(exp)}"
    for i, (g, e) in enumerate(zip(got, exp)):
        gnull = g is None or (isinstance(g, float) and math.isnan(g))
        enull = e is None or (isinstance(e, float) and math.isnan(e))
        if gnull != enull:
            return f"fill/NaN mask differs at [{i}]: {g!r} vs {e!r}"
        if gnull:
            continue
        if abs(float(g) - float(e)) > ATOL + RTOL * abs(float(e)):
            return (
                f"value differs at [{i}]: {g} != {e} "
                f"(beyond atol={ATOL:g}, rtol={RTOL:g})"
            )
    return None


def _cmp_field(got: Dict[str, Any], exp: Dict[str, Any], label: str) -> List[str]:
    errs: List[str] = []
    if got.get("dtype") != exp["dtype"]:
        errs.append(f"{label}: dtype {got.get('dtype')} != {exp['dtype']}")
    if got.get("dims") != exp["dims"]:
        errs.append(f"{label}: dims {got.get('dims')} != {exp['dims']}")
    if got.get("shape") != exp["shape"]:
        errs.append(f"{label}: shape {got.get('shape')} != {exp['shape']}")
    verr = _values_equal(got.get("data", []), exp["data"])
    if verr:
        errs.append(f"{label}: {verr}")
    return errs


# --------------------------------------------------------------------------- #
# Structural metadata comparison — read each store's zarr.json directly.
# --------------------------------------------------------------------------- #


def _read_array_meta(store_dir: pathlib.Path, name: str) -> Optional[Dict[str, Any]]:
    p = store_dir / name / "zarr.json"
    if not p.exists():
        return None
    return json.loads(p.read_text())


def _structural(meta: Dict[str, Any]) -> Dict[str, Any]:
    """Extract the language-neutral structural fingerprint of an array node."""
    cg = meta.get("chunk_grid", {}).get("configuration", {})
    codecs = meta.get("codecs", [])
    shard = codecs[0] if codecs else {}
    scfg = shard.get("configuration", {})
    inner = scfg.get("chunk_shape")
    blosc = None
    for c in scfg.get("codecs", []):
        if c.get("name") == "blosc":
            bc = c.get("configuration", {})
            blosc = {
                "cname": bc.get("cname"),
                "clevel": bc.get("clevel"),
                "shuffle": bc.get("shuffle"),
            }
    attrs = meta.get("attributes", {}) or {}
    cf = {k: attrs[k] for k in CF_ATTR_KEYS if k in attrs}
    return {
        "data_type": meta.get("data_type"),
        "shape": meta.get("shape"),
        "dimension_names": meta.get("dimension_names"),
        "shard_shape": cg.get("chunk_shape"),
        "inner_chunk_shape": inner,
        "blosc": blosc,
        "fill_value": meta.get("fill_value"),
        "cf_attrs": cf,
    }


def compare_stores(stores: Dict[str, pathlib.Path], spec: Dict[str, Any]) -> Tuple[int, List[str]]:
    """Pairwise structural comparison across writer stores. Returns (failures, log)."""
    log: List[str] = []
    failures = 0
    labels = list(stores)
    arrays = [c["name"] for c in spec["coords"]] + [v["name"] for v in spec["vars"]]

    # group-level attributes
    group_meta = {lab: json.loads((d / "zarr.json").read_text()) for lab, d in stores.items()}
    for i in range(len(labels)):
        for j in range(i + 1, len(labels)):
            la, lb = labels[i], labels[j]
            ga = group_meta[la].get("attributes", {})
            gb = group_meta[lb].get("attributes", {})
            if ga != gb:
                failures += 1
                log.append(f"  FAIL group attrs: {la}={ga} != {lb}={gb}")
            else:
                log.append(f"  group attrs: {la} = {lb} OK")

    for arr in arrays:
        metas = {lab: _read_array_meta(d, arr) for lab, d in stores.items()}
        present = [lab for lab in labels if metas[lab] is not None]
        if len(present) < 2:
            log.append(f"  array {arr}: only {present} present — no structural pair")
            continue
        fps = {lab: _structural(metas[lab]) for lab in present}
        for i in range(len(present)):
            for j in range(i + 1, len(present)):
                la, lb = present[i], present[j]
                diffs = _diff_struct(fps[la], fps[lb])
                if diffs:
                    failures += 1
                    log.append(f"  FAIL structural {arr}: {la} vs {lb}")
                    for d in diffs:
                        log.append(f"      - {d}")
                else:
                    log.append(f"  structural {arr}: {la} = {lb} OK")
    return failures, log


def _diff_struct(a: Dict[str, Any], b: Dict[str, Any]) -> List[str]:
    diffs = []
    for k in ("data_type", "shape", "dimension_names", "shard_shape",
              "inner_chunk_shape", "blosc", "fill_value", "cf_attrs"):
        if a.get(k) != b.get(k):
            diffs.append(f"{k}: {a.get(k)!r} != {b.get(k)!r}")
    return diffs


# --------------------------------------------------------------------------- #
# Driver.
# --------------------------------------------------------------------------- #


def main(argv: List[str]) -> int:
    dump_paths: List[str] = []
    stores: Dict[str, pathlib.Path] = {}
    i = 1
    while i < len(argv):
        a = argv[i]
        if a == "--store":
            i += 1
            label, _, d = argv[i].partition("=")
            stores[label] = pathlib.Path(d).resolve()
        else:
            dump_paths.append(a)
        i += 1

    if not dump_paths and not stores:
        print(__doc__)
        print("error: provide ≥1 readback dump and/or --store entries", file=sys.stderr)
        return 2

    spec = json.loads(SPEC_PATH.read_text())
    oracle = build_oracle(spec)

    dumps = []
    for p in dump_paths:
        d = json.loads(pathlib.Path(p).read_text())
        if d.get("schema") != "earthsciio/write-native-dump/v1":
            print(f"ERROR: {p}: unexpected schema {d.get('schema')!r}")
            return 1
        d["_id"] = f"{d['writer']}-written / {d['reader']}-read"
        dumps.append(d)

    print("=== Cross-language WRITE conformance (streaming-output-sinks Wave 5) ===")
    print(f"spec:      {SPEC_PATH}")
    print(f"readbacks: {', '.join(d['_id'] for d in dumps) if dumps else '(none)'}")
    print(f"stores:    {', '.join(stores) if stores else '(none)'}")
    print(f"tolerance: atol={ATOL:g}, rtol={RTOL:g} (float64 decoded); "
          f"null↔NaN exact; fill_value 0.0 kept (NOT mapped to NaN) [RFC §16.6]")
    print()

    failures = 0

    # 1. decoded-array agreement: each readback vs oracle
    print("--- decoded arrays vs spec oracle ---")
    for d in dumps:
        errs: List[str] = []
        fields = d["fields"]
        for name, exp in oracle.items():
            if name not in fields:
                errs.append(f"{name}: missing from {d['_id']}")
                continue
            errs += _cmp_field(fields[name], exp, name)
        # readback must not invent fields
        for name in fields:
            if name not in oracle:
                errs.append(f"{name}: unexpected field not in spec")
        if errs:
            failures += 1
            print(f"  FAIL vs oracle: {d['_id']}")
            for e in errs:
                print(f"      - {e}")
        else:
            print(f"  vs oracle: {d['_id']} OK ({len(oracle)} fields)")
    print()

    # 1b. decoded-array agreement: pairwise between readbacks
    if len(dumps) >= 2:
        print("--- decoded arrays pairwise (writer/reader agreement) ---")
        for i in range(len(dumps)):
            for j in range(i + 1, len(dumps)):
                da, db = dumps[i], dumps[j]
                fa, fb = da["fields"], db["fields"]
                errs = []
                if set(fa) != set(fb):
                    errs.append(f"field set differs: {sorted(set(fa) ^ set(fb))}")
                for key in sorted(set(fa) & set(fb)):
                    errs += _cmp_field(fa[key], fb[key], key)
                if errs:
                    failures += 1
                    print(f"  FAIL pairwise: {da['_id']} vs {db['_id']}")
                    for e in errs:
                        print(f"      - {e}")
                else:
                    print(f"  pairwise: {da['_id']} = {db['_id']} OK")
        print()

    # 2. structural / CF-metadata agreement across writer stores
    if len(stores) >= 2:
        print("--- structural / CF metadata (zarr.json, read directly) ---")
        sfail, slog = compare_stores(stores, spec)
        for line in slog:
            print(line)
        failures += sfail
        print()
    elif stores:
        print(f"--- structural: only 1 store ({list(stores)}) — no pair to compare ---\n")

    print("=== summary ===")
    print(f"readback dumps: {len(dumps)}   stores: {len(stores)}")
    if failures:
        print(f"RESULT: FAIL ({failures} problem(s))")
        return 1
    print("RESULT: PASS — every writer's store agrees with the spec oracle and "
          "pairwise within tolerance; structural/CF metadata agrees.")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
