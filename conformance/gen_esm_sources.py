"""Writes conformance/corpus/scenarios/esm-sources.json (spec/esm-sources.md section 3).

Usage: python3 conformance/gen_esm_sources.py conformance/corpus/scenarios/esm-sources.json
The samples read the calendar-cadence blobs that generate.py writes.
"""

import datetime as dt
import json
import sys

UTC = dt.timezone.utc


def s(*a):
    return int(dt.datetime(*a, tzinfo=UTC).timestamp())


def nei_grid(m):
    return [[[float(m * 100 + r * 10 + c) for c in range(3)] for r in range(2)]]


B = "https://data.earthsci.dev/calendar"
nei_entry = {
    "kind": "grid",
    "source": {
        "url_template": B + "/nei2016_{sector}_month_{date:%m}.ncf",
        "mirrors": ["file:///data/nei/2016fh_16j_{sector}_12US1_month_{date:%m}.ncf"],
    },
    "temporal": {
        "start": "2016-01-01T00:00:00Z",
        "end": "2016-12-31T23:59:59Z",
        "file_period": "P1M",
        "frequency": "P1M",
        "records_per_file": 1,
        "time_variable": "TSTEP",
    },
    "metadata": {"esio_format": "netcdf", "tags": ["emissions"]},
}
geos = {
    "kind": "grid",
    "source": {
        "url_template": "https://geos-chem.s3-us-west-2.amazonaws.com/GEOS_0.25x0.3125/GEOS_FP/{date:%Y}/{date:%m}/GEOSFP.{date:%Y%m%d}.A1.025x03125.nc"
    },
    "temporal": {
        "start": "2013-01-01T00:30:00Z",
        "file_period": "P1D",
        "frequency": "PT1H",
        "records_per_file": 24,
        "time_variable": "time",
    },
    "metadata": {"esio_format": "netcdf"},
}


def gurl(d):
    return f"https://geos-chem.s3-us-west-2.amazonaws.com/GEOS_0.25x0.3125/GEOS_FP/2016/05/GEOSFP.2016050{d}.A1.025x03125.nc"


ceds = {
    "kind": "grid",
    "source": {"url_template": B + "/ceds_{species}_{date:%Y}.nc"},
    "temporal": {
        "start": "2015-01-01T00:00:00Z",
        "end": "2017-01-01T00:00:00Z",
        "file_period": "P1Y",
        "frequency": "P1M",
        "records_per_file": 12,
        "time_variable": "time",
    },
    "metadata": {"esio_format": "netcdf"},
}


def err(id, entry, code, opts=None, mentions=()):
    # Every message names the source (spec section 3.3), template errors too.
    return {
        "id": id,
        "name": "S",
        "entry": entry,
        "options": opts or {"variables": ["v"]},
        "error": code,
        "mentions": ["S", *(m for m in mentions if m != "S")],
    }


six_hourly = {
    "kind": "grid",
    "source": {"url_template": "https://h/{date:%Y%m%d%H}.nc"},
    "temporal": {"start": "2020-01-01T00:00:00Z", "frequency": "PT6H"},
    "metadata": {"esio_format": "netcdf"},
}


def six_hourly_case(id, start):
    """six_hourly with `start` spelled another way for the same instant."""
    return {
        "id": id,
        "name": "H",
        "entry": setp(six_hourly, ["temporal", "start"], start),
        "options": {"variables": ["v"], "window_s": [s(2020, 1, 1), s(2020, 1, 1, 12)]},
        "expect": {
            "format": "netcdf",
            "const": False,
            "time_dim": "time",
            "refresh_times_s": [s(2020, 1, 1), s(2020, 1, 1, 6)],
            "files": [
                {
                    "t_s": s(2020, 1, 1, 7),
                    "file_anchor_s": s(2020, 1, 1, 6),
                    "record": 0,
                    "url": "https://h/2020010106.nc",
                    "mirrors": [],
                }
            ],
        },
    }


def drop(d, *path):
    d = json.loads(json.dumps(d))
    x = d
    for p in path[:-1]:
        x = x[p]
    del x[path[-1]]
    return d


