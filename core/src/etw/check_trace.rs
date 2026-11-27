//! Port of `CheckTrace.cs`.
//!
//! The measurement behind the "Check" page (1.16.0 Test 1), the same arithmetic as Domain SchedTest: Windows' own kernel
//! trace gives every ready-thread wait (ReadyThread -> the context switch that puts the thread on a processor); a second
//! session gives the game's frames (DXGI "Present start", event 42). Both are private real-time sessions on the QPC
//! clock that stop with the measurement. Nothing is opened in the game and nothing in the system is changed. Kept only
//! between Begin and End.

use super::qpc;
use super::raw::{self, SessionSpec, DXGI_GUID, DXGI_U128, THREAD_GUID};
use crate::base::hooks::lock;
use crate::base::l::L;
use crate::lf;
use indexmap::IndexSet;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

const KERNEL_NAME: &str = "DomainScheduler-Check";
const FRAMES_NAME: &str = "DomainScheduler-CheckFrames";
const KERNEL_GUID: [u8; 16] = raw::guid_bytes(0x6e2a8c31_4b7d_4f0e_9a55_1d3c7b2e9f40);
const FRAMES_GUID: [u8; 16] = raw::guid_bytes(0xa9d40e72_1c3b_46f8_8e21_7b5f0c9d3a16);
/// PROCESS | THREAD (thread -> process rundown) | CSWITCH | DISPATCHER (ReadyThread)
const FLAGS: u32 = 0x1 | 0x2 | 0x10 | 0x800;

struct State {
    /// Waits of the watched process's threads, in QPC ticks.
    game_waits: Vec<i64>,
    /// Present timestamps (QPC) of the watched process.
    presents: Vec<i64>,
    first: i64,
    last: i64,
    game_tids: IndexSet<u32>,
    ready: HashMap<u32, i64>,
}

#[derive(Default)]
struct Run {
    k_session: u64,
    k_trace: u64,
    f_session: u64,
    f_trace: u64,
    k_thread: Option<std::thread::JoinHandle<()>>,
    f_thread: Option<std::thread::JoinHandle<()>>,
}

pub(crate) struct Inner {
    pid: u32,
    gate: Mutex<State>,
    collect: AtomicBool,
    running: AtomicBool,
}

/// What `Snapshot` returns: (waits, presents ascending, first, last).
pub type CheckSnapshot = (Vec<i64>, Vec<i64>, i64, i64);

/// `CheckTrace` (IDisposable: call `dispose`).
pub struct CheckTrace {
    inner: Arc<Inner>,
    error: Mutex<String>,
    lost: Mutex<u32>,
    run: Mutex<Run>,
}

impl CheckTrace {
    pub fn new(pid: u32) -> Self {
        CheckTrace {
            inner: Arc::new(Inner {
                pid,
                gate: Mutex::new(State {
                    game_waits: Vec::with_capacity(1 << 18),
                    presents: Vec::with_capacity(1 << 15),
                    first: 0,
                    last: 0,
                    game_tids: IndexSet::new(),
                    ready: HashMap::new(),
                }),
                collect: AtomicBool::new(false),
                running: AtomicBool::new(false),
            }),
            error: Mutex::new(String::new()),
            lost: Mutex::new(0),
            run: Mutex::new(Run::default()),
        }
    }

    pub fn ticks_per_second() -> f64 {
        qpc::frequency() as f64
    }

    pub fn error(&self) -> String {
        lock(&self.error).clone()
    }

    pub fn running(&self) -> bool {
        self.inner.running.load(Ordering::SeqCst)
    }

    /// Events lost by the kernel session, read at `end`.
    pub fn lost(&self) -> u32 {
        *lock(&self.lost)
    }

    pub fn first(&self) -> i64 {
        lock(&self.inner.gate).first
    }

    pub fn last(&self) -> i64 {
        lock(&self.inner.gate).last
    }

