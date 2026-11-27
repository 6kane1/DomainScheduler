//! Burst guard (1.16.0 Test 1.10, port of `BurstGuard.cs`). The idea of an owner's "low-latency module", rewritten so
//! it can only help and acts only where it measures a real problem:
//!  1. CPU: while a game is handled, background programs of the user's session are sampled every 2 s. One that keeps
//!     using more than [`BurstPolicy::HEAVY_CORES`] of a logical processor for 3 samples in a row gets a hard CPU cap
//!     of [`BurstPolicy::CAP_CORES`] processor through a Job Object of its own. The job lets child processes break
//!     away silently, so a game started later from a capped launcher is never capped. The cap is lifted at once when
//!     the program comes to the front, and for all of them when the game ends, Auto stops or Toolbox exits; the job
//!     names are saved, so after a crash the next start lifts them ([`BurstGuard::recover_after_crash`]).
//!  2. Memory: only when free memory is low (under 10% of RAM or 1.5 GB) the game's current working set becomes its
//!     hard minimum (at most 25% of RAM), so Windows does not trim the pages the game already uses. Lifted with the
//!     game.
//!
//! It never touches services, other sessions, the shell, launchers, audio / voice / streaming / VR, network
//! boosters, security software or anti-cheats (the same deny lists as the background layer, plus
//! [`BurstPolicy::never`]), and the game itself only for the working set and only when Auto may touch the game (not
//! under FACEIT and the like). On a clean PC it does nothing at all: that is the point.

use crate::base::app_log::{AppLog, Sev};
use crate::base::ci::{self, CiSet, CiSetExt};
use crate::base::fmt::{num, Culture};
use crate::base::hooks::lock;
use crate::base::l::L;
use crate::base::time::DateTime;
use crate::lf;
use crate::native::Handle;
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::{Arc, Mutex, OnceLock, RwLock};

/// `BurstPolicy` (static class).
pub struct BurstPolicy;

impl BurstPolicy {
    /// A program is "heavy" above this share of one logical processor (averaged over a sample).
    pub const HEAVY_CORES: f64 = 0.30;
    /// The cap a heavy program gets, in logical processors.
    pub const CAP_CORES: f64 = 0.50;
    /// Samples in a row above HeavyCores before the cap.
    pub const HEAVY_SAMPLES: i32 = 3;
    pub const SAMPLE_SECONDS: i32 = 2;
    /// Soft mode: the job's scheduling weight (1..9, 5 = what everyone else has; 1 = the smallest share).
    pub const SOFT_WEIGHT: i32 = 1;
    /// 1.16.1 Test 2: the game's hard page faults per second above which its memory is held (2 samples in a row).
    pub const HARD_FAULTS_PER_SEC: f64 = 30.0;
    /// If the hold did not bring the hard faults under half of the level that started it within this time, it is lifted.
    pub const HOLD_CHECK_SEC: i32 = 10;

    /// Programs never capped even when heavy: network boosters carry the game's traffic; since 1.16.2 also the
    /// graphics driver's own processes (NVIDIA / AMD containers, panels, overlays: part of the driver) and Domain OS'
    /// measuring tools (a capped DomainSchedTest can fall behind its kernel trace and measure less exactly).
    pub fn never() -> &'static CiSet {
        static V: OnceLock<CiSet> = OnceLock::new();
        V.get_or_init(|| {
            ci::set_of([
                "ExitLag.exe", "GearUP_Booster.exe", "GearUP Booster.exe", "NoPing.exe", "WTFast.exe", "Mudfish.exe",
                "MsMpEng.exe", "NisSrv.exe", "MpDefenderCoreService.exe", "SecurityHealthService.exe", "SecurityHealthSystray.exe",
                "smartscreen.exe", "dwm.exe", "explorer.exe", "csrss.exe", "ctfmon.exe", "TextInputHost.exe",
                "NVDisplay.Container.exe", "nvcontainer.exe", "NVIDIA Overlay.exe", "nvsphelper64.exe", "NVIDIA app.exe", "NVIDIA Share.exe",
                "RadeonSoftware.exe", "AMDRSServ.exe", "AMDRSSrcExt.exe", "amdow.exe", "atieclxx.exe", "atiesrxx.exe", "amdfendrsr.exe",
                "DomainSchedTest.exe", "DomainOS.LoadLab.exe",
            ])
        })
    }

    /// Processors used during a sample: CPU time (100 ns units) over wall time.
    pub fn cores(cpu_delta_100ns: i64, seconds: f64) -> f64 {
        if seconds <= 0.0 { 0.0 } else { cpu_delta_100ns as f64 / 1e7 / seconds }
    }

    /// JOBOBJECT_CPU_RATE_CONTROL_INFORMATION.CpuRate: share of the whole machine in 1/100 of a percent.
    pub fn cpu_rate(cores: f64, logical: i32) -> u32 {
        // Math.Round: banker's rounding (to even)
        let v = cores / logical.max(1) as f64 * 10000.0;
        round_even(v).clamp(1.0, 10000.0) as u32
    }

    /// Low free memory: under 10% of RAM or under 1.5 GB, whichever is larger.
    pub fn memory_pressure(avail_bytes: u64, total_bytes: u64) -> bool {
        total_bytes > 0 && avail_bytes < (total_bytes / 10).max(1536u64 << 20)
    }

    /// The hard working-set minimum for the game: what it uses now, at most 25% of RAM; 0 = do not set.
    pub fn game_minimum(working_set: u64, total_bytes: u64) -> u64 {
        let cap = total_bytes / 4;
        let want = working_set.min(cap);
        if want < (256u64 << 20) { 0 } else { want }
    }

    /// One sample of one program: the new count of heavy samples in a row.
    pub fn next_streak(streak: i32, cores: f64) -> i32 {
        if cores >= Self::HEAVY_CORES { streak + 1 } else { 0 }
    }
}