def setp(d, path, v):
    d = json.loads(json.dumps(d))
    x = d
    for p in path[:-1]:
        x = x[p]
    x[path[-1]] = v
    return d


cases = [
    {
        "id": "nei2016-monthly",
        "name": "NEI2016",
        "entry": nei_entry,
        "options": {
            "variables": ["NOX"],
            "substitutions": {"sector": "agr"},
            "window_s": [s(2016, 1, 1), s(2016, 4, 1)],
        },
        "expect": {
            "format": "netcdf",
            "const": False,
            "time_dim": "TSTEP",
            "refresh_times_s": [s(2016, 1, 1), s(2016, 2, 1), s(2016, 3, 1)],
            "files": [
                {
                    "t_s": s(2016, m, d),
                    "file_anchor_s": s(2016, m, 1),
                    "record": 0,
                    "url": f"{B}/nei2016_agr_month_0{m}.ncf",
                    "mirrors": [f"file:///data/nei/2016fh_16j_agr_12US1_month_0{m}.ncf"],
                }
                for m, d in ((1, 1), (2, 29), (3, 15))
            ],
            "samples": [
                {"t_s": s(2016, 2, 29), "records_per_sample": 1, "NOX": nei_grid(2)[0]},
                {
                    "t_s": s(2016, 2, 15),
                    "records_per_sample": 2,
                    "NOX": [nei_grid(2)[0], nei_grid(3)[0]],
                    "time_s": [s(2016, 2, 1), s(2016, 3, 1)],
                },
            ],
        },
    },
    {
        "id": "nei2016-url-override",
        "name": "NEI2016",
        "entry": nei_entry,
        "options": {
            "variables": ["NOX"],
            "substitutions": {"sector": "agr"},
            "url_override": "file:///mirror/nei_{sector}_{date:%Y%m}.ncf",
            "window_s": [s(2016, 1, 1), s(2016, 2, 1)],
        },
        "expect": {
            "format": "netcdf",
            "const": False,
            "time_dim": "TSTEP",
            "refresh_times_s": [s(2016, 1, 1)],
            "files": [
                {
                    "t_s": s(2016, 1, 10),
                    "file_anchor_s": s(2016, 1, 1),
                    "record": 0,
                    "url": "file:///mirror/nei_agr_201601.ncf",
                    "mirrors": [],
                }
            ],
        },
    },
    {
        "id": "geosfp-a1",
        "name": "GEOSFP_A1",
        "entry": geos,
        "options": {"variables": ["T2M"], "window_s": [s(2016, 5, 1, 22), s(2016, 5, 2, 2)]},
        "expect": {
            "format": "netcdf",
            "const": False,
            "time_dim": "time",
            "refresh_times_s": [
                s(2016, 5, 1, 22, 30),
                s(2016, 5, 1, 23, 30),
                s(2016, 5, 2, 0, 30),
                s(2016, 5, 2, 1, 30),
            ],
            "files": [
                {
                    "t_s": s(2016, 5, 1, 22, 30),
                    "file_anchor_s": s(2016, 5, 1, 0, 30),
                    "record": 22,
                    "url": gurl(1),
                    "mirrors": [],
                },
                {
                    "t_s": s(2016, 5, 2, 0, 10),
                    "file_anchor_s": s(2016, 5, 1, 0, 30),
                    "record": 23,
                    "url": gurl(1),
                    "mirrors": [],
                },
                {
                    "t_s": s(2016, 5, 2, 0, 30),
                    "file_anchor_s": s(2016, 5, 2, 0, 30),
                    "record": 0,
                    "url": gurl(2),
                    "mirrors": [],
                },
            ],
        },
    },
    {
        "id": "ceds-yearly-files",
        "name": "CEDS",
        "entry": ceds,
        "options": {
            "variables": ["SO2"],
            "substitutions": {"species": "so2"},
            "window_s": [s(2015, 11, 1), s(2016, 3, 1)],
        },
        "expect": {
            "format": "netcdf",
            "const": False,
            "time_dim": "time",
            "refresh_times_s": [s(2015, 11, 1), s(2015, 12, 1), s(2016, 1, 1), s(2016, 2, 1)],
            "files": [
                {
                    "t_s": s(2015, 12, 15),
                    "file_anchor_s": s(2015, 1, 1),
                    "record": 11,
                    "url": f"{B}/ceds_so2_2015.nc",
                    "mirrors": [],
                }
            ],
            "samples": [
                {"t_s": s(2016, 2, 29), "records_per_sample": 1, "SO2": [13.0, 13.0]},
                {
                    "t_s": s(2015, 12, 15),
                    "records_per_sample": 2,
                    "SO2": [[11.0, 11.0], [12.0, 12.0]],
                    "time_s": [s(2015, 12, 1), s(2016, 1, 1)],
                },
            ],
        },
    },
    {
        "id": "var-substitution",
        "name": "CEDS",
        "entry": setp(ceds, ["source", "url_template"], B + "/ceds_{var}_{date:%Y}.nc"),
        "options": {"variables": ["SO2"], "window_s": [s(2016, 1, 1), s(2016, 2, 1)]},
        "expect": {
            "format": "netcdf",
            "const": False,
            "time_dim": "time",
            "refresh_times_s": [s(2016, 1, 1)],
            "files": [
                {
                    "t_s": s(2016, 1, 1),
                    "file_anchor_s": s(2016, 1, 1),
                    "record": 0,
                    "url": f"{B}/ceds_SO2_2016.nc",
                    "mirrors": [],
                }
            ],
        },
    },
    {
        "id": "static",
        "name": "DEM",
        "entry": {
            "kind": "static",
            "source": {
                "url_template": "s3://prd-tnm/StagedProducts/Elevation/1/TIFF/USGS_Seamless_DEM_1.tif"
            },
            "metadata": {"esio_format": "geotiff"},
        },
        "options": {"variables": ["Band1"]},
        "expect": {
            "format": "geotiff",
            "const": True,
            "refresh_times_s": [],
            "files": [
                {
                    "t_s": 0,
                    "url": "s3://prd-tnm/StagedProducts/Elevation/1/TIFF/USGS_Seamless_DEM_1.tif",
                    "mirrors": [],
                }
            ],
        },
    },
    six_hourly_case("frequency-alone", "2020-01-01T00:00:00Z"),
    # One instant, spelled every way an entry may (spec section 3.1).
    six_hourly_case("start-offset", "2019-12-31T19:00:00-05:00"),
    six_hourly_case("start-space-separator", "2020-01-01 00:00:00"),
    six_hourly_case("start-space-separator-z", "2020-01-01 00:00:00Z"),
    six_hourly_case("start-one-digit-fraction", "2020-01-01T00:00:00.0Z"),
    {
        # records_per_file is checked from anchor indices, not by stepping
        # through 31.5M records.
        "id": "records-per-file-many",
        "name": "S1",
        "entry": {
            "kind": "grid",
            "source": {"url_template": "https://h/{date:%Y}.nc"},
            "temporal": {
                "start": "2015-01-01T00:00:00Z",
                "frequency": "PT1S",
                "file_period": "P1Y",
                "records_per_file": 31536000,
            },
            "metadata": {"esio_format": "netcdf"},
        },
        "options": {"variables": ["v"], "window_s": [s(2015, 1, 1), s(2015, 1, 1, 0, 0, 2)]},
        "expect": {
            "format": "netcdf",
            "const": False,
            "time_dim": "time",
            "refresh_times_s": [s(2015, 1, 1), s(2015, 1, 1, 0, 0, 1)],
            "files": [
                {
                    "t_s": s(2015, 1, 1, 0, 0, 1),
                    "file_anchor_s": s(2015, 1, 1),
                    "record": 1,
                    "url": "https://h/2015.nc",
                    "mirrors": [],
                }
            ],
        },
    },
    err(
        "no-format",
        drop(nei_entry, "metadata", "esio_format"),
        "esm_source_no_format",
        {"variables": ["NOX"], "substitutions": {"sector": "agr"}},
        ["S", "esio_format"],
    ),
    err("unbound-sector", nei_entry, "esm_template_unbound", {"variables": ["NOX"]}, ["sector"]),
    err(
        "relative-url",
        setp(nei_entry, ["source", "url_template"], "../data/nei_{sector}_{date:%m}.ncf"),
        "esm_source_url",
        {"variables": ["NOX"], "substitutions": {"sector": "agr"}},
        ["S"],
    ),
    err(
        "env-var-url",
        setp(nei_entry, ["source", "url_template"], "file://${HOME}/nei_{date:%m}.ncf"),
        "esm_source_url",
        {"variables": ["NOX"]},
        ["S"],
    ),
    err(
        "no-cadence",
        drop(drop(nei_entry, "temporal", "frequency"), "temporal", "file_period"),
        "esm_source_temporal",
        {"variables": ["NOX"], "substitutions": {"sector": "agr"}},
        ["S"],
    ),
    err(
        "records-per-file-mismatch",
        setp(geos, ["temporal", "records_per_file"], 8),
        "esm_source_records_per_file",
        {"variables": ["T2M"]},
        ["S", "8", "24"],
    ),
    err(
        "bad-period",
        setp(geos, ["temporal", "frequency"], "PT1.5H"),
        "esm_period_invalid",
        {"variables": ["T2M"]},
    ),
    err(
        "unbound-sector-in-mirror",
        {
            "kind": "grid",
            "source": {
                "url_template": "https://x/{date:%m}.nc",
                "mirrors": ["file:///data/{sector}_{date:%m}.nc"],
            },
            "temporal": {"start": "2016-01-01T00:00:00Z", "frequency": "P1M"},
            "metadata": {"esio_format": "netcdf"},
        },
        "esm_template_unbound",
        {"variables": ["v"]},
        ["sector"],
    ),
    err(
        "frequency-not-a-string",
        drop(setp(geos, ["temporal", "frequency"], 3600), "temporal", "records_per_file"),
        "esm_period_invalid",
        {"variables": ["T2M"]},
        ["frequency"],
    ),
    err(
        "time-variable-not-a-string",
        setp(geos, ["temporal", "time_variable"], 5),
        "esm_source_temporal",
        {"variables": ["T2M"]},
        ["time_variable"],
    ),
    err(
        "start-not-a-string",
        setp(geos, ["temporal", "start"], 0),
        "esm_source_temporal",
        {"variables": ["T2M"]},
        ["start"],
    ),
    err(
        "cadence-without-start",
        drop(nei_entry, "temporal", "start"),
        "esm_source_temporal",
        {"variables": ["NOX"], "substitutions": {"sector": "agr"}},
    ),
    err(
        "date-without-anchor",
        {
            "kind": "static",
            "source": {"url_template": "https://h/nei_{date:%m}.ncf"},
            "metadata": {"esio_format": "netcdf"},
        },
        "esm_template_no_anchor",
    ),
    err(
        "mirrors-not-an-array",
        setp(geos, ["source", "mirrors"], "file:///data/x.nc"),
        "esm_source_url",
        {"variables": ["T2M"]},
        ["array"],
    ),
    err(
        "mirror-not-a-string",
        setp(geos, ["source", "mirrors"], [1]),
        "esm_source_url",
        {"variables": ["T2M"]},
        ["string"],
    ),
    err(
        "var-ambiguous",
        setp(ceds, ["source", "url_template"], B + "/ceds_{var}_{date:%Y}.nc"),
        "esm_template_unbound",
        {"variables": ["SO2", "NOX"]},
        ["var"],
    ),
]
sc = {
    "schema": "earthsciio/scenario/v1",
    "id": "esm-sources",
    "description": (
        "Building a provider from an esm data_sources entry (spec/esm-sources.md section 3). Each case passes `entry` "
        "(named `name`) and `options` (variables, substitutions, url_override, window_s) to the track's constructor. A case with "
        "`error` fails with that code in the message, and every `mentions` string. Otherwise the provider has `format`, is CONST "
        "iff `const`, has `time_dim`, `refresh_times_s` over window_s, and at each `files[].t_s` reads record `record` of the file "
        "at `file_anchor_s` whose primary URL is `url` and whose mirrors are `mirrors`. `samples` are read offline from the corpus "
        "cache (blobs of the calendar-cadence scenario), with `records_per_sample` as given, the arrays nested in file dimension order."
    ),
    "cases": cases,
}
json.dump(sc, open(sys.argv[1], "w"), indent=2)
open(sys.argv[1], "a").write("\n")
