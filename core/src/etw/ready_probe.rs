//! Port of `ReadyProbe.cs`.
//!
//! The game's own pain, measured cheaply while Auto handles it (1.16.1 Test 2). One private kernel session runs with
//! only process and thread events (a few per second); every `ProbePolicy::PERIOD_MS` it switches on context
//! switches, ready threads and hard page faults for `ProbePolicy::WINDOW_MS` and off again, so the heavy events
//! flow about 12% of the time. From each window:
//!  - per game thread: the 99th percentile of its ready waits (ReadyThread -> the context switch that runs it), which
//!    Dynamic Top-5 uses to rank and move threads by what they WAIT, not only by what they use;
//!  - the game's hard page faults per second (a page read from disk because Windows had trimmed it), which the burst
//!    guard uses to decide whether holding the game's memory helps.
//! A window that lost events is thrown away. Nothing is changed; the trace only reads.

use super::qpc;
use super::raw::{self, FastBuild, FastMap, SessionSpec, PAGE_FAULT_GUID, THREAD_GUID};
use crate::base::hooks::lock;
use crate::base::l::L;
use crate::base::time::DateTime;
use crate::lf;
use indexmap::{IndexMap, IndexSet};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};

pub type NowFn = Arc<dyn Fn() -> DateTime + Send + Sync>;

/// `ProbePolicy`.
pub struct ProbePolicy;

impl ProbePolicy {
    pub const WINDOW_MS: i32 = 250;
    pub const PERIOD_MS: i32 = 2000;
    /// fewer waits in a window: no p99 for that thread
    pub const MIN_WAITS: usize = 3;
}

const NAME: &str = "DomainScheduler-Probe";
const SESSION_GUID: [u8; 16] = raw::guid_bytes(0x5e8b2d71_c4a9_4f36_b1d0_8a7e3c6f2b94);
const BASE_FLAGS: u32 = 0x1 | 0x2; // PROCESS | THREAD
const WINDOW_FLAGS: u32 = 0x10 | 0x800 | 0x2000; // CSWITCH | DISPATCHER | MEMORY_HARD_FAULTS

struct State {
    tid_pid: FastMap<u32, u32>,
    game_tids: IndexSet<u32, FastBuild>,
    ready: FastMap<u32, i64>,
    /// per game thread, the waits of the current window; the lists are emptied, not dropped, when a window opens, so
    /// the hot path does not allocate once they reached their working size (an empty list gives no p99)
    waits: IndexMap<u32, Vec<i64>, FastBuild>,
    faults: i32,
    window: bool,
    window_start: i64,
    pid: u32,
    p99: FastMap<u32, f64>,
    faults_per_sec: Option<f64>,
    lost_seen: u32,
}

struct Run {
    session: u64,
    trace: u64,
    thread: Option<std::thread::JoinHandle<()>>,
    next_window: DateTime,
}

pub(crate) struct Inner {
    gate: Mutex<State>,
    tpm: f64,
    running: AtomicBool,
    error: Mutex<String>,
    now: RwLock<NowFn>,
    run: Mutex<Run>,
    /// the closing timer of the current window (0 = none): a newer timer or Stop cancels the older one
    closer: AtomicU64,
}

/// `ReadyProbe` (IDisposable: call `dispose`).
pub struct ReadyProbe {
    inner: Arc<Inner>,
}

impl Default for ReadyProbe {
    fn default() -> Self {
        Self::new()
    }
}

impl ReadyProbe {
    pub fn new() -> Self {
        ReadyProbe {
            inner: Arc::new(Inner {
                gate: Mutex::new(State {
                    tid_pid: FastMap::default(),
                    game_tids: IndexSet::default(),
                    ready: FastMap::default(),
                    waits: IndexMap::default(),
                    faults: 0,
                    window: false,
                    window_start: 0,
                    pid: 0,
                    p99: FastMap::default(),
                    faults_per_sec: None,
                    lost_seen: 0,
                }),
                tpm: qpc::frequency() as f64 / 1000.0,
                running: AtomicBool::new(false),
                error: Mutex::new(String::new()),
                now: RwLock::new(Arc::new(DateTime::utc_now)),
                run: Mutex::new(Run { session: 0, trace: 0, thread: None, next_window: DateTime::default() }),
                closer: AtomicU64::new(0),
            }),
        }
    }

    pub fn set_now(&self, f: NowFn) {
        *self.inner.now.write().unwrap_or_else(|e| e.into_inner()) = f;
    }