    pub fn start(&self) -> bool {
        raw::stop_by_name(KERNEL_NAME);
        raw::stop_by_name(FRAMES_NAME);
        let kspec = SessionSpec { guid: KERNEL_GUID, mode: 0x100 /* REAL_TIME */ | 0x02000000 /* SYSTEM_LOGGER */, buffer_kb: 1024, min_buffers: 64, max_buffers: 512, flags: FLAGS };
        let (rc, ks) = raw::start(KERNEL_NAME, &kspec);
        lock(&self.run).k_session = ks;
        if rc == 5 {
            *lock(&self.error) = L::t("no administrator rights").to_string();
            return false;
        }
        if rc != 0 {
            *lock(&self.error) = lf!("the kernel trace did not start (error {0})", rc);
            return false;
        }
        let r2 = raw::set_flags(ks, FLAGS);
        if r2 != 0 {
            raw::stop_by_name(KERNEL_NAME);
            *lock(&self.error) = lf!("the kernel trace did not start (error {0})", r2);
            return false;
        }
        let ctx = Arc::as_ptr(&self.inner) as *const core::ffi::c_void;
        let kt = raw::open(KERNEL_NAME, on_kernel, ctx);
        lock(&self.run).k_trace = kt;
        if kt == u64::MAX {
            raw::stop_by_name(KERNEL_NAME);
            *lock(&self.error) = L::t("the session could not be opened").to_string();
            return false;
        }
        let th = raw::spawn_process("check-kernel", 1, Some(L::n("Check trace")), kt, self.inner.clone());
        lock(&self.run).k_thread = th;
        self.inner.running.store(true, Ordering::SeqCst);

        // Frames: optional; without them the waits are still measured.
        let fspec = SessionSpec { guid: FRAMES_GUID, mode: 0x100, buffer_kb: 64, min_buffers: 16, max_buffers: 128, flags: 0 };
        let (frc, fs) = raw::start(FRAMES_NAME, &fspec);
        lock(&self.run).f_session = fs;
        if frc == 0 && raw::enable(fs, DXGI_U128, 4, u64::MAX, &[]) == 0 {
            let ft = raw::open(FRAMES_NAME, on_frames, ctx);
            lock(&self.run).f_trace = ft;
            if ft != u64::MAX {
                let fth = raw::spawn_process("check-frames", 0, Some(L::n("Check frames trace")), ft, self.inner.clone());
                lock(&self.run).f_thread = fth;
                let inner = self.inner.clone();
                let _ = std::thread::Builder::new().name("check-flush".into()).spawn(move || {
                    while inner.running.load(Ordering::SeqCst) {
                        std::thread::sleep(std::time::Duration::from_millis(500));
                        raw::flush(FRAMES_NAME);
                    }
                });
            }
        }
        true
    }

    /// The first seconds of a session carry the rundown; the measurement starts here.
    pub fn begin(&self) {
        {
            let mut st = lock(&self.inner.gate);
            st.ready.clear();
            st.game_waits.clear();
            st.presents.clear();
            st.first = 0;
            st.last = 0;
        }
        self.inner.collect.store(true, Ordering::SeqCst);
    }

    pub fn end(&self) {
        self.inner.collect.store(false, Ordering::SeqCst);
        *lock(&self.lost) = raw::lost(KERNEL_NAME);
    }

    /// A copy of what was collected (the sessions keep running until Dispose).
    pub fn snapshot(&self) -> CheckSnapshot {
        let st = lock(&self.inner.gate);
        let mut p = st.presents.clone();
        p.sort(); // OrderBy: stable, equal stamps are equal
        (st.game_waits.clone(), p, st.first, st.last)
    }

    pub fn dispose(&self) {
        if !self.running() {
            return;
        }
        self.inner.running.store(false, Ordering::SeqCst);
        raw::flush(FRAMES_NAME);
        raw::stop_by_name(KERNEL_NAME);
        raw::stop_by_name(FRAMES_NAME);
        let (kt, ft, kth, fth) = {
            let mut r = lock(&self.run);
            (r.k_trace, r.f_trace, r.k_thread.take(), r.f_thread.take())
        };
        raw::close(kt);
        if ft != 0 && ft != u64::MAX {
            raw::close(ft);
        }
        raw::join_timeout(kth, 5000);
        raw::join_timeout(fth, 3000);
    }

