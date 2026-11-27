//! `DateTime` and `TimeSpan` as in .NET: 100 ns ticks since 0001-01-01, a kind (Utc / Local / Unspecified), the
//! round-trip "o" format, custom formats ("HH:mm:ss", "yyyyMMdd-HHmmss") and ISO 8601 parsing.
//!
//! Comparisons and arithmetic use the ticks only (like .NET, the kind is ignored). `DateTime::MIN` is
//! `DateTime.MinValue` / `default(DateTime)`.

use std::fmt;
use std::ops::{Add, AddAssign, Neg, Sub, SubAssign};

pub const TICKS_PER_MS: i64 = 10_000;
pub const TICKS_PER_SECOND: i64 = 10_000_000;
pub const TICKS_PER_MINUTE: i64 = 600_000_000;
pub const TICKS_PER_HOUR: i64 = 36_000_000_000;
pub const TICKS_PER_DAY: i64 = 864_000_000_000;
/// Ticks of 1970-01-01T00:00:00Z.
pub const UNIX_EPOCH_TICKS: i64 = 621_355_968_000_000_000;
/// Ticks of 1601-01-01T00:00:00Z (the FILETIME epoch).
pub const FILETIME_EPOCH_TICKS: i64 = 504_911_232_000_000_000;
const MAX_TICKS: i64 = 3_155_378_975_999_999_999;

/// `System.TimeSpan`: a signed duration in 100 ns ticks.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Debug)]
pub struct TimeSpan(pub i64);

impl TimeSpan {
    pub const ZERO: TimeSpan = TimeSpan(0);
    pub const MAX: TimeSpan = TimeSpan(i64::MAX);
    pub const MIN: TimeSpan = TimeSpan(i64::MIN);
    pub const fn from_ticks(t: i64) -> Self {
        TimeSpan(t)
    }
    /// `TimeSpan.FromMilliseconds` (.NET 8 rounds the double to whole ticks).
    pub fn from_ms(ms: f64) -> Self {
        TimeSpan((ms * TICKS_PER_MS as f64).round() as i64)
    }
    pub const fn from_ms_i(ms: i64) -> Self {
        TimeSpan(ms * TICKS_PER_MS)
    }
    pub fn from_secs(s: f64) -> Self {
        TimeSpan((s * TICKS_PER_SECOND as f64).round() as i64)
    }
    pub const fn from_secs_i(s: i64) -> Self {
        TimeSpan(s * TICKS_PER_SECOND)
    }
    pub fn from_minutes(m: f64) -> Self {
        TimeSpan((m * TICKS_PER_MINUTE as f64).round() as i64)
    }
    pub fn from_hours(h: f64) -> Self {
        TimeSpan((h * TICKS_PER_HOUR as f64).round() as i64)
    }
    pub fn from_days(d: f64) -> Self {
        TimeSpan((d * TICKS_PER_DAY as f64).round() as i64)
    }
    pub const fn ticks(self) -> i64 {
        self.0
    }
    pub fn total_ms(self) -> f64 {
        self.0 as f64 / TICKS_PER_MS as f64
    }
    pub fn total_seconds(self) -> f64 {
        self.0 as f64 / TICKS_PER_SECOND as f64
    }
    pub fn total_minutes(self) -> f64 {
        self.0 as f64 / TICKS_PER_MINUTE as f64
    }
    pub fn total_hours(self) -> f64 {
        self.0 as f64 / TICKS_PER_HOUR as f64
    }
    pub fn total_days(self) -> f64 {
        self.0 as f64 / TICKS_PER_DAY as f64
    }
    /// The whole-millisecond part of the duration (`TimeSpan.Milliseconds` is only 0..999; this is `(long)TotalMilliseconds`).
    pub fn whole_ms(self) -> i64 {
        self.0 / TICKS_PER_MS
    }
    pub fn to_std(self) -> std::time::Duration {
        std::time::Duration::from_nanos((self.0.max(0) as u64).saturating_mul(100))
    }
    pub fn from_std(d: std::time::Duration) -> Self {
        TimeSpan((d.as_nanos() / 100).min(i64::MAX as u128) as i64)
    }
}