    fn now(&self) -> DateTime {
        let f = self.inner.now.read().unwrap_or_else(|e| e.into_inner()).clone();
        f()
    }

    pub fn running(&self) -> bool {
        self.inner.running.load(Ordering::SeqCst)
    }

    pub fn error(&self) -> String {
        lock(&self.inner.error).clone()
    }

    pub fn game_pid(&self) -> u32 {
        lock(&self.inner.gate).pid
    }

    /// p99 ready wait (ms) of a game thread in the last good window; -1 when unknown.
    pub fn wait_p99_ms(&self, tid: u32) -> f64 {
        lock(&self.inner.gate).p99.get(&tid).copied().unwrap_or(-1.0)
    }

    /// The game's hard page faults per second in the last good window; None when unknown.
    pub fn hard_faults_per_sec(&self) -> Option<f64> {
        lock(&self.inner.gate).faults_per_sec
    }

    pub fn start(&self, pid: u32) {
        if self.running() && pid == self.game_pid() {
            return;
        }
        self.stop();
        {
            let mut st = lock(&self.inner.gate);
            st.pid = pid;
            st.tid_pid.clear();
            st.game_tids.clear();
            st.ready.clear();
            st.waits.clear();
            st.p99 = FastMap::default();
            st.faults_per_sec = None;
            st.window = false;
        }
        let ok = self.start_session();
        self.inner.running.store(ok, Ordering::SeqCst);
        lock(&self.inner.run).next_window = self.now();
    }

    pub fn stop(&self) {
        if !self.running() {
            return;
        }
        self.inner.running.store(false, Ordering::SeqCst);
        self.inner.closer.fetch_add(1, Ordering::SeqCst);
        raw::stop_by_name(NAME);
        let (t, th) = {
            let mut r = lock(&self.inner.run);
            (r.trace, r.thread.take())
        };
        if t != 0 && t != u64::MAX {
            raw::close(t);
        }
        raw::join_timeout(th, 2000);
        lock(&self.inner.run).trace = 0;
    }

    pub fn dispose(&self) {
        self.stop();
    }

    pub fn stop_orphan() {
        raw::stop_by_name(NAME);
    }

    /// About once a second: opens or closes a window and computes its results.
    pub fn tick(&self) {
        if !self.running() {
            return;
        }
        let now = self.now();
        let window = lock(&self.inner.gate).window;
        let next_window = lock(&self.inner.run).next_window;
        if !window && now >= next_window {
            {
                let mut st = lock(&self.inner.gate);
                st.ready.clear();
                reset_waits(&mut st.waits);
                st.faults = 0;
                st.window = true;
                st.window_start = qpc::timestamp();
            }
            let session = lock(&self.inner.run).session;
            raw::set_flags(session, BASE_FLAGS | WINDOW_FLAGS);
            lock(&self.inner.run).next_window = now.add_ms(ProbePolicy::PERIOD_MS as f64);
            // The hub ticks once a second; a one-shot timer closes the window after WindowMs, so the heavy events flow
            // only ~250 ms of every 2 s.
            let id = self.inner.closer.fetch_add(1, Ordering::SeqCst) + 1;
            let inner = self.inner.clone();
            let _ = std::thread::Builder::new().name("probe-close".into()).spawn(move || {
                std::thread::sleep(std::time::Duration::from_millis(ProbePolicy::WINDOW_MS as u64));
                if inner.closer.load(Ordering::SeqCst) == id {
                    inner.close();
                }
                // C#: a pool timer; here a short thread per window, counted under the probe's name, not "ended".
                crate::native::self_threads::SelfThreads::ended(L::n("Wait probe"));
            });
        }
    }

    /// `P99`: the 99th percentile (nearest rank) of the values.
    pub fn p99(v: &[i64]) -> i64 {
        let mut s = v.to_vec();
        s.sort();
        let n = s.len() as i64;
        let i = (((0.99 * n as f64).ceil() as i64) - 1).clamp(0, n - 1);
        s[i as usize]
    }

    // ---- events ---------------------------------------------------------------------------------------------------
    pub fn on_thread(&self, pid: u32, tid: u32) {
        self.inner.on_thread(pid, tid);
    }
    pub fn on_ready(&self, tid: u32, ts: i64) {
        self.inner.on_ready(tid, ts);
    }
    pub fn on_switch(&self, new_tid: u32, ts: i64) {
        self.inner.on_switch(new_tid, ts);
    }
    pub fn on_hard_fault(&self, tid: u32) {
        self.inner.on_hard_fault(tid);
    }

