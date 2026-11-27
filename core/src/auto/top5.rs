//! The dynamic Top-5 mode (port of `Top5.cs`): the slot logic without any Windows call ([`Top5Plan`]) and the runner
//! that keeps it in force on a game through the engine ([`Top5`]).

use super::all_threads::{AllCounters, AllCpus, AllEvent, AllPlan, AllReason, AllRelease, AllThread};
use crate::base::app_log::{AppLog, Sev};
use crate::base::l::L;
use crate::base::fmt::Culture;
use crate::base::hooks::lock;
use crate::base::model::ThreadSnap;
use crate::base::time::DateTime;
use crate::engine::auto_logic::{AutoLogic, ThreadRole};
use crate::engine::engine::Engine;
use crate::lf;
use crate::native::{Handle, INative};
use crate::topo::masks::Masks;
use crate::topo::model::Topo;
use indexmap::{IndexMap, IndexSet};
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};

/// The numbers of the dynamic Top-5 mode. Not settings: nobody can set them wrong.
pub struct Top5Policy;

impl Top5Policy {
    pub const SLOTS: usize = 5;
    pub const HOLD_TICKS: i32 = 2;
    pub const MAX_MOVES_PER_TICK: i32 = 2;
    pub const FOREIGN_HOLD_TICKS: i32 = 3;
    pub const DWELL_MS: i32 = 2000;
    pub const META_REFRESH_MS: i32 = 30000;
    /// smoothing of the load share (about 3 s of memory)
    pub const ALPHA: f64 = 0.3;
    /// below this a thread is not worth a slot
    pub const MIN_SCORE: f64 = 0.03;
    /// how much better a challenger must be
    pub const MARGIN: f64 = 0.15;
    pub const FOREIGN_BUSY: f64 = 0.20;
    pub const FOREIGN_QUIET: f64 = 0.10;
    // 1.16.1 Test 2: the game's own pain and the interrupt tax are separate signals.
    /// a thread whose p99 ready wait reaches WaitFullMs gets +0.30 (like a 30% share)
    pub const WAIT_WEIGHT: f64 = 0.30;
    pub const WAIT_FULL_MS: f64 = 1.0;
    /// a slot's own p99 wait above this for ForeignHoldTicks: it leaves its processor
    pub const OWN_WAIT_MS: f64 = 0.5;
    /// DPC/ISR share of a processor above this: the slot leaves it (4% = a busy interrupt core)
    pub const IRQ_BUSY: f64 = 0.05;
    pub const BONUS_MAIN: f64 = 0.15;
    pub const BONUS_RENDER: f64 = 0.10;
    pub const BONUS_AUDIO: f64 = 0.08;
}

/// What one slot holds now (for the page).
#[derive(Clone, Debug, PartialEq)]
pub struct Top5Slot {
    pub index: i32,
    pub tid: u32,
    pub lp: u32,
    /// None = the thread's own priority
    pub prio: Option<i32>,
    pub share: f64,
    pub score: f64,
    pub role: ThreadRole,
    pub name: String,
    pub since: DateTime,
}

/// `Top5Plan.Cand`.
#[derive(Clone, Debug, PartialEq)]
pub struct Cand {
    pub tid: u32,
    pub share: f64,
    pub role: ThreadRole,
    /// p99 ready wait of the thread in the last probe window (ms); -1 = not measured.
    pub wait_ms: f64,
}

impl Default for Cand {
    fn default() -> Self {
        Cand { tid: 0, share: 0.0, role: ThreadRole::Main, wait_ms: -1.0 }
    }
}

/// The decisions of the dynamic Top-5 mode, without any Windows call: five slots (a processor and a priority each),
/// re-ranked every tick. A thread from outside takes the weakest slot when it leads by Margin for HoldTicks ticks;
/// neighbours swap slots the same way; a slot leaves a processor that other work keeps busy. A thread is not moved
/// twice within DwellMs and no more than MaxMovesPerTick moves happen per tick, so the threads migrate but do not
/// thrash.
#[derive(Clone, Debug, Default)]
pub struct Top5Plan {
    ema: IndexMap<u32, f64>,
    wait: IndexMap<u32, f64>,
    role: IndexMap<u32, ThreadRole>,
    lead: IndexMap<u32, i32>,
    low: IndexMap<u32, i32>,
    moved: IndexMap<u32, DateTime>,
    since: IndexMap<u32, DateTime>,
    swap_lead: [i32; Top5Policy::SLOTS],
    foreign_hold: [i32; Top5Policy::SLOTS],
    tid: [u32; Top5Policy::SLOTS],
    lp: [u32; Top5Policy::SLOTS],
    spare: Vec<u32>,
    slots: usize,
    moves: i32,
    /// What happened in the last step, in words (for the page).
    pub events: Vec<String>,
}

impl Top5Plan {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn moves(&self) -> i32 {
        self.moves
    }

    /// The processors the slots may use, best first: the first five become the slots, the rest are spares.
    pub fn set_cores(&mut self, lps: &[u32]) {
        self.slots = Top5Policy::SLOTS.min(lps.len());
        for i in 0..Top5Policy::SLOTS {
            self.lp[i] = if i < self.slots { lps[i] } else { 0 };
        }
        self.spare = lps.iter().skip(self.slots).copied().collect();
    }

    pub fn slot_count(&self) -> i32 {
        self.slots as i32
    }

    /// 1.17.1: the processors of every slot, filled or not (kept free for Top-5 by the all-threads layer).
    pub fn slot_mask(&self) -> u64 {
        self.lp.iter().take(self.slots).filter(|lp| **lp < 64).fold(0u64, |m, lp| m | 1u64 << lp)
    }

    fn bonus(r: ThreadRole) -> f64 {
        match r {
            ThreadRole::Main => Top5Policy::BONUS_MAIN,
            ThreadRole::Render => Top5Policy::BONUS_RENDER,
            ThreadRole::Audio => Top5Policy::BONUS_AUDIO,
            _ => 0.0,
        }
    }

    fn role_of(&self, tid: u32) -> ThreadRole {
        self.role.get(&tid).copied().unwrap_or(ThreadRole::Aux)
    }

