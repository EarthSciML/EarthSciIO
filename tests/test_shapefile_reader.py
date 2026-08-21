"""The active ``shapefile`` reader — an ESRI shapefile as a feature table.

The committed conformance case ``shapefile-polygon-zip`` already pins the decode
CONTRACT across the three tracks (one row per part, the §8.6.1 padding, the
``*``-only deletion rule, the dtype rules). What these tests cover is the
Python-side surface the corpus cannot reach with a single blob: the zip member
seam (auto-discovery, an explicit member, the ambiguous/absent cases), the bare
``.shp`` blob with no sidecars, variable selection, and the collision/option
errors — plus a direct read of the committed corpus blob so the two agree.
"""

from __future__ import annotations

import io
import json
import pathlib
import zipfile

import numpy as np
import pytest

# pyshp authors the fixtures (and is the reader's backend); a base install
# without the `shapefile` extra simply skips this module.
pyshp = pytest.importorskip("shapefile")

from earthsciio import ShapefileReader  # noqa: E402
from earthsciio.native import NativeDataset  # noqa: E402
from earthsciio.provider import DataSource, check_reader_options  # noqa: E402
from earthsciio.registry import format_registry  # noqa: E402

CORPUS = pathlib.Path(__file__).resolve().parents[1] / "conformance" / "corpus"

PRJ = 'GEOGCS["GCS_WGS_1984",DATUM["D_WGS_1984",SPHEROID["WGS_1984",6378137.0,298.257223563]]]'

# Two records: a single square, then a square + a triangular island. The island
# is one vertex shorter, so the stack pads it.
SQUARE = [(0.0, 0.0), (0.0, 2.0), (2.0, 2.0), (2.0, 0.0), (0.0, 0.0)]
MAINLAND = [(4.0, 0.0), (4.0, 2.0), (6.0, 2.0), (6.0, 0.0), (4.0, 0.0)]
ISLAND = [(7.0, 0.0), (7.0, 1.0), (8.0, 1.0), (7.0, 0.0)]


def _write_layer(*, prj=True, shx=True, dbf=True, stem="layer/parcels"):
    """A two-record Polygon layer as a dict of zip-member name -> bytes."""
    shp_io, shx_io, dbf_io = io.BytesIO(), io.BytesIO(), io.BytesIO()
    with pyshp.Writer(shp=shp_io, shx=shx_io, dbf=dbf_io) as w:
        w.field("NAME", "C", 10)
        w.field("EMIS", "N", 10, 3)
        w.poly([SQUARE])
        w.record("one", 1.25)
        w.poly([MAINLAND, ISLAND])
        w.record("two", 2.5)
    members = {f"{stem}.shp": shp_io.getvalue()}
    if shx:
        members[f"{stem}.shx"] = shx_io.getvalue()
    if dbf:
        members[f"{stem}.dbf"] = dbf_io.getvalue()
    if prj:
        members[f"{stem}.prj"] = PRJ.encode()
    return members


def _zip(tmp_path, members, name="layer.zip"):
    path = tmp_path / name
    with zipfile.ZipFile(path, "w") as zf:
        for member in sorted(members):
            zf.writestr(member, members[member])
    return path


def test_a_zipped_layer_decodes_one_row_per_part(tmp_path):
    ds = ShapefileReader().read_native(_zip(tmp_path, _write_layer()))
    assert isinstance(ds, NativeDataset)
    geom = ds.variables["geometry"]
    # 2 records -> 3 rows: the second record's mainland AND its island.
    assert geom.dims == ("index", "vertex", "xy")
    assert geom.shape == (3, 5, 2)
    assert ds.variables["shape_index"].data.tolist() == [0, 1, 1]
    assert ds.variables["part_index"].data.tolist() == [0, 0, 1]
    assert ds.variables["n_parts"].data.tolist() == [1, 2, 2]
    assert ds.variables["n_vertices"].data.tolist() == [5, 5, 4]
    # The record's attributes are REPLICATED across its parts.
    assert list(ds.variables["NAME"].data) == ["one", "two", "two"]
    assert ds.variables["EMIS"].data.tolist() == [1.25, 2.5, 2.5]
    # The stored per-record bbox, likewise replicated (the island is inside it).
    assert ds.variables["xmin"].data.tolist() == [0.0, 4.0, 4.0]
    assert ds.variables["xmax"].data.tolist() == [2.0, 8.0, 8.0]