/// `Math.Round(double)` (MidpointRounding.ToEven).
fn round_even(v: f64) -> f64 {
    let r = v.round();
    if (v - v.trunc()).abs() == 0.5 {
        2.0 * (v / 2.0).round()
    } else {
        r
    }
}

/// The Windows calls of the burst guard (a fake in the tests).
pub trait IBurstNative: Send + Sync {
    fn cpu_time(&self, pid: u32, created: &mut i64, cpu100ns: &mut i64) -> bool;
    /// Creates the named job, lets children break away, sets the limit and puts the process in it. weight = 0: a hard
    /// cap of `cpu_rate`; weight 1..9: weight-based sharing (1 = the smallest share), never a stop.
    fn cap(&self, pid: u32, job_name: &str, cpu_rate: u32, weight: i32, job: &mut Handle, error: &mut i32) -> bool;
    /// Lifts the cap (the process stays in the job until it exits; a job without limits does nothing).
    fn lift(&self, job: Handle);
    /// Opens a job left by a crashed Toolbox and lifts its cap; false when it no longer exists.
    fn lift_by_name(&self, job_name: &str) -> bool;
    fn close(&self, job: Handle);
    fn memory(&self, avail: &mut u64, total: &mut u64) -> bool;
    fn working_set(&self, pid: u32, ws: &mut u64, min: &mut u64, max: &mut u64) -> bool;
    fn set_working_set_min(&self, pid: u32, min: u64, max: u64, hard: bool) -> bool;
}

/// Where Windows is not there (Linux tests): every call fails, nothing is changed.
pub struct NoBurstNative;

impl IBurstNative for NoBurstNative {
    fn cpu_time(&self, _pid: u32, created: &mut i64, cpu100ns: &mut i64) -> bool {
        *created = 0;
        *cpu100ns = 0;
        false
    }
    fn cap(&self, _pid: u32, _job_name: &str, _cpu_rate: u32, _weight: i32, job: &mut Handle, error: &mut i32) -> bool {
        *job = Handle::NULL;
        *error = 50;
        false
    }
    fn lift(&self, _job: Handle) {}
    fn lift_by_name(&self, _job_name: &str) -> bool {
        false
    }
    fn close(&self, _job: Handle) {}
    fn memory(&self, avail: &mut u64, total: &mut u64) -> bool {
        *avail = 0;
        *total = 0;
        false
    }
    fn working_set(&self, _pid: u32, ws: &mut u64, min: &mut u64, max: &mut u64) -> bool {
        *ws = 0;
        *min = 0;
        *max = 0;
        false
    }
    fn set_working_set_min(&self, _pid: u32, _min: u64, _max: u64, _hard: bool) -> bool {
        false
    }
}

/// The product's native layer: the Windows calls on Windows, none elsewhere.
pub fn default_native() -> Arc<dyn IBurstNative> {
    #[cfg(windows)]
    {
        Arc::new(super::burst_native::WinBurstNative)
    }
    #[cfg(not(windows))]
    {
        Arc::new(NoBurstNative)
    }
}

struct Watch {
    created: i64,
    cpu: i64,
    at: DateTime,
    streak: i32,
    peak: f64,
}

struct Capped {
    created: i64,
    name: String,
    job: String,
    handle: Handle,
    peak: f64,
}