    fn score(&self, tid: u32) -> f64 {
        match self.ema.get(&tid) {
            Some(e) => {
                e + Self::bonus(self.role_of(tid))
                    + Top5Policy::WAIT_WEIGHT * (self.wait.get(&tid).copied().unwrap_or(0.0) / Top5Policy::WAIT_FULL_MS).min(1.0)
            }
            None => 0.0,
        }
    }

    /// The smoothed p99 ready wait of a thread (ms), 0 when never measured.
    pub fn wait_of(&self, tid: u32) -> f64 {
        self.wait.get(&tid).copied().unwrap_or(0.0)
    }

    fn locked(&self, tid: u32, now: DateTime) -> bool {
        self.moved.get(&tid).map(|at| (now - *at).total_ms() < Top5Policy::DWELL_MS as f64).unwrap_or(false)
    }

    fn put(&mut self, slot: usize, tid: u32, now: DateTime) {
        self.tid[slot] = tid;
        if tid != 0 {
            self.moved.insert(tid, now);
            self.since.insert(tid, now);
        }
    }

    /// One tick: the candidates with their share of the process' load, the foreign load of a processor (0..1).
    pub fn step(&mut self, now: DateTime, cands: &[Cand], foreign: Option<&dyn Fn(u32) -> f64>, irq: Option<&dyn Fn(u32) -> f64>) -> i32 {
        self.events.clear();
        let mut budget = Top5Policy::MAX_MOVES_PER_TICK;
        let mut moves = 0;
        let mut alive: IndexSet<u32> = IndexSet::new();
        for c in cands {
            alive.insert(c.tid);
            self.role.insert(c.tid, c.role);
            let e = match self.ema.get(&c.tid) {
                Some(e) => e + Top5Policy::ALPHA * (c.share - e),
                None => c.share,
            };
            self.ema.insert(c.tid, e);
            if c.wait_ms >= 0.0 {
                let w = match self.wait.get(&c.tid) {
                    Some(w) => w + Top5Policy::ALPHA * (c.wait_ms - w),
                    None => c.wait_ms,
                };
                self.wait.insert(c.tid, w);
            }
        }
        let gone: Vec<u32> = self.ema.keys().copied().filter(|k| !alive.contains(k)).collect();
        for g in gone {
            self.ema.shift_remove(&g);
            self.wait.shift_remove(&g);
            self.role.shift_remove(&g);
            self.lead.shift_remove(&g);
            self.low.shift_remove(&g);
            self.moved.shift_remove(&g);
            self.since.shift_remove(&g);
        }

        // Members that ended, may no longer be touched, or went quiet leave their slot.
        for i in 0..self.slots {
            let t = self.tid[i];
            if t == 0 {
                continue;
            }
            if !alive.contains(&t) {
                self.tid[i] = 0;
                self.events.push(lf!("slot {0}: thread {1} is gone", i + 1, t));
                continue;
            }
            if self.score(t) < Top5Policy::MIN_SCORE / 2.0 {
                let n = self.low.get(&t).copied().unwrap_or(0) + 1;
                self.low.insert(t, n);
                if n >= Top5Policy::HOLD_TICKS {
                    self.tid[i] = 0;
                    self.low.shift_remove(&t);
                    self.events.push(lf!("slot {0}: thread {1} went quiet", i + 1, t));
                }
            } else {
                self.low.shift_remove(&t);
            }
        }

        let mut members: IndexSet<u32> = self.tid.iter().copied().filter(|t| *t != 0).collect();
        let mut outside: Vec<u32> = alive.iter().copied().filter(|t| !members.contains(t) && self.score(*t) >= Top5Policy::MIN_SCORE).collect();
        // OrderByDescending(Score).ThenBy(t)
        outside.sort_by(|a, b| self.score(*b).total_cmp(&self.score(*a)).then(a.cmp(b)));

        // Free slots are filled at once, the best thread into the best free slot.
        let mut i = 0;
        while i < self.slots && budget > 0 && !outside.is_empty() {
            if self.tid[i] != 0 {
                i += 1;
                continue;
            }
            let t = outside.remove(0);
            self.put(i, t, now);
            members.insert(t);
            self.lead.shift_remove(&t);
            budget -= 1;
            moves += 1;
            self.events.push(lf!("slot {0}: thread {1} enters", i + 1, t));
            i += 1;
        }

        // A challenger from outside against the weakest member.
        let first_out = outside.first().copied();
        let drop: Vec<u32> = self.lead.keys().copied().filter(|k| first_out.is_none() || Some(*k) != first_out).collect();
        for k in drop {
            self.lead.shift_remove(&k);
        }
        if let Some(c) = first_out {
            if self.tid.iter().take(self.slots).all(|t| *t != 0) {
                // OrderBy(Score(_tid[i])).ThenByDescending(i).First()
                let mut weakest = 0usize;
                let mut ws = self.score(self.tid[0]);
                for i in 1..self.slots {
                    let si = self.score(self.tid[i]);
                    if si <= ws {
                        weakest = i;
                        ws = si;
                    }
                }
                let w = self.tid[weakest];
                if self.score(c) > self.score(w) * (1.0 + Top5Policy::MARGIN) {
                    let n = self.lead.get(&c).copied().unwrap_or(0) + 1;
                    self.lead.insert(c, n);
                    if n >= Top5Policy::HOLD_TICKS && budget > 0 && !self.locked(w, now) {
                        self.put(weakest, c, now);
                        self.lead.shift_remove(&c);
                        self.since.shift_remove(&w);
                        budget -= 1;
                        moves += 1;
                        self.events.push(lf!("slot {0}: thread {1} replaces {2}", weakest + 1, c, w));
                    }
                } else {
                    self.lead.shift_remove(&c);
                }
            }
        }

        // Order inside the five: a stronger thread moves up one slot at a time.
        let mut i = 0;
        while i + 1 < self.slots {
            let (a, b) = (self.tid[i], self.tid[i + 1]);
            if b == 0 {
                self.swap_lead[i] = 0;
                i += 1;
                continue;
            }
            let better = a == 0 || self.score(b) > self.score(a) * (1.0 + Top5Policy::MARGIN);
            self.swap_lead[i] = if better { self.swap_lead[i] + 1 } else { 0 };
            if !better
                || (a != 0 && self.swap_lead[i] < Top5Policy::HOLD_TICKS)
                || budget <= 0
                || self.locked(b, now)
                || (a != 0 && self.locked(a, now))
            {
                i += 1;
                continue;
            }
            self.put(i, b, now);
            self.put(i + 1, a, now);
            self.swap_lead[i] = 0;
            budget -= 1;
            moves += 1;
            self.events.push(if a == 0 {
                lf!("slot {0}: thread {1} moves up", i + 1, b)
            } else {
                lf!("slots {0} and {1}: threads {2} and {3} swap", i + 1, i + 2, b, a)
            });
            i += 1;
        }

        // A slot leaves its processor when (1.16.1 Test 2, three separate reasons): other programs keep it busy, drivers
        // take more than IrqBusy of it in DPC/ISR, or the slot's own thread keeps waiting for it (p99 > OwnWaitMs).
        // It goes to the quietest spare processor (other work plus interrupt time).
        let irq_of = |lp: u32| irq.map(|f| f(lp)).unwrap_or(0.0);
        let foreign_of = |lp: u32| foreign.map(|f| f(lp)).unwrap_or(0.0);
        let mut i = 0;
        while i < self.slots && (foreign.is_some() || irq.is_some() || !self.wait.is_empty()) {
            if self.tid[i] == 0 {
                self.foreign_hold[i] = 0;
                i += 1;
                continue;
            }
            let (f, q, w) = (foreign_of(self.lp[i]), irq_of(self.lp[i]), self.wait.get(&self.tid[i]).copied().unwrap_or(0.0));
            let why = if f > Top5Policy::FOREIGN_BUSY {
                Some("other")
            } else if q > Top5Policy::IRQ_BUSY {
                Some("irq")
            } else if w > Top5Policy::OWN_WAIT_MS {
                Some("wait")
            } else {
                None
            };
            self.foreign_hold[i] = if why.is_some() { self.foreign_hold[i] + 1 } else { 0 };
            if self.foreign_hold[i] < Top5Policy::FOREIGN_HOLD_TICKS || budget <= 0 || self.locked(self.tid[i], now) {
                i += 1;
                continue;
            }
            let mut quiet: Vec<(u32, f64)> = self
                .spare
                .iter()
                .map(|lp| (*lp, foreign_of(*lp) + irq_of(*lp)))
                .filter(|x| x.1 < Top5Policy::FOREIGN_QUIET && irq_of(x.0) <= Top5Policy::IRQ_BUSY)
                .collect();
            quiet.sort_by(|a, b| a.1.total_cmp(&b.1).then(a.0.cmp(&b.0)));
            if quiet.is_empty() {
                i += 1;
                continue;
            }
            let old = self.lp[i];
            let to = quiet[0].0;
            if let Some(pos) = self.spare.iter().position(|x| *x == to) {
                self.spare.remove(pos);
            }
            self.spare.push(old);
            self.lp[i] = to;
            self.moved.insert(self.tid[i], now);
            self.foreign_hold[i] = 0;
            budget -= 1;
            moves += 1;
            self.events.push(match why {
                Some("irq") => lf!("slot {0}: drivers take processor {1} ({2:0}% in DPC/ISR), moved to {3}", i + 1, old, q * 100.0, to),
                Some("wait") => lf!("slot {0}: thread {1} kept waiting for processor {2} ({3:0.00} ms), moved to {4}", i + 1, self.tid[i], old, w, to),
                _ => lf!("slot {0}: processor {1} is busy with other work, moved to {2}", i + 1, old, to),
            });
            i += 1;
        }
        self.moves += moves;
        moves
    }

