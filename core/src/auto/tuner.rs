//! The automatic A/B tuner (port of `Tuner.cs`): while the user plays, it switches between a few setting
//! combinations, measures the frame time of each, and keeps a combination only when it was better than the current
//! one in every round. Round 1 goes A, B, C…, round 2 in the reverse order, so a scene that slowly gets heavier does
//! not favour one variant. The first seconds after a switch are not measured (threads move, caches refill).
//! Pure logic: the caller applies the variants and supplies frame intervals.

use super::check_stats::NL;
use crate::base::fmt::{num, Culture};
use crate::base::l::L;
use crate::base::time::{DateTime, TimeSpan};
use crate::lf;

/// One combination the tuner tries.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TunerVariant {
    pub name: String,
    pub top5: bool,
    pub live_mask: bool,
}

impl TunerVariant {
    pub fn new(name: &str, top5: bool, live_mask: bool) -> Self {
        TunerVariant { name: name.to_string(), top5, live_mask }
    }
}

/// Frame statistics of one measured phase.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TunerPhase {
    pub variant: i32,
    pub round: i32,
    pub frames: i32,
    pub hitches: i32,
    pub avg_fps: f64,
    pub low1: f64,
    pub low01: f64,
    pub valid: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    None,
    Switch,
    Finished,
}

/// LINQ `Average()` of a sequence; an empty one gives NaN here (C# throws InvalidOperationException; see the report).
fn average(it: impl Iterator<Item = f64>) -> f64 {
    let (mut s, mut n) = (0.0, 0usize);
    for x in it {
        s += x;
        n += 1;
    }
    if n == 0 { f64::NAN } else { s / n as f64 }
}

/// `AbTuner`.
#[derive(Clone, Debug, Default)]
pub struct AbTuner {
    schedule: Vec<(i32, i32)>,
    intervals: Vec<f64>,
    step_start: DateTime,
    last_fetch: DateTime,
    index: usize,
    variants: Vec<TunerVariant>,
    baseline: i32,
    phases: Vec<TunerPhase>,
    running: bool,
    winner: i32,
    verdict: String,
}

impl AbTuner {
    pub const PHASE_SECONDS: i32 = 60;
    pub const SETTLE_SECONDS: i32 = 8;
    pub const ROUNDS: i32 = 2;
    pub const MIN_FRAMES: i32 = 600;
    /// Frame events reach the program in batches, up to a few seconds late: frames are read this far behind the
    /// clock, and a phase is closed only after its last frames had time to arrive.
    pub const LAG_SECONDS: i32 = 3;
    /// A variant must beat the current one by this much (1% low) in every round to be chosen.
    pub const MIN_GAIN: f64 = 0.03;

    pub fn new() -> Self {
        AbTuner { winner: -1, ..Default::default() }
    }

    pub fn variants(&self) -> &[TunerVariant] {
        &self.variants
    }
    pub fn baseline(&self) -> i32 {
        self.baseline
    }
    pub fn phases(&self) -> &[TunerPhase] {
        &self.phases
    }
    pub fn running(&self) -> bool {
        self.running
    }
    pub fn winner(&self) -> i32 {
        self.winner
    }
    pub fn verdict(&self) -> &str {
        &self.verdict
    }
    pub fn current(&self) -> Option<TunerVariant> {
        if self.running && self.index < self.schedule.len() {
            self.variants.get(self.schedule[self.index].0 as usize).cloned()
        } else {
            None
        }
    }
    pub fn step_index(&self) -> i32 {
        self.index as i32
    }
    pub fn step_count(&self) -> i32 {
        self.schedule.len() as i32
    }
    pub fn total(&self) -> TimeSpan {
        TimeSpan::from_secs(((Self::PHASE_SECONDS + Self::LAG_SECONDS) as usize * self.schedule.len()) as f64)
    }

    /// Starts; the first variant to apply is returned (the baseline is measured too).
    pub fn start(&mut self, variants: &[TunerVariant], baseline: i32, now: DateTime) -> Option<TunerVariant> {
        self.variants = variants.to_vec();
        self.baseline = baseline;
        self.schedule.clear();
        self.phases.clear();
        for r in 0..Self::ROUNDS {
            let mut order: Vec<i32> = (0..variants.len() as i32).collect();
            if r % 2 == 1 {
                order.reverse();
            }
            for v in order {
                self.schedule.push((v, r));
            }
        }
        self.index = 0;
        self.winner = -1;
        self.verdict = String::new();
        self.running = true;
        self.begin_step(now);
        self.current()
    }