/// `record SavedCap(uint Pid, long Created, string Job)` in System.Text.Json's shape.
#[derive(Serialize, Deserialize)]
struct SavedCap {
    #[serde(rename = "Pid", default)]
    pid: u32,
    #[serde(rename = "Created", default)]
    created: i64,
    #[serde(rename = "Job", default)]
    job: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct SavedState {
    #[serde(rename = "Caps", default)]
    caps: Option<Vec<SavedCap>>,
}

struct St {
    watch: IndexMap<u32, Watch>,
    capped: IndexMap<u32, Capped>,
    history: Vec<(String, f64)>,
    last: DateTime,
    game: u32,
    game_created: i64,
    ws_set: bool,
    ws_min0: u64,
    ws_max0: u64,
    pressure_seen: bool,
    fault_high: i32,
    faults_at_hold: f64,
    held_at: DateTime,
}

pub type LogFn = dyn Fn(&str, Sev) + Send + Sync;
pub type NowFn = dyn Fn() -> DateTime + Send + Sync;
pub type HardFaultsFn = dyn Fn() -> Option<f64> + Send + Sync;

/// `BurstGuard`.
pub struct BurstGuard {
    n: Arc<dyn IBurstNative>,
    now: Arc<NowFn>,
    st: Mutex<St>,
    on: AtomicBool,
    soft: AtomicBool,
    state_path: Mutex<Option<PathBuf>>,
    logical: AtomicI32,
    log: RwLock<Arc<LogFn>>,
    game_hard_faults: RwLock<Option<Arc<HardFaultsFn>>>,
}

impl BurstGuard {
    /// `new BurstGuard(native, now)`: None = the Windows calls and the UTC clock.
    pub fn new(native: Option<Arc<dyn IBurstNative>>, now: Option<Arc<NowFn>>) -> Self {
        BurstGuard {
            n: native.unwrap_or_else(default_native),
            now: now.unwrap_or_else(|| Arc::new(DateTime::utc_now)),
            st: Mutex::new(St {
                watch: IndexMap::new(),
                capped: IndexMap::new(),
                history: Vec::new(),
                last: DateTime::MIN,
                game: 0,
                game_created: 0,
                ws_set: false,
                ws_min0: 0,
                ws_max0: 0,
                pressure_seen: false,
                fault_high: 0,
                faults_at_hold: 0.0,
                held_at: DateTime::MIN,
            }),
            on: AtomicBool::new(false),
            soft: AtomicBool::new(true),
            state_path: Mutex::new(None),
            logical: AtomicI32::new(std::thread::available_parallelism().map(|n| n.get() as i32).unwrap_or(1)),
            log: RwLock::new(Arc::new(|m: &str, s: Sev| AppLog::add_sev(m, s))),
            game_hard_faults: RwLock::new(None),
        }
    }

    /// The switch (setting BurstGuard; on by default since 1.16.0).
    pub fn on(&self) -> bool {
        self.on.load(Ordering::SeqCst)
    }
    pub fn set_on(&self, on: bool) {
        self.on.store(on, Ordering::SeqCst);
    }
    /// Soft mode (1.16.0 Test 1.11, setting BurstGuardSoft, on by default): instead of a hard cap (which stops the
    /// program until the next accounting interval once its share is used, up to hundreds of ms), the job gets the
    /// smallest scheduling weight: the program is never stopped, it only gives way when the processors are busy.
    pub fn soft(&self) -> bool {
        self.soft.load(Ordering::SeqCst)
    }
    pub fn set_soft(&self, on: bool) {
        self.soft.store(on, Ordering::SeqCst);
    }
    /// Where the names of open jobs are kept for crash recovery (None: not kept, tests).
    pub fn state_path(&self) -> Option<PathBuf> {
        lock(&self.state_path).clone()
    }
    pub fn set_state_path(&self, p: Option<PathBuf>) {
        *lock(&self.state_path) = p;
    }
    pub fn logical(&self) -> i32 {
        self.logical.load(Ordering::SeqCst)
    }
    pub fn set_logical(&self, n: i32) {
        self.logical.store(n, Ordering::SeqCst);
    }
    pub fn set_log(&self, f: Arc<LogFn>) {
        *self.log.write().unwrap_or_else(|e| e.into_inner()) = f;
    }
    /// 1.16.1 Test 2: the game's hard page faults per second from the wait probe (None = not measured: free-memory rule).
    pub fn set_game_hard_faults(&self, f: Option<Arc<HardFaultsFn>>) {
        *self.game_hard_faults.write().unwrap_or_else(|e| e.into_inner()) = f;
    }

    fn log_(&self, m: &str, s: Sev) {
        let f = self.log.read().unwrap_or_else(|e| e.into_inner()).clone();
        f(m, s);
    }

    pub fn capped_count(&self) -> i32 {
        lock(&self.st).capped.len() as i32
    }

    fn alive(&self, pid: u32, created: i64) -> bool {
        let (mut cr, mut cpu) = (0i64, 0i64);
        self.n.cpu_time(pid, &mut cr, &mut cpu) && cr == created
    }

