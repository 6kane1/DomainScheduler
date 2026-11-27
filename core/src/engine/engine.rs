//! Applies masks, priorities and ideal processors with baselines (section 10, port of `Engine.cs`). Every write
//! happens only when the current value differs, every change can be reverted, SetThreadAffinityMask is never called.
//!
//! Locking: C# takes `lock(_gate)` (a re-entrant monitor) in every public method, and the hooks it calls under that
//! lock may call the engine again (BeforeMutate saves the session through `export`, a gate hook reads
//! `processes_now`). The port keeps that with a re-entrant gate ([`ReGate`]); the engine's maps sit in a separate
//! mutex that is only held for short stretches and never while a hook (gate, BeforeMutate, Sleep, pick) runs.

use super::auto_logic::{AutoLogic, AutoPolicy, Classified, ThreadRole};
use super::background_deny::BackgroundDeny;
use super::guard::Guard;
use super::session::{SessionData, SessionProc, SessionThread};
use crate::base::app_log::{AppLog, Sev};
use crate::base::ci::{self, CiMap, CiMapExt, CiSet, CiSetExt};
use crate::base::flags::Flags;
use crate::base::hooks::lock;
use crate::base::l::L;
use crate::base::model::{
    ApplyOptions, ApplyReport, CpuSet, ModCacheE, ModRange, Preset, ProcBefore, ThrBefore, ThrMetaE, ThrPrevE, ThreadRow, ThreadRule, ThreadSnap,
};
use crate::base::settings::{parse_int_leading_sign, parse_int_none};
use crate::base::time::{DateTime, TimeSpan};
use crate::lf;
use crate::native::{Handle, INative};
use crate::topo::builder::TopoBuilder;
use crate::topo::game_mask::GameMask;
use crate::topo::masks::Masks;
use crate::topo::model::Topo;
use crate::topo::reservation::Reservation;
use indexmap::{IndexMap, IndexSet};
use std::collections::{HashMap, HashSet};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicI64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock, RwLock};
use std::thread::ThreadId;

// ---- the re-entrant gate (C# lock / Monitor) ------------------------------------------------------------------------

/// A re-entrant lock without data: the C# `lock (_gate)`.
pub struct ReGate {
    m: Mutex<(Option<ThreadId>, usize)>,
    cv: Condvar,
}

pub struct ReGuard<'a>(&'a ReGate);

impl Default for ReGate {
    fn default() -> Self {
        ReGate { m: Mutex::new((None, 0)), cv: Condvar::new() }
    }
}

impl ReGate {
    pub fn enter(&self) -> ReGuard<'_> {
        let me = std::thread::current().id();
        let mut g = lock(&self.m);
        loop {
            match g.0 {
                None => {
                    *g = (Some(me), 1);
                    break;
                }
                Some(o) if o == me => {
                    g.1 += 1;
                    break;
                }
                _ => g = self.cv.wait(g).unwrap_or_else(|e| e.into_inner()),
            }
        }
        ReGuard(self)
    }
}

impl Drop for ReGuard<'_> {
    fn drop(&mut self) {
        let mut g = lock(&self.0.m);
        g.1 -= 1;
        if g.1 == 0 {
            g.0 = None;
            self.0.cv.notify_one();
        }
    }
}

// ---- tables ---------------------------------------------------------------------------------------------------------

const PRIO: [(&str, u32); 6] = [("idle", 64), ("below", 16384), ("normal", 32), ("above", 32768), ("high", 128), ("realtime", 256)];
const TPRIO: [(&str, i32); 7] = [("idle", -15), ("lowest", -2), ("below", -1), ("normal", 0), ("above", 1), ("highest", 2), ("critical", 15)];
const IOP: [(&str, u32); 5] = [("verylow", 0), ("low", 1), ("normal", 2), ("high", 3), ("critical", 4)];
const MEM: [(&str, u32); 5] = [("verylow", 1), ("low", 2), ("medium", 3), ("belownormal", 4), ("normal", 5)];

fn find<T: Copy>(t: &[(&str, T)], k: &str) -> Option<T> {
    t.iter().find(|(n, _)| ci::eq(n, k)).map(|(_, v)| *v)
}

const PRIO_REALTIME: u32 = 256;
const PRIO_HIGH: u32 = 128;
const PRIO_NORMAL: u32 = 32;
const PRIO_ABOVE: u32 = 32768;
const PRIO_BELOW: u32 = 16384;
const IOP_LOW: u32 = 1;
const MEM_LOW: u32 = 2;

const TIMER_BIT: u32 = 4; // PROCESS_POWER_THROTTLING_IGNORE_TIMER_RESOLUTION
const ECO_ONLY: u32 = 1;
const ECO_AND_TIMER: u32 = 1 | TIMER_BIT;

/// Processes whose timing must not be coarsened: system, audio and voice, streaming, browsers (media).
fn timer_keepers() -> &'static CiSet {
    static V: OnceLock<CiSet> = OnceLock::new();
    V.get_or_init(|| {
        ci::set_of([
            "System", "Registry", "smss.exe", "csrss.exe", "wininit.exe", "winlogon.exe", "services.exe", "lsass.exe", "svchost.exe",
            "dwm.exe", "audiodg.exe", "fontdrvhost.exe", "MsMpEng.exe", "explorer.exe",
            "Discord.exe", "Vesktop.exe", "ts3client_win64.exe", "TeamSpeak.exe", "mumble.exe", "Spotify.exe",
            "obs64.exe", "obs32.exe", "Streamlabs OBS.exe", "Streamlabs Desktop.exe", "NVIDIA Share.exe", "nvcontainer.exe",
            "chrome.exe", "msedge.exe", "firefox.exe", "opera.exe", "brave.exe", "vivaldi.exe", "msedgewebview2.exe",
            "steam.exe", "steamwebhelper.exe",
        ])
    })
}

fn key(pid: u32, tid: u32) -> String {
    format!("{}:{}", pid, tid)
}

/// `uint.TryParse(s, NumberStyles.None, InvariantCulture)`.
fn parse_uint_none(s: &str) -> Option<u32> {
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    s.parse::<u32>().ok()
}

fn ids_key(ids: &[u32]) -> String {
    let mut v = ids.to_vec();
    v.sort();
    v.iter().map(|x| x.to_string()).collect::<Vec<_>>().join(",")
}

/// The C# code compares `want.GetHashCode()` with what it remembered (in memory only, per run); any stable hash of
/// the string does the same.
fn str_hash(s: &str) -> u64 {
    let mut h: u64 = 1469598103934665603;
    for b in s.bytes() {
        h = (h ^ b as u64).wrapping_mul(1099511628211);
    }
    h
}

fn split_key(k: &str) -> (u32, u32) {
    let i = k.find(':').unwrap_or(k.len());
    (k[..i].parse().unwrap_or(0), k.get(i + 1..).and_then(|x| x.parse().ok()).unwrap_or(0))
}

/// `Engine.Wildcard(p)`: `^p$` with `*` = any run and `?` = one char (neither crosses a line break), case-insensitive,
/// `$` also before one final line break (the .NET regex the C# code builds).
#[derive(Clone, Debug)]
pub struct Wildcard(Vec<char>);

impl Wildcard {
    pub fn new(p: &str) -> Self {
        Wildcard(p.chars().map(ci::lower_char).collect())
    }

    fn matches(&self, s: &[char]) -> bool {
        let p = &self.0;
        // dp[j]: p[..j] matches s[..i]
        let mut dp = vec![false; p.len() + 1];
        dp[0] = true;
        for j in 1..=p.len() {
            dp[j] = dp[j - 1] && p[j - 1] == '*';
        }
        for &c in s {
            let c = ci::lower_char(c);
            let mut next = vec![false; p.len() + 1];
            for j in 1..=p.len() {
                next[j] = match p[j - 1] {
                    '*' => next[j - 1] || (dp[j] && c != '\n'),
                    '?' => dp[j - 1] && c != '\n',
                    x => dp[j - 1] && x == c,
                };
            }
            dp = next;
        }
        dp[p.len()]
    }

    pub fn is_match(&self, s: &str) -> bool {
        let c: Vec<char> = s.chars().collect();
        self.matches(&c) || (c.last() == Some(&'\n') && self.matches(&c[..c.len() - 1]))
    }
}

// ---- public small types ---------------------------------------------------------------------------------------------

/// One target of a pass (`Engine.Target`).
#[derive(Clone, Debug, Default)]
pub struct Target {
    pub pid: u32,
    pub preset: Preset,
    pub options: ApplyOptions,
}

/// What a process has now (read for the Processes page). Denied processes are not opened. (`Engine.ProcInfo`)
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ProcInfo {
    pub ok: bool,
    pub denied: bool,
    pub changed: bool,
    pub boost_off: bool,
    pub class: u32,
    pub affinity: u64,
    pub level: i32,
}

pub type NowFn = dyn Fn() -> DateTime + Send + Sync;
pub type SleepFn = dyn Fn(i32) + Send + Sync;
pub type MutateFn = dyn Fn() + Send + Sync;
/// Why a process must not be touched (anti-cheat); None = may.
pub type GateFn = dyn Fn(u32, &str) -> Option<String> + Send + Sync;
pub type Procs = IndexMap<u32, (String, u32)>;
pub type Tids = IndexMap<u32, Vec<u32>>;

struct Hooks {
    now: Arc<NowFn>,
    sleep: Arc<SleepFn>,
    before_mutate: Option<Arc<MutateFn>>,
    gate: Option<Arc<GateFn>>,
}

#[derive(Default)]
struct St {
    /// "pid:tid" -> processor whose CPU Set the thread is limited to
    thr_sets: IndexMap<String, u32>,
    warn_once: HashSet<String>,
    timer_requested: bool,
    timer_value: u32,
    proc_before: IndexMap<u32, ProcBefore>,
    proc_name: HashMap<u32, String>,
    bg_pids: IndexSet<u32>,
    thr_before: IndexMap<String, ThrBefore>,
    ideal: IndexMap<String, u32>,
    thr_prev: IndexMap<String, ThrPrevE>,
    thr_meta: IndexMap<String, ThrMetaE>,
    mod_cache: IndexMap<u32, ModCacheE>,
    born: IndexMap<u32, i64>,
    tid_cache: Option<Tids>,
    pid_cache: Option<Procs>,
    preset_tid_count: IndexMap<String, i32>,
    advice_once: HashSet<String>,
    /// "pid:field" Windows refused once: not tried again for this process
    skip_field: HashSet<String>,
    /// "pid:field" written while the value cannot be read back
    written: HashMap<String, u64>,
    /// fields this Windows does not support at all (error 1 / 50)
    unsupported: HashSet<String>,
    debug: bool,
    last_reports: Vec<ApplyReport>,
    advice: Vec<String>,
    last_snap: Option<ThreadSnap>,
    last_denied_share: f64,
    failure_once: HashSet<String>,
    snaps: HashMap<u32, ThreadSnap>,
    timer_unsupported: bool,
    bg_skip_logged: CiSet,
}

impl St {
    fn forget(&mut self, pid: u32) {
        let prefix = format!("{}:", pid);
        self.proc_before.shift_remove(&pid);
        self.proc_name.remove(&pid);
        self.bg_pids.shift_remove(&pid);
        self.mod_cache.shift_remove(&pid);
        self.born.shift_remove(&pid);
        self.snaps.remove(&pid);
        self.thr_before.retain(|k, _| !k.starts_with(&prefix));
        self.ideal.retain(|k, _| !k.starts_with(&prefix));
        self.thr_sets.retain(|k, _| !k.starts_with(&prefix));
        self.thr_prev.retain(|k, _| !k.starts_with(&prefix));
        self.thr_meta.retain(|k, _| !k.starts_with(&prefix));
        self.preset_tid_count.retain(|k, _| !k.starts_with(&prefix));
        self.skip_field.retain(|k| !k.starts_with(&prefix));
        self.written.retain(|k, _| !k.starts_with(&prefix));
    }

    /// `_born` check of every path that opens a process: a new process under the same PID drops what was known.
    fn note_born(&mut self, pid: u32, created: i64) {
        if let Some(b) = self.born.get(&pid) {
            if *b != created {
                self.forget(pid);
            }
        }
        self.born.insert(pid, created);
    }

