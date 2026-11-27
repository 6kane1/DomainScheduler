//! The tracker: takes events (in any order across cpus, inside the reorder window), keeps the scheduling state of
//! every thread and cpu, derives the links and writes everything into the 30 s history.

use std::cmp::Ordering;
use std::collections::BinaryHeap;

use crate::events::*;
use crate::fx::{FxMap, FxSet};
use crate::history::*;
use crate::kwait::{self, state};

/// "No thread" / idle.
pub const NONE: u32 = u32::MAX;
/// Process serial for threads whose process is not known (yet).
pub const PROC_UNKNOWN: u32 = 0;
/// Process serial of the "DPC/ISR" pseudo process.
pub const PROC_KERNEL: u32 = 1;
/// Thread serial of the "DPC/ISR" pseudo thread (readier of DPC, timer and idle-cpu wakes).
pub const THREAD_KERNEL: u32 = 0;
pub const PID_UNKNOWN: u32 = u32::MAX;
/// Pseudo pid of "DPC/ISR". Real pids and tids are multiples of 4, so this never collides.
pub const PID_KERNEL: u32 = 0xFFFF_FFFE;
pub const TID_KERNEL: u32 = 0xFFFF_FFFE;
const MAX_CPUS: usize = 2048;
/// Run segments remembered per cpu (for "who held the core while the victim waited").
const SEGS: usize = 8;

#[derive(Clone, Debug)]
pub struct Config {
    /// QPC frequency (ticks per second) of the timestamps.
    pub qpc_freq: u64,
    /// How long events are held to sort them across cpus. ETW real-time delivers per-cpu buffers, so events of a
    /// quiet cpu can arrive late by up to the session's FlushTimer; events older than this window are dropped and
    /// counted in `Stats::late_dropped`.
    pub reorder_ms: u32,
    /// A ready wait at least this long counts the threads that ran on the cpu meanwhile as preempters.
    pub preempt_min_wait_us: u32,
    /// Logical cpus (0 = the highest cpu seen + 1).
    pub cpu_count: u32,
}

impl Config {
    pub fn new(qpc_freq: u64) -> Self {
        Self { qpc_freq, reorder_ms: 250, preempt_min_wait_us: 50, cpu_count: 0 }
    }
}

#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunState {
    Unknown = 0,
    Running = 1,
    Ready = 2,
    Waiting = 3,
    Gone = 4,
}

impl RunState {
    pub fn from_u8(v: u8) -> Self {
        match v {
            1 => Self::Running,
            2 => Self::Ready,
            3 => Self::Waiting,
            4 => Self::Gone,
            _ => Self::Unknown,
        }
    }
}

pub(crate) struct Proc {
    pub pid: u32,
    pub image: Option<String>,
    pub ended: Option<i64>,
}

pub(crate) struct Thread {
    pub tid: u32,
    pub proc: u32,
    pub desc: Option<String>,
    pub start_module: Option<String>,
    pub start_offset: u64,
    pub ended: Option<i64>,
    pub last_seen: i64,
    pub state: RunState,
    pub reason: u8,
    pub ready_since: Option<i64>,
    pub preempted_by: u32,
    /// The current ready episode began with a yield (WrYieldExecution): nobody preempted it.
    pub yielded: bool,
    pub prio: i8,
    pub cpu: u32,
    /// The cpu it last ran on (NONE before it ran): the "core" of the selected-node panel.
    pub last_cpu: u32,
}

impl Thread {
    fn new(tid: u32, proc: u32, now: i64) -> Self {
        Self {
            tid,
            proc,
            desc: None,
            start_module: None,
            start_offset: 0,
            ended: None,
            last_seen: now,
            state: RunState::Unknown,
            reason: 0,
            ready_since: None,
            preempted_by: NONE,
            yielded: false,
            prio: 0,
            cpu: NONE,
            last_cpu: NONE,
        }
    }
}

/// Threads by serial: the serial is the slot index, so lookups are array reads. A slot is reused only after the GC
/// removed its thread, which happens when no bucket of the history refers to it any more.
pub(crate) struct Slab<T> {
    items: Vec<Option<T>>,
    free: Vec<u32>,
    len: usize,
}

impl<T> Default for Slab<T> {
    fn default() -> Self {
        Self { items: Vec::new(), free: Vec::new(), len: 0 }
    }
}

