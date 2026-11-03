//! Finding a record by the file's own time axis (`spec/esm-sources.md` §2.5).
//!
//! A DISCRETE [`Provider`](crate::Provider) locates a record in its file by the
//! file's decoded CF time axis when it has one, and by the cadence offset (§2.3)
//! otherwise. That is what the Python track does (`provider.py`
//! `_record_index`, decoding through xarray's `decode_cf_datetime`); this module
//! reproduces its decode rule by rule, so a trimmed or irregular file gives the
//! same record in every track. The shared fixture is
//! `conformance/corpus/scenarios/decoded-time-axis.json`.

use time::OffsetDateTime;

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

/// Days from 1970-01-01 to the proleptic Gregorian date `y-m-d`.
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// The proleptic Gregorian year of a day count from 1970-01-01.
fn year_of_days(z: i64) -> i64 {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    yoe + era * 400 + i64::from(m <= 2)
}

fn month_days(y: i64, m: i64) -> i64 {
    let leap = (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
    match m {
        2 if leap => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
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

/// A CF reference date as epoch nanoseconds (UTC), or `None`. The forms read are
/// spec/esm-sources.md §2.5's: `YYYY-M-D`, then optionally (`T` or spaces) a
/// time `HH`, `H:M`, `H:M:S` or `H:M:S.fff…`, then optionally (after spaces) `Z`,
/// `UTC`, `GMT` or an offset `±HH`, `±HHMM`, `±HH:MM`.
fn parse_reference(s: &str) -> Option<i128> {
    if !s.is_ascii() {
        return None;
    }
    let mut c = Cur {
        b: s.as_bytes(),
        i: 0,
    };
    let (y, n) = c.digits(4);
    if n != 4 || !c.eat(b'-') {
        return None;
    }
    let (mo, n) = c.digits(2);
    if n == 0 || !c.eat(b'-') {
        return None;
    }
    let (d, n) = c.digits(2);
    if n == 0 {
        return None;
    }
    let (mut hh, mut mi, mut ss, mut frac) = (0i64, 0i64, 0i64, 0i128);
    let mark = c.i;
    if !c.eat(b'T') {
        c.spaces();
    }
    if matches!(c.peek(), Some(b) if b.is_ascii_digit()) {
        let (h, n) = c.digits(2);
        hh = h;
        if c.eat(b':') {
            let (m, n) = c.digits(2);
            if n == 0 {
                return None;
            }
            mi = m;
            if c.eat(b':') {
                let (sec, n) = c.digits(2);
                if n == 0 {
                    return None;
                }
                ss = sec;
                if c.eat(b'.') {
                    let mut nd = 0u32;
                    while let Some(b) = c.peek().filter(u8::is_ascii_digit) {
                        if nd < 9 {
                            frac = frac * 10 + i128::from(b - b'0');
                        }
                        nd += 1;
                        c.i += 1;
                    }
                    if nd == 0 {
                        return None;
                    }
                    frac *= 10i128.pow(9u32.saturating_sub(nd));
                }
            }
        } else if n != 2 {
            return None; // a bare hour must have two digits
        }
    } else {
        c.i = mark;
    }
    c.spaces();
    let rest = &s[c.i..];
    let off =
        if rest.is_empty() || ["z", "utc", "gmt"].contains(&rest.to_ascii_lowercase().as_str()) {
            0
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
            sign * (oh * 3600 + om * 60)
        } else {
            return None;
        };
    if !(1..=12).contains(&mo) || d < 1 || d > month_days(y, mo) {
        return None;
    }
    if hh > 23 || mi > 59 || ss > 59 {
        return None;
    }
    let secs =
        i128::from(days_from_civil(y, mo, d)) * 86_400 + i128::from(hh * 3600 + mi * 60 + ss - off);
    Some(secs * NS + frac)
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
/// `gregorian` or `proleptic_gregorian`, a NaN or `fill` value, or a date outside
/// the years 1678 to 2261. The decode is the Python track's (xarray's), rule by
/// rule.
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
    let reference = parse_reference(reference)?;
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
    if let Some(pf) = pandas_unit(&punit).filter(|_| in_i64(reference)) {
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
    // cftime: round to the microsecond; a "standard" reference before the
    // Gregorian reform is on the Julian calendar there, which is not decoded.
    let cf = cftime_unit(&unit)?;
    if cal != "proleptic_gregorian"
        && reference < i128::from(days_from_civil(1582, 10, 15)) * 86_400 * NS
    {
        return None;
    }
    let mut out = Vec::with_capacity(vals.len());
    for v in &vals {
        let us = v.as_f64() * (cf as f64 / 1000.0);
        if us.abs() >= 1.0e30 {
            return None;
        }
        let t = reference + (us.round() as i128) * 1000;
        let year = year_of_days(floor_s(t).div_euclid(86_400));
        if !(1678..=2261).contains(&year) {
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

/// The 0-based record of `anchor` in `file` (anchored at `file_anchor`). By the
/// file's decoded `time_dim` axis when the anchor lies in its span — the latest
/// record at or before the anchor, the first such in file order on a tie — else
/// the cadence offset ([`SourceTemporal::record_in_file`]). Python
/// `provider._record_index`.
pub(crate) fn record_index(
    file: &NativeDataset,
    temporal: &SourceTemporal,
    anchor: OffsetDateTime,
    file_anchor: OffsetDateTime,
) -> usize {
    if let Some(times) = file
        .coords
        .get(temporal.time_dim.as_str())
        .and_then(decode_coord_times)
    {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reference_forms() {
        let s = |r: &str| parse_reference(r).map(floor_s);
        assert_eq!(s("1970-01-01"), Some(0));
        assert_eq!(s("1970-01-01T01Z"), Some(3600));
        assert_eq!(s("1970-1-1 0:0:1"), Some(1));
        assert_eq!(s("1970-01-01 01:00:00 +01:00"), Some(0));
        assert_eq!(s("1970-01-01 6"), None);
        assert_eq!(s("1970-02-30"), None);
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