    /// One tick of Auto while a game is handled. `candidates`: background processes of the user's session that the
    /// background layer may change (pid, exe); `foreground`: the window in front.
    pub fn tick(&self, game: u32, game_created: i64, touch_game: bool, candidates: &[(u32, String)], foreground: u32) {
        let mut s = lock(&self.st);
        if !self.on() {
            self.release_locked(&mut s, false);
            return;
        }
        if game != s.game || game_created != s.game_created {
            self.release_locked(&mut s, false);
            s.game = game;
            s.game_created = game_created;
        }
        let now = (self.now)();
        if s.last != DateTime::MIN && (now - s.last).total_seconds() < BurstPolicy::SAMPLE_SECONDS as f64 {
            return;
        }
        s.last = now;

        // A capped program that came to the front, or that ended, is released.
        let pids: Vec<u32> = s.capped.keys().copied().collect();
        for pid in pids {
            let (created, handle, name) = {
                let c = &s.capped[&pid];
                (c.created, c.handle, c.name.clone())
            };
            let alive = self.alive(pid, created);
            if !alive || pid == foreground {
                self.n.lift(handle);
                self.n.close(handle);
                s.capped.shift_remove(&pid);
                if alive {
                    self.log_(&lf!("Burst guard: {0} is in front, its CPU cap is lifted", &name), Sev::Info);
                }
            }
        }

        let mut seen: HashSet<u32> = HashSet::new();
        let soft = self.soft();
        for (pid, name) in candidates {
            let pid = *pid;
            if pid == game || pid == foreground || BurstPolicy::never().ci_contains(name) {
                continue;
            }
            let (mut created, mut cpu) = (0i64, 0i64);
            if !self.n.cpu_time(pid, &mut created, &mut cpu) {
                continue;
            }
            seen.insert(pid);
            let fresh = s.watch.get(&pid).map(|w| w.created != created).unwrap_or(true);
            if fresh {
                s.watch.insert(pid, Watch { created, cpu, at: now, streak: 0, peak: 0.0 });
                continue;
            }
            let (cores, peak, streak) = {
                let w = s.watch.get_mut(&pid).unwrap();
                let cores = BurstPolicy::cores(cpu - w.cpu, (now - w.at).total_seconds());
                w.cpu = cpu;
                w.at = now;
                w.peak = w.peak.max(cores);
                w.streak = BurstPolicy::next_streak(w.streak, cores);
                (cores, w.peak, w.streak)
            };
            if streak >= BurstPolicy::HEAVY_SAMPLES && !s.capped.contains_key(&pid) {
                let job = format!("DomainOS.BurstGuard.{}.{}", pid, created);
                let rate = BurstPolicy::cpu_rate(BurstPolicy::CAP_CORES, self.logical());
                let (mut h, mut err) = (Handle::NULL, 0i32);
                if self.n.cap(pid, &job, rate, if soft { BurstPolicy::SOFT_WEIGHT } else { 0 }, &mut h, &mut err) {
                    s.capped.insert(pid, Capped { created, name: name.clone(), job, handle: h, peak });
                    self.log_(
                        &if soft {
                            lf!("Burst guard: {0} used {1:0.0} processors in the background during the game; it now gives way to the game (lowest share)", name, peak)
                        } else {
                            lf!("Burst guard: {0} used {1:0.0} processors in the background during the game; capped at {2:0.0}", name, peak, BurstPolicy::CAP_CORES)
                        },
                        Sev::Info,
                    );
                    self.save(&s);
                } else {
                    s.watch.get_mut(&pid).unwrap().streak = i32::MIN / 2; // refused (protected, or a job that forbids it): not tried again
                    self.log_(&lf!("Burst guard: {0} could not be capped (error {1})", name, err), Sev::Info);
                }
            }
            if let Some(cp) = s.capped.get_mut(&pid) {
                cp.peak = cp.peak.max(cores);
            }
        }
        s.watch.retain(|p, _| seen.contains(p));

        // Memory (1.16.1 Test 2): what hurts a frame is the game reading its own pages back from disk (hard faults),
        // not the amount of free memory. With the probe running, the game's current working set becomes a hard minimum
        // when its hard faults stay above HardFaultsPerSec, and the hold is lifted again if it did not help. Without
        // the probe the old rule stays: only under low free memory.
        let hf_fn = self.game_hard_faults.read().unwrap_or_else(|e| e.into_inner()).clone();
        let hf: Option<f64> = hf_fn.and_then(|f| f());
        if let Some(h) = hf {
            if s.ws_set && (now - s.held_at).total_seconds() >= BurstPolicy::HOLD_CHECK_SEC as f64 && h >= s.faults_at_hold / 2.0 {
                if self.alive(s.game, s.game_created) {
                    self.n.set_working_set_min(s.game, s.ws_min0, s.ws_max0, false);
                }
                s.ws_set = false;
                self.log_(&lf!("Burst guard: holding the game's memory did not help (still {0:0} reads from disk per second); lifted", h), Sev::Info);
                s.fault_high = i32::MIN / 2; // not again during this game
            }
        }
        s.fault_high = match hf {
            Some(h) if h > BurstPolicy::HARD_FAULTS_PER_SEC => s.fault_high + 1,
            _ => s.fault_high.min(0),
        };
        let want = match hf {
            Some(_) => s.fault_high >= 2,
            None => {
                let (mut av0, mut tot0) = (0u64, 0u64);
                self.n.memory(&mut av0, &mut tot0) && BurstPolicy::memory_pressure(av0, tot0)
            }
        };
        let (mut avail, mut total) = (0u64, 0u64);
        if touch_game && !s.ws_set && want && self.n.memory(&mut avail, &mut total) {
            s.pressure_seen = true;
            let (mut ws, mut min0, mut max0) = (0u64, 0u64, 0u64);
            if self.n.working_set(game, &mut ws, &mut min0, &mut max0) {
                let min = BurstPolicy::game_minimum(ws, total);
                if min > 0 && self.n.set_working_set_min(game, min, max0.max(min + (256u64 << 20)), true) {
                    s.ws_set = true;
                    s.ws_min0 = min0;
                    s.ws_max0 = max0;
                    s.held_at = now;
                    s.faults_at_hold = hf.unwrap_or(0.0);
                    if let Some(h) = hf {
                        self.log_(&lf!("Burst guard: the game read {0:0} of its own pages per second back from disk; it keeps its {1} MB in memory", h, min >> 20), Sev::Info);
                    } else {
                        self.log_(&lf!("Burst guard: little free memory ({0} MB); the game keeps its {1} MB in memory", avail >> 20, min >> 20), Sev::Info);
                    }
                }
            }
        }
    }