impl<T> Slab<T> {
    pub fn insert_new(&mut self, t: T) -> u32 {
        self.len += 1;
        match self.free.pop() {
            Some(i) => {
                self.items[i as usize] = Some(t);
                i
            }
            None => {
                self.items.push(Some(t));
                (self.items.len() - 1) as u32
            }
        }
    }
    #[inline]
    pub fn get(&self, k: &u32) -> Option<&T> {
        self.items.get(*k as usize).and_then(|x| x.as_ref())
    }
    #[inline]
    pub fn get_mut(&mut self, k: &u32) -> Option<&mut T> {
        self.items.get_mut(*k as usize).and_then(|x| x.as_mut())
    }
    #[inline]
    pub fn contains_key(&self, k: &u32) -> bool {
        self.get(k).is_some()
    }
    pub fn remove(&mut self, k: &u32) -> Option<T> {
        let t = self.items.get_mut(*k as usize)?.take()?;
        self.free.push(*k);
        self.len -= 1;
        Some(t)
    }
    pub fn len(&self) -> usize {
        self.len
    }
    /// Highest slot + 1 (the range of serials).
    pub fn slots(&self) -> usize {
        self.items.len()
    }
    pub fn iter(&self) -> impl Iterator<Item = (u32, &T)> {
        self.items.iter().enumerate().filter_map(|(i, x)| x.as_ref().map(|t| (i as u32, t)))
    }
    pub fn values(&self) -> impl Iterator<Item = &T> {
        self.items.iter().filter_map(|x| x.as_ref())
    }
}

#[derive(Clone, Copy)]
struct Seg {
    th: u32,
    from: i64,
    to: i64,
}

#[derive(Clone, Copy)]
struct Cpu {
    cur: u32,
    since: i64,
    known: bool,
    segs: [Seg; SEGS],
    head: usize,
}

impl Cpu {
    fn new() -> Self {
        Self { cur: NONE, since: 0, known: false, segs: [Seg { th: NONE, from: 0, to: 0 }; SEGS], head: 0 }
    }
    fn push_seg(&mut self, s: Seg) {
        self.segs[self.head] = s;
        self.head = (self.head + 1) % SEGS;
    }
}

enum Ev {
    CSwitch(CSwitch),
    Ready(ReadyThread),
    Link(LinkEvent),
    ProcStart(u32, Box<str>),
    ProcEnd(u32),
    ThStart(Box<ThreadInfo>),
    ThEnd(u32),
    ThName(u32, Box<str>),
}

struct Pending {
    ns: i64,
    seq: u64,
    ev: Ev,
}
impl PartialEq for Pending {
    fn eq(&self, o: &Self) -> bool {
        self.ns == o.ns && self.seq == o.seq
    }
}
impl Eq for Pending {}
impl PartialOrd for Pending {
    fn partial_cmp(&self, o: &Self) -> Option<Ordering> {
        Some(self.cmp(o))
    }
}
impl Ord for Pending {
    // Reversed: BinaryHeap is a max-heap, we want the oldest first (ties: arrival order).
    fn cmp(&self, o: &Self) -> Ordering {
        o.ns.cmp(&self.ns).then(o.seq.cmp(&self.seq))
    }
}

#[derive(Clone, Debug, Default, serde::Serialize)]
pub struct Stats {
    pub events: u64,
    pub cswitch: u64,
    pub ready: u64,
    pub ext_links: u64,
    pub late_dropped: u64,
    pub pending: usize,
    pub threads: usize,
    pub processes: usize,
    pub gc_threads: u64,
    pub history_bytes: usize,
}

pub struct ThreadMap {
    pub(crate) cfg: Config,
    base_ticks: Option<i64>,
    seq: u64,
    heap: BinaryHeap<Pending>,
    max_ns: i64,
    pub(crate) now_ns: i64,
    pushes: u32,
    pub(crate) procs: FxMap<u32, Proc>,
    by_pid: FxMap<u32, u32>,
    next_proc: u32,
    pub(crate) threads: Slab<Thread>,
    by_tid: FxMap<u32, u32>,
    cpus: Vec<Cpu>,
    ready_set: FxSet<u32>,
    pub(crate) ring: Ring,
    pub(crate) game_pid: Option<u32>,
    /// Bumped whenever a known thread moves to another process (named late, tid reuse): frozen buckets stamped
    /// with an older epoch are read through the slow path.
    pub(crate) proc_epoch: u64,
    closes: u32,
    stats: Stats,
}

