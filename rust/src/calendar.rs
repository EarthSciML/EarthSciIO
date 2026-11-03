//! Calendar cadences (`spec/esm-sources.md` §2).
//!
//! A source's `temporal.frequency` and `temporal.file_period` are ISO 8601
//! durations. A month or a year is not a fixed number of seconds, so a period
//! is held as whole calendar months plus whole seconds, and every anchor is
//! computed on the proleptic Gregorian calendar in UTC from the cadence's
//! `start` — never by a mean month length, and never from the previous anchor.

use time::{Duration, Month, OffsetDateTime};

use crate::error::{Error, Result};

/// Seconds in a mean Gregorian month (30.436875 days). Used only to *estimate*
/// an anchor index before the exact walk in [`CalendarPeriod::floor_index`];
/// no anchor is ever computed with it.
const MEAN_MONTH_S: i128 = 2_629_746;

/// A cadence step of whole calendar `months` plus whole `seconds`
/// (`spec/esm-sources.md` §2.1): `P1M` is one month, `P1Y` twelve, `PT3H`
/// 10800 seconds, `P1D` 86400 seconds (a UTC day).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct CalendarPeriod {
    /// Whole calendar months (`12·Y + M`).
    pub months: i64,
    /// Whole seconds (`604800·W + 86400·D + 3600·H + 60·M + S`).
    pub seconds: i64,
}

impl From<Duration> for CalendarPeriod {
    /// A fixed duration: `months = 0`, its whole seconds (a fraction of a
    /// second is dropped).
    fn from(d: Duration) -> Self {
        CalendarPeriod {
            months: 0,
            seconds: d.whole_seconds(),
        }
    }
}

fn bad(period: &str, detail: impl Into<String>) -> Error {
    Error::BadPeriod {
        period: period.to_string(),
        detail: detail.into(),
    }
}

/// Read `<digits><designator>` components off `s`, the designators in the
/// order `order` lists them, each at most once. Returns their values.
fn components(period: &str, s: &str, order: &[char]) -> Result<Vec<i64>> {
    let mut values = vec![0i64; order.len()];
    let mut next = 0usize; // the first designator still allowed
    let mut rest = s;
    while !rest.is_empty() {
        let n = rest
            .find(|c: char| !c.is_ascii_digit())
            .ok_or_else(|| bad(period, format!("{rest:?} has no designator")))?;
        if n == 0 {
            return Err(bad(
                period,
                format!("expected a whole number before {:?}", &rest[..1]),
            ));
        }
        let d = rest[n..].chars().next().expect("a non-digit follows");
        let pos = order[next..]
            .iter()
            .position(|&o| o == d)
            .map(|p| p + next)
            .ok_or_else(|| {
                bad(
                    period,
                    format!(
                        "{d:?} is not allowed here (a fraction, a sign, or a designator out of order)"
                    ),
                )
            })?;
        values[pos] = rest[..n]
            .parse::<i64>()
            .map_err(|_| bad(period, format!("{:?} is too large", &rest[..n])))?;
        next = pos + 1;
        rest = &rest[n + d.len_utf8()..];
    }
    Ok(values)
}

impl CalendarPeriod {
    /// A period of `months` calendar months.
    pub fn months(months: i64) -> Self {
        CalendarPeriod { months, seconds: 0 }
    }

    /// A period of `seconds` seconds.
    pub fn seconds(seconds: i64) -> Self {
        CalendarPeriod { months: 0, seconds }
    }

    /// Parse an ISO 8601 duration `P[nY][nM][nW][nD][T[nH][nM][nS]]` with
    /// unsigned integer components, designators in that order
    /// (`spec/esm-sources.md` §2.1). Anything else, and an all-zero period, is
    /// [`Error::BadPeriod`], whose message carries `esm_period_invalid`.
    pub fn parse(iso: &str) -> Result<Self> {
        let body = iso
            .strip_prefix('P')
            .ok_or_else(|| bad(iso, "a period starts with 'P'"))?;
        let (date, time) = match body.split_once('T') {
            Some((d, t)) => {
                if t.is_empty() {
                    return Err(bad(iso, "a 'T' must be followed by a time component"));
                }
                (d, Some(t))
            }
            None => (body, None),
        };
        if date.is_empty() && time.is_none() {
            return Err(bad(iso, "it has no components"));
        }
        let ymwd = components(iso, date, &['Y', 'M', 'W', 'D'])?;
        let hms = match time {
            Some(t) => components(iso, t, &['H', 'M', 'S'])?,
            None => vec![0, 0, 0],
        };
        let overflow = || bad(iso, "it is too large");
        let months = ymwd[0]
            .checked_mul(12)
            .and_then(|y| y.checked_add(ymwd[1]))
            .ok_or_else(overflow)?;
        let seconds = [
            (ymwd[2], 604_800i64),
            (ymwd[3], 86_400),
            (hms[0], 3_600),
            (hms[1], 60),
            (hms[2], 1),
        ]
        .iter()
        .try_fold(0i64, |acc, &(n, unit)| {
            n.checked_mul(unit).and_then(|v| acc.checked_add(v))
        })
        .ok_or_else(overflow)?;
        let p = CalendarPeriod { months, seconds };
        if !p.is_positive() {
            return Err(bad(iso, "every component is zero"));
        }
        Ok(p)
    }

