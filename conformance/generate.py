#!/usr/bin/env python3
"""Reproducible generator for the EarthSciIO conformance corpus.

This script is the single source of truth for the golden fixtures under
``conformance/corpus/``. It writes, deterministically:

  * the cached *blobs* (a tiny real NetCDF-3 grid file + a CSV points file),
    laid out exactly as a populated ``$EARTHSCIDATADIR`` cache
    (``cache/v1/blobs/<key[:2]>/<key>.<ext>``) so a provider in offline mode
    can be pointed straight at ``corpus/cache`` and find every blob by hashing
    its resolved URL;
  * the per-blob *manifests* (``cache/v1/meta/<key>.json``);
  * the language-neutral conformance *cases* (``corpus/cases/*.json``) carrying
    the expected CF-decoded native arrays + coordinates;
  * the case index (``corpus/cases.json``).

Determinism: the NetCDF blob is written as ``NETCDF3_CLASSIC`` (no embedded
HDF5 timestamps/UUIDs), all data values are fixed, and ``fetched_at`` in the
manifests is a pinned constant. Re-running this script on the same numpy /
netCDF4 stack reproduces byte-identical blobs. Conformance readers consume the
*committed* blobs, so other language tracks need no Python at all.

Run from anywhere:  ``python3 conformance/generate.py``

Spec references: ../spec/cache-format.md, ../spec/conformance.md,
../spec/schemas/manifest.schema.json, ../spec/schemas/cache-case.schema.json.
"""

from __future__ import annotations

import csv
import hashlib
import io
import json
import os
import pathlib

import numpy as np
from netCDF4 import Dataset

# --- spec constants (keep in sync with ../spec/cache-format.md) ---------------
CACHE_FORMAT_VERSION = "v1"
# Pinned so manifests are byte-stable across regenerations (never "now").
FIXED_FETCHED_AT = "2026-06-26T00:00:00Z"

HERE = pathlib.Path(__file__).resolve().parent
CORPUS = HERE / "corpus"
CACHE_ROOT = CORPUS / "cache" / CACHE_FORMAT_VERSION
CASES_DIR = CORPUS / "cases"


def cache_key(resolved_url: str) -> str:
    """The shared cache key: sha256 of the resolved URL, lowercase hex.

    The URL is encoded as UTF-8 with no trailing newline, exactly as resolved
    (after time-anchor + parameter expansion). This MUST be identical across
    Python / Julia / Rust so a file fetched by one language is reused by the
    others. See ../spec/cache-format.md#1-cache-key.
    """
    return hashlib.sha256(resolved_url.encode("utf-8")).hexdigest()


def sha256_bytes(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def blob_relpath(key: str, ext: str) -> str:
    return f"cache/{CACHE_FORMAT_VERSION}/blobs/{key[:2]}/{key}.{ext}"


def meta_relpath(key: str) -> str:
    return f"cache/{CACHE_FORMAT_VERSION}/meta/{key}.json"


def write_blob(key: str, ext: str, data: bytes) -> str:
    rel = blob_relpath(key, ext)
    path = CORPUS / rel
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(data)
    return rel


def write_manifest(key: str, manifest: dict) -> str:
    rel = meta_relpath(key)
    path = CORPUS / rel
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(manifest, indent=2, sort_keys=True) + "\n")
    return rel


def write_json(path: pathlib.Path, obj: dict) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(obj, indent=2) + "\n")