impl ThreadMap {
    pub fn new(cfg: Config) -> Self {
        let mut m = Self {
            cfg,
            base_ticks: None,
            seq: 0,
            heap: BinaryHeap::new(),
            max_ns: i64::MIN,
            now_ns: i64::MIN,
            pushes: 0,
            procs: FxMap::default(),
            by_pid: FxMap::default(),
            next_proc: 2,
            threads: Slab::default(),
            by_tid: FxMap::default(),
            cpus: Vec::new(),
            ready_set: FxSet::default(),
            ring: Ring::default(),
            game_pid: None,
            proc_epoch: 0,
            closes: 0,
            stats: Stats::default(),
        };
        m.seed_pseudo();
        m
    }

    fn seed_pseudo(&mut self) {
        self.procs.insert(PROC_UNKNOWN, Proc { pid: PID_UNKNOWN, image: None, ended: None });
        self.procs.insert(PROC_KERNEL, Proc { pid: PID_KERNEL, image: Some("DPC/ISR".into()), ended: None });
        let mut k = Thread::new(TID_KERNEL, PROC_KERNEL, 0);
        k.desc = Some("DPC/ISR".into());
        let s = self.threads.insert_new(k);
        debug_assert_eq!(s, THREAD_KERNEL);
    }

    /// Drops all events and history (the configuration and the game pid stay).
    pub fn clear(&mut self) {
        let cfg = self.cfg.clone();
        let game = self.game_pid;
        *self = Self::new(cfg);
        self.game_pid = game;
    }

    pub fn config(&self) -> &Config {
        &self.cfg
    }
    pub fn set_reorder_ms(&mut self, ms: u32) {
        self.cfg.reorder_ms = ms;
    }
    pub fn set_cpu_count(&mut self, n: u32) {
        self.cfg.cpu_count = n;
    }
    pub fn cpu_count(&self) -> u32 {
        if self.cfg.cpu_count > 0 {
            self.cfg.cpu_count
        } else {
            (self.cpus.len() as u32).max(1)
        }
    }
    /// The game process (time strip, "game" class). None clears it.
    pub fn set_game(&mut self, pid: Option<u32>) {
        self.game_pid = pid;
    }
    pub fn game(&self) -> Option<u32> {
        self.game_pid
    }

    pub fn stats(&self) -> Stats {
        let mut s = self.stats.clone();
        s.pending = self.heap.len();
        s.threads = self.threads.len();
        s.processes = self.procs.len();
        s.history_bytes = self.ring.bytes();
        s
    }

    /// Time of the newest processed event, in ns since the first event (None before any).
    pub fn now_ns(&self) -> Option<i64> {
        (self.now_ns != i64::MIN).then_some(self.now_ns)
    }

    // ---- feeding -----------------------------------------------------------------------------------------------

    fn to_ns(&mut self, ticks: i64) -> i64 {
        let base = *self.base_ticks.get_or_insert(ticks);
        let f = self.cfg.qpc_freq.max(1) as i128;
        ((ticks as i128 - base as i128) * 1_000_000_000).div_euclid(f) as i64
    }

    fn push(&mut self, ticks: i64, ev: Ev, timed: bool) {
        let ns = self.to_ns(ticks);
        if ns < self.now_ns {
            if timed {
                self.stats.late_dropped += 1;
                return;
            }
            // Late names are metadata: apply them now instead of losing them.
            self.apply(ns, ev);
            return;
        }
        self.seq += 1;
        self.heap.push(Pending { ns, seq: self.seq, ev });
        self.max_ns = self.max_ns.max(ns);
        self.pushes += 1;
        if self.pushes >= 4096 {
            self.pump();
        }
    }

    pub fn push_cswitch(&mut self, e: &CSwitch) {
        self.push(e.ts, Ev::CSwitch(*e), true)
    }
    pub fn push_ready(&mut self, e: &ReadyThread) {
        self.push(e.ts, Ev::Ready(*e), true)
    }
    pub fn push_link(&mut self, e: &LinkEvent) {
        self.push(e.ts, Ev::Link(*e), true)
    }
    pub fn process_start(&mut self, p: &ProcessInfo) {
        self.push(p.ts, Ev::ProcStart(p.pid, p.image.as_str().into()), false)
    }
    pub fn process_end(&mut self, ts: i64, pid: u32) {
        self.push(ts, Ev::ProcEnd(pid), false)
    }
    pub fn thread_start(&mut self, t: &ThreadInfo) {
        self.push(t.ts, Ev::ThStart(Box::new(t.clone())), false)
    }
    pub fn thread_end(&mut self, ts: i64, tid: u32) {
        self.push(ts, Ev::ThEnd(tid), false)
    }
    /// SetThreadDescription name (Thread SetName event or a user-mode query).
    pub fn thread_name(&mut self, ts: i64, tid: u32, name: &str) {
        self.push(ts, Ev::ThName(tid, name.into()), false)
    }

