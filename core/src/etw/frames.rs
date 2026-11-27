//! Port of `EtwFrames.cs`: the game's frames from the DXGI provider's "Present start" event (42) in a private
//! real-time session, filtered to the watched process. The C# class uses Microsoft.Diagnostics.Tracing.TraceEvent;
//! the port makes the same session with the raw ETW API (see the report: the session buffers are the ones of the
//! owner's other DXGI sessions, CheckTrace and BlackBox, not TraceEvent's defaults).

use super::qpc;
use super::raw::{self, SessionSpec, DXGI_GUID, DXGI_U128};
use crate::base::hooks::lock;
use crate::base::l::L;
use crate::base::time::{DateTime, TICKS_PER_SECOND};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// `IFrameSource` (declared in AutoController.cs, auto area): frame times of one process (PresentMon or ETW);
/// `available()` = false when none can be measured.
pub trait IFrameSource: Send + Sync {
    fn available(&self) -> bool;
    fn name(&self) -> String;
    fn error(&self) -> String;
    fn watch(&self, pid: u32);
    /// Frame intervals in ms of frames that ended between from and to (UTC).
    fn intervals(&self, from: DateTime, to: DateTime) -> Vec<f64>;
}

const SESSION_NAME: &str = "DomainScheduler-Frames";
const PRESENT_START: u16 = 42;
/// A minute of frames at up to ~660 FPS for the home chart (1.14.0; was 20000).
const KEEP: usize = 40000;

struct Session {
    generation: u64,
    trace: u64,
}

struct State {
    frames: VecDeque<DateTime>,
    session: Option<Session>,
}

pub(crate) struct Inner {
    gate: Mutex<State>,
    pid: AtomicU32,
    available: AtomicBool,
    error: Mutex<String>,
    /// the session generation the trace thread / flusher belong to (0 = none)
    current: AtomicU64,
    next_gen: AtomicU64,
    /// QPC -> UTC: (qpc at open, UTC ticks at open, QPC frequency)
    base_qpc: AtomicU64,
    base_utc: AtomicU64,
}

/// `EtwFrames` (IFrameSource, IDisposable).
pub struct EtwFrames {
    inner: Arc<Inner>,
}

impl Default for EtwFrames {
    fn default() -> Self {
        Self::new()
    }
}

impl EtwFrames {
    pub fn new() -> Self {
        EtwFrames {
            inner: Arc::new(Inner {
                gate: Mutex::new(State { frames: VecDeque::new(), session: None }),
                pid: AtomicU32::new(0),
                available: AtomicBool::new(false),
                error: Mutex::new(String::new()),
                current: AtomicU64::new(0),
                next_gen: AtomicU64::new(1),
                base_qpc: AtomicU64::new(0),
                base_utc: AtomicU64::new(0),
            }),
        }
    }

    pub fn start(&self) {
        if !is_elevated() {
            *lock(&self.inner.error) = L::t("no administrator rights").to_string();
        } else {
            self.inner.available.store(true, Ordering::SeqCst);
        }
    }

    fn open(&self, st: &mut State, pid: u32) {
        self.close(st);
        match self.try_open(st, pid) {
            Ok(()) => {}
            Err(msg) => {
                *lock(&self.inner.error) = msg;
                self.inner.available.store(false, Ordering::SeqCst);
                self.close(st);
            }
        }
    }

