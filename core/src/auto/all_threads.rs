//! 1.17.1: ideal cores for all game threads, the part of Dynamic Top-5 that looks after the threads outside the five
//! slots (port of the keeper of Domain ThreadGraph, `keeper.cpp` + `PlanAllIdeal` in `advice.cpp`).
//!
//! While Auto handles a game, on the same 1 s tick as Top-5 (no thread or trace of its own), every other thread of the
//! game (DLL threads included) gets an ideal processor. Only the ideal processor (a hint: the threads still migrate
//! freely) is ever written, through the engine (`Engine::place_ideals`: baseline, crash-safe session, revert); no
//! affinity, no CPU Sets, no priority.
//!
//! The rules (the same as ThreadGraph's):
//! - never core 0 (both its processors), never a core whose processor drivers keep busy (DPC/ISR share above 1.5% and
//!   3x the median, held 60 s, plus the graphics-driver cores), never a slower core on a hybrid processor, never a
//!   processor outside the game mask when one is in force, never a Top-5 slot processor; the SMT sibling of a slot
//!   only after every quiet core (it counts 0.5 processors busier);
//! - new threads (all of them on the first tick) are planned together, heaviest first, each onto the processor with
//!   the lowest projected load of its core; the busiest ones (5% of a processor or more) keep different physical cores
//!   while there are enough; idle ones go round-robin; a current ideal processor that is allowed and within 0.1 of the
//!   best is kept without a write; at most 32 writes per tick;
//! - a placed thread moves only for a clear reason: its processor is no longer allowed (driver, slot, rule), or its
//!   core has been worse than the best by more than the margin for 3 ticks in a row (an idle thread never moves for
//!   load: where it sleeps does not matter); after a move it stays 2 s; at most 2 moves per tick, heaviest first;
//! - a thread that enters Top-5 belongs to Top-5 (this layer lets go without a write: Top-5 keeps the original
//!   baseline); one that leaves it is taken over again; a thread whose ideal processor someone else changed is left to
//!   them for the rest of its life; a failed write backs the thread off for 30 s.
//!
//! The steady tick (no new threads, no moves) allocates nothing: every buffer keeps its capacity.

use crate::base::time::DateTime;
use crate::topo::model::Topo;

/// The numbers of the layer. Not settings.
pub struct AllPolicy;

impl AllPolicy {
    pub const MAX_MOVES: usize = 2;
    /// writes per tick for threads not placed yet (the first placement, new threads)
    pub const FIRST_BUDGET: usize = 32;
    pub const HYSTERESIS_TICKS: i32 = 3;
    pub const DWELL_MS: i64 = 2000;
    pub const BACKOFF_MS: i64 = 30000;
    /// in processors: a core this much worse than the best still counts as good
    pub const MARGIN: f64 = 0.10;
    /// threads at least this busy get different physical cores where possible
    pub const HEAVY: f64 = 0.05;
    /// below this a thread is idle: round-robin
    pub const IDLE: f64 = 0.005;
    pub const SIBLING_PENALTY: f64 = 0.50;
    pub const DRIVER_MIN_SHARE: f64 = 0.015;
    pub const DRIVER_RATIO: f64 = 3.0;
    pub const DRIVER_HOLD_MS: i64 = 60000;
    pub const EVENTS: usize = 8;
}

const TICKS_PER_MS: i64 = 10_000;

/// Why a thread was written or let go.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AllReason {
    #[default]
    New,
    /// drivers keep its core busy
    Driver,
    /// its processor became a Top-5 slot
    Slot,
    /// its processor is not allowed any more (core 0, mask, slower core)
    Rule,
    /// its core stayed busier than the best one
    Worse,
    /// it entered Top-5
    ToTop5,
    /// someone else changed its ideal processor
    Other,
    /// Windows refused the write
    Failed,
}

/// One entry of the short event list (moves and hand-overs; first placements are only counted).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct AllEvent {
    pub at: DateTime,
    pub tid: u32,
    pub from: i32,
    pub to: i32,
    pub reason: AllReason,
}

/// One game thread as the tick sees it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct AllThread {
    pub tid: u32,
    pub born: i64,
    /// processors busy since the last tick (1.0 = one logical processor)
    pub load: f64,
    /// the ideal processor now (-1 = could not be read; a Top-5 thread: its slot processor)
    pub ideal: i32,
    /// a Top-5 slot holds it
    pub top5: bool,
    /// its module is known and it is not an anti-cheat thread
    pub allowed: bool,
}

/// A write the tick wants: `idx` is the thread's index in the tick's input.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AllWrite {
    pub tid: u32,
    pub idx: usize,
    pub from: i32,
    pub to: u32,
    pub reason: AllReason,
}

/// What to do with a thread the layer lets go.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AllRelease {
    /// put the original back (only while the value is still ours: `Engine::release_ideal`)
    Restore,
    /// someone else's value is in force: forget without a write (`Engine::forget_ideal`)
    Forget,
}

