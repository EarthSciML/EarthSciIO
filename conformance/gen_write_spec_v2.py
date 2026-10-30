#!/usr/bin/env python3
"""Regenerate the **v2 write-conformance** corpus (``write_spec_v2.json``).

The v1 corpus (``write_spec.json``) is one dataset streamed by one writer. The v2
corpus (``spec/writer.md`` §10) is a list of CASES, each a stream schema plus a
script of writer operations — create / join / write / flush / commit / resume /
close — that every language's driver executes against a fresh store. It covers
the RFC U12 surface: regions and the commit protocol, indexed records, statics,
blobs, dtypes and resume.

Data values are not stored: every driver computes them from the generator in
``spec/writer.md`` §10 (``value(seed, r, idx)``), so the corpus stays small and
a region-shaped array is just the generator evaluated over that region. The
values are exact in every dtype, so the comparator checks them **bitwise**.

Each case carries ``expect``: what the committed state of the store must be once
the script has run (records and the generator record each one holds, statics,
blobs, manifest label, ``commit_seq``, and in indexed mode each record's
generation).

Usage:
  python3 conformance/gen_write_spec_v2.py
"""

from __future__ import annotations

import copy
import json
import pathlib

HERE = pathlib.Path(__file__).resolve().parent

DT = 3600.0
NLAT, NLON = 6, 5

#: The region grid used by the multi-writer cases. lat shards are 2 long, lon
#: shards 4 long, so lon's last block [4, 5) ends at the array end rather than
#: on a shard boundary — the "stop == len" arm of the alignment rule.
LAT_A, LAT_B = [0, 4], [4, 6]
LON_A, LON_B = [0, 4], [4, 5]


def base_schema(*, vars_=None, statics=None, time_mode=None, profile="diagnostic"):
    if vars_ is None:
        vars_ = [
            {
                "name": "temperature",
                "dims": ["time", "lat", "lon"],
                "dtype": "float64",
                "seed": 100,
                "attrs": {"units": "K", "standard_name": "air_temperature"},
            },
            {
                "name": "pressure",
                "dims": ["time", "lat", "lon"],
                "dtype": "float32",
                "seed": 200,
                "attrs": {"units": "Pa"},
            },
            # Only one of the 2x2 case's two region dims: partitioned, supplied
            # by every writer, written only by the lat-start-0 writers (§4).
            {
                "name": "zonal_mean",
                "dims": ["time", "lon"],
                "dtype": "float64",
                "seed": 400,
                "attrs": {"units": "K"},
            },
            # No lat/lon: replicated in every multi-writer case (owner-only).
            {
                "name": "global_mean",
                "dims": ["time"],
                "dtype": "float64",
                "seed": 300,
                "attrs": {"units": "K"},
            },
        ]
    return {
        "dims": [["time", 0], ["lat", NLAT], ["lon", NLON]],
        "time_dim": "time",
        "chunk_shape": {"time": 1, "lat": 1, "lon": 2},
        "shard_shape": {"time": 2, "lat": 2, "lon": 4},
        "coords": [
            {
                "name": "lat",
                "values": [float(10 * i) for i in range(NLAT)],
                "attrs": {"units": "degrees_north", "standard_name": "latitude"},
            },
            {
                "name": "lon",
                "values": [float(i) for i in range(NLON)],
                "attrs": {"units": "degrees_east", "standard_name": "longitude"},
            },
            {
                "name": "time",
                "values": [],
                "attrs": {"units": "seconds since 2020-01-01 00:00:00"},
            },
        ],
        "vars": vars_,
        "statics": statics or [],
        "time_mode": time_mode or {"kind": "append"},
        "group_attrs": {"title": "EarthSciIO write-conformance v2"},
        "profile": profile,
    }


def rec(w, r):
    return {"op": "write_record", "writer": w, "t": DT * r, "r": r}


def rec_at(w, i, r):
    return {"op": "write_record_at", "writer": w, "index": i, "r": r}


def flush(w, tok):
    return {"op": "flush", "writer": w, "token": tok}