    /// Priority of a slot: the two leading slots +1, the audio thread +2, the others keep their own.
    fn prio_of(&self, slot: usize) -> Option<i32> {
        if self.role_of(self.tid[slot]) == ThreadRole::Audio {
            Some(2)
        } else if slot < 2 {
            Some(1)
        } else {
            None
        }
    }

    /// What should be in force now: slot, thread, processor, priority.
    pub fn desired(&self) -> Vec<Top5Slot> {
        let mut list = Vec::new();
        for i in 0..self.slots {
            if self.tid[i] == 0 {
                continue;
            }
            let t = self.tid[i];
            list.push(Top5Slot {
                index: i as i32,
                tid: t,
                lp: self.lp[i],
                prio: self.prio_of(i),
                share: self.ema.get(&t).copied().unwrap_or(0.0),
                score: self.score(t),
                role: self.role_of(t),
                name: String::new(),
                since: self.since.get(&t).copied().unwrap_or(DateTime::MIN),
            });
        }
        list
    }
}

// ---- the runner --------------------------------------------------------------------------------------------------

struct H {
    handle: Handle,
    born: i64,
    cycles: u64,
    cpu: u64,
    first: bool,
    /// processors busy since the last tick (1.0 = one logical processor)
    load: f64,
}

struct Meta {
    born: i64,
    role: ThreadRole,
    name: String,
}

struct St {
    h: IndexMap<u32, H>,
    meta: IndexMap<u32, Meta>,
    applied: IndexMap<u32, (u32, Option<i32>)>,
    log: VecDeque<String>,
    /// every thread the last snapshot saw, candidate or not
    known: IndexSet<u32>,
    plan: Top5Plan,
    view: Vec<Top5Slot>,
    pid: u32,
    reverts: i64,
    prev_at: DateTime,
    idle: Option<Vec<i64>>,
    total: Option<Vec<i64>>,
    failures: i32,
    running: bool,
    exe: String,
    // 1.17.1: ideal cores for all game threads (auto::all_threads), on the same tick.
    all: Option<AllPlan>,
    all_topo: Option<Topo>,
    all_buf: Vec<AllThread>,
    all_items: Vec<(u32, Handle, i64, u32)>,
    all_ok: Vec<bool>,
    busy: Vec<f64>,
    all_logged: bool,
}

