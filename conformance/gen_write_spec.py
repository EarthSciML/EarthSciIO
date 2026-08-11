#!/usr/bin/env python3
"""Regenerate the shared **write-conformance** input spec (``write_spec.json``).

The spec is a small, deterministic dataset that every language's Zarr v3 sharded
WRITER is driven to emit (streaming-output-sinks RFC, Wave 5). It is language
neutral — plain JSON, values written out in full so no language re-derives them
from a formula — and it is the single source of truth for:

  * the store SHAPE the writers must produce (dims, dim order, chunk/shard grid,
    codec profile, CF coordinate variables + attributes, group attributes);
  * the streaming RECORDS fed to ``write_record`` one time-step at a time;
  * the EXPECTED decoded arrays (the oracle) the readback is checked against.

The dataset is intentionally tiny but exercises the load-bearing features:

  * two spatial dims (``lat`` 3, ``lon`` 4) + a growable ``time`` axis;
  * ``lon`` split into 2 inner chunks per shard, ``time`` sharded 2 records per
    shard object → 4 records ⇒ 2 committed shards + streaming resize;
  * CF coordinate variables (``lat``/``lon``) with ``units``/``standard_name``/
    ``axis`` and a growable ``time`` coordinate with CF time attrs;
  * two float64 variables carrying CF ``units``/``standard_name``/``coordinates``.

Row-major (C order) is the canonical layout: every record's ``vars[name]`` is a
nested ``[lat][lon]`` list; the full variable array is ``[time][lat][lon]``.

**Codec profiles.** The dataset is identical across profiles — only the inner
(per-chunk) compressor differs, so the decoded ORACLE is profile-independent and
one comparator serves every variant. Two variants are generated:

  * ``write_spec.json``      — ``profile: "diagnostic"`` (Blosc(zstd)+shuffle).
  * ``write_spec_wasm.json`` — ``profile: "wasm"`` (plain v3 ``zstd``, NO Blosc),
    proving the wasm-loadable store shape is written identically by every track.

Usage:
  python3 conformance/gen_write_spec.py                 # regenerate BOTH variants
  python3 conformance/gen_write_spec.py --profile wasm  # just the wasm variant
"""

from __future__ import annotations

import json
import pathlib
import sys

HERE = pathlib.Path(__file__).resolve().parent

NLAT, NLON, NREC = 3, 4, 4
LAT_VALUES = [10.0, 20.0, 30.0]
LON_VALUES = [0.0, 1.0, 2.0, 3.0]
DT = 3600.0  # seconds between records


def temperature(r: int, i: int, j: int) -> float:
    return 280.0 + 10.0 * r + i + 0.5 * j


def pressure(r: int, i: int, j: int) -> float:
    return 101325.0 - 100.0 * r + 3.0 * i - 0.25 * j


#: profile name -> (spec filename, one-line description of the inner codec).
PROFILES = {
    "diagnostic": (
        "write_spec.json",
        "inner codec Blosc(cname=zstd, clevel=5, byte-shuffle)",
    ),
    "wasm": (
        "write_spec_wasm.json",
        "inner codec plain Zarr v3 zstd (level 5, no Blosc) so the store is "
        "loadable by a WebAssembly/browser Zarr reader",
    ),
}


def build(profile: str = "diagnostic") -> dict:
    records = []
    for r in range(NREC):
        records.append(
            {
                "t": float(r) * DT,
                "vars": {
                    "temperature": [
                        [temperature(r, i, j) for j in range(NLON)] for i in range(NLAT)
                    ],
                    "pressure": [
                        [pressure(r, i, j) for j in range(NLON)] for i in range(NLAT)
                    ],
                },
            }
        )
    return {
        "schema": "earthsciio/write-conformance-spec/v1",
        "description": (
            "Tiny deterministic Zarr v3 sharded write-conformance dataset: two "
            "float64 variables over (time, lat, lon), CF coordinate variables, "
            "time sharded 2 records/shard, lon split into 2 inner chunks/shard. "
            f"Codec profile {profile!r}: {PROFILES[profile][1]}."
        ),
        "dims": [["time", 0], ["lat", NLAT], ["lon", NLON]],
        "time_dim": "time",
        "time_dtype": "float64",
        "profile": profile,
        "group_attrs": {
            "title": "EarthSciIO write-conformance tiny dataset",
            "Conventions": "CF-1.8",
        },
        "coords": [
            {
                "name": "lat",
                "values": LAT_VALUES,
                "attrs": {
                    "units": "degrees_north",
                    "standard_name": "latitude",
                    "axis": "Y",
                },
            },
            {
                "name": "lon",
                "values": LON_VALUES,
                "attrs": {
                    "units": "degrees_east",
                    "standard_name": "longitude",
                    "axis": "X",
                },
            },
            {
                "name": "time",
                "values": [],  # values come from each record's t
                "attrs": {
                    "units": "seconds since 2020-01-01 00:00:00",
                    "standard_name": "time",
                    "axis": "T",
                    "calendar": "proleptic_gregorian",
                },
            },
        ],
        "vars": [
            {
                "name": "temperature",
                "dims": ["time", "lat", "lon"],
                "dtype": "float64",
                "attrs": {
                    "units": "K",
                    "standard_name": "air_temperature",
                    "coordinates": "lat lon",
                },
            },
            {
                "name": "pressure",
                "dims": ["time", "lat", "lon"],
                "dtype": "float64",
                "attrs": {
                    "units": "Pa",
                    "standard_name": "air_pressure",
                    "coordinates": "lat lon",
                },
            },
        ],
        "chunk_shape": {"time": 1, "lat": NLAT, "lon": 2},
        "shard_shape": {"time": 2, "lat": NLAT, "lon": NLON},
        "records": records,
    }


def main(argv: list[str] | None = None) -> int:
    argv = list(sys.argv[1:] if argv is None else argv)
    if "--profile" in argv:
        name = argv[argv.index("--profile") + 1]
        if name not in PROFILES:
            print(
                f"error: unknown profile {name!r} (expected one of "
                f"{', '.join(PROFILES)})",
                file=sys.stderr,
            )
            return 2
        wanted = [name]
    else:
        wanted = list(PROFILES)

    for name in wanted:
        spec = build(name)
        out = HERE / PROFILES[name][0]
        out.write_text(json.dumps(spec, indent=2) + "\n")
        print(
            f"wrote {out} (profile={name}, {NREC} records, "
            f"{len(spec['vars'])} vars)"
        )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