# -----------------------------------------------------------------------------
# Fixture 1 — ERA5-like NetCDF grid sub-tile (transport=file, format=netcdf).
#
# Exercises the cross-language CF-decode parity risk directly: one *packed*
# variable (int16 + scale_factor/add_offset/_FillValue) and one plain float64
# variable, a CF time axis (hours since ... + calendar). The "native array" a
# conformant reader returns is the CF-DECODED value as float64, keyed by the
# on-disk file_variable name. Variable-name remap + unit_conversion are NOT the
# reader's job (they stay in ESS) — see ../spec/conformance.md#decode.
# -----------------------------------------------------------------------------
def build_era5_netcdf() -> tuple[bytes, dict, dict]:
    lat = np.array([40.0, 39.5, 39.0], dtype="f8")        # N->S, ERA5 order
    lon = np.array([-122.0, -121.5, -121.0], dtype="f8")  # Camp Fire vicinity
    time = np.array([0, 1], dtype="i4")  # hours since 2018-11-08 00:00:00

    # t2m: target DECODED values (Kelvin). Packed as int16 with these CF attrs.
    scale_factor = 0.01
    add_offset = 280.0
    fill_short = np.int16(-32767)
    t2m_decoded = np.array(
        [
            [[282.50, 282.75, 283.00],
             [283.25, 283.50, 283.75],
             [284.00, 284.25, 284.50]],
            [[282.60, 282.85, 283.10],
             [283.35, 283.60, 283.85],
             [284.10, 284.35, np.nan]],  # one masked cell -> _FillValue on disk
        ],
        dtype="f8",
    )
    # Pack to int16 exactly as CF specifies: raw = round((value - off) / scale).
    raw = np.empty(t2m_decoded.shape, dtype="i2")
    mask = np.isnan(t2m_decoded)
    raw[~mask] = np.round((t2m_decoded[~mask] - add_offset) / scale_factor).astype("i2")
    raw[mask] = fill_short

    # sp: plain float64 surface pressure (Pa), no packing, no fills.
    sp = np.array(
        [
            [[100000.0, 100100.0, 100200.0],
             [100300.0, 100400.0, 100500.0],
             [100600.0, 100700.0, 100800.0]],
            [[100050.0, 100150.0, 100250.0],
             [100350.0, 100450.0, 100550.0],
             [100650.0, 100750.0, 100850.0]],
        ],
        dtype="f8",
    )

    buf = io.BytesIO()
    # netCDF4 needs a filename; write to a temp path then read bytes back so the
    # committed artifact is exactly what lands on disk.
    tmp = CORPUS / ".tmp_era5.nc"
    tmp.parent.mkdir(parents=True, exist_ok=True)
    ds = Dataset(tmp, "w", format="NETCDF3_CLASSIC")
    ds.createDimension("time", None)  # record dim
    ds.createDimension("latitude", lat.size)
    ds.createDimension("longitude", lon.size)

    vlat = ds.createVariable("latitude", "f8", ("latitude",))
    vlat.units = "degrees_north"
    vlat.standard_name = "latitude"
    vlat[:] = lat

    vlon = ds.createVariable("longitude", "f8", ("longitude",))
    vlon.units = "degrees_east"
    vlon.standard_name = "longitude"
    vlon[:] = lon

    vtime = ds.createVariable("time", "i4", ("time",))
    vtime.units = "hours since 2018-11-08 00:00:00"
    vtime.calendar = "gregorian"
    vtime.standard_name = "time"
    vtime[:] = time

    vt2m = ds.createVariable("t2m", "i2", ("time", "latitude", "longitude"),
                             fill_value=fill_short)
    # Write the already-packed int16 verbatim: disable netCDF4's auto pack/mask
    # so our hand-computed raw values (and the -32767 fill cell) land as-is. CF
    # decoding happens on READ (xarray / NCDatasets / netcdf-rs), not on write.
    vt2m.set_auto_maskandscale(False)
    vt2m.scale_factor = scale_factor
    vt2m.add_offset = add_offset
    vt2m.units = "K"
    vt2m.long_name = "2 metre temperature"
    vt2m[:] = raw

    vsp = ds.createVariable("sp", "f8", ("time", "latitude", "longitude"))
    vsp.units = "Pa"
    vsp.long_name = "Surface pressure"
    vsp[:] = sp

    ds.close()
    data = tmp.read_bytes()
    tmp.unlink()
    buf.write(data)

    # Expected native arrays = CF-decoded float64 (fill -> null/NaN).
    def f64(a):
        return [[[None if np.isnan(v) else round(float(v), 10) for v in row]
                 for row in slab] for slab in a]

    expected = {
        "variables": {
            "t2m": {
                "dtype": "float64",
                "dims": ["time", "latitude", "longitude"],
                "shape": [2, 3, 3],
                "fill_value": None,
                "data": f64(t2m_decoded),
            },
            "sp": {
                "dtype": "float64",
                "dims": ["time", "latitude", "longitude"],
                "shape": [2, 3, 3],
                "fill_value": None,
                "data": f64(sp),
            },
        },
        "coords": {
            "latitude": {"dtype": "float64", "data": [round(float(v), 10) for v in lat]},
            "longitude": {"dtype": "float64", "data": [round(float(v), 10) for v in lon]},
            "time": {
                "dtype": "int32",
                "units": "hours since 2018-11-08 00:00:00",
                "calendar": "gregorian",
                "data": [int(v) for v in time],
            },
        },
    }
    decode = {
        "scale_factor_offset": True,
        "fill_to_nan": True,
        "time_decoded": False,  # raw hours retained; calendar decoding is ESS's job
    }
    return data, expected, decode