    fn start_session(&self) -> bool {
        raw::stop_by_name(NAME);
        let spec = SessionSpec { guid: SESSION_GUID, mode: 0x100 | 0x02000000, buffer_kb: 256, min_buffers: 32, max_buffers: 256, flags: 0 };
        let (rc, s) = raw::start(NAME, &spec);
        lock(&self.inner.run).session = s;
        if rc != 0 {
            *lock(&self.inner.error) = if rc == 5 { L::t("no administrator rights").to_string() } else { lf!("error {0}", rc) };
            return false;
        }
        if raw::set_flags(s, BASE_FLAGS) != 0 {
            raw::stop_by_name(NAME);
            return false;
        }
        let ctx = Arc::as_ptr(&self.inner) as *const core::ffi::c_void;
        let t = raw::open(NAME, on_event, ctx);
        lock(&self.inner.run).trace = t;
        if t == u64::MAX {
            raw::stop_by_name(NAME);
            *lock(&self.inner.error) = L::t("the session could not be opened").to_string();
            return false;
        }
        let th = raw::spawn_process("probe", -1, Some(L::n("Wait probe")), t, self.inner.clone());
        lock(&self.inner.run).thread = th;
        lock(&self.inner.gate).lost_seen = raw::lost(NAME);
        true
    }

    /// Tests: feed a window of synthetic events.
    pub fn test_window(&self, pid: u32, events: impl FnOnce(&ReadyProbe), seconds: f64) {
        {
            let mut st = lock(&self.inner.gate);
            st.pid = pid;
            st.window = true;
            st.window_start = qpc::timestamp() - (seconds * 1000.0 * self.inner.tpm) as i64;
            st.ready.clear();
            reset_waits(&mut st.waits);
            st.faults = 0;
        }
        events(self);
        let mut st = lock(&self.inner.gate);
        st.window = false;
        let tpm = self.inner.tpm;
        st.p99 = st.waits.iter().filter(|(_, v)| v.len() >= ProbePolicy::MIN_WAITS).map(|(k, v)| (*k, Self::p99(v) as f64 / tpm)).collect();
        st.faults_per_sec = Some(st.faults as f64 / seconds);
    }
}

impl Inner {
    /// Ends the window: the heavy events go off and the window's results are computed.
    fn close(&self) {
        if !self.running.load(Ordering::SeqCst) {
            return;
        }
        let session = lock(&self.run).session;
        raw::set_flags(session, BASE_FLAGS);
        let lost = raw::lost(NAME);
        let mut st = lock(&self.gate);
        st.window = false;
        let sec = (qpc::timestamp() - st.window_start) as f64 / self.tpm / 1000.0;
        if lost != st.lost_seen {
            st.lost_seen = lost;
            return; // events were lost: this window is not trusted
        }
        let tpm = self.tpm;
        st.p99 = st.waits.iter().filter(|(_, v)| v.len() >= ProbePolicy::MIN_WAITS).map(|(k, v)| (*k, ReadyProbe::p99(v) as f64 / tpm)).collect();
        st.faults_per_sec = if sec > 0.0 { Some(st.faults as f64 / sec) } else { None };
    }

    fn on_thread(&self, pid: u32, tid: u32) {
        let mut st = lock(&self.gate);
        st.tid_pid.insert(tid, pid);
        if pid == st.pid {
            st.game_tids.insert(tid);
        }
    }

    fn on_ready(&self, tid: u32, ts: i64) {
        let mut st = lock(&self.gate);
        if st.window && st.game_tids.contains(&tid) {
            st.ready.entry(tid).or_insert(ts);
        }
    }

    fn on_switch(&self, new_tid: u32, ts: i64) {
        let mut st = lock(&self.gate);
        if st.window && new_tid != 0 {
            if let Some(at) = st.ready.remove(&new_tid) {
                if ts >= at {
                    st.waits.entry(new_tid).or_default().push(ts - at);
                }
            }
        }
    }

    fn on_hard_fault(&self, tid: u32) {
        let mut st = lock(&self.gate);
        if st.window && st.game_tids.contains(&tid) {
            st.faults += 1;
        }
    }
}

/// A new window: every list emptied, capacity kept (a game that churned through very many threads starts afresh).
fn reset_waits(w: &mut IndexMap<u32, Vec<i64>, FastBuild>) {
    if w.len() > 4096 {
        w.clear();
    } else {
        for v in w.values_mut() {
            v.clear();
        }
    }
}