impl Add for TimeSpan {
    type Output = TimeSpan;
    fn add(self, o: TimeSpan) -> TimeSpan {
        TimeSpan(self.0.saturating_add(o.0))
    }
}
impl Sub for TimeSpan {
    type Output = TimeSpan;
    fn sub(self, o: TimeSpan) -> TimeSpan {
        TimeSpan(self.0.saturating_sub(o.0))
    }
}
impl Neg for TimeSpan {
    type Output = TimeSpan;
    fn neg(self) -> TimeSpan {
        TimeSpan(-self.0)
    }
}
impl AddAssign for TimeSpan {
    fn add_assign(&mut self, o: TimeSpan) {
        self.0 = self.0.saturating_add(o.0);
    }
}
impl SubAssign for TimeSpan {
    fn sub_assign(&mut self, o: TimeSpan) {
        self.0 = self.0.saturating_sub(o.0);
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub enum Kind {
    #[default]
    Unspecified,
    Utc,
    Local,
}

/// `System.DateTime`.
#[derive(Clone, Copy, Debug, Default)]
pub struct DateTime {
    pub ticks: i64,
    pub kind: Kind,
}

impl PartialEq for DateTime {
    fn eq(&self, o: &Self) -> bool {
        self.ticks == o.ticks
    }
}
impl Eq for DateTime {}
impl PartialOrd for DateTime {
    fn partial_cmp(&self, o: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(o))
    }
}
impl Ord for DateTime {
    fn cmp(&self, o: &Self) -> std::cmp::Ordering {
        self.ticks.cmp(&o.ticks)
    }
}
impl std::hash::Hash for DateTime {
    fn hash<H: std::hash::Hasher>(&self, h: &mut H) {
        self.ticks.hash(h)
    }
}

impl Sub for DateTime {
    type Output = TimeSpan;
    fn sub(self, o: DateTime) -> TimeSpan {
        TimeSpan(self.ticks.saturating_sub(o.ticks))
    }
}
impl Add<TimeSpan> for DateTime {
    type Output = DateTime;
    fn add(self, d: TimeSpan) -> DateTime {
        self.add_ticks(d.0)
    }
}
impl Sub<TimeSpan> for DateTime {
    type Output = DateTime;
    fn sub(self, d: TimeSpan) -> DateTime {
        self.add_ticks(d.0.saturating_neg())
    }
}
impl AddAssign<TimeSpan> for DateTime {
    fn add_assign(&mut self, d: TimeSpan) {
        *self = self.add_ticks(d.0);
    }
}
impl SubAssign<TimeSpan> for DateTime {
    fn sub_assign(&mut self, d: TimeSpan) {
        *self = self.add_ticks(d.0.saturating_neg());
    }
}

/// Days since 1970-01-01 to (year, month, day) (H. Hinnant's civil_from_days).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = (y - era * 400) as u64;
    let m = m as u64;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d as u64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe as i64 - 719_468
}

fn days_in_month(y: i64, m: u32) -> u32 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        _ => {
            if (y % 4 == 0 && y % 100 != 0) || y % 400 == 0 {
                29
            } else {
                28
            }
        }
    }
}

/// The parts of a date and time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Parts {
    pub year: i64,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
    /// 0..9_999_999 (100 ns units within the second).
    pub frac: u32,
}

impl DateTime {
    pub const MIN: DateTime = DateTime { ticks: 0, kind: Kind::Unspecified };
    pub const MAX: DateTime = DateTime { ticks: MAX_TICKS, kind: Kind::Unspecified };

    pub const fn new(ticks: i64, kind: Kind) -> Self {
        DateTime { ticks, kind }
    }
    pub const fn utc(ticks: i64) -> Self {
        DateTime { ticks, kind: Kind::Utc }
    }

    /// `new DateTime(y, m, d, h, mi, s, kind)`; None when a part is out of range.
    pub fn from_parts(year: i64, month: u32, day: u32, hour: u32, minute: u32, second: u32, kind: Kind) -> Option<Self> {
        if !(1..=9999).contains(&year) || !(1..=12).contains(&month) || day < 1 || day > days_in_month(year, month) || hour > 23 || minute > 59 || second > 59 {
            return None;
        }
        let days = days_from_civil(year, month, day) + UNIX_EPOCH_TICKS / TICKS_PER_DAY;
        Some(DateTime {
            ticks: days * TICKS_PER_DAY + hour as i64 * TICKS_PER_HOUR + minute as i64 * TICKS_PER_MINUTE + second as i64 * TICKS_PER_SECOND,
            kind,
        })
    }

    /// `DateTime.UtcNow`.
    pub fn utc_now() -> Self {
        let d = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
        DateTime::utc(UNIX_EPOCH_TICKS + (d.as_nanos() / 100) as i64)
    }

    /// `DateTime.Now`.
    pub fn now() -> Self {
        Self::utc_now().to_local()
    }