# -----------------------------------------------------------------------------
# Fixture 2 — OpenAQ-like CSV points slice (transport=file, format=csv).
#
# Demonstrates a SECOND reader plugging into the FORMAT registry and yielding
# native 1-D arrays. Contract: numeric columns -> float64 arrays keyed by
# column (file_variable) name; non-numeric columns -> string arrays. Row
# filtering / variable remap are higher layers (ESS), not the reader.
# -----------------------------------------------------------------------------
def build_openaq_csv() -> tuple[bytes, dict, dict]:
    rows = [
        ("location_id", "datetime", "latitude", "longitude", "parameter", "value", "unit"),
        ("1", "2018-11-08T00:00:00Z", "39.76", "-121.62", "pm25", "152.3", "ug/m3"),
        ("1", "2018-11-08T01:00:00Z", "39.76", "-121.62", "pm25", "168.7", "ug/m3"),
        ("2", "2018-11-08T00:00:00Z", "39.50", "-121.50", "pm25", "98.1", "ug/m3"),
        ("2", "2018-11-08T01:00:00Z", "39.50", "-121.50", "pm25", "110.4", "ug/m3"),
    ]
    sio = io.StringIO()
    w = csv.writer(sio, lineterminator="\n")
    for r in rows:
        w.writerow(r)
    data = sio.getvalue().encode("utf-8")

    numeric = {"latitude", "longitude", "value"}
    header = rows[0]
    body = rows[1:]
    variables = {}
    for j, col in enumerate(header):
        vals = [r[j] for r in body]
        if col in numeric:
            variables[col] = {
                "dtype": "float64",
                "dims": ["index"],
                "shape": [len(body)],
                "fill_value": None,
                "data": [round(float(v), 10) for v in vals],
            }
        else:
            variables[col] = {
                "dtype": "string",
                "dims": ["index"],
                "shape": [len(body)],
                "fill_value": None,
                "data": list(vals),
            }
    expected = {"variables": variables, "coords": {}}
    decode = {"delimiter": ",", "header_row": 0, "numeric_columns": sorted(numeric)}
    return data, expected, decode


