//! The pure logic behind the HUD home screen (1.14.0, port of `Hud.cs`): frame statistics, hitch detection, the
//! reduction of a minute of frames to a fixed number of chart points, the chart scale, the processor-map colours and
//! the session timer text.

use crate::base::ci;
use crate::base::time::{DateTime, TimeSpan, TICKS_PER_MINUTE, TICKS_PER_SECOND};

/// Frame-time numbers of the last minute for the home screen. Zero values mean "not enough frames".
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FrameStats {
    pub frames: i32,
    pub span_sec: f64,
    pub avg_fps: f64,
    pub low1_fps: f64,
    pub low01_fps: f64,
    pub frame_ms: f64,
    pub median_ms: f64,
    pub p99_ms: f64,
    pub hitches: i32,
    pub hitches_per_min: f64,
}

impl FrameStats {
    pub fn valid(&self) -> bool {
        self.frames >= HudMath::MIN_FRAMES
    }
}

/// What a logical processor is on the home screen's processor map.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CoreKind {
    Background,
    Game,
    Interrupt,
}

/// `HudMath` (static class).
pub struct HudMath;

impl HudMath {
    /// A frame longer than this many medians is a hitch (a visible stutter).
    pub const HITCH_FACTOR: f64 = 2.5;
    /// Fewer frames than this give no 1% low (the old home tile used the same limit).
    pub const MIN_FRAMES: i32 = 200;
    /// The 0.1% low needs this many frames to mean anything.
    pub const MIN_FRAMES01: i32 = 1000;

    pub fn is_hitch(ms: f64, median_ms: f64) -> bool {
        median_ms > 0.0 && ms > Self::HITCH_FACTOR * median_ms
    }

    /// Nearest-rank percentile of an ascending list (the same rule as the 1.13 home tile).
    pub fn percentile(sorted: &[f64], q: f64) -> f64 {
        let n = sorted.len() as i32;
        if n == 0 {
            return 0.0;
        }
        sorted[(n - 1).min(0.max((n as f64 * q).ceil() as i32 - 1)) as usize]
    }

    /// `Stats(intervals)`; None is the C# null list.
    pub fn stats(intervals: Option<&[f64]>) -> FrameStats {
        let n = intervals.map(|x| x.len()).unwrap_or(0) as i32;
        if n < Self::MIN_FRAMES {
            return FrameStats { frames: n, ..Default::default() };
        }
        let intervals = intervals.unwrap_or(&[]);
        let mut sorted: Vec<f64> = Vec::with_capacity(n as usize);
        let mut sum = 0.0;
        for &x in intervals {
            sorted.push(x);
            sum += x;
        }
        if sum <= 0.0 {
            return FrameStats { frames: n, ..Default::default() };
        }
        sorted.sort_by(|a, b| a.total_cmp(b));
        let median = Self::percentile(&sorted, 0.5);
        let p99 = Self::percentile(&sorted, 0.99);
        let p999 = if n >= Self::MIN_FRAMES01 { Self::percentile(&sorted, 0.999) } else { 0.0 };
        let mut hitches = 0;
        for &x in intervals {
            if Self::is_hitch(x, median) {
                hitches += 1;
            }
        }
        let span = sum / 1000.0;
        FrameStats {
            frames: n,
            span_sec: span,
            avg_fps: n as f64 / span,
            low1_fps: if p99 > 0.0 { 1000.0 / p99 } else { 0.0 },
            low01_fps: if p999 > 0.0 { 1000.0 / p999 } else { 0.0 },
            frame_ms: sum / n as f64,
            median_ms: median,
            p99_ms: p99,
            hitches,
            hitches_per_min: if span >= 1.0 { hitches as f64 * 60.0 / span } else { 0.0 },
        }
    }

