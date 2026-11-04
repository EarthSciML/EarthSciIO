//! Finding a record by the file's own time axis (`spec/esm-sources.md` §2.5).
//!
//! A DISCRETE [`Provider`](crate::Provider) locates a record in its file by the
//! file's decoded CF time axis when it has one, and by the cadence offset (§2.3)
//! otherwise. That is what the Python track does (`provider.py`
//! `_record_index`, decoding through xarray's `decode_cf_datetime`); this module
//! reproduces its decode rule by rule, so a trimmed or irregular file gives the
//! same record in every track. The shared fixture is
//! `conformance/corpus/scenarios/decoded-time-axis.json`.

use std::sync::{Arc, Mutex, PoisonError, Weak};

use time::{Date, Month, OffsetDateTime};

use crate::format::{ArrayData, Coord, NativeDataset};
use crate::provider::SourceTemporal;

const NS: i128 = 1_000_000_000;

/// Units xarray decodes through pandas, after lower-casing and adding a missing
/// final `s`: the value is truncated to whole nanoseconds.
fn pandas_unit(unit: &str) -> Option<i64> {
    Some(match unit {
        "nanoseconds" => 1,
        "microseconds" => 1_000,
        "milliseconds" => 1_000_000,
        "seconds" => 1_000_000_000,
        "minutes" => 60_000_000_000,
        "hours" => 3_600_000_000_000,
        "days" => 86_400_000_000_000,
        _ => return None,
    })
}

/// Units cftime decodes (xarray's fallback, also for the spellings pandas does
/// not know): the value is rounded to the nearest microsecond.
fn cftime_unit(unit: &str) -> Option<i64> {
    Some(match unit {
        "microsecond" | "microseconds" => 1_000,
        "millisecond" | "milliseconds" | "msec" | "msecs" | "ms" => 1_000_000,
        "second" | "seconds" | "sec" | "secs" | "s" => 1_000_000_000,
        "minute" | "minutes" | "min" | "mins" => 60_000_000_000,
        "hour" | "hours" | "hr" | "hrs" | "h" => 3_600_000_000_000,
        "day" | "days" | "d" => 86_400_000_000_000,
        _ => return None,
    })
}

/// The Julian Day Number of 1970-01-01.
const UNIX_JD: i64 = 2_440_588;

/// Days from 1970-01-01 to the proleptic Gregorian date `y-m-d`, or `None` when
/// there is no such date.
fn gregorian_days(y: i64, m: i64, d: i64) -> Option<i64> {
    let month = Month::try_from(u8::try_from(m).ok()?).ok()?;
    let date = Date::from_calendar_date(i32::try_from(y).ok()?, month, u8::try_from(d).ok()?);
    Some(i64::from(date.ok()?.to_julian_day()) - UNIX_JD)
}

/// Days from 1970-01-01 to the date `y-m-d` of the Julian calendar (year 1 or
/// later), or `None` when there is no such date. The `time` crate has no Julian
/// calendar, so this is the usual Julian Day Number arithmetic.
fn julian_days(y: i64, m: i64, d: i64) -> Option<i64> {
    let len = match m {
        2 if y % 4 == 0 => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        1..=12 => 31,
        _ => return None,
    };
    if y < 1 || d < 1 || d > len {
        return None;
    }
    let a = (14 - m) / 12;
    let yy = y + 4800 - a;
    let mm = m + 12 * a - 3;
    Some(d + (153 * mm + 2) / 5 + 365 * yy + yy / 4 - 32_083 - UNIX_JD)
}

/// Is an epoch-nanoseconds time in the years 1678 to 2261 (proleptic
/// Gregorian)?
fn year_ok(ns: i128) -> bool {
    i32::try_from(ns.div_euclid(86_400 * NS) + i128::from(UNIX_JD))
        .ok()
        .and_then(|jd| Date::from_julian_day(jd).ok())
        .is_some_and(|d| (1678..=2261).contains(&d.year()))
}

/// A cursor over the ASCII bytes of a reference date.
struct Cur<'a> {
    b: &'a [u8],
    i: usize,
}