    fn begin_step(&mut self, now: DateTime) {
        self.step_start = now;
        self.last_fetch = now.add_seconds(Self::SETTLE_SECONDS as f64);
        self.intervals.clear();
    }

    /// Once a second. frames(from, to) returns the frame intervals (ms) of frames that ended in that span.
    /// Returns Switch with the next variant to apply, Finished when done, otherwise None.
    pub fn tick(&mut self, now: DateTime, frames: &mut dyn FnMut(DateTime, DateTime) -> Vec<f64>) -> (Step, Option<TunerVariant>) {
        if !self.running {
            return (Step::None, None);
        }
        // Measured span of this step: from the end of the settle time to the end of the phase, read LagSeconds late.
        let phase_end = self.step_start.add_seconds(Self::PHASE_SECONDS as f64);
        let mut up_to = now.add_seconds(-(Self::LAG_SECONDS as f64));
        if up_to > phase_end {
            up_to = phase_end;
        }
        if up_to > self.last_fetch {
            let got = frames(self.last_fetch, up_to);
            self.intervals.extend(got);
            self.last_fetch = up_to;
        }
        if (now - self.step_start).total_seconds() < (Self::PHASE_SECONDS + Self::LAG_SECONDS) as f64 {
            return (Step::None, None);
        }
        let (v, r) = self.schedule[self.index];
        let mut ph = Self::measure(&self.intervals);
        ph.variant = v;
        ph.round = r;
        self.phases.push(ph);
        self.index += 1;
        if self.index >= self.schedule.len() {
            self.running = false;
            let (w, verdict) = Self::decide(&self.variants, &self.phases, self.baseline);
            self.winner = w;
            self.verdict = verdict;
            return (Step::Finished, None);
        }
        self.begin_step(now);
        (Step::Switch, self.current())
    }

    pub fn cancel(&mut self) {
        self.running = false;
        self.verdict = L::t("Stopped before the end: nothing was changed").to_string();
    }

    /// Average FPS, 1% and 0.1% low (FPS of the 99th / 99.9th percentile frame time), hitches (> 2.5× median).
    pub fn measure(intervals: &[f64]) -> TunerPhase {
        let mut ph = TunerPhase { frames: intervals.len() as i32, ..Default::default() };
        if (intervals.len() as i32) < Self::MIN_FRAMES {
            return ph;
        }
        let mut sorted: Vec<f64> = intervals.iter().copied().filter(|x| *x > 0.0 && *x < 1000.0).collect();
        sorted.sort_by(|a, b| a.total_cmp(b));
        if (sorted.len() as i32) < Self::MIN_FRAMES {
            return ph;
        }
        let n = sorted.len() as i32;
        let p = |q: f64| sorted[((q * n as f64).ceil() as i32 - 1).clamp(0, n - 1) as usize];
        let median = p(0.5);
        ph.avg_fps = 1000.0 / average(sorted.iter().copied());
        ph.low1 = 1000.0 / p(0.99);
        ph.low01 = 1000.0 / p(0.999);
        ph.hitches = sorted.iter().filter(|x| **x > median * 2.5).count() as i32;
        ph.valid = true;
        ph
    }