    /// Tests: the kernel events as the callback hands them over.
    pub fn on_thread_for_tests(&self, pid: u32, tid: u32) {
        self.inner.kernel(1, 8, &[pid, tid], 0);
    }
    pub fn on_ready_for_tests(&self, tid: u32, ts: i64) {
        self.inner.kernel(50, 4, &[tid], ts);
    }
    pub fn on_switch_for_tests(&self, new_tid: u32, old_tid: u32, ts: i64) {
        self.inner.kernel(36, 8, &[new_tid, old_tid], ts);
    }
    pub fn on_present_for_tests(&self, pid: u32, ts: i64) {
        self.inner.frames(42, pid, ts);
    }
}

impl Inner {
    /// One Thread-provider event: opcode, payload length, the payload's first u32 fields, time stamp.
    fn kernel(&self, opcode: u8, len: u16, u: &[u32], ts: i64) {
        let mut st = lock(&self.gate);
        let collect = self.collect.load(Ordering::SeqCst);
        match opcode {
            1 | 3 => {
                // Thread Start / DCStart: ProcessId, TThreadId
                if len >= 8 && u[0] == self.pid {
                    st.game_tids.insert(u[1]);
                }
            }
            50 => {
                // ReadyThread: TThreadId
                if !collect || len < 4 {
                    return;
                }
                let t = u[0];
                if st.game_tids.contains(&t) {
                    st.ready.entry(t).or_insert(ts);
                }
            }
            36 => {
                // CSwitch: NewThreadId, OldThreadId
                if !collect || len < 8 {
                    return;
                }
                if st.first == 0 {
                    st.first = ts;
                }
                st.last = ts;
                let nt = u[0];
                if nt != 0 {
                    if let Some(ready_at) = st.ready.remove(&nt) {
                        if ts >= ready_at {
                            st.game_waits.push(ts - ready_at);
                        }
                    }
                }
            }
            _ => {}
        }
    }

    fn frames(&self, id: u16, pid: u32, ts: i64) {
        if !self.collect.load(Ordering::SeqCst) {
            return;
        }
        if id != 42 || pid != self.pid {
            return;
        }
        lock(&self.gate).presents.push(ts);
    }
}

unsafe extern "system" fn on_kernel(record: *mut core::ffi::c_void) {
    let r = raw::rec(record as *const u8);
    if r.context.is_null() || r.provider != THREAD_GUID {
        return;
    }
    if r.user.is_null() {
        return;
    }
    let inner = &*(r.context as *const Inner);
    let len = r.len;
    let f0: u32 = if len >= 4 { raw::rd(r.user, 0) } else { 0 };
    let f1: u32 = if len >= 8 { raw::rd(r.user, 4) } else { 0 };
    inner.kernel(r.opcode, len, &[f0, f1], r.stamp);
}

unsafe extern "system" fn on_frames(record: *mut core::ffi::c_void) {
    let r = raw::rec(record as *const u8);
    if r.context.is_null() {
        return;
    }
    let inner = &*(r.context as *const Inner);
    if !inner.collect.load(Ordering::SeqCst) {
        return;
    }
    if r.provider != DXGI_GUID {
        return;
    }
    inner.frames(r.id, r.pid, r.stamp);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn waits_of_the_game_threads_between_begin_and_end() {
        let c = CheckTrace::new(100);
        c.on_thread_for_tests(100, 11);
        c.on_thread_for_tests(200, 99);
        c.on_ready_for_tests(11, 10); // before Begin: not collected
        c.begin();
        c.on_ready_for_tests(11, 100);
        c.on_ready_for_tests(11, 120); // the first ready time stays
        c.on_ready_for_tests(99, 100); // not the game
        c.on_switch_for_tests(99, 0, 150);
        c.on_switch_for_tests(11, 5, 400);
        c.on_present_for_tests(100, 900);
        c.on_present_for_tests(100, 500);
        c.on_present_for_tests(200, 600);
        c.end();
        c.on_present_for_tests(100, 1000); // after End
        let (w, p, first, last) = c.snapshot();
        assert_eq!(w, vec![300]);
        assert_eq!(p, vec![500, 900]);
        assert_eq!((first, last), (150, 400));
        assert!(!c.start() || cfg!(windows)); // no ETW on the test host
    }
}
