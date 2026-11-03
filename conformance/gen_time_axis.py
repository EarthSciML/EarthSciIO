"""Writes the decoded-time-axis blobs and scenario (spec/esm-sources.md section 2.5).

Usage: python3 conformance/gen_time_axis.py

Every provider finds a record in its file by the file's own CF time axis when it
can decode one, and by the cadence offset (section 2.3) otherwise. This writes
small synthetic NetCDF-3 files whose axes differ from the cadence (a day file
missing its first records, one missing its last records, one with an irregular
and unsorted axis, one on a calendar no track decodes) into the corpus cache, and
`corpus/scenarios/decoded-time-axis.json`, which every track's test suite reads.

The expected samples come from `expected_record` below, a direct statement of
the rule written independently of any track. The `decode` cases are the Python
track's `decode_time_axis` (xarray's CF decode), which is the reference; the
Python test suite also checks every sample against its provider.
"""

from __future__ import annotations

import datetime as dt
import pathlib
import sys
import warnings

import numpy as np

HERE = pathlib.Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
sys.path.insert(0, str(HERE.parent))

from generate import CORPUS, emit_scenario_blob, write_json  # noqa: E402

from earthsciio import decode_time_axis  # noqa: E402

UTC = dt.timezone.utc
BASE = "https://data.earthsci.dev/timeaxis"
T0 = int(dt.datetime(2024, 3, 1, tzinfo=UTC).timestamp())  # 2024-03-01T00:00:00Z
H = 3600
DAY = 86400


def _iso(s):
    return dt.datetime.fromtimestamp(s, UTC).strftime("%Y-%m-%dT%H:%M:%SZ")


def _day_url(stem, day_s):
    return f"{BASE}/{stem}.{dt.datetime.fromtimestamp(day_s, UTC):%Y%m%d}.nc"


def build_file(hours, *, units, calendar, raw):
    """A day file whose records are at `hours` (hours since T0, in file order).
    `T[time, x]` is `hour * 10 + x`, so a value names the real time it is for.
    `raw(hour)` is the stored time value for that hour under `units`."""
    from netCDF4 import Dataset

    tmp = CORPUS / ".tmp_timeaxis.nc"
    ds = Dataset(tmp, "w", format="NETCDF3_CLASSIC")
    ds.createDimension("time", None)
    ds.createDimension("x", 2)
    vals = np.array([raw(h) for h in hours])
    v = ds.createVariable("time", "f8" if vals.dtype.kind == "f" else "i4", ("time",))
    v.units = units
    if calendar is not None:
        v.calendar = calendar
    v[:] = vals
    v = ds.createVariable("x", "f8", ("x",))
    v[:] = np.array([0.0, 1.0])
    v = ds.createVariable("T", "f8", ("time", "x"))
    v.units = "K"
    v[:] = np.array([[h * 10.0 + x for x in range(2)] for h in hours])
    ds.close()
    data = tmp.read_bytes()
    tmp.unlink()
    return data


# Each source: a cadence of hourly records in daily files from T0, and its files.
# `hours` are the records each file really holds (hours since T0, file order).
SOURCES = {
    "trimmed": {
        "note": (
            "Hourly records in daily files. The first file lacks its first three "
            "records, the second counts in float minutes from T0, and the third "
            "lacks its last four records."
        ),
        "end_s": T0 + 3 * DAY,
        "files": [
            {
                "day": 0,
                "hours": list(range(3, 24)),
                "units": "hours since 2024-03-01 00:00:00",
                "calendar": "standard",
                "raw": lambda h: np.int32(h),
            },
            {
                "day": 1,
                "hours": list(range(24, 48)),
                "units": "minutes since 2024-03-01T00:00:00Z",
                "calendar": "gregorian",
                "raw": lambda h: np.float64(h * 60.0),
            },
            {
                "day": 2,
                "hours": list(range(48, 68)),
                "units": "seconds since 1970-01-01",
                "calendar": None,
                "raw": lambda h: np.int32(T0 + h * H),
            },
        ],
    },
    "irregular": {
        "note": (
            "One daily file whose axis is irregular and out of order: records at "
            "00, 01, 03, 12 and 06 hours, in that order."
        ),
        "end_s": T0 + DAY,
        "files": [
            {
                "day": 0,
                "hours": [0, 1, 3, 12, 6],
                "units": "hours since 2024-03-01",
                "calendar": "proleptic_gregorian",
                "raw": lambda h: np.int32(h),
            }
        ],
    },
    "noleap": {
        "note": (
            "A daily file missing its first two records on the noleap calendar, "
            "which no track decodes: the record is the cadence offset, so it is "
            "the wrong one."
        ),
        "end_s": T0 + DAY,
        "files": [
            {
                "day": 0,
                "hours": list(range(2, 24)),
                "units": "hours since 2024-03-01 00:00:00",
                "calendar": "noleap",
                "raw": lambda h: np.int32(h),
            }
        ],
    },
}


