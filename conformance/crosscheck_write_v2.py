#!/usr/bin/env python3
"""Comparator for the **v2 write-conformance** corpus (``spec/writer.md`` §10).

Inputs, produced by the per-language drivers over ``write_spec_v2.json``:

* a WRITE result per (writer, case): ``<root>/<writer>/<case>/result.json``
  (the store itself at ``<root>/<writer>/<case>/store/``, whose
  ``output_manifest.json`` is validated against
  ``spec/schemas/output-manifest.schema.json`` when ``jsonschema`` is installed)

    {"case": "<name>", "ops": [{"index": k, "op": "<op>", "ok": true|false,
                                "error": null | "RegionNotShardAligned" |
                                         "CommitError" | "Error",
                                "message": "<text>"}]}

  one entry per op, in script order. An op that raised records ``ok: false``
  and the error kind (``Error`` for anything that is neither named kind).

* a READ dump per (reader, writer): ``earthsciio/write-v2-dump/v1``

    {"schema": "earthsciio/write-v2-dump/v1", "reader": "<lang>",
     "writer": "<lang>",
     "cases": {"<case>": {
        "manifest_schema": "earthsciio/output-manifest/v1" | ".../v2",
        "commit_seq": <int> | null,                 # null for v1
        "committed": [<record index>, ...],         # ascending
        "generations": {"<i>": <int>} | null,       # indexed mode only
        "records": {"<var>": {"<i>": {"dtype": "<dtype>", "shape": [...],
                                      "values": [<C-order flat>]}}},
        "statics": {"<name>": {"dtype": ..., "shape": ..., "values": ...}},
        "blobs": [{"scope": "static"|"record", "record": <int>|null,
                   "writer": <int>, "key": "<key>", "hex": "<bytes>"}]}}}

  ``records`` holds every committed record of every streaming variable and
  ``statics`` every committed static, read whole (region = everything) through
  the reader's read-back API (``read_record`` / ``read_static`` / ``read_blob``).

Every dump is checked against the oracle computed from the case's ``expect``
block and the generator, **bitwise** (all profiles are lossless and dtypes are
preserved, so a float that differs in its last bit is a failure).

Usage:
  crosscheck_write_v2.py --spec write_spec_v2.json --results ROOT DUMP.json...
"""

from __future__ import annotations

import argparse
import json
import pathlib
import sys
from typing import Any, Dict, List

INT_DTYPES = {"int32", "int64", "uint8"}


def gen_value(gen: Dict[str, Any], dtype: str, seed: int, r: int, idx: List[int]):
    P = gen["P"]
    base = seed + gen["record_stride"] * r + sum((i + 1) * P[j] for j, i in enumerate(idx))
    if dtype == "uint8":
        return base % gen["uint8_modulus"]
    if dtype in INT_DTYPES:
        return base
    return base + gen["float_offset"]


def gen_array(gen, dtype, seed, r, shape):
    """C-order flat values over the full extent ``shape`` (time excluded)."""
    out = []

    def rec(prefix):
        if len(prefix) == len(shape):
            out.append(gen_value(gen, dtype, seed, r, prefix))
            return
        for i in range(shape[len(prefix)]):
            rec(prefix + [i])

    rec([])
    return out


def _bits_equal(a, b, dtype) -> bool:
    if dtype in INT_DTYPES:
        return isinstance(a, int) and not isinstance(a, bool) and a == b
    # Every generated float is exact in float32 and float64, so an exact
    # equality on the parsed JSON number IS the bitwise check.
    return isinstance(a, (int, float)) and float(a) == float(b)


def _check_array(label, got, dtype, shape, expected, errs):
    if got is None:
        errs.append(f"{label}: missing")
        return
    if got.get("dtype") != dtype:
        errs.append(f"{label}: dtype {got.get('dtype')!r} != {dtype!r}")
    if list(got.get("shape", [])) != list(shape):
        errs.append(f"{label}: shape {got.get('shape')} != {shape}")
        return
    vals = got.get("values", [])
    if len(vals) != len(expected):
        errs.append(f"{label}: {len(vals)} values != {len(expected)}")
        return
    for k, (a, b) in enumerate(zip(vals, expected)):
        if not _bits_equal(a, b, dtype):
            errs.append(f"{label}: value[{k}] = {a!r}, expected {b!r}")
            return


def _manifest_validator():
    """A validator for ``spec/schemas/output-manifest.schema.json``, or None when
    ``jsonschema`` is not installed (the check is then skipped, not failed)."""
    try:
        import jsonschema
    except ImportError:
        return None
    path = pathlib.Path(__file__).resolve().parent.parent / "spec" / "schemas" / "output-manifest.schema.json"
    return jsonschema.Draft202012Validator(json.loads(path.read_text()))


