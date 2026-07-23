#!/usr/bin/env python3
"""Python track's WRITE driver for the cross-language write-conformance harness.

Drives the **Python Zarr v3 sharded writer**
(:class:`earthsciio.backends.zarr_write.ZarrWriter`) from the shared, language-
neutral input spec (``conformance/write_spec.json``) and emits a Zarr v3 sharded
store into an output directory. The write mirror of ``dumpers/dump_python.py``
(streaming-output-sinks RFC, Wave 5).

The store this produces is then read back by every available track's reader
(``dumpers/read_python.py`` / ``dumpers/read_julia.jl``) and cross-checked, and
its ``zarr.json`` metadata is structurally compared against the Julia- and Rust-
written stores by :mod:`conformance.crosscheck_write`. Conformance is
**tolerance-based on decoded arrays** (RFC §16.6), never byte identity.

Usage:  python3 conformance/dumpers/write_python.py OUT_DIR [SPEC.json]
"""

from __future__ import annotations

import json
import pathlib
import sys
from typing import Any, Dict, List

import numpy as np

HERE = pathlib.Path(__file__).resolve().parent
CONF = HERE.parent
REPO_ROOT = CONF.parent
sys.path.insert(0, str(REPO_ROOT))

from earthsciio.backends.zarr_write import (  # noqa: E402
    OutputSchema,
    OutputVar,
    ZarrWriter,
)

_NP_DTYPE = {"float64": np.float64, "float32": np.float32, "int32": np.int32,
             "int64": np.int64}


def build_schema(spec: Dict[str, Any]) -> OutputSchema:
    """Map the language-neutral spec onto the Python writer's ``OutputSchema``."""
    coords: List = []
    for co in spec["coords"]:
        vals = np.asarray(co["values"], dtype=np.float64)
        coords.append((co["name"], (vals, dict(co["attrs"]))))
    variables: List = []
    for v in spec["vars"]:
        variables.append(
            (v["name"], OutputVar(v["dims"], _NP_DTYPE[v["dtype"]], dict(v["attrs"])))
        )
    return OutputSchema(
        dims=[(d, int(n)) for d, n in spec["dims"]],
        time_dim=spec["time_dim"],
        vars=variables,
        chunk_shape=dict(spec["chunk_shape"]),
        shard_shape=dict(spec["shard_shape"]),
        coords=coords,
        profile=spec["profile"],
        attrs=dict(spec["group_attrs"]),
        time_dtype=_NP_DTYPE[spec.get("time_dtype", "float64")],
    )


def main(argv: List[str]) -> int:
    if len(argv) < 2:
        print("usage: write_python.py OUT_DIR [SPEC.json]", file=sys.stderr)
        return 2
    out_dir = pathlib.Path(argv[1]).resolve()
    spec_path = pathlib.Path(argv[2]) if len(argv) > 2 else CONF / "write_spec.json"
    spec = json.loads(spec_path.read_text())

    schema = build_schema(spec)
    writer = ZarrWriter()
    handle = writer.write_open(str(out_dir), schema)

    for rec in spec["records"]:
        t = float(rec["t"])
        arrays = {
            name: np.asarray(block, dtype=np.float64)
            for name, block in rec["vars"].items()
        }
        writer.write_record(handle, t, arrays)
    manifest = writer.write_close(handle)

    print(
        f"[python-writer] wrote {manifest['total_records']} records to {out_dir} "
        f"(profile={spec['profile']}, {len(spec['vars'])} vars)"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