def test_a_short_ring_is_padded_by_repeating_its_final_vertex(tmp_path):
    ds = ShapefileReader().read_native(_zip(tmp_path, _write_layer()))
    island = ds.variables["geometry"].data[2]
    assert [tuple(p) for p in island[:4]] == ISLAND
    # esm-spec §8.6.1: the padding slot repeats the FINAL vertex, never NaN —
    # so the padded ring has the same area as the ring itself.
    assert tuple(island[4]) == ISLAND[-1]
    assert not np.isnan(island).any()


def test_nvert_max_lets_the_document_declare_the_vertex_axis(tmp_path):
    path = _zip(tmp_path, _write_layer())
    wide = ShapefileReader().read_native(path, nvert_max=8)
    assert wide.variables["geometry"].shape == (3, 8, 2)
    # The extra slots still repeat the final vertex, so the ring is unchanged.
    island = wide.variables["geometry"].data[2]
    assert [tuple(p) for p in island[3:]] == [ISLAND[-1]] * 5
    with pytest.raises(ValueError, match="declared nvert_max=4"):
        ShapefileReader().read_native(path, nvert_max=4)


def test_the_prj_and_shape_type_ride_as_meta_fields(tmp_path):
    ds = ShapefileReader().read_native(_zip(tmp_path, _write_layer()))
    assert ds.variables["shape_type"].dims == ("meta",)
    assert list(ds.variables["shape_type"].data) == ["Polygon"]
    assert list(ds.variables["crs_wkt"].data) == [PRJ]
    # No `.prj` in the archive -> no CRS is invented.
    bare = ShapefileReader().read_native(_zip(tmp_path, _write_layer(prj=False),
                                              name="noprj.zip"))
    assert "crs_wkt" not in bare.variables


def test_a_bare_shp_blob_decodes_geometry_without_attributes(tmp_path):
    """The cache holds ONE blob; a `.shp` on its own still decodes, geometry only."""
    path = tmp_path / "blob"
    path.write_bytes(_write_layer()["layer/parcels.shp"])
    ds = ShapefileReader().read_native(path)
    assert ds.variables["geometry"].shape == (3, 5, 2)
    assert "NAME" not in ds.variables
    assert "crs_wkt" not in ds.variables


def test_the_member_seam_finds_names_and_reports_the_ambiguous_cases(tmp_path):
    two = {**_write_layer(stem="a/one"), **_write_layer(stem="b/two")}
    path = _zip(tmp_path, two, name="two.zip")
    with pytest.raises(KeyError, match="2 .shp members"):
        ShapefileReader().read_native(path)
    ds = ShapefileReader().read_native(path, member="b/two.shp")
    assert list(ds.variables["NAME"].data) == ["one", "two", "two"]
    with pytest.raises(KeyError, match="not in the archive"):
        ShapefileReader().read_native(path, member="b/nope.shp")
    with pytest.raises(KeyError, match="no .shp member"):
        ShapefileReader().read_native(_zip(tmp_path, {"readme.txt": b"hi"},
                                           name="empty.zip"))


def test_numeric_columns_forces_a_text_code_column_to_float(tmp_path):
    members = {}
    shp_io, shx_io, dbf_io = io.BytesIO(), io.BytesIO(), io.BytesIO()
    with pyshp.Writer(shp=shp_io, shx=shx_io, dbf=dbf_io) as w:
        w.field("GEOID", "C", 5)
        w.poly([SQUARE])
        w.record("01001")
    members["l.shp"], members["l.shx"], members["l.dbf"] = (
        shp_io.getvalue(), shx_io.getvalue(), dbf_io.getvalue())
    path = _zip(tmp_path, members, name="codes.zip")
    text = ShapefileReader().read_native(path)
    assert list(text.variables["GEOID"].data) == ["01001"]
    numeric = ShapefileReader().read_native(path, numeric_columns=["GEOID"])
    assert numeric.variables["GEOID"].data.tolist() == [1001.0]
    with pytest.raises(KeyError, match="no such .dbf column"):
        ShapefileReader().read_native(path, numeric_columns=["NOPE"])