    /// Processes every held event older than the reorder window and charges running threads up to that point.
    pub fn pump(&mut self) {
        self.pushes = 0;
        if self.max_ns == i64::MIN {
            return;
        }
        let wm = self.max_ns - self.cfg.reorder_ms as i64 * 1_000_000;
        while self.heap.peek().is_some_and(|p| p.ns <= wm) {
            let p = self.heap.pop().unwrap();
            self.apply(p.ns, p.ev);
        }
        if wm > self.now_ns {
            self.settle(wm);
        }
    }

    /// Processes everything held (end of a recording, tests).
    pub fn flush(&mut self) {
        self.pushes = 0;
        while let Some(p) = self.heap.pop() {
            self.apply(p.ns, p.ev);
        }
        if self.max_ns != i64::MIN && self.max_ns >= self.now_ns {
            self.settle(self.max_ns);
        }
    }

    fn apply(&mut self, ns: i64, ev: Ev) {
        self.stats.events += 1;
        let in_order = ns >= self.now_ns;
        if in_order {
            self.advance_to(ns.div_euclid(BUCKET_NS));
            self.now_ns = ns;
        }
        match ev {
            Ev::CSwitch(e) => {
                self.stats.cswitch += 1;
                self.on_cswitch(&e, ns)
            }
            Ev::Ready(e) => {
                self.stats.ready += 1;
                self.on_ready(&e, ns)
            }
            Ev::Link(e) => {
                self.stats.ext_links += 1;
                let f = self.th_old(e.from_tid, ns);
                let t = self.th_old(e.to_tid, ns);
                if let Some(k) = LinkKind::from_u8(e.kind) {
                    self.link(f, t, k, e.via, e.count.max(1));
                }
            }
            Ev::ProcStart(pid, image) => self.on_proc_start(pid, image.into()),
            Ev::ProcEnd(pid) => {
                if let Some(&s) = self.by_pid.get(&pid) {
                    if let Some(p) = self.procs.get_mut(&s) {
                        p.ended.get_or_insert(ns);
                    }
                }
            }
            Ev::ThStart(t) => self.on_thread_start(*t, ns),
            Ev::ThEnd(tid) => {
                if let Some(&s) = self.by_tid.get(&tid) {
                    if let Some(t) = self.threads.get_mut(&s) {
                        t.ended.get_or_insert(ns);
                    }
                }
            }
            Ev::ThName(tid, name) => {
                let s = self.th_old(tid, ns);
                if let Some(t) = self.threads.get_mut(&s) {
                    t.desc = (!name.is_empty()).then(|| name.into());
                }
            }
        }
    }

    // ---- registry ----------------------------------------------------------------------------------------------

    fn new_thread(&mut self, tid: u32, proc: u32, now: i64) -> u32 {
        let s = self.threads.insert_new(Thread::new(tid, proc, now));
        self.by_tid.insert(tid, s);
        s
    }

    /// A tid that is running or being readied: must be a live thread; an ended one means the tid was reused
    /// without a start event, so a new thread is made.
    fn th_live(&mut self, tid: u32, now: i64) -> u32 {
        if tid == 0 {
            return NONE;
        }
        if let Some(&s) = self.by_tid.get(&tid) {
            if self.threads.get(&s).is_some_and(|t| t.ended.is_none()) {
                return s;
            }
        }
        self.new_thread(tid, PROC_UNKNOWN, now)
    }

    /// A tid that is switching out / sending: an ended thread is fine (its last switch-out comes after Thread End).
    fn th_old(&mut self, tid: u32, now: i64) -> u32 {
        if tid == 0 {
            return NONE;
        }
        if let Some(&s) = self.by_tid.get(&tid) {
            if self.threads.contains_key(&s) {
                return s;
            }
        }
        self.new_thread(tid, PROC_UNKNOWN, now)
    }