/// The processors as the layer sees them (built once when Top-5 starts on a game).
#[derive(Clone, Debug, Default)]
pub struct AllCpus {
    n: usize,
    core_of: Vec<i16>,
    core_mask: Vec<u64>,
    /// processors the fixed rules allow (group 0, fastest class, not core 0, inside the mask)
    base: u64,
}

impl AllCpus {
    /// The fixed rules: group 0 (ideal processors live in the thread's group), the fastest efficiency class on a
    /// hybrid processor, never core 0, inside `mask` when it is not 0.
    pub fn new(t: &Topo, mask: u64) -> AllCpus {
        let sets: Vec<_> = t.sets.iter().filter(|s| s.group == 0 && s.lp < 64).collect();
        let n = sets.iter().map(|s| s.lp as usize + 1).max().unwrap_or(0);
        let mut core_of = vec![-1i16; n];
        let mut ids: Vec<u8> = Vec::new();
        let mut core_mask: Vec<u64> = Vec::new();
        for s in &sets {
            let c = match ids.iter().position(|x| *x == s.core) {
                Some(c) => c,
                None => {
                    ids.push(s.core);
                    core_mask.push(0);
                    ids.len() - 1
                }
            };
            core_of[s.lp as usize] = c as i16;
            core_mask[c] |= 1u64 << s.lp;
        }
        let top = sets.iter().map(|s| s.eff).max().unwrap_or(0);
        let core0 = if sets.iter().any(|s| s.lp == 0) { core_of[0] } else { -1 };
        let mut base = 0u64;
        for s in &sets {
            let x = s.lp as usize;
            if s.eff == top && core_of[x] != core0 && (mask == 0 || (mask >> x) & 1 == 1) {
                base |= 1u64 << x;
            }
        }
        AllCpus { n, core_of, core_mask, base }
    }

    pub fn base(&self) -> u64 {
        self.base
    }

