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
import itertools
import json
import os
import pathlib

import numpy as np

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
    # A bare key (no extension, e.g. a Zarr chunk/metadata object) is stored
    # WITHOUT a trailing dot, matching the real LocalStore (glob lookup is by
    # <key>*, so the suffix is human-debug only).
    suffix = f".{ext}" if ext else ""
    return f"cache/{CACHE_FORMAT_VERSION}/blobs/{key[:2]}/{key}{suffix}"


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
    from netCDF4 import Dataset  # lazy: only the netcdf fixture needs it

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
# Fixture 3 — synthetic Zarr v2 store (transport=s3, format=zarr, store=local).
#
# A tiny multi-chunk Zarr v2 store that pins the load-bearing capability: LAZY
# orthogonal selection on an arbitrary dimension driven by a runtime index list —
# fetch ONLY the chunk objects the selection intersects, never the whole array
# (the ISRM workflow depends on this). `field3d` is [2,5,4] chunked [1,2,4] so
# dim1 has a PARTIAL EDGE CHUNK (5 % 2 = 1, fill-padded), and `pop1d` is a 1-D
# single-chunk array. All arrays: blosc {cname lz4, clevel 5, shuffle 1}, order
# C, fill_value 0.0, zarr_format 2, dimension_separator null (-> "."). Each
# .zarray/.zattrs/chunk is its OWN object with its OWN URL, keyed by
# sha256(object_url) — so "lazy partial read" is just fetching a subset of small
# whole objects through the existing content-addressed cache.
#
# The committed blosc bytes are produced with the same c-blosc (numcodecs) the
# readers decode with, so they decode identically in all three tracks.
# -----------------------------------------------------------------------------
ZARR_BASE_URL = "s3://earthsci-fixtures/isrm-mini.zarr"

# The orthogonal selection the tile case exercises (a single selection applied to
# each array whose rank matches its axis count; other-rank arrays read whole):
#   field3d (ndim 3): layer=[1], y=[1,4], x=all -> only chunks {1}x{0,2}x{0}
#     = field3d/1.0.0 and field3d/1.2.0 (2 of 6). Skips ALL layer-0 chunks and
#     the middle y-chunk field3d/1.1.0 — the laziness contract, verified.
#   pop1d   (ndim 1): rank != 3 -> read whole.
ZARR_SELECT = {"axes": [{"indices": [1]}, {"indices": [1, 4]}, "all"]}


def _blosc_codec():
    """The pinned numcodecs Blosc codec (matches the .zarray compressor)."""
    import numcodecs

    return numcodecs.Blosc(
        cname="lz4", clevel=5, shuffle=numcodecs.Blosc.SHUFFLE, blocksize=0
    )


def _zarr_compressor():
    return {"id": "blosc", "cname": "lz4", "clevel": 5, "shuffle": 1, "blocksize": 0}


def _zarray_meta(shape, chunks, dtype):
    return {
        "zarr_format": 2,
        "shape": list(shape),
        "chunks": list(chunks),
        "dtype": dtype,
        "compressor": _zarr_compressor(),
        "fill_value": 0.0,
        "order": "C",
        "filters": None,
        "dimension_separator": None,
    }


