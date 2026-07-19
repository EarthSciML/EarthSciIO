#!/usr/bin/env python3
"""Python track's native-array dumper for the cross-language conformance harness.

Drives the **Python Provider** (:class:`earthsciio.Provider`) over every committed
corpus case, fully OFFLINE (the cache is rooted at the corpus and refuses the
network), and emits the decoded native arrays as a canonical JSON dump. The
cross-language comparator (:mod:`conformance.crosscheck`) diffs this dump against
the Julia and Rust dumps and the corpus oracle to prove array equality across all
three tracks (``esio-9nb.9``).

This is the **reference dumper**: the Julia (``dump_julia.jl``) and Rust
(``rust/examples/conformance_dump.rs``) dumpers emit the *same* schema.

Dump schema — ``earthsciio/native-dump/v1`` (see ``conformance/CROSSLANG.md``):

    {
      "schema": "earthsciio/native-dump/v1",
      "language": "python",
      "provider": "earthsciio.Provider",
      "readers": ["csv", "netcdf"],          # active format names this track has
      "cases": {
        "<case_id>": {
          "format": "netcdf",
          "status": "decoded",
          "variables": {"<name>": {"dtype","dims","shape","data"}},
          "coords":    {"<name>": {"dtype","dims","shape","data","units?","calendar?"}}
        },
        "<case_id>": {"format":"csv","status":"skipped","reason":"..."}  # no reader
      }
    }

``data`` is the field flattened **row-major (C order)** per ``shape``; a masked /
``_FillValue`` cell is ``null`` (== NaN); strings are emitted verbatim. A case
whose ``format`` has no reader in this track is ``status="skipped"`` (explicit,
never silently dropped) so the comparator can tell a real coverage gap from a bug.

Usage:  python3 conformance/dumpers/dump_python.py [out.json]   # default: stdout
"""

from __future__ import annotations

import json
import math
import pathlib
import sys
from typing import Any, Dict, List

import numpy as np

HERE = pathlib.Path(__file__).resolve().parent
CORPUS = HERE.parent / "corpus"
REPO_ROOT = HERE.parent.parent

# Run standalone (no install needed) by putting the repo root on the path, so the
# harness driver can invoke this from anywhere — mirrors verify.py's offline ethos.
sys.path.insert(0, str(REPO_ROOT))

from earthsciio import Cache, DataLoader, Provider  # noqa: E402
from earthsciio.registry import format_registry  # noqa: E402


def _dtype_str(arr: np.ndarray) -> str:
    """The native-field schema dtype name for a numeric numpy array."""
    if np.issubdtype(arr.dtype, np.floating):
        return "float64"
    if arr.dtype == np.int32:
        return "int32"
    if np.issubdtype(arr.dtype, np.integer):
        return "int64"
    raise TypeError(f"unexpected numeric dtype {arr.dtype}")


def _encode_field(field: Any) -> Dict[str, Any]:
    """Encode a :class:`earthsciio.native.NativeField` to the dump schema.

    Numeric arrays flatten row-major with ``null`` for NaN; string columns become
    a flat list of ``str``. ``dims``/``shape`` are carried in file order.
    """
    data = field.data
    dims = list(field.dims)
    if isinstance(data, np.ndarray):
        flat = np.asarray(data).reshape(-1)
        shape = list(data.shape)
        if np.issubdtype(flat.dtype, np.floating):
            values: List[Any] = [
                None if (math.isnan(x)) else float(x) for x in flat.tolist()
            ]
        else:
            values = [int(x) for x in flat.tolist()]
        return {"dtype": _dtype_str(flat), "dims": dims, "shape": shape, "data": values}
    # string column: a plain Python list of str
    values = [str(x) for x in data]
    return {"dtype": "string", "dims": dims, "shape": [len(values)], "data": values}


def _encode_coord(field: Any) -> Dict[str, Any]:
    """A coord is a field plus the CF ``units``/``calendar`` it carries (if any)."""
    enc = _encode_field(field)
    for k in ("units", "calendar"):
        if k in field.attrs:
            enc[k] = str(field.attrs[k])
    return enc


def dump_case(case: Dict[str, Any]) -> Dict[str, Any]:
    """Run the Python Provider over one corpus case and encode its native arrays.

    Skips (without error) a case whose ``format`` has no registered reader, so the
    harness reports the gap instead of failing — matching the Rust track, which
    ships ``netcdf`` only.
    """
    fmt = case["format"]
    if fmt not in format_registry or format_registry.status(fmt) != "active":
        return {
            "format": fmt,
            "status": "skipped",
            "reason": f"no active reader registered for format '{fmt}' in the Python track",
        }

    # An OFFLINE cache rooted at the corpus: every case resolves from disk by its
    # sha256(resolved_url) key; a network attempt would raise (verify=True checks
    # the blob against the manifest on read).
    cache = Cache(root=CORPUS / "cache", offline=True, verify=True)

    reader_kwargs: Dict[str, Any] = {}
    if fmt == "csv":
        # numeric_columns is REQUIRED by the loader spec (digit-only text columns
        # like location_id must stay strings); the corpus case pins the list.
        reader_kwargs["numeric_columns"] = list(case["decode"]["numeric_columns"])
    elif fmt == "ff10":
        # FF10 point: the case pins the 42 numeric columns, the schema kind, and
        # member=null (the committed fixture is the already-extracted CSV member).
        reader_kwargs["numeric_columns"] = list(case["decode"]["numeric_columns"])
        reader_kwargs["kind"] = case["decode"].get("kind", "point")
        reader_kwargs["member"] = case["decode"].get("member")

    loader = DataLoader(
        name=case["loader"],
        format=fmt,
        url=case["resolved_url"],
        reader_kwargs=reader_kwargs,
    )
    provider = Provider(loader, cache)
    nds = provider.materialize()  # CONST: read the single corpus blob once

    return {
        "format": fmt,
        "status": "decoded",
        "variables": {n: _encode_field(f) for n, f in nds.variables.items()},
        "coords": {n: _encode_coord(f) for n, f in nds.coords.items()},
    }


def main(argv: List[str]) -> int:
    index = json.loads((CORPUS / "cases.json").read_text())
    cases: Dict[str, Any] = {}
    for entry in index["cases"]:
        case = json.loads((CORPUS / entry["file"]).read_text())
        cases[case["id"]] = dump_case(case)

    out = {
        "schema": "earthsciio/native-dump/v1",
        "language": "python",
        "provider": "earthsciio.Provider",
        "readers": sorted(
            k for k in format_registry.keys() if format_registry.status(k) == "active"
        ),
        "cases": cases,
    }
    text = json.dumps(out, indent=2, sort_keys=True)
    if len(argv) > 1:
        pathlib.Path(argv[1]).write_text(text + "\n")
    else:
        print(text)
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
