//! Port of `EtwIrq.cs`.
//!
//! Live DPC/ISR time per processor from Windows' own kernel trace, run only while a game is handled. Two uses:
//!  - busy interrupt cores: a physical core whose processor spends more than 4% of its time in DPC/ISR (over the
//!    last 10 s) counts as busy until it falls under 2%, and a new set is accepted only after it held 20 s, so the
//!    game mask does not flap. Auto leaves busy cores (both SMT threads) out of the game mask: with the narrow rule
//!    (default) only the busiest of them, next to the one strongest graphics-driver core;
//!  - the latency watchdog: DPCs and interrupts of 0.5 ms and longer are counted per driver and logged once a minute.
//! Nothing is changed by this class; the session stops when the game ends and on exit.

use super::drivers::{driver_of, drivers};
use super::qpc;
use super::raw::{self, FastMap, SessionSpec, PERF_INFO_GUID};
use crate::base::app_log::{AppLog, Sev};
use crate::base::ci;
use crate::base::hooks::lock;
use crate::base::l::L;
use crate::base::time::{DateTime, TICKS_PER_SECOND};
use crate::lf;
use crate::topo::builder::TopoBuilder;
use crate::topo::masks::Masks;
use crate::topo::model::Topo;
use indexmap::IndexMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};

pub type NowFn = Arc<dyn Fn() -> DateTime + Send + Sync>;

const WS: usize = 10;

#[derive(Clone, Copy, Debug, Default)]
struct LongStat {
    count: i64,
    very_long: i64,
    max_ms: f64,
}

struct State {
    /// per processor, per second slot
    ms: [[f64; WS]; 64],
    /// the graphics driver's part of it
    gpu_ms: [[f64; WS]; 64],
    gpu_routine: FastMap<u64, bool>,
    slot_second: [i64; WS],
    busy: u64,
    candidate: u64,
    candidate_since: DateTime,
    started_at: DateTime,
    ready: bool,
    gpu_logged: u64,
    gpu_seen: [DateTime; 64],
    long: IndexMap<u64, LongStat>,
}

struct Run {
    handle: u64,
    trace: u64,
    thread: Option<std::thread::JoinHandle<()>>,
}

pub(crate) struct Inner {
    gate: Mutex<State>,
    drivers: RwLock<Arc<Vec<(u64, String)>>>,
    available: AtomicBool,
    error: Mutex<String>,
    now: RwLock<NowFn>,
    /// a test clock was set: the callback asks `now`; otherwise it reads the system clock directly (no lock, no
    /// reference count per event)
    custom_now: AtomicBool,
    /// ms per QPC tick (f64 bits)
    us_per_tick: AtomicU64,
    run: Mutex<Run>,
}

/// `EtwIrq` (IDisposable: call `dispose`).
pub struct EtwIrq {
    inner: Arc<Inner>,
}

impl Default for EtwIrq {
    fn default() -> Self {
        Self::new()
    }
}