    fn advice_once(&mut self, key: &str, text: &str) {
        if text.is_empty() || !self.advice_once.insert(key.to_string()) {
            return;
        }
        self.advice.push(text.to_string());
        AppLog::add_sev(text, Sev::Warn);
    }
}

/// `Engine`.
pub struct Engine {
    n: Arc<dyn INative>,
    gate: ReGate,
    st: Mutex<St>,
    hooks: RwLock<Hooks>,
    set_calls: AtomicI64,
    reverts: AtomicI64,
    timer_guard: AtomicBool,
    manual_calls: AtomicI32,
}

impl Engine {
    /// Section 22.10: masks and affinity are always compiled in; there is no runtime gate.
    pub const FEATURE_AFFINITY: bool = true;

    pub fn new(native: Arc<dyn INative>) -> Self {
        Engine {
            n: native,
            gate: ReGate::default(),
            st: Mutex::new(St::default()),
            hooks: RwLock::new(Hooks {
                now: Arc::new(DateTime::utc_now),
                sleep: Arc::new(|ms| std::thread::sleep(std::time::Duration::from_millis(ms.max(0) as u64))),
                before_mutate: None,
                gate: None,
            }),
            set_calls: AtomicI64::new(0),
            reverts: AtomicI64::new(0),
            timer_guard: AtomicBool::new(true),
            manual_calls: AtomicI32::new(0),
        }
    }

    // ---- the C# tables and public fields ------------------------------------------------------------------------

