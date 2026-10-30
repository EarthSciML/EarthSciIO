#!/usr/bin/env python3
"""Python track's WRITE driver for the **v2 write-conformance** corpus
(``conformance/write_spec_v2.json``, ``spec/writer.md`` §10).

Executes every case's script of writer operations (create / join / resume /
write_record / write_record_at / write_static / write_blob / flush / commit /
invalidate / close / drop) against a fresh store ``OUT_ROOT/<case>/store/``
through :class:`earthsciio.backends.zarr_write.ZarrWriter`, and records each
op's outcome in ``OUT_ROOT/<case>/result.json`` (format documented at the top
of ``conformance/crosscheck_write_v2.py``). Data arrays come from the corpus
generator evaluated over the writer's region (global indices). Tokens are
moved through their JSON wire form, as a caller would move them.

Usage:  python3 conformance/dumpers/write_v2_python.py SPEC.json OUT_ROOT
"""

from __future__ import annotations

import json
import pathlib
import shutil
import sys
from typing import Any, Dict, List, Optional

import numpy as np

HERE = pathlib.Path(__file__).resolve().parent
REPO_ROOT = HERE.parent.parent
sys.path.insert(0, str(REPO_ROOT))

from earthsciio.backends.zarr_stream import (  # noqa: E402
    CommitToken,
    IndexedTime,
    OutputSchema,
    OutputVar,
    ZarrWriter,
)
from earthsciio.errors import CommitError, RegionNotShardAligned  # noqa: E402


def build_schema(s: Dict[str, Any]) -> OutputSchema:
    """Map a case's language-neutral schema onto ``OutputSchema``."""
    tm = s.get("time_mode", {"kind": "append"})
    return OutputSchema(
        dims=[(d, int(n)) for d, n in s["dims"]],
        time_dim=s["time_dim"],
        vars=[(v["name"], OutputVar(v["dims"], v["dtype"], v.get("attrs", {}))) for v in s["vars"]],
        chunk_shape=dict(s["chunk_shape"]),
        shard_shape=dict(s["shard_shape"]),
        coords=[
            (c["name"], (np.asarray(c["values"], dtype=np.float64), dict(c["attrs"])))
            for c in s["coords"]
        ],
        profile=s["profile"],
        attrs=dict(s.get("group_attrs", {})),
        statics=[
            (v["name"], OutputVar(v["dims"], v["dtype"], v.get("attrs", {})))
            for v in s.get("statics", [])
        ],
        time_mode=IndexedTime(tm["times"]) if tm["kind"] == "indexed" else "append",
    )


def gen_array(
    gen: Dict[str, Any], dtype: str, seed: int, r: int, ranges: List[range]
) -> np.ndarray:
    """The generator (``spec/writer.md`` §10) over global index ``ranges``."""
    P = gen["P"]
    shape = tuple(len(rg) for rg in ranges)
    base = np.full(shape, seed + gen["record_stride"] * r, dtype=np.int64)
    for j, rg in enumerate(ranges):
        ax = np.asarray(rg, dtype=np.int64) + 1
        base = base + (ax * P[j]).reshape([-1 if k == j else 1 for k in range(len(shape))])
    if dtype == "uint8":
        return (base % gen["uint8_modulus"]).astype(np.uint8)
    if dtype in ("int32", "int64"):
        return base.astype(dtype)
    return (base.astype(np.float64) + gen["float_offset"]).astype(dtype)


class Writer:
    """A driver-side handle: the ZarrWriter handle plus what the driver needs
    to build arrays (role and region)."""

    def __init__(self, h, role: str, region: Dict[str, List[int]]) -> None:
        self.h = h
        self.role = role
        self.region = region


def parse_role(role: Any):
    if role == "single":
        return dict(role="single")
    ((k, v),) = role.items()
    if k == "owner":
        return dict(role="owner", n_writers=int(v))
    if k == "contributor":
        return dict(role="contributor", id=int(v))
    raise ValueError(f"unknown role {role!r}")