/// What the all-threads layer shows on the Game threads page.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AllView {
    /// the setting
    pub on: bool,
    /// the layer runs on a game now
    pub running: bool,
    /// threads the layer gave an ideal processor (written)
    pub placed: i32,
    /// threads whose ideal processor was already right (kept without a write)
    pub kept: i32,
    /// processors in use by those threads
    pub cpus: u64,
    /// driver-busy processors kept free
    pub driver: u64,
    pub counters: AllCounters,
    /// newest first
    pub events: Vec<AllEvent>,
}

pub type WaitOfFn = dyn Fn(u32) -> f64 + Send + Sync;
pub type IrqSharesFn = dyn Fn() -> Option<Vec<f64>> + Send + Sync;
/// 1.17.1: fills the DPC/ISR share of every processor (zeros when the live trace is off) and returns the
/// graphics-driver cores (EtwIrq::shares, EtwIrq::gpu_cores).
pub type DriverFn = dyn Fn(&Topo, &mut [f64; 64]) -> u64 + Send + Sync;

/// Runs the plan on a game: keeps one handle per thread open (a thread is opened once, when it appears), reads the
/// cycle counters every tick, and writes only the differences through the engine (baseline, birth check, revert,
/// crash-safe session). Threads of denylisted modules and threads whose module is not known yet are never
/// candidates.
pub struct Top5 {
    e: Arc<Engine>,
    n: Arc<dyn INative>,
    st: Mutex<St>,
    wait_of: RwLock<Option<Arc<WaitOfFn>>>,
    irq_shares: RwLock<Option<Arc<IrqSharesFn>>>,
    drivers: RwLock<Option<Arc<DriverFn>>>,
    all_on: AtomicBool,
}

impl Top5 {
    pub fn new(engine: Arc<Engine>) -> Self {
        let n = engine.native();
        Top5 {
            e: engine,
            n,
            st: Mutex::new(St {
                h: IndexMap::new(),
                meta: IndexMap::new(),
                applied: IndexMap::new(),
                log: VecDeque::new(),
                known: IndexSet::new(),
                plan: Top5Plan::new(),
                view: Vec::new(),
                pid: 0,
                reverts: 0,
                prev_at: DateTime::MIN,
                idle: None,
                total: None,
                failures: 0,
                running: false,
                exe: String::new(),
                all: None,
                all_topo: None,
                all_buf: Vec::with_capacity(1024),
                all_items: Vec::with_capacity(64),
                all_ok: Vec::with_capacity(64),
                busy: Vec::with_capacity(64),
                all_logged: false,
            }),
            wait_of: RwLock::new(None),
            irq_shares: RwLock::new(None),
            drivers: RwLock::new(None),
            all_on: AtomicBool::new(true),
        }
    }

    /// 1.16.1 Test 2: p99 ready wait (ms) of a game thread from the wait probe; None or -1 = unknown.
    pub fn set_wait_of(&self, f: Option<Arc<WaitOfFn>>) {
        *self.wait_of.write().unwrap_or_else(|e| e.into_inner()) = f;
    }
    /// 1.16.1 Test 2: DPC/ISR share (0..1) per processor from the live interrupt trace; None = unknown.
    pub fn set_irq_shares(&self, f: Option<Arc<IrqSharesFn>>) {
        *self.irq_shares.write().unwrap_or_else(|e| e.into_inner()) = f;
    }

    /// 1.17.1: the driver-busy processors for the all-threads layer (the hub hands EtwIrq over).
    pub fn set_drivers(&self, f: Option<Arc<DriverFn>>) {
        *self.drivers.write().unwrap_or_else(|e| e.into_inner()) = f;
    }

    /// 1.17.1: setting "AllThreadsIdeal" (on by default). Off: the next tick puts the layer's values back.
    pub fn set_all_threads(&self, on: bool) {
        self.all_on.store(on, Ordering::SeqCst);
    }
    pub fn all_threads(&self) -> bool {
        self.all_on.load(Ordering::SeqCst)
    }

    /// The all-threads layer for the page.
    pub fn all_view(&self) -> AllView {
        let s = lock(&self.st);
        let mut v = AllView { on: self.all_threads(), ..Default::default() };
        if let Some(p) = s.all.as_ref() {
            let (w, k, cpus) = p.tracked();
            v.running = s.running && v.on;
            v.placed = w;
            v.kept = k;
            v.cpus = cpus;
            v.driver = p.driver_cpus();
            v.counters = p.counters();
            v.events = p.events();
        }
        v
    }

    pub fn running(&self) -> bool {
        lock(&self.st).running
    }
    pub fn exe(&self) -> String {
        lock(&self.st).exe.clone()
    }
    pub fn moves(&self) -> i32 {
        lock(&self.st).plan.moves()
    }
    pub fn threads(&self) -> i32 {
        lock(&self.st).h.len() as i32
    }
    /// True when threads appeared that are not classified yet.
    pub fn needs_meta(&self) -> bool {
        let s = lock(&self.st);
        s.h.keys().any(|t| !s.known.contains(t))
    }

    pub fn view(&self) -> Vec<Top5Slot> {
        lock(&self.st).view.clone()
    }

    pub fn log(&self) -> Vec<String> {
        lock(&self.st).log.iter().cloned().collect()
    }

    /// Starts on a game: the slots take the best processors of the game's mask (never processor 0).
    pub fn start(&self, pid: u32, exe: &str, t: Option<&Topo>, mask: u64) -> bool {
        let mut s = lock(&self.st);
        self.stop_locked(&mut s, false);
        let Some(t) = t else { return false };
        if self.e.gate(pid).is_some() {
            return false;
        }
        let cores: Vec<u32> = Masks::ideal_candidates(t, true, mask).iter().map(|c| c.lp as u32).collect();
        s.plan = Top5Plan::new();
        s.plan.set_cores(&cores);
        if s.plan.slot_count() == 0 {
            return false;
        }
        s.pid = pid;
        s.exe = exe.to_string();
        s.reverts = self.e.reverts();
        s.prev_at = DateTime::MIN;
        s.idle = None;
        s.total = None;
        s.failures = 0;
        s.log.clear();
        s.running = true;
        s.all = Some(AllPlan::new(AllCpus::new(t, mask)));
        s.all_topo = Some(t.clone());
        s.all_logged = false;
        true
    }