    /// Reduces frame times (ascending UTC present times) to `max.len()` buckets that end at `end`.
    /// max[k]: the longest frame that covered bucket k (a long frame covers every bucket it spans, so a freeze is a
    /// plateau, not a gap); 0 = no frame there. end_max[k]: the longest frame that ENDED in bucket k (one hitch marker
    /// per frame). Frames after `end` are left for the next update.
    pub fn buckets(times: Option<&[DateTime]>, end: DateTime, bucket: TimeSpan, max: &mut [f64], mut end_max: Option<&mut [f64]>) {
        let count = max.len();
        max.iter_mut().for_each(|x| *x = 0.0);
        if let Some(e) = end_max.as_deref_mut() {
            e.iter_mut().for_each(|x| *x = 0.0);
        }
        let Some(times) = times else { return };
        if times.len() < 2 || count == 0 || bucket <= TimeSpan::ZERO {
            return;
        }
        let start = end - TimeSpan::from_ticks(bucket.ticks() * count as i64);
        let index = |t: DateTime| -> usize {
            if t <= start {
                return 0;
            }
            // A frame that ends exactly on a bucket edge belongs to the bucket before it.
            let ticks = (t - start).ticks();
            let k = ((ticks - 1) / bucket.ticks()) as i32;
            k.clamp(0, count as i32 - 1) as usize
        };
        for i in 1..times.len() {
            let (a, b) = (times[i - 1], times[i]);
            if b <= start || b > end || b <= a {
                continue;
            }
            let ms = (b - a).total_ms();
            let (bi, ai) = (index(b), index(a));
            for m in max.iter_mut().take(bi + 1).skip(ai) {
                if ms > *m {
                    *m = ms;
                }
            }
            if let Some(e) = end_max.as_deref_mut() {
                if ms > e[bi] {
                    e[bi] = ms;
                }
            }
        }
    }

    /// The chart end: the clock minus the ETW delivery lag, cut down to a whole bucket so points do not jitter.
    pub fn align_end(now: DateTime, lag: TimeSpan, bucket: TimeSpan) -> DateTime {
        let t = (now - lag).ticks;
        DateTime::new(t - t % bucket.ticks(), now.kind)
    }

    /// Rounds up to 1, 2, 2.5 or 5 times a power of ten.
    pub fn nice(v: f64) -> f64 {
        if !(v > 0.0) || v.is_infinite() {
            return 1.0;
        }
        let p = 10f64.powf(v.log10().floor());
        let f = v / p;
        let nf = if f <= 1.0000001 {
            1.0
        } else if f <= 2.0000001 {
            2.0
        } else if f <= 2.5000001 {
            2.5
        } else if f <= 5.0000001 {
            5.0
        } else {
            10.0
        };
        nf * p
    }

    /// The top of the frame-time chart in ms: room for the longest point and the 1% low line, but a long
    /// pause (alt-tab, loading) does not squash the normal frames into the floor: it is capped at 8 medians.
    pub fn chart_top(max_ms: f64, low1_ms: f64, median_ms: f64) -> f64 {
        let mut want = max_ms * 1.1;
        if median_ms > 0.0 {
            want = want.min(median_ms * 8.0);
        }
        want = want.max(low1_ms * 1.3);
        want = want.max(median_ms * 1.5);
        Self::nice(want.max(1.0))
    }

    pub fn classify(lp: i32, game_mask: u64, irq_mask: u64) -> CoreKind {
        if !(0..=63).contains(&lp) {
            return CoreKind::Background;
        }
        let bit = 1u64 << lp;
        // A processor the game may use is the game's, whatever else runs there; amber marks only the interrupt
        // processors the game was kept off.
        if game_mask & bit != 0 {
            return CoreKind::Game;
        }
        if irq_mask & bit != 0 {
            return CoreKind::Interrupt;
        }
        CoreKind::Background
    }

    /// The session timer of the status pill: hh:mm:ss, hours keep counting past 24.
    pub fn elapsed(t: TimeSpan) -> String {
        let t = if t < TimeSpan::ZERO { TimeSpan::ZERO } else { t };
        let h = t.total_hours().floor() as i64;
        let minutes = (t.ticks() / TICKS_PER_MINUTE) % 60;
        let seconds = (t.ticks() / TICKS_PER_SECOND) % 60;
        format!("{:02}:{:02}:{:02}", h, minutes, seconds)
    }

    /// Upper case with a narrow no-break space after every character: the letter spacing of the HUD captions
    /// (WPF has no letter spacing; U+202F does not allow a line break inside a word). The C# version takes a culture
    /// for `ToUpper`; the port uses the simple (culture-free) mapping, which is the same for English and Russian.
    pub fn spaced(s: Option<&str>) -> String {
        let Some(s) = s else { return String::new() };
        if s.is_empty() {
            return String::new();
        }
        let up = ci::upper(s);
        let mut sb = String::with_capacity(up.len() * 2);
        for (i, c) in up.chars().enumerate() {
            if i > 0 {
                sb.push('\u{202F}');
            }
            sb.push(c);
        }
        sb
    }
}