    /// A FILETIME (100 ns since 1601, UTC) as a UTC DateTime (`DateTime.FromFileTimeUtc`).
    pub fn from_file_time_utc(ft: i64) -> Self {
        DateTime::utc(ft + FILETIME_EPOCH_TICKS)
    }

    /// `ToFileTimeUtc()`.
    pub fn to_file_time_utc(self) -> i64 {
        self.to_universal().ticks - FILETIME_EPOCH_TICKS
    }

    pub fn add_ticks(self, t: i64) -> Self {
        DateTime { ticks: self.ticks.saturating_add(t).clamp(0, MAX_TICKS), kind: self.kind }
    }
    pub fn add_ms(self, ms: f64) -> Self {
        self + TimeSpan::from_ms(ms)
    }
    pub fn add_seconds(self, s: f64) -> Self {
        self + TimeSpan::from_secs(s)
    }
    pub fn add_minutes(self, m: f64) -> Self {
        self + TimeSpan::from_minutes(m)
    }
    pub fn add_hours(self, h: f64) -> Self {
        self + TimeSpan::from_hours(h)
    }
    pub fn add_days(self, d: f64) -> Self {
        self + TimeSpan::from_days(d)
    }

    /// `ToLocalTime()`: a Local value stays; Utc and Unspecified are taken as UTC (like .NET).
    pub fn to_local(self) -> Self {
        if self.kind == Kind::Local {
            return self;
        }
        let off = crate::native::sys::utc_offset_ticks(self.ticks);
        DateTime { ticks: (self.ticks + off).clamp(0, MAX_TICKS), kind: Kind::Local }
    }

    /// `ToUniversalTime()`: a Utc value stays; Local and Unspecified are taken as local time (like .NET).
    pub fn to_universal(self) -> Self {
        if self.kind == Kind::Utc {
            return self;
        }
        let off = crate::native::sys::utc_offset_ticks(self.ticks);
        // The offset of the local time is close to the one of the same ticks read as UTC; check once more.
        let off2 = crate::native::sys::utc_offset_ticks(self.ticks - off);
        DateTime { ticks: (self.ticks - off2).clamp(0, MAX_TICKS), kind: Kind::Utc }
    }

    pub fn parts(self) -> Parts {
        let days = self.ticks.div_euclid(TICKS_PER_DAY);
        let rem = self.ticks.rem_euclid(TICKS_PER_DAY);
        let (y, m, d) = civil_from_days(days - UNIX_EPOCH_TICKS / TICKS_PER_DAY);
        Parts {
            year: y,
            month: m,
            day: d,
            hour: (rem / TICKS_PER_HOUR) as u32,
            minute: (rem % TICKS_PER_HOUR / TICKS_PER_MINUTE) as u32,
            second: (rem % TICKS_PER_MINUTE / TICKS_PER_SECOND) as u32,
            frac: (rem % TICKS_PER_SECOND) as u32,
        }
    }

    /// The date part (`.Date`).
    pub fn date(self) -> Self {
        DateTime { ticks: self.ticks - self.ticks.rem_euclid(TICKS_PER_DAY), kind: self.kind }
    }

    /// The offset written by "o" / "K" / "zzz" for a Local value.
    fn local_offset(self) -> i64 {
        crate::native::sys::utc_offset_ticks(self.to_universal().ticks)
    }

    /// `ToString("o")`: `2026-11-26T12:00:00.0000000Z` (Utc), `...+03:00` (Local), no suffix (Unspecified).
    pub fn to_o(self) -> String {
        self.format("yyyy-MM-ddTHH:mm:ss.fffffffK", &super::fmt::Culture::invariant())
    }