def _decodable(f):
    return f["calendar"] in (None, "standard", "gregorian", "proleptic_gregorian")


def _file_for(src, anchor):
    day = (anchor - T0) // DAY
    for f in src["files"]:
        if f["day"] == day:
            return f
    return None


def expected_record(src, anchor):
    """The 0-based record of `anchor` in its file, or None when that record is
    outside the file (an error). Spec section 2.5, stated directly."""
    f = _file_for(src, anchor)
    hours = f["hours"]
    fa = T0 + f["day"] * DAY
    times = [T0 + h * H for h in hours]
    if _decodable(f) and min(times) <= anchor <= max(times):
        best = None
        for i, t in enumerate(times):
            if t <= anchor and (best is None or t > times[best]):
                best = i
        return best
    rec = (anchor - fa) // H  # the cadence offset
    return rec if 0 <= rec < len(hours) else None


def _row(f, rec):
    h = f["hours"][rec]
    return [h * 10.0 + x for x in range(2)]


def sample(src, t, rps):
    """What `sample_at(t)` returns for `rps` records per sample, or None for an
    error. The bracket follows spec/esm-sources.md section 2.5 too."""
    anchor = T0 + (t - T0) // H * H
    rec0 = expected_record(src, anchor)
    if rec0 is None:
        return None
    f0 = _file_for(src, anchor)
    if rps == 1:
        return {"T": _row(f0, rec0)}
    nxt = anchor + H
    succ = None
    if nxt < src["end_s"]:
        if rec0 + 1 < len(f0["hours"]):
            succ = (f0, rec0 + 1)
        else:
            f1 = _file_for(src, nxt)
            r1 = expected_record(src, nxt) if f1 is not None else None
            if r1 is not None:
                succ = (f1, r1)
    if succ is None:
        return {"T": [_row(f0, rec0), _row(f0, rec0)], "time_s": [anchor, anchor]}
    return {"T": [_row(f0, rec0), _row(*succ)], "time_s": [anchor, nxt]}


PROBES = {
    "trimmed": [
        (T0 + 5 * H, "05:00 is the third record of the trimmed first file"),
        (T0 + 5 * H + 1800, "a time between records snaps to the record before"),
        (T0 + 23 * H, "the last record of the first file; its successor is in the next file"),
        (T0 + 31 * H, "the minutes axis of the second file"),
        (T0 + 67 * H, "the last record of the file trimmed at its end"),
        (
            T0 + 1 * H,
            "before the first record of a trimmed file: the cadence offset (record 1, "
            "which holds 04:00) is used, as the Python track does",
        ),
        (
            T0 + 69 * H,
            "after the last record of a file trimmed at its end: the cadence offset "
            "is past the file's end, an error",
        ),
    ],
    "irregular": [
        (T0 + 30 * 60, "00:30 snaps to the 00:00 record"),
        (T0 + 2 * H, "02:00 is between the 01:00 and 03:00 records: the 01:00 one"),
        (T0 + 7 * H, "07:00: the 06:00 record, the last in file order"),
        (T0 + 12 * H, "the last time in the file, stored fourth"),
        (T0 + 13 * H, "after the axis: the cadence offset (13) is past the file, an error"),
    ],
    "noleap": [
        (T0 + 5 * H, "an axis not decoded: the cadence offset (record 5, which holds 07:00)"),
    ],
}