    /// `Engine.PRIO[name]` (case-insensitive).
    pub fn prio(name: &str) -> Option<u32> {
        find(&PRIO, name)
    }
    pub fn tprio(name: &str) -> Option<i32> {
        find(&TPRIO, name)
    }
    pub fn iop(name: &str) -> Option<u32> {
        find(&IOP, name)
    }
    pub fn mem(name: &str) -> Option<u32> {
        find(&MEM, name)
    }
    pub fn prio_contains_value(v: u32) -> bool {
        PRIO.iter().any(|(_, x)| *x == v)
    }
    pub fn tprio_contains_value(v: i32) -> bool {
        TPRIO.iter().any(|(_, x)| *x == v)
    }
    /// The tables in their C# order (for the UI).
    pub fn prio_table() -> &'static [(&'static str, u32)] {
        &PRIO
    }
    pub fn tprio_table() -> &'static [(&'static str, i32)] {
        &TPRIO
    }
    pub fn iop_table() -> &'static [(&'static str, u32)] {
        &IOP
    }
    pub fn mem_table() -> &'static [(&'static str, u32)] {
        &MEM
    }

    fn st(&self) -> MutexGuard<'_, St> {
        lock(&self.st)
    }

    fn hooks(&self) -> std::sync::RwLockReadGuard<'_, Hooks> {
        self.hooks.read().unwrap_or_else(|e| e.into_inner())
    }

    /// `Engine.Now` (default DateTime.UtcNow).
    pub fn set_now(&self, f: Arc<NowFn>) {
        self.hooks.write().unwrap_or_else(|e| e.into_inner()).now = f;
    }
    /// `Engine.Sleep` (default Thread.Sleep).
    pub fn set_sleep(&self, f: Arc<SleepFn>) {
        self.hooks.write().unwrap_or_else(|e| e.into_inner()).sleep = f;
    }
    /// `Engine.BeforeMutate`: called after baselines were captured and before the first change (crash-safe session).
    pub fn set_before_mutate(&self, f: Option<Arc<MutateFn>>) {
        self.hooks.write().unwrap_or_else(|e| e.into_inner()).before_mutate = f;
    }
    /// `Engine.GateHook`: why a process must not be touched (anti-cheat); None = may.
    pub fn set_gate_hook(&self, f: Option<Arc<GateFn>>) {
        self.hooks.write().unwrap_or_else(|e| e.into_inner()).gate = f;
    }

    pub fn now(&self) -> DateTime {
        let f = self.hooks().now.clone();
        f()
    }
    /// `Engine.Sleep(ms)` (the hook; the Auto controller calls it too).
    pub fn sleep(&self, ms: i32) {
        let f = self.hooks().sleep.clone();
        f(ms)
    }
    fn before_mutate(&self) {
        let f = self.hooks().before_mutate.clone();
        if let Some(f) = f {
            f();
        }
    }
    fn gate_hook(&self, pid: u32, exe: &str) -> Option<String> {
        let f = self.hooks().gate.clone();
        f.and_then(|f| f(pid, exe))
    }
    pub fn has_gate_hook(&self) -> bool {
        self.hooks().gate.is_some()
    }

    pub fn set_calls(&self) -> i64 {
        self.set_calls.load(Ordering::SeqCst)
    }

    fn set(&self, ok: bool) -> bool {
        self.set_calls.fetch_add(1, Ordering::SeqCst);
        ok
    }

    /// The Windows layer (C# `internal INative Native`).
    pub fn native(&self) -> Arc<dyn INative> {
        self.n.clone()
    }

    pub fn advice(&self) -> Vec<String> {
        let _g = self.gate.enter();
        self.st().advice.clone()
    }
    pub fn last_reports(&self) -> Vec<ApplyReport> {
        let _g = self.gate.enter();
        self.st().last_reports.clone()
    }
    /// Threads of the last snapshot per target (read by the Auto planner).
    pub fn last_snap(&self) -> Option<ThreadSnap> {
        self.st().last_snap.clone()
    }
    /// Share of the target threads that could not be opened or changed in the last Apply.
    pub fn last_denied_share(&self) -> f64 {
        self.st().last_denied_share
    }
    pub fn manual_calls(&self) -> i32 {
        self.manual_calls.load(Ordering::SeqCst)
    }
    /// Grows with every revert: layers that cache "already applied" drop their cache.
    pub fn reverts(&self) -> i64 {
        self.reverts.load(Ordering::SeqCst)
    }

    pub fn module_of(sym: &str) -> &str {
        match sym.find('+') {
            Some(i) => &sym[..i],
            None => sym,
        }
    }

    pub fn is_denied_module(r: &ThreadRow) -> bool {
        Guard::is_anti_cheat(Self::module_of(&r.sym))
    }
    fn safe_module(r: &ThreadRow) -> bool {
        !Self::is_denied_module(r)
    }
    pub fn thread_eligible(r: &ThreadRow, prot: bool) -> bool {
        if prot { !Self::is_denied_module(r) } else { Self::safe_module(r) }
    }

    // ---- passes -----------------------------------------------------------------------------------------------

    /// Starts a pass: one toolhelp snapshot of threads and processes for all Applies of the pass.
    pub fn begin_pass(&self) {
        let _g = self.gate.enter();
        let tids = self.n.threads_by_pid();
        let procs = self.n.processes();
        let mut st = self.st();
        st.tid_cache = Some(tids);
        st.pid_cache = Some(procs);
    }

    pub fn end_pass(&self) {
        let _g = self.gate.enter();
        let mut st = self.st();
        st.tid_cache = None;
        st.pid_cache = None;
    }

    pub fn name_of(&self, pid: u32) -> String {
        let _g = self.gate.enter();
        // Outside a pass the list is read anew: a cache left behind would hide ended processes.
        let cached = self.st().pid_cache.as_ref().map(|p| p.get(&pid).map(|x| x.0.clone()).unwrap_or_default());
        match cached {
            Some(n) => n,
            None => self.n.processes().get(&pid).map(|p| p.0.clone()).unwrap_or_default(),
        }
    }

    /// The processes of the current pass (or read now).
    pub fn processes_now(&self) -> Procs {
        let _g = self.gate.enter();
        let cached = self.st().pid_cache.clone();
        cached.unwrap_or_else(|| self.n.processes())
    }

    fn tids_of(&self, pid: u32) -> Vec<u32> {
        let cached = self.st().tid_cache.as_ref().map(|t| t.get(&pid).cloned().unwrap_or_default());
        match cached {
            Some(l) => l,
            None => self.n.threads_by_pid().get(&pid).cloned().unwrap_or_default(),
        }
    }

    // ---- snapshot ---------------------------------------------------------------------------------------------

    /// The threads of a process with their cycle rate, start symbol and name.
    pub fn snapshot(&self, pid: u32) -> ThreadSnap {
        let _g = self.gate.enter();
        self.snapshot_locked(pid)
    }

    fn snapshot_locked(&self, pid: u32) -> ThreadSnap {
        let snap = self.snapshot_core(pid);
        if snap.level >= 0 {
            self.st().snaps.insert(pid, snap.clone());
        }
        snap
    }

    fn snapshot_core(&self, pid: u32) -> ThreadSnap {
        let mut snap = ThreadSnap::default();
        let mut level = 0;
        let ph = self.n.open_process_graceful(pid, &mut level);
        snap.level = level;
        if level < 0 {
            return snap;
        }
        let r = catch_unwind(AssertUnwindSafe(|| {
            let created = self.n.process_created(ph);
            self.st().note_born(pid, created);
            let now = self.now();
            let mut mods: Option<Vec<ModRange>> = None;
            for tid in self.tids_of(pid) {
                let th = self.n.open_thread_of(pid, tid);
                if th.is_null() {
                    continue;
                }
                let r = catch_unwind(AssertUnwindSafe(|| {
                    let k = key(pid, tid);
                    let mut born = 0i64;
                    let mut kernel_user = 0u64;
                    self.n.thread_times(th, &mut born, &mut kernel_user);
                    let mut meta = {
                        let mut st = self.st();
                        let m = st.thr_meta.get(&k).cloned();
                        match m {
                            Some(m) if m.born != born => {
                                st.thr_meta.shift_remove(&k);
                                st.thr_prev.shift_remove(&k);
                                st.thr_before.shift_remove(&k);
                                st.ideal.shift_remove(&k);
                                None
                            }
                            m => m,
                        }
                    };
                    if meta.is_none() {
                        let mut m = ThrMetaE { born, desc: self.n.thread_desc(th), ..Default::default() };
                        if level >= 2 {
                            if mods.is_none() {
                                mods = Some(self.modules_of(pid, ph));
                            }
                            let addr = self.n.thread_start(th);
                            m.sym = self.symbol(addr, mods.as_mut().unwrap(), pid, ph);
                        }
                        meta = Some(m);
                    }
                    let mut meta = meta.unwrap();
                    meta.seen = now;
                    let (sym, desc) = (meta.sym.clone(), meta.desc.clone());
                    self.st().thr_meta.insert(k.clone(), meta);
                    let mut c = 0u64;
                    if !self.n.thread_cycles(th, &mut c) {
                        c = kernel_user;
                        snap.cycle_fallback_tids.push(tid);
                    }
                    let mut rate = 0u64;
                    let mut st = self.st();
                    if let Some(prev) = st.thr_prev.get(&k) {
                        let dt = (now - prev.t).total_seconds();
                        if dt > 0.05 && c >= prev.c {
                            rate = ((c - prev.c) as f64 / dt) as u64;
                        }
                    }
                    st.thr_prev.insert(k, ThrPrevE { c, t: now });
                    drop(st);
                    let mut prio = 0;
                    self.n.get_thread_prio(th, &mut prio);
                    snap.rows.push(ThreadRow { tid, rate, prio, sym, desc, born });
                }));
                self.n.close(th);
                if let Err(p) = r {
                    std::panic::resume_unwind(p);
                }
            }
            // List.Sort by rate, descending (see the report: the C# sort is not stable, this one is)
            snap.rows.sort_by(|a, b| b.rate.cmp(&a.rate));
            snap.ok = true;
        }));
        self.n.close(ph);
        if let Err(p) = r {
            std::panic::resume_unwind(p);
        }
        snap
    }

    fn modules_of(&self, pid: u32, ph: Handle) -> Vec<ModRange> {
        let now = self.now();
        if let Some(c) = self.st().mod_cache.get(&pid) {
            if now - c.t < TimeSpan::from_secs(60.0) {
                return c.m.clone();
            }
        }
        let m = self.n.modules(ph);
        let t = self.now();
        self.st().mod_cache.insert(pid, ModCacheE { m: m.clone(), t });
        m
    }

    fn symbol(&self, addr: u64, mods: &mut Vec<ModRange>, pid: u32, ph: Handle) -> String {
        if addr == 0 {
            return String::new();
        }
        let find = |m: &[ModRange]| -> String {
            for r in m {
                if addr >= r.base && addr < r.base.wrapping_add(r.size) {
                    return format!("{}+0x{:x}", r.name, addr - r.base);
                }
            }
            String::new()
        };
        let mut s = find(mods);
        if !s.is_empty() {
            return s;
        }
        // Not resolved: a module may have loaded since; refresh when the cache is older than 5 s.
        let now = self.now();
        let stale = self.st().mod_cache.get(&pid).map(|c| now - c.t > TimeSpan::from_secs(5.0)).unwrap_or(false);
        if stale {
            self.st().mod_cache.shift_remove(&pid);
            *mods = self.modules_of(pid, ph);
            s = find(mods);
        }
        s
    }

    // ---- matching ---------------------------------------------------------------------------------------------

    /// `\.(dll|exe|sys|drv|ocx|cpl|mui|bin)$` on the trimmed pattern: "module", else "desc".
    pub fn match_for(pattern: &str) -> &'static str {
        let p = pattern.trim();
        let p = p.strip_suffix('\n').unwrap_or(p);
        let is_mod = ["dll", "exe", "sys", "drv", "ocx", "cpl", "mui", "bin"].iter().any(|e| ci::ends_with(p, &format!(".{}", e)));
        if is_mod { "module" } else { "desc" }
    }

    pub fn wildcard(p: &str) -> Wildcard {
        Wildcard::new(p)
    }

    pub fn rule_hits(rule: &ThreadRule, rows: &[ThreadRow]) -> Vec<ThreadRow> {
        let pat = rule.pattern.trim();
        if pat.is_empty() {
            return Vec::new();
        }
        if rule.match_ == "tid" {
            return match parse_uint_none(pat) {
                Some(tid) => rows.iter().filter(|r| r.tid == tid).cloned().collect(),
                None => Vec::new(),
            };
        }
        let re = Self::wildcard(pat);
        let m = if rule.match_.is_empty() { Self::match_for(pat) } else { rule.match_.as_str() };
        let no_ext = !pat.contains('.');
        let mut hits = Vec::new();
        for r in rows {
            let mut hit = false;
            if m == "module" {
                let mut module = Self::module_of(&r.sym);
                if no_ext && module.contains('.') {
                    module = &module[..module.rfind('.').unwrap()];
                }
                hit = !module.is_empty() && re.is_match(module);
            }
            if !hit {
                hit = !r.desc.is_empty() && re.is_match(&r.desc); // fallback to the thread name
            }
            if hit {
                hits.push(r.clone());
            }
        }
        hits
    }

    pub fn filter_tid(hits: Vec<ThreadRow>, tid: &str) -> Vec<ThreadRow> {
        let tid = ci::lower(tid.trim());
        if tid.is_empty() || hits.is_empty() {
            return hits;
        }
        let mut s = hits;
        s.sort_by_key(|r| r.tid);
        if tid == "first" {
            s.truncate(1);
            return s;
        }
        if tid == "last" {
            let n = s.len();
            return s.split_off(n - 1);
        }
        match parse_int_none(&tid) {
            Some(n) if (n as usize) < s.len() => vec![s[n as usize].clone()],
            _ => Vec::new(),
        }
    }

    // ---- baselines --------------------------------------------------------------------------------------------

    /// The process' values before the first change. Everything is read first and stored at the end (a read that
    /// fails hard leaves no half baseline, like the C# exception did).
    fn baseline(&self, pid: u32, ph: Handle, created: i64, name: &str, background: bool) -> ProcBefore {
        if let Some(b) = self.st().proc_before.get(&pid) {
            if b.created == created {
                return b.clone();
            }
        }
        let mut b = ProcBefore { have: true, created, ..Default::default() };
        b.prio_set = self.n.get_priority_class(ph, &mut b.prio);
        b.aff_set = self.n.get_affinity(ph, &mut b.aff);
        b.sets_set = self.n.get_cpu_sets(ph, &mut b.sets);
        b.io_set = self.n.get_io(ph, &mut b.io);
        b.mem_set = self.n.get_mem(ph, &mut b.mem);
        b.eco_set = self.n.get_eco(ph, &mut b.eco_ctl, &mut b.eco_state);
        b.boost_set = self.n.get_boost_disabled(ph, &mut b.boost_disabled);
        let mut st = self.st();
        st.proc_before.insert(pid, b.clone());
        st.proc_name.insert(pid, name.to_string());
        if background {
            st.bg_pids.insert(pid);
        } else {
            st.bg_pids.shift_remove(&pid);
        }
        b
    }

    fn ids_for(t: Option<&Topo>, mask: u64) -> Vec<u32> {
        let Some(t) = t else { return Vec::new() };
        if mask == t.full_mask() {
            return Vec::new();
        }
        let mut v: Vec<u32> = t.sets.iter().filter(|s| s.group == 0 && s.lp < 64 && ((mask >> s.lp) & 1) != 0).map(|s| s.id).collect();
        v.sort();
        v
    }

    // ---- apply ------------------------------------------------------------------------------------------------

    /// The preset path (rules from the store); the mask comes from the options or the rule.
    pub fn apply(&self, pid: u32, preset: &Preset, o: &ApplyOptions, aff_rules: Option<&CiMap<String>>) -> ApplyReport {
        let _g = self.gate.enter();
        let t = TopoBuilder::live();
        let exe = self.name_of(pid);
        let mut mask = 0u64;
        let rep = ApplyReport { exe: exe.clone(), pid: pid as i32, ..Default::default() };
        if let Some(t) = t.as_deref() {
            if o.do_mask {
                let spec = aff_rules.and_then(|r| r.ci_get(&exe).cloned()).unwrap_or_else(|| o.mask_name.clone());
                let mr = Masks::resolve(t, if spec.is_empty() { "auto" } else { &spec });
                let mut st = self.st();
                if !mr.satisfied {
                    for n in &mr.notes {
                        st.advice_once(&format!("mask:{}{}", exe, n), &Masks::note_text(n));
                    }
                }
                drop(st);
                let gm = GameMask::for_rule(t, mr.mask, &GameMask::mode());
                mask = Reservation::unmix(gm.final_, Reservation::reserved(), false);
                let mut st = self.st();
                if !t.split().can_split() {
                    st.advice_once("split", &t.split().why());
                }
                for a in Masks::advice(t, &mr) {
                    st.advice_once(&format!("adv:{}", a), &a);
                }
            }
        }
        self.apply_core(pid, &exe, preset, mask, o, rep)
    }

    /// The Auto path (section 17.2): a synthesized preset and a mask computed by the caller.
    pub fn apply_auto(&self, pid: u32, synthesized: &Preset, mask: u64, o: &ApplyOptions) -> ApplyReport {
        let _g = self.gate.enter();
        let exe = self.name_of(pid);
        let rep = ApplyReport { exe: exe.clone(), pid: pid as i32, ..Default::default() };
        self.apply_core(pid, &exe, synthesized, mask, o, rep)
    }

    fn enable_debug_once(&self) {
        let need = !self.st().debug;
        if need {
            let d = self.n.enable_debug_privilege();
            self.st().debug = d;
        }
    }

    fn apply_core(&self, pid: u32, exe: &str, preset: &Preset, mask: u64, o: &ApplyOptions, mut rep: ApplyReport) -> ApplyReport {
        // 1. Debug privilege (once).
        self.enable_debug_once();
        // 2. Gate.
        if let Some(why) = self.gate_hook(pid, exe) {
            self.st().advice_once(&format!("gate:{}", exe), &why);
            rep.failures.push(why);
            self.report(&rep);
            return rep;
        }
        // 3. Topology.
        let t = TopoBuilder::live();
        let t = t.as_deref();
        if let Some(t) = t {
            if t.groups > 1 {
                self.st().advice_once("groups", L::t("More than one processor group: masks and ideal processors work in group 0 only"));
            }
        }
        // 7. The process.
        let mut level = 0;
        let ph = self.n.open_process_graceful(pid, &mut level);
        rep.handle_level = level;
        if level < 0 {
            rep.failures.push(L::t("The process cannot be opened").to_string());
            self.report(&rep);
            return rep;
        }
        let r = catch_unwind(AssertUnwindSafe(|| -> bool {
            let prot = self.n.is_protected(ph);
            if prot && !o.allow_elevated_priority {
                rep.failures.push(lf!("{0} is a protected process", exe));
                self.report(&rep);
                return false;
            }
            let created = self.n.process_created(ph);
            self.st().note_born(pid, created);
            let fresh = !self.st().proc_before.contains_key(&pid);
            self.baseline(pid, ph, created, exe, false);
            if fresh {
                self.before_mutate();
            }

            if let Some(cls) = Self::prio(&preset.class) {
                if cls != PRIO_REALTIME && cls != PRIO_HIGH {
                    let mut cur = 0;
                    let r = self.n.get_priority_class(ph, &mut cur);
                    self.field(pid, "class", r, cur as u64, cls as u64, || self.n.set_priority_class(ph, cls), &mut rep, L::t("Priority class"));
                }
            }
            if let Some(io) = Self::iop(&preset.io) {
                let mut cur = 0;
                let r = self.n.get_io(ph, &mut cur);
                self.field(pid, "io", r, cur as u64, io as u64, || self.n.set_io(ph, io), &mut rep, L::t("I/O priority"));
            }
            if let Some(mem) = Self::mem(&preset.mem) {
                let mut cur = 5;
                let r = self.n.get_mem(ph, &mut cur);
                self.field(pid, "mem", r, cur as u64, mem as u64, || self.n.set_mem(ph, mem), &mut rep, L::t("Memory priority"));
            }
            let (ectl, estate, eco) = Self::eco_of(&preset.eco);
            if eco {
                let (mut c0, mut s0) = (0u32, 0u32);
                let r = self.n.get_eco(ph, &mut c0, &mut s0);
                self.field(
                    pid,
                    "eco",
                    r,
                    ((c0 as u64) << 32) | s0 as u64,
                    ((ectl as u64) << 32) | estate as u64,
                    || self.n.set_eco(ph, ectl, estate),
                    &mut rep,
                    L::t("EcoQoS"),
                );
            }
            if preset.boost == "on" || preset.boost == "off" {
                let dis = preset.boost == "off";
                let mut cur = false;
                let r = self.n.get_boost_disabled(ph, &mut cur);
                self.field(pid, "boost", r, cur as u64, dis as u64, || self.n.set_boost_disabled(ph, dis), &mut rep, L::t("Priority boost"));
            }
            if Self::FEATURE_AFFINITY && o.do_mask && t.is_some() && mask != 0 {
                if level >= 1 {
                    let mut cur_aff = 0u64;
                    let r = self.n.get_affinity(ph, &mut cur_aff);
                    self.field(pid, "affinity", r, cur_aff, mask, || self.n.set_affinity(ph, mask), &mut rep, L::t("Affinity"));
                }
                let ids = Self::ids_for(t, mask);
                let want = ids_key(&ids);
                let k = format!("{}:sets", pid);
                let mut cur_ids = Vec::new();
                let readable = self.n.get_cpu_sets(ph, &mut cur_ids);
                let differs = {
                    let st = self.st();
                    !st.skip_field.contains(&k)
                        && if readable { ids_key(&cur_ids) != want } else { st.written.get(&k).map(|wh| *wh != str_hash(&want)).unwrap_or(true) }
                };
                if differs {
                    if self.set(self.n.set_cpu_sets(ph, &ids)) {
                        if !readable {
                            self.st().written.insert(k, str_hash(&want));
                        }
                    } else {
                        self.st().skip_field.insert(k);
                        rep.failures.push(lf!("CPU Sets: error {0}", self.n.last_error()));
                    }
                }
                rep.mask_hex = format!("0x{:x}", mask);
                rep.mask_lps = Masks::ranges(mask);
            }
            // 8. Threads.
            if o.do_threads && (preset.threads.iter().any(|r| r.enabled && !r.pattern.trim().is_empty()) || Self::heat_on(preset)) {
                self.apply_threads(pid, exe, preset, mask, o, &mut rep, t, prot);
            }
            true
        }));
        self.n.close(ph);
        match r {
            Ok(true) => {
                self.report(&rep);
                rep
            }
            Ok(false) => rep,
            Err(p) => std::panic::resume_unwind(p),
        }
    }

    /// Writes one process field only when it differs. A value that cannot be read is compared with what was written
    /// before; a field Windows refused is not tried again for this process.
    #[allow(clippy::too_many_arguments)]
    fn field(&self, pid: u32, field: &str, readable: bool, current: u64, want: u64, write: impl FnOnce() -> bool, rep: &mut ApplyReport, what: &str) {
        let k = format!("{}:{}", pid, field);
        {
            let st = self.st();
            if st.unsupported.contains(field) || st.skip_field.contains(&k) {
                return;
            }
            let same = if readable { current == want } else { st.written.get(&k).map(|w| *w == want).unwrap_or(false) };
            if same {
                return;
            }
        }
        if self.set(write()) {
            if !readable {
                self.st().written.insert(k, want);
            }
            return;
        }
        let err = self.n.last_error();
        if err == 1 || err == 50 {
            // ERROR_INVALID_FUNCTION / ERROR_NOT_SUPPORTED: not a failure, the feature is off here
            if self.st().unsupported.insert(field.to_string()) {
                AppLog::add_sev(
                    if field == "eco" {
                        L::t("EcoQoS is off in this Windows (power throttling disabled): the game cannot be throttled anyway, nothing to set").to_string()
                    } else {
                        lf!("{0}: not supported by this Windows, skipped", what)
                    },
                    Sev::Info,
                );
            }
            return;
        }
        self.st().skip_field.insert(k);
        rep.failures.push(lf!("{0}: error {1}", what, err));
    }

    fn eco_of(eco: &str) -> (u32, u32, bool) {
        match ci::lower(eco).as_str() {
            "off" => (1, 0, true),
            "on" => (1, 1, true),
            "default" => (0, 0, true),
            _ => (0, 0, false),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn apply_threads(&self, pid: u32, exe: &str, preset: &Preset, mask: u64, o: &ApplyOptions, rep: &mut ApplyReport, t: Option<&Topo>, prot: bool) {
        let cand: Vec<CpuSet> = match t {
            Some(t) => Masks::ideal_candidates(t, false, mask),
            None => Vec::new(),
        };
        if o.warmup {
            self.snapshot_locked(pid);
            self.sleep(700);
        }
        let snap = self.snapshot_locked(pid);
        self.st().last_snap = Some(snap.clone());
        rep.last_threads = snap.rows.len() as i32;
        rep.cycle_fallback_tids.extend(snap.cycle_fallback_tids.iter().copied());
        if let Some(ac) = snap.rows.iter().find(|r| Self::is_denied_module(r)) {
            self.st().advice_once(&format!("ac:{}", exe), &lf!("{0} has anti-cheat threads ({1}): they are not touched", exe, Self::module_of(&ac.sym)));
        }
        let alive: HashSet<String> = snap.rows.iter().map(|r| key(pid, r.tid)).collect();
        let mut used: HashSet<u32> = self.st().ideal.iter().filter(|(k, _)| alive.contains(*k)).map(|(_, v)| *v).collect();
        let mut ci_: usize = 0;
        let mut next = |used: &mut HashSet<u32>| -> u32 {
            for i in 0..cand.len() {
                let lp = cand[(ci_ + i) % cand.len()].lp as u32;
                if used.contains(&lp) {
                    continue;
                }
                used.insert(lp);
                ci_ += i + 1;
                return lp;
            }
            let lp = cand[ci_ % cand.len()].lp as u32;
            ci_ += 1;
            lp
        };
        let (mut targets, mut denied) = (0i32, 0i32);
        let mut hot: HashSet<u32> = HashSet::new();
        for rule in preset.threads.iter().filter(|r| r.enabled && !r.pattern.trim().is_empty()) {
            for row in Self::filter_tid(Self::rule_hits(rule, &snap.rows), &rule.tid) {
                if !Self::thread_eligible(&row, prot) {
                    continue;
                }
                targets += 1;
                let k = key(pid, row.tid);
                let th = self.n.open_thread_of(pid, row.tid);
                if th.is_null() {
                    denied += 1;
                    rep.failures.push(lf!("Thread {0} cannot be opened", row.tid));
                    continue;
                }
                let r = catch_unwind(AssertUnwindSafe(|| {
                    let (mut born, mut ku) = (0i64, 0u64);
                    self.n.thread_times(th, &mut born, &mut ku);
                    if born != row.born {
                        return; // the ID was reused since the snapshot
                    }
                    rep.named_hits += 1;
                    hot.insert(row.tid);
                    self.thread_baseline(th, &k, born);
                    let mut failed = false;
                    let want = Self::tprio(&rule.prio)
                        .or_else(|| parse_int_leading_sign(&rule.prio).map(|v| v as i32).filter(|v| Self::tprio_contains_value(*v)));
                    if let Some(want) = want {
                        let mut cur = 0;
                        if want != 15 && (!self.n.get_thread_prio(th, &mut cur) || cur != want) && !self.set(self.n.set_thread_prio(th, want)) {
                            failed = true;
                            rep.failures.push(lf!("Thread {0} priority: error {1}", row.tid, self.n.last_error()));
                        }
                    }
                    if rule.boost == "on" || rule.boost == "off" {
                        let dis = rule.boost == "off";
                        let mut cur = false;
                        if (!self.n.get_thread_boost_disabled(th, &mut cur) || cur != dis) && !self.set(self.n.set_thread_boost_disabled(th, dis)) {
                            failed = true;
                            rep.failures.push(lf!("Thread {0} boost: error {1}", row.tid, self.n.last_error()));
                        }
                    }
                    if let Some(t) = t {
                        if !cand.is_empty() && !rule.ideal.is_empty() {
                            if rule.ideal == "auto" {
                                if !self.st().ideal.contains_key(&k) {
                                    let lp = next(&mut used);
                                    self.st().ideal.insert(k.clone(), lp);
                                    if !self.set(self.n.set_thread_ideal(th, lp)) {
                                        failed = true;
                                        self.st().ideal.shift_remove(&k);
                                        rep.failures.push(lf!("Thread {0} ideal processor: error {1}", row.tid, self.n.last_error()));
                                    }
                                }
                            } else if let Some(n) = parse_int_none(&rule.ideal).filter(|n| *n >= 0 && *n < t.logical_count) {
                                let old = self.st().ideal.get(&k).copied();
                                if old != Some(n as u32) {
                                    self.st().ideal.insert(k.clone(), n as u32);
                                    if !self.set(self.n.set_thread_ideal(th, n as u32)) {
                                        failed = true;
                                        self.st().ideal.shift_remove(&k);
                                        rep.failures.push(lf!("Thread {0} ideal processor: error {1}", row.tid, self.n.last_error()));
                                    }
                                }
                            }
                        }
                    }
                    if let Some(t) = t {
                        if !rule.ideal.is_empty() {
                            let ilp = self.st().ideal.get(&k).copied();
                            if let Some(ilp) = ilp {
                                self.thread_sets(pid, exe, th, &k, t, ilp, rule.pin);
                            }
                        }
                    }
                    if failed && self.n.last_error() == 5 {
                        denied += 1;
                    }
                }));
                self.n.close(th);
                if let Err(p) = r {
                    std::panic::resume_unwind(p);
                }
            }
        }
        // Heat policy (section 22.6): the hottest matched thread gets HotFirst, the next ones HotOthers.
        if let Some(t) = t {
            if Self::heat_on(preset) && !cand.is_empty() {
                let any_rule = preset.threads.iter().any(|r| r.enabled && !r.pattern.trim().is_empty());
                let mut ranked: Vec<&ThreadRow> =
                    snap.rows.iter().filter(|r| r.rate > 0 && Self::thread_eligible(r, prot) && (!any_rule || hot.contains(&r.tid))).collect();
                ranked.sort_by(|a, b| b.rate.cmp(&a.rate));
                let budget = (cand.len() as i32 - AutoPolicy::OS_RESERVED_CORES).max(1) as usize;
                let slots: Vec<Classified> =
                    ranked.iter().take(budget).map(|r| Classified { row: (*r).clone(), role: AutoLogic::classify(r, &snap), ..Default::default() }).collect();
                let sure = AutoLogic::confidence(&snap, &slots) >= AutoPolicy::CONFIDENCE_FLOOR;
                for (i, slot) in slots.iter().enumerate() {
                    let policy = if i == 0 { preset.hot_first.as_str() } else { preset.hot_others.as_str() };
                    if policy != "pin" && policy != "ideal" {
                        continue;
                    }
                    let row = &slot.row;
                    let k = key(pid, row.tid);
                    let th = self.n.open_thread_of(pid, row.tid);
                    if th.is_null() {
                        continue;
                    }
                    let r = catch_unwind(AssertUnwindSafe(|| {
                        let (mut born, mut ku) = (0i64, 0u64);
                        self.n.thread_times(th, &mut born, &mut ku);
                        if born != row.born {
                            return;
                        }
                        self.thread_baseline(th, &k, born);
                        let had = self.st().ideal.get(&k).copied();
                        let lp = match had {
                            Some(lp) => lp,
                            None => {
                                let lp = next(&mut used);
                                self.st().ideal.insert(k.clone(), lp);
                                if !self.set(self.n.set_thread_ideal(th, lp)) {
                                    self.st().ideal.shift_remove(&k);
                                    rep.failures.push(lf!("Thread {0} ideal processor: error {1}", row.tid, self.n.last_error()));
                                    return;
                                }
                                lp
                            }
                        };
                        let role = matches!(slot.role, ThreadRole::Main | ThreadRole::Render | ThreadRole::Audio | ThreadRole::Worker);
                        self.thread_sets(pid, exe, th, &k, t, lp, policy == "pin" && sure && role);
                    }));
                    self.n.close(th);
                    if let Err(p) = r {
                        std::panic::resume_unwind(p);
                    }
                }
            }
        }
        let mut st = self.st();
        st.preset_tid_count.insert(key(pid, 0), targets);
        st.last_denied_share = if targets == 0 { 0.0 } else { denied as f64 / targets as f64 };
    }

    fn heat_on(p: &Preset) -> bool {
        matches!(p.hot_first.as_str(), "pin" | "ideal") || matches!(p.hot_others.as_str(), "pin" | "ideal")
    }

    /// What the thread had before the first change: priority, boost, selected CPU Sets.
    fn thread_baseline(&self, th: Handle, k: &str, born: i64) {
        // 1.17.1: a baseline of the all-threads layer holds the original ideal processor only; when Top-5 (or a rule)
        // takes the thread, the rest is read now (the layer never changed it) and the original ideal is kept.
        let ideal_only = self.st().thr_before.get(k).map(|tb| (tb.ideal_only, tb.ideal, tb.ideal_set));
        if let Some((false, _, _)) = ideal_only {
            return;
        }
        let mut tb = ThrBefore { born, ..Default::default() };
        if let Some((true, ideal, ideal_set)) = ideal_only {
            tb.prio_set = self.n.get_thread_prio(th, &mut tb.prio);
            tb.boost_set = self.n.get_thread_boost_disabled(th, &mut tb.boost_disabled);
            tb.sets_set = self.n.get_thread_cpu_sets(th, &mut tb.sets);
            tb.ideal = ideal;
            tb.ideal_set = ideal_set;
            self.st().thr_before.insert(k.to_string(), tb);
            self.before_mutate(); // the priority baseline goes into the session before Top-5 writes one
            return;
        }
        tb.prio_set = self.n.get_thread_prio(th, &mut tb.prio);
        tb.boost_set = self.n.get_thread_boost_disabled(th, &mut tb.boost_disabled);
        tb.sets_set = self.n.get_thread_cpu_sets(th, &mut tb.sets);
        tb.ideal_set = self.n.get_thread_ideal(th, &mut tb.ideal) && tb.ideal < 64; // kept in the session too: a crash restore puts it back
        self.st().thr_before.insert(k.to_string(), tb);
        self.before_mutate();
    }

    /// Section 22.4: limits a pinned thread to the CPU Set of its ideal processor (pin = true) or gives back what it
    /// had (pin = false). A refused call degrades to "ideal only" with one warning; it is not a failure of the pin.
    #[allow(clippy::too_many_arguments)]
    fn thread_sets(&self, pid: u32, exe: &str, th: Handle, k: &str, t: &Topo, lp: u32, pin: bool) {
        if !pin {
            let restore = {
                let mut st = self.st();
                if st.thr_sets.shift_remove(k).is_some() {
                    st.thr_before.get(k).filter(|tb| tb.sets_set).map(|tb| tb.sets.clone())
                } else {
                    None
                }
            };
            if let Some(sets) = restore {
                self.set(self.n.set_thread_cpu_sets(th, &sets));
            }
            return;
        }
        let refused = format!("{}:tsets", k);
        {
            let st = self.st();
            if st.skip_field.contains(&refused) || st.thr_sets.get(k) == Some(&lp) {
                return;
            }
        }
        let sets: Vec<u32> = t.sets.iter().filter(|s| s.group == 0 && s.lp as u32 == lp).map(|s| s.id).take(1).collect();
        if sets.len() == 1 && self.set(self.n.set_thread_cpu_sets(th, &sets)) {
            self.st().thr_sets.insert(k.to_string(), lp);
            return;
        }
        let first = {
            let mut st = self.st();
            st.skip_field.insert(refused);
            st.warn_once.insert(format!("tsets:{}", pid))
        };
        if first {
            AppLog::add_sev(lf!("{0}: thread CPU Sets were refused (error {1}); the threads keep the ideal processor only", exe, self.n.last_error()), Sev::Warn);
        }
    }

    // ---- dynamic placement (the Top-5 mode): the caller keeps the thread handle open -------------------------------

    /// The threads of a process from the current pass (or read now).
    pub fn threads_now(&self, pid: u32) -> Vec<u32> {
        let _g = self.gate.enter();
        self.tids_of(pid)
    }

    /// Why the process may not be touched (denylist); None = it may.
    pub fn gate(&self, pid: u32) -> Option<String> {
        let _g = self.gate.enter();
        let name = self.name_of(pid);
        self.gate_hook(pid, &name)
    }

    /// Puts a thread on a processor (ideal, a hint) with a priority; prio = None gives the thread's own priority
    /// back. The baseline (priority, boost, ideal) is taken before the first change. Writes only what differs. Never
    /// above "highest".
    pub fn place_thread(&self, pid: u32, tid: u32, th: Handle, born: i64, prio: Option<i32>, lp: u32) -> bool {
        let _g = self.gate.enter();
        let k = key(pid, tid);
        {
            let mut st = self.st();
            if st.thr_before.get(&k).map(|o| o.born != born).unwrap_or(false) {
                st.thr_before.shift_remove(&k);
            }
        }
        self.thread_baseline(th, &k, born);
        let tb = {
            let need = !self.st().thr_before.get(&k).map(|t| t.ideal_set).unwrap_or(false);
            if need {
                let mut ideal = 0u32;
                let set = self.n.get_thread_ideal(th, &mut ideal);
                let mut st = self.st();
                if let Some(tb) = st.thr_before.get_mut(&k) {
                    tb.ideal = ideal;
                    tb.ideal_set = set;
                }
            }
            self.st().thr_before.get(&k).cloned().unwrap_or_default()
        };
        let mut ok = true;
        let want = match prio {
            Some(p) => p.clamp(-2, 2),
            None => tb.prio,
        };
        let mut cur = 0;
        if (prio.is_some() || tb.prio_set) && (!self.n.get_thread_prio(th, &mut cur) || cur != want) {
            ok &= self.set(self.n.set_thread_prio(th, want));
        }
        let had = self.st().ideal.get(&k).copied();
        if had != Some(lp) {
            if self.set(self.n.set_thread_ideal(th, lp)) {
                self.st().ideal.insert(k, lp);
            } else {
                ok = false;
            }
        }
        ok
    }

    /// Gives a thread back what it had before PlaceThread; returns how many values were written.
    pub fn release_thread(&self, pid: u32, tid: u32, th: Handle) -> i32 {
        let _g = self.gate.enter();
        let k = key(pid, tid);
        let tb = {
            let mut st = self.st();
            st.ideal.shift_remove(&k);
            match st.thr_before.get(&k) {
                None => return 0,
                Some(tb) => tb.clone(),
            }
        };
        let mut n = 0;
        let (mut born, mut ku) = (0i64, 0u64);
        self.n.thread_times(th, &mut born, &mut ku);
        if born == tb.born {
            let mut cur = 0;
            if tb.prio_set && (!self.n.get_thread_prio(th, &mut cur) || cur != tb.prio) && self.set(self.n.set_thread_prio(th, tb.prio)) {
                n += 1;
            }
            if tb.ideal_set && self.set(self.n.set_thread_ideal(th, tb.ideal)) {
                n += 1;
            }
        }
        self.st().thr_before.shift_remove(&k);
        n
    }

    // ---- 1.17.1: ideal processors of the other game threads (Dynamic Top-5's all-threads layer) ------------------

    /// Sets only the ideal processor (SetThreadIdealProcessorEx: no affinity, no CPU Sets, no priority) of several
    /// threads of one process. A thread whose original ideal processor cannot be read (or is outside group 0) is
    /// never written: it could not be put back. A thread with a baseline of another kind (Top-5 or a rule holds it)
    /// is skipped. The new baselines go into the crash-safe session once, before the first write. `ok[i]` tells
    /// whether item i now has its processor; returns the number of writes.
    pub fn place_ideals(&self, pid: u32, items: &[(u32, Handle, i64, u32)], ok: &mut Vec<bool>) -> i32 {
        let _g = self.gate.enter();
        ok.clear();
        ok.resize(items.len(), false);
        let mut fresh = false;
        let mut can = Vec::with_capacity(items.len());
        for &(tid, th, born, _) in items {
            let k = key(pid, tid);
            let mut st = self.st();
            if st.thr_before.get(&k).map(|o| o.born != born).unwrap_or(false) {
                st.thr_before.shift_remove(&k);
                st.ideal.shift_remove(&k);
            }
            match st.thr_before.get(&k) {
                Some(tb) => {
                    can.push(tb.ideal_only && tb.ideal_set);
                }
                None => {
                    drop(st);
                    let mut ideal = 0u32;
                    let read = self.n.get_thread_ideal(th, &mut ideal) && ideal < 64;
                    if read {
                        self.st().thr_before.insert(k, ThrBefore { born, ideal, ideal_set: true, ideal_only: true, ..Default::default() });
                        fresh = true;
                    }
                    can.push(read);
                }
            }
        }
        // The session lists a thread's ideal processor only once the program holds a value for it, so the values
        // about to be written are entered first (a crash between the save and the write only puts back an original
        // that is still in place) and taken out again when Windows refuses.
        let mut had: Vec<Option<Option<u32>>> = Vec::with_capacity(items.len());
        for (i, &(tid, _, _, lp)) in items.iter().enumerate() {
            if !can[i] {
                had.push(None);
                continue;
            }
            let k = key(pid, tid);
            let mut st = self.st();
            let old = st.ideal.get(&k).copied();
            if old == Some(lp) {
                ok[i] = true;
                had.push(None);
                continue;
            }
            fresh |= old.is_none();
            st.ideal.insert(k, lp);
            had.push(Some(old));
        }
        if fresh {
            self.before_mutate();
        }
        let mut n = 0;
        for (i, &(tid, th, _, lp)) in items.iter().enumerate() {
            let Some(old) = had[i] else { continue };
            n += 1;
            if self.set(self.n.set_thread_ideal(th, lp)) {
                ok[i] = true;
            } else {
                let k = key(pid, tid);
                let mut st = self.st();
                match old {
                    Some(v) => st.ideal.insert(k, v),
                    None => st.ideal.shift_remove(&k),
                };
            }
        }
        n
    }

    /// Gives back the original ideal processor of a thread placed by `place_ideals`, only while the thread still has
    /// the value the program set (someone else's later choice is left alone). Returns true when a value was written.
    pub fn release_ideal(&self, pid: u32, tid: u32, th: Handle) -> bool {
        let _g = self.gate.enter();
        let k = key(pid, tid);
        let (tb, ours) = {
            let mut st = self.st();
            match st.thr_before.get(&k) {
                Some(tb) if tb.ideal_only => {}
                _ => return false,
            }
            let tb = st.thr_before.shift_remove(&k).unwrap_or_default();
            (tb, st.ideal.shift_remove(&k))
        };
        let Some(ours) = ours else { return false };
        let (mut born, mut ku) = (0i64, 0u64);
        self.n.thread_times(th, &mut born, &mut ku);
        let mut cur = 0u32;
        born == tb.born && tb.ideal_set && self.n.get_thread_ideal(th, &mut cur) && cur == ours && self.set(self.n.set_thread_ideal(th, tb.ideal))
    }

    /// Forgets a thread placed by `place_ideals` without writing anything (its ideal processor was changed by
    /// someone else: that value stays).
    pub fn forget_ideal(&self, pid: u32, tid: u32) {
        let _g = self.gate.enter();
        let k = key(pid, tid);
        let mut st = self.st();
        if st.thr_before.get(&k).map(|tb| tb.ideal_only).unwrap_or(false) {
            st.thr_before.shift_remove(&k);
            st.ideal.shift_remove(&k);
        }
    }

    /// Threads limited to one CPU Set right now ("pid:tid" -> processor).
    pub fn thread_pins(&self) -> IndexMap<String, u32> {
        let _g = self.gate.enter();
        self.st().thr_sets.clone()
    }

    // ---- timer resolution (section 22.5) ----------------------------------------------------------------------

    pub fn timer_requested(&self) -> bool {
        let _g = self.gate.enter();
        self.st().timer_requested
    }

    /// Asks for the finest system timer resolution while a game is active. False when refused or unavailable.
    pub fn timer_request(&self) -> bool {
        let _g = self.gate.enter();
        if self.st().timer_requested {
            return true;
        }
        if Flags::refuse(Flags::TIMER_RES, L::t("Timer resolution")) {
            return false;
        }
        let (mut coarsest, mut finest, mut current) = (0u32, 0u32, 0u32);
        if !self.n.timer_query(&mut coarsest, &mut finest, &mut current) {
            AppLog::add_sev(L::t("Timer resolution: Windows did not answer, left as it is"), Sev::Warn);
            return false;
        }
        {
            let mut st = self.st();
            st.timer_requested = true; // written to the session before the request
            st.timer_value = finest;
        }
        self.before_mutate();
        let mut actual = 0u32;
        if !self.n.timer_set(finest, true, &mut actual) {
            self.st().timer_requested = false;
            AppLog::add_sev(L::t("Timer resolution: the request was refused"), Sev::Warn);
            return false;
        }
        AppLog::add(lf!("Timer resolution requested: {0:0.00} ms (was {1:0.00} ms)", actual as f64 / 10000.0, current as f64 / 10000.0));
        true
    }

    /// Gives the timer request back (revert, fault, game exit). Idempotent.
    pub fn timer_release(&self) -> bool {
        let _g = self.gate.enter();
        let value = {
            let mut st = self.st();
            if !st.timer_requested {
                return false;
            }
            st.timer_requested = false;
            st.timer_value
        };
        let mut actual = 0u32;
        let ok = self.n.timer_set(value, false, &mut actual);
        if ok {
            AppLog::add_sev(lf!("Timer resolution released: {0:0.00} ms now", actual as f64 / 10000.0), Sev::Info);
        } else {
            AppLog::add_sev(L::t("Timer resolution: the release was refused"), Sev::Warn);
        }
        ok
    }

    fn last_snap_of(&self, pid: u32) -> Option<ThreadSnap> {
        self.st().snaps.get(&pid).cloned()
    }

    fn report(&self, rep: &ApplyReport) {
        let mut st = self.st();
        for f in &rep.failures {
            // a process Windows does not let us change (protected, system): expected, it is skipped; a note in the log
            // file, not an error window
            let line = format!("{}: {}", rep.exe, f);
            if st.failure_once.insert(line.clone()) {
                AppLog::add_sev(line, Sev::Warn);
            }
        }
        st.last_reports.retain(|r| r.pid != rep.pid);
        st.last_reports.push(rep.clone());
        if st.last_reports.len() > 32 {
            st.last_reports.remove(0);
        }
    }

    // ---- all targets of one pass (section 10) -------------------------------------------------------------------

    /// One toolhelp snapshot and one pid -> name cache for the whole pass; the caller picks the targets from that
    /// snapshot (watchlist presets, then system presets), each goes through Apply with the per-game mask rules; the
    /// caches are dropped in the end whatever happens.
    pub fn apply_all(&self, pick: &mut dyn FnMut(&Procs, &Tids) -> Vec<Target>, aff_rules: Option<&CiMap<String>>) -> Vec<ApplyReport> {
        let mut reports = Vec::new();
        let own;
        {
            let _g = self.gate.enter();
            own = self.st().tid_cache.is_none();
            if own {
                let tids = self.n.threads_by_pid();
                let procs = self.n.processes();
                let mut st = self.st();
                st.tid_cache = Some(tids);
                st.pid_cache = Some(procs);
            }
        }
        struct EndGuard<'a>(&'a Engine, bool);
        impl Drop for EndGuard<'_> {
            fn drop(&mut self) {
                if self.1 {
                    self.0.end_pass();
                }
            }
        }
        let _end = EndGuard(self, own);
        let targets = {
            let _g = self.gate.enter();
            let (procs, tids) = {
                let st = self.st();
                (st.pid_cache.clone().unwrap_or_default(), st.tid_cache.clone().unwrap_or_default())
            };
            pick(&procs, &tids)
        };
        for t in &targets {
            reports.push(self.apply(t.pid, &t.preset, &t.options, if t.options.do_aff_rules { aff_rules } else { None }));
        }
        reports
    }

    pub fn read_process(&self, pid: u32, exe: &str) -> ProcInfo {
        let _g = self.gate.enter();
        let mut info = ProcInfo { level: -1, ..Default::default() };
        if self.gate_hook(pid, exe).is_some() {
            info.denied = true;
            return info;
        }
        let mut level = 0;
        let ph = self.n.open_process_graceful(pid, &mut level);
        info.level = level;
        if level < 0 {
            return info;
        }
        let r = catch_unwind(AssertUnwindSafe(|| {
            info.ok = self.n.get_priority_class(ph, &mut info.class);
            self.n.get_affinity(ph, &mut info.affinity);
            self.n.get_boost_disabled(ph, &mut info.boost_off);
            info.changed = self.st().proc_before.contains_key(&pid);
        }));
        self.n.close(ph);
        if let Err(p) = r {
            std::panic::resume_unwind(p);
        }
        info
    }

    /// Process ids with a saved baseline (changed by the program).
    pub fn changed_pids(&self) -> IndexSet<u32> {
        let _g = self.gate.enter();
        let st = self.st();
        st.proc_before.keys().copied().chain(st.thr_before.keys().map(|k| split_key(k).0)).collect()
    }

    /// Removes the ideal-processor pins of a process (they stay as hints in Windows; the allocator forgets them).
    pub fn drop_pins(&self, pid: u32) {
        let _g = self.gate.enter();
        let prefix = format!("{}:", pid);
        let keys: Vec<String> = self.st().thr_sets.keys().filter(|k| k.starts_with(&prefix)).cloned().collect();
        for k in keys {
            let tb = {
                let mut st = self.st();
                st.thr_sets.shift_remove(&k);
                st.thr_before.get(&k).filter(|tb| tb.sets_set).cloned()
            };
            let Some(tb) = tb else { continue };
            let th = self.n.open_thread_of(pid, k[prefix.len()..].parse().unwrap_or(0));
            if th.is_null() {
                continue;
            }
            let r = catch_unwind(AssertUnwindSafe(|| {
                let (mut born, mut ku) = (0i64, 0u64);
                self.n.thread_times(th, &mut born, &mut ku);
                if born == tb.born {
                    self.set(self.n.set_thread_cpu_sets(th, &tb.sets));
                }
            }));
            self.n.close(th);
            if let Err(p) = r {
                std::panic::resume_unwind(p);
            }
        }
        self.st().ideal.retain(|k, _| !k.starts_with(&prefix));
    }

    pub fn ideals(&self) -> IndexMap<String, u32> {
        let _g = self.gate.enter();
        self.st().ideal.clone()
    }

    // ---- manual API (section 21): escape hatch for debugging, never called by Auto --------------------------------

    /// Sets a thread's priority, ideal processor and boost by hand (baseline kept, birth checked).
    pub fn thread_manual(&self, pid: i32, tid: u32, prio: &str, ideal: &str, boost: &str) -> String {
        let _g = self.gate.enter();
        self.manual_calls.fetch_add(1, Ordering::SeqCst);
        let upid = pid as u32;
        let name = self.name_of(upid);
        if let Some(denied) = self.gate_hook(upid, &name) {
            return denied;
        }
        let known = self.last_snap_of(upid).and_then(|s| s.rows.into_iter().find(|r| r.tid == tid));
        if let Some(k) = known {
            if Self::is_denied_module(&k) {
                return lf!("Thread {0} belongs to a denylisted module: not changed", tid);
            }
        }
        let th = self.n.open_thread_of(upid, tid);
        if th.is_null() {
            return lf!("Thread {0} of PID {1} cannot be opened", tid, pid);
        }
        let r = catch_unwind(AssertUnwindSafe(|| -> String {
            let k = key(upid, tid);
            let (mut born, mut ku) = (0i64, 0u64);
            self.n.thread_times(th, &mut born, &mut ku);
            {
                let mut st = self.st();
                if st.thr_before.get(&k).map(|o| o.born != born).unwrap_or(false) {
                    st.thr_before.shift_remove(&k);
                }
            }
            self.thread_baseline(th, &k, born);
            let mut done: Vec<String> = Vec::new();
            if !prio.is_empty() {
                let p = Self::tprio(prio).or_else(|| parse_int_leading_sign(prio).map(|v| v as i32).filter(|v| Self::tprio_contains_value(*v)));
                let p = match p {
                    Some(p) if p != 15 => p,
                    _ => return lf!("Priority {0} is not allowed", prio),
                };
                let mut cur = 0;
                if !self.n.get_thread_prio(th, &mut cur) || cur != p {
                    done.push(if self.set(self.n.set_thread_prio(th, p)) { lf!("priority {0}", p) } else { lf!("priority: error {0}", self.n.last_error()) });
                }
            }
            if boost == "on" || boost == "off" {
                let dis = boost == "off";
                let mut cur = false;
                if !self.n.get_thread_boost_disabled(th, &mut cur) || cur != dis {
                    done.push(if self.set(self.n.set_thread_boost_disabled(th, dis)) { lf!("boost {0}", boost) } else { lf!("boost: error {0}", self.n.last_error()) });
                }
            }
            if !ideal.is_empty() {
                let t = TopoBuilder::live();
                let n = match (parse_int_none(ideal), t) {
                    (Some(n), Some(t)) if n < t.logical_count => n,
                    _ => return lf!("Ideal processor {0} is not valid", ideal),
                };
                let o = self.st().ideal.get(&k).copied();
                if o != Some(n as u32) {
                    self.st().ideal.insert(k.clone(), n as u32);
                    done.push(if self.set(self.n.set_thread_ideal(th, n as u32)) { lf!("ideal processor {0}", n) } else { lf!("ideal: error {0}", self.n.last_error()) });
                }
            }
            if done.is_empty() { lf!("Thread {0}: nothing to change", tid) } else { lf!("Thread {0}: {1}", tid, done.join(", ")) }
        }));
        self.n.close(th);
        match r {
            Ok(s) => s,
            Err(p) => std::panic::resume_unwind(p),
        }
    }

    fn manual_process(&self, pid: i32, log: Option<&mut Vec<String>>, act: &dyn Fn(Handle) -> Option<String>) -> bool {
        let _g = self.gate.enter();
        self.manual_calls.fetch_add(1, Ordering::SeqCst);
        let upid = pid as u32;
        let exe = self.name_of(upid);
        let mut log = log;
        if let Some(denied) = self.gate_hook(upid, &exe) {
            if let Some(l) = log.as_deref_mut() {
                l.push(denied);
            }
            return false;
        }
        let mut level = 0;
        let ph = self.n.open_process_graceful(upid, &mut level);
        if level < 0 {
            if let Some(l) = log.as_deref_mut() {
                l.push(lf!("PID {0} cannot be opened", pid));
            }
            return false;
        }
        let r = catch_unwind(AssertUnwindSafe(|| -> bool {
            if self.n.is_protected(ph) {
                if let Some(l) = log.as_deref_mut() {
                    l.push(lf!("{0} is a protected process", &exe));
                }
                return false;
            }
            let created = self.n.process_created(ph);
            self.st().note_born(upid, created);
            let fresh = !self.st().proc_before.contains_key(&upid);
            self.baseline(upid, ph, created, &exe, false);
            if fresh {
                self.before_mutate();
            }
            let err = act(ph);
            let ok = err.is_none();
            if let Some(l) = log.as_deref_mut() {
                l.push(err.unwrap_or_else(|| lf!("{0}: done", &exe)));
            }
            ok
        }));
        self.n.close(ph);
        match r {
            Ok(b) => b,
            Err(p) => std::panic::resume_unwind(p),
        }
    }

    /// Sets a process' priority class by hand (never realtime).
    pub fn process_class_manual(&self, pid: i32, cls: &str, log: Option<&mut Vec<String>>) -> bool {
        self.manual_process(pid, log, &|ph| {
            let want = match Self::prio(cls) {
                Some(w) if w != PRIO_REALTIME => w,
                _ => return Some(lf!("Class {0} is not allowed", cls)),
            };
            let mut cur = 0;
            if self.n.get_priority_class(ph, &mut cur) && cur == want {
                return None;
            }
            if self.set(self.n.set_priority_class(ph, want)) { None } else { Some(lf!("Class: error {0}", self.n.last_error())) }
        })
    }

    /// Turns a process' dynamic priority boost off (or back on) by hand.
    pub fn process_boost(&self, pid: i32, off: bool, log: Option<&mut Vec<String>>) -> bool {
        self.manual_process(pid, log, &|ph| {
            let mut cur = false;
            if self.n.get_boost_disabled(ph, &mut cur) && cur == off {
                return None;
            }
            if self.set(self.n.set_boost_disabled(ph, off)) { None } else { Some(lf!("Boost: error {0}", self.n.last_error())) }
        })
    }

    // ---- background -------------------------------------------------------------------------------------------

    /// Lowers background programs (section 12.8): baselines of all of them first (then the session is saved), then
    /// BelowNormal (only lowering), I/O low (never "very low": that starves a program's disk reads behind the game's),
    /// memory low, EcoQoS on and, when the mask does not mix with the reserved processors, the background mask.
    /// Returns how many processes were changed. A process that makes a call fail hard (an exception in C#, a panic
    /// here) is skipped and logged once; the others are still handled.
    pub fn apply_background(&self, pids: &[u32], bg_mask: u64, do_affinity: bool, timer_only: Option<&[u32]>) -> i32 {
        let _g = self.gate.enter();
        self.enable_debug_once();
        let res = Reservation::reserved();
        let bg = if bg_mask != 0 { Reservation::unmix(bg_mask, res, true) } else { 0 };
        let aff_ok = do_affinity && bg != 0 && !Reservation::mixes(bg, res);
        let mut open: Vec<(u32, Handle, i32)> = Vec::new();
        let names: Procs = {
            let c = self.st().pid_cache.clone();
            c.unwrap_or_else(|| self.n.processes())
        };
        let name_of = |pid: u32| names.get(&pid).map(|p| p.0.clone()).unwrap_or_default();
        let mut added = false;
        for &pid in pids {
            let exe = name_of(pid);
            let mut h = Handle::NULL;
            let r = catch_unwind(AssertUnwindSafe(|| {
                if BackgroundDeny::contains(&exe) || self.gate_hook(pid, &exe).is_some() || Guard::is_anti_cheat_process(&exe) {
                    return;
                }
                let mut level = 0;
                h = self.n.open_process_graceful(pid, &mut level);
                if level < 0 {
                    return;
                }
                if self.n.is_protected(h) {
                    self.n.close(h);
                    return;
                }
                let created = self.n.process_created(h);
                self.st().note_born(pid, created);
                if !self.st().proc_before.contains_key(&pid) {
                    added = true;
                }
                self.baseline(pid, h, created, &exe, true);
                open.push((pid, h, level));
            }));
            if let Err(p) = r {
                // One odd process must never stop the background layer (or Auto with it).
                if !h.is_null() && !open.iter().any(|o| o.1 == h) {
                    self.n.close(h);
                }
                self.bg_skip(&exe, &AppLog::describe_panic(p.as_ref()));
            }
        }
        if added {
            self.before_mutate();
        }
        let mut changed = 0;
        let timer_guard = self.timer_guard();
        for &(pid, h, level) in &open {
            let r = catch_unwind(AssertUnwindSafe(|| {
                let before = self.set_calls();
                let mut cls = 0;
                if self.n.get_priority_class(h, &mut cls) && (cls == PRIO_NORMAL || cls == PRIO_ABOVE || cls == PRIO_HIGH) {
                    self.set(self.n.set_priority_class(h, PRIO_BELOW));
                }
                let mut io = 0;
                if self.n.get_io(h, &mut io) && io > IOP_LOW {
                    self.set(self.n.set_io(h, IOP_LOW));
                }
                let mut mem = 5;
                if self.n.get_mem(h, &mut mem) && mem > MEM_LOW {
                    self.set(self.n.set_mem(h, MEM_LOW));
                }
                let want = if timer_guard && !Self::keeps_timer(&name_of(pid)) { ECO_AND_TIMER } else { ECO_ONLY };
                let (mut c, mut s) = (0u32, 0u32);
                if !self.n.get_eco(h, &mut c, &mut s) || c != want || s != want {
                    self.set_eco_guarded(h, want, u32::MAX);
                }
                let mut aff = 0u64;
                if aff_ok && level >= 1 && self.n.get_affinity(h, &mut aff) && aff != bg {
                    self.set(self.n.set_affinity(h, bg));
                }
                if self.set_calls() != before {
                    changed += 1;
                }
            }));
            if let Err(p) = r {
                let n = names.get(&pid).map(|x| x.0.clone()).unwrap_or_else(|| pid.to_string());
                self.bg_skip(&n, &AppLog::describe_panic(p.as_ref()));
            }
        }
        // Timer guard for processes outside the user's session (services such as RGB and peripheral daemons): only
        // their timer-resolution requests are ignored; priority, I/O, memory, EcoQoS and affinity stay untouched.
        // The baseline keeps their original masks for the revert.
        let timer_unsupported = self.st().timer_unsupported;
        if let (true, Some(timer_only), false) = (timer_guard, timer_only, timer_unsupported) {
            let mut extra: Vec<Handle> = Vec::new();
            let mut added_t = false;
            for &pid in timer_only {
                let r = catch_unwind(AssertUnwindSafe(|| {
                    let exe = name_of(pid);
                    if Self::keeps_timer(&exe) || BackgroundDeny::contains(&exe) || self.gate_hook(pid, &exe).is_some() || Guard::is_anti_cheat_process(&exe) {
                        return;
                    }
                    let mut level = 0;
                    let h = self.n.open_process_graceful(pid, &mut level);
                    if level < 0 {
                        return;
                    }
                    extra.push(h);
                    let (mut c, mut st_) = (0u32, 0u32);
                    if self.n.is_protected(h) || !self.n.get_eco(h, &mut c, &mut st_) {
                        return;
                    }
                    if (c & TIMER_BIT) != 0 && (st_ & TIMER_BIT) != 0 {
                        return;
                    }
                    let created = self.n.process_created(h);
                    self.st().note_born(pid, created);
                    if !self.st().proc_before.contains_key(&pid) {
                        added_t = true;
                        self.baseline(pid, h, created, &exe, true);
                    }
                    if added_t {
                        self.before_mutate();
                        added_t = false;
                    }
                    let before = self.set_calls();
                    self.set_eco_guarded(h, c | TIMER_BIT, st_ | TIMER_BIT);
                    if self.set_calls() != before {
                        changed += 1;
                    }
                }));
                if let Err(p) = r {
                    let n = names.get(&pid).map(|x| x.0.clone()).unwrap_or_else(|| pid.to_string());
                    self.bg_skip(&n, &AppLog::describe_panic(p.as_ref()));
                }
            }
            for h in extra {
                self.n.close(h);
            }
        }
        for (_, h, _) in &open {
            self.n.close(*h);
        }
        changed
    }

    /// A process the background layer could not handle is skipped; the reason is logged once per name.
    fn bg_skip(&self, exe: &str, why: &str) {
        let first = self.st().bg_skip_logged.ci_add(exe);
        if first {
            AppLog::add_sev(lf!("Background: {0} skipped ({1})", exe, why), Sev::Warn);
        }
    }

    // ---- timer guard ------------------------------------------------------------------------------------------

    /// When on, background processes also get PROCESS_POWER_THROTTLING_IGNORE_TIMER_RESOLUTION (Windows 11): Windows
    /// stops honouring their requests for a finer system timer (a peripheral service asking for 0.5 ms keeps the whole
    /// system ticking at 2000 Hz). The game's own request is never touched. Reverted with the rest of the background
    /// baseline; on Windows 10 the flag is refused and only EcoQoS is set, as before. Default on.
    pub fn timer_guard(&self) -> bool {
        self.timer_guard.load(Ordering::SeqCst)
    }
    pub fn set_timer_guard(&self, on: bool) {
        self.timer_guard.store(on, Ordering::SeqCst);
    }

    pub fn keeps_timer(exe: &str) -> bool {
        timer_keepers().ci_contains(exe) || BackgroundDeny::contains(exe)
    }

    /// Writes the power-throttling masks; if Windows refuses the timer flag (Windows 10), retries without it.
    /// `state` u32::MAX = the same as `control` (the C# default argument).
    fn set_eco_guarded(&self, h: Handle, mut control: u32, mut state: u32) {
        if state == u32::MAX {
            state = control;
        }
        if (control & TIMER_BIT) != 0 && self.st().timer_unsupported {
            control &= !TIMER_BIT;
            state &= !TIMER_BIT;
        }
        if self.n.set_eco(h, control, state) {
            self.set(true);
            return;
        }
        if (control & TIMER_BIT) != 0 {
            let err = self.n.last_error();
            if err == 87 || err == 1 || err == 50 {
                self.st().timer_unsupported = true;
                AppLog::add_sev(L::t("Timer guard: this Windows does not support ignoring timer requests; only EcoQoS is set"), Sev::Info);
                self.set(self.n.set_eco(h, control & !TIMER_BIT, state & !TIMER_BIT));
                return;
            }
        }
        self.set(false);
    }

    // ---- revert -----------------------------------------------------------------------------------------------

    /// Gives a process back what was changed: process values, then the threads whose birth time still matches.
    pub fn revert(&self, pid: u32) -> i32 {
        let _g = self.gate.enter();
        self.revert_locked(pid)
    }

    /// Restores one process and its threads. A value Windows refused to put back (the process still lives) keeps its
    /// baseline: `pending` stays true, the session is not marked clean and the next revert or the crash restore of
    /// the next start tries again. A process or thread that ended (or whose ID now belongs to another one) is
    /// forgotten: there is nothing left to put back.
    fn revert_locked(&self, pid: u32) -> i32 {
        self.reverts.fetch_add(1, Ordering::SeqCst);
        let mut n = 0;
        let mut proc_left = false;
        let b = self.st().proc_before.get(&pid).cloned();
        if let Some(b) = &b {
            let mut level = 0;
            let ph = self.n.open_process_graceful(pid, &mut level);
            if level >= 0 {
                let r = catch_unwind(AssertUnwindSafe(|| {
                    let created = self.n.process_created(ph);
                    if created == 0 || created == b.created {
                        let (done, failed) = self.restore_proc_counted(ph, level, b);
                        n += done;
                        proc_left = failed > 0;
                    }
                }));
                self.n.close(ph);
                if let Err(p) = r {
                    std::panic::resume_unwind(p);
                }
            }
        }
        let prefix = format!("{}:", pid);
        let mut threads_left: Vec<String> = Vec::new();
        let list: Vec<(String, ThrBefore)> = self.st().thr_before.iter().filter(|(k, _)| k.starts_with(&prefix)).map(|(k, v)| (k.clone(), v.clone())).collect();
        for (k, tb) in list {
            let tid: u32 = k[prefix.len()..].parse().unwrap_or(0);
            let th = self.n.open_thread_of(pid, tid);
            if th.is_null() {
                continue;
            }
            let r = catch_unwind(AssertUnwindSafe(|| {
                let (mut born, mut ku) = (0i64, 0u64);
                self.n.thread_times(th, &mut born, &mut ku);
                if born != tb.born {
                    return; // another thread with the same ID: skipped
                }
                let mut bad = 0;
                let mut cur = 0;
                if tb.prio_set && (!self.n.get_thread_prio(th, &mut cur) || cur != tb.prio) {
                    if self.set(self.n.set_thread_prio(th, tb.prio)) {
                        n += 1
                    } else {
                        bad += 1
                    }
                }
                let mut cb = false;
                if tb.boost_set && (!self.n.get_thread_boost_disabled(th, &mut cb) || cb != tb.boost_disabled) {
                    if self.set(self.n.set_thread_boost_disabled(th, tb.boost_disabled)) {
                        n += 1
                    } else {
                        bad += 1
                    }
                }
                let pinned = self.st().thr_sets.contains_key(&k);
                if tb.sets_set && pinned {
                    if self.set(self.n.set_thread_cpu_sets(th, &tb.sets)) {
                        n += 1
                    } else {
                        bad += 1
                    }
                }
                let ours = self.st().ideal.get(&k).copied();
                let mut has_ideal = ours.is_some();
                if tb.ideal_only && has_ideal {
                    // 1.17.1: the all-threads layer's value goes back only while it is still in force.
                    let mut now_lp = 0u32;
                    has_ideal = self.n.get_thread_ideal(th, &mut now_lp) && Some(now_lp) == ours;
                }
                if tb.ideal_set && has_ideal {
                    if self.set(self.n.set_thread_ideal(th, tb.ideal)) {
                        n += 1
                    } else {
                        bad += 1
                    }
                }
                if bad > 0 {
                    threads_left.push(k.clone());
                }
            }));
            self.n.close(th);
            if let Err(p) = r {
                std::panic::resume_unwind(p);
            }
        }
        let mut st = self.st();
        if !proc_left && threads_left.is_empty() {
            st.forget(pid);
            return n;
        }
        // Something could not be put back: its baseline stays (only what was restored or ended is dropped).
        let keep_proc = if proc_left { b } else { None };
        let keep_threads: Vec<(String, ThrBefore)> = threads_left.iter().map(|k| (k.clone(), st.thr_before[k].clone())).collect();
        let keep_ideal: Vec<(String, u32)> = threads_left.iter().filter_map(|k| st.ideal.get(k).map(|v| (k.clone(), *v))).collect();
        let keep_sets: Vec<(String, u32)> = threads_left.iter().filter_map(|k| st.thr_sets.get(k).map(|v| (k.clone(), *v))).collect();
        let name = st.proc_name.get(&pid).cloned().unwrap_or_default();
        let bg = st.bg_pids.contains(&pid);
        st.forget(pid);
        if let Some(kp) = keep_proc {
            st.proc_before.insert(pid, kp);
            st.proc_name.insert(pid, name.clone());
            if bg {
                st.bg_pids.insert(pid);
            }
        }
        for (k, tb) in keep_threads {
            st.thr_before.insert(k, tb);
        }
        for (k, v) in keep_ideal {
            st.ideal.insert(k, v);
        }
        for (k, v) in keep_sets {
            st.thr_sets.insert(k, v);
        }
        drop(st);
        AppLog::add_sev(
            lf!("Revert: some values of {0} (PID {1}) could not be put back; they are tried again later", if !name.is_empty() { name.as_str() } else { "?" }, pid),
            Sev::Warn,
        );
        n
    }

    fn restore_proc(&self, ph: Handle, level: i32, b: &ProcBefore) -> i32 {
        self.restore_proc_counted(ph, level, b).0
    }

    fn restore_proc_counted(&self, ph: Handle, level: i32, b: &ProcBefore) -> (i32, i32) {
        let (mut n, mut bad) = (0, 0);
        let mut one = |needed: bool, write: &dyn Fn() -> bool| {
            if !needed {
                return;
            }
            if self.set(write()) {
                n += 1
            } else {
                bad += 1
            }
        };
        let mut cur = Vec::new();
        one(b.sets_set && (!self.n.get_cpu_sets(ph, &mut cur) || ids_key(&cur) != ids_key(&b.sets)), &|| self.n.set_cpu_sets(ph, &b.sets));
        let mut a = 0u64;
        one(b.aff_set && level >= 1 && (!self.n.get_affinity(ph, &mut a) || a != b.aff), &|| self.n.set_affinity(ph, b.aff));
        let mut p = 0u32;
        one(b.prio_set && (!self.n.get_priority_class(ph, &mut p) || p != b.prio), &|| self.n.set_priority_class(ph, b.prio));
        let mut io = 0u32;
        one(b.io_set && (!self.n.get_io(ph, &mut io) || io != b.io), &|| self.n.set_io(ph, b.io));
        let mut m = 5u32;
        one(b.mem_set && (!self.n.get_mem(ph, &mut m) || m != b.mem), &|| self.n.set_mem(ph, b.mem));
        let (mut c, mut s) = (0u32, 0u32);
        one(b.eco_set && (!self.n.get_eco(ph, &mut c, &mut s) || c != b.eco_ctl || s != b.eco_state), &|| self.n.set_eco(ph, b.eco_ctl, b.eco_state));
        let mut d = false;
        one(b.boost_set && (!self.n.get_boost_disabled(ph, &mut d) || d != b.boost_disabled), &|| self.n.set_boost_disabled(ph, b.boost_disabled));
        (n, bad)
    }

    /// Baselines are left (a value could not be put back) or the timer request is still held.
    pub fn pending(&self) -> bool {
        let _g = self.gate.enter();
        let st = self.st();
        !st.proc_before.is_empty() || !st.thr_before.is_empty() || st.timer_requested
    }

    fn revert_each(&self, pids: Vec<u32>) -> i32 {
        let mut n = 0;
        for pid in pids {
            match catch_unwind(AssertUnwindSafe(|| self.revert_locked(pid))) {
                Ok(x) => n += x,
                Err(p) => AppLog::add_sev(lf!("Revert of PID {0}: {1}", pid, AppLog::describe_panic(p.as_ref())), Sev::Warn),
            }
        }
        n
    }

    /// Reverts every process (game and background); one process failing does not stop the others. Idempotent.
    pub fn revert_all(&self) -> i32 {
        let _g = self.gate.enter();
        let pids: Vec<u32> = {
            let st = self.st();
            let s: IndexSet<u32> = st.proc_before.keys().copied().chain(st.thr_before.keys().map(|k| split_key(k).0)).collect();
            s.into_iter().collect()
        };
        self.revert_each(pids)
    }

    pub fn revert_background(&self) -> i32 {
        let _g = self.gate.enter();
        let pids: Vec<u32> = self.st().bg_pids.iter().copied().collect();
        self.revert_each(pids)
    }

    pub fn is_background(&self, pid: u32) -> bool {
        let _g = self.gate.enter();
        self.st().bg_pids.contains(&pid)
    }

    pub fn background_count(&self) -> i32 {
        let _g = self.gate.enter();
        self.st().bg_pids.len() as i32
    }

    /// Forgets ended processes, ended threads of living processes, and thread data older than 5 minutes.
    pub fn prune(&self, live: Option<&Tids>) {
        let _g = self.gate.enter();
        let read;
        let live = match live {
            Some(l) => l,
            None => {
                read = self.n.threads_by_pid();
                &read
            }
        };
        let cut = self.now() - TimeSpan::from_minutes(5.0);
        let mut st = self.st();
        let gone: IndexSet<u32> =
            st.proc_before.keys().chain(st.born.keys()).chain(st.mod_cache.keys()).copied().filter(|p| !live.contains_key(p)).collect();
        for pid in gone {
            st.forget(pid);
        }
        let alive = |k: &str| {
            let (pid, tid) = split_key(k);
            live.get(&pid).map(|tids| tid == 0 || tids.contains(&tid)).unwrap_or(false)
        };
        st.thr_before.retain(|k, _| alive(k));
        st.ideal.retain(|k, _| alive(k));
        st.thr_sets.retain(|k, _| alive(k));
        st.preset_tid_count.retain(|k, _| alive(k));
        st.thr_prev.retain(|k, v| alive(k) && v.t >= cut);
        st.thr_meta.retain(|k, v| alive(k) && v.seen >= cut);
        if st.advice.len() > 200 {
            let n = st.advice.len() - 200;
            st.advice.drain(0..n);
        }
    }

    /// Sizes of the per-thread maps (leak checks): (Ideal, Meta, Prev, Before).
    pub fn sizes(&self) -> (i32, i32, i32, i32) {
        let _g = self.gate.enter();
        let st = self.st();
        (st.ideal.len() as i32, st.thr_meta.len() as i32, st.thr_prev.len() as i32, st.thr_before.len() as i32)
    }

    // ---- crash-safe session -----------------------------------------------------------------------------------

    pub fn export(&self) -> SessionData {
        let _g = self.gate.enter();
        let st = self.st();
        let mut s = SessionData { timer_requested: st.timer_requested, ..Default::default() };
        for (pid, b) in &st.proc_before {
            s.procs.push(SessionProc {
                pid: *pid,
                name: st.proc_name.get(pid).cloned().unwrap_or_default(),
                created: b.created,
                background: st.bg_pids.contains(pid),
                prio: b.prio,
                prio_set: b.prio_set,
                aff: b.aff,
                aff_set: b.aff_set,
                sets: b.sets.clone(),
                sets_set: b.sets_set,
                io: b.io,
                io_set: b.io_set,
                mem: b.mem,
                mem_set: b.mem_set,
                eco_ctl: b.eco_ctl,
                eco_state: b.eco_state,
                eco_set: b.eco_set,
                boost_disabled: b.boost_disabled,
                boost_set: b.boost_set,
            });
        }
        for (k, tb) in &st.thr_before {
            let (pid, tid) = split_key(k);
            s.threads.push(SessionThread {
                pid,
                tid,
                born: tb.born,
                prio: tb.prio,
                prio_set: tb.prio_set,
                boost_disabled: tb.boost_disabled,
                boost_set: tb.boost_set,
                sets: tb.sets.clone(),
                sets_set: tb.sets_set,
                ideal: tb.ideal,
                ideal_set: tb.ideal_set && st.ideal.contains_key(k),
            });
        }
        s
    }

    /// The values a crash restore may write (pure): the ones the program itself could have saved as a baseline. A
    /// priority class of realtime, a time-critical thread priority, the critical I/O priority, an empty affinity or a
    /// processor number past the last one are refused (the session file is never trusted further than that).
    pub fn session_proc_valid(p: &SessionProc) -> bool {
        (!p.prio_set || (Self::prio_contains_value(p.prio) && p.prio != PRIO_REALTIME))
            && (!p.aff_set || p.aff != 0)
            && (!p.io_set || p.io <= 3)
            && (!p.mem_set || (1..=5).contains(&p.mem))
            && (!p.sets_set || p.sets.len() <= 1024)
    }

    /// The ideal processor is a number inside the thread's processor group (0-63).
    pub fn session_thread_valid(t: &SessionThread) -> bool {
        (!t.prio_set || (Self::tprio_contains_value(t.prio) && t.prio != 15)) && (!t.ideal_set || t.ideal < 64) && (!t.sets_set || t.sets.len() <= 1024)
    }

    /// Restores a session left by a crash: only processes and threads whose creation times still match, only values
    /// that pass `session_proc_valid` / `session_thread_valid`, and only processes the guard lets the program touch
    /// in normal operation (the name must still be the one saved; anti-cheat and protected ones are skipped).
    pub fn restore_session(&self, s: Option<&SessionData>) -> i32 {
        let _g = self.gate.enter();
        let mut n = 0;
        let Some(s) = s else { return 0 };
        let procs = self.n.processes();
        let refused = |pid: u32, saved_name: Option<&str>| -> Option<String> {
            let now = procs.get(&pid).map(|p| p.0.clone()).unwrap_or_default();
            if now.is_empty() {
                return Some("gone".into());
            }
            if let Some(sn) = saved_name {
                if !sn.is_empty() && !ci::eq(&now, sn) {
                    return Some("renamed".into());
                }
            }
            self.gate_hook(pid, &now)
        };
        let mut names: IndexMap<u32, String> = IndexMap::new();
        for p in &s.procs {
            names.entry(p.pid).or_insert_with(|| p.name.clone());
        }
        // Section 22.5: a timer request left by a crashed run is released before anything else.
        let (mut c0, mut finest, mut c2, mut a) = (0u32, 0u32, 0u32, 0u32);
        if s.timer_requested && self.n.timer_query(&mut c0, &mut finest, &mut c2) && self.n.timer_set(finest, false, &mut a) {
            n += 1;
            AppLog::add(L::t("Timer resolution: the request of the crashed run was released"));
        }
        for _ in 0..s.null_procs {
            AppLog::add_sev(lf!("Crash restore: the saved values of PID {0} are not ones the program writes; skipped", 0), Sev::Warn);
        }
        for p in &s.procs {
            if !Self::session_proc_valid(p) {
                AppLog::add_sev(lf!("Crash restore: the saved values of PID {0} are not ones the program writes; skipped", p.pid), Sev::Warn);
                continue;
            }
            if refused(p.pid, Some(&p.name)).is_some() {
                continue;
            }
            let mut level = 0;
            let ph = self.n.open_process_graceful(p.pid, &mut level);
            if level < 0 {
                continue;
            }
            let r = catch_unwind(AssertUnwindSafe(|| {
                if self.n.process_created(ph) != p.created {
                    return; // someone else's PID now
                }
                let b = ProcBefore {
                    have: true,
                    created: p.created,
                    prio: p.prio,
                    prio_set: p.prio_set,
                    aff: p.aff,
                    aff_set: p.aff_set,
                    sets: p.sets.clone(),
                    sets_set: p.sets_set,
                    io: p.io,
                    io_set: p.io_set,
                    mem: p.mem,
                    mem_set: p.mem_set,
                    eco_ctl: p.eco_ctl,
                    eco_state: p.eco_state,
                    eco_set: p.eco_set,
                    boost_disabled: p.boost_disabled,
                    boost_set: p.boost_set,
                };
                n += self.restore_proc(ph, level, &b);
            }));
            self.n.close(ph);
            if let Err(e) = r {
                std::panic::resume_unwind(e);
            }
        }
        for _ in 0..s.null_threads {
            AppLog::add_sev(lf!("Crash restore: the saved values of thread {0} are not ones the program writes; skipped", 0), Sev::Warn);
        }
        for t in &s.threads {
            if !Self::session_thread_valid(t) {
                AppLog::add_sev(lf!("Crash restore: the saved values of thread {0} are not ones the program writes; skipped", t.tid), Sev::Warn);
                continue;
            }
            if refused(t.pid, names.get(&t.pid).map(|x| x.as_str())).is_some() {
                continue;
            }
            let th = self.n.open_thread_of(t.pid, t.tid);
            if th.is_null() {
                continue;
            }
            let r = catch_unwind(AssertUnwindSafe(|| {
                let (mut born, mut ku) = (0i64, 0u64);
                self.n.thread_times(th, &mut born, &mut ku);
                if born != t.born {
                    return;
                }
                let mut cur = 0;
                if t.prio_set && (!self.n.get_thread_prio(th, &mut cur) || cur != t.prio) && self.set(self.n.set_thread_prio(th, t.prio)) {
                    n += 1;
                }
                let mut cb = false;
                if t.boost_set && (!self.n.get_thread_boost_disabled(th, &mut cb) || cb != t.boost_disabled) && self.set(self.n.set_thread_boost_disabled(th, t.boost_disabled)) {
                    n += 1;
                }
                let mut cs = Vec::new();
                if t.sets_set && self.n.get_thread_cpu_sets(th, &mut cs) && ids_key(&cs) != ids_key(&t.sets) && self.set(self.n.set_thread_cpu_sets(th, &t.sets)) {
                    n += 1;
                }
                if t.ideal_set && self.set(self.n.set_thread_ideal(th, t.ideal)) {
                    n += 1;
                }
            }));
            self.n.close(th);
            if let Err(e) = r {
                std::panic::resume_unwind(e);
            }
        }
        n
    }
}

#[cfg(test)]
#[path = "engine_tests.rs"]
mod tests;