    fn proc_for(&mut self, pid: u32) -> u32 {
        if let Some(&s) = self.by_pid.get(&pid) {
            if self.procs.get(&s).is_some_and(|p| p.ended.is_none()) {
                return s;
            }
        }
        self.new_proc(pid, None)
    }

    fn new_proc(&mut self, pid: u32, image: Option<String>) -> u32 {
        let mut s = self.next_proc;
        while s == PROC_UNKNOWN || s == PROC_KERNEL || self.procs.contains_key(&s) {
            s = s.wrapping_add(1);
        }
        self.next_proc = s.wrapping_add(1);
        self.procs.insert(s, Proc { pid, image, ended: None });
        self.by_pid.insert(pid, s);
        s
    }

    fn on_proc_start(&mut self, pid: u32, image: String) {
        if let Some(&s) = self.by_pid.get(&pid) {
            if let Some(p) = self.procs.get_mut(&s) {
                if p.ended.is_none() && (p.image.is_none() || p.image.as_deref() == Some(image.as_str())) {
                    p.image = Some(image);
                    return;
                }
                p.ended.get_or_insert(self.now_ns.max(0));
            }
        }
        self.new_proc(pid, Some(image));
    }

    fn on_thread_start(&mut self, info: ThreadInfo, now: i64) {
        let proc = self.proc_for(info.pid);
        let reuse = match self.by_tid.get(&info.tid) {
            Some(&s) => match self.threads.get(&s) {
                Some(t) if t.ended.is_none() && (t.proc == PROC_UNKNOWN || t.proc == proc) => Some(s),
                Some(_) => {
                    if let Some(t) = self.threads.get_mut(&s) {
                        t.ended.get_or_insert(now);
                    }
                    None
                }
                None => None,
            },
            None => None,
        };
        let s = reuse.unwrap_or_else(|| self.new_thread(info.tid, proc, now));
        if let Some(t) = self.threads.get_mut(&s) {
            if t.proc != proc {
                t.proc = proc;
                self.proc_epoch += 1;
            }
            if info.description.as_deref().is_some_and(|d| !d.is_empty()) {
                t.desc = info.description;
            }
            if info.start_module.is_some() {
                t.start_module = info.start_module;
                t.start_offset = info.start_offset;
            }
        }
    }

    // ---- history -----------------------------------------------------------------------------------------------

    fn advance_to(&mut self, k: i64) {
        let n = match self.ring.newest {
            None => {
                self.ring.open(k, self.proc_epoch);
                return;
            }
            Some(n) if k <= n => return,
            Some(n) => n,
        };
        self.charge_running((n + 1) * BUCKET_NS);
        self.close(n);
        let start = (n + 1).max(k - BUCKETS as i64 + 1);
        if start > n + 1 {
            for c in &mut self.cpus {
                c.since = c.since.max(start * BUCKET_NS);
            }
        }
        for c in start..=k {
            self.ring.open(c, self.proc_epoch);
            if c < k {
                self.charge_running((c + 1) * BUCKET_NS);
                self.close(c);
            }
        }
    }

    fn close(&mut self, k: i64) {
        let running: Vec<(u32, u32)> =
            self.cpus.iter().filter(|c| c.cur != NONE).map(|c| (c.cur, self.proc_of(c.cur))).collect();
        let ready: Vec<(u32, u32)> = self.ready_set.iter().map(|&s| (s, self.proc_of(s))).collect();
        let agg = self.ring.get(k).map(|b| self.game_agg(b));
        if let Some(b) = self.ring.get_mut(k) {
            for (s, p) in running {
                b.set_state(s, p, RunState::Running as u8, 0);
            }
            for (s, p) in ready {
                b.set_state(s, p, RunState::Ready as u8, 0);
            }
            let threads = &self.threads;
            b.freeze(&|s| threads.get(&s).map(|t| t.proc).unwrap_or(NO_PROC), self.proc_epoch);
            if let Some(a) = agg {
                b.game = a;
                b.game_for = self.game_pid;
            }
        }
        self.closes += 1;
        if self.closes % 100 == 0 {
            self.gc();
        }
    }