    /// The game ended, Auto stopped or Toolbox exits: every cap is lifted, the game's minimum is put back.
    pub fn release(&self) {
        let mut s = lock(&self.st);
        self.release_locked(&mut s, true);
    }

    fn release_locked(&self, s: &mut St, summary: bool) {
        let caps: Vec<(Handle, String, f64)> = s.capped.values().map(|c| (c.handle, c.name.clone(), c.peak)).collect();
        for (h, name, peak) in caps {
            self.n.lift(h);
            self.n.close(h);
            s.history.push((name, peak));
        }
        let had = !s.capped.is_empty();
        s.capped.clear();
        if s.ws_set && s.game != 0 && self.alive(s.game, s.game_created) {
            self.n.set_working_set_min(s.game, s.ws_min0, s.ws_max0, false);
        }
        if summary && (!s.history.is_empty() || s.pressure_seen) {
            // GroupBy(name, OrdinalIgnoreCase): the first spelling, the highest peak, in the order of first appearance
            let mut groups: Vec<(String, f64)> = Vec::new();
            for (name, peak) in &s.history {
                match groups.iter_mut().find(|g| ci::eq(&g.0, name)) {
                    Some(g) => g.1 = g.1.max(*peak),
                    None => groups.push((name.clone(), *peak)),
                }
            }
            let cur = Culture::current();
            let list = groups.iter().map(|(n, p)| format!("{} ({})", n, num(*p, "0.0", &cur))).collect::<Vec<_>>().join(", ");
            let text = if !s.history.is_empty() {
                lf!(
                    "Burst guard after the game: capped in the background: {0}; little free memory: {1}",
                    list,
                    if s.pressure_seen { L::t("yes") } else { L::t("no") }
                )
            } else {
                L::t("Burst guard after the game: nothing was capped; free memory was low at times").to_string()
            };
            self.log_(&text, Sev::Info);
        }
        s.history.clear();
        s.ws_set = false;
        s.pressure_seen = false;
        s.fault_high = 0;
        s.watch.clear();
        s.game = 0;
        s.game_created = 0;
        s.last = DateTime::MIN;
        if had || self.state_path().is_some() {
            self.save(s);
        }
    }

    fn save(&self, s: &St) {
        let Some(path) = self.state_path() else { return };
        let _ = (|| -> Result<(), String> {
            if s.capped.is_empty() {
                if path.exists() {
                    std::fs::remove_file(&path).map_err(|e| e.to_string())?;
                }
                return Ok(());
            }
            let st = SavedState {
                caps: Some(s.capped.iter().map(|(pid, c)| SavedCap { pid: *pid, created: c.created, job: Some(c.job.clone()) }).collect()),
            };
            let json = serde_json::to_string(&st).map_err(|e| e.to_string())?;
            std::fs::write(&path, json).map_err(|e| e.to_string())
        })();
    }