    /// Names and roles of the threads from a full snapshot (taken at the start and at most twice a minute).
    pub fn set_meta(&self, snap: Option<&ThreadSnap>) {
        let Some(snap) = snap else { return };
        let mut s = lock(&self.st);
        s.meta.clear();
        s.known.clear();
        for r in &snap.rows {
            s.known.insert(r.tid);
            // The thread ID was reused: the old handle is dropped, the new thread is opened on the next tick.
            let stale = s.h.get(&r.tid).map(|h| (h.born, h.handle));
            if let Some((born, handle)) = stale {
                if born != r.born {
                    self.n.close(handle);
                    s.h.shift_remove(&r.tid);
                    s.applied.shift_remove(&r.tid);
                }
            }
            if Engine::is_denied_module(r) || (r.sym.is_empty() && r.desc.is_empty()) {
                continue; // never a candidate
            }
            let role = AutoLogic::classify(r, snap);
            if role == ThreadRole::Skip {
                continue;
            }
            let name = if !r.desc.is_empty() { r.desc.clone() } else { Engine::module_of(&r.sym).to_string() };
            s.meta.insert(r.tid, Meta { born: r.born, role, name });
        }
    }

    /// Every second: measure, re-rank, move what has to move. Returns the number of moves.
    pub fn tick(&self, now: DateTime, tids: Option<&[u32]>) -> i32 {
        let mut s = lock(&self.st);
        let Some(tids) = tids else { return 0 };
        if !s.running {
            return 0;
        }
        let reverts = self.e.reverts();
        if reverts != s.reverts {
            s.applied.clear(); // the engine put everything back (a revert or the frame-time check)
            if let Some(p) = s.all.as_mut() {
                p.reset();
            }
            s.reverts = reverts;
        }
        // Handles: a thread is opened once; ended threads are closed.
        let live: IndexSet<u32> = tids.iter().copied().collect();
        let gone: Vec<u32> = s.h.keys().copied().filter(|k| !live.contains(k)).collect();
        for g in gone {
            let h = s.h[&g].handle;
            self.n.close(h);
            s.h.shift_remove(&g);
            s.applied.shift_remove(&g);
        }
        let pid = s.pid;
        for &tid in tids {
            if s.h.contains_key(&tid) {
                continue;
            }
            let th = self.n.open_thread_of(pid, tid);
            if th.is_null() {
                continue;
            }
            let (mut born, mut ku) = (0i64, 0u64);
            self.n.thread_times(th, &mut born, &mut ku);
            s.h.insert(tid, H { handle: th, born, cycles: 0, cpu: 0, first: true, load: 0.0 });
        }
        let dt = if s.prev_at == DateTime::MIN { 0.0 } else { (now - s.prev_at).total_seconds() };
        s.prev_at = now;

        // Load of every thread since the last tick.
        let mut delta: IndexMap<u32, u64> = IndexMap::new();
        let mut cpu_share: HashMap<u32, f64> = HashMap::new();
        let mut total: u64 = 0;
        for (tid, h) in s.h.iter_mut() {
            let mut cycles = 0u64;
            if !self.n.thread_cycles(h.handle, &mut cycles) {
                continue;
            }
            let (mut c, mut cpu) = (0i64, 0u64);
            self.n.thread_times(h.handle, &mut c, &mut cpu);
            h.load = if !h.first && dt > 0.0 && cpu >= h.cpu { (cpu - h.cpu) as f64 / 1e7 / dt } else { 0.0 };
            if !h.first && cycles >= h.cycles {
                delta.insert(*tid, cycles - h.cycles);
                total = total.wrapping_add(cycles - h.cycles);
                if dt > 0.0 && cpu >= h.cpu {
                    cpu_share.insert(*tid, (cpu - h.cpu) as f64 / 1e7 / dt);
                }
            }
            h.cycles = cycles;
            h.cpu = cpu;
            h.first = false;
        }
        if dt < 0.2 || total == 0 {
            return 0;
        }

        let wait_of = self.wait_of.read().unwrap_or_else(|e| e.into_inner()).clone();
        let mut cands: Vec<Cand> = Vec::new();
        for (tid, d) in &delta {
            if let Some(m) = s.meta.get(tid) {
                if m.born == s.h[tid].born {
                    cands.push(Cand {
                        tid: *tid,
                        share: *d as f64 / total as f64,
                        role: m.role,
                        wait_ms: wait_of.as_ref().map(|f| f(*tid)).unwrap_or(-1.0),
                    });
                }
            }
        }

        // Foreign load of a processor: everything on it except the thread the slot holds there.
        let (mut idle, mut tot) = (Vec::new(), Vec::new());
        let ok = self.n.cpu_times(&mut idle, &mut tot);
        let mut foreign: Option<Box<dyn Fn(u32) -> f64>> = None;
        let mut irq_of: Option<Box<dyn Fn(u32) -> f64>> = None;
        if ok && s.idle.as_ref().map(|p| p.len() == idle.len()).unwrap_or(false) {
            let (pi, pt) = (s.idle.as_ref().unwrap(), s.total.as_ref().unwrap());
            let mut busy = vec![0.0; idle.len()];
            for i in 0..idle.len() {
                let (d_tot, d_idle) = (tot[i] - pt[i], idle[i] - pi[i]);
                busy[i] = if d_tot > 0 { (1.0 - d_idle as f64 / d_tot as f64).clamp(0.0, 1.0) } else { 0.0 };
            }
            s.busy.clear();
            s.busy.extend_from_slice(&busy);
            let mut own: HashMap<u32, f64> = HashMap::new();
            for (tid, a) in &s.applied {
                *own.entry(a.0).or_insert(0.0) += cpu_share.get(tid).copied().unwrap_or(0.0);
            }
            // 1.16.1 Test 2: DPC/ISR time is not idle time, so "busy" holds the interrupt tax too; it is taken out here
            // and handed to the plan as its own signal (a driver is not fixed the way a busy program is).
            let shares = self.irq_shares.read().unwrap_or_else(|e| e.into_inner()).clone();
            let irq: Option<Vec<f64>> = shares.and_then(|f| f());
            let irq = Arc::new(irq);
            let i1 = irq.clone();
            let irq_fn = move |lp: u32| -> f64 {
                match i1.as_ref() {
                    Some(v) if (lp as usize) < v.len() => v[lp as usize],
                    _ => 0.0,
                }
            };
            let i2 = irq_fn.clone();
            irq_of = Some(Box::new(irq_fn));
            foreign = Some(Box::new(move |lp: u32| {
                if (lp as usize) < busy.len() {
                    (busy[lp as usize] - own.get(&lp).copied().unwrap_or(0.0) - i2(lp)).max(0.0)
                } else {
                    0.0
                }
            }));
        } else {
            s.busy.clear();
        }
        s.idle = Some(idle);
        s.total = Some(tot);

        let moves = s.plan.step(now, &cands, foreign.as_deref(), irq_of.as_deref());
        let stamp = now.to_local().format("HH:mm:ss", &Culture::invariant());
        let events = s.plan.events.clone();
        for ev in events {
            s.log.push_front(format!("{}  {}", stamp, ev));
            while s.log.len() > 12 {
                s.log.pop_back();
            }
        }
        self.reconcile(&mut s);
        self.all_tick(&mut s, now);
        moves
    }