unsafe extern "system" fn on_event(record: *mut core::ffi::c_void) {
    let r = raw::rec(record as *const u8);
    if r.context.is_null() {
        return;
    }
    let inner = &*(r.context as *const Inner);
    let (op, len, u) = (r.opcode, r.len, r.user);
    if u.is_null() {
        return;
    }
    let thread = r.provider == THREAD_GUID;
    let fault = r.provider == PAGE_FAULT_GUID;
    if thread {
        let ts = r.stamp;
        match op {
            1 | 3 => {
                if len >= 8 {
                    inner.on_thread(raw::rd(u, 0), raw::rd(u, 4)); // Start / DCStart
                }
            }
            50 => {
                if len >= 4 {
                    inner.on_ready(raw::rd(u, 0), ts); // ReadyThread
                }
            }
            36 => {
                if len >= 4 {
                    inner.on_switch(raw::rd(u, 0), ts); // CSwitch: NewThreadId
                }
            }
            _ => {}
        }
    } else if fault && op == 32 && len >= 40 {
        inner.on_hard_fault(raw::rd(u, 32)); // HardFault: TThreadId at 32 (x64)
    }
}

/// Benchmarks (feature "testsupport"): the real ETW callback and window bookkeeping, without a session.
#[cfg(any(test, feature = "testsupport"))]
impl ReadyProbe {
    /// The UserContext the session's records carry.
    pub fn bench_context(&self) -> *const core::ffi::c_void {
        Arc::as_ptr(&self.inner) as *const core::ffi::c_void
    }
    /// A window opens for pid (what `tick` does, without TraceSetInformation).
    pub fn bench_open(&self, pid: u32) {
        let mut st = lock(&self.inner.gate);
        st.pid = pid;
        st.ready.clear();
        reset_waits(&mut st.waits);
        st.faults = 0;
        st.window = true;
        st.window_start = qpc::timestamp();
    }
    /// The window closes and its results are computed (what `close` does after the flags went off).
    pub fn bench_close(&self) {
        let mut st = lock(&self.inner.gate);
        st.window = false;
        let tpm = self.inner.tpm;
        st.p99 = st.waits.iter().filter(|(_, v)| v.len() >= ProbePolicy::MIN_WAITS).map(|(k, v)| (*k, Self::p99(v) as f64 / tpm)).collect();
    }
    /// The session's EVENT_RECORD callback.
    ///
    /// # Safety
    /// `record` points to an EVENT_RECORD whose UserContext is `bench_context()` of a live probe.
    pub unsafe fn bench_event(record: *mut core::ffi::c_void) {
        on_event(record)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ReadyProbeTests.AWindowGivesP99WaitsOfGameThreadsAndTheirHardFaults
    #[test]
    fn a_window_gives_p99_waits_of_game_threads_and_their_hard_faults() {
        let p = ReadyProbe::new();
        let tpm = qpc::frequency() as f64 / 1000.0;
        p.test_window(
            100,
            |pr| {
                pr.on_thread(100, 11);
                pr.on_thread(100, 12);
                pr.on_thread(200, 99);
                let mut t: i64 = 1_000_000;
                for i in 0..100 {
                    pr.on_ready(11, t);
                    pr.on_switch(11, t + ((if i >= 98 { 2.0 } else { 0.01 }) * tpm) as i64);
                    t += (5.0 * tpm) as i64;
                }
                for _ in 0..2 {
                    pr.on_ready(12, t);
                    pr.on_switch(12, t + (3.0 * tpm) as i64);
                    t += (5.0 * tpm) as i64;
                } // too few waits
                pr.on_ready(99, t);
                pr.on_switch(99, t + (9.0 * tpm) as i64); // not the game
                for _ in 0..25 {
                    pr.on_hard_fault(11);
                }
                for _ in 0..40 {
                    pr.on_hard_fault(99);
                }
            },
            0.25,
        );
        let w = p.wait_p99_ms(11);
        assert!((1.9..=2.1).contains(&w), "{}", w); // p99 of 100 waits with two at 2 ms = 2 ms
        assert_eq!(-1.0, p.wait_p99_ms(12));
        assert_eq!(-1.0, p.wait_p99_ms(99));
        assert!((p.hard_faults_per_sec().unwrap() - 100.0).abs() < 0.05); // 25 in a quarter of a second
    }
}