# Each a raw axis and its decode (None: not decoded). Checked against the Python
# track's xarray decode when this script runs.
DECODE = [
    ("int-hours", [0, 1, 24], "hours since 2016-05-01 00:00:00", "standard"),
    ("float-days", [0.0, 0.5, 1.25], "days since 2015-01-01", None),
    ("iso-z-minutes", [0, 30, 90], "minutes since 2016-05-01T00:30:00Z", "gregorian"),
    ("offset-reference", [0, 1], "hours since 2016-05-01 00:00:00 +01:00", "standard"),
    ("compact-offset", [0, 1], "hours since 2016-05-01 00:00:00 -0130", "standard"),
    ("utc-suffix", [0, 1], "seconds since 2016-05-01 00:00:00 UTC", "standard"),
    ("short-reference", [0, 2], "hours since 2016-5-1 06", "standard"),
    ("loose-time", [0, 1], "hours since 2016-05-01 6:5:7", "standard"),
    ("fractional-offset", [0], "hours since 2016-05-01 06:00:00.5 +01:00", "standard"),
    ("fractional-reference", [0, 1], "hours since 2016-05-01 06:30:00.25", "standard"),
    ("upper-case-unit", [0, 1], "HOURS since 2016-05-01", "Standard"),
    ("singular-unit", [0, 1], "hour since 2016-05-01", "standard"),
    ("hr-alias", [0, 1, 2], "hr since 2016-05-01", "standard"),
    ("min-alias", [0, 90], "mins since 2016-05-01", "standard"),
    ("s-alias", [0, 59, 61], "s since 2016-05-01", "standard"),
    ("d-alias", [0, 1], "d since 2016-05-01", "standard"),
    ("msec-alias", [0, 1500, 2000], "msec since 2016-05-01", "standard"),
    ("nanoseconds", [0, 1500000000], "nanoseconds since 2016-05-01", "standard"),
    ("float-truncates", [0.9999999, 1.5, -0.5], "seconds since 2016-05-01", "standard"),
    ("negative", [-1, -25], "hours since 2016-05-02", "standard"),
    ("unsorted", [5, 1, 3], "hours since 2016-05-01", "standard"),
    ("epoch-seconds", [1709251200, 1709254800], "seconds since 1970-01-01", None),
    ("proleptic", [0, 1], "days since 2000-01-01", "proleptic_gregorian"),
    ("noleap", [0, 1], "days since 2000-01-01", "noleap"),
    ("360-day", [0, 1], "days since 2000-01-01", "360_day"),
    ("julian", [0, 1], "days since 2000-01-01", "julian"),
    ("months", [0, 1], "months since 2016-01-01", "standard"),
    ("years", [0, 1], "years since 2016-01-01", "standard"),
    ("no-since", [0, 1], "hours", "standard"),
    ("no-units", [0, 1], None, "standard"),
    ("bad-reference", [0, 1], "hours since yesterday", "standard"),
    ("bad-month", [0, 1], "hours since 2016-13-01", "standard"),
    ("bad-day", [0, 1], "hours since 2016-02-30", "standard"),
    ("bad-hour", [0, 1], "hours since 2016-01-01 25:00", "standard"),
    ("week-unit", [0, 1], "weeks since 2016-01-01", "standard"),
    ("missing-value", [0.0, float("nan")], "hours since 2016-05-01", "standard"),
    ("out-of-range", [0, 200000], "days since 1970-01-01", "standard"),
    ("empty", [], "hours since 2016-05-01", "standard"),
]


def main() -> None:
    blobs = []
    providers = {}
    for name, src in SOURCES.items():
        for f in src["files"]:
            day_s = T0 + f["day"] * DAY
            data = build_file(f["hours"], units=f["units"], calendar=f["calendar"], raw=f["raw"])
            blobs.append(emit_scenario_blob(_day_url(name, day_s), data, "timeaxis"))
        samples, errors = [], []
        for t, note in PROBES[name]:
            for rps in (1, 2):
                got = sample(src, t, rps)
                if got is None:
                    errors.append({"t_s": t, "records_per_sample": rps, "note": note})
                else:
                    samples.append({"t_s": t, "records_per_sample": rps, "note": note, **got})
        providers[name] = {
            "note": src["note"],
            "format": "netcdf",
            "url_template": f"{BASE}/{name}" + ".{date:%Y%m%d}.nc",
            "substitutions": {},
            "variables": ["T"],
            "start": _iso(T0),
            "start_s": T0,
            "end": _iso(src["end_s"]),
            "end_s": src["end_s"],
            "frequency": "PT1H",
            "file_period": "P1D",
            "time_dim": "time",
            "window_s": [T0, src["end_s"]],
            "files": [
                {
                    "url": _day_url(name, T0 + f["day"] * DAY),
                    "times_s": [T0 + h * H for h in f["hours"]],
                    "units": f["units"],
                    "calendar": f["calendar"],
                }
                for f in src["files"]
            ],
            "samples": samples,
            "errors": errors,
        }

    decode = []
    for cid, values, units, calendar in DECODE:
        with warnings.catch_warnings():
            warnings.simplefilter("ignore")
            got = decode_time_axis(np.asarray(values, dtype=_dtype(values)), units, calendar)
        decode.append(
            {
                "id": cid,
                "values": [None if v != v else v for v in values],
                "dtype": _dtype(values),
                "units": units,
                "calendar": calendar,
                "seconds": got,
            }
        )

    write_json(
        CORPUS / "scenarios" / "decoded-time-axis.json",
        {
            "schema": "earthsciio/scenario/v1",
            "id": "decoded-time-axis",
            "description": (
                "Finding a record by the file's own time axis (spec/esm-sources.md "
                "section 2.5). `decode`: a raw CF time axis (`values` stored as "
                "`dtype`, a null value is NaN) with its `units` and `calendar` "
                "decodes to `seconds` (Unix seconds, UTC, floored), or is not "
                "decoded (`seconds` null). `providers`: a provider over the "
                "committed blobs (offline, the corpus cache), its URL an esm "
                "template, returns `samples` at `t_s` (`T` in file dimension order; "
                "with records_per_sample 2 the time dimension comes first and its "
                "coordinate is `time_s`), and every entry of `errors` is an error. "
                "`files[].times_s` are the times each file really holds."
            ),
            "blobs": blobs,
            "decode": decode,
            "providers": providers,
        },
    )
    print(f"wrote {len(blobs)} blobs and scenarios/decoded-time-axis.json")


def _dtype(values):
    return "float64" if any(isinstance(v, float) for v in values) else "int64"


if __name__ == "__main__":
    main()
