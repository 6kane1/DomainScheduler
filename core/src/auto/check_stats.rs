//! The pure arithmetic of the Check page (1.16.0 Test 1, port of `CheckStats.cs`): the same definitions as Domain
//! SchedTest (lows are frame-time percentiles, a hitch is a frame longer than 2.5× the median).

use crate::base::fmt::{self, Arg, Culture};
use crate::base::l::L;
use crate::lf;

/// `Environment.NewLine` (what `StringBuilder.AppendLine` writes).
pub(crate) const NL: &str = if cfg!(windows) { "\r\n" } else { "\n" };

/// One check run: frames and the game's ready-thread waits (1.16.0 Test 1.1: single runs, no CPU/GPU hint).
#[derive(Clone, Debug, PartialEq)]
pub struct CheckResult {
    pub group: String,
    pub run: i32,
    pub seconds: f64,
    pub frames: i32,
    pub fps: f64,
    pub low1: f64,
    pub low01: f64,
    pub p99_ms: f64,
    pub max_ms: f64,
    pub hitches: i32,
    pub waits: i32,
    pub wait_p99_us: f64,
    pub wait_p999_us: f64,
    pub wait_max_us: f64,
    pub over500: i32,
    pub lost: u32,
}

/// `CheckStats` (static class).
pub struct CheckStats;

impl CheckStats {
    pub const HITCH_FACTOR: f64 = 2.5;

    fn p(sorted: &[f64], q: f64) -> f64 {
        if sorted.is_empty() {
            return 0.0;
        }
        let n = sorted.len() as i32;
        sorted[((q * n as f64).ceil() as i32 - 1).clamp(0, n - 1) as usize]
    }

    #[allow(clippy::too_many_arguments)]
    pub fn from(group: &str, run: i32, waits: &[i64], presents: &[i64], first: i64, last: i64, ticks_per_second: f64, lost: u32) -> CheckResult {
        let seconds = ((last - first) as f64 / ticks_per_second).max(1e-9);
        let us_per_tick = 1e6 / ticks_per_second;
        let ms_per_tick = 1e3 / ticks_per_second;
        let mut w: Vec<f64> = waits.iter().map(|t| *t as f64 * us_per_tick).collect();
        w.sort_by(|a, b| a.total_cmp(b));
        let frames = (presents.len() as i32 - 1).max(0);
        let ft: Vec<f64> = (0..frames as usize).map(|i| (presents[i + 1] - presents[i]) as f64 * ms_per_tick).collect();
        let mut fs = ft.clone();
        fs.sort_by(|a, b| a.total_cmp(b));
        let fsec = if frames > 0 { (presents[presents.len() - 1] - presents[0]) as f64 * ms_per_tick / 1000.0 } else { 0.0 };
        let med = Self::p(&fs, 0.5);
        CheckResult {
            group: group.to_string(),
            run,
            seconds,
            frames,
            fps: if frames > 0 && fsec > 0.0 { frames as f64 / fsec } else { 0.0 },
            low1: if !fs.is_empty() { 1000.0 / Self::p(&fs, 0.99) } else { 0.0 },
            low01: if !fs.is_empty() { 1000.0 / Self::p(&fs, 0.999) } else { 0.0 },
            p99_ms: Self::p(&fs, 0.99),
            max_ms: fs.last().copied().unwrap_or(0.0),
            hitches: ft.iter().filter(|x| **x > med * Self::HITCH_FACTOR).count() as i32,
            waits: w.len() as i32,
            wait_p99_us: Self::p(&w, 0.99),
            wait_p999_us: Self::p(&w, 0.999),
            wait_max_us: w.last().copied().unwrap_or(0.0),
            over500: w.iter().filter(|x| **x > 500.0).count() as i32,
            lost,
        }
    }

