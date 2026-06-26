"""ERA5 → CDS request mapping (``esio-9nb.10``): the loader fields → request Dict
→ ``cds://`` URL, ported faithfully from ``era5.jl``'s ``ERA5PressureLevelFileSet``.

Pure offline data-mapping tests — no server, no cache. They pin the field shapes
(``area`` from the domain bbox, ``pressure_level`` high→low strings, the 24
hourly steps), the level-index↔hPa lookup, the month/day partition of a time
span, and that the produced URL decodes back to the same request.
"""

from __future__ import annotations

import json
from datetime import datetime, timedelta

import pytest

from earthsciio import decode_cds_url, era5


# --------------------------------------------------------------------------- #
# area-from-domain — era5.jl rounds outward with a 1° margin.
# --------------------------------------------------------------------------- #


def test_area_from_bbox_rounds_outward():
    # [north, west, south, east] = [ceil(max_lat+1), floor(min_lon-1),
    #                               floor(min_lat-1), ceil(max_lon+1)]
    assert era5.era5_area_from_bbox(-100.5, 39.0, -80.2, 45.7) == [47, -102, 38, -79]


def test_area_from_bbox_is_order_insensitive():
    a = era5.era5_area_from_bbox(-100.5, 39.0, -80.2, 45.7)
    b = era5.era5_area_from_bbox(-80.2, 45.7, -100.5, 39.0)  # corners swapped
    assert a == b


# --------------------------------------------------------------------------- #
# level index ↔ hPa.
# --------------------------------------------------------------------------- #


def test_levels_from_indices():
    # index 1 → 1000 hPa, 12 → 700 hPa, 37 → 1 hPa (surface-up ordering)
    assert era5.era5_levels_from_indices([1, 12, 37]) == [1000, 700, 1]


def test_levels_from_indices_rounds_and_range_checks():
    assert era5.era5_levels_from_indices([1.4]) == [1000]  # rounds to nearest
    with pytest.raises(ValueError):
        era5.era5_levels_from_indices([0])
    with pytest.raises(ValueError):
        era5.era5_levels_from_indices([38])


# --------------------------------------------------------------------------- #
# The request Dict.
# --------------------------------------------------------------------------- #


def test_request_matches_era5jl_shape():
    req = era5.era5_request(
        2018, 11, [8, 9],
        variables=["u_component_of_wind", "temperature"],  # unsorted on purpose
        pressure_levels=[500, 1000, 850],                   # unsorted on purpose
        area=[47, -102, 38, -79],
    )
    assert req["product_type"] == ["reanalysis"]
    # variable list canonicalized (sorted) for a stable cache key
    assert req["variable"] == ["temperature", "u_component_of_wind"]
    # pressure levels high→low, as strings (era5.jl: sort(plevels, rev=true))
    assert req["pressure_level"] == ["1000", "850", "500"]
    assert req["year"] == ["2018"]
    assert req["month"] == ["11"]
    assert req["day"] == ["08", "09"]
    assert req["time"][0] == "00:00" and req["time"][-1] == "23:00"
    assert len(req["time"]) == 24
    assert req["data_format"] == "netcdf"
    assert req["download_format"] == "unarchived"
    assert req["area"] == [47, -102, 38, -79]


def test_request_rejects_unknown_variable():
    with pytest.raises(ValueError):
        era5.era5_request(2018, 1, [1], ["not_a_real_variable"], [1000], [1, 0, 0, 1])


def test_request_requires_variable_level_and_day():
    with pytest.raises(ValueError):
        era5.era5_request(2018, 1, [], ["temperature"], [1000], [1, 0, 0, 1])
    with pytest.raises(ValueError):
        era5.era5_request(2018, 1, [1], [], [1000], [1, 0, 0, 1])
    with pytest.raises(ValueError):
        era5.era5_request(2018, 1, [1], ["temperature"], [], [1, 0, 0, 1])


# --------------------------------------------------------------------------- #
# The cds:// URL.
# --------------------------------------------------------------------------- #


def test_cds_url_decodes_to_the_request():
    days = [8]
    url = era5.era5_cds_url(
        2018, 11, days, ["temperature"], [1000, 500],
        era5.era5_area_from_bbox(-100.5, 39.0, -80.2, 45.7),
    )
    dataset, request = decode_cds_url(url)
    assert dataset == era5.ERA5_DATASET
    assert request == era5.era5_request(
        2018, 11, days, ["temperature"], [1000, 500], [47, -102, 38, -79]
    )


def test_cds_url_stable_for_same_fields():
    args = (2018, 11, [8], ["temperature"], [1000], [47, -102, 38, -79])
    assert era5.era5_cds_url(*args) == era5.era5_cds_url(*args)


def test_cds_url_is_the_shared_cross_language_form():
    # cds://<dataset>?<canonical-json> with the request as raw sorted-key compact
    # JSON — byte-identical to the Julia/Rust tracks so the same ERA5 month is one
    # shared cache key across languages. Lists are sorted regardless of input
    # order (variables asc, levels desc, days asc).
    url = era5.era5_cds_url(
        2018, 1, [8, 1], ["temperature", "geopotential"], [850, 1000],
        [50, -130, 20, -60],
    )
    request = era5.era5_request(
        2018, 1, [8, 1], ["temperature", "geopotential"], [850, 1000],
        [50, -130, 20, -60],
    )
    canonical = json.dumps(request, sort_keys=True, separators=(",", ":"))
    assert url == f"cds://{era5.ERA5_DATASET}?{canonical}"
    # the canonicalization is order-independent
    assert url == era5.era5_cds_url(
        2018, 1, [1, 8], ["geopotential", "temperature"], [1000, 850],
        [50, -130, 20, -60],
    )


# --------------------------------------------------------------------------- #
# month/day partition of a time span (era5.jl's per-month file iteration).
# --------------------------------------------------------------------------- #


def test_months_in_span_single_month_no_buffer():
    months = era5.era5_months_in_span(
        datetime(2018, 11, 8, 1), datetime(2018, 11, 8, 5), buffer=timedelta(0)
    )
    assert months == [(2018, 11, [8])]


def test_months_in_span_crosses_month_and_year_boundary():
    months = era5.era5_months_in_span(
        datetime(2018, 12, 30), datetime(2019, 1, 2), buffer=timedelta(0)
    )
    assert months == [(2018, 12, [30, 31]), (2019, 1, [1, 2])]


def test_months_in_span_buffer_pulls_in_adjacent_month():
    # 3h buffer before 2018-12-01T01:00 reaches back into November.
    months = era5.era5_months_in_span(
        datetime(2018, 12, 1, 1, 0), datetime(2018, 12, 1, 5, 0)
    )
    assert months[0][0:2] == (2018, 11)
    assert months[0][2][-1] == 30  # Nov 30 included by the back-buffer
    assert months[-1][0:2] == (2018, 12)


def test_months_in_span_rejects_reversed():
    with pytest.raises(ValueError):
        era5.era5_months_in_span(
            datetime(2019, 1, 2), datetime(2018, 1, 1), buffer=timedelta(0)
        )