    /// At start: caps a crashed Toolbox left behind are lifted (by the saved job names).
    pub fn recover_after_crash(&self) -> i32 {
        let Some(path) = self.state_path() else { return 0 };
        if !path.is_file() {
            return 0;
        }
        let mut n = 0;
        if let Ok(text) = std::fs::read_to_string(&path) {
            if let Ok(s) = serde_json::from_str::<Option<SavedState>>(&text) {
                for c in s.and_then(|s| s.caps).unwrap_or_default() {
                    // a cap without a job name: C# throws on it and stops the loop (caught)
                    let Some(job) = c.job else { break };
                    if job.starts_with("DomainOS.BurstGuard.") && self.n.lift_by_name(&job) {
                        n += 1;
                    }
                }
            }
        }
        let _ = std::fs::remove_file(&path);
        if n > 0 {
            self.log_(&lf!("Burst guard: {0} CPU caps left from the last run were lifted", n), Sev::Info);
        }
        n
    }
}

#[cfg(test)]
mod tests {
    //! BurstGuardTests.cs (9 tests) plus the crash-recovery file.
    use super::*;
    use crate::base::time::{Kind, TimeSpan};
    use std::collections::HashMap;

    #[derive(Default)]
    struct FS {
        procs: HashMap<u32, (i64, i64)>,
        jobs: HashMap<Handle, bool>, // handle -> capped
        capped_pids: Vec<u32>,
        avail: u64,
        total: u64,
        ws: u64,
        set_ws: Option<(u64, bool)>,
        refuse: bool,
        next: isize,
        last_weight: i32,
        lifted_names: Vec<String>,
    }

    struct Fake(Mutex<FS>);

    impl Fake {
        fn new() -> Self {
            Fake(Mutex::new(FS { avail: 16u64 << 30, total: 32u64 << 30, ws: 3u64 << 30, next: 1, last_weight: -1, ..Default::default() }))
        }
        fn s(&self) -> std::sync::MutexGuard<'_, FS> {
            lock(&self.0)
        }
        /// Adds cores×seconds of CPU time to a process.
        fn burn(&self, pid: u32, cores: f64, seconds: f64) {
            let mut s = self.s();
            let p = s.procs[&pid];
            s.procs.insert(pid, (p.0, p.1 + (cores * seconds * 1e7) as i64));
        }
    }

    impl IBurstNative for Fake {
        fn cpu_time(&self, pid: u32, created: &mut i64, cpu: &mut i64) -> bool {
            *created = 0;
            *cpu = 0;
            match self.s().procs.get(&pid) {
                None => false,
                Some(p) => {
                    *created = p.0;
                    *cpu = p.1;
                    true
                }
            }
        }
        fn cap(&self, pid: u32, _job_name: &str, _cpu_rate: u32, weight: i32, job: &mut Handle, error: &mut i32) -> bool {
            *job = Handle::NULL;
            *error = 5;
            let mut s = self.s();
            if s.refuse {
                return false;
            }
            s.last_weight = weight;
            *job = Handle(s.next);
            s.next += 1;
            s.jobs.insert(*job, true);
            s.capped_pids.push(pid);
            *error = 0;
            true
        }
        fn lift(&self, job: Handle) {
            let mut s = self.s();
            if let Some(j) = s.jobs.get_mut(&job) {
                *j = false;
            }
        }
        fn lift_by_name(&self, job_name: &str) -> bool {
            self.s().lifted_names.push(job_name.to_string());
            true
        }
        fn close(&self, _job: Handle) {}
        fn memory(&self, avail: &mut u64, total: &mut u64) -> bool {
            let s = self.s();
            *avail = s.avail;
            *total = s.total;
            true
        }
        fn working_set(&self, _pid: u32, ws: &mut u64, min: &mut u64, max: &mut u64) -> bool {
            *ws = self.s().ws;
            *min = 200 << 10;
            *max = 1380 << 10;
            true
        }
        fn set_working_set_min(&self, _pid: u32, min: u64, _max: u64, hard: bool) -> bool {
            self.s().set_ws = Some((min, hard));
            true
        }
    }

    struct M {
        g: BurstGuard,
        n: Arc<Fake>,
        t: Arc<Mutex<DateTime>>,
    }

    impl M {
        fn adv(&self, s: f64) {
            let mut t = lock(&self.t);
            *t += TimeSpan::from_secs(s);
        }
    }

    fn make() -> M {
        let n = Arc::new(Fake::new());
        let t = Arc::new(Mutex::new(DateTime::from_parts(2026, 11, 21, 12, 0, 0, Kind::Utc).unwrap()));
        let t2 = t.clone();
        let g = BurstGuard::new(Some(n.clone()), Some(Arc::new(move || *lock(&t2))));
        g.set_on(true);
        g.set_logical(16);
        g.set_log(Arc::new(|_, _| {}));
        M { g, n, t }
    }