    /// Forget threads and processes that ended (or, for unnamed ones, were last seen) before the history start.
    fn gc(&mut self) {
        let Some(old) = self.ring.oldest() else { return };
        let cutoff = old * BUCKET_NS - 1_000_000_000;
        let dead: Vec<u32> = self
            .threads
            .iter()
            .filter(|&(s, t)| {
                s != THREAD_KERNEL
                    && t.state != RunState::Running
                    && t.state != RunState::Ready
                    && (t.ended.is_some_and(|e| e < cutoff) || (t.proc == PROC_UNKNOWN && t.last_seen < cutoff))
            })
            .map(|(s, _)| s)
            .collect();
        for s in dead {
            if let Some(t) = self.threads.remove(&s) {
                if self.by_tid.get(&t.tid) == Some(&s) {
                    self.by_tid.remove(&t.tid);
                }
                self.stats.gc_threads += 1;
            }
        }
        let used: FxSet<u32> = self.threads.values().map(|t| t.proc).collect();
        let dead: Vec<u32> = self
            .procs
            .iter()
            .filter(|(&s, p)| s > PROC_KERNEL && !used.contains(&s) && p.ended.is_some_and(|e| e < cutoff))
            .map(|(&s, _)| s)
            .collect();
        for s in dead {
            if let Some(p) = self.procs.remove(&s) {
                if self.by_pid.get(&p.pid) == Some(&s) {
                    self.by_pid.remove(&p.pid);
                }
            }
        }
    }

    fn charge(&mut self, s: u32, from: i64, to: i64) {
        let Some(old) = self.ring.oldest() else { return };
        let proc = self.proc_of(s);
        let mut t = from.max(old * BUCKET_NS);
        while t < to {
            let k = t.div_euclid(BUCKET_NS);
            let end = to.min((k + 1) * BUCKET_NS);
            if let Some(b) = self.ring.get_mut(k) {
                b.add_run(s, proc, end - t);
            }
            t = end;
        }
    }

    fn charge_running(&mut self, t: i64) {
        for i in 0..self.cpus.len() {
            let c = self.cpus[i];
            if c.known && c.cur != NONE && c.since < t {
                self.charge(c.cur, c.since, t);
            }
            if self.cpus[i].known {
                self.cpus[i].since = c.since.max(t);
            }
        }
    }

    fn settle(&mut self, t: i64) {
        self.advance_to(t.div_euclid(BUCKET_NS));
        self.charge_running(t);
        self.now_ns = self.now_ns.max(t);
    }

    #[inline]
    fn proc_of(&self, s: u32) -> u32 {
        self.threads.get(&s).map(|t| t.proc).unwrap_or(NO_PROC)
    }

    fn cur_bucket(&mut self) -> Option<&mut Bucket> {
        let n = self.ring.newest?;
        self.ring.get_mut(n)
    }

    fn link(&mut self, from: u32, to: u32, kind: LinkKind, via: u8, count: u32) {
        if from == NONE || to == NONE || from == to {
            return;
        }
        let (pf, pt) = (self.proc_of(from), self.proc_of(to));
        if let Some(b) = self.cur_bucket() {
            b.add_link(from, to, pf, pt, kind as u8, via, count);
        }
    }

    fn set_state(&mut self, s: u32, st: RunState, reason: u8) {
        let mut proc = NO_PROC;
        if let Some(t) = self.threads.get_mut(&s) {
            t.state = st;
            t.reason = reason;
            proc = t.proc;
        }
        if st == RunState::Ready {
            self.ready_set.insert(s);
        } else {
            self.ready_set.remove(&s);
        }
        if let Some(b) = self.cur_bucket() {
            b.set_state(s, proc, st as u8, reason);
        }
    }

    // ---- scheduling events -------------------------------------------------------------------------------------