impl EtwIrq {
    pub const WINDOW_SECONDS: i32 = 10;
    pub const NOISY_SHARE: f64 = 0.02;
    pub const BUSY_ENTER: f64 = 0.04;
    pub const BUSY_LEAVE: f64 = 0.02;
    pub const BUSY_HOLD_SECONDS: i32 = 20;
    pub const LONG_MS: f64 = 0.5;
    pub const VERY_LONG_MS: f64 = 1.0;
    /// Graphics-driver cores: a core whose processors ran DPCs/ISRs of the graphics driver for at least 0.2% of the
    /// last 3 s. A game thread woken by that driver cannot start on that core until the DPC ends (~80 us measured), so
    /// the game keeps off it, and from the third second of a game, not after the 30 s of the busy-core rule.
    pub const GPU_SHARE: f64 = 0.002;
    pub const GPU_SECONDS: i32 = 3;
    /// 1.16.1 Test 6: a graphics-driver core stays marked until the driver has been quiet on it for this long, and a
    /// busy interrupt core is let go only after it stayed calm this long (it is still taken after 20 s). The owner's
    /// log showed both sets flipping every 5-45 s in CS2, and every flip may move the game's Top-5 threads.
    pub const KEEP_SECONDS: i32 = 60;
    const GPU_DRIVERS: [&'static str; 6] = ["nvlddmkm", "amdkmdag", "atikmdag", "igdkmd", "dxgkrnl", "dxgmms"];
    const SESSION_NAME: &'static str = "DomainScheduler-Irq";
    const FLAGS: u32 = 0x20 /* DPC */ | 0x40 /* INTERRUPT */;
    const SESSION_GUID: [u8; 16] = raw::guid_bytes(0x6b0f3c2e_1d4a_4c8e_9f21_7a5e3b9d4c18);

    pub fn new() -> Self {
        EtwIrq {
            inner: Arc::new(Inner {
                gate: Mutex::new(State {
                    ms: [[0.0; WS]; 64],
                    gpu_ms: [[0.0; WS]; 64],
                    gpu_routine: FastMap::default(),
                    slot_second: [0; WS],
                    busy: 0,
                    candidate: 0,
                    candidate_since: DateTime::default(),
                    started_at: DateTime::default(),
                    ready: false,
                    gpu_logged: u64::MAX,
                    gpu_seen: [DateTime::default(); 64],
                    long: IndexMap::new(),
                }),
                drivers: RwLock::new(Arc::new(Vec::new())),
                available: AtomicBool::new(false),
                error: Mutex::new(String::new()),
                now: RwLock::new(Arc::new(DateTime::utc_now)),
                custom_now: AtomicBool::new(false),
                us_per_tick: AtomicU64::new(0f64.to_bits()),
                run: Mutex::new(Run { handle: 0, trace: 0, thread: None }),
            }),
        }
    }

    /// `Now` (tests set their clock).
    pub fn set_now(&self, f: NowFn) {
        *self.inner.now.write().unwrap_or_else(|e| e.into_inner()) = f;
        self.inner.custom_now.store(true, Ordering::SeqCst);
    }

    fn now(&self) -> DateTime {
        let f = self.inner.now.read().unwrap_or_else(|e| e.into_inner()).clone();
        f()
    }

    pub fn available(&self) -> bool {
        self.inner.available.load(Ordering::SeqCst)
    }

    pub fn error(&self) -> String {
        lock(&self.inner.error).clone()
    }

    /// Starts the tracer; false (with one warning) when it cannot run: the registry pins are used alone.
    pub fn start(&self) -> bool {
        if self.available() {
            return true;
        }
        match self.try_start() {
            Ok(()) => {
                self.inner.available.store(true, Ordering::SeqCst);
                AppLog::add(L::t("Live DPC/ISR: on"));
                true
            }
            Err(msg) => {
                *lock(&self.inner.error) = msg.clone();
                self.stop(false);
                AppLog::add_sev(lf!("Live DPC/ISR is not available ({0}): only the registry pins are used", msg.as_str()), Sev::Warn);
                false
            }
        }
    }

    fn try_start(&self) -> Result<(), String> {
        self.load_drivers();
        self.inner.us_per_tick.store((1e3 / qpc::frequency() as f64).to_bits(), Ordering::SeqCst); // ms per QPC tick
        raw::stop_by_name(Self::SESSION_NAME);
        // A private system-logger session (Windows 8+): the shared "NT Kernel Logger" stays free for other tools.
        let spec = SessionSpec {
            guid: Self::SESSION_GUID,
            mode: 0x100 | 0x02000000, // REAL_TIME | SYSTEM_LOGGER
            buffer_kb: 256,
            min_buffers: 8,
            // at most 8 MB of kernel memory, also if a killed copy left the session behind
            max_buffers: 32,
            flags: Self::FLAGS,
        };
        let (rc, handle) = raw::start(Self::SESSION_NAME, &spec);
        lock(&self.inner.run).handle = handle;
        if rc == 5 {
            return Err(L::t("no administrator rights").to_string());
        }
        if rc != 0 {
            return Err(lf!("error {0}", rc));
        }
        let rc = raw::set_flags(handle, Self::FLAGS);
        if rc != 0 {
            raw::stop_by_name(Self::SESSION_NAME);
            return Err(lf!("error {0}", rc));
        }
        let ctx = Arc::as_ptr(&self.inner) as *const core::ffi::c_void;
        let trace = raw::open(Self::SESSION_NAME, on_event, ctx);
        lock(&self.inner.run).trace = trace;
        if trace == u64::MAX {
            raw::stop_by_name(Self::SESSION_NAME);
            return Err(L::t("the session could not be opened").to_string());
        }
        let keep = self.inner.clone();
        let th = raw::spawn_process("etw-irq", -1, Some(L::n("DPC/ISR trace")), trace, keep);
        lock(&self.inner.run).thread = th;
        Ok(())
    }

    // ---- raw ETW: one pointer read per field, no event objects --------------------------------------------------

    /// `Add(cpu, ms)`.
    pub fn add(&self, cpu: i32, ms: f64) {
        self.add_routine(cpu, ms, 0);
    }

    /// `Add(cpu, ms, routine)`.
    pub fn add_routine(&self, cpu: i32, ms: f64, routine: u64) {
        self.inner.add(cpu, ms, routine, self.now());
    }

    /// DPC + ISR share of every processor over the last window (0..1).
    pub fn shares(&self) -> [f64; 64] {
        let mut shares = [0.0; 64];
        if !self.available() {
            return shares;
        }
        let sec = self.now().ticks / TICKS_PER_SECOND;
        let st = lock(&self.inner.gate);
        for (c, share) in shares.iter_mut().enumerate() {
            let mut total = 0.0;
            for s in 0..WS {
                if sec - st.slot_second[s] < Self::WINDOW_SECONDS as i64 {
                    total += st.ms[c][s];
                }
            }
            *share = total / (Self::WINDOW_SECONDS as f64 * 1000.0);
        }
        shares
    }

    /// Mask of the processors whose DPC + ISR share over the last 10 s is above 2%.
    pub fn noisy(&self) -> u64 {
        let sh = self.shares();
        let mut mask = 0u64;
        for (c, v) in sh.iter().enumerate() {
            if *v > Self::NOISY_SHARE {
                mask |= 1u64 << c;
            }
        }
        mask
    }

    // ---- busy interrupt cores (hysteresis + hold) ---------------------------------------------------------------

    /// Cores the game keeps off: the busy interrupt cores once their set has held, plus the graphics-driver cores as
    /// soon as 3 s of trace exist. None while neither is known (callers keep their old rule: the registry pins).
    pub fn busy_cores_if_ready(&self, t: Option<&Topo>) -> Option<u64> {
        let extra = self.busy_cores(t);
        let gpu = self.gpu_cores(t);
        let now = self.now();
        let mut st = lock(&self.inner.gate);
        let gpu_ready = st.started_at != DateTime::default() && (now - st.started_at).total_seconds() >= Self::GPU_SECONDS as f64;
        if gpu_ready && gpu != st.gpu_logged {
            st.gpu_logged = gpu;
            AppLog::add(lf!("Graphics driver cores (game keeps off them): {0}", if gpu == 0 { L::t("none").to_string() } else { Masks::ranges(gpu) }));
        }
        if st.ready {
            return Some(extra | gpu);
        }
        if gpu_ready && gpu != 0 { Some(gpu) } else { None }
    }

    /// The graphics driver's DPC/ISR share of every processor over the last 3 s (0..1).
    fn gpu_shares(&self) -> [f64; 64] {
        let mut share = [0.0; 64];
        if !self.available() {
            return share;
        }
        let sec = self.now().ticks / TICKS_PER_SECOND;
        let st = lock(&self.inner.gate);
        for (c, v) in share.iter_mut().enumerate() {
            let mut total = 0.0;
            for sl in 0..WS {
                if sec - st.slot_second[sl] < Self::GPU_SECONDS as i64 {
                    total += st.gpu_ms[c][sl];
                }
            }
            *v = total / (Self::GPU_SECONDS as f64 * 1000.0);
        }
        share
    }

    /// Whole physical cores whose processors ran the graphics driver's DPCs/ISRs >= 0.2% of the last 3 s.
    pub fn gpu_cores(&self, t: Option<&Topo>) -> u64 {
        let Some(t) = t else { return 0 };
        if !self.available() {
            return 0;
        }
        let share = self.gpu_shares();
        let now = self.now();
        let mut m = 0u64;
        let mut st = lock(&self.inner.gate);
        for core in cores(t).values() {
            let cm = TopoBuilder::mask(core.iter().copied());
            let first = core.iter().map(|x| x.lp).min().unwrap_or(0) as usize;
            let max = core.iter().map(|x| share[x.lp as usize]).fold(f64::NEG_INFINITY, f64::max);
            if max >= Self::GPU_SHARE {
                st.gpu_seen[first] = now;
            }
            let seen = st.gpu_seen[first];
            if seen != DateTime::default() && (now - seen).total_seconds() < Self::KEEP_SECONDS as f64 {
                m |= cm;
            }
        }
        m
    }

    /// Tests: driver load addresses without Windows.
    pub fn set_drivers(&self, drivers: Vec<(u64, String)>) {
        *self.inner.drivers.write().unwrap_or_else(|e| e.into_inner()) = Arc::new(drivers);
        lock(&self.inner.gate).gpu_routine.clear();
    }

    /// Whole physical cores (all their processors) that are busy with interrupts: enter above 4%, leave under 2%,
    /// a changed set is accepted after it held 20 s. 0 while the tracer is off or has no full window yet.
    pub fn busy_cores(&self, t: Option<&Topo>) -> u64 {
        let Some(t) = t else { return 0 };
        if !self.available() {
            return 0;
        }
        let sh = self.shares();
        let busy_now = lock(&self.inner.gate).busy;
        let mut next = 0u64;
        for core in cores(t).values() {
            let core_mask = TopoBuilder::mask(core.iter().copied());
            let max = core.iter().map(|s| sh[s.lp as usize]).fold(f64::NEG_INFINITY, f64::max);
            let was = (busy_now & core_mask) != 0;
            if max >= Self::BUSY_ENTER || (was && max >= Self::BUSY_LEAVE) {
                next |= core_mask;
            }
        }
        let now = self.now();
        let mut st = lock(&self.inner.gate);
        if st.started_at == DateTime::default() {
            st.started_at = now;
        }
        if !st.ready
            && (now - st.started_at).total_seconds() >= (Self::WINDOW_SECONDS + Self::BUSY_HOLD_SECONDS) as f64
            && next == st.candidate
            && (now - st.candidate_since).total_seconds() >= Self::BUSY_HOLD_SECONDS as f64
        {
            st.ready = true;
            st.busy = next;
            AppLog::add(lf!("Interrupt cores busy (game keeps off them): {0}", if next == 0 { L::t("none").to_string() } else { Masks::ranges(next) }));
            return st.busy;
        }
        if next == st.busy && st.ready {
            st.candidate = next;
            return st.busy;
        }
        if next != st.candidate || st.candidate_since == DateTime::default() {
            st.candidate = next;
            st.candidate_since = now;
            return st.busy;
        }
        // a core joins after the new set held 20 s; a core leaves only after the smaller set held 60 s
        let only_leaving = (next & !st.busy) == 0;
        if st.ready && (now - st.candidate_since).total_seconds() >= (if only_leaving { Self::KEEP_SECONDS } else { Self::BUSY_HOLD_SECONDS }) as f64 {
            let old = st.busy;
            st.busy = next;
            if old != next {
                AppLog::add(lf!("Interrupt cores busy (game keeps off them): {0}", if next == 0 { L::t("none").to_string() } else { Masks::ranges(next) }));
            }
        }
        st.busy
    }

    // ---- latency watchdog ---------------------------------------------------------------------------------------

    fn load_drivers(&self) {
        *self.inner.drivers.write().unwrap_or_else(|e| e.into_inner()) = Arc::new(drivers());
    }

    /// Long DPCs/ISRs since the last call, per driver: (driver, >=0.5 ms, >=1 ms, longest ms), longest first.
    pub fn take_long(&self) -> Vec<(String, i64, i64, f64)> {
        let copy: IndexMap<u64, LongStat> = {
            let mut st = lock(&self.inner.gate);
            std::mem::take(&mut st.long)
        };
        let drivers = self.inner.drivers.read().unwrap_or_else(|e| e.into_inner()).clone();
        let mut groups: IndexMap<String, (i64, i64, f64)> = IndexMap::new();
        for (k, v) in &copy {
            let d = driver_of(&drivers, *k);
            let e = groups.entry(d).or_insert((0, 0, f64::NEG_INFINITY));
            e.0 += v.count;
            e.1 += v.very_long;
            if v.max_ms > e.2 {
                e.2 = v.max_ms;
            }
        }
        let mut list: Vec<(String, i64, i64, f64)> = groups.into_iter().map(|(d, (a, b, c))| (d, a, b, c)).collect();
        // OrderByDescending: stable
        list.sort_by(|a, b| b.3.partial_cmp(&a.3).unwrap_or(std::cmp::Ordering::Equal));
        list
    }

    /// Tests: the tracer counts as running.
    pub fn mark_available_for_tests(&self) {
        self.inner.available.store(true, Ordering::SeqCst);
    }

    fn stop(&self, log: bool) {
        let (trace, thread) = {
            let mut r = lock(&self.inner.run);
            (r.trace, r.thread.take())
        };
        if self.available() || trace != 0 {
            raw::stop_by_name(Self::SESSION_NAME);
            if trace != 0 && trace != u64::MAX {
                raw::close(trace);
            }
            raw::join_timeout(thread, 2000);
        }
        lock(&self.inner.run).trace = 0;
        if self.available() && log {
            AppLog::add(L::t("Live DPC/ISR: off"));
        }
        self.inner.available.store(false, Ordering::SeqCst);
        let mut st = lock(&self.inner.gate);
        st.busy = 0;
        st.candidate = 0;
        st.ready = false;
        st.started_at = DateTime::default();
        st.candidate_since = DateTime::default();
        st.long.clear();
        st.ms = [[0.0; WS]; 64];
        st.gpu_ms = [[0.0; WS]; 64];
        st.gpu_logged = u64::MAX;
    }

    /// A session left by a killed copy of the program is stopped (called once at start).
    pub fn stop_orphan() {
        raw::stop_by_name(Self::SESSION_NAME);
    }

    /// Stops the scheduler trace an earlier test build (Domain Engine, removed) may have left running after a crash.
    pub fn stop_orphan_sched() {
        raw::stop_by_name("DomainScheduler-Sched");
    }

    /// Stops the session (the game ended); Start can be called again.
    pub fn pause(&self) {
        self.stop(true);
    }

    pub fn dispose(&self) {
        self.stop(true);
    }
}

/// `t.Sets.Where(x => x.Group == 0 && x.LP < 64).GroupBy(x => x.Core)`, in first-seen order.
fn cores(t: &Topo) -> IndexMap<u8, Vec<&crate::base::model::CpuSet>> {
    let mut g: IndexMap<u8, Vec<&crate::base::model::CpuSet>> = IndexMap::new();
    for s in t.sets.iter().filter(|s| s.group == 0 && s.lp < 64) {
        g.entry(s.core).or_default().push(s);
    }
    g
}

impl Inner {
    fn add(&self, cpu: i32, ms: f64, routine: u64, now: DateTime) {
        if !(0..64).contains(&cpu) || ms <= 0.0 {
            return;
        }
        let sec = now.ticks / TICKS_PER_SECOND;
        let slot = (sec % EtwIrq::WINDOW_SECONDS as i64) as usize;
        let cpu = cpu as usize;
        let mut st = lock(&self.gate);
        if st.slot_second[slot] != sec {
            st.slot_second[slot] = sec;
            for c in 0..64 {
                st.ms[c][slot] = 0.0;
                st.gpu_ms[c][slot] = 0.0;
            }
        }
        st.ms[cpu][slot] += ms;
        if routine != 0 {
            let gpu = match st.gpu_routine.get(&routine) {
                Some(g) => *g,
                None => {
                    let drivers = self.drivers.read().unwrap_or_else(|e| e.into_inner()).clone();
                    let drv = driver_of(&drivers, routine);
                    let g = EtwIrq::GPU_DRIVERS.iter().any(|g| ci::starts_with(&drv, g));
                    st.gpu_routine.insert(routine, g);
                    g
                }
            };
            if gpu {
                st.gpu_ms[cpu][slot] += ms;
            }
        }
        if ms >= EtwIrq::LONG_MS && ms < 1000.0 {
            let s = st.long.entry(routine).or_default();
            s.count += 1;
            if ms >= EtwIrq::VERY_LONG_MS {
                s.very_long += 1;
            }
            if ms > s.max_ms {
                s.max_ms = ms;
            }
        }
    }
}

/// The ETW callback (EVENT_RECORD): PerfInfo DPC kinds (66..70), ISR (50).
unsafe extern "system" fn on_event(record: *mut core::ffi::c_void) {
    let r = raw::rec(record as *const u8);
    if r.context.is_null() || r.provider != PERF_INFO_GUID {
        return;
    }
    let op = r.opcode;
    if op != 66 && op != 67 && op != 68 && op != 69 && op != 70 && op != 50 {
        return; // DPC kinds, ISR, ISR-MSI
    }
    if r.user.is_null() || r.len < 16 {
        return;
    }
    let initial: i64 = raw::rd(r.user, 0); // InitialTime (QPC)
    let routine: u64 = raw::rd(r.user, 8);
    let inner = &*(r.context as *const Inner);
    let per_tick = f64::from_bits(inner.us_per_tick.load(Ordering::Relaxed));
    let now = if inner.custom_now.load(Ordering::Relaxed) {
        let f = inner.now.read().unwrap_or_else(|e| e.into_inner()).clone();
        f()
    } else {
        DateTime::utc_now()
    };
    inner.add(r.cpu as i32, (r.stamp - initial) as f64 * per_tick, routine, now);
}

/// Benchmarks (feature "testsupport"): the real ETW callback, without a session.
#[cfg(any(test, feature = "testsupport"))]
impl EtwIrq {
    pub fn bench_context(&self) -> *const core::ffi::c_void {
        self.inner.us_per_tick.store((1e3 / qpc::frequency() as f64).to_bits(), Ordering::SeqCst);
        Arc::as_ptr(&self.inner) as *const core::ffi::c_void
    }
    /// # Safety
    /// `record` points to an EVENT_RECORD whose UserContext is `bench_context()` of a live tracer.
    pub unsafe fn bench_event(record: *mut core::ffi::c_void) {
        on_event(record)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::base::hooks::static_hooks;
    use crate::base::time::Kind;
    use crate::topo::game_mask::{GameMask, IRQ_HOOK, NOISY_HOOK};
    use crate::topo::synth;

    fn clock() -> (Arc<Mutex<DateTime>>, NowFn) {
        let now = Arc::new(Mutex::new(DateTime::from_parts(2026, 1, 1, 12, 0, 0, Kind::Utc).unwrap()));
        let n2 = now.clone();
        (now, Arc::new(move || *lock(&n2)))
    }

    fn adv(now: &Arc<Mutex<DateTime>>, s: f64) {
        let mut g = lock(now);
        *g = g.add_seconds(s);
    }

    /// RoadmapTests.LiveDpcIsrChangesTheMaskOnlyWhenItIsOn
    #[test]
    fn live_dpc_isr_changes_the_mask_only_when_it_is_on() {
        let _g = static_hooks();
        let x = synth::x7950x3d();
        IRQ_HOOK.set(Some(Arc::new(|| vec![("GPU".to_string(), 0x30u64)])));
        let registry_only = GameMask::for_rule(&x, 0xFFFF, GameMask::CORE_MODE);

        let (now, f) = clock();
        let irq = Arc::new(EtwIrq::new());
        irq.set_now(f);
        irq.mark_available_for_tests();
        for _ in 0..10 {
            irq.add(8, 30.0); // 3% of processor 8
            irq.add(12, 10.0); // 1% of processor 12: quiet
            adv(&now, 1.0);
        }
        assert_eq!(0x100u64, irq.noisy());

        NOISY_HOOK.set(None); // EtwIrq = false
        let off = GameMask::for_rule(&x, 0xFFFF, GameMask::CORE_MODE);
        assert_eq!(registry_only.final_, off.final_);
        assert_eq!(registry_only.irq, off.irq);

        let i2 = irq.clone();
        NOISY_HOOK.set(Some(Arc::new(move || i2.noisy()))); // EtwIrq = true
        let on = GameMask::for_rule(&x, 0xFFFF, GameMask::CORE_MODE);
        assert_eq!(registry_only.final_ & !0x300u64, on.final_); // the whole core of processor 8 is stepped around
        adv(&now, 30.0); // the noise is over
        assert_eq!(registry_only.final_, GameMask::for_rule(&x, 0xFFFF, GameMask::CORE_MODE).final_);
        NOISY_HOOK.set(None);
        IRQ_HOOK.set(None);
    }

    /// RoadmapTests.BusyInterruptCoresUseHysteresisAndHold
    #[test]
    fn busy_interrupt_cores_use_hysteresis_and_hold() {
        let x = synth::x7800x3d();
        let (now, f) = clock();
        let irq = EtwIrq::new();
        irq.set_now(f);
        irq.mark_available_for_tests();
        let mut got = None;
        for s in 0..40 {
            irq.add(12, 100.0); // 10% of processor 12 (core 6)
            irq.add(14, 100.0); // 10% of processor 14 (core 7)
            irq.add(2, 10.0); // 1% of processor 2: quiet
            got = irq.busy_cores_if_ready(Some(&x));
            if s < 25 {
                assert_eq!(None, got); // no verdict before a full window and the hold
            }
            adv(&now, 1.0);
        }
        assert_eq!(Some(0xF000u64), got); // both cores, both SMT threads
        for _ in 0..40 {
            // 3%: under "enter", above "leave": stays busy
            irq.add(12, 30.0);
            irq.add(14, 30.0);
            assert_eq!(Some(0xF000u64), irq.busy_cores_if_ready(Some(&x)));
            adv(&now, 1.0);
        }
        for _ in 0..60 {
            // quiet: still kept for 60 s (Test 6), no flip-flop
            irq.add(12, 5.0);
            assert_eq!(Some(0xF000u64), irq.busy_cores_if_ready(Some(&x)));
            adv(&now, 1.0);
        }
        for _ in 0..15 {
            irq.add(12, 5.0);
            irq.busy_cores_if_ready(Some(&x));
            adv(&now, 1.0);
        }
        assert_eq!(Some(0u64), irq.busy_cores_if_ready(Some(&x))); // quiet for longer than window + 60 s
    }

    /// RoadmapTests.GraphicsDriverCoresAreAvoidedFromTheThirdSecond
    #[test]
    fn graphics_driver_cores_are_avoided_from_the_third_second() {
        let x = synth::x7800x3d();
        let (now, f) = clock();
        let irq = EtwIrq::new();
        irq.set_now(f);
        irq.mark_available_for_tests();
        irq.set_drivers(vec![(0x1000, "nvlddmkm.sys".into()), (0x9000000, "storport.sys".into())]);
        let mut got = None;
        for s in 0..6 {
            irq.add_routine(4, 6.0, 0x1500); // the graphics driver: 0.6% of processor 4 (core 2)
            irq.add_routine(9, 6.0, 0x9000100); // another driver, same share: not a graphics core
            got = irq.busy_cores_if_ready(Some(&x));
            if s < EtwIrq::GPU_SECONDS {
                assert_eq!(None, got); // nothing known before 3 s of trace
            }
            adv(&now, 1.0);
        }
        assert_eq!(Some(0x30u64), got); // the whole core 2 (processors 4 and 5), long before the busy-core rule
        for _ in 0..5 {
            irq.add_routine(4, 0.5, 0x1500);
            irq.busy_cores_if_ready(Some(&x));
            adv(&now, 1.0);
        }
        assert_eq!(0x30u64, irq.gpu_cores(Some(&x))); // quiet for 5 s: still kept (Test 6: 60 s, no flip-flop)
        for _ in 0..60 {
            irq.add_routine(4, 0.5, 0x1500);
            irq.busy_cores_if_ready(Some(&x));
            adv(&now, 1.0);
        }
        assert_eq!(0u64, irq.gpu_cores(Some(&x))); // quiet (0.05%) for over 60 s: the core is free again
    }

    #[test]
    fn long_dpcs_are_grouped_per_driver_longest_first() {
        let (_now, f) = clock();
        let irq = EtwIrq::new();
        irq.set_now(f);
        irq.set_drivers(vec![(0x1000, "nvlddmkm.sys".into()), (0x9000000, "storport.sys".into())]);
        irq.add_routine(1, 0.6, 0x1500);
        irq.add_routine(1, 1.2, 0x1600);
        irq.add_routine(2, 2.5, 0x9000100);
        irq.add_routine(2, 0.4, 0x9000100); // not long
        let l = irq.take_long();
        assert_eq!(l.len(), 2);
        assert_eq!(l[0], ("storport.sys".to_string(), 1, 1, 2.5));
        assert_eq!(l[1], ("nvlddmkm.sys".to_string(), 2, 1, 1.2));
        assert!(irq.take_long().is_empty());
    }
}