    fn cands(list: &[(u32, &str)]) -> Vec<(u32, String)> {
        list.iter().map(|(p, n)| (*p, n.to_string())).collect()
    }

    #[test]
    fn policy_numbers() {
        assert_eq!((BurstPolicy::cores(20_000_000, 2.0) * 1000.0).round() / 1000.0, 1.0);
        assert_eq!(312, BurstPolicy::cpu_rate(0.5, 16)); // half a processor of 16 = 3.12% of the machine
        assert_eq!(10000, BurstPolicy::cpu_rate(64.0, 16));
        assert!(BurstPolicy::memory_pressure(1u64 << 30, 16u64 << 30)); // 1 GB free of 16
        assert!(!BurstPolicy::memory_pressure(8u64 << 30, 16u64 << 30));
        assert_eq!(4u64 << 30, BurstPolicy::game_minimum(6u64 << 30, 16u64 << 30)); // at most 25% of RAM
        assert_eq!(0, BurstPolicy::game_minimum(100u64 << 20, 16u64 << 30)); // too small to bother
    }

    #[test]
    fn only_a_program_heavy_for_three_samples_is_capped_and_released_with_the_game() {
        let m = make();
        for (p, c) in [(10, 1), (20, 2), (30, 3), (99, 9)] {
            m.n.s().procs.insert(p, (c, 0));
        }
        let c = cands(&[(20, "heavy.exe"), (30, "light.exe")]);
        m.g.tick(99, 9, true, &c, 10); // first sample: baseline only
        for _ in 0..2 {
            m.adv(2.0);
            m.n.burn(20, 1.5, 2.0);
            m.n.burn(30, 0.1, 2.0);
            m.g.tick(99, 9, true, &c, 10);
        }
        assert!(m.n.s().capped_pids.is_empty()); // two heavy samples are not enough
        m.adv(2.0);
        m.n.burn(20, 1.5, 2.0);
        m.n.burn(30, 0.1, 2.0);
        m.g.tick(99, 9, true, &c, 10);
        assert_eq!(vec![20], m.n.s().capped_pids);
        assert_eq!(1, m.g.capped_count());
        assert_eq!(BurstPolicy::SOFT_WEIGHT, m.n.s().last_weight); // soft mode by default: weight, not a hard cap
        m.g.release();
        assert_eq!(0, m.g.capped_count());
        assert!(m.n.s().jobs.values().all(|c| !*c));
    }

    #[test]
    fn network_boosters_and_the_foreground_are_never_capped() {
        let m = make();
        for (p, c) in [(20, 2), (21, 3), (99, 9)] {
            m.n.s().procs.insert(p, (c, 0));
        }
        let c = cands(&[(20, "ExitLag.exe"), (21, "busy.exe")]);
        m.g.tick(99, 9, true, &c, 21);
        for _ in 0..5 {
            m.adv(2.0);
            m.n.burn(20, 2.0, 2.0);
            m.n.burn(21, 2.0, 2.0);
            m.g.tick(99, 9, true, &c, 21);
        }
        assert!(m.n.s().capped_pids.is_empty());
    }

    #[test]
    fn graphics_driver_processes_and_the_measuring_tool_are_never_capped() {
        let m = make();
        for (p, c) in [(20, 2), (21, 3), (22, 4), (99, 9)] {
            m.n.s().procs.insert(p, (c, 0));
        }
        let c = cands(&[(20, "NVDisplay.Container.exe"), (21, "DomainSchedTest.exe"), (22, "RadeonSoftware.exe")]);
        m.g.tick(99, 9, true, &c, 1);
        for _ in 0..5 {
            m.adv(2.0);
            m.n.burn(20, 1.0, 2.0);
            m.n.burn(21, 1.0, 2.0);
            m.n.burn(22, 1.0, 2.0);
            m.g.tick(99, 9, true, &c, 1);
        }
        assert!(m.n.s().capped_pids.is_empty()); // 1.16.2: the driver's own processes and DomainSchedTest are left alone
    }

    #[test]
    fn a_capped_program_coming_to_the_front_is_released() {
        let m = make();
        m.n.s().procs.insert(20, (2, 0));
        m.n.s().procs.insert(99, (9, 0));
        let c = cands(&[(20, "busy.exe")]);
        m.g.tick(99, 9, true, &c, 1);
        for _ in 0..3 {
            m.adv(2.0);
            m.n.burn(20, 1.0, 2.0);
            m.g.tick(99, 9, true, &c, 1);
        }
        assert_eq!(1, m.g.capped_count());
        m.adv(2.0);
        m.g.tick(99, 9, true, &c, 20);
        assert_eq!(0, m.g.capped_count());
    }