    /// 1.17.1: the other game threads get their ideal processors (auto::all_threads), after Top-5 placed its five.
    fn all_tick(&self, s: &mut St, now: DateTime) {
        if !self.all_threads() {
            if s.all.as_ref().map(|p| p.owned().next().is_some()).unwrap_or(false) {
                let n = self.all_release(s);
                AppLog::add(lf!("All game threads: off, {0} ideal processors put back", n));
            }
            s.all_logged = false;
            return;
        }
        let Some(topo) = s.all_topo.as_ref() else { return };
        let Some(plan) = s.all.as_mut() else { return };
        s.all_buf.clear();
        for (tid, h) in s.h.iter() {
            let slot = s.applied.get(tid).map(|a| a.0 as i32);
            let allowed = s.meta.get(tid).map(|m| m.born == h.born).unwrap_or(false);
            let ideal = match slot {
                Some(lp) => lp,
                None if allowed => {
                    let mut v = 0u32;
                    if self.n.get_thread_ideal(h.handle, &mut v) && v < 64 {
                        v as i32
                    } else {
                        -1
                    }
                }
                None => -1,
            };
            s.all_buf.push(AllThread { tid: *tid, born: h.born, load: h.load, ideal, top5: slot.is_some(), allowed });
        }
        let mut irq = [0.0f64; 64];
        let drivers = self.drivers.read().unwrap_or_else(|e| e.into_inner()).clone();
        let gpu = drivers.map(|f| f(topo, &mut irq)).unwrap_or(0);
        plan.step(now, &s.all_buf, &s.busy, &irq, gpu, s.plan.slot_mask());
        for k in 0..plan.releases.len() {
            let (tid, how) = plan.releases[k];
            match how {
                AllRelease::Restore => {
                    if let Some(h) = s.h.get(&tid) {
                        self.e.release_ideal(s.pid, tid, h.handle);
                    }
                }
                AllRelease::Forget => self.e.forget_ideal(s.pid, tid),
            }
        }
        let first = plan.writes.iter().any(|w| w.reason == AllReason::New);
        if !plan.writes.is_empty() {
            if self.e.gate(s.pid).is_some() {
                plan.cancel(); // nothing is written while the guard (anti-cheat, denylist) refuses the process
                return;
            }
            s.all_items.clear();
            for w in &plan.writes {
                let h = &s.h[&w.tid];
                s.all_items.push((w.tid, h.handle, h.born, w.to));
            }
            self.e.place_ideals(s.pid, &s.all_items, &mut s.all_ok);
            for k in 0..plan.writes.len() {
                plan.confirm(now, k, s.all_ok.get(k).copied().unwrap_or(false));
            }
        }
        // One line once the first placement is done and Top-5 holds all its slots.
        if !s.all_logged && !first && s.view.len() as i32 >= s.plan.slot_count() {
            let (w, k, cpus) = plan.tracked();
            if w + k > 0 {
                s.all_logged = true;
                let d = plan.driver_cpus();
                let drivers = if d == 0 { L::t("none").to_string() } else { Masks::ranges(d) };
                AppLog::add(lf!("All game threads: {0} threads on cpus {1} (driver cpus {2} avoided)", w + k, Masks::ranges(cpus), drivers));
            }
        }
    }

    /// Puts back every ideal processor the all-threads layer owns (only where the value is still ours); returns how
    /// many were written.
    fn all_release(&self, s: &mut St) -> i32 {
        let Some(plan) = s.all.as_mut() else { return 0 };
        let owned: Vec<u32> = plan.owned().map(|o| o.0).collect();
        let mut n = 0;
        for tid in owned {
            if let Some(h) = s.h.get(&tid) {
                if self.e.release_ideal(s.pid, tid, h.handle) {
                    n += 1;
                }
            } else {
                self.e.forget_ideal(s.pid, tid);
            }
        }
        plan.reset();
        n
    }

    /// Makes Windows match the plan: only the differences are written.
    fn reconcile(&self, s: &mut St) {
        let mut want = s.plan.desired();
        let keep: IndexSet<u32> = want.iter().map(|w| w.tid).collect();
        let drop: Vec<u32> = s.applied.keys().copied().filter(|k| !keep.contains(k)).collect();
        for tid in drop {
            if let Some(h) = s.h.get(&tid) {
                self.e.release_thread(s.pid, tid, h.handle); // back to its own priority and processor
            }
            s.applied.shift_remove(&tid);
        }
        for w in want.iter_mut() {
            if let Some(m) = s.meta.get(&w.tid) {
                w.name = m.name.clone();
            }
            let Some(h) = s.h.get(&w.tid) else { continue };
            let (handle, born) = (h.handle, h.born);
            if let Some(a) = s.applied.get(&w.tid) {
                if a.0 == w.lp && a.1 == w.prio {
                    continue;
                }
            }
            if self.e.place_thread(s.pid, w.tid, handle, born, w.prio, w.lp) {
                s.applied.insert(w.tid, (w.lp, w.prio));
            } else {
                s.failures += 1;
                if s.failures == 1 {
                    AppLog::add_sev(lf!("Dynamic Top-5: Windows refused a change for thread {0} (error {1})", w.tid, self.n.last_error()), Sev::Warn);
                }
            }
        }
        s.view = want;
    }