    fn try_open(&self, st: &mut State, pid: u32) -> Result<(), String> {
        // TraceEventSession(name): a session of that name left behind is replaced.
        raw::stop_by_name(SESSION_NAME);
        let spec = SessionSpec { guid: raw::guid_bytes(0), mode: 0x100, buffer_kb: 64, min_buffers: 16, max_buffers: 128, flags: 0 };
        let (rc, session) = raw::start(SESSION_NAME, &spec);
        if rc != 0 {
            return Err(win_error(rc));
        }
        let generation = self.inner.next_gen.fetch_add(1, Ordering::SeqCst);
        st.session = Some(Session { generation, trace: 0 });
        self.inner.current.store(generation, Ordering::SeqCst);
        // Only "Present start" (42) is used (OnEvent): the driver then does not log the provider's other events at
        // all, and the frame thread does not parse them (1.16.0 Test 1: own load).
        let pid_filter = pid.to_le_bytes().to_vec();
        let mut ids = vec![1u8, 0u8]; // FilterIn = TRUE, Reserved
        ids.extend_from_slice(&1u16.to_le_bytes()); // Count
        ids.extend_from_slice(&PRESENT_START.to_le_bytes());
        let rc = raw::enable(session, DXGI_U128, 4 /* Informational */, u64::MAX, &[(0x80000004 /* PID */, pid_filter), (0x80000200 /* EVENT_ID */, ids)]);
        if rc != 0 {
            return Err(win_error(rc));
        }
        self.inner.base_qpc.store(qpc::timestamp() as u64, Ordering::SeqCst);
        self.inner.base_utc.store(DateTime::utc_now().ticks as u64, Ordering::SeqCst);
        let ctx = Arc::as_ptr(&self.inner) as *const core::ffi::c_void;
        let trace = raw::open(SESSION_NAME, on_event, ctx);
        if trace == u64::MAX {
            return Err(L::t("the session could not be opened").to_string());
        }
        if let Some(s) = st.session.as_mut() {
            s.trace = trace;
        }
        let keep = self.inner.clone();
        let _ = raw::spawn_process("etw-frames", -1, Some(L::n("frame trace")), trace, keep);
        // Real-time ETW hands buffers over about once a second or when they fill; a flush twice a second keeps
        // the frames close to the clock for the watchdog (as Domain FrameTime does).
        let inner = self.inner.clone();
        let _ = std::thread::Builder::new().name("etw-frames-flush".into()).spawn(move || {
            raw::set_priority(-1);
            crate::native::self_threads::SelfThreads::register(L::n("frame trace flush"));
            while inner.current.load(Ordering::SeqCst) == generation {
                std::thread::sleep(std::time::Duration::from_millis(500));
                if inner.current.load(Ordering::SeqCst) != generation {
                    break;
                }
                raw::flush(SESSION_NAME);
            }
            crate::native::self_threads::SelfThreads::ended(L::n("frame trace flush"));
        });
        Ok(())
    }

    fn close(&self, st: &mut State) {
        if let Some(s) = st.session.take() {
            // StopOnDispose: the session ends, the trace thread's ProcessTrace returns.
            if self.inner.current.load(Ordering::SeqCst) == s.generation {
                self.inner.current.store(0, Ordering::SeqCst);
            }
            raw::stop_by_name(SESSION_NAME);
            if s.trace != 0 && s.trace != u64::MAX {
                raw::close(s.trace);
            }
        }
    }

    /// Present times (UTC, ascending) after `from`, copied into `into` (reused by the caller: no garbage per update).
    pub fn times(&self, from: DateTime, into: &mut Vec<DateTime>) -> usize {
        into.clear();
        let st = lock(&self.inner.gate);
        for f in &st.frames {
            if *f > from {
                into.push(*f);
            }
        }
        into.len()
    }

    pub fn dispose(&self) {
        {
            let mut st = lock(&self.inner.gate);
            self.close(&mut st);
        }
        self.inner.available.store(false, Ordering::SeqCst);
    }

    /// Tests: a frame of the watched process.
    pub fn add_for_tests(&self, utc: DateTime) {
        self.inner.push(utc);
    }
}

impl IFrameSource for EtwFrames {
    fn available(&self) -> bool {
        self.inner.available.load(Ordering::SeqCst)
    }

    fn name(&self) -> String {
        "ETW DXGI Present".into()
    }

    fn error(&self) -> String {
        lock(&self.inner.error).clone()
    }

    fn watch(&self, pid: u32) {
        let mut st = lock(&self.inner.gate);
        if pid != self.inner.pid.load(Ordering::SeqCst) || (pid != 0 && st.session.is_none()) {
            self.inner.pid.store(pid, Ordering::SeqCst);
            st.frames.clear();
            if pid == 0 {
                self.close(&mut st);
            } else if self.available() {
                self.open(&mut st, pid);
            }
        }
    }