    /// True when the period steps forward: no negative part and not zero.
    pub fn is_positive(&self) -> bool {
        self.months >= 0 && self.seconds >= 0 && (self.months > 0 || self.seconds > 0)
    }

    /// Anchor `k` of a cadence with epoch `start` (§2.2):
    /// `add_months(start, k·months) + k·seconds`, the day of the month clamped
    /// to the target month's last day and the time of day kept.
    ///
    /// # Panics
    /// When the anchor is outside the `time` crate's date range (years
    /// −9999 to 9999).
    pub fn anchor(&self, start: OffsetDateTime, k: i64) -> OffsetDateTime {
        add_months(start, k * self.months) + Duration::seconds(k * self.seconds)
    }

    /// The largest `k` with `anchor(start, k) ≤ t`. The period must be
    /// [positive](Self::is_positive).
    pub fn floor_index(&self, start: OffsetDateTime, t: OffsetDateTime) -> i64 {
        let step = self.months as i128 * MEAN_MONTH_S + self.seconds as i128;
        assert!(
            step > 0,
            "floor_index needs a positive period, got {self:?}"
        );
        let elapsed = (t - start).whole_seconds() as i128;
        let mut k = elapsed.div_euclid(step) as i64;
        while self.anchor(start, k) > t {
            k -= 1;
        }
        while self.anchor(start, k + 1) <= t {
            k += 1;
        }
        k
    }

    /// The smallest `k` with `anchor(start, k) ≥ t`.
    pub fn ceil_index(&self, start: OffsetDateTime, t: OffsetDateTime) -> i64 {
        let k = self.floor_index(start, t);
        if self.anchor(start, k) == t {
            k
        } else {
            k + 1
        }
    }

    /// The anchor at or before `t`: `anchor(start, floor_index(start, t))`.
    pub fn floor(&self, start: OffsetDateTime, t: OffsetDateTime) -> OffsetDateTime {
        self.anchor(start, self.floor_index(start, t))
    }
}

/// `t` moved by `n` calendar months, keeping the time of day and the day of
/// the month clamped to the last day of the target month.
fn add_months(t: OffsetDateTime, n: i64) -> OffsetDateTime {
    if n == 0 {
        return t;
    }
    let m0 = t.year() as i64 * 12 + (u8::from(t.month()) as i64 - 1) + n;
    let year = m0.div_euclid(12) as i32;
    let month = Month::try_from((m0.rem_euclid(12) + 1) as u8).expect("month in 1..=12");
    let day = t.day().min(month.length(year));
    let date = time::Date::from_calendar_date(year, month, day)
        .expect("anchor within the time crate's date range");
    t.replace_date(date)
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::datetime;

    #[test]
    fn months_clamp_from_start_not_from_the_previous_anchor() {
        let p = CalendarPeriod::months(1);
        let s = datetime!(2016-01-31 06:00 UTC);
        assert_eq!(p.anchor(s, 1), datetime!(2016-02-29 06:00 UTC));
        assert_eq!(p.anchor(s, 2), datetime!(2016-03-31 06:00 UTC));
        assert_eq!(p.anchor(s, -2), datetime!(2015-11-30 06:00 UTC));
    }

    #[test]
    fn floor_and_ceil_bracket_an_anchor() {
        let p = CalendarPeriod::months(1);
        let s = datetime!(2016-01-01 0:00 UTC);
        assert_eq!(p.floor_index(s, datetime!(2016-02-29 23:59 UTC)), 1);
        assert_eq!(p.floor_index(s, datetime!(2016-03-01 0:00 UTC)), 2);
        assert_eq!(p.ceil_index(s, datetime!(2016-02-10 0:00 UTC)), 2);
        assert_eq!(p.ceil_index(s, datetime!(2016-02-01 0:00 UTC)), 1);
        assert_eq!(p.floor_index(s, datetime!(2015-12-31 0:00 UTC)), -1);
        let h = CalendarPeriod::seconds(3600);
        assert_eq!(
            h.floor_index(s, datetime!(2015-12-31 23:59:59.5 UTC)),
            -1,
            "a fraction of a second before an anchor is before it"
        );
    }
}