    /// The winner beats the baseline's 1% low by MinGain in every round, with no more hitches on average; among
    /// several, the best average 1% low. Otherwise the baseline stays (-1 = keep current).
    pub fn decide(variants: &[TunerVariant], phases: &[TunerPhase], baseline: i32) -> (i32, String) {
        let rounds = phases.iter().map(|p| p.round).max().map(|m| m + 1).unwrap_or(0);
        if phases.iter().any(|p| !p.valid) {
            return (
                -1,
                L::t("Too few frames were measured in some phases (was the game in front all the time?): nothing was changed").to_string(),
            );
        }
        let get = |v: i32, r: i32| phases.iter().find(|p| p.variant == v && p.round == r);
        let mut best = -1;
        let mut best_low = 0.0;
        for v in 0..variants.len() as i32 {
            if v == baseline {
                continue;
            }
            let mut all_better = true;
            for r in 0..rounds {
                let (a, b) = (get(v, r), get(baseline, r));
                match (a, b) {
                    (Some(a), Some(b)) if !(a.low1 < b.low1 * (1.0 + Self::MIN_GAIN)) => {}
                    _ => {
                        all_better = false;
                        break;
                    }
                }
            }
            let hit_v = average(phases.iter().filter(|p| p.variant == v).map(|p| p.hitches as f64));
            let hit_b = average(phases.iter().filter(|p| p.variant == baseline).map(|p| p.hitches as f64));
            let low = average(phases.iter().filter(|p| p.variant == v).map(|p| p.low1));
            if all_better && hit_v <= hit_b && low > best_low {
                best = v;
                best_low = low;
            }
        }
        if best < 0 {
            (-1, L::t("No combination was better than the current one in every round: the current settings stay").to_string())
        } else {
            (best, lf!("\"{0}\" was better in every round: press Apply on the home screen to keep it", &variants[best as usize].name))
        }
    }

    /// Average 1% low over the valid rounds and total hitches of one variant.
    pub fn stats(&self, variant: i32) -> (f64, i32) {
        let ph: Vec<&TunerPhase> = self.phases.iter().filter(|p| p.variant == variant).collect();
        let valid: Vec<&&TunerPhase> = ph.iter().filter(|p| p.valid).collect();
        let low = if !valid.is_empty() { average(valid.iter().map(|p| p.low1)) } else { 0.0 };
        (low, ph.iter().map(|p| p.hitches).sum())
    }

    /// The result table for the log and the report.
    pub fn table(&self) -> String {
        let ci = Culture::invariant();
        let mut keys: Vec<i32> = Vec::new();
        for p in &self.phases {
            if !keys.contains(&p.variant) {
                keys.push(p.variant);
            }
        }
        keys.sort();
        let mut sb = String::new();
        for k in keys {
            let mut g: Vec<&TunerPhase> = self.phases.iter().filter(|p| p.variant == k).collect();
            let name = format!(
                "{}{}",
                self.variants[k as usize].name,
                if k == self.baseline { format!(" ({})", L::t("current")) } else { String::new() }
            );
            g.sort_by_key(|p| p.round);
            let lows: Vec<String> = g.iter().map(|p| if p.valid { num(p.low1, "0", &ci) } else { "—".into() }).collect();
            let lows01: Vec<String> = g.iter().map(|p| if p.valid { num(p.low01, "0", &ci) } else { "—".into() }).collect();
            let valid: Vec<f64> = g.iter().filter(|p| p.valid).map(|p| p.avg_fps).collect();
            let avg = if valid.is_empty() { 0.0 } else { average(valid.into_iter()) };
            let hitches: i32 = g.iter().map(|p| p.hitches).sum();
            sb.push_str(&format!(
                "{}: 1% low {} FPS, 0.1% low {}, avg {} FPS, hitches {}",
                name,
                lows.join(" / "),
                lows01.join(" / "),
                num(avg, "0", &ci),
                hitches
            ));
            sb.push_str(NL);
        }
        sb.trim_end().to_string()
    }
}

#[cfg(test)]
mod tests {
    //! TunerTests.cs (4 tests).
    use super::*;
    use crate::base::time::Kind;

    fn v() -> Vec<TunerVariant> {
        vec![
            TunerVariant::new("roles", false, false),
            TunerVariant::new("roles + live", false, true),
            TunerVariant::new("top5", true, false),
            TunerVariant::new("top5 + live", true, true),
        ]
    }

    fn frames(ms: f64, spike_ms: f64, spikes: usize, n: usize) -> Vec<f64> {
        let mut l = vec![ms; n];
        for i in 0..spikes {
            l[i * 37 % n] = spike_ms;
        }
        l
    }

    fn day() -> DateTime {
        DateTime::from_parts(2026, 1, 1, 0, 0, 0, Kind::Unspecified).unwrap()
    }