    fn core_bits(&self, cpu: usize) -> u64 {
        match self.core_of.get(cpu) {
            Some(c) if *c >= 0 => self.core_mask[*c as usize],
            _ => 0,
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct Ts {
    tid: u32,
    born: i64,
    idx: usize,
    load: f64,
    /// last value seen (or written)
    ideal: i32,
    /// the value the layer wrote and still owns, -1 none
    ours: i32,
    placed: bool,
    hands_off: bool,
    top5: bool,
    allowed: bool,
    seen: bool,
    pending: bool,
    worse: i32,
    dwell_until: i64,
    backoff_until: i64,
}

impl Ts {
    fn new(tid: u32, born: i64) -> Ts {
        Ts { tid, born, ideal: -1, ours: -1, ..Default::default() }
    }
}

/// Counters for the page and the log.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct AllCounters {
    /// first placements written
    pub placed: i32,
    /// first placements kept as they were (no write)
    pub adopted: i32,
    /// moves of threads already placed
    pub moves: i32,
    /// handed to Top-5 or left to someone else
    pub released: i32,
    pub failed: i32,
    /// the last tick had something to write but the gate was closed
    pub blocked: bool,
}

/// The decisions, without any Windows call.
#[derive(Clone, Debug)]
pub struct AllPlan {
    cpus: AllCpus,
    states: Vec<Ts>,
    /// what the last step wants written
    pub writes: Vec<AllWrite>,
    /// threads the last step lets go
    pub releases: Vec<(u32, AllRelease)>,
    cand: Vec<(usize, f64, AllReason)>,
    order: Vec<usize>,
    rr: Vec<usize>,
    proj: [f64; 64],
    tier: [i8; 64],
    driver_seen: [i64; 64],
    driver: u64,
    slots: u64,
    eligible: u64,
    events: [AllEvent; AllPolicy::EVENTS],
    ev_head: usize,
    ev_len: usize,
    counters: AllCounters,
}

impl AllPlan {
    pub fn new(cpus: AllCpus) -> AllPlan {
        AllPlan {
            cpus,
            states: Vec::with_capacity(1024),
            writes: Vec::with_capacity(64),
            releases: Vec::with_capacity(64),
            cand: Vec::with_capacity(1024),
            order: Vec::with_capacity(1024),
            rr: Vec::with_capacity(64),
            proj: [0.0; 64],
            tier: [-1; 64],
            driver_seen: [i64::MIN; 64],
            driver: 0,
            slots: 0,
            eligible: 0,
            events: [AllEvent::default(); AllPolicy::EVENTS],
            ev_head: 0,
            ev_len: 0,
            counters: AllCounters::default(),
        }
    }

    pub fn counters(&self) -> AllCounters {
        self.counters
    }
    /// Processors the rules allow right now.
    pub fn eligible(&self) -> u64 {
        self.eligible
    }
    /// Driver-busy processors (whole cores) right now.
    pub fn driver_cpus(&self) -> u64 {
        self.driver
    }
    /// Threads whose ideal processor the layer set and still owns: (tid, processor).
    pub fn owned(&self) -> impl Iterator<Item = (u32, u32)> + '_ {
        self.states.iter().filter(|s| s.ours >= 0).map(|s| (s.tid, s.ours as u32))
    }
    /// (placed by a write, kept as they were, processors in use).
    pub fn tracked(&self) -> (i32, i32, u64) {
        let (mut w, mut k, mut cpus) = (0, 0, 0u64);
        for s in self.states.iter().filter(|s| s.placed && !s.top5 && !s.hands_off && s.allowed) {
            if s.ours >= 0 {
                w += 1;
            } else {
                k += 1;
            }
            if s.ideal >= 0 && s.ideal < 64 {
                cpus |= 1u64 << s.ideal;
            }
        }
        (w, k, cpus)
    }
    /// The events, newest first.
    pub fn events(&self) -> Vec<AllEvent> {
        (0..self.ev_len).map(|i| self.events[(self.ev_head + AllPolicy::EVENTS - 1 - i) % AllPolicy::EVENTS]).collect()
    }

    fn event(&mut self, at: DateTime, tid: u32, from: i32, to: i32, reason: AllReason) {
        self.events[self.ev_head] = AllEvent { at, tid, from, to, reason };
        self.ev_head = (self.ev_head + 1) % AllPolicy::EVENTS;
        self.ev_len = (self.ev_len + 1).min(AllPolicy::EVENTS);
    }

    fn find(&self, tid: u32) -> Result<usize, usize> {
        self.states.binary_search_by(|s| s.tid.cmp(&tid))
    }

    fn own(&self, x: usize, self_cpu: i32, self_load: f64) -> f64 {
        let mut v = self.proj[x];
        if x as i32 == self_cpu {
            v -= self_load;
        }
        v.max(0.0)
    }

    /// Projected load of the processor's core plus the processor itself (without the thread), plus the sibling
    /// penalty.
    fn score(&self, cpu: usize, self_cpu: i32, self_load: f64) -> f64 {
        let mut core = 0.0;
        let mut m = self.cpus.core_bits(cpu);
        while m != 0 {
            let x = m.trailing_zeros() as usize;
            m &= m - 1;
            core += self.own(x, self_cpu, self_load);
        }
        core + self.own(cpu, self_cpu, self_load) + if self.tier[cpu] == 1 { AllPolicy::SIBLING_PENALTY } else { 0.0 }
    }

    /// The best allowed processor (lowest score, then the lowest number), skipping the cores in `skip_cores`.
    fn best(&self, self_cpu: i32, self_load: f64, skip_cores: u64) -> i32 {
        let mut best = -1i32;
        let mut bs = 0.0;
        let mut m = self.eligible;
        while m != 0 {
            let x = m.trailing_zeros() as usize;
            m &= m - 1;
            let c = self.cpus.core_of[x];
            if c >= 0 && (skip_cores >> c) & 1 == 1 {
                continue;
            }
            let v = self.score(x, self_cpu, self_load);
            if best < 0 || v < bs - 1e-9 {
                best = x as i32;
                bs = v;
            }
        }
        best
    }

    fn is_eligible(&self, cpu: i32) -> bool {
        (0..64).contains(&cpu) && (self.eligible >> cpu) & 1 == 1
    }

    /// The driver-busy processors: share above DRIVER_MIN_SHARE and DRIVER_RATIO x the median, held DRIVER_HOLD_MS;
    /// the graphics-driver cores (`gpu`, already held by the trace) too; then whole cores.
    fn drivers(&mut self, now: i64, irq: &[f64; 64], gpu: u64) {
        let n = self.cpus.n.min(64);
        let mut sorted = [0.0f64; 64];
        sorted[..n].copy_from_slice(&irq[..n]);
        sorted[..n].sort_unstable_by(|a, b| a.total_cmp(b));
        let median = if n == 0 {
            0.0
        } else if n % 2 == 1 {
            sorted[n / 2]
        } else {
            (sorted[n / 2 - 1] + sorted[n / 2]) * 0.5
        };
        let mut cpus = gpu;
        for x in 0..n {
            if n >= 2 && irq[x] > AllPolicy::DRIVER_MIN_SHARE && irq[x] > AllPolicy::DRIVER_RATIO * median {
                self.driver_seen[x] = now;
            }
            if self.driver_seen[x] != i64::MIN && now - self.driver_seen[x] < AllPolicy::DRIVER_HOLD_MS * TICKS_PER_MS {
                cpus |= 1u64 << x;
            }
        }
        let mut whole = 0u64;
        let mut m = cpus;
        while m != 0 {
            let x = m.trailing_zeros() as usize;
            m &= m - 1;
            whole |= self.cpus.core_bits(x) | (1u64 << x);
        }
        self.driver = whole;
    }

    fn rules(&mut self) {
        self.tier = [-1; 64];
        self.eligible = 0;
        for c in 0..self.cpus.core_mask.len() {
            let all = self.cpus.core_mask[c];
            let cm = all & self.cpus.base;
            if cm == 0 || all & self.driver != 0 {
                continue;
            }
            let slot = all & self.slots;
            let free = cm & !slot;
            if free == 0 {
                continue;
            }
            let mut m = free;
            while m != 0 {
                let x = m.trailing_zeros() as usize;
                m &= m - 1;
                self.tier[x] = if slot != 0 { 1 } else { 0 };
            }
            self.eligible |= free;
        }
    }

    /// One tick. `busy` = share of each processor's time that was busy since the last tick (may be empty), `irq` =
    /// DPC/ISR share per processor from the live trace (zeros when it is off), `gpu` = graphics-driver cores,
    /// `slot_cpus` = the processors of Top-5's slots (filled or not; the processors of the threads Top-5 holds are
    /// added from `threads`).
    /// Fills `writes` and `releases`; the caller performs them and reports each write back with `confirm` (or
    /// `cancel` when the gate is closed).
    pub fn step(&mut self, now: DateTime, threads: &[AllThread], busy: &[f64], irq: &[f64; 64], gpu: u64, slot_cpus: u64) {
        let now_t = now.ticks;
        self.writes.clear();
        self.releases.clear();
        self.counters.blocked = false;
        for s in self.states.iter_mut() {
            s.seen = false;
            s.pending = false;
        }
        self.slots = slot_cpus;

        // 1. Every live thread: Top-5 hand-over, allowed or not, changes made by someone else.
        for (i, t) in threads.iter().enumerate() {
            let si = match self.find(t.tid) {
                Ok(si) => si,
                Err(at) => {
                    self.states.insert(at, Ts::new(t.tid, t.born));
                    at
                }
            };
            if self.states[si].born != t.born {
                self.states[si] = Ts::new(t.tid, t.born); // the thread ID belongs to another thread now
            }
            let mut st = self.states[si];
            st.seen = true;
            st.idx = i;
            st.load = t.load.max(0.0);
            st.allowed = t.allowed;
            if t.top5 {
                if (0..64).contains(&t.ideal) {
                    self.slots |= 1u64 << t.ideal;
                }
                if !st.top5 {
                    if st.placed {
                        // Top-5's own write already took it (its baseline keeps the original): nothing to write here.
                        self.counters.released += 1;
                        self.event(now, t.tid, st.ideal, t.ideal, AllReason::ToTop5);
                    }
                    st.top5 = true;
                    st.placed = false;
                    st.ours = -1;
                    st.worse = 0;
                }
                st.ideal = t.ideal;
                self.states[si] = st;
                continue;
            }
            if st.top5 {
                // Left Top-5 (its original values were put back): taken over like a new thread.
                st.top5 = false;
                st.placed = false;
                st.worse = 0;
                st.dwell_until = 0;
            }
            if !t.allowed {
                if st.ours >= 0 {
                    self.releases.push((t.tid, AllRelease::Restore));
                }
                st.ours = -1;
                st.placed = false;
                self.states[si] = st;
                continue;
            }
            if st.hands_off || st.backoff_until > now_t {
                self.states[si] = st;
                continue;
            }
            if t.ideal < 0 {
                st.backoff_until = now_t + AllPolicy::BACKOFF_MS * TICKS_PER_MS;
                self.states[si] = st;
                continue;
            }
            if st.ours >= 0 && t.ideal != st.ours {
                // Someone else set it: theirs now, for the rest of the thread's life.
                self.releases.push((t.tid, AllRelease::Forget));
                self.counters.released += 1;
                self.event(now, t.tid, st.ours, t.ideal, AllReason::Other);
                st.ours = -1;
                st.placed = false;
                st.hands_off = true;
            }
            st.ideal = t.ideal;
            self.states[si] = st;
        }
        self.states.retain(|s| s.seen);
        self.drivers(now_t, irq, gpu);
        self.rules();

        // Projected load: what the processor did since the last tick, at least what the threads whose ideal processor
        // is known (Top-5 slots, placed threads) bring to it.
        let n = self.cpus.n.min(64);
        let mut kept = [0.0f64; 64];
        for s in &self.states {
            if (s.top5 || (s.placed && s.allowed && !s.hands_off)) && (0..64).contains(&s.ideal) {
                kept[s.ideal as usize] += s.load;
            }
        }
        for x in 0..64 {
            let b = if x < n { busy.get(x).copied().unwrap_or(0.0) } else { 0.0 };
            self.proj[x] = b.max(kept[x]);
        }

        let writable = |s: &Ts| s.allowed && !s.top5 && !s.hands_off && s.backoff_until <= now_t && s.ideal >= 0;

        // 2. First placements (new threads; every thread on the first tick), heaviest first.
        self.order.clear();
        for (i, s) in self.states.iter().enumerate() {
            if writable(s) && !s.placed {
                self.order.push(i);
            }
        }
        if !self.order.is_empty() {
            if self.eligible == 0 {
                for k in 0..self.order.len() {
                    let i = self.order[k];
                    self.states[i].placed = true; // nothing is allowed: no write; moved once a processor is allowed again
                }
            } else {
                let states = &self.states;
                self.order.sort_unstable_by(|a, b| states[*b].load.total_cmp(&states[*a].load).then(states[*a].tid.cmp(&states[*b].tid)));
                let mut eligible_cores = 0u64;
                let mut m = self.eligible;
                while m != 0 {
                    let x = m.trailing_zeros() as usize;
                    m &= m - 1;
                    eligible_cores |= 1u64 << self.cpus.core_of[x];
                }
                let mut heavy_cores = 0u64;
                let mut budget = AllPolicy::FIRST_BUDGET;
                for k in 0..self.order.len() {
                    let i = self.order[k];
                    let s = self.states[i];
                    if s.load < AllPolicy::IDLE {
                        continue; // below, round-robin
                    }
                    let heavy = s.load >= AllPolicy::HEAVY && heavy_cores.count_ones() < eligible_cores.count_ones();
                    let skip = if heavy { heavy_cores } else { 0 };
                    // The thread is not counted anywhere yet (its ideal processor is not known to the plan).
                    let best = self.best(-1, 0.0, skip);
                    if best < 0 {
                        continue;
                    }
                    let cur = s.ideal;
                    let cur_ok = self.is_eligible(cur) && (skip >> self.cpus.core_of[cur as usize]) & 1 == 0;
                    let (cpu, keep) = if cur_ok && self.score(cur as usize, -1, 0.0) <= self.score(best as usize, -1, 0.0) + AllPolicy::MARGIN + 1e-9 {
                        (cur, true)
                    } else {
                        (best, false)
                    };
                    if !keep && budget == 0 {
                        continue; // next tick
                    }
                    if heavy {
                        heavy_cores |= 1u64 << self.cpus.core_of[cpu as usize];
                    }
                    self.proj[cpu as usize] += s.load;
                    if keep {
                        self.states[i].placed = true;
                        self.counters.adopted += 1;
                    } else {
                        budget -= 1;
                        self.states[i].pending = true;
                        self.writes.push(AllWrite { tid: s.tid, idx: s.idx, from: cur, to: cpu as u32, reason: AllReason::New });
                    }
                }
                // Idle threads: an allowed current ideal is kept, the rest go round-robin over the quiet cores'
                // processors (the slot siblings only when no core is quiet), least loaded first.
                self.rr.clear();
                for want in 0..2i8 {
                    if !self.rr.is_empty() {
                        break;
                    }
                    let mut m = self.eligible;
                    while m != 0 {
                        let x = m.trailing_zeros() as usize;
                        m &= m - 1;
                        if self.tier[x] == want {
                            self.rr.push(x);
                        }
                    }
                }
                let mut rr = std::mem::take(&mut self.rr);
                rr.sort_unstable_by(|a, b| self.score(*a, -1, 0.0).total_cmp(&self.score(*b, -1, 0.0)).then(a.cmp(b)));
                let mut next = 0usize;
                for k in 0..self.order.len() {
                    let i = self.order[k];
                    let s = self.states[i];
                    if s.load >= AllPolicy::IDLE {
                        continue;
                    }
                    if self.is_eligible(s.ideal) {
                        self.states[i].placed = true;
                        self.counters.adopted += 1;
                    } else if budget > 0 && !rr.is_empty() {
                        let cpu = rr[next % rr.len()];
                        next += 1;
                        budget -= 1;
                        self.states[i].pending = true;
                        self.writes.push(AllWrite { tid: s.tid, idx: s.idx, from: s.ideal, to: cpu as u32, reason: AllReason::New });
                    }
                }
                self.rr = rr;
            }
        }

        // 3. Moves of placed threads, only for a clear reason.
        self.cand.clear();
        if self.eligible != 0 {
            for i in 0..self.states.len() {
                let s = self.states[i];
                if !writable(&s) || !s.placed || s.pending {
                    continue;
                }
                let cur = s.ideal;
                let mut have = true;
                let mut reason = AllReason::Worse;
                if !self.is_eligible(cur) {
                    reason = if self.driver >> cur & 1 == 1 {
                        AllReason::Driver
                    } else if self.slots >> cur & 1 == 1 {
                        AllReason::Slot
                    } else {
                        AllReason::Rule
                    };
                    self.states[i].worse = 0;
                } else if s.load < AllPolicy::IDLE {
                    // Where an idle thread sleeps does not matter: it moves only when its processor is no longer
                    // allowed (ThreadGraph moved it for load too, one write per idle thread for nothing).
                    self.states[i].worse = 0;
                    have = false;
                } else {
                    let best = self.best(cur, s.load, 0);
                    let worse = best >= 0
                        && best != cur
                        && self.score(cur as usize, cur, s.load) > self.score(best as usize, cur, s.load) + AllPolicy::MARGIN + 1e-9;
                    self.states[i].worse = if worse { s.worse + 1 } else { 0 };
                    have = self.states[i].worse >= AllPolicy::HYSTERESIS_TICKS;
                }
                if have && now_t >= s.dwell_until {
                    self.cand.push((i, s.load, reason));
                }
            }
        }
        let states = &self.states;
        self.cand.sort_unstable_by(|a, b| b.1.total_cmp(&a.1).then(states[a.0].tid.cmp(&states[b.0].tid)));
        let mut moves = 0;
        for k in 0..self.cand.len() {
            if moves >= AllPolicy::MAX_MOVES {
                break;
            }
            let (i, load, reason) = self.cand[k];
            let s = self.states[i];
            let from = s.ideal;
            let counted = (0..64).contains(&from);
            let best = self.best(if counted { from } else { -1 }, load, 0);
            if best < 0 || best == from {
                continue;
            }
            moves += 1;
            if (0..64).contains(&from) {
                self.proj[from as usize] = (self.proj[from as usize] - load).max(0.0);
            }
            self.proj[best as usize] += load;
            self.states[i].pending = true;
            self.writes.push(AllWrite { tid: s.tid, idx: s.idx, from, to: best as u32, reason });
        }
    }

    /// The result of write `w` (an entry of `writes`).
    pub fn confirm(&mut self, now: DateTime, w: usize, ok: bool) {
        let wr = self.writes[w];
        let Ok(i) = self.find(wr.tid) else { return };
        let st = &mut self.states[i];
        st.pending = false;
        if ok {
            st.ours = wr.to as i32;
            st.ideal = wr.to as i32;
            st.placed = true;
            st.dwell_until = now.ticks + AllPolicy::DWELL_MS * TICKS_PER_MS;
            st.worse = 0;
            if wr.reason == AllReason::New {
                self.counters.placed += 1;
            } else {
                self.counters.moves += 1;
                self.event(now, wr.tid, wr.from, wr.to as i32, wr.reason);
            }
        } else {
            st.backoff_until = now.ticks + AllPolicy::BACKOFF_MS * TICKS_PER_MS;
            self.counters.failed += 1;
            self.event(now, wr.tid, wr.from, wr.to as i32, AllReason::Failed);
        }
    }

    /// The gate is closed: nothing of this tick is written (the threads stay as they are and are tried next tick).
    pub fn cancel(&mut self) {
        if !self.writes.is_empty() {
            self.counters.blocked = true;
        }
        for w in 0..self.writes.len() {
            let tid = self.writes[w].tid;
            if let Ok(i) = self.find(tid) {
                self.states[i].pending = false;
            }
        }
        self.writes.clear();
    }

    /// Everything the layer owns was put back (or the engine reverted it): forget every thread.
    pub fn reset(&mut self) {
        self.states.clear();
        self.writes.clear();
        self.releases.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::base::time::Kind;
    use crate::topo::synth;

    fn t0() -> DateTime {
        DateTime::from_parts(2026, 1, 1, 12, 0, 0, Kind::Utc).unwrap()
    }
    fn th(tid: u32, load: f64, ideal: i32) -> AllThread {
        AllThread { tid, born: tid as i64, load, ideal, top5: false, allowed: true }
    }
    fn slot(tid: u32, load: f64, lp: i32) -> AllThread {
        AllThread { tid, born: tid as i64, load, ideal: lp, top5: true, allowed: true }
    }
    fn bits(v: &[usize]) -> u64 {
        v.iter().fold(0, |m, x| m | 1u64 << x)
    }
    /// Applies the step's writes as Windows would (all accepted) and returns them.
    fn run(p: &mut AllPlan, now: DateTime, ts: &mut [AllThread], irq: &[f64; 64]) -> Vec<AllWrite> {
        p.step(now, ts, &[], irq, 0, 0);
        let w = p.writes.clone();
        for (k, x) in w.iter().enumerate() {
            ts[x.idx].ideal = x.to as i32;
            p.confirm(now, k, true);
        }
        w
    }
    /// The owner's PC: 7800X3D, Top-5 on 2,4,6,8,10, the graphics driver on 12-15, 150 game threads.
    fn owner() -> (AllPlan, Vec<AllThread>, [f64; 64]) {
        let p = AllPlan::new(AllCpus::new(&synth::x7800x3d(), 0));
        let mut ts = vec![slot(1, 0.9, 2), slot(2, 0.6, 4), slot(3, 0.3, 6), slot(4, 0.2, 8), slot(5, 0.1, 10)];
        for i in 0..150u32 {
            let load = match i {
                0..=4 => 0.08,
                5..=29 => 0.01,
                _ => 0.0005,
            };
            ts.push(th(100 + i, load, (i % 16) as i32)); // Windows' own round-robin ideal processors
        }
        let mut irq = [0.0; 64];
        for x in 12..16 {
            irq[x] = 0.06;
        }
        (p, ts, irq)
    }

    #[test]
    fn on_the_owners_pc_only_the_siblings_of_the_slots_are_used() {
        let (mut p, mut ts, irq) = owner();
        let mut now = t0();
        let mut writes = run(&mut p, now, &mut ts, &irq).len();
        // the five busiest got five different physical cores
        let heavy: std::collections::HashSet<i32> = ts.iter().filter(|t| !t.top5 && t.load >= 0.05).map(|t| t.ideal).collect();
        assert_eq!(5, heavy.len());
        now = now.add_seconds(1.0);
        for _ in 0..8 {
            writes += run(&mut p, now, &mut ts, &irq).len();
            now = now.add_seconds(1.0);
        }
        assert_eq!(bits(&[3, 5, 7, 9, 11]), p.eligible());
        assert_eq!(bits(&[12, 13, 14, 15]), p.driver_cpus());
        for t in ts.iter().filter(|t| !t.top5) {
            assert!([3, 5, 7, 9, 11].contains(&t.ideal), "thread {} on {}", t.tid, t.ideal);
        }
        let (w, k, cpus) = p.tracked();
        assert_eq!(150, w + k);
        assert_eq!(bits(&[3, 5, 7, 9, 11]), cpus);
        // threads whose Windows ideal was already allowed were kept without a write (and the budget held: 32 per tick)
        assert!(k > 0, "{} writes, {} kept", writes, k);
        assert_eq!(writes as i32, p.counters().placed + p.counters().moves);
        // steady: nothing more is written
        for _ in 0..20 {
            assert!(run(&mut p, now, &mut ts, &irq).is_empty());
            now = now.add_seconds(1.0);
        }
    }

    #[test]
    fn the_first_placement_writes_at_most_32_per_tick_then_moves_at_most_2() {
        let (mut p, mut ts, irq) = owner();
        let now = t0();
        let w = run(&mut p, now, &mut ts, &irq);
        assert!(w.len() <= AllPolicy::FIRST_BUDGET && w.len() > 2);
        assert!(w.iter().all(|x| x.reason == AllReason::New));
        // later a whole batch of threads becomes not allowed (the slot moves): at most two moves per tick
        let mut now = now;
        for _ in 0..10 {
            now = now.add_seconds(1.0);
            run(&mut p, now, &mut ts, &irq);
        }
        let on3 = ts.iter().filter(|t| !t.top5 && t.ideal == 3).count();
        assert!(on3 > 2);
        ts[0].ideal = 3; // slot 1 moves from 2 to 3
        let mut slot_moves = 0;
        for _ in 0..(on3 + 4) {
            now = now.add_seconds(1.0);
            let w = run(&mut p, now, &mut ts, &irq);
            assert!(w.len() <= AllPolicy::MAX_MOVES);
            assert!(w.iter().all(|x| x.to != 3 && x.to != 2 || x.reason != AllReason::Slot));
            slot_moves += w.iter().filter(|x| x.reason == AllReason::Slot).count();
        }
        assert_eq!(on3, slot_moves);
        assert!(ts.iter().all(|t| t.top5 || t.ideal != 3));
    }

    #[test]
    fn a_move_needs_three_worse_ticks_and_the_dwell_time() {
        // 4 cores without SMT, no slots: allowed 1, 2, 3 (core 0 never)
        let mut p = AllPlan::new(AllCpus::new(&synth::symmetric(4, false, 4, &[8 << 20], "4c", 0), 0));
        let irq = [0.0; 64];
        let mut ts = vec![th(10, 0.3, 1), th(11, 0.3, 2), th(12, 0.3, 3)];
        let mut now = t0();
        assert!(run(&mut p, now, &mut ts, &irq).is_empty()); // all three kept where they are
        let mut busy = vec![0.0, 0.3, 0.3, 0.3];
        // another program loads processor 3: two ticks are not enough
        busy[3] = 1.0;
        for _ in 0..2 {
            now = now.add_seconds(1.0);
            p.step(now, &ts, &busy, &irq, 0, 0);
            assert!(p.writes.is_empty());
        }
        now = now.add_seconds(1.0);
        p.step(now, &ts, &busy, &irq, 0, 0);
        assert_eq!(1, p.writes.len());
        let w = p.writes[0];
        assert_eq!((12, 3, AllReason::Worse), (w.tid, w.from, w.reason));
        ts[w.idx].ideal = w.to as i32;
        p.confirm(now, 0, true);
        // the processor it went to gets busy at once: it stays at least 2 s (and 3 ticks)
        busy = vec![0.0, 0.3, 0.3, 0.3];
        busy[w.to as usize] = 1.0;
        busy[3] = 0.0;
        for _ in 0..2 {
            now = now.add_ms(500.0);
            p.step(now, &ts, &busy, &irq, 0, 0);
            assert!(p.writes.is_empty());
        }
        assert_eq!(1, p.counters().moves);
        assert_eq!(1, p.events().len());
    }

    #[test]
    fn top5_takes_a_thread_and_gives_it_back() {
        let (mut p, mut ts, irq) = owner();
        let mut now = t0();
        for _ in 0..6 {
            run(&mut p, now, &mut ts, &irq);
            now = now.add_seconds(1.0);
        }
        let i = ts.iter().position(|t| t.tid == 100).unwrap();
        assert!(p.owned().any(|o| o.0 == 100) || p.tracked().0 + p.tracked().1 == 150);
        // it enters Top-5: no write, no release here (Top-5's own write took it)
        ts[i].top5 = true;
        ts[i].ideal = 12;
        p.step(now, &ts, &[], &irq, 0, 0);
        assert!(p.writes.iter().all(|w| w.tid != 100));
        assert!(p.releases.is_empty());
        assert!(!p.owned().any(|o| o.0 == 100));
        assert_eq!(AllReason::ToTop5, p.events()[0].reason);
        // it leaves Top-5 with its original (core 0) back: taken over as a new thread at once
        ts[i].top5 = false;
        ts[i].ideal = 0;
        now = now.add_seconds(1.0);
        let w = run(&mut p, now, &mut ts, &irq);
        assert!(w.iter().any(|w| w.tid == 100 && w.reason == AllReason::New));
        assert!([3, 5, 7, 9, 11].contains(&ts[i].ideal));
    }

    #[test]
    fn someone_elses_change_is_left_alone_and_unallowed_threads_are_released() {
        let mut p = AllPlan::new(AllCpus::new(&synth::x7800x3d(), 0));
        let irq = [0.0; 64];
        let mut ts = vec![th(10, 0.2, 0), th(11, 0.2, 0)];
        let now = t0();
        assert_eq!(2, run(&mut p, now, &mut ts, &irq).len());
        ts[0].ideal = 1; // the game set it itself
        ts[1].allowed = false;
        p.step(now.add_seconds(1.0), &ts, &[], &irq, 0, 0);
        assert_eq!(vec![(10, AllRelease::Forget), (11, AllRelease::Restore)], p.releases);
        assert!(p.writes.is_empty());
        for _ in 0..10 {
            p.step(now.add_seconds(2.0), &ts, &[], &irq, 0, 0);
            assert!(p.writes.is_empty()); // thread 10 is never written again
        }
    }

    #[test]
    fn a_mask_in_force_keeps_the_threads_inside_it() {
        let t = synth::x7950x3d();
        let mask = 0xFFFC; // the cache CCD without core 0
        let mut p = AllPlan::new(AllCpus::new(&t, mask));
        let irq = [0.0; 64];
        let mut ts: Vec<AllThread> = (0..40).map(|i| th(10 + i, if i < 8 { 0.1 } else { 0.001 }, (i % 32) as i32)).collect();
        let mut now = t0();
        for _ in 0..4 {
            run(&mut p, now, &mut ts, &irq);
            now = now.add_seconds(1.0);
        }
        assert!(ts.iter().all(|t| (mask >> t.ideal) & 1 == 1), "{:?}", ts.iter().map(|t| t.ideal).collect::<Vec<_>>());
    }

    #[test]
    fn a_hybrid_processor_uses_p_cores_only() {
        let t = synth::i13900k();
        let mut p = AllPlan::new(AllCpus::new(&t, 0));
        let irq = [0.0; 64];
        let mut ts: Vec<AllThread> = (0..60).map(|i| th(10 + i, if i < 10 { 0.1 } else { 0.001 }, (i % 32) as i32)).collect();
        let mut now = t0();
        for _ in 0..4 {
            run(&mut p, now, &mut ts, &irq);
            now = now.add_seconds(1.0);
        }
        assert!(ts.iter().all(|t| (2..16).contains(&t.ideal)), "{:?}", ts.iter().map(|t| t.ideal).collect::<Vec<_>>());
    }

    #[test]
    fn nothing_allowed_means_nothing_written() {
        // 2 cores: core 0 is never used, core 1 is busy with drivers
        let mut p = AllPlan::new(AllCpus::new(&synth::symmetric(2, true, 2, &[8 << 20], "2c", 0), 0));
        let mut irq = [0.0; 64];
        irq[2] = 0.2;
        let mut ts = vec![th(10, 0.2, 0), th(11, 0.0, 1)];
        for k in 0..5 {
            assert!(run(&mut p, t0().add_seconds(k as f64), &mut ts, &irq).is_empty());
        }
        assert_eq!(0, p.eligible());
    }
}