#[cfg(test)]
mod tests {
    //! HudTests.cs (11 tests).
    use super::*;
    use crate::base::time::Kind;

    fn flat(n: i32, ms: f64) -> Vec<f64> {
        vec![ms; n as usize]
    }

    fn close(a: f64, b: f64, digits: i32) {
        let p = 10f64.powi(digits);
        assert_eq!((a * p).round() / p, (b * p).round() / p, "{} vs {}", a, b);
    }

    #[test]
    fn too_few_frames_give_no_numbers() {
        let s = HudMath::stats(Some(&flat(HudMath::MIN_FRAMES - 1, 5.0)));
        assert!(!s.valid());
        assert_eq!(0.0, s.low1_fps);
        assert!(!HudMath::stats(None).valid());
    }

    #[test]
    fn steady_frames_give_equal_numbers() {
        let s = HudMath::stats(Some(&flat(1200, 4.0)));
        assert!(s.valid());
        close(250.0, s.avg_fps, 6);
        close(250.0, s.low1_fps, 6);
        close(250.0, s.low01_fps, 6);
        close(4.0, s.frame_ms, 6);
        close(4.0, s.median_ms, 6);
        assert_eq!(0, s.hitches);
        close(4.8, s.span_sec, 6);
    }

    #[test]
    fn zero_point_one_low_needs_a_thousand_frames() {
        let s = HudMath::stats(Some(&flat(500, 4.0)));
        assert!(s.valid());
        assert_eq!(0.0, s.low01_fps);
    }

    #[test]
    fn hitches_are_frames_longer_than_two_and_a_half_medians() {
        let mut list = flat(2000, 4.0);
        list[100] = 10.0; // exactly 2.5 medians: not a hitch
        list[200] = 10.1; // a hitch
        list[300] = 40.0; // a hitch
        let s = HudMath::stats(Some(&list));
        assert_eq!(2, s.hitches);
        assert!(HudMath::is_hitch(10.1, 4.0));
        assert!(!HudMath::is_hitch(10.0, 4.0));
        assert!(!HudMath::is_hitch(100.0, 0.0));
        // 2000 frames ≈ 8 s: 2 hitches are 15 a minute.
        close(2.0 * 60.0 / s.span_sec, s.hitches_per_min, 6);
    }

    #[test]
    fn one_percent_low_is_the_ninety_ninth_percentile_frame() {
        let mut list = flat(1000, 5.0);
        for i in 0..11 {
            list[i * 7] = 10.0; // just over 1% of the frames are slow
        }
        let s = HudMath::stats(Some(&list));
        close(100.0, s.low1_fps, 6);
        assert!(s.avg_fps < 200.0 && s.avg_fps > 190.0);
    }

    fn utc(y: i64, mo: u32, d: u32, h: u32, mi: u32, s: u32) -> DateTime {
        DateTime::from_parts(y, mo, d, h, mi, s, Kind::Utc).unwrap()
    }

    #[test]
    fn buckets_keep_the_longest_frame_and_align_to_the_end() {
        let t0 = utc(2026, 11, 18, 12, 0, 0);
        let mut times = Vec::new();
        let mut t = t0;
        for _ in 0..400 {
            t = t.add_ms(5.0);
            times.push(t);
        } // 2 s of 5 ms frames
        t = t.add_ms(30.0);
        times.push(t); // one hitch
        for _ in 0..100 {
            t = t.add_ms(5.0);
            times.push(t);
        }
        let end = HudMath::align_end(t.add_seconds(1.0), TimeSpan::from_secs(1.0), TimeSpan::from_ms(250.0));
        assert_eq!(0, end.ticks % TimeSpan::from_ms(250.0).ticks());
        let mut max = vec![0.0; 240];
        let mut end_max = vec![0.0; 240];
        HudMath::buckets(Some(&times), end, TimeSpan::from_ms(250.0), &mut max, Some(&mut end_max));
        close(30.0, max.iter().cloned().fold(f64::MIN, f64::max), 6);
        assert_eq!(1, end_max.iter().filter(|x| **x == 30.0).count());
        // Buckets before the first frame stay empty; the ones with frames are 5 ms or the hitch.
        assert_eq!(0.0, max[0]);
        assert!(max.iter().filter(|x| **x > 0.0).all(|x| *x == 5.0 || *x == 30.0));
        assert!(max.iter().filter(|x| **x > 0.0).count() >= 10);
    }