impl Cur<'_> {
    fn peek(&self) -> Option<u8> {
        self.b.get(self.i).copied()
    }
    fn eat(&mut self, c: u8) -> bool {
        if self.peek() == Some(c) {
            self.i += 1;
            true
        } else {
            false
        }
    }
    /// 1 to `max` digits: (value, count).
    fn digits(&mut self, max: usize) -> (i64, usize) {
        let (mut v, mut n) = (0i64, 0usize);
        while n < max {
            match self.peek() {
                Some(c) if c.is_ascii_digit() => {
                    v = v * 10 + i64::from(c - b'0');
                    n += 1;
                    self.i += 1;
                }
                _ => break,
            }
        }
        (v, n)
    }
    fn spaces(&mut self) -> usize {
        let start = self.i;
        while matches!(self.peek(), Some(b' ') | Some(b'\t')) {
            self.i += 1;
        }
        self.i - start
    }
}

/// How a reference gave its time of day.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Clock {
    None,
    /// A bare two-digit hour, `HH`.
    Hour,
    /// `H:M`, `H:M:S` or `H:M:S.f…`.
    Minutes,
}

/// A CF reference date in the forms spec/esm-sources.md §2.5 lists:
/// `Y-M-D` (a year of one to four digits), then optionally (`T` or spaces) a
/// time `HH`, `H:M`, `H:M:S` or `H:M:S.fff…`, then optionally (after spaces)
/// `Z`, `UTC`, `GMT` or an offset `±HH`, `±HHMM`, `±HH:MM`. The date itself is
/// checked by whichever reading uses it, since the two read it on different
/// calendars.
struct Reference {
    year: i64,
    year_digits: usize,
    month: i64,
    day: i64,
    hour: i64,
    minute: i64,
    second: i64,
    frac_ns: i128,
    clock: Clock,
    /// Characters between the date and the time (`T` is one).
    clock_gap: usize,
    /// The offset east of UTC, in seconds.
    offset: i64,
    /// Whether the zone is spelled `Z` or `±…` (not `UTC`/`GMT`, not absent).
    zone: bool,
    /// Spaces before the zone.
    zone_gap: usize,
}

impl Reference {
    fn parse(s: &str) -> Option<Self> {
        if !s.is_ascii() {
            return None;
        }
        let mut c = Cur {
            b: s.as_bytes(),
            i: 0,
        };
        let (year, year_digits) = c.digits(4);
        if year_digits == 0 || !c.eat(b'-') {
            return None;
        }
        // xarray pads a short year to four digits only when the reference has
        // no four-digit run elsewhere; otherwise pandas reads it its own way.
        if year_digits < 4
            && s.as_bytes()
                .windows(4)
                .any(|w| w.iter().all(u8::is_ascii_digit))
        {
            return None;
        }
        let (month, n) = c.digits(2);
        if n == 0 || !c.eat(b'-') {
            return None;
        }
        let (day, n) = c.digits(2);
        if n == 0 {
            return None;
        }
        let (mut hour, mut minute, mut second, mut frac_ns) = (0i64, 0i64, 0i64, 0i128);
        let mut clock = Clock::None;
        let mark = c.i;
        let mut clock_gap = if c.eat(b'T') { 1 } else { c.spaces() };
        if matches!(c.peek(), Some(b) if b.is_ascii_digit()) {
            let (h, n) = c.digits(2);
            hour = h;
            if c.eat(b':') {
                clock = Clock::Minutes;
                let (m, n) = c.digits(2);
                if n == 0 {
                    return None;
                }
                minute = m;
                if c.eat(b':') {
                    let (sec, n) = c.digits(2);
                    if n == 0 {
                        return None;
                    }
                    second = sec;
                    if c.eat(b'.') {
                        let mut nd = 0u32;
                        while let Some(b) = c.peek().filter(u8::is_ascii_digit) {
                            if nd < 9 {
                                frac_ns = frac_ns * 10 + i128::from(b - b'0');
                            }
                            nd += 1;
                            c.i += 1;
                        }
                        if nd == 0 {
                            return None;
                        }
                        frac_ns *= 10i128.pow(9u32.saturating_sub(nd));
                    }
                }
            } else if n != 2 {
                return None; // a bare hour must have two digits
            } else {
                clock = Clock::Hour;
            }
        } else {
            c.i = mark;
            clock_gap = 0;
        }
        let zone_gap = c.spaces();
        let rest = &s[c.i..];
        let lower = rest.to_ascii_lowercase();
        let (offset, zone) = if rest.is_empty() || lower == "utc" || lower == "gmt" {
            (0, false)
        } else if lower == "z" {
            (0, true)
        } else if rest.starts_with('+') || rest.starts_with('-') {
            let sign = if rest.starts_with('+') { 1 } else { -1 };
            let mut o = Cur {
                b: rest.as_bytes(),
                i: 1,
            };
            let (oh, n) = o.digits(2);
            if n != 2 {
                return None;
            }
            let mut om = 0;
            if o.peek().is_some() {
                o.eat(b':');
                let (m, n) = o.digits(2);
                if n != 2 || o.peek().is_some() {
                    return None;
                }
                om = m;
            }
            if oh > 23 || om > 59 {
                return None;
            }
            (sign * (oh * 3600 + om * 60), true)
        } else {
            return None;
        };
        if hour > 23 || minute > 59 || second > 59 {
            return None;
        }
        Some(Reference {
            year,
            year_digits,
            month,
            day,
            hour,
            minute,
            second,
            frac_ns,
            clock,
            clock_gap,
            offset,
            zone,
            zone_gap,
        })
    }