    /// Stops: with release = true the threads get their own values back here; otherwise the engine's revert does it.
    pub fn stop(&self, release: bool) {
        let mut s = lock(&self.st);
        self.stop_locked(&mut s, release);
    }

    fn stop_locked(&self, s: &mut St, release: bool) {
        if release {
            self.all_release(s);
            let tids: Vec<u32> = s.applied.keys().copied().collect();
            for tid in tids {
                if let Some(h) = s.h.get(&tid) {
                    self.e.release_thread(s.pid, tid, h.handle);
                }
            }
        }
        for h in s.h.values() {
            self.n.close(h.handle);
        }
        s.h.clear();
        s.applied.clear();
        s.meta.clear();
        s.known.clear();
        s.view = Vec::new();
        s.running = false;
        s.all = None;
        s.all_topo = None;
        s.all_logged = false;
    }

    /// `Dispose()`.
    pub fn dispose(&self) {
        self.stop(false);
    }
}

#[cfg(test)]
mod tests {
    //! Top5Tests.cs, the slot logic (6 tests); the tests with Auto are in `auto::tests`.
    use super::*;
    use crate::base::time::Kind;

    fn t0() -> DateTime {
        DateTime::from_parts(2026, 1, 1, 12, 0, 0, Kind::Utc).unwrap()
    }

    fn c(tid: u32, share: f64, role: ThreadRole) -> Cand {
        Cand { tid, share, role, wait_ms: -1.0 }
    }
    fn cw(tid: u32, share: f64) -> Cand {
        c(tid, share, ThreadRole::Worker)
    }
    fn w(tid: u32, share: f64, wait_ms: f64) -> Cand {
        Cand { tid, share, role: ThreadRole::Worker, wait_ms }
    }
    fn tids(p: &Top5Plan) -> Vec<u32> {
        p.desired().iter().map(|s| s.tid).collect()
    }
    fn slot_of(p: &Top5Plan, tid: u32) -> Top5Slot {
        p.desired().into_iter().find(|s| s.tid == tid).unwrap()
    }

    #[test]
    fn slots_fill_by_score_two_moves_per_tick_and_roles_get_their_priority() {
        let mut plan = Top5Plan::new();
        plan.set_cores(&[2, 4, 6, 8, 10, 12, 14]);
        let cands = vec![
            c(1, 0.40, ThreadRole::Main),
            c(2, 0.20, ThreadRole::Render),
            cw(3, 0.12),
            cw(4, 0.10),
            c(5, 0.02, ThreadRole::Audio),
            cw(6, 0.09),
            cw(7, 0.01),
        ];
        let mut t = t0();
        assert_eq!(2, plan.step(t, &cands, None, None)); // never more than two moves in a tick
        assert_eq!(vec![1, 2], tids(&plan));
        t = t.add_seconds(1.0);
        plan.step(t, &cands, None, None);
        t = t.add_seconds(1.0);
        plan.step(t, &cands, None, None);
        let d = plan.desired();
        assert_eq!(vec![1, 2, 3, 4, 5], d.iter().map(|s| s.tid).collect::<Vec<_>>()); // audio: 2% of the load, but its role earns the slot
        assert_eq!(vec![2, 4, 6, 8, 10], d.iter().map(|s| s.lp).collect::<Vec<_>>()); // one processor each, never processor 0
        assert_eq!(vec![Some(1), Some(1), None, None, Some(2)], d.iter().map(|s| s.prio).collect::<Vec<_>>());
        assert!(!d.iter().any(|s| s.tid == 7)); // too quiet for a slot
        let moves = plan.moves();
        for _ in 0..20 {
            t = t.add_seconds(1.0);
            plan.step(t, &cands, None, None);
        }
        assert_eq!(moves, plan.moves()); // a steady load moves nothing
    }

    #[test]
    fn a_challenger_needs_a_lead_for_two_ticks_and_threads_are_not_moved_twice_in_a_row() {
        let mut plan = Top5Plan::new();
        plan.set_cores(&[2, 4, 6, 8, 10]);
        let steady = vec![cw(1, 0.40), cw(2, 0.20), cw(3, 0.12), cw(4, 0.10), cw(5, 0.08), cw(6, 0.07)];
        let mut t = t0();
        for _ in 0..6 {
            t = t.add_seconds(1.0);
            plan.step(t, &steady, None, None);
        }
        assert_eq!(vec![1, 2, 3, 4, 5], tids(&plan));

        // Thread 6 is only a little better than the weakest: not enough.
        let close = vec![cw(1, 0.40), cw(2, 0.20), cw(3, 0.12), cw(4, 0.10), cw(5, 0.08), cw(6, 0.085)];
        for _ in 0..10 {
            t = t.add_seconds(1.0);
            plan.step(t, &close, None, None);
        }
        assert!(tids(&plan).contains(&5));

        // One tick of lead does not move anything; the second one does, into the weakest slot.
        let hot = vec![cw(1, 0.40), cw(2, 0.20), cw(3, 0.12), cw(4, 0.10), cw(5, 0.08), cw(6, 0.60)];
        let before = plan.moves();
        t = t.add_seconds(1.0);
        plan.step(t, &hot, None, None);
        assert_eq!(before, plan.moves());
        assert!(tids(&plan).contains(&5));
        t = t.add_seconds(1.0);
        plan.step(t, &hot, None, None);
        assert!(!tids(&plan).contains(&5));
        let six = slot_of(&plan, 6);
        assert_eq!(4, six.index);
        assert_eq!(10, six.lp);

        // It climbs one slot at a time, never twice within the dwell time, and ends on top.
        let mut seen = vec![six.index];
        let mut last_move = t;
        for _ in 0..30 {
            t = t.add_ms(500.0);
            plan.step(t, &hot, None, None);
            let idx = slot_of(&plan, 6).index;
            if idx != *seen.last().unwrap() {
                assert_eq!(seen.last().unwrap() - 1, idx);
                assert!((t - last_move).total_ms() >= 2000.0);
                last_move = t;
                seen.push(idx);
            }
        }
        assert_eq!(0, *seen.last().unwrap());
        assert_eq!(Some(1), plan.desired()[0].prio);
    }