    fn on_cswitch(&mut self, e: &CSwitch, ns: i64) {
        let ci = e.cpu as usize;
        if ci >= MAX_CPUS {
            return;
        }
        if self.cpus.len() <= ci {
            self.cpus.resize(ci + 1, Cpu::new());
        }
        let new = self.th_live(e.new_tid, ns);
        let old = self.th_old(e.old_tid, ns);
        let cpu = self.cpus[ci];
        if old != NONE && cpu.known && cpu.since < ns {
            self.charge(old, cpu.since, ns);
        }
        self.cpus[ci].push_seg(Seg { th: old, from: if cpu.known { cpu.since } else { ns }, to: ns });

        if old != NONE && old != new {
            let st = e.old_state;
            if let Some(t) = self.threads.get_mut(&old) {
                t.cpu = NONE;
                t.prio = e.old_priority;
                t.last_seen = ns;
            }
            if st == state::READY || st == state::DEFERRED_READY || st == state::STANDBY {
                // Switched out while still runnable: someone took the core (unless it yielded on its own).
                let attacker = if new != NONE && e.old_wait_reason != kwait::WR_YIELD_EXECUTION { new } else { NONE };
                if let Some(t) = self.threads.get_mut(&old) {
                    t.ready_since.get_or_insert(ns);
                    t.preempted_by = attacker;
                    t.yielded = e.old_wait_reason == kwait::WR_YIELD_EXECUTION;
                }
                self.set_state(old, RunState::Ready, e.old_wait_reason);
                if attacker != NONE {
                    self.link(attacker, old, LinkKind::Preempt, via::SCHED, 1);
                }
            } else if st == state::WAITING {
                if let Some(t) = self.threads.get_mut(&old) {
                    t.ready_since = None;
                    t.preempted_by = NONE;
                }
                self.set_state(old, RunState::Waiting, e.old_wait_reason);
            } else if st == state::TERMINATED {
                self.set_state(old, RunState::Gone, 0);
            } else {
                self.set_state(old, RunState::Unknown, e.old_wait_reason);
            }
        }

        if new != NONE {
            let (since, pb, yielded) = match self.threads.get_mut(&new) {
                Some(t) => (t.ready_since.take(), t.preempted_by, std::mem::take(&mut t.yielded)),
                None => (None, NONE, false),
            };
            if let Some(r) = since {
                let w = ns - r;
                if w >= 0 {
                    let proc = self.proc_of(new);
                    if let Some(b) = self.cur_bucket() {
                        b.add_wait(new, proc, w);
                    }
                    if !yielded && w >= self.cfg.preempt_min_wait_us as i64 * 1000 {
                        // Who held this core while `new` waited for it (the direct preempter is already counted).
                        let segs = self.cpus[ci].segs;
                        let mut seen = [NONE; SEGS];
                        let mut n = 0;
                        for sg in segs {
                            if sg.th != NONE && sg.th != new && sg.th != pb && sg.to > r && sg.from < ns && !seen[..n].contains(&sg.th) {
                                seen[n] = sg.th;
                                n += 1;
                            }
                        }
                        for &a in &seen[..n] {
                            self.link(a, new, LinkKind::Preempt, via::SCHED, 1);
                        }
                    }
                }
            }
            if let Some(t) = self.threads.get_mut(&new) {
                t.preempted_by = NONE;
                t.cpu = ci as u32;
                t.last_cpu = ci as u32;
                t.prio = e.new_priority;
                t.last_seen = ns;
            }
            self.set_state(new, RunState::Running, 0);
        }
        let c = &mut self.cpus[ci];
        c.cur = new;
        c.since = ns;
        c.known = true;
    }

    fn on_ready(&mut self, e: &ReadyThread, ns: i64) {
        let target = self.th_live(e.target_tid, ns);
        if target == NONE {
            return;
        }
        let readier = if e.flags & READY_FLAG_DPC != 0 {
            THREAD_KERNEL
        } else if e.readier_tid != READIER_UNKNOWN {
            if e.readier_tid == 0 {
                THREAD_KERNEL
            } else {
                self.th_old(e.readier_tid, ns)
            }
        } else {
            match self.cpus.get(e.cpu as usize) {
                Some(c) if c.known => {
                    if c.cur == NONE {
                        THREAD_KERNEL
                    } else {
                        c.cur
                    }
                }
                _ => NONE,
            }
        };
        let (st, reason) = match self.threads.get_mut(&target) {
            Some(t) => {
                t.last_seen = ns;
                (t.state, t.reason)
            }
            None => return,
        };
        let real = readier != NONE && readier != THREAD_KERNEL && readier != target;
        if st == RunState::Waiting && real {
            if kwait::is_lock(reason) {
                self.link(target, readier, LinkKind::Lock, via::SCHED, 1);
            } else if reason == kwait::WR_LPC_REPLY {
                // The client waited for the answer; the server replied.
                self.link(target, readier, LinkKind::Ipc, via::LPC_WAIT, 1);
            } else if reason == kwait::WR_LPC_RECEIVE {
                // The server waited for a request; the client sent it.
                self.link(readier, target, LinkKind::Ipc, via::LPC_WAIT, 1);
            }
        }
        self.link(readier, target, LinkKind::Wake, via::SCHED, 1);
        if st != RunState::Running {
            if let Some(t) = self.threads.get_mut(&target) {
                if t.ready_since.is_none() {
                    t.ready_since = Some(ns);
                    t.preempted_by = NONE;
                    t.yielded = false;
                }
            }
            self.set_state(target, RunState::Ready, 0);
        }
    }