    /// The reference as pandas reads it, in epoch nanoseconds: a four-digit
    /// year on the proleptic Gregorian calendar, every part of the time and
    /// zone applied.
    fn pandas_ns(&self) -> Option<i128> {
        if self.year_digits != 4 {
            return None;
        }
        let days = gregorian_days(self.year, self.month, self.day)?;
        let secs = i128::from(days) * 86_400
            + i128::from(self.hour * 3600 + self.minute * 60 + self.second - self.offset);
        Some(secs * NS + self.frac_ns)
    }

    /// The reference as cftime reads it, in epoch nanoseconds. On the
    /// `standard`/`gregorian` calendar a date before 1582-10-15 is a Julian
    /// date (and 1582-10-05 to 10-14 do not exist); `proleptic_gregorian` is
    /// Gregorian throughout. cftime reads a time only with minutes and right
    /// after the date (one separator), and a `Z`/`±` zone only right after that
    /// (at most one space); otherwise it stops reading, so the rest counts as
    /// midnight UTC. Fractions are cut to the microsecond.
    fn cftime_ns(&self, proleptic: bool) -> Option<i128> {
        let (y, m, d) = (self.year, self.month, self.day);
        let days = if proleptic || (y, m, d) >= (1582, 10, 15) {
            gregorian_days(y, m, d)?
        } else if (y, m, d) > (1582, 10, 4) {
            return None; // the days the reform skipped
        } else {
            julian_days(y, m, d)?
        };
        let clock = self.clock == Clock::Minutes && self.clock_gap == 1;
        let zone = self.zone && self.zone_gap <= 1 && (clock || self.clock == Clock::None);
        let (h, mi, sec, frac) = if clock {
            (
                self.hour,
                self.minute,
                self.second,
                self.frac_ns / 1000 * 1000,
            )
        } else {
            (0, 0, 0, 0)
        };
        let off = if zone { self.offset } else { 0 };
        let secs = i128::from(days) * 86_400 + i128::from(h * 3600 + mi * 60 + sec - off);
        Some(secs * NS + frac)
    }
}

/// Split `"<unit> since <reference>"` as Python's `re.match(r"(.+) since (.+)")`
/// does: the last `" since "` with something before and after it.
fn split_units(units: &str) -> Option<(&str, &str)> {
    if units.contains('\n') {
        return None;
    }
    let pat = " since ";
    let mut end = units.len();
    while let Some(i) = units[..end].rfind(pat) {
        if i > 0 && i + pat.len() < units.len() {
            return Some((units[..i].trim(), units[i + pat.len()..].trim()));
        }
        if i == 0 {
            break;
        }
        end = i + pat.len() - 1;
    }
    None
}

fn floor_s(ns: i128) -> i64 {
    ns.div_euclid(NS) as i64
}

fn in_i64(x: i128) -> bool {
    (i128::from(i64::MIN) + 1..=i128::from(i64::MAX)).contains(&x)
}

/// One raw axis value: an integer or a float, as stored.
#[derive(Clone, Copy)]
enum Raw {
    Int(i64),
    Float(f64),
}

impl Raw {
    fn as_f64(self) -> f64 {
        match self {
            Raw::Int(v) => v as f64,
            Raw::Float(v) => v,
        }
    }
}