    #[test]
    fn a_long_frame_covers_every_bucket_it_spans() {
        let b = TimeSpan::from_ms(250.0);
        let end = utc(2026, 1, 1, 0, 0, 10);
        let times = vec![end.add_seconds(-3.0), end.add_seconds(-2.0), end.add_seconds(-1.0).add_ms(-10.0), end.add_seconds(-1.0)];
        let mut max = vec![0.0; 8];
        let mut end_max = vec![0.0; 8];
        HudMath::buckets(Some(&times), end, b, &mut max, Some(&mut end_max));
        // The 990 ms frame from -2 s to -1.01 s spans buckets 0..3 (the window starts at -2 s); the last 10 ms frame ends at -1 s.
        close(990.0, max[0], 6);
        close(990.0, max[3], 6);
        close(990.0, end_max[3], 6);
        assert_eq!(0.0, max[4]);
        assert_eq!(0.0, end_max[0]);
    }

    #[test]
    fn frames_after_the_end_wait_for_the_next_update() {
        let b = TimeSpan::from_ms(250.0);
        let end = utc(2026, 1, 1, 0, 0, 10);
        let times = vec![end.add_ms(-20.0), end.add_ms(-10.0), end.add_ms(30.0)];
        let mut max = vec![0.0; 4];
        HudMath::buckets(Some(&times), end, b, &mut max, None);
        close(10.0, max[3], 6);
        assert_eq!(0.0, max[2]);
    }

    #[test]
    fn chart_top_is_nice_and_ignores_long_pauses() {
        assert_eq!(1.0, HudMath::nice(0.9));
        assert_eq!(2.0, HudMath::nice(1.3));
        assert_eq!(2.5, HudMath::nice(2.2));
        assert_eq!(5.0, HudMath::nice(4.4));
        assert_eq!(10.0, HudMath::nice(7.0));
        assert_eq!(20.0, HudMath::nice(11.0));
        assert_eq!(1.0, HudMath::nice(f64::NAN));
        // A 4.2 ms spike over 1.6 ms frames: the spike fits.
        assert_eq!(5.0, HudMath::chart_top(4.2, 2.3, 1.6));
        // A 3 s pause over 1.6 ms frames is capped at 8 medians (12.8 ms -> 20).
        assert_eq!(20.0, HudMath::chart_top(3000.0, 2.3, 1.6));
        // The 1% low line always fits.
        assert!(HudMath::chart_top(2.0, 4.0, 1.6) >= 4.0 * 1.3);
    }

    #[test]
    fn core_map_classification() {
        let (game, irq) = (0xFFu64, 0b1_0100_0000_0000u64 | 0b10); // processors 10, 12 and 1
        assert_eq!(CoreKind::Game, HudMath::classify(0, game, irq));
        assert_eq!(CoreKind::Game, HudMath::classify(1, game, irq)); // the game's even if an interrupt is there
        assert_eq!(CoreKind::Interrupt, HudMath::classify(10, game, irq));
        assert_eq!(CoreKind::Interrupt, HudMath::classify(12, game, irq));
        assert_eq!(CoreKind::Background, HudMath::classify(11, game, irq));
        assert_eq!(CoreKind::Background, HudMath::classify(64, u64::MAX, 0));
        assert_eq!(CoreKind::Game, HudMath::classify(63, 1u64 << 63, 0));
    }

    #[test]
    fn session_timer_text() {
        assert_eq!("00:00:00", HudMath::elapsed(TimeSpan::from_secs(-5.0)));
        assert_eq!("00:42:17", HudMath::elapsed(TimeSpan::from_secs(42.0 * 60.0 + 17.0)));
        assert_eq!("27:03:09", HudMath::elapsed(TimeSpan::from_secs(86400.0 + 3.0 * 3600.0 + 3.0 * 60.0 + 9.0)));
        assert_eq!("123:00:00", HudMath::elapsed(TimeSpan::from_hours(123.0)));
    }

    #[test]
    fn spaced_caps() {
        assert_eq!("O\u{202F}K", HudMath::spaced(Some("ok")));
        assert_eq!("", HudMath::spaced(None));
        assert_eq!("И\u{202F}Г\u{202F}Р\u{202F}А", HudMath::spaced(Some("Игра")));
        assert!(!HudMath::spaced(Some("frame time")).contains('\u{2009}'));
    }
}