def records(pairs):
    """``[(index, r), ...]`` -> the ``expect.records`` list."""
    return [{"index": i, "r": r} for i, r in pairs]


def expect(recs, *, schema="v2", commit_seq=None, statics=(), blobs=(), generations=None):
    e = {
        "manifest_schema": f"earthsciio/output-manifest/{schema}",
        "records": records(recs),
        "statics": list(statics),
        "blobs": list(blobs),
    }
    if commit_seq is not None:
        e["commit_seq"] = commit_seq
    if generations is not None:
        e["generations"] = {str(k): v for k, v in generations.items()}
    return e


QUAD = [
    ("w0", {"owner": 4}, {"lat": LAT_A, "lon": LON_A}),
    ("w1", {"contributor": 1}, {"lat": LAT_A, "lon": LON_B}),
    ("w2", {"contributor": 2}, {"lat": LAT_B, "lon": LON_A}),
    ("w3", {"contributor": 3}, {"lat": LAT_B, "lon": LON_B}),
]


def open_quad():
    ops = []
    for w, role, region in QUAD:
        op = "create" if "owner" in role else "join"
        ops.append({"op": op, "writer": w, "role": role, "region": region})
    return ops


def cases():
    out = []

    # 1. region-tiling: a single writer and 2x2 region writers produce the same
    #    committed arrays. Both are checked against one oracle, so they are equal.
    single = [{"op": "create", "writer": "w0", "role": "single", "region": {}}]
    single += [rec("w0", r) for r in range(3)]
    single += [flush("w0", "a")]
    single += [rec("w0", r) for r in range(3, 5)]
    single += [{"op": "close", "writer": "w0", "tokens": []}]
    out.append(
        {
            "name": "region-tiling-single",
            "schema": base_schema(),
            "ops": single,
            "expect": expect([(i, i) for i in range(5)], schema="v1"),
        }
    )

    quad = open_quad()
    for r in range(3):
        quad += [rec(w, r) for w, _, _ in QUAD]
    quad += [flush(w, f"{w}a") for w, _, _ in QUAD]
    quad += [{"op": "commit", "writer": "w0", "tokens": [f"{w}a" for w, _, _ in QUAD]}]
    for r in range(3, 5):
        quad += [rec(w, r) for w, _, _ in QUAD]
    quad += [flush(w, f"{w}b") for w, _, _ in QUAD[1:]]
    quad += [{"op": "close", "writer": w, "tokens": []} for w, _, _ in QUAD[1:]]
    quad += [{"op": "close", "writer": "w0", "tokens": [f"{w}b" for w, _, _ in QUAD[1:]]}]
    out.append(
        {
            "name": "region-tiling-2x2",
            "schema": base_schema(),
            "ops": quad,
            "expect": expect([(i, i) for i in range(5)], commit_seq=2),
        }
    )

    # 2. misaligned regions are refused at open, before any I/O.
    out.append(
        {
            "name": "region-misaligned",
            "schema": base_schema(),
            "ops": [
                {
                    "op": "create",
                    "writer": "bad",
                    "role": {"owner": 2},
                    "region": {"lat": [0, 3]},
                    "expect_error": "RegionNotShardAligned",
                },
                {"op": "create", "writer": "w0", "role": {"owner": 2}, "region": {"lat": [0, 2]}},
                {
                    "op": "join",
                    "writer": "w1",
                    "role": {"contributor": 1},
                    "region": {"lat": [1, 6]},
                    "expect_error": "RegionNotShardAligned",
                },
                {
                    "op": "join",
                    "writer": "w2",
                    "role": {"contributor": 1},
                    "region": {"lon": [2, 5]},
                    "expect_error": "RegionNotShardAligned",
                },
            ],
            "expect": expect([], commit_seq=0),
        }
    )

    # 3. a gap and an overlap are CommitErrors; the committed prefix (one good
    #    commit of record 0) is unchanged.
    for name, r1 in (("region-gap", [4, 6]), ("region-overlap", [0, 6])):
        ops = [
            {"op": "create", "writer": "w0", "role": {"owner": 2}, "region": {"lat": [0, 2]}},
            {"op": "join", "writer": "w1", "role": {"contributor": 1}, "region": {"lat": [2, 6]}},
            rec("w0", 0),
            rec("w1", 0),
            flush("w0", "a0"),
            flush("w1", "a1"),
            {"op": "commit", "writer": "w0", "tokens": ["a0", "a1"]},
            {"op": "join", "writer": "w9", "role": {"contributor": 1}, "region": {"lat": r1}},
            rec("w0", 1),
            rec("w9", 1),
            flush("w0", "b0"),
            flush("w9", "b1"),
            {"op": "commit", "writer": "w0", "tokens": ["b0", "b1"], "expect_error": "CommitError"},
        ]
        out.append(
            {
                "name": name,
                "schema": base_schema(),
                "ops": ops,
                "expect": expect([(0, 0)], commit_seq=1),
            }
        )

    # 4. indexed records written in descending order equal forward append.
    n = 5
    times = [DT * i for i in range(n)]
    ops = [{"op": "create", "writer": "w0", "role": "single", "region": {}}]
    ops += [rec_at("w0", i, i) for i in reversed(range(n))]
    ops += [{"op": "close", "writer": "w0", "tokens": []}]
    out.append(
        {
            "name": "indexed-reverse",
            "schema": base_schema(time_mode={"kind": "indexed", "times": times}),
            "ops": ops,
            "expect": expect(
                [(i, i) for i in range(n)], commit_seq=1, generations={i: 1 for i in range(n)}
            ),
        }
    )

    # ... and the same with two region writers, committed in two batches, the
    # second batch holding the earlier indices (reverse order across commits).
    ops = [
        {"op": "create", "writer": "w0", "role": {"owner": 2}, "region": {"lat": LAT_A}},
        {"op": "join", "writer": "w1", "role": {"contributor": 1}, "region": {"lat": LAT_B}},
    ]
    for i in (4, 3):
        ops += [rec_at("w0", i, i), rec_at("w1", i, i)]
    ops += [
        flush("w0", "a0"),
        flush("w1", "a1"),
        {"op": "commit", "writer": "w0", "tokens": ["a0", "a1"]},
    ]
    for i in (2, 1, 0):
        ops += [rec_at("w0", i, i), rec_at("w1", i, i)]
    ops += [
        flush("w1", "b1"),
        {"op": "close", "writer": "w1", "tokens": []},
        {"op": "close", "writer": "w0", "tokens": ["b1"]},
    ]
    out.append(
        {
            "name": "indexed-reverse-regions",
            "schema": base_schema(time_mode={"kind": "indexed", "times": times}),
            "ops": ops,
            "expect": expect(
                [(i, i) for i in range(n)], commit_seq=2, generations={i: 1 for i in range(n)}
            ),
        }
    )

    # 5. indexed ring: rewriting a committed record needs invalidate first.
    ops = [
        {"op": "create", "writer": "w0", "role": "single", "region": {}},
        rec_at("w0", 0, 0),
        rec_at("w0", 1, 1),
        flush("w0", "a"),
        {**rec_at("w0", 0, 7), "expect_error": "Error"},
        {"op": "invalidate", "writer": "w0", "indices": [0]},
        rec_at("w0", 0, 7),
        flush("w0", "b"),
        {"op": "close", "writer": "w0", "tokens": []},
    ]
    out.append(
        {
            "name": "indexed-invalidate",
            "schema": base_schema(time_mode={"kind": "indexed", "times": [0.0, DT]}),
            "ops": ops,
            # commit a (1), invalidate (2), commit b (3); close has nothing pending.
            "expect": expect([(0, 7), (1, 1)], commit_seq=3, generations={0: 2, 1: 1}),
        }
    )

    # 6. flush mid-shard, drop every handle uncommitted, resume, continue.
    ops = [{"op": "create", "writer": "w0", "role": "single", "region": {}}]
    ops += [rec("w0", r) for r in range(3)] + [flush("w0", "a")]
    ops += [rec("w0", 3), {"op": "drop", "writer": "w0"}]
    ops += [{"op": "resume", "writer": "w0b", "role": "single", "region": {}, "at": None}]
    ops += [rec("w0b", r) for r in range(3, 5)]
    ops += [{"op": "close", "writer": "w0b", "tokens": []}]
    out.append(
        {
            "name": "partial-shard-resume",
            "schema": base_schema(),
            "ops": ops,
            "expect": expect([(i, i) for i in range(5)], schema="v1"),
        }
    )

    ops = [
        {"op": "create", "writer": "w0", "role": {"owner": 2}, "region": {"lat": LAT_A}},
        {"op": "join", "writer": "w1", "role": {"contributor": 1}, "region": {"lat": LAT_B}},
    ]
    for r in range(3):
        ops += [rec("w0", r), rec("w1", r)]
    ops += [
        flush("w0", "a0"),
        flush("w1", "a1"),
        {"op": "commit", "writer": "w0", "tokens": ["a0", "a1"]},
    ]
    ops += [
        rec("w0", 3),
        rec("w1", 3),
        flush("w0", "x0"),
        flush("w1", "x1"),
        {"op": "drop", "writer": "w0"},
        {"op": "drop", "writer": "w1"},
    ]
    ops += [
        {
            "op": "resume",
            "writer": "v0",
            "role": {"owner": 2},
            "region": {"lat": LAT_A},
            "at": None,
        },
        {
            "op": "resume",
            "writer": "v1",
            "role": {"contributor": 1},
            "region": {"lat": LAT_B},
            "at": None,
        },
    ]
    for r in range(3, 5):
        ops += [rec("v0", r), rec("v1", r)]
    ops += [
        flush("v1", "b1"),
        {"op": "close", "writer": "v1", "tokens": []},
        {"op": "close", "writer": "v0", "tokens": ["b1"]},
    ]
    out.append(
        {
            "name": "partial-shard-resume-regions",
            "schema": base_schema(),
            "ops": ops,
            # commit (1), close-commit (2); a resume that truncates nothing is not a commit
            "expect": expect([(i, i) for i in range(5)], commit_seq=2),
        }
    )

    # 7. truncate-on-resume: records after `at` are discarded; the re-emitted
    #    record 3 holds new data.
    ops = [{"op": "create", "writer": "w0", "role": "single", "region": {}}]
    ops += [rec("w0", r) for r in range(5)]
    ops += [flush("w0", "a"), {"op": "drop", "writer": "w0"}]
    ops += [{"op": "resume", "writer": "w1", "role": "single", "region": {}, "at": 2 * DT}]
    ops += [{"op": "write_record", "writer": "w1", "t": 3 * DT, "r": 13}]
    ops += [{"op": "close", "writer": "w1", "tokens": []}]
    out.append(
        {
            "name": "truncate-on-resume",
            "schema": base_schema(),
            "ops": ops,
            "expect": expect([(0, 0), (1, 1), (2, 2), (3, 13)], schema="v1"),
        }
    )

    # 8. dtypes round-trip bitwise.
    dvars = [
        {"name": f"v_{dt}", "dims": ["time", "lat", "lon"], "dtype": dt, "seed": 10 * k + 1}
        for k, dt in enumerate(["float64", "float32", "int32", "int64", "uint8"])
    ]
    ops = [{"op": "create", "writer": "w0", "role": "single", "region": {}}]
    ops += [rec("w0", r) for r in range(3)]
    ops += [{"op": "close", "writer": "w0", "tokens": []}]
    out.append(
        {
            "name": "dtypes",
            "schema": base_schema(vars_=dvars),
            "ops": ops,
            "expect": expect([(i, i) for i in range(3)], schema="v1"),
        }
    )

    # 9. statics and blobs over two region writers.
    statics = [
        {
            "name": "dJ_dfield",
            "dims": ["lat", "lon"],
            "dtype": "float64",
            "seed": 500,
            "attrs": {"long_name": "gradient wrt field"},
        },
        {"name": "dJ_dlon", "dims": ["lon"], "dtype": "float64", "seed": 600},
        {"name": "step_count", "dims": ["lat", "lon"], "dtype": "int64", "seed": 700},
    ]
    ops = [
        {"op": "create", "writer": "w0", "role": {"owner": 2}, "region": {"lat": LAT_A}},
        {"op": "join", "writer": "w1", "role": {"contributor": 1}, "region": {"lat": LAT_B}},
        {"op": "write_static", "writer": "w1", "name": "dJ_dlon", "expect_error": "Error"},
        rec("w0", 0),
        rec("w1", 0),
        rec("w0", 1),
        rec("w1", 1),
        {"op": "write_static", "writer": "w0", "name": "dJ_dfield"},
        {"op": "write_static", "writer": "w1", "name": "dJ_dfield"},
        {"op": "write_static", "writer": "w0", "name": "dJ_dlon"},
        {"op": "write_static", "writer": "w0", "name": "step_count"},
        {"op": "write_static", "writer": "w1", "name": "step_count"},
        {
            "op": "write_blob",
            "writer": "w0",
            "scope": "static",
            "record": None,
            "key": "rng",
            "hex": "00010203",
        },
        {
            "op": "write_blob",
            "writer": "w1",
            "scope": "static",
            "record": None,
            "key": "rng",
            "hex": "ff10",
        },
        {
            "op": "write_blob",
            "writer": "w1",
            "scope": "record",
            "record": 1,
            "key": "ctrl.state",
            "hex": "deadbeef",
        },
        flush("w1", "a1"),
        {"op": "close", "writer": "w1", "tokens": []},
        {"op": "close", "writer": "w0", "tokens": ["a1"]},
    ]
    out.append(
        {
            "name": "statics-and-blobs",
            "schema": base_schema(statics=statics),
            "ops": ops,
            "expect": expect(
                [(0, 0), (1, 1)],
                commit_seq=1,
                statics=["dJ_dfield", "dJ_dlon", "step_count"],
                blobs=[
                    {
                        "scope": "record",
                        "record": 1,
                        "writer": 1,
                        "key": "ctrl.state",
                        "hex": "deadbeef",
                    },
                    {
                        "scope": "static",
                        "record": None,
                        "writer": 0,
                        "key": "rng",
                        "hex": "00010203",
                    },
                    {"scope": "static", "record": None, "writer": 1, "key": "rng", "hex": "ff10"},
                ],
            ),
        }
    )

    # 10. a single-writer append store still writes the v1 manifest.
    ops = [{"op": "create", "writer": "w0", "role": "single", "region": {}}]
    ops += [rec("w0", r) for r in range(3)]
    ops += [{"op": "close", "writer": "w0", "tokens": []}]
    out.append(
        {
            "name": "manifest-v1-compat",
            "schema": base_schema(),
            "ops": ops,
            "expect": expect([(i, i) for i in range(3)], schema="v1"),
        }
    )

    for c in out:
        c["schema"] = copy.deepcopy(c["schema"])
    return out


def build() -> dict:
    return {
        "schema": "earthsciio/write-conformance-spec/v2",
        "description": (
            "RFC U12 writer-contract cases (spec/writer.md §10): regions and the "
            "commit protocol, indexed records, statics, blobs, dtypes, resume. "
            "Values come from the generator base = seed + 13*r + sum_j (i_j+1)*P[j], "
            "P = [3, 5, 11, 17, 23]; floats add 0.25, uint8 takes base % 251."
        ),
        "generator": {
            "P": [3, 5, 11, 17, 23],
            "record_stride": 13,
            "float_offset": 0.25,
            "uint8_modulus": 251,
        },
        "cases": cases(),
    }


def main() -> int:
    spec = build()
    path = HERE / "write_spec_v2.json"
    path.write_text(json.dumps(spec, indent=1) + "\n")
    print(f"wrote {path} ({len(spec['cases'])} cases)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