    // ---- game figures ------------------------------------------------------------------------------------------

    pub(crate) fn pid_of(&self, serial: u32) -> Option<u32> {
        let t = self.threads.get(&serial)?;
        self.procs.get(&t.proc).map(|p| p.pid)
    }

    pub(crate) fn game_agg(&self, b: &Bucket) -> GameAgg {
        let mut a = GameAgg { p99_us: -1.0, ..Default::default() };
        let Some(g) = self.game_pid else { return a };
        let mut hist = [0u32; BINS];
        if b.stamped(self.proc_epoch) {
            let is_game = |p: u32| self.procs.get(&p).is_some_and(|x| x.pid == g);
            b.for_p_th(|r| {
                if is_game(r.proc) {
                    a.run_ns += r.run_ns;
                    a.ready_ns += r.ready_ns;
                    a.waits += r.waits;
                }
            });
            b.for_p_waits(|w| {
                if is_game(w.proc) {
                    hist[w.bin as usize] += w.count;
                }
            });
        } else {
            let mut cache: FxMap<u32, bool> = FxMap::default();
            let mut is_game = |s: u32| *cache.entry(s).or_insert_with(|| self.pid_of(s) == Some(g));
            b.for_th(|r| {
                if is_game(r.serial) {
                    a.run_ns += r.run_ns as u64;
                    a.ready_ns += r.ready_ns as u64;
                    a.waits += r.waits;
                }
            });
            b.for_waits(|w| {
                if is_game(w.serial) {
                    hist[w.bin as usize] += w.count;
                }
            });
        }
        a.p99_us = p99_hist(&hist, 1).map(|v| v as f32).unwrap_or(-1.0);
        a
    }

    /// The time strip: one entry per bucket, oldest first (index 299 = the newest, open bucket).
    pub fn series(&self) -> Series {
        let mut s = Series {
            bucket_ms: BUCKET_MS as u32,
            cpu_count: self.cpu_count(),
            game_pid: self.game_pid,
            load: vec![0.0; BUCKETS],
            wait_ms: vec![0.0; BUCKETS],
            p99_us: vec![-1.0; BUCKETS],
            waits: vec![0; BUCKETS],
            valid: vec![0; BUCKETS],
        };
        let Some(n) = self.ring.newest else { return s };
        for i in 0..BUCKETS {
            let k = n - (BUCKETS - 1 - i) as i64;
            let Some(b) = self.ring.get(k) else { continue };
            let a = if !b.open && b.game_for == self.game_pid { b.game } else { self.game_agg(b) };
            let dur = if b.open { (self.now_ns - k * BUCKET_NS).clamp(1, BUCKET_NS) } else { BUCKET_NS };
            s.load[i] = (a.run_ns as f64 / dur as f64) as f32;
            s.wait_ms[i] = (a.ready_ns as f64 / 1e6) as f32;
            s.p99_us[i] = a.p99_us;
            s.waits[i] = a.waits;
            s.valid[i] = 1;
        }
        s
    }

    // ---- read access for the graph builder ---------------------------------------------------------------------

    pub(crate) fn ready_since(&self, s: u32) -> Option<i64> {
        self.threads.get(&s).and_then(|t| t.ready_since)
    }
    pub(crate) fn live_active(&self) -> impl Iterator<Item = u32> + '_ {
        self.cpus.iter().filter(|c| c.cur != NONE).map(|c| c.cur).chain(self.ready_set.iter().copied())
    }
}

/// Time strip data (`ThreadMap::series`). All vectors have 300 entries, oldest first.
#[derive(Clone, Debug, serde::Serialize)]
pub struct Series {
    pub bucket_ms: u32,
    pub cpu_count: u32,
    pub game_pid: Option<u32>,
    /// Game cpu load in cores (1.0 = one logical cpu busy for the whole bucket); divide by cpu_count for a share.
    pub load: Vec<f32>,
    /// Sum of the game threads' waits for a core (ready -> running) that ended in the bucket, in ms.
    pub wait_ms: Vec<f32>,
    /// p99 of those waits in us (-1 = none).
    pub p99_us: Vec<f32>,
    pub waits: Vec<u32>,
    /// 1 when the bucket holds data.
    pub valid: Vec<u8>,
}