fn raw_values(values: &ArrayData) -> Option<Vec<Raw>> {
    Some(match values {
        ArrayData::I32(v) => v.iter().map(|&x| Raw::Int(i64::from(x))).collect(),
        ArrayData::I64(v) => v.iter().map(|&x| Raw::Int(x)).collect(),
        ArrayData::F64(v) => v.iter().map(|&x| Raw::Float(x)).collect(),
        _ => return None,
    })
}

/// The Unix seconds (UTC, floored to the second) of a raw CF time axis, or
/// `None` when it is not decoded (`spec/esm-sources.md` §2.5).
///
/// A DISCRETE [`Provider`](crate::Provider) uses this to find a record in a file
/// by the file's own time axis, and falls back to the cadence offset on `None`:
/// no `units`, units without `since`, a calendar other than `standard`,
/// `gregorian` or `proleptic_gregorian`, a NaN or `fill` value, or a date that
/// is not representable (outside 1677-09-21 to 2262-04-11 on the first reading,
/// outside the years 1678 to 2261 on the second). The decode is the Python
/// track's (xarray's), rule by rule.
pub fn decode_time_axis(
    values: &ArrayData,
    units: Option<&str>,
    calendar: Option<&str>,
    fill: Option<f64>,
) -> Option<Vec<i64>> {
    let (unit, reference) = split_units(units?)?;
    let unit = unit.to_lowercase();
    let cal = calendar.map_or_else(|| "standard".to_string(), str::to_lowercase);
    if !["standard", "gregorian", "proleptic_gregorian"].contains(&cal.as_str()) {
        return None;
    }
    let reference = Reference::parse(reference)?;
    let vals = raw_values(values)?;
    for v in &vals {
        let f = v.as_f64();
        if let Raw::Float(x) = v {
            if !x.is_finite() {
                return None;
            }
        }
        if fill.is_some_and(|fv| f == fv) {
            return None;
        }
    }
    // pandas: truncate to the nanosecond, while the reference and every time fit
    // in int64 nanoseconds.
    let punit = if unit.ends_with('s') {
        unit.clone()
    } else {
        format!("{unit}s")
    };
    let pandas_ref = reference.pandas_ns().filter(|&r| in_i64(r));
    if let (Some(pf), Some(reference)) = (pandas_unit(&punit), pandas_ref) {
        let mut out = Vec::with_capacity(vals.len());
        let mut ok = true;
        for v in &vals {
            let ns = match *v {
                Raw::Int(x) => i128::from(x) * i128::from(pf),
                Raw::Float(x) => {
                    let p = x * pf as f64;
                    if p.abs() >= 9.2e18 {
                        ok = false;
                        break;
                    }
                    p.trunc() as i128
                }
            };
            let t = reference + ns;
            if !(in_i64(ns) && in_i64(t)) {
                ok = false;
                break;
            }
            out.push(floor_s(t));
        }
        if ok {
            return Some(out);
        }
    }
    // cftime: its own reading of the reference, each value rounded to the
    // microsecond.
    let cf = cftime_unit(&unit)?;
    let reference = reference.cftime_ns(cal == "proleptic_gregorian")?;
    let mut out = Vec::with_capacity(vals.len());
    for v in &vals {
        let us = v.as_f64() * (cf as f64 / 1000.0);
        if us.abs() >= 1.0e30 {
            return None;
        }
        let t = reference + (us.round() as i128) * 1000;
        if !year_ok(t) {
            return None;
        }
        out.push(floor_s(t));
    }
    Some(out)
}

/// [`decode_time_axis`] of a decoded coordinate: its values, `units`,
/// `calendar` and the fill value the reader kept.
pub fn decode_coord_times(coord: &Coord) -> Option<Vec<i64>> {
    decode_time_axis(
        &coord.field.data,
        coord.units.as_deref(),
        coord.calendar.as_deref(),
        coord.field.fill_value,
    )
}

/// The decoded time axes of the files a provider is reading, so a file's axis
/// is decoded once however many anchors it serves. An entry is keyed by the
/// decoded file's allocation, which the `Weak` keeps from being reused, and is
/// pruned once that file is dropped.
#[derive(Default)]
pub(crate) struct AxisMemo(Mutex<Vec<AxisEntry>>);

type AxisEntry = (Weak<NativeDataset>, Option<Arc<[i64]>>);

