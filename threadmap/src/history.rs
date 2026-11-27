//! History: a ring of 300 buckets x 100 ms (30 s). Each bucket keeps, per thread serial, run time, ready-wait time
//! and count, a sparse wait histogram, link counts and the last scheduling state, compactly. The newest bucket is
//! "open" (hash maps that take updates); closing freezes it into sorted vectors. Every record carries the process
//! of its thread(s) at the time it was added, and each bucket keeps per-process sums next to the thread records, so
//! a snapshot of collapsed processes never touches thread records. When a thread moves to another process (named
//! late, tid reuse) the map's proc epoch changes: buckets of an older epoch are read through the slow path and are
//! stamped again when they freeze.

use crate::fx::FxMap;

/// `proc` of a thread the map does not know (any more).
pub const NO_PROC: u32 = u32::MAX;

pub const BUCKET_MS: i64 = 100;
pub const BUCKET_NS: i64 = BUCKET_MS * 1_000_000;
pub const BUCKETS: usize = 300;

/// Wait histogram: bin 0 is < 1 us, bin b >= 1 covers [2^((b-1)/4), 2^(b/4)) us (4 bins per octave, about 19 %
/// wide). 128 bins reach far beyond any real wait.
pub const BINS: usize = 128;

pub fn wait_bin(ns: i64) -> u8 {
    let us = ns as f64 / 1000.0;
    if us < 1.0 {
        0
    } else {
        ((us.log2() * 4.0).floor() as i64 + 1).clamp(1, BINS as i64 - 1) as u8
    }
}

/// The upper edge of a bin in microseconds (a p99 is reported rounded up to its bin).
pub fn bin_upper_us(b: u8) -> f64 {
    if b == 0 {
        1.0
    } else {
        2f64.powf(b as f64 / 4.0)
    }
}