    #[test]
    fn a_thread_that_waits_for_a_core_earns_a_slot_over_a_busier_one_that_does_not() {
        let mut plan = Top5Plan::new();
        plan.set_cores(&[2, 4, 6, 8, 10, 12, 14]);
        // thread 6 uses only 3% but waits 1.2 ms (p99); thread 5 uses 8% and never waits
        let cands = vec![w(1, 0.40, 0.0), w(2, 0.20, 0.0), w(3, 0.12, 0.0), w(4, 0.10, 0.0), w(5, 0.08, 0.0), w(6, 0.03, 1.2)];
        let mut t = t0();
        for _ in 0..12 {
            t = t.add_seconds(1.0);
            plan.step(t, &cands, None, None);
        }
        assert!(tids(&plan).contains(&6));
        assert!(!tids(&plan).contains(&5));
        assert!(plan.wait_of(6) > 1.0);
        // without wait data the old ranking by load stays
        let mut no_wait = Top5Plan::new();
        no_wait.set_cores(&[2, 4, 6, 8, 10, 12, 14]);
        let load = vec![cw(1, 0.40), cw(2, 0.20), cw(3, 0.12), cw(4, 0.10), cw(5, 0.08), cw(6, 0.03)];
        for _ in 0..12 {
            t = t.add_seconds(1.0);
            no_wait.step(t, &load, None, None);
        }
        assert!(tids(&no_wait).contains(&5));
    }

    #[test]
    fn a_slot_leaves_a_processor_drivers_keep_busy_or_where_its_thread_keeps_waiting() {
        let mut plan = Top5Plan::new();
        plan.set_cores(&[2, 4, 6, 8, 10, 12, 14]);
        let mut t = t0();
        let irq: std::cell::RefCell<HashMap<u32, f64>> = Default::default();
        let irq_f = |lp: u32| irq.borrow().get(&lp).copied().unwrap_or(0.0);
        let none = |_lp: u32| 0.0;
        let calm = vec![w(1, 0.40, 0.0), w(2, 0.20, 0.0), w(3, 0.12, 0.0), w(4, 0.10, 0.0), w(5, 0.08, 0.0)];
        for _ in 0..6 {
            t = t.add_seconds(1.0);
            plan.step(t, &calm, Some(&none), Some(&irq_f));
        }
        let lp1 = slot_of(&plan, 1).lp;
        irq.borrow_mut().insert(lp1, 0.08); // a driver takes 8% of that processor: not "foreign" programs
        for _ in 0..4 {
            t = t.add_seconds(1.0);
            plan.step(t, &calm, Some(&none), Some(&irq_f));
        }
        let moved = slot_of(&plan, 1).lp;
        assert_ne!(lp1, moved);
        // a slot whose own thread keeps waiting moves too, though nothing else looks busy
        let lp3 = slot_of(&plan, 3).lp;
        let waiting = vec![w(1, 0.40, 0.0), w(2, 0.20, 0.0), w(3, 0.12, 0.9), w(4, 0.10, 0.0), w(5, 0.08, 0.0)];
        for _ in 0..8 {
            t = t.add_seconds(1.0);
            plan.step(t, &waiting, Some(&none), Some(&irq_f));
        }
        assert_ne!(lp3, slot_of(&plan, 3).lp);
    }

    #[test]
    fn a_slot_leaves_a_processor_that_other_work_keeps_busy() {
        let mut plan = Top5Plan::new();
        plan.set_cores(&[2, 4, 6, 8, 10, 12, 14]);
        let cands = vec![cw(1, 0.40), cw(2, 0.20), cw(3, 0.12), cw(4, 0.10), cw(5, 0.08)];
        let mut t = t0();
        let load: std::cell::RefCell<HashMap<u32, f64>> = Default::default();
        let foreign = |lp: u32| load.borrow().get(&lp).copied().unwrap_or(0.0);
        for _ in 0..6 {
            t = t.add_seconds(1.0);
            plan.step(t, &cands, Some(&foreign), None);
        }
        load.borrow_mut().insert(4, 0.5); // somebody else works on processor 4
        load.borrow_mut().insert(12, 0.3); // one spare is busy too, the other is quiet
        t = t.add_seconds(1.0);
        plan.step(t, &cands, Some(&foreign), None);
        t = t.add_seconds(1.0);
        plan.step(t, &cands, Some(&foreign), None);
        assert_eq!(4, plan.desired()[1].lp); // two seconds are not enough
        t = t.add_seconds(1.0);
        plan.step(t, &cands, Some(&foreign), None);
        assert_eq!(14, plan.desired()[1].lp);
        assert!(plan.events.iter().any(|e| e.contains("busy with other work")));
        load.borrow_mut().insert(14, 0.5); // now 14 is busy; 4 is still busy, 12 too: nowhere better to go
        for _ in 0..6 {
            t = t.add_seconds(1.0);
            plan.step(t, &cands, Some(&foreign), None);
        }
        assert_eq!(14, plan.desired()[1].lp);
    }

    #[test]
    fn quiet_and_ended_threads_free_their_slot() {
        let mut plan = Top5Plan::new();
        plan.set_cores(&[2, 4, 6]);
        assert_eq!(3, plan.slot_count()); // fewer processors: fewer slots
        let mut t = t0();
        let cands = vec![cw(1, 0.5), cw(2, 0.3), cw(3, 0.2)];
        for _ in 0..4 {
            t = t.add_seconds(1.0);
            plan.step(t, &cands, None, None);
        }
        assert_eq!(3, plan.desired().len());
        for _ in 0..20 {
            t = t.add_seconds(1.0);
            plan.step(t, &[cw(1, 0.99), cw(2, 0.01), cw(3, 0.0)], None, None);
        }
        assert_eq!(vec![1], tids(&plan)); // nobody is kept in a slot for nothing
        t = t.add_seconds(1.0);
        plan.step(t, &[], None, None);
        assert!(plan.desired().is_empty());
    }
}