def run_case(case: Dict[str, Any], gen: Dict[str, Any], out: pathlib.Path) -> Dict[str, Any]:
    sch = case["schema"]
    schema = build_schema(sch)
    dimlen = {d: int(n) for d, n in sch["dims"]}
    store = out / "store"
    if store.exists():
        shutil.rmtree(store)
    base = str(store)
    zw = ZarrWriter()
    handles: Dict[str, Writer] = {}
    tokens: Dict[str, str] = {}
    times = sch.get("time_mode", {}).get("times")

    def arrays_for(w: Writer, entries, r: int, *, only_partitioned: bool) -> Dict[str, np.ndarray]:
        out_: Dict[str, np.ndarray] = {}
        for v in entries:
            sdims = [d for d in v["dims"] if d != sch["time_dim"]]
            part = any(d in w.region for d in sdims)
            if only_partitioned and not part:
                continue
            ranges = [range(*w.region[d]) if d in w.region else range(dimlen[d]) for d in sdims]
            out_[v["name"]] = gen_array(gen, v["dtype"], v["seed"], r, ranges)
        return out_

    def run(op: Dict[str, Any]) -> None:
        kind = op["op"]
        name = op.get("writer")
        if kind in ("create", "join"):
            kw = parse_role(op["role"])
            region = {d: tuple(rg) for d, rg in op["region"].items()}
            h = zw.write_open(base, schema, region=region, **kw)
            handles[name] = Writer(h, kw["role"], op["region"])
        elif kind == "resume":
            kw = parse_role(op["role"])
            region = {d: tuple(rg) for d, rg in op["region"].items()}
            h = zw.write_resume(base, region=region, at=op.get("at"), **kw)
            handles[name] = Writer(h, kw["role"], op["region"])
        else:
            w = handles[name]
            contributor = w.role == "contributor"
            if kind == "write_record":
                zw.write_record(
                    w.h,
                    float(op["t"]),
                    arrays_for(w, sch["vars"], op["r"], only_partitioned=contributor),
                )
            elif kind == "write_record_at":
                i = int(op["index"])
                zw.write_record_at(
                    w.h,
                    i,
                    float(times[i]),
                    arrays_for(w, sch["vars"], op["r"], only_partitioned=contributor),
                )
            elif kind == "write_static":
                st = [s for s in sch["statics"] if s["name"] == op["name"]]
                arr = arrays_for(w, st, 0, only_partitioned=False)[op["name"]]
                zw.write_static(w.h, op["name"], arr)
            elif kind == "write_blob":
                scope = "static" if op["scope"] == "static" else ("record", int(op["record"]))
                zw.write_blob(w.h, scope, op["key"], bytes.fromhex(op["hex"]))
            elif kind == "flush":
                tokens[op["token"]] = zw.write_flush(w.h).to_json()
            elif kind == "commit":
                zw.write_commit(w.h, [CommitToken.from_json(tokens[t]) for t in op["tokens"]])
            elif kind == "invalidate":
                zw.write_invalidate(w.h, op["indices"])
            elif kind == "close":
                zw.write_close(w.h, [CommitToken.from_json(tokens[t]) for t in op["tokens"]])
            elif kind == "drop":
                del handles[name]
            else:
                raise ValueError(f"unknown op {kind!r}")

    results = []
    for k, op in enumerate(case["ops"]):
        err: Optional[str] = None
        msg = ""
        try:
            run(op)
        except RegionNotShardAligned as e:
            err, msg = "RegionNotShardAligned", str(e)
        except CommitError as e:
            err, msg = "CommitError", str(e)
        except Exception as e:  # noqa: BLE001 — every other failure is "Error"
            err, msg = "Error", f"{type(e).__name__}: {e}"
        results.append(
            {"index": k, "op": op["op"], "ok": err is None, "error": err, "message": msg}
        )
    return {"case": case["name"], "ops": results}


def main(argv: List[str]) -> int:
    if len(argv) != 3:
        print("usage: write_v2_python.py SPEC.json OUT_ROOT", file=sys.stderr)
        return 2
    spec = json.loads(pathlib.Path(argv[1]).read_text())
    root = pathlib.Path(argv[2]).resolve()
    n_bad = 0
    for case in spec["cases"]:
        out = root / case["name"]
        out.mkdir(parents=True, exist_ok=True)
        res = run_case(case, spec["generator"], out)
        (out / "result.json").write_text(json.dumps(res, indent=1) + "\n")
        unexpected = [
            r for r, op in zip(res["ops"], case["ops"]) if r["ok"] == ("expect_error" in op)
        ]
        n_bad += bool(unexpected)
        for r in unexpected:
            print(
                f"[python-writer] {case['name']} op {r['index']} ({r['op']}): "
                f"ok={r['ok']} {r['error'] or ''} {r['message']}",
                file=sys.stderr,
            )
    print(
        f"[python-writer] ran {len(spec['cases'])} cases into {root} "
        f"({n_bad} with unexpected outcomes)"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