    #[test]
    fn schedule_is_reversed_in_the_second_round_and_skips_the_settle_time() {
        let mut t = AbTuner::new();
        let mut now = day();
        let mut order = vec![t.start(&v(), 0, now).unwrap().name];
        let mut asked = Vec::new();
        for _ in 1..=(AbTuner::PHASE_SECONDS + AbTuner::LAG_SECONDS) * 8 {
            now = now.add_seconds(1.0);
            let (st, next) = t.tick(now, &mut |a, _b| {
                asked.push(a);
                frames(2.0, 2.0, 0, 50)
            });
            if st == Step::Switch {
                order.push(next.unwrap().name);
            }
        }
        assert_eq!(vec!["roles", "roles + live", "top5", "top5 + live", "top5 + live", "top5", "roles + live", "roles"], order);
        assert!(!t.running());
        assert!(!asked.iter().any(|a| *a < day().add_seconds(AbTuner::SETTLE_SECONDS as f64)));
    }

    #[test]
    fn frames_that_arrive_late_are_still_counted() {
        // Frames reach the program in batches about 2 s late: a frame is visible only 2 s after it ended.
        let mut t = AbTuner::new();
        let start = day();
        let mut delivered: Vec<DateTime> = Vec::new();
        t.start(&v()[..2], 0, start);
        let mut first: Option<TunerPhase> = None;
        let mut ms = 0;
        while ms <= (AbTuner::PHASE_SECONDS + AbTuner::LAG_SECONDS) * 1000 + 1000 && first.is_none() {
            let frame_end = start.add_ms(ms as f64); // 500 FPS, one frame every 2 ms
            delivered.push(frame_end);
            if ms % 1000 == 0 {
                let now = frame_end;
                let visible: Vec<DateTime> = delivered.iter().copied().filter(|f| *f <= now.add_seconds(-2.0)).collect();
                t.tick(now, &mut |a, b| visible.iter().filter(|f| **f > a && **f <= b).map(|_| 2.0).collect());
                if !t.phases().is_empty() {
                    first = Some(t.phases()[0].clone());
                }
            }
            ms += 2;
        }
        let first = first.expect("a phase");
        assert!(first.valid);
        let lo = (AbTuner::PHASE_SECONDS - AbTuner::SETTLE_SECONDS) * 500 - 600;
        let hi = (AbTuner::PHASE_SECONDS - AbTuner::SETTLE_SECONDS) * 500 + 10;
        assert!(first.frames >= lo && first.frames <= hi, "{}", first.frames);
    }

    #[test]
    fn measure_gives_lows_and_hitches() {
        let ph = AbTuner::measure(&frames(2.0, 8.0, 30, 2000)); // 500 FPS, 30 spikes of 8 ms in 2000 frames
        assert!(ph.valid);
        assert_eq!(125.0, ph.low1.round()); // 1.5% of frames are 8 ms → p99 = 8 ms
        assert_eq!(30, ph.hitches);
        assert!(!AbTuner::measure(&frames(2.0, 2.0, 0, 100)).valid);
    }

    #[test]
    fn winner_must_be_better_in_every_round() {
        let _g = crate::base::hooks::static_hooks(); // the verdict texts read the language
        let p = |v: i32, r: i32, low1: f64, hitches: i32| TunerPhase { variant: v, round: r, low1, hitches, valid: true, ..Default::default() };
        let two = &v()[..2];
        let both = vec![p(0, 0, 300.0, 0), p(1, 0, 330.0, 0), p(0, 1, 300.0, 0), p(1, 1, 320.0, 0)];
        assert_eq!(1, AbTuner::decide(two, &both, 0).0);
        let once = vec![p(0, 0, 300.0, 0), p(1, 0, 360.0, 0), p(0, 1, 300.0, 0), p(1, 1, 302.0, 0)];
        assert_eq!(-1, AbTuner::decide(two, &once, 0).0); // better only once: no change
        let hitchy = vec![p(0, 0, 300.0, 2), p(1, 0, 330.0, 9), p(0, 1, 300.0, 2), p(1, 1, 330.0, 9)];
        assert_eq!(-1, AbTuner::decide(two, &hitchy, 0).0); // higher lows but more hitches
        let invalid = vec![p(0, 0, 300.0, 0), TunerPhase { variant: 1, round: 0, ..Default::default() }];
        assert_eq!(-1, AbTuner::decide(two, &invalid, 0).0);
    }
}