    fn intervals(&self, from: DateTime, to: DateTime) -> Vec<f64> {
        let mut list = Vec::new();
        let st = lock(&self.inner.gate);
        let mut prev = DateTime::default();
        for f in &st.frames {
            if prev != DateTime::default() && *f > from && *f <= to {
                list.push((*f - prev).total_ms());
            }
            prev = *f;
        }
        list
    }
}

impl Inner {
    fn push(&self, t: DateTime) {
        let mut st = lock(&self.gate);
        st.frames.push_back(t);
        while st.frames.len() > KEEP {
            st.frames.pop_front();
        }
    }

    /// A raw QPC stamp as UTC (the session clock is QPC).
    fn utc_of(&self, stamp: i64) -> DateTime {
        let bq = self.base_qpc.load(Ordering::SeqCst) as i64;
        let bu = self.base_utc.load(Ordering::SeqCst) as i64;
        let f = qpc::frequency() as f64;
        DateTime::utc(bu + ((stamp - bq) as f64 * TICKS_PER_SECOND as f64 / f) as i64)
    }
}

/// The DXGI callback: Present start of the watched process.
unsafe extern "system" fn on_event(record: *mut core::ffi::c_void) {
    let r = raw::rec(record as *const u8);
    if r.context.is_null() {
        return;
    }
    let inner = &*(r.context as *const Inner);
    let pid = inner.pid.load(Ordering::SeqCst);
    if pid == 0 || r.pid != pid || r.id != PRESENT_START || r.provider != DXGI_GUID {
        return;
    }
    inner.push(inner.utc_of(r.stamp));
}

fn win_error(rc: u32) -> String {
    std::io::Error::from_raw_os_error(rc as i32).to_string()
}

/// `TraceEventSession.IsElevated()`: the process token is elevated.
#[cfg(windows)]
fn is_elevated() -> bool {
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::Security::{GetTokenInformation, TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY};
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
    let mut tok: HANDLE = std::ptr::null_mut();
    // SAFETY: plain out parameters; the token handle is closed below.
    unsafe {
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut tok) == 0 {
            return false;
        }
        let mut e = TOKEN_ELEVATION { TokenIsElevated: 0 };
        let mut len = 0u32;
        let ok = GetTokenInformation(tok, TokenElevation, &mut e as *mut _ as *mut core::ffi::c_void, std::mem::size_of::<TOKEN_ELEVATION>() as u32, &mut len) != 0;
        CloseHandle(tok);
        ok && e.TokenIsElevated != 0
    }
}

#[cfg(not(windows))]
fn is_elevated() -> bool {
    false
}

/// Benchmarks (feature "testsupport"): the real ETW callback, without a session.
#[cfg(any(test, feature = "testsupport"))]
impl EtwFrames {
    pub fn bench_context(&self, pid: u32) -> *const core::ffi::c_void {
        self.inner.pid.store(pid, Ordering::SeqCst);
        Arc::as_ptr(&self.inner) as *const core::ffi::c_void
    }
    /// # Safety
    /// `record` points to an EVENT_RECORD whose UserContext is `bench_context()` of a live trace.
    pub unsafe fn bench_event(record: *mut core::ffi::c_void) {
        on_event(record)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::base::time::{Kind, TimeSpan};

    #[test]
    fn intervals_and_times_use_the_kept_frames() {
        let f = EtwFrames::new();
        let t0 = DateTime::from_parts(2026, 1, 1, 12, 0, 0, Kind::Utc).unwrap();
        for i in 0..5 {
            f.add_for_tests(t0 + TimeSpan::from_ms(10.0 * i as f64));
        }
        // the first frame has no interval; frames in (from, to]
        assert_eq!(f.intervals(t0, t0 + TimeSpan::from_ms(30.0)), vec![10.0, 10.0, 10.0]);
        let mut v = Vec::new();
        assert_eq!(f.times(t0 + TimeSpan::from_ms(15.0), &mut v), 3);
        assert_eq!(f.name(), "ETW DXGI Present");
        f.start(); // Linux/Wine: no session; an elevated Windows host (CI runner) really starts it
        if f.available() {
            f.dispose();
        } else {
            f.watch(100); // not available: no session
        }
    }
}