    #[test]
    fn game_memory_is_held_only_under_pressure_and_only_when_the_game_may_be_touched() {
        let m = make();
        m.n.s().procs.insert(99, (9, 0));
        m.g.tick(99, 9, true, &[], 1);
        assert_eq!(None, m.n.s().set_ws); // plenty of free memory
        m.n.s().avail = 1u64 << 30;
        m.adv(2.0);
        m.g.tick(99, 9, false, &[], 1);
        assert_eq!(None, m.n.s().set_ws); // the game may not be touched (anti-cheat)
        m.adv(2.0);
        m.g.tick(99, 9, true, &[], 1);
        assert_eq!(Some((3u64 << 30, true)), m.n.s().set_ws);
        m.g.release();
        assert!(!m.n.s().set_ws.unwrap().1); // put back with the game
    }

    #[test]
    fn off_does_nothing_and_a_refused_cap_is_not_retried() {
        let m = make();
        m.n.s().procs.insert(20, (2, 0));
        m.n.s().procs.insert(99, (9, 0));
        m.n.s().refuse = true;
        let c = cands(&[(20, "busy.exe")]);
        m.g.tick(99, 9, true, &c, 1);
        for _ in 0..8 {
            m.adv(2.0);
            m.n.burn(20, 1.0, 2.0);
            m.g.tick(99, 9, true, &c, 1);
        }
        assert_eq!(0, m.g.capped_count());
        m.g.set_on(false);
        m.n.s().refuse = false;
        m.n.s().avail = 1;
        m.adv(2.0);
        m.g.tick(99, 9, true, &c, 1);
        assert_eq!(None, m.n.s().set_ws);
    }

    #[test]
    fn hard_mode_uses_the_cap() {
        let m = make();
        m.g.set_soft(false);
        m.n.s().procs.insert(20, (2, 0));
        m.n.s().procs.insert(99, (9, 0));
        let c = cands(&[(20, "busy.exe")]);
        m.g.tick(99, 9, true, &c, 1);
        for _ in 0..3 {
            m.adv(2.0);
            m.n.burn(20, 1.0, 2.0);
            m.g.tick(99, 9, true, &c, 1);
        }
        assert_eq!(0, m.n.s().last_weight);
    }

    #[test]
    fn with_the_probe_the_game_memory_is_held_on_hard_faults_and_lifted_when_it_does_not_help() {
        let m = make();
        m.n.s().procs.insert(99, (9, 0));
        let hf = Arc::new(Mutex::new(Some(5.0)));
        let h2 = hf.clone();
        m.g.set_game_hard_faults(Some(Arc::new(move || *lock(&h2))));
        m.n.s().avail = 1; // little free memory, but the game does not fault: no hold
        for _ in 0..3 {
            m.adv(2.0);
            m.g.tick(99, 9, true, &[], 1);
        }
        assert_eq!(None, m.n.s().set_ws);
        *lock(&hf) = Some(80.0); // the game reads its pages back from disk
        m.adv(2.0);
        m.g.tick(99, 9, true, &[], 1);
        assert_eq!(None, m.n.s().set_ws); // one sample is not enough
        m.adv(2.0);
        m.g.tick(99, 9, true, &[], 1);
        assert!(m.n.s().set_ws.unwrap().1);
        *lock(&hf) = Some(70.0); // 10 s later still high: the hold did not help, lifted
        for _ in 0..6 {
            m.adv(2.0);
            m.g.tick(99, 9, true, &[], 1);
        }
        assert!(!m.n.s().set_ws.unwrap().1);
    }

    /// Not a C# test: the saved job names have the C# JSON shape and are lifted at the next start.
    #[test]
    fn caps_left_by_a_crash_are_lifted_at_the_next_start() {
        let m = make();
        let dir = std::env::temp_dir().join(format!("ds-burst-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("burst-guard.json");
        m.g.set_state_path(Some(path.clone()));
        m.n.s().procs.insert(20, (2, 0));
        m.n.s().procs.insert(99, (9, 0));
        let c = cands(&[(20, "busy.exe")]);
        m.g.tick(99, 9, true, &c, 1);
        for _ in 0..3 {
            m.adv(2.0);
            m.n.burn(20, 1.0, 2.0);
            m.g.tick(99, 9, true, &c, 1);
        }
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(r#"{"Caps":[{"Pid":20,"Created":2,"Job":"DomainOS.BurstGuard.20.2"}]}"#, text);
        assert_eq!(1, m.g.recover_after_crash());
        assert_eq!(vec!["DomainOS.BurstGuard.20.2".to_string()], m.n.s().lifted_names);
        assert!(!path.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