    /// A plain-text report (the file saved on the desktop) in the language of the program.
    pub fn report(all: &[CheckResult], game: &str, header: &str) -> String {
        let ci = Culture::invariant();
        let mut sb = String::new();
        sb.push_str(header);
        sb.push_str(NL);
        sb.push_str(&lf!("Game: {0}", game));
        sb.push_str(NL);
        sb.push_str(NL);
        sb.push_str(L::t("run  group             FPS   1% low  0.1% low  p99 ms  max ms  hitches  waits p99 µs  p99.9 µs  max µs  >500 µs"));
        sb.push_str(NL);
        for r in all {
            let args = [
                Arg::from(r.run),
                Arg::from(r.group.as_str()),
                Arg::from(r.fps),
                Arg::from(r.low1),
                Arg::from(r.low01),
                Arg::from(r.p99_ms),
                Arg::from(r.max_ms),
                Arg::from(r.hitches),
                Arg::from(r.wait_p99_us),
                Arg::from(r.wait_p999_us),
                Arg::from(r.wait_max_us),
                Arg::from(r.over500),
            ];
            let line = fmt::format(&ci, "{0,3}  {1,-16} {2,5:0} {3,8:0} {4,9:0} {5,7:0.00} {6,7:0.00} {7,8} {8,13:0.0} {9,9:0.0} {10,7:0} {11,8}", &args)
                .unwrap_or_default();
            sb.push_str(&line);
            if r.lost > 0 {
                sb.push_str("  ");
                sb.push_str(&lf!("lost events: {0}", r.lost));
            }
            sb.push_str(NL);
        }
        sb
    }
}

#[cfg(test)]
mod tests {
    //! CheckTests.cs (3 tests).
    use super::*;

    const TPS: f64 = 10_000_000.0; // 100 ns ticks: easy numbers

    fn presents(n: usize, ms: f64, start: i64) -> Vec<i64> {
        (0..=n).map(|i| start + (i as f64 * ms * 10_000.0) as i64).collect()
    }

    fn close(a: f64, b: f64, digits: i32) {
        let p = 10f64.powi(digits);
        assert_eq!((a * p).round() / p, (b * p).round() / p, "{} vs {}", a, b);
    }

    #[test]
    fn frames_and_waits_like_sched_test() {
        let mut pr = presents(1000, 2.0, 0);
        for x in pr.iter_mut().skip(501) {
            *x += 80_000; // one 10 ms frame in the middle
        }
        let mut waits = vec![20i64; 989];
        waits.extend(std::iter::repeat(6_000i64).take(11)); // 2 µs ×989, 600 µs ×11
        let r = CheckStats::from("with Auto", 1, &waits, &pr, 0, 300_000_000, TPS, 0);
        assert_eq!(1000, r.frames);
        close(10.0, r.max_ms, 3);
        assert_eq!(1, r.hitches);
        assert_eq!(11, r.over500);
        close(600.0, r.wait_max_us, 3);
        close(600.0, r.wait_p99_us, 3); // the 990th of 1000 sorted waits is the first 600 µs one
        close(1000.0 / 2.008, r.fps, 0);
    }

    #[test]
    fn empty_run_does_not_throw() {
        let r = CheckStats::from("x", 1, &[], &[], 0, 0, TPS, 3);
        assert_eq!(0, r.frames);
        assert_eq!(0.0, r.fps);
        assert_eq!(0, r.waits);
    }

    #[test]
    fn report_lists_the_run() {
        let _g = crate::base::hooks::static_hooks(); // the language is a static
        let r = CheckResult {
            group: "with Auto".into(),
            run: 1,
            seconds: 30.0,
            frames: 1000,
            fps: 700.0,
            low1: 400.0,
            low01: 200.0,
            p99_ms: 2.3,
            max_ms: 8.0,
            hitches: 10,
            waits: 1000,
            wait_p99_us: 4.0,
            wait_p999_us: 6.0,
            wait_max_us: 100.0,
            over500: 0,
            lost: 2,
        };
        let text = CheckStats::report(&[r], "cs2.exe", "header");
        assert!(text.contains("cs2.exe"));
        assert!(text.contains("with Auto"));
        assert!(text.contains("lost events: 2"));
        assert!(text.contains("  1  with Auto          700"));
    }
}
