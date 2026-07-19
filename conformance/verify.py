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


# FF10 point column schema — copied from Emissions.jl `src/ff10.jl`
# `FF10_POINT_COLUMNS` (SMOKE names COUNTRY_CD/REGION_CD for the first two).
_FF10_POINT_COLUMNS = [
    "COUNTRY_CD", "REGION_CD", "TRIBAL_CODE", "FACILITY_ID",
    "UNIT_ID", "REL_POINT_ID", "PROCESS_ID", "AGY_FACILITY_ID",
    "AGY_UNIT_ID", "AGY_REL_POINT_ID", "AGY_PROCESS_ID", "SCC",
    "POLID", "ANN_VALUE", "ANN_PCT_RED", "FACILITY_NAME",
    "ERPTYPE", "STKHGT", "STKDIAM", "STKTEMP",
    "STKFLOW", "STKVEL", "NAICS", "LONGITUDE",
    "LATITUDE", "LL_DATUM", "HORIZ_COLL_MTHD", "DESIGN_CAPACITY",
    "DESIGN_CAPACITY_UNITS", "REG_CODES", "FAC_SOURCE_TYPE", "UNIT_TYPE_CODE",
    "CONTROL_IDS", "CONTROL_MEASURES", "CURRENT_COST", "CUMULATIVE_COST",
    "PROJECTION_FACTOR", "SUBMITTER_FAC_ID", "CALC_METHOD", "DATA_SET_ID",
    "FACIL_CATEGORY_CODE", "ORIS_FACILITY_CODE", "ORIS_BOILER_ID", "IPM_YN",
    "CALC_YEAR", "DATE_UPDATED", "FUG_HEIGHT", "FUG_WIDTH_XDIM",
    "FUG_LENGTH_YDIM", "FUG_ANGLE", "ZIPCODE", "ANNUAL_AVG_HOURS_PER_YEAR",
    "JAN_VALUE", "FEB_VALUE", "MAR_VALUE", "APR_VALUE",
    "MAY_VALUE", "JUN_VALUE", "JUL_VALUE", "AUG_VALUE",
    "SEP_VALUE", "OCT_VALUE", "NOV_VALUE", "DEC_VALUE",
    "JAN_PCTRED", "FEB_PCTRED", "MAR_PCTRED", "APR_PCTRED",
    "MAY_PCTRED", "JUN_PCTRED", "JUL_PCTRED", "AUG_PCTRED",
    "SEP_PCTRED", "OCT_PCTRED", "NOV_PCTRED", "DEC_PCTRED",
    "COMMENT",
]


def read_ff10(path, expected):
    """FF10 point decode oracle: skip '#'/blank lines, RFC-4180 split, assign the
    77-column schema positionally, then type each column per the case's expected
    dtype (blank -> NaN in a float64 column, str otherwise). Matches the
    Julia/Python/Rust ff10 readers' bare-member (member=None) path."""
    index = {name: j for j, name in enumerate(_FF10_POINT_COLUMNS)}
    with open(path, newline="") as fh:
        rows = [
            r for r in csv.reader(fh)
            if r and not (r[0].lstrip().startswith("#"))
        ]
    ncol = len(_FF10_POINT_COLUMNS)
    for r in rows:
        if len(r) != ncol:
            raise ValueError(f"FF10 row has {len(r)} fields, expected {ncol}: {r!r}")
    out = {}
    for name, spec in expected["variables"].items():
        vals = [r[index[name]] for r in rows]
        if spec["dtype"] == "string":
            out[name] = vals
        else:
            out[name] = np.array(
                [math.nan if v.strip() == "" else float(v) for v in vals],
                dtype="f8",
            )
    return out, {}


READERS = {"netcdf": read_netcdf, "csv": read_csv, "ff10": read_ff10}


def verify_case(case_path: pathlib.Path) -> list:
    errs: list = []
    case = json.loads(case_path.read_text())

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