def check_results(spec, root: pathlib.Path, writer: str) -> List[str]:
    errs = []
    validator = _manifest_validator()
    for case in spec["cases"]:
        mpath = root / writer / case["name"] / "store" / "output_manifest.json"
        if validator is not None and mpath.is_file():
            for e in validator.iter_errors(json.loads(mpath.read_text())):
                errs.append(f"[{writer}/{case['name']}] manifest schema: {e.message}")
                break
        p = root / writer / case["name"] / "result.json"
        if not p.is_file():
            errs.append(f"[{writer}/{case['name']}] no result.json")
            continue
        res = json.loads(p.read_text())
        ops = res.get("ops", [])
        if len(ops) != len(case["ops"]):
            errs.append(
                f"[{writer}/{case['name']}] {len(ops)} op results for {len(case['ops'])} ops"
            )
            continue
        for k, (op, got) in enumerate(zip(case["ops"], ops)):
            want = op.get("expect_error")
            tag = f"[{writer}/{case['name']}] op {k} ({op['op']} {op.get('writer')})"
            if want is None and not got.get("ok"):
                errs.append(f"{tag}: unexpected {got.get('error')}: {got.get('message')}")
            elif want is not None:
                if got.get("ok"):
                    errs.append(f"{tag}: expected {want}, succeeded")
                elif want != "Error" and got.get("error") != want:
                    errs.append(
                        f"{tag}: expected {want}, got {got.get('error')}: {got.get('message')}"
                    )
    return errs


def check_dump(spec, dump) -> List[str]:
    gen = spec["generator"]
    who = f"{dump.get('reader')}<-{dump.get('writer')}"
    errs = []
    if dump.get("schema") != "earthsciio/write-v2-dump/v1":
        return [f"[{who}] bad dump schema {dump.get('schema')!r}"]
    for case in spec["cases"]:
        name = case["name"]
        tag = f"[{who}/{name}]"
        got = dump.get("cases", {}).get(name)
        if got is None:
            errs.append(f"{tag} missing from dump")
            continue
        exp = case["expect"]
        sch = case["schema"]
        dimlen = {d: n for d, n in sch["dims"]}
        if got.get("manifest_schema") != exp["manifest_schema"]:
            errs.append(f"{tag} manifest {got.get('manifest_schema')!r} != {exp['manifest_schema']!r}")
        if "commit_seq" in exp and got.get("commit_seq") != exp["commit_seq"]:
            errs.append(f"{tag} commit_seq {got.get('commit_seq')} != {exp['commit_seq']}")
        want_idx = [e["index"] for e in exp["records"]]
        if list(got.get("committed", [])) != want_idx:
            errs.append(f"{tag} committed {got.get('committed')} != {want_idx}")
        if "generations" in exp:
            g = {str(k): v for k, v in (got.get("generations") or {}).items()}
            for k, v in exp["generations"].items():
                if g.get(k) != v:
                    errs.append(f"{tag} generation[{k}] {g.get(k)} != {v}")
        for v in sch["vars"]:
            shape = [dimlen[d] for d in v["dims"] if d != sch["time_dim"]]
            per = got.get("records", {}).get(v["name"], {})
            for e in exp["records"]:
                want = gen_array(gen, v["dtype"], v["seed"], e["r"], shape)
                _check_array(f"{tag} {v['name']}[{e['index']}]", per.get(str(e["index"])),
                             v["dtype"], shape, want, errs)
            extra = set(per) - {str(i) for i in want_idx}
            if extra:
                errs.append(f"{tag} {v['name']}: uncommitted records dumped: {sorted(extra)}")
        want_statics = set(exp["statics"])
        if set(got.get("statics", {})) != want_statics:
            errs.append(f"{tag} statics {sorted(got.get('statics', {}))} != {sorted(want_statics)}")
        for s in sch.get("statics", []):
            if s["name"] not in want_statics:
                continue
            shape = [dimlen[d] for d in s["dims"]]
            want = gen_array(gen, s["dtype"], s["seed"], 0, shape)
            _check_array(f"{tag} static {s['name']}", got.get("statics", {}).get(s["name"]),
                         s["dtype"], shape, want, errs)
        key = lambda b: (b["scope"], -1 if b["record"] is None else b["record"], b["writer"], b["key"])
        gb = sorted(got.get("blobs", []), key=key)
        eb = sorted(exp["blobs"], key=key)
        if gb != eb:
            errs.append(f"{tag} blobs {gb} != {eb}")
    return errs


def main(argv: List[str]) -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--spec", required=True)
    ap.add_argument("--results", help="root holding <writer>/<case>/result.json")
    ap.add_argument("--writer", action="append", default=[], help="writer labels to check results for")
    ap.add_argument("dumps", nargs="*")
    a = ap.parse_args(argv)
    spec = json.loads(pathlib.Path(a.spec).read_text())

    errs: List[str] = []
    if a.results:
        for w in a.writer:
            errs += check_results(spec, pathlib.Path(a.results), w)
    for d in a.dumps:
        errs += check_dump(spec, json.loads(pathlib.Path(d).read_text()))
    if not a.dumps:
        errs.append("no read dumps given")

    n_cases = len(spec["cases"])
    if errs:
        for e in errs:
            print("FAIL", e)
        print(f"v2 write conformance: {len(errs)} failure(s)")
        return 1
    print(f"v2 write conformance: PASS ({len(a.writer)} writer(s), {len(a.dumps)} dump(s), "
          f"{n_cases} cases each)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