def _zarr_chunks(shape, chunks, np_dtype, value_fn, fill=0.0):
    """Every chunk of an array as ``(chunk_key, encoded_bytes)``.

    Edge chunks are stored FULL-SIZE, fill-padded (the Zarr v2 contract) so the
    decompressed length is always ``prod(chunks)``. Bytes are C-order, blosc-lz4
    with the shuffle filter, exactly what the readers decode.
    """
    codec = _blosc_codec()
    ndim = len(shape)
    nchunks = [-(-shape[d] // chunks[d]) for d in range(ndim)]  # ceil-div
    out = []
    for cidx in itertools.product(*[range(n) for n in nchunks]):
        chunk = np.full(chunks, fill, dtype=np_dtype)
        for local in itertools.product(*[range(chunks[d]) for d in range(ndim)]):
            g = tuple(cidx[d] * chunks[d] + local[d] for d in range(ndim))
            if all(g[d] < shape[d] for d in range(ndim)):
                chunk[local] = value_fn(g)
        enc = bytes(codec.encode(np.ascontiguousarray(chunk)))
        out.append((".".join(str(c) for c in cidx), enc))
    return out


def build_zarr_store():
    """Return ``(objects, expected)`` for the synthetic Zarr v2 store.

    ``objects`` is a list of ``(relative_object_path, bytes)`` where the path is
    ``<array>/<name>`` (``.zarray``/``.zattrs``/``<chunk_key>``). ``expected`` is
    the sub-selected native arrays the tile case pins.
    """
    objects = []

    # field3d: [2,5,4] chunked [1,2,4], <f4. value = layer*100 + y*10 + x.
    f3_shape, f3_chunks = (2, 5, 4), (1, 2, 4)
    objects.append(("field3d/.zarray",
                    json.dumps(_zarray_meta(f3_shape, f3_chunks, "<f4"),
                               sort_keys=True).encode("utf-8")))
    objects.append(("field3d/.zattrs",
                    json.dumps({"_ARRAY_DIMENSIONS": ["layer", "y", "x"]},
                               sort_keys=True).encode("utf-8")))
    for key, enc in _zarr_chunks(f3_shape, f3_chunks, np.dtype("<f4"),
                                 lambda g: g[0] * 100 + g[1] * 10 + g[2]):
        objects.append((f"field3d/{key}", enc))

    # pop1d: [8] chunked [8], <f8. value = 2*i + 1.
    p_shape, p_chunks = (8,), (8,)
    objects.append(("pop1d/.zarray",
                    json.dumps(_zarray_meta(p_shape, p_chunks, "<f8"),
                               sort_keys=True).encode("utf-8")))
    objects.append(("pop1d/.zattrs",
                    json.dumps({"_ARRAY_DIMENSIONS": ["cell"]},
                               sort_keys=True).encode("utf-8")))
    for key, enc in _zarr_chunks(p_shape, p_chunks, np.dtype("<f8"),
                                 lambda g: 2 * g[0] + 1):
        objects.append((f"pop1d/{key}", enc))

    # Expected sub-selected arrays (float64; fill_value 0.0 is NOT mapped to NaN).
    expected = {
        "variables": {
            "field3d": {
                "dtype": "float64",
                "dims": ["layer", "y", "x"],
                "shape": [1, 2, 4],
                "fill_value": None,
                "data": [[[110.0, 111.0, 112.0, 113.0],
                          [140.0, 141.0, 142.0, 143.0]]],
            },
            "pop1d": {
                "dtype": "float64",
                "dims": ["cell"],
                "shape": [8],
                "fill_value": None,
                "data": [1.0, 3.0, 5.0, 7.0, 9.0, 11.0, 13.0, 15.0],
            },
        },
        "coords": {},
    }
    return objects, expected


def emit_zarr_case(case_id, *, loader, base_url, objects, variables, expected,
                   decode, select, notes):
    """Write every store object as a cache blob + manifest, then the case JSON.

    A Zarr case's ``blob_path``/``content_sha256``/``bytes``/``cache_key`` anchor
    on the primary array's ``.zarray`` object; the full per-object key/integrity
    table is the additive ``objects`` array (each ``{url, cache_key, blob_path,
    content_sha256, bytes}``), which the runner verifies per object.
    """
    obj_records = []
    primary = None
    for rel, data in objects:
        url = f"{base_url}/{rel}"
        key = cache_key(url)
        content_sha = sha256_bytes(data)
        blob_rel = write_blob(key, "", data)  # bare-key blob (found by <key> glob)
        manifest = {
            "schema": "earthsciio/manifest/v1",
            "url": url,
            "etag": None,
            "last_modified": None,
            "sha256_content": content_sha,
            "bytes": len(data),
            "fetched_at": FIXED_FETCHED_AT,
            "source_loader": loader,
            "auth_realm": None,
        }
        write_manifest(key, manifest)
        rec = {"url": url, "cache_key": key, "blob_path": blob_rel,
               "content_sha256": content_sha, "bytes": len(data)}
        obj_records.append(rec)
        if rel.endswith(f"{variables[0]}/.zarray"):
            primary = rec

    assert primary is not None, "primary .zarray object not found among store objects"
    case = {
        "schema": "earthsciio/cache-case/v1",
        "id": case_id,
        "loader": loader,
        "kind": "grid",
        "format": "zarr",
        "transport": "s3",
        "store": "local",
        "resolved_url": base_url,
        "cache_key": primary["cache_key"],
        "blob_path": primary["blob_path"],
        "manifest_path": meta_relpath(primary["cache_key"]),
        "content_sha256": primary["content_sha256"],
        "bytes": primary["bytes"],
        "variables": list(variables),
        "objects": obj_records,
        "select": select,
        "decode": decode,
        "expected": expected,
        "notes": notes,
    }
    write_json(CASES_DIR / f"{case_id}.json", case)
    return (primary["cache_key"], primary["content_sha256"], primary["bytes"],
            primary["blob_path"])


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

    zarr_objects, zarr_expected = build_zarr_store()
    summary.append(("isrm-zarr-tile",) + emit_zarr_case(
        "isrm-zarr-tile",
        loader="isrm", base_url=ZARR_BASE_URL, objects=zarr_objects,
        variables=["field3d", "pop1d"], expected=zarr_expected,
        decode={"compressor": "blosc-lz4-shuffle", "fill_to_nan": False,
                "order": "C", "zarr_format": 2},
        select=ZARR_SELECT,
        notes=("Synthetic Zarr v2 store. field3d [2,5,4] chunked [1,2,4] (partial "
               "edge chunk on dim1); pop1d [8] chunked [8]. Orthogonal selection "
               "layer=[1], y=[1,4], x=all fetches ONLY field3d/1.0.0 + field3d/"
               "1.2.0 (2 of 6 chunks) — never layer 0, never the middle y-chunk "
               "field3d/1.1.0; pop1d (rank 1) reads whole. fill_value 0.0 is real "
               "data, NOT mapped to NaN. No coordinate arrays."),
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