    /// A custom or standard .NET date/time format ("o", "s", "t", "T", "d", "g", "G" or a pattern such as "HH:mm:ss").
    pub fn format(self, pattern: &str, c: &super::fmt::Culture) -> String {
        let pat: String = match pattern {
            "" | "G" => format!("{} {}", c.short_date, c.long_time),
            "o" | "O" => "yyyy-MM-ddTHH:mm:ss.fffffffK".into(),
            "s" => "yyyy-MM-ddTHH:mm:ss".into(),
            "u" => "yyyy-MM-dd HH:mm:ssZ".into(),
            "t" => c.short_time.clone(),
            "T" => c.long_time.clone(),
            "d" => c.short_date.clone(),
            "g" => format!("{} {}", c.short_date, c.short_time),
            _ => pattern.into(),
        };
        let p = self.parts();
        let chars: Vec<char> = pat.chars().collect();
        let mut out = String::new();
        let mut i = 0;
        let run = |i: usize, ch: char| -> usize {
            let mut n = 0;
            while i + n < chars.len() && chars[i + n] == ch {
                n += 1;
            }
            n
        };
        while i < chars.len() {
            let ch = chars[i];
            match ch {
                'y' => {
                    let n = run(i, ch);
                    if n <= 2 {
                        let v = p.year % 100;
                        out.push_str(&if n == 1 { v.to_string() } else { format!("{:02}", v) });
                    } else {
                        out.push_str(&format!("{:0w$}", p.year, w = n));
                    }
                    i += n;
                }
                'M' => {
                    let n = run(i, ch);
                    out.push_str(&match n {
                        1 => p.month.to_string(),
                        2 => format!("{:02}", p.month),
                        3 => c.month_abbr(p.month),
                        _ => c.month_name(p.month),
                    });
                    i += n;
                }
                'd' => {
                    let n = run(i, ch);
                    out.push_str(&match n {
                        1 => p.day.to_string(),
                        2 => format!("{:02}", p.day),
                        _ => {
                            let dow = ((self.ticks.div_euclid(TICKS_PER_DAY) + 1) % 7) as usize; // 0001-01-01 is a Monday
                            if n == 3 { c.day_abbr(dow) } else { c.day_name(dow) }
                        }
                    });
                    i += n;
                }
                'H' | 'h' | 'm' | 's' => {
                    let n = run(i, ch);
                    let v = match ch {
                        'H' => p.hour,
                        'h' => {
                            let h = p.hour % 12;
                            if h == 0 { 12 } else { h }
                        }
                        'm' => p.minute,
                        _ => p.second,
                    };
                    out.push_str(&if n == 1 { v.to_string() } else { format!("{:02}", v) });
                    i += n;
                }
                'f' | 'F' => {
                    let n = run(i, ch).min(7);
                    let digits = format!("{:07}", p.frac);
                    let mut part = digits[..n].to_string();
                    if ch == 'F' {
                        while part.ends_with('0') {
                            part.pop();
                        }
                        if part.is_empty() && out.ends_with('.') {
                            out.pop();
                        }
                    }
                    out.push_str(&part);
                    i += run(i, ch);
                }
                't' => {
                    let n = run(i, ch);
                    let d = if p.hour < 12 { &c.am } else { &c.pm };
                    if n == 1 {
                        out.extend(d.chars().take(1));
                    } else {
                        out.push_str(d);
                    }
                    i += n;
                }
                'K' => {
                    match self.kind {
                        Kind::Utc => out.push('Z'),
                        Kind::Local => out.push_str(&offset_text(self.local_offset(), true)),
                        Kind::Unspecified => {}
                    }
                    i += 1;
                }
                'z' => {
                    let n = run(i, ch);
                    let off = if self.kind == Kind::Utc { 0 } else { self.local_offset() };
                    let h = off / TICKS_PER_HOUR;
                    out.push_str(&match n {
                        1 => format!("{}{}", if off < 0 { '-' } else { '+' }, h.abs()),
                        2 => format!("{}{:02}", if off < 0 { '-' } else { '+' }, h.abs()),
                        _ => offset_text(off, true),
                    });
                    i += n;
                }
                ':' => {
                    out.push_str(&c.time_sep);
                    i += 1;
                }
                '/' => {
                    out.push_str(&c.date_sep);
                    i += 1;
                }
                '\'' | '"' => {
                    i += 1;
                    while i < chars.len() && chars[i] != ch {
                        out.push(chars[i]);
                        i += 1;
                    }
                    i += 1;
                }
                '\\' => {
                    if i + 1 < chars.len() {
                        out.push(chars[i + 1]);
                    }
                    i += 2;
                }
                '%' => i += 1,
                _ => {
                    out.push(ch);
                    i += 1;
                }
            }
        }
        out
    }