# -----------------------------------------------------------------------------
# Fixture 3 — FF10 point long-format slice (transport=file, format=ff10).
#
# A NEW reader (the generic CSV reader skips only empty lines, not '#' comments).
# Pins: the '#' header block is skipped; the fixed 77-column FF10_POINT schema is
# applied positionally (data rows carry no clean header row); the 42 numeric
# columns -> float64 (blank -> NaN), the other 35 ids/codes/free-text -> string
# (blank -> ""); RFC-4180 quoting so a FACILITY_NAME can embed a comma. It is
# READER-ONLY: no pollutant pivot (3 rows share one stack, differing only in
# POLID/ANN_VALUE), no unit conversion (STKHGT stays feet, STKTEMP °F), no
# FIPS/SCC normalization, no EGU filter — those move downstream into the .esm.
#
# Column names copied from Emissions.jl `src/ff10.jl` `FF10_POINT_COLUMNS`, with
# the SMOKE FF10_POINT spec names COUNTRY_CD / REGION_CD for the first two
# (Emissions.jl: COUNTRY / FIPS — identical values, positional alias).
# -----------------------------------------------------------------------------
FF10_POINT_COLUMNS = [
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
FF10_POINT_NUMERIC = {
    "ANN_VALUE", "ANN_PCT_RED", "STKHGT", "STKDIAM", "STKTEMP", "STKFLOW",
    "STKVEL", "LONGITUDE", "LATITUDE", "DESIGN_CAPACITY", "CURRENT_COST",
    "CUMULATIVE_COST", "PROJECTION_FACTOR", "FUG_HEIGHT", "FUG_WIDTH_XDIM",
    "FUG_LENGTH_YDIM", "FUG_ANGLE", "ANNUAL_AVG_HOURS_PER_YEAR",
    "JAN_VALUE", "FEB_VALUE", "MAR_VALUE", "APR_VALUE", "MAY_VALUE", "JUN_VALUE",
    "JUL_VALUE", "AUG_VALUE", "SEP_VALUE", "OCT_VALUE", "NOV_VALUE", "DEC_VALUE",
    "JAN_PCTRED", "FEB_PCTRED", "MAR_PCTRED", "APR_PCTRED", "MAY_PCTRED",
    "JUN_PCTRED", "JUL_PCTRED", "AUG_PCTRED", "SEP_PCTRED", "OCT_PCTRED",
    "NOV_PCTRED", "DEC_PCTRED",
}


def build_ff10_point() -> tuple[bytes, dict, dict]:
    def row(**over) -> list:
        r = {c: "" for c in FF10_POINT_COLUMNS}
        r.update(over)
        return [r[c] for c in FF10_POINT_COLUMNS]

    # One stack (facility F001, unit U1, point R1, process P1) emitting THREE
    # pollutants (NOX/SO2/PM25) — identical stack params, distinct POLID/ANN_VALUE
    # (the reader must NOT pivot/aggregate). Row 1 has a quoted-comma FACILITY_NAME
    # and one non-blank monthly value; DESIGN_CAPACITY is blank (numeric -> NaN).
    stack = dict(
        COUNTRY_CD="US", REGION_CD="01001", FACILITY_ID="F001", UNIT_ID="U1",
        REL_POINT_ID="R1", PROCESS_ID="P1", SCC="0030700101",
        FACILITY_NAME="Autauga Plant, Unit 1", ERPTYPE="01",
        STKHGT="100.0", STKDIAM="5.0", STKTEMP="500.0", STKFLOW="25.0",
        STKVEL="12.5", NAICS="221112", LONGITUDE="-86.51045", LATITUDE="32.43878",
        LL_DATUM="NAD83", ZIPCODE="36066",
    )
    rows = [
        row(**stack, POLID="NOX", ANN_VALUE="123.45", JAN_VALUE="10.0",
            CALC_YEAR="2016", DATE_UPDATED="20130210"),
        row(**stack, POLID="SO2", ANN_VALUE="67.89"),
        row(**stack, POLID="PM25", ANN_VALUE="4.2"),
        # A second facility: plain (unquoted) FACILITY_NAME, ZIPCODE "00000".
        row(COUNTRY_CD="US", REGION_CD="01001", FACILITY_ID="F002", UNIT_ID="U9",
            REL_POINT_ID="R9", PROCESS_ID="P9", SCC="0030700201",
            POLID="NOX", ANN_VALUE="8.0", FACILITY_NAME="Prattville Facility",
            ERPTYPE="01", STKHGT="50.0", STKDIAM="2.5", STKTEMP="350.0",
            STKFLOW="10.0", STKVEL="6.0", NAICS="221112", LONGITUDE="-86.40000",
            LATITUDE="32.50000", LL_DATUM="NAD83", ZIPCODE="00000"),
    ]

    header_lines = ["#FORMAT=FF10_POINT", "#COUNTRY US", "#YEAR 2016",
                    "#DESC synthetic conformance fixture"]
    sio = io.StringIO()
    w = csv.writer(sio, lineterminator="\n")  # quotes the comma-bearing name
    for r in rows:
        w.writerow(r)
    data = ("".join(ln + "\n" for ln in header_lines) + sio.getvalue()).encode("utf-8")

    nrows = len(rows)
    variables = {}
    for j, col in enumerate(FF10_POINT_COLUMNS):
        vals = [r[j] for r in rows]
        if col in FF10_POINT_NUMERIC:
            variables[col] = {
                "dtype": "float64", "dims": ["index"], "shape": [nrows],
                "fill_value": None,
                "data": [None if str(v).strip() == "" else round(float(v), 10)
                         for v in vals],
            }
        else:
            variables[col] = {
                "dtype": "string", "dims": ["index"], "shape": [nrows],
                "fill_value": None, "data": [str(v) for v in vals],
            }
    expected = {"variables": variables, "coords": {}}
    decode = {"kind": "point", "member": None, "delimiter": ",", "comment": "#",
              "numeric_columns": sorted(FF10_POINT_NUMERIC)}
    return data, expected, decode


def emit_case(case_id, *, loader, kind, fmt, transport, store, resolved_url,
              ext, data, expected, decode, select, notes):
    key = cache_key(resolved_url)
    content_sha = sha256_bytes(data)
    blob_rel = write_blob(key, ext, data)
    manifest = {
        "schema": "earthsciio/manifest/v1",
        "url": resolved_url,
        "etag": None,
        "last_modified": None,
        "sha256_content": content_sha,
        "bytes": len(data),
        "fetched_at": FIXED_FETCHED_AT,
        "source_loader": loader,
        "auth_realm": None,
    }
    meta_rel = write_manifest(key, manifest)
    case = {
        "schema": "earthsciio/cache-case/v1",
        "id": case_id,
        "loader": loader,
        "kind": kind,
        "format": fmt,
        "transport": transport,
        "store": store,
        "resolved_url": resolved_url,
        "cache_key": key,
        "blob_path": blob_rel,
        "manifest_path": meta_rel,
        "content_sha256": content_sha,
        "bytes": len(data),
        "select": select,
        "decode": decode,
        "expected": expected,
        "notes": notes,
    }
    write_json(CASES_DIR / f"{case_id}.json", case)
    return key, content_sha, len(data), blob_rel


def main() -> None:
    summary = []

    nc_data, nc_expected, nc_decode = build_era5_netcdf()
    summary.append(("era5-grid-sub-tile",) + emit_case(
        "era5-grid-sub-tile",
        loader="era5", kind="grid", fmt="netcdf", transport="file", store="local",
        resolved_url="https://data.earthsci.dev/era5/2018/11/20181108.nc",
        ext="nc", data=nc_data, expected=nc_expected, decode=nc_decode,
        select={"all_records": True},
        notes=("ERA5-like 2x3x3 sub-tile. t2m is int16-packed (scale_factor/"
               "add_offset/_FillValue) -> decoded float64; one masked cell. sp "
               "is plain float64. Pins CF scale/offset/fill decode parity."),
    ))

    csv_data, csv_expected, csv_decode = build_openaq_csv()
    summary.append(("openaq-points-slice",) + emit_case(
        "openaq-points-slice",
        loader="openaq", kind="points", fmt="csv", transport="file", store="local",
        resolved_url="https://openaq-data-archive.s3.amazonaws.com/records/openaq/locationid=1/2018-11-08.csv",
        ext="csv", data=csv_data, expected=csv_expected, decode=csv_decode,
        select={"all_rows": True},
        notes=("OpenAQ-like points CSV. Numeric columns -> float64 1-D arrays; "
               "others -> string arrays. Second reader behind the FORMAT "
               "registry; proves a non-NetCDF format plugs in unchanged."),
    ))

    ff10_data, ff10_expected, ff10_decode = build_ff10_point()
    summary.append(("ff10-point-slice",) + emit_case(
        "ff10-point-slice",
        loader="nei2016", kind="points", fmt="ff10", transport="file", store="local",
        resolved_url="https://gaftp.epa.gov/air/emismod/2016/v1/2016fd/point/ff10_point.csv",
        ext="csv", data=ff10_data, expected=ff10_expected, decode=ff10_decode,
        select={"all_rows": True},
        notes=("FF10 point long-format slice (NEI 2016). '#' header skipped; fixed "
               "77-col FF10_POINT schema applied positionally; 42 numeric cols -> "
               "float64 (blank->NaN), 35 ids/codes/text -> string; RFC-4180 quoted "
               "FACILITY_NAME with an embedded comma. 3 rows share one stack, "
               "differing only in POLID/ANN_VALUE (reader-only: no pivot/convert/"
               "filter). member=null decodes the bare extracted CSV member."),
    ))

    index = {
        "schema": "earthsciio/cases-index/v1",
        "cache_format_version": CACHE_FORMAT_VERSION,
        "cache_root": f"cache/{CACHE_FORMAT_VERSION}",
        "cases": [
            {"id": cid, "file": f"cases/{cid}.json", "cache_key": key,
             "blob_path": blob_rel}
            for (cid, key, _sha, _n, blob_rel) in summary
        ],
    }
    write_json(CORPUS / "cases.json", index)

    print(f"cache format: {CACHE_FORMAT_VERSION}   fetched_at(pinned): {FIXED_FETCHED_AT}")
    for cid, key, sha, nbytes, blob_rel in summary:
        print(f"  {cid:24s} key={key}  content_sha256={sha[:16]}…  {nbytes:>6d} B")
        print(f"  {'':24s} blob={blob_rel}")


if __name__ == "__main__":
    main()
