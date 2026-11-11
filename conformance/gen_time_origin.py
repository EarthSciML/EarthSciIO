"""Writes conformance/corpus/scenarios/time-origin.json (spec/esm-sources.md section 4).

Usage: python3 conformance/gen_time_origin.py conformance/corpus/scenarios/time-origin.json
The samples read the calendar-cadence blobs that generate.py writes.
"""

import datetime as dt
import json
import sys

UTC = dt.timezone.utc


def s(*a):
    return int(dt.datetime(*a, tzinfo=UTC).timestamp())


def nei_grid(m):
    return [[float(m * 100 + r * 10 + c) for c in range(3)] for r in range(2)]


def ceds(m):
    # Record m (0-based months from 2015-01) of the CEDS SO2 files.
    return [float(m), float(m)]


B = "https://data.earthsci.dev/calendar"
nei_entry = {
    "kind": "grid",
    "source": {"url_template": B + "/nei2016_{sector}_month_{date:%m}.ncf"},
    "temporal": {
        "start": "2016-01-01T00:00:00Z",
        "end": "2016-04-01T00:00:00Z",
        "file_period": "P1M",
        "frequency": "P1M",
        "records_per_file": 1,
        "time_variable": "TSTEP",
    },
    "metadata": {"esio_format": "netcdf"},
}
nei_opts = {"variables": ["NOX"], "substitutions": {"sector": "agr"}}
ceds_entry = {
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
ceds_opts = {"variables": ["SO2"], "substitutions": {"species": "so2"}}

O_NEI = s(2016, 1, 1)
O_CEDS = s(2015, 11, 15, 12)  # an origin off the cadence: mid-month, mid-day
O_YEAR1 = s(1, 1, 1)  # an origin before year 1000: its units pad the year to 4 digits


def nei(*a):
    return s(*a) - O_NEI


def cd(*a):
    return s(*a) - O_CEDS


def y1(*a):
    return s(*a) - O_YEAR1


cases = [
    {
        "id": "nei-monthly",
        "note": "The origin is the source start: model time 0 is the first record.",
        "name": "NEI2016",
        "entry": nei_entry,
        "options": {
            **nei_opts,
            "time_origin": "2016-01-01T00:00:00Z",
            "window": [0, nei(2016, 4, 1)],
        },
        "expect": {
            "time_units": "seconds since 2016-01-01T00:00:00Z",
            "refresh_times": [0, nei(2016, 2, 1), nei(2016, 3, 1)],
            "window_records": {
                "1": [0, nei(2016, 2, 1), nei(2016, 3, 1)],
                "2": [0, nei(2016, 2, 1), nei(2016, 3, 1)],
            },
            "epochs": [{"boundary": b, "anchor": b} for b in (0, nei(2016, 2, 1), nei(2016, 3, 1))],
            "samples": [
                {"t": nei(2016, 2, 29), "records_per_sample": 1, "NOX": nei_grid(2)},
                {
                    "t": nei(2016, 2, 15),
                    "records_per_sample": 2,
                    "NOX": [nei_grid(2), nei_grid(3)],
                    "time": [nei(2016, 2, 1), nei(2016, 3, 1)],
                },
                {
                    "t": nei(2016, 3, 16),
                    "records_per_sample": 2,
                    "NOX": [nei_grid(3), nei_grid(3)],
                    "time": [nei(2016, 3, 1), nei(2016, 3, 1)],
                },
            ],
        },
    },
    {
        "id": "ceds-origin-off-cadence",
        "note": (
            "An origin between two records: the window starts at model time 0, where the "
            "November record (anchored before the origin, at a negative time) is in effect."
        ),
        "name": "CEDS",
        "entry": ceds_entry,
        "options": {
            **ceds_opts,
            "time_origin": "2015-11-15T12:00:00Z",
            "window": [0, cd(2016, 3, 1)],
        },
        "expect": {
            "time_units": "seconds since 2015-11-15T12:00:00Z",
            "refresh_times": [cd(2015, 12, 1), cd(2016, 1, 1), cd(2016, 2, 1)],
            "window_records": {
                "1": [
                    cd(2015, 11, 1),
                    cd(2015, 12, 1),
                    cd(2016, 1, 1),
                    cd(2016, 2, 1),
                    cd(2016, 3, 1),
                ],
                "2": [
                    cd(2015, 11, 1),
                    cd(2015, 12, 1),
                    cd(2016, 1, 1),
                    cd(2016, 2, 1),
                    cd(2016, 3, 1),
                    cd(2016, 4, 1),
                ],
            },
            "epochs": [
                {"boundary": 0, "anchor": cd(2015, 11, 1)},
                {"boundary": cd(2015, 12, 1), "anchor": cd(2015, 12, 1)},
                {"boundary": cd(2016, 1, 1), "anchor": cd(2016, 1, 1)},
                {"boundary": cd(2016, 2, 1), "anchor": cd(2016, 2, 1)},
                {"boundary": cd(2016, 3, 1), "anchor": cd(2016, 3, 1)},
            ],
            "samples": [
                {"t": 0, "records_per_sample": 1, "SO2": ceds(10)},
                {"t": cd(2016, 2, 29), "records_per_sample": 1, "SO2": ceds(13)},
                {
                    "t": cd(2015, 12, 20),
                    "records_per_sample": 2,
                    "SO2": [ceds(11), ceds(12)],
                    "time": [cd(2015, 12, 1), cd(2016, 1, 1)],
                },
            ],
        },
    },
    {
        "id": "no-origin",
        "note": "No time origin: times are Unix seconds, as before (Python returns datetimes).",
        "name": "NEI2016",
        "entry": nei_entry,
        "options": {**nei_opts, "time_origin": None, "window": [s(2016, 1, 1), s(2016, 4, 1)]},
        "expect": {
            "time_units": "seconds since 1970-01-01T00:00:00Z",
            "refresh_times": [s(2016, 1, 1), s(2016, 2, 1), s(2016, 3, 1)],
            "window_records": {
                "1": [s(2016, 1, 1), s(2016, 2, 1), s(2016, 3, 1)],
                "2": [s(2016, 1, 1), s(2016, 2, 1), s(2016, 3, 1)],
            },
            "epochs": [
                {"boundary": b, "anchor": b} for b in (s(2016, 1, 1), s(2016, 2, 1), s(2016, 3, 1))
            ],
            "samples": [
                {
                    "t": s(2016, 2, 15),
                    "records_per_sample": 2,
                    "NOX": [nei_grid(2), nei_grid(3)],
                    "time": [s(2016, 2, 1), s(2016, 3, 1)],
                },
            ],
        },
    },
    {
        "id": "window-starts-mid-second",
        "note": (
            "A window that starts half a second after the origin: that start is the first "
            "boundary, kept to the fraction (seconds out are the instant's Unix seconds minus "
            "the origin's, fractions included), and the January record serves it."
        ),
        "name": "NEI2016",
        "entry": nei_entry,
        "options": {
            **nei_opts,
            "time_origin": "2016-01-01T00:00:00Z",
            "window": [0.5, nei(2016, 4, 1)],
        },
        "expect": {
            "time_units": "seconds since 2016-01-01T00:00:00Z",
            "refresh_times": [nei(2016, 2, 1), nei(2016, 3, 1)],
            "window_records": {
                "1": [0, nei(2016, 2, 1), nei(2016, 3, 1)],
                "2": [0, nei(2016, 2, 1), nei(2016, 3, 1)],
            },
            "epochs": [
                {"boundary": 0.5, "anchor": 0},
                {"boundary": nei(2016, 2, 1), "anchor": nei(2016, 2, 1)},
                {"boundary": nei(2016, 3, 1), "anchor": nei(2016, 3, 1)},
            ],
            "samples": [
                {
                    "t": 0.5,
                    "records_per_sample": 2,
                    "NOX": [nei_grid(1), nei_grid(2)],
                    "time": [0, nei(2016, 2, 1)],
                },
            ],
        },
    },
    {
        "id": "origin-year-1",
        "note": (
            "An origin before year 1000: the units pad the year to four digits "
            "(0001-01-01T00:00:00Z)."
        ),
        "name": "NEI2016",
        "entry": nei_entry,
        "options": {
            **nei_opts,
            "time_origin": "0001-01-01T00:00:00Z",
            "window": [y1(2016, 1, 1), y1(2016, 4, 1)],
        },
        "expect": {
            "time_units": "seconds since 0001-01-01T00:00:00Z",
            "refresh_times": [y1(2016, 1, 1), y1(2016, 2, 1), y1(2016, 3, 1)],
            "window_records": {
                "1": [y1(2016, 1, 1), y1(2016, 2, 1), y1(2016, 3, 1)],
                "2": [y1(2016, 1, 1), y1(2016, 2, 1), y1(2016, 3, 1)],
            },
            "epochs": [
                {"boundary": b, "anchor": b}
                for b in (y1(2016, 1, 1), y1(2016, 2, 1), y1(2016, 3, 1))
            ],
            "samples": [
                {
                    "t": y1(2016, 2, 15),
                    "records_per_sample": 2,
                    "NOX": [nei_grid(2), nei_grid(3)],
                    "time": [y1(2016, 2, 1), y1(2016, 3, 1)],
                },
            ],
        },
    },
    {
        "id": "window-before-data",
        "note": "A window in model seconds is checked as any other (spec section 2.6).",
        "name": "NEI2016",
        "entry": nei_entry,
        "options": {**nei_opts, "time_origin": "2016-01-01T00:00:00Z", "window": [-86400, 86400]},
        "error": "esm_window_uncovered",
    },
    {
        "id": "origin-not-whole-second",
        "name": "NEI2016",
        "entry": nei_entry,
        "options": {**nei_opts, "time_origin": "2016-01-01T00:00:00.500Z", "window": [0, 86400]},
        "error": "time_origin_invalid",
    },
    {
        "id": "origin-fraction-below-a-millisecond",
        "note": "Any nonzero fraction is refused, however small (a reader at millisecond resolution checks the digits as written).",
        "name": "NEI2016",
        "entry": nei_entry,
        "options": {**nei_opts, "time_origin": "2016-01-01T00:00:00.0004Z", "window": [0, 86400]},
        "error": "time_origin_invalid",
    },
    {
        "id": "origin-fraction-below-a-microsecond",
        "note": "Any nonzero fraction is refused, however small (a reader at microsecond resolution checks the digits as written).",
        "name": "NEI2016",
        "entry": nei_entry,
        "options": {
            **nei_opts,
            "time_origin": "2016-01-01T00:00:00.0000001Z",
            "window": [0, 86400],
        },
        "error": "time_origin_invalid",
    },
    {
        "id": "origin-fraction-of-zeros",
        "note": "A fraction of zeros is a whole second.",
        "name": "NEI2016",
        "entry": nei_entry,
        "options": {
            **nei_opts,
            "time_origin": "2016-01-01T00:00:00.000Z",
            "window": [0, nei(2016, 2, 1)],
        },
        "expect": {
            "time_units": "seconds since 2016-01-01T00:00:00Z",
            "refresh_times": [0],
            "window_records": {
                "1": [0, nei(2016, 2, 1)],
                "2": [0, nei(2016, 2, 1), nei(2016, 3, 1)],
            },
            "epochs": [
                {"boundary": 0, "anchor": 0},
                {"boundary": nei(2016, 2, 1), "anchor": nei(2016, 2, 1)},
            ],
            "samples": [],
        },
    },
]
sc = {
    "schema": "earthsciio/scenario/v1",
    "id": "time-origin",
    "description": (
        "A provider that counts time from a time origin (spec/esm-sources.md section 4). Each case builds the "
        "provider from the esm `entry` (named `name`) with `options`: variables, substitutions, `time_origin` (an "
        "RFC 3339 instant, or null for none) and the run `window`, in seconds from the origin (Unix seconds "
        "without one). A case with `error` fails, when the provider is built, with that code in the message. "
        "Otherwise, in seconds from the origin: `refresh_times`; `window_records` with records_per_sample 1 and "
        "2; a PrefetchGroup of this one provider (records_per_sample 1) has the `epochs[].boundary` as its "
        "boundaries and takes `epochs[].anchor` as the anchor of each epoch; `samples[]` at model time `t` "
        "return the arrays given (file dimension order) and, for a two-record bracket, the time coordinate "
        "`time` with units `time_units`. Samples read the calendar-cadence blobs, offline."
    ),
    "cases": cases,
}
with open(sys.argv[1], "w") as f:
    json.dump(sc, f, indent=2)
    f.write("\n")