def test_requested_variables_are_filtered_and_an_unknown_one_is_an_error(tmp_path):
    path = _zip(tmp_path, _write_layer())
    ds = ShapefileReader().read_native(path, variables=["geometry", "EMIS"])
    assert sorted(ds.variables) == ["EMIS", "geometry"]
    with pytest.raises(KeyError, match="not in the shapefile"):
        ShapefileReader().read_native(path, variables=["nope"])


def test_a_dbf_column_named_like_a_reader_field_is_refused(tmp_path):
    shp_io, shx_io, dbf_io = io.BytesIO(), io.BytesIO(), io.BytesIO()
    with pyshp.Writer(shp=shp_io, shx=shx_io, dbf=dbf_io) as w:
        w.field("xmin", "N", 10, 3)  # collides with the reader's own bbox field
        w.poly([SQUARE])
        w.record(1.0)
    path = _zip(tmp_path, {"c.shp": shp_io.getvalue(), "c.shx": shx_io.getvalue(),
                           "c.dbf": dbf_io.getvalue()}, name="clash.zip")
    with pytest.raises(ValueError, match="collide with the reader's own fields"):
        ShapefileReader().read_native(path)


def test_a_nul_deletion_flag_is_not_a_deletion(tmp_path):
    """pyshp calls ANY non-space flag deleted; the reader pins the ``*``-only rule."""
    members = _write_layer()
    raw = bytearray(members["layer/parcels.dbf"])
    hdr = int.from_bytes(raw[8:10], "little")
    rec = int.from_bytes(raw[10:12], "little")
    raw[hdr] = 0x00              # record 0: NUL — a live row
    raw[hdr + rec] = 0x2A        # record 1: `*` — deleted, with its two parts
    members["layer/parcels.dbf"] = bytes(raw)
    ds = ShapefileReader().read_native(_zip(tmp_path, members, name="flags.zip"))
    assert list(ds.variables["NAME"].data) == ["one"]
    assert ds.variables["shape_index"].data.tolist() == [0]


def test_the_registry_serves_the_reader_and_screens_its_options():
    assert "shapefile" in format_registry
    assert format_registry.status("shapefile") == "active"
    reader = format_registry.create("shapefile")
    assert isinstance(reader, ShapefileReader)
    loader = DataSource(name="l", format="shapefile", url="file:///x.zip",
                        reader_kwargs={"member": "a.shp"})
    check_reader_options(reader, loader)  # recognised -> no raise
    bad = DataSource(name="l", format="shapefile", url="file:///x.zip",
                     reader_kwargs={"membre": "a.shp"})
    with pytest.raises(Exception, match="membre"):
        check_reader_options(reader, bad)


def test_the_committed_corpus_blob_decodes_to_its_expected_arrays():
    case = json.loads((CORPUS / "cases" / "shapefile-polygon-zip.json").read_text())
    ds = ShapefileReader().read_native(
        CORPUS / case["blob_path"],
        member=case["decode"]["member"],
        numeric_columns=case["decode"]["numeric_columns"],
    )
    expected = case["expected"]["variables"]
    assert sorted(ds.variables) == sorted(expected)
    for name, spec in expected.items():
        got = ds.variables[name].data
        if spec["dtype"] == "string":
            assert list(got) == spec["data"], name
        else:
            flat = np.asarray(got).reshape(-1).astype("float64")
            want = np.array([np.nan if v is None else float(v) for v in spec["data"]])
            assert np.allclose(flat, want, equal_nan=True), name