    /// `DateTime.TryParse(s, InvariantCulture, RoundtripKind)` for the ISO 8601 forms the program writes and reads:
    /// `yyyy-MM-dd`, `yyyy-MM-ddTHH:mm[:ss[.fffffff]]` (or a space for `T`), optionally `Z` or `±HH:mm`.
    /// A value with `Z` or an offset becomes Utc (with `assume_universal` false and an offset: converted to Local,
    /// like .NET's RoundtripKind); without one it is Unspecified, or Utc when `assume_universal` is true.
    pub fn parse_iso(s: &str, assume_universal: bool) -> Option<DateTime> {
        let s = s.trim();
        let b = s.as_bytes();
        let num = |from: usize, len: usize| -> Option<u32> {
            if from + len > b.len() || !b[from..from + len].iter().all(|c| c.is_ascii_digit()) {
                return None;
            }
            s[from..from + len].parse().ok()
        };
        let year = num(0, 4)? as i64;
        if b.get(4) != Some(&b'-') {
            return None;
        }
        let month = num(5, 2)?;
        if b.get(7) != Some(&b'-') {
            return None;
        }
        let day = num(8, 2)?;
        let (mut hour, mut minute, mut second, mut frac) = (0u32, 0u32, 0u32, 0u32);
        let mut i = 10;
        if i < b.len() && (b[i] == b'T' || b[i] == b' ') {
            hour = num(i + 1, 2)?;
            if b.get(i + 3) != Some(&b':') {
                return None;
            }
            minute = num(i + 4, 2)?;
            i += 6;
            if b.get(i) == Some(&b':') {
                second = num(i + 1, 2)?;
                i += 3;
                if b.get(i) == Some(&b'.') {
                    let start = i + 1;
                    let mut j = start;
                    while j < b.len() && b[j].is_ascii_digit() {
                        j += 1;
                    }
                    if j == start {
                        return None;
                    }
                    let mut digits = s[start..j.min(start + 7)].to_string();
                    while digits.len() < 7 {
                        digits.push('0');
                    }
                    frac = digits.parse().ok()?;
                    i = j;
                }
            }
        }
        let mut offset: Option<i64> = None;
        if i < b.len() {
            match b[i] {
                b'Z' | b'z' if i + 1 == b.len() => offset = Some(0),
                b'+' | b'-' => {
                    let sign = if b[i] == b'-' { -1 } else { 1 };
                    let oh = num(i + 1, 2)? as i64;
                    let om = if b.get(i + 3) == Some(&b':') { num(i + 4, 2)? } else { num(i + 3, 2)? } as i64;
                    let end = if b.get(i + 3) == Some(&b':') { i + 6 } else { i + 5 };
                    if end != b.len() || oh > 14 || om > 59 {
                        return None;
                    }
                    offset = Some(sign * (oh * TICKS_PER_HOUR + om * TICKS_PER_MINUTE));
                }
                _ => return None,
            }
        }
        let base = DateTime::from_parts(year, month, day, hour, minute, second, Kind::Unspecified)?;
        let ticks = base.ticks + frac as i64;
        Some(match offset {
            Some(off) => {
                let u = DateTime::utc(ticks - off);
                if assume_universal || off == 0 { u } else { u.to_local() }
            }
            None => DateTime { ticks, kind: if assume_universal { Kind::Utc } else { Kind::Unspecified } },
        })
    }
}

fn offset_text(off: i64, colon: bool) -> String {
    let sign = if off < 0 { '-' } else { '+' };
    let a = off.abs();
    let h = a / TICKS_PER_HOUR;
    let m = a % TICKS_PER_HOUR / TICKS_PER_MINUTE;
    if colon {
        format!("{}{:02}:{:02}", sign, h, m)
    } else {
        format!("{}{:02}{:02}", sign, h, m)
    }
}

impl fmt::Display for DateTime {
    /// `ToString()` in the current culture ("G").
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.format("G", &super::fmt::Culture::current()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_parts() {
        let d = DateTime::from_parts(2026, 11, 26, 13, 4, 5, Kind::Utc).unwrap().add_ticks(1234567);
        assert_eq!(d.to_o(), "2026-11-26T13:04:05.1234567Z");
        assert_eq!(DateTime::parse_iso(&d.to_o(), true).unwrap(), d);
        assert_eq!(d.format("yyyyMMdd-HHmmss", &crate::base::fmt::Culture::invariant()), "20261126-130405");
        assert_eq!(DateTime::from_parts(1970, 1, 1, 0, 0, 0, Kind::Utc).unwrap().ticks, UNIX_EPOCH_TICKS);
        assert_eq!(DateTime::from_parts(1601, 1, 1, 0, 0, 0, Kind::Utc).unwrap().ticks, FILETIME_EPOCH_TICKS);
        assert_eq!(DateTime::MIN.parts().year, 1);
        assert!(DateTime::parse_iso("2026-02-30", true).is_none());
        assert!(DateTime::parse_iso("evil", true).is_none());
        let z = DateTime::parse_iso("2026-01-01T03:00:00+03:00", true).unwrap();
        assert_eq!(z.to_o(), "2026-01-01T00:00:00.0000000Z");
        assert_eq!(DateTime::utc(0).format("dddd", &crate::base::fmt::Culture::invariant()), "Monday");
    }
}
