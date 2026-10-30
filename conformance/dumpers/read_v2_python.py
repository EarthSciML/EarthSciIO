#!/usr/bin/env python3
"""Python track's READ driver for the **v2 write-conformance** corpus
(``spec/writer.md`` §10).

Reads every ``STORES_ROOT/<case>/store`` — written by ANY track's
``write_v2_*`` driver — through the Python read-back API
(:func:`~earthsciio.backends.zarr_stream.read_output_manifest`,
``read_record``, ``read_static``, ``read_blob``), which needs only standard
Zarr v3 plus the output manifest, and emits an ``earthsciio/write-v2-dump/v1``
dump (format documented at the top of ``conformance/crosscheck_write_v2.py``).
Every committed record of every streaming variable and every committed static
is read whole (region = everything).

Usage:  python3 conformance/dumpers/read_v2_python.py SPEC.json STORES_ROOT WRITER_LABEL OUT_DUMP.json
"""

from __future__ import annotations

import json
import pathlib
import sys
from typing import Any, Dict, List

import numpy as np

HERE = pathlib.Path(__file__).resolve().parent
REPO_ROOT = HERE.parent.parent
sys.path.insert(0, str(REPO_ROOT))

from earthsciio.backends.zarr_stream import (  # noqa: E402
    OUTPUT_MANIFEST_V2,
    committed_records,
    read_blob,
    read_output_manifest,
    read_record,
    read_static,
)


def encode(a: np.ndarray) -> Dict[str, Any]:
    a = np.asarray(a)
    if np.issubdtype(a.dtype, np.floating):
        values = [float(x) for x in a.reshape(-1).tolist()]
    else:
        values = [int(x) for x in a.reshape(-1).tolist()]
    return {"dtype": a.dtype.name, "shape": list(a.shape), "values": values}


def dump_store(base: str) -> Dict[str, Any]:
    m = read_output_manifest(base)
    v2 = m["schema"] == OUTPUT_MANIFEST_V2
    committed = committed_records(m)
    generations = None
    if v2 and m.get("time_mode", {}).get("kind") == "indexed":
        generations = {str(e["index"]): int(e["generation"]) for e in m["records"]["entries"]}
    records = {
        v["name"]: {str(i): encode(read_record(base, v["name"], i)) for i in committed}
        for v in m["vars"]
    }
    statics = {
        s["name"]: encode(read_static(base, s["name"]))
        for s in m.get("statics", [])
        if s["committed"]
    }
    blobs = []
    for b in m.get("blobs", []):
        scope = "static" if b["scope"] == "static" else ("record", int(b["record"]))
        data = read_blob(base, scope, int(b["writer"]), b["key"])
        blobs.append({"scope": b["scope"], "record": b["record"], "writer": int(b["writer"]),
                      "key": b["key"], "hex": data.hex()})
    return {
        "manifest_schema": m["schema"],
        "commit_seq": int(m["commit_seq"]) if v2 else None,
        "committed": committed,
        "generations": generations,
        "records": records,
        "statics": statics,
        "blobs": blobs,
    }


def main(argv: List[str]) -> int:
    if len(argv) != 5:
        print("usage: read_v2_python.py SPEC.json STORES_ROOT WRITER_LABEL OUT_DUMP.json",
              file=sys.stderr)
        return 2
    spec = json.loads(pathlib.Path(argv[1]).read_text())
    root = pathlib.Path(argv[2]).resolve()
    cases: Dict[str, Any] = {}
    for case in spec["cases"]:
        store = root / case["name"] / "store"
        try:
            cases[case["name"]] = dump_store(str(store))
        except Exception as e:  # noqa: BLE001 — a missing case fails the crosscheck
            print(f"[python-reader] {case['name']}: {type(e).__name__}: {e}", file=sys.stderr)
    out = {"schema": "earthsciio/write-v2-dump/v1", "reader": "python",
           "writer": argv[3], "cases": cases}
    pathlib.Path(argv[4]).write_text(json.dumps(out, indent=1) + "\n")
    print(f"[python-reader] dumped {len(cases)}/{len(spec['cases'])} cases from {root}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
