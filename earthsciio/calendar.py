"""Calendar cadences (spec/esm-sources.md §2).

A cadence's frequency and file period are ISO 8601 durations, and a month or a
year is not a fixed number of seconds. :class:`CalendarPeriod` holds a period as
``months`` and ``seconds`` and computes its anchors on the proleptic Gregorian
calendar in UTC, exactly as the Julia and Rust tracks do.
"""

from __future__ import annotations

import calendar as _calendar
import datetime as _dt
import re
from dataclasses import dataclass

from .errors import EarthSciIOError

__all__ = ["CalendarPeriod", "CalendarPeriodError"]

# ASCII digits only, and `\Z` rather than `$` (which also matches before a final
# newline): `\d` on a `str` would also take other scripts' digits,
# which the Julia and Rust tracks refuse.
_ISO = re.compile(
    r"^P(?:(?P<Y>[0-9]+)Y)?(?:(?P<Mo>[0-9]+)M)?(?:(?P<W>[0-9]+)W)?(?:(?P<D>[0-9]+)D)?"
    r"(?:T(?=[0-9])(?:(?P<H>[0-9]+)H)?(?:(?P<Mi>[0-9]+)M)?(?:(?P<S>[0-9]+)S)?)?\Z"
)

# The largest months or seconds a period may hold: a signed 64-bit integer, as
# in the Julia and Rust tracks, so all three refuse the same periods.
_INT64_MAX = 2**63 - 1


class CalendarPeriodError(EarthSciIOError, ValueError):
    """An ISO 8601 period EarthSciIO cannot read; the message contains
    ``esm_period_invalid``."""


@dataclass(frozen=True)
class CalendarPeriod:
    """A cadence step of ``months`` calendar months plus ``seconds`` seconds."""

    months: int = 0
    seconds: int = 0

    def __post_init__(self) -> None:
        if self.months < 0 or self.seconds < 0 or (self.months == 0 and self.seconds == 0):
            raise CalendarPeriodError(
                f"esm_period_invalid: a period must be positive, got "
                f"months={self.months} seconds={self.seconds}"
            )

    @classmethod
    def parse(cls, iso: str) -> CalendarPeriod:
        """Read ``P[nY][nM][nW][nD][T[nH][nM][nS]]`` (spec §2.1)."""
        m = _ISO.match(iso) if isinstance(iso, str) else None
        if m is None or iso in ("P",):
            raise CalendarPeriodError(
                f"esm_period_invalid: cannot read {iso!r} as an ISO 8601 period"
            )
        g = {k: int(v) if v else 0 for k, v in m.groupdict().items()}
        months = 12 * g["Y"] + g["Mo"]
        seconds = 604800 * g["W"] + 86400 * g["D"] + 3600 * g["H"] + 60 * g["Mi"] + g["S"]
        if months > _INT64_MAX or seconds > _INT64_MAX:
            raise CalendarPeriodError(f"esm_period_invalid: the period {iso!r} is too large")
        if months == 0 and seconds == 0:
            raise CalendarPeriodError(f"esm_period_invalid: the period {iso!r} is zero")
        return cls(months, seconds)

    @classmethod
    def of(cls, value: CalendarPeriod | _dt.timedelta | str | int) -> CalendarPeriod:
        """A period from a period, a fixed ``timedelta``, an ISO string or whole seconds."""
        if isinstance(value, CalendarPeriod):
            return value
        if isinstance(value, str):
            return cls.parse(value)
        if isinstance(value, _dt.timedelta):
            secs = value.total_seconds()
            if secs != int(secs):
                raise CalendarPeriodError(
                    f"esm_period_invalid: a cadence step must be whole seconds, got {value!r}"
                )
            return cls(0, int(secs))
        if isinstance(value, int) and not isinstance(value, bool):
            return cls(0, value)
        raise CalendarPeriodError(f"esm_period_invalid: cannot read {value!r} as a period")

    def approximate_seconds(self) -> float:
        """A mean length, only for estimating an anchor index (never an anchor)."""
        return self.months * 2629746.0 + self.seconds

    # -- anchors (spec §2.2) ---------------------------------------------- #

    def anchor(self, start: _dt.datetime, k: int) -> _dt.datetime:
        """``add_months(start, k*months) + k*seconds``."""
        out = start
        if self.months:
            m0 = start.month - 1 + k * self.months
            y, m = start.year + m0 // 12, m0 % 12 + 1
            out = start.replace(year=y, month=m, day=min(start.day, _calendar.monthrange(y, m)[1]))
        if self.seconds:
            out = out + _dt.timedelta(seconds=k * self.seconds)
        return out

    def floor(self, start: _dt.datetime, t: _dt.datetime) -> int:
        """The largest ``k`` with ``anchor(start, k) <= t``."""
        if self.months == 0:
            return int((t - start) // _dt.timedelta(seconds=self.seconds))
        k = int((t - start).total_seconds() // self.approximate_seconds())
        while self.anchor(start, k) > t:
            k -= 1
        while self.anchor(start, k + 1) <= t:
            k += 1
        return k

    def ceil(self, start: _dt.datetime, t: _dt.datetime) -> int:
        """The smallest ``k`` with ``anchor(start, k) >= t``."""
        k = self.floor(start, t)
        return k if self.anchor(start, k) == t else k + 1