/// p99 the way ReadyProbe.P99 picks it: sorted values, index ceil(0.99 n) - 1. `None` below `min` samples.
pub fn p99_hist(hist: &[u32], min: u32) -> Option<f64> {
    let n: u64 = hist.iter().map(|&c| c as u64).sum();
    if n == 0 || n < min as u64 {
        return None;
    }
    let idx = ((0.99 * n as f64).ceil() as u64).clamp(1, n); // 1-based rank
    let mut acc = 0u64;
    for (b, &c) in hist.iter().enumerate() {
        acc += c as u64;
        if acc >= idx {
            return Some(bin_upper_us(b as u8));
        }
    }
    None
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ThRec {
    pub serial: u32,
    pub proc: u32,
    pub run_ns: u32,
    pub ready_ns: u32,
    pub waits: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WaitRec {
    pub serial: u32,
    pub proc: u32,
    pub bin: u8,
    pub count: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LinkRec {
    pub from: u32,
    pub to: u32,
    pub pf: u32,
    pub pt: u32,
    pub kind: u8,
    pub via_mask: u8,
    pub count: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StateRec {
    pub serial: u32,
    pub proc: u32,
    pub st: u8,
    pub reason: u8,
}

/// Per-process sums of a frozen bucket.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PThRec {
    pub proc: u32,
    pub run_ns: u64,
    pub ready_ns: u64,
    pub waits: u32,
    /// Threads of the process with a record in this bucket.
    pub threads: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PWaitRec {
    pub proc: u32,
    pub bin: u8,
    pub count: u32,
}

/// Process pair links of a frozen bucket, self pairs included (they carry the wakes inside a process).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PLinkRec {
    pub pf: u32,
    pub pt: u32,
    pub kind: u8,
    pub via_mask: u8,
    pub count: u32,
}

/// Per-bucket figures of the game for the time strip.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct GameAgg {
    pub run_ns: u64,
    pub ready_ns: u64,
    pub waits: u32,
    /// p99 wait for a core in us; < 0 when unknown.
    pub p99_us: f32,
}

#[derive(Default)]
pub struct Bucket {
    /// Absolute bucket index (ns / BUCKET_NS); `i64::MIN` = empty slot.
    pub k: i64,
    pub open: bool,
    th_map: FxMap<u32, ThRec>,
    wait_map: FxMap<(u32, u8), WaitRec>,
    link_map: FxMap<(u32, u32, u8), LinkRec>,
    state_map: FxMap<u32, StateRec>,
    pth_map: FxMap<u32, PThRec>,
    pwait_map: FxMap<(u32, u8), u32>,
    plink_map: FxMap<(u32, u32, u8), (u32, u8)>,
    th: Vec<ThRec>,
    waits: Vec<WaitRec>,
    links: Vec<LinkRec>,
    states: Vec<StateRec>,
    p_th: Vec<PThRec>,
    p_waits: Vec<PWaitRec>,
    p_links: Vec<PLinkRec>,
    /// Indices into `links` sorted by the target's process (the sources' order is `links` itself).
    links_pt: Vec<u32>,
    /// ThreadMap::proc_epoch the process stamps belong to.
    pub proc_epoch: u64,
    pub game: GameAgg,
    /// The game pid `game` was computed for (None: not computed).
    pub game_for: Option<u32>,
}

#[inline]
fn sat(ns: i64) -> u32 {
    ns.clamp(0, u32::MAX as i64) as u32
}

impl Bucket {
    fn empty() -> Self {
        Self { k: i64::MIN, ..Default::default() }
    }

    pub fn reset(&mut self, k: i64, epoch: u64, hint: [usize; 7]) {
        *self = Self::empty();
        self.k = k;
        self.open = true;
        self.proc_epoch = epoch;
        // Sized like the previous bucket: no rehashing while it fills.
        self.th_map.reserve(hint[0]);
        self.wait_map.reserve(hint[1]);
        self.link_map.reserve(hint[2]);
        self.state_map.reserve(hint[3]);
        self.pth_map.reserve(hint[4]);
        self.pwait_map.reserve(hint[5]);
        self.plink_map.reserve(hint[6]);
    }

    /// Record counts (sizing hint for the next bucket).
    pub fn sizes(&self) -> [usize; 7] {
        [
            self.th.len() + self.th_map.len(),
            self.waits.len() + self.wait_map.len(),
            self.links.len() + self.link_map.len(),
            self.states.len() + self.state_map.len(),
            self.p_th.len() + self.pth_map.len(),
            self.p_waits.len() + self.pwait_map.len(),
            self.p_links.len() + self.plink_map.len(),
        ]
    }

    #[inline]
    fn th_entry(&mut self, s: u32, proc: u32) -> &mut ThRec {
        let pth = &mut self.pth_map;
        self.th_map.entry(s).or_insert_with(|| {
            pth.entry(proc).or_insert(PThRec { proc, ..Default::default() }).threads += 1;
            ThRec { serial: s, proc, ..Default::default() }
        })
    }

    #[inline]
    fn pth_entry(&mut self, proc: u32) -> &mut PThRec {
        self.pth_map.entry(proc).or_insert(PThRec { proc, ..Default::default() })
    }

    /// Run time of thread `s` (of process `proc`).
    #[inline]
    pub fn add_run(&mut self, s: u32, proc: u32, ns: i64) {
        let e = self.th_entry(s, proc);
        e.run_ns = e.run_ns.saturating_add(sat(ns));
        self.pth_entry(proc).run_ns += sat(ns) as u64;
    }

    /// One wait for a core (ready -> running) of `ns`.
    #[inline]
    pub fn add_wait(&mut self, s: u32, proc: u32, ns: i64) {
        let e = self.th_entry(s, proc);
        e.ready_ns = e.ready_ns.saturating_add(sat(ns));
        e.waits += 1;
        let p = self.pth_entry(proc);
        p.ready_ns += sat(ns) as u64;
        p.waits += 1;
        let b = wait_bin(ns);
        self.wait_map.entry((s, b)).or_insert(WaitRec { serial: s, proc, bin: b, count: 0 }).count += 1;
        *self.pwait_map.entry((proc, b)).or_default() += 1;
    }

    #[allow(clippy::too_many_arguments)]
    #[inline]
    pub fn add_link(&mut self, from: u32, to: u32, pf: u32, pt: u32, kind: u8, via: u8, count: u32) {
        let bit = 1u8 << via.min(7);
        let e = self.link_map.entry((from, to, kind)).or_insert(LinkRec { from, to, pf, pt, kind, via_mask: 0, count: 0 });
        e.count = e.count.saturating_add(count);
        e.via_mask |= bit;
        let p = self.plink_map.entry((pf, pt, kind)).or_default();
        p.0 = p.0.saturating_add(count);
        p.1 |= bit;
    }

    #[inline]
    pub fn set_state(&mut self, s: u32, proc: u32, st: u8, reason: u8) {
        self.state_map.insert(s, StateRec { serial: s, proc, st, reason });
    }

    /// Freeze into sorted, shrunk vectors. When the process stamps are older than `epoch` (a thread moved to
    /// another process meanwhile), every record is stamped again with `proc_of` and the per-process sums rebuilt.
    /// Later updates (rare robustness paths) go to the maps again and the readers below see both.
    pub fn freeze(&mut self, proc_of: &dyn Fn(u32) -> u32, epoch: u64) {
        let mut th: Vec<ThRec> = self.th_map.drain().map(|(_, r)| r).collect();
        let mut waits: Vec<WaitRec> = self.wait_map.drain().map(|(_, r)| r).collect();
        let mut links: Vec<LinkRec> = self.link_map.drain().map(|(_, r)| r).collect();
        let mut states: Vec<StateRec> = self.state_map.drain().map(|(_, r)| r).collect();
        if self.proc_epoch != epoch {
            let mut pc: FxMap<u32, u32> = FxMap::default();
            let mut pof = |s: u32| *pc.entry(s).or_insert_with(|| proc_of(s));
            th.iter_mut().for_each(|r| r.proc = pof(r.serial));
            waits.iter_mut().for_each(|r| r.proc = pof(r.serial));
            links.iter_mut().for_each(|r| {
                r.pf = pof(r.from);
                r.pt = pof(r.to);
            });
            states.iter_mut().for_each(|r| r.proc = pof(r.serial));
            self.pth_map.clear();
            self.pwait_map.clear();
            self.plink_map.clear();
            for r in &th {
                let e = self.pth_map.entry(r.proc).or_insert(PThRec { proc: r.proc, ..Default::default() });
                e.run_ns += r.run_ns as u64;
                e.ready_ns += r.ready_ns as u64;
                e.waits += r.waits;
                e.threads += 1;
            }
            for w in &waits {
                *self.pwait_map.entry((w.proc, w.bin)).or_default() += w.count;
            }
            for l in &links {
                let e = self.plink_map.entry((l.pf, l.pt, l.kind)).or_default();
                e.0 = e.0.saturating_add(l.count);
                e.1 |= l.via_mask;
            }
            self.proc_epoch = epoch;
        }
        // Sorted by process, so a snapshot finds the records of one expanded process by binary search.
        th.sort_unstable_by_key(|r| (r.proc, r.serial));
        waits.sort_unstable_by_key(|r| (r.proc, r.serial, r.bin));
        links.sort_unstable_by_key(|r| (r.pf, r.from, r.to, r.kind));
        states.sort_unstable_by_key(|r| r.serial);
        let mut links_pt: Vec<u32> = (0..links.len() as u32).collect();
        links_pt.sort_unstable_by_key(|&i| (links[i as usize].pt, i));
        self.links_pt = links_pt;
        let mut p_th: Vec<PThRec> = self.pth_map.drain().map(|(_, r)| r).collect();
        p_th.sort_unstable_by_key(|r| r.proc);
        let mut p_waits: Vec<PWaitRec> = self.pwait_map.drain().map(|((p, b), c)| PWaitRec { proc: p, bin: b, count: c }).collect();
        p_waits.sort_unstable_by_key(|r| (r.proc, r.bin));
        let mut p_links: Vec<PLinkRec> =
            self.plink_map.drain().map(|((f, t, k), (c, v))| PLinkRec { pf: f, pt: t, kind: k, via_mask: v, count: c }).collect();
        p_links.sort_unstable_by_key(|r| (r.pf, r.pt, r.kind));
        (self.th, self.waits, self.links, self.states) = (th, waits, links, states);
        (self.p_th, self.p_waits, self.p_links) = (p_th, p_waits, p_links);
        self.th_map = FxMap::default();
        self.wait_map = FxMap::default();
        self.link_map = FxMap::default();
        self.state_map = FxMap::default();
        self.pth_map = FxMap::default();
        self.pwait_map = FxMap::default();
        self.plink_map = FxMap::default();
        self.open = false;
    }

    /// True when the process stamps and per-process sums are right for `epoch` (the snapshot's fast path).
    pub fn stamped(&self, epoch: u64) -> bool {
        self.proc_epoch == epoch
    }

    pub fn for_th(&self, mut f: impl FnMut(ThRec)) {
        self.th.iter().for_each(|r| f(*r));
        self.th_map.values().for_each(|r| f(*r));
    }
    pub fn for_waits(&self, mut f: impl FnMut(WaitRec)) {
        self.waits.iter().for_each(|r| f(*r));
        self.wait_map.values().for_each(|r| f(*r));
    }
    pub fn for_links(&self, mut f: impl FnMut(LinkRec)) {
        self.links.iter().for_each(|r| f(*r));
        self.link_map.values().for_each(|r| f(*r));
    }
    pub fn for_states(&self, mut f: impl FnMut(StateRec)) {
        self.states.iter().for_each(|r| f(*r));
        self.state_map.values().for_each(|r| f(*r));
    }
    /// Thread records of process `proc`.
    pub fn for_th_of(&self, proc: u32, mut f: impl FnMut(ThRec)) {
        let a = self.th.partition_point(|r| r.proc < proc);
        let b = self.th.partition_point(|r| r.proc <= proc);
        self.th[a..b].iter().for_each(|r| f(*r));
        self.th_map.values().filter(|r| r.proc == proc).for_each(|r| f(*r));
    }
    /// Wait records of process `proc`.
    pub fn for_waits_of(&self, proc: u32, mut f: impl FnMut(WaitRec)) {
        let a = self.waits.partition_point(|r| r.proc < proc);
        let b = self.waits.partition_point(|r| r.proc <= proc);
        self.waits[a..b].iter().for_each(|r| f(*r));
        self.wait_map.values().filter(|r| r.proc == proc).for_each(|r| f(*r));
    }
    /// Thread links with at least one end in `procs` (sorted, no duplicates), each once.
    pub fn for_links_touching(&self, procs: &[u32], mut f: impl FnMut(LinkRec)) {
        let inside = |p: u32| procs.binary_search(&p).is_ok();
        for &p in procs {
            let a = self.links.partition_point(|r| r.pf < p);
            let b = self.links.partition_point(|r| r.pf <= p);
            self.links[a..b].iter().for_each(|r| f(*r));
            let a = self.links_pt.partition_point(|&i| self.links[i as usize].pt < p);
            let b = self.links_pt.partition_point(|&i| self.links[i as usize].pt <= p);
            for &i in &self.links_pt[a..b] {
                let r = self.links[i as usize];
                if !inside(r.pf) {
                    f(r);
                }
            }
        }
        self.link_map.values().filter(|r| inside(r.pf) || inside(r.pt)).for_each(|r| f(*r));
    }
    pub fn for_p_th(&self, mut f: impl FnMut(PThRec)) {
        self.p_th.iter().for_each(|r| f(*r));
        self.pth_map.values().for_each(|r| f(*r));
    }
    pub fn for_p_waits(&self, mut f: impl FnMut(PWaitRec)) {
        self.p_waits.iter().for_each(|r| f(*r));
        self.pwait_map.iter().for_each(|(&(p, b), &c)| f(PWaitRec { proc: p, bin: b, count: c }));
    }
    pub fn for_p_links(&self, mut f: impl FnMut(PLinkRec)) {
        self.p_links.iter().for_each(|r| f(*r));
        self.plink_map.iter().for_each(|(&(pf, pt, k), &(c, v))| f(PLinkRec { pf, pt, kind: k, via_mask: v, count: c }));
    }

    /// Approximate heap bytes held (for stats).
    pub fn bytes(&self) -> usize {
        use std::mem::size_of;
        self.th.capacity() * size_of::<ThRec>()
            + self.waits.capacity() * size_of::<WaitRec>()
            + self.links.capacity() * size_of::<LinkRec>()
            + self.states.capacity() * size_of::<StateRec>()
            + self.p_th.capacity() * size_of::<PThRec>()
            + self.p_waits.capacity() * size_of::<PWaitRec>()
            + self.p_links.capacity() * size_of::<PLinkRec>()
            + self.links_pt.capacity() * 4
            + self.th_map.capacity() * (size_of::<ThRec>() + 5)
            + self.wait_map.capacity() * (size_of::<WaitRec>() + 9)
            + self.link_map.capacity() * (size_of::<LinkRec>() + 13)
            + self.state_map.capacity() * (size_of::<StateRec>() + 5)
            + self.pth_map.capacity() * (size_of::<PThRec>() + 5)
            + self.pwait_map.capacity() * 13
            + self.plink_map.capacity() * 21
    }
}

pub struct Ring {
    buckets: Vec<Bucket>,
    /// Newest (open) bucket index; None before the first event.
    pub newest: Option<i64>,
    /// First bucket ever opened (the ring is shorter than 300 at start).
    pub first: i64,
}

impl Default for Ring {
    fn default() -> Self {
        Self { buckets: (0..BUCKETS).map(|_| Bucket::empty()).collect(), newest: None, first: 0 }
    }
}

impl Ring {
    #[inline]
    fn slot(k: i64) -> usize {
        k.rem_euclid(BUCKETS as i64) as usize
    }
    pub fn get(&self, k: i64) -> Option<&Bucket> {
        let b = &self.buckets[Self::slot(k)];
        (b.k == k && k != i64::MIN).then_some(b)
    }
    pub fn get_mut(&mut self, k: i64) -> Option<&mut Bucket> {
        let b = &mut self.buckets[Self::slot(k)];
        (b.k == k && k != i64::MIN).then_some(b)
    }
    /// Oldest bucket still held.
    pub fn oldest(&self) -> Option<i64> {
        self.newest.map(|n| (n - BUCKETS as i64 + 1).max(self.first))
    }
    pub fn open(&mut self, k: i64, epoch: u64) {
        if self.newest.is_none() {
            self.first = k;
        }
        let hint = self.newest.and_then(|n| self.get(n)).map(|b| b.sizes()).unwrap_or([0; 7]);
        self.buckets[Self::slot(k)].reset(k, epoch, hint);
        self.newest = Some(k);
    }
    pub fn clear(&mut self) {
        *self = Self::default();
    }
    pub fn bytes(&self) -> usize {
        self.buckets.iter().map(|b| b.bytes()).sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bins_are_monotonic_and_bounded() {
        assert_eq!(wait_bin(0), 0);
        assert_eq!(wait_bin(999), 0);
        assert_eq!(wait_bin(1000), 1);
        let mut last = 0;
        for us in [1, 2, 5, 10, 50, 100, 1000, 10_000, 100_000, 1_000_000] {
            let b = wait_bin(us * 1000);
            assert!(b >= last);
            assert!(bin_upper_us(b) >= us as f64, "{us} us in bin {b} upper {}", bin_upper_us(b));
            assert!(bin_upper_us(b) <= us as f64 * 1.2 + 1.0);
            last = b;
        }
        assert_eq!(wait_bin(i64::MAX / 2), (BINS - 1) as u8);
    }

    #[test]
    fn p99_follows_ready_probe_rank() {
        let mut h = vec![0u32; BINS];
        // 100 samples: 99 at ~10 us, 1 at ~5 ms -> rank ceil(99) = 99 -> the 10 us bin.
        h[wait_bin(10_000) as usize] = 99;
        h[wait_bin(5_000_000) as usize] = 1;
        assert_eq!(p99_hist(&h, 3), Some(bin_upper_us(wait_bin(10_000))));
        // 10 samples: rank ceil(9.9) = 10 -> the max.
        let mut h = vec![0u32; BINS];
        h[wait_bin(10_000) as usize] = 9;
        h[wait_bin(5_000_000) as usize] = 1;
        assert_eq!(p99_hist(&h, 3), Some(bin_upper_us(wait_bin(5_000_000))));
        let mut h = vec![0u32; BINS];
        h[3] = 2;
        assert_eq!(p99_hist(&h, 3), None);
    }

    #[test]
    fn freeze_keeps_values_and_readers_see_both() {
        let mut b = Bucket::default();
        b.reset(7, 3, [4; 7]);
        b.add_run(5, 50, 1000);
        b.add_run(5, 50, 500);
        b.add_wait(5, 50, 20_000);
        b.add_link(5, 6, 50, 60, 0, 0, 2);
        b.add_link(5, 6, 50, 60, 0, 2, 1);
        b.set_state(5, 50, 1, 0);
        let mut pl = vec![];
        b.for_p_links(|r| pl.push(r));
        assert_eq!(pl, vec![PLinkRec { pf: 50, pt: 60, kind: 0, via_mask: 0b101, count: 3 }]);
        b.freeze(&|s| s * 10, 3);
        assert!(b.stamped(3) && !b.stamped(4));
        let mut pt = vec![];
        b.for_p_th(|r| pt.push(r));
        assert_eq!(pt, vec![PThRec { proc: 50, run_ns: 1500, ready_ns: 20_000, waits: 1, threads: 1 }]);
        b.add_run(6, 60, 10);
        let mut th = vec![];
        b.for_th(|r| th.push(r));
        th.sort_by_key(|r| r.serial);
        assert_eq!(
            th,
            vec![
                ThRec { serial: 5, proc: 50, run_ns: 1500, ready_ns: 20_000, waits: 1 },
                ThRec { serial: 6, proc: 60, run_ns: 10, ready_ns: 0, waits: 0 }
            ]
        );
        let mut l = vec![];
        b.for_links(|r| l.push(r));
        assert_eq!(l, vec![LinkRec { from: 5, to: 6, pf: 50, pt: 60, kind: 0, via_mask: 0b101, count: 3 }]);
    }

    #[test]
    fn per_process_readers() {
        let mut b = Bucket::default();
        b.reset(1, 0, [0; 7]);
        for s in 0..20u32 {
            b.add_run(s, s % 4, 10);
            b.add_wait(s, s % 4, 5000);
            b.add_link(s, (s + 1) % 20, s % 4, ((s + 1) % 20) % 4, 0, 0, 1);
        }
        let check = |b: &Bucket| {
            let mut n = 0;
            b.for_th_of(2, |r| {
                assert_eq!(r.proc, 2);
                n += 1
            });
            assert_eq!(n, 5);
            let mut n = 0;
            b.for_waits_of(3, |_| n += 1);
            assert_eq!(n, 5);
            let mut l = vec![];
            b.for_links_touching(&[1, 2], |r| l.push((r.from, r.to)));
            l.sort();
            let mut want: Vec<(u32, u32)> =
                (0..20u32).map(|s| (s, (s + 1) % 20)).filter(|&(f, t)| matches!(f % 4, 1 | 2) || matches!(t % 4, 1 | 2)).collect();
            want.sort();
            assert_eq!(l, want);
        };
        check(&b);
        b.freeze(&|s| s % 4, 0);
        check(&b);
    }

    #[test]
    fn freeze_restamps_after_a_process_change() {
        let mut b = Bucket::default();
        b.reset(1, 0, [0; 7]);
        b.add_run(5, 0, 100); // unknown process at the time
        b.add_link(5, 6, 0, 0, 0, 0, 1);
        b.freeze(&|s| s * 10, 1);
        let mut pt = vec![];
        b.for_p_th(|r| pt.push(r));
        assert_eq!(pt, vec![PThRec { proc: 50, run_ns: 100, ready_ns: 0, waits: 0, threads: 1 }]);
        let mut pl = vec![];
        b.for_p_links(|r| pl.push(r));
        assert_eq!(pl, vec![PLinkRec { pf: 50, pt: 60, kind: 0, via_mask: 1, count: 1 }]);
        assert!(b.stamped(1));
    }

    #[test]
    fn ring_wraps() {
        let mut r = Ring::default();
        r.open(10, 0);
        for k in 11..=400 {
            r.open(k, 0);
        }
        assert_eq!(r.oldest(), Some(101));
        assert!(r.get(100).is_none());
        assert!(r.get(101).is_some());
        assert!(r.get(400).is_some());
    }
}
