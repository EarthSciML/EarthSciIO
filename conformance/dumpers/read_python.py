#!/usr/bin/env python3
"""Python track's READBACK driver for the write-conformance harness.

Reads a produced Zarr v3 sharded store (written by ANY track's writer) with the
**Python store-backed reader** (:class:`earthsciio.backends.zarr.ZarrReader`) —
the same reader the read-conformance harness proves conformant — and emits its
decoded native arrays as a canonical JSON dump (schema
``earthsciio/write-native-dump/v1``). :mod:`conformance.crosscheck_write` then
asserts that every writer's store, decoded here, agrees with the spec oracle and
pairwise within tolerance (RFC §16.6). The write mirror of the read-side dumpers.

The reader is store-backed: it is handed ``(cache, base_url, variables)`` and
fetches each object through a content-addressed cache. Here the "cache" is a
trivial local shim (:class:`_LocalDirCache`) that maps ``<base>/<key>`` straight
to a file on disk — no network, no sha256 indirection — so the real reader code
path (zarr-python 3.x sharding/blosc decode, ``dimension_names``) runs unchanged
over a freshly-written local store.

The dump lists both the schema variables and the coordinate variables (the
store-backed reader returns every requested array as a variable), each encoded as
``{dtype, dims, shape, data}`` with ``data`` flattened row-major (C order).

Usage:  python3 conformance/dumpers/read_python.py STORE_DIR WRITER_LABEL [OUT.json]
"""

from __future__ import annotations

import json
import math
import os
import pathlib
import sys
from typing import Any, Dict, List

import numpy as np

HERE = pathlib.Path(__file__).resolve().parent
CONF = HERE.parent
REPO_ROOT = CONF.parent
sys.path.insert(0, str(REPO_ROOT))

from earthsciio.backends.zarr import ZarrReader  # noqa: E402
from earthsciio.errors import CacheMiss  # noqa: E402


class _Entry:
    def __init__(self, path: str) -> None:
        self.path = path


class _LocalDirCache:
    """A minimal ``cache.fetch(url)`` shim over a local directory tree.

    The reader builds object URLs as ``<base_url>/<key>``; with ``base_url`` set
    to the store directory, each URL is just a filesystem path. A missing object
    raises :class:`CacheMiss` — exactly the "object absent" signal the reader
    turns into ``fill_value`` (never NaN)."""

    def fetch(self, url: str, **_: Any) -> _Entry:
        path = url[len("file://"):] if url.startswith("file://") else url
        if not os.path.exists(path):
            raise CacheMiss(url, "write-conformance-readback")
        return _Entry(path)


def _encode_field(data: np.ndarray, dims: List[str]) -> Dict[str, Any]:
    flat = np.asarray(data).reshape(-1)
    if np.issubdtype(flat.dtype, np.floating):
        dtype = "float64"
        values: List[Any] = [None if math.isnan(x) else float(x) for x in flat.tolist()]
    elif flat.dtype == np.int32:
        dtype = "int32"
        values = [int(x) for x in flat.tolist()]
    else:
        dtype = "int64"
        values = [int(x) for x in flat.tolist()]
    return {"dtype": dtype, "dims": list(dims), "shape": list(data.shape), "data": values}


def main(argv: List[str]) -> int:
    if len(argv) < 3:
        print("usage: read_python.py STORE_DIR WRITER_LABEL [OUT.json]", file=sys.stderr)
        return 2
    store_dir = str(pathlib.Path(argv[1]).resolve())
    writer_label = argv[2]
    spec = json.loads((CONF / "write_spec.json").read_text())

    arrays = [c["name"] for c in spec["coords"]] + [v["name"] for v in spec["vars"]]
    reader = ZarrReader()
    nds = reader.read_store(_LocalDirCache(), store_dir, arrays)

    fields = {n: _encode_field(f.data, f.dims) for n, f in nds.variables.items()}
    out = {
        "schema": "earthsciio/write-native-dump/v1",
        "writer": writer_label,
        "reader": "python",
        "reader_impl": "earthsciio.backends.zarr.ZarrReader",
        "fields": fields,
    }
    text = json.dumps(out, indent=2, sort_keys=True)
    if len(argv) > 3:
        pathlib.Path(argv[3]).write_text(text + "\n")
    else:
        print(text)
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