impl AxisMemo {
    /// `file`'s decoded `time_dim` axis ([`decode_coord_times`]), or `None`.
    fn times(&self, file: &Arc<NativeDataset>, time_dim: &str) -> Option<Arc<[i64]>> {
        let lock = || self.0.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some((_, t)) = lock()
            .iter()
            .find(|(w, _)| std::ptr::eq(w.as_ptr(), Arc::as_ptr(file)))
        {
            return t.clone();
        }
        let times: Option<Arc<[i64]>> = file
            .coords
            .get(time_dim)
            .and_then(decode_coord_times)
            .map(Into::into);
        let mut memo = lock();
        memo.retain(|(w, _)| w.strong_count() > 0);
        memo.push((Arc::downgrade(file), times.clone()));
        times
    }

    /// The 0-based record of `anchor` in `file` (anchored at `file_anchor`). By
    /// the file's decoded `time_dim` axis when the anchor lies in its span — the
    /// latest record at or before the anchor, the first such in file order on a
    /// tie — else the cadence offset ([`SourceTemporal::record_in_file`]).
    /// Python `provider._record_index`.
    pub(crate) fn record_index(
        &self,
        file: &Arc<NativeDataset>,
        temporal: &SourceTemporal,
        anchor: OffsetDateTime,
        file_anchor: OffsetDateTime,
    ) -> usize {
        if let Some(times) = self.times(file, temporal.time_dim.as_str()) {
            let a = anchor.unix_timestamp();
            let lo = times.iter().min();
            let hi = times.iter().max();
            if let (Some(&lo), Some(&hi)) = (lo, hi) {
                if lo <= a && a <= hi {
                    let mut best: Option<usize> = None;
                    for (i, &ti) in times.iter().enumerate() {
                        if ti <= a && best.is_none_or(|b| ti > times[b]) {
                            best = Some(i);
                        }
                    }
                    if let Some(b) = best {
                        return b;
                    }
                }
            }
        }
        temporal.record_in_file(anchor, file_anchor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reference_forms() {
        let s = |r: &str| Reference::parse(r).and_then(|r| r.pandas_ns()).map(floor_s);
        assert_eq!(s("1970-01-01"), Some(0));
        assert_eq!(s("1970-01-01T01Z"), Some(3600));
        assert_eq!(s("1970-1-1 0:0:1"), Some(1));
        assert_eq!(s("1970-01-01 01:00:00 +01:00"), Some(0));
        assert_eq!(s("1970-01-01 6"), None);
        assert_eq!(s("1970-02-30"), None);
        assert_eq!(s("1-01-01"), None); // pandas never reads a short year
    }

    #[test]
    fn cftime_reading() {
        let c = |r: &str, proleptic: bool| {
            Reference::parse(r)
                .and_then(|r| r.cftime_ns(proleptic))
                .map(floor_s)
        };
        // the last Julian day is the day before the first Gregorian one
        assert_eq!(
            c("1582-10-04", false).map(|s| s + 86_400),
            c("1582-10-15", false)
        );
        assert_eq!(c("1582-10-10", false), None);
        assert!(c("1582-10-10", true).is_some());
        assert!(c("1500-02-29", false).is_some()); // a Julian leap day
        assert_eq!(c("1500-02-29", true), None);
        assert_eq!(c("0-01-01", false), None); // no year 0 on the Julian calendar
        assert!(c("0-01-01", true).is_some());
        // a bare hour, or a time two spaces off, is not read: midnight
        assert_eq!(c("2016-05-01 06", false), c("2016-05-01", false));
        assert_eq!(c("2016-05-01  06:30", false), c("2016-05-01", false));
        assert_eq!(
            c("2016-05-01 06:30  +01:00", false),
            c("2016-05-01 06:30", false)
        );
        assert_eq!(
            c("2016-05-01 06:30 +01:00", false).map(|s| s + 3600),
            c("2016-05-01 06:30", false)
        );
        assert!(Reference::parse("1-1-1 00:00:00.0000").is_none());
    }

    #[test]
    fn split_like_python() {
        assert_eq!(
            split_units("hours since 2016-01-01 since"),
            Some(("hours", "2016-01-01 since"))
        );
        assert_eq!(split_units("hours"), None);
        assert_eq!(split_units(" since x"), None);
    }
}
