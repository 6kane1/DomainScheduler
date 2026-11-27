//! Thread map feed (native-only, the "Thread map" page of the advanced layout).
//!
//! One private real-time system-logger session ("DomainScheduler-ThreadMap") with process, thread, image, context
//! switch and dispatcher (ReadyThread) events feeds a `threadmap` map (crate `threadmap`, C ABI `tm_*`, header
//! threadmap/threadmap.h). The session runs ONLY while the page is on screen: the page opens it (`dos_tm_open`) and
//! closes it when it is left or the window hides (`dos_tm_close`). It is a session of its own like the black box
//! ("DomainScheduler-BlackBox") and the wait probe ("DomainScheduler-Probe"): Windows runs several private system
//! loggers side by side (up to 8 with the NT Kernel Logger and the Circular Kernel Context Logger), so none of them is
//! paused while the page is open.
//!
//! Hot path (CSwitch, ReadyThread): the callback decodes a few fields into fixed-size structs and appends them to
//! preallocated batches (no allocation); a batch goes into the map when it is full or 50 ms of event time passed
//! (`Feeder`). Process/thread/image events are rare and carry strings; they flush the batches first so the map sees
//! them in feed order. Thread start addresses are resolved to "module+offset" from the image events, so unnamed
//! threads of one pool merge into one "Worker xN" node.
//!
//! Without a trace (Wine, no rights) the page says why. `DOS_TM_DEMO=1` (or the DOS_UI_SHOT build) runs a synthetic
//! feed instead (`thread_map_demo`), through the same `Feeder` and `tm_feed_*` calls; it is never used otherwise.

use super::qpc;
use super::raw::{self, SessionSpec, THREAD_GUID};
use super::thread_map_demo::Demo;
use crate::base::hooks::lock;
use crate::base::l::L;
use crate::lf;
use std::collections::HashMap;
use std::ffi::CString;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
pub use threadmap;
use threadmap::events::{CSwitch, LinkEvent, ReadyThread, READIER_UNKNOWN};
use threadmap::ffi::{self as tm, TmMap};

pub const PROCESS_GUID: [u8; 16] = raw::guid_bytes(0x3d6fa8d0_fe05_11d0_9dda_00c04fd7ba7c);
pub const IMAGE_GUID: [u8; 16] = raw::guid_bytes(0x2cb15d1d_5fc1_11d2_abe1_00a0c911f518);

const NAME: &str = "DomainScheduler-ThreadMap";
const SESSION_GUID: [u8; 16] = raw::guid_bytes(0x9a3c5e17_2b84_4d6f_a1c9_6e0b7d4f3a52);
/// PROCESS | THREAD | IMAGE_LOAD | CSWITCH | DISPATCHER (ReadyThread).
const FLAGS: u32 = 0x1 | 0x2 | 0x4 | 0x10 | 0x800;
/// EVENT_TRACE_USE_MS_FLUSH_TIMER: FlushTimer in ms (Windows 8+), so a quiet cpu's buffer is late by 100 ms at most.
const MS_FLUSH: u32 = raw::MODE_MS_FLUSH_TIMER;
const FLUSH_TIMER_MS: u32 = 100;
/// How long `stop` waits for the trace thread (ProcessTrace returns once the session is stopped and closed).
const JOIN_MS: u64 = 2000;
/// The map's reorder window with the 100 ms flush timer, and with the 1 s one (older Windows refused the ms timer).
const REORDER_MS: u32 = 300;
const REORDER_SLOW_MS: u32 = 1200;

/// Events per batch (CSwitch and ReadyThread each): 4096 * 40 bytes = 160 KB, allocated once.
pub const BATCH: usize = 4096;
/// A batch goes into the map after this much event time even when it is not full.
pub const FLUSH_MS: i64 = 50;

// ================================================================ decoding (pure, tested on Linux)

fn u32_at(u: &[u8], off: usize) -> Option<u32> {
    u.get(off..off + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

fn u64_at(u: &[u8], off: usize) -> Option<u64> {
    u.get(off..off + 8).map(|b| u64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]))
}

/// Thread/CSwitch (opcode 36, x64): NewThreadId, OldThreadId, NewThreadPriority, OldThreadPriority, PreviousCState,
/// SpareByte, OldThreadWaitReason, OldThreadWaitMode, OldThreadState, OldThreadWaitIdealProcessor,
/// NewThreadWaitTime, Reserved (24 bytes).
pub fn parse_cswitch(u: &[u8], ts: i64, cpu: u32) -> Option<CSwitch> {
    if u.len() < 24 {
        return None;
    }
    Some(CSwitch {
        ts,
        cpu,
        new_tid: u32_at(u, 0)?,
        old_tid: u32_at(u, 4)?,
        new_priority: u[8] as i8,
        old_priority: u[9] as i8,
        old_wait_reason: u[12],
        old_wait_mode: u[13],
        old_state: u[14],
        reserved: [0; 3],
        new_wait_time: u32_at(u, 16)?,
        reserved2: 0,
    })
}

/// Thread/ReadyThread (opcode 50): TThreadId, AdjustReason, AdjustIncrement, Flag, Reserved. The readying thread is
/// the event header's ThreadId (as WPA's "Readying Thread"); -1 or the target itself is not trusted.
pub fn parse_ready(u: &[u8], ts: i64, cpu: u32, header_tid: u32) -> Option<ReadyThread> {
    if u.len() < 8 {
        return None;
    }
    let target = u32_at(u, 0)?;
    let readier = if header_tid == u32::MAX || header_tid == target { READIER_UNKNOWN } else { header_tid };
    Some(ReadyThread { ts, cpu, readier_tid: readier, target_tid: target, adjust_reason: u[4] as i8, adjust_increment: u[5] as i8, flags: u[6], reserved: 0 })
}

/// Size of the SID field of kernel process events at `off` (TraceEvent's SkipSID): 4 bytes when null, else a
/// TOKEN_USER (16 bytes on x64) followed by the SID (8 + 4 * SubAuthorityCount). None when it does not look like one.
fn sid_len(u: &[u8], off: usize) -> Option<usize> {
    if u32_at(u, off)? == 0 {
        return Some(4);
    }
    let rev = *u.get(off + 16)?;
    let n = *u.get(off + 17)? as usize;
    if rev != 1 || n > 15 {
        return None;
    }
    let len = 16 + 8 + 4 * n;
    (off + len <= u.len()).then_some(len)
}

/// A NUL-terminated ANSI string at `off` (the image name of a process event).
fn ansi_z(u: &[u8], off: usize) -> Option<String> {
    let rest = u.get(off..)?;
    let end = rest.iter().position(|&b| b == 0)?;
    let s = &rest[..end];
    // An image name starts with a letter or digit ("cs2.exe", "System", "Registry"), never a pointer byte.
    if s.len() < 2 || !s[0].is_ascii_alphanumeric() || !s.iter().all(|&b| (0x20..0x7f).contains(&b) || b >= 0x80) {
        return None;
    }
    Some(String::from_utf8_lossy(s).into_owned())
}

/// A NUL-terminated UTF-16 string at `off` (to the end of the data when there is no NUL).
fn utf16_z(u: &[u8], off: usize) -> Option<String> {
    let rest = u.get(off..)?;
    let w: Vec<u16> = rest.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).take_while(|&c| c != 0).collect();
    Some(String::from_utf16_lossy(&w))
}

/// Process/Start, DCStart (x64, v3/v4): UniqueProcessKey, ProcessId, ParentId, SessionId, ExitStatus,
/// DirectoryTableBase, (v4: Flags), UserSID, ImageFileName (ANSI), ... -> (pid, image).
pub fn parse_process(u: &[u8], version: u8) -> Option<(u32, String)> {
    let pid = u32_at(u, 8)?;
    // v4 has Flags before the SID; try the version's offset first, then the other one.
    let offs: [usize; 2] = if version >= 4 { [36, 32] } else { [32, 36] };
    for off in offs {
        if let Some(n) = sid_len(u, off) {
            if let Some(img) = ansi_z(u, off + n) {
                return Some((pid, img));
            }
        }
    }
    Some((pid, String::new()))
}

/// Process/End: the pid.
pub fn parse_process_end(u: &[u8]) -> Option<u32> {
    u32_at(u, 8)
}

/// Thread/Start, DCStart (x64, v3/v4): ProcessId, TThreadId, StackBase, StackLimit, UserStackBase, UserStackLimit,
/// Affinity, Win32StartAddr (48), TebBase, SubProcessTag, BasePriority, PagePriority, IoPriority, ThreadFlags,
/// (v4: ThreadName at 72) -> (pid, tid, start address, name).
pub fn parse_thread(u: &[u8]) -> Option<(u32, u32, u64, Option<String>)> {
    let pid = u32_at(u, 0)?;
    let tid = u32_at(u, 4)?;
    let start = u64_at(u, 48).unwrap_or(0);
    let name = if u.len() > 74 { utf16_z(u, 72).filter(|s| !s.trim().is_empty()) } else { None };
    Some((pid, tid, start, name))
}

/// Thread/SetName (opcode 72): ProcessId, TThreadId, ThreadName (UTF-16).
pub fn parse_set_name(u: &[u8]) -> Option<(u32, u32, String)> {
    Some((u32_at(u, 0)?, u32_at(u, 4)?, utf16_z(u, 8)?))
}

/// Image/Load, DCStart (x64): ImageBase, ImageSize, ProcessId, ..., FileName (UTF-16 kernel path) at 56 (v2/v3) or
/// 48 (v1) -> (pid, base, size, file name without the folder).
pub fn parse_image(u: &[u8]) -> Option<(u32, u64, u64, String)> {
    let base = u64_at(u, 0)?;
    let size = u64_at(u, 8)?;
    let pid = u32_at(u, 16)?;
    for off in [56usize, 48] {
        if u.get(off..off + 2) == Some(&[b'\\', 0]) {
            let path = utf16_z(u, off)?;
            let file = path.rsplit('\\').next().unwrap_or("").to_string();
            if !file.is_empty() {
                return Some((pid, base, size, file));
            }
        }
    }
    None
}

// ================================================================ batching

/// Where decoded events go: the map (`TmSink`) or a recorder in tests.
pub trait Sink {
    fn cswitch(&mut self, ev: &[CSwitch]);
    fn ready(&mut self, ev: &[ReadyThread]);
    fn link(&mut self, ev: &[LinkEvent]);
    fn process_start(&mut self, ts: i64, pid: u32, image: &str);
    fn process_end(&mut self, ts: i64, pid: u32);
    fn thread_start(&mut self, ts: i64, tid: u32, pid: u32, name: Option<&str>, module: Option<&str>, offset: u64);
    fn thread_end(&mut self, ts: i64, tid: u32);
    fn thread_name(&mut self, ts: i64, tid: u32, name: &str);
}

/// Batches of the frequent events. The vectors are allocated once with `cap` and flushed before they would grow.
pub struct Feeder {
    cs: Vec<CSwitch>,
    rd: Vec<ReadyThread>,
    lk: Vec<LinkEvent>,
    cap: usize,
    flush_ticks: i64,
    last: i64,
}

impl Feeder {
    pub fn new(cap: usize, flush_ticks: i64) -> Self {
        let cap = cap.max(1);
        Self { cs: Vec::with_capacity(cap), rd: Vec::with_capacity(cap), lk: Vec::with_capacity(cap.min(1024)), cap, flush_ticks, last: i64::MIN }
    }

    /// The batches the map has not seen yet.
    pub fn pending(&self) -> usize {
        self.cs.len() + self.rd.len() + self.lk.len()
    }

    /// Capacities (tests: they never grow).
    pub fn capacity(&self) -> (usize, usize, usize) {
        (self.cs.capacity(), self.rd.capacity(), self.lk.capacity())
    }

    pub fn clear(&mut self) {
        self.cs.clear();
        self.rd.clear();
        self.lk.clear();
        self.last = i64::MIN;
    }

    #[inline]
    fn after_push(&mut self, ts: i64, sink: &mut dyn Sink) {
        if self.last == i64::MIN {
            self.last = ts;
        }
        if self.cs.len() >= self.cap || self.rd.len() >= self.cap || self.lk.len() >= self.lk.capacity() || ts - self.last >= self.flush_ticks {
            self.flush(sink);
            self.last = ts;
        }
    }

    #[inline]
    pub fn push_cswitch(&mut self, e: CSwitch, sink: &mut dyn Sink) {
        self.cs.push(e);
        self.after_push(e.ts, sink);
    }

    #[inline]
    pub fn push_ready(&mut self, e: ReadyThread, sink: &mut dyn Sink) {
        self.rd.push(e);
        self.after_push(e.ts, sink);
    }

    #[inline]
    pub fn push_link(&mut self, e: LinkEvent, sink: &mut dyn Sink) {
        self.lk.push(e);
        self.after_push(e.ts, sink);
    }

    /// Hands every batch to the sink (ReadyThread first: at equal times the map keeps the feed order).
    pub fn flush(&mut self, sink: &mut dyn Sink) {
        if !self.rd.is_empty() {
            sink.ready(&self.rd);
            self.rd.clear();
        }
        if !self.cs.is_empty() {
            sink.cswitch(&self.cs);
            self.cs.clear();
        }
        if !self.lk.is_empty() {
            sink.link(&self.lk);
            self.lk.clear();
        }
    }
}

/// The map handle as a sink (the `tm_feed_*` C ABI of the threadmap crate).
pub struct TmSink(pub *mut TmMap);

fn cstr(s: &str) -> CString {
    CString::new(s.replace('\0', " ")).unwrap_or_default()
}

impl Sink for TmSink {
    fn cswitch(&mut self, ev: &[CSwitch]) {
        tm::tm_feed_cswitch(self.0, ev.as_ptr(), ev.len());
    }
    fn ready(&mut self, ev: &[ReadyThread]) {
        tm::tm_feed_ready(self.0, ev.as_ptr(), ev.len());
    }
    fn link(&mut self, ev: &[LinkEvent]) {
        tm::tm_feed_link(self.0, ev.as_ptr(), ev.len());
    }
    fn process_start(&mut self, ts: i64, pid: u32, image: &str) {
        let c = cstr(image);
        tm::tm_process_start(self.0, ts, pid, c.as_ptr());
    }
    fn process_end(&mut self, ts: i64, pid: u32) {
        tm::tm_process_end(self.0, ts, pid);
    }
    fn thread_start(&mut self, ts: i64, tid: u32, pid: u32, name: Option<&str>, module: Option<&str>, offset: u64) {
        let n = name.map(cstr);
        let m = module.map(cstr);
        tm::tm_thread_start(self.0, ts, tid, pid, n.as_ref().map_or(std::ptr::null(), |c| c.as_ptr()), m.as_ref().map_or(std::ptr::null(), |c| c.as_ptr()), offset);
    }
    fn thread_end(&mut self, ts: i64, tid: u32) {
        tm::tm_thread_end(self.0, ts, tid);
    }
    fn thread_name(&mut self, ts: i64, tid: u32, name: &str) {
        let c = cstr(name);
        tm::tm_thread_name(self.0, ts, tid, c.as_ptr());
    }
}

// ================================================================ start addresses -> module+offset

/// Modules per process from the image events, and thread starts that came before their module (rundown order).
#[derive(Default)]
pub struct Modules {
    by_pid: HashMap<u32, Vec<(u64, u64, Arc<str>)>>,
    waiting: HashMap<u32, Vec<(u32, u64, i64)>>,
    waiting_total: usize,
}

const WAITING_MAX: usize = 50_000;

impl Modules {
    fn find(&self, pid: u32, addr: u64) -> Option<(Arc<str>, u64)> {
        // Kernel addresses (system threads) are in the drivers' list, which the rundown reports under pid 0.
        let pid = if addr >= 0xFFFF_8000_0000_0000 { 0 } else { pid };
        self.by_pid.get(&pid)?.iter().find(|(b, s, _)| addr >= *b && addr - *b < *s).map(|(b, _, n)| (n.clone(), addr - b))
    }

    /// A thread start: its module now, or later when the image event comes.
    pub fn thread(&mut self, ts: i64, pid: u32, tid: u32, addr: u64) -> Option<(Arc<str>, u64)> {
        if addr == 0 {
            return None;
        }
        let r = self.find(pid, addr);
        if r.is_none() && self.waiting_total < WAITING_MAX {
            self.waiting.entry(pid).or_default().push((tid, addr, ts));
            self.waiting_total += 1;
        }
        r
    }

    /// An image load: (tid, ts, module, offset) of the waiting thread starts it resolves.
    pub fn image(&mut self, pid: u32, base: u64, size: u64, name: &str) -> Vec<(u32, i64, Arc<str>, u64)> {
        let n: Arc<str> = Arc::from(name);
        self.by_pid.entry(pid).or_default().push((base, size, n.clone()));
        let mut out = Vec::new();
        let keys: Vec<u32> = if pid == 0 { self.waiting.keys().copied().collect() } else { vec![pid] };
        for k in keys {
            if let Some(w) = self.waiting.get_mut(&k) {
                let before = w.len();
                w.retain(|&(tid, addr, ts)| {
                    let kernel = addr >= 0xFFFF_8000_0000_0000;
                    if (kernel == (pid == 0)) && addr >= base && addr - base < size {
                        out.push((tid, ts, n.clone(), addr - base));
                        false
                    } else {
                        true
                    }
                });
                self.waiting_total -= before - w.len();
            }
        }
        out
    }

    pub fn process_end(&mut self, pid: u32) {
        self.by_pid.remove(&pid);
        if let Some(w) = self.waiting.remove(&pid) {
            self.waiting_total -= w.len();
        }
    }

    pub fn clear(&mut self) {
        *self = Self::default();
    }
}

// ================================================================ the session

/// The map handle (created once, lives as long as the process; internally locked, see threadmap.h).
struct MapPtr(*mut TmMap);
// SAFETY: a tm_map is internally locked (threadmap/src/ffi.rs) and never freed.
unsafe impl Send for MapPtr {}
unsafe impl Sync for MapPtr {}

struct Run {
    trace: u64,
    thread: Option<std::thread::JoinHandle<()>>,
    demo_stop: Option<Arc<AtomicBool>>,
    /// A trace thread that had not ended when `stop` gave up waiting. While it runs no new session is started, so
    /// there is never a second consumer (or a second session) next to it.
    leftover: Option<std::thread::JoinHandle<()>>,
}

/// The session: real time, a private system logger, 512 KB buffers (32..256).
pub(crate) fn session_spec() -> SessionSpec {
    SessionSpec { guid: SESSION_GUID, mode: raw::MODE_REAL_TIME | raw::MODE_SYSTEM_LOGGER, buffer_kb: 512, min_buffers: 32, max_buffers: 256, flags: 0 }
}

/// The properties block tried first: the session's spec with EVENT_TRACE_USE_MS_FLUSH_TIMER and FlushTimer 100 (ms).
/// Where Windows refuses it (ERROR_INVALID_PARAMETER) `raw::session_props(&session_spec())` is used: FlushTimer 1 (s).
pub(crate) fn session_props_ms() -> Vec<u64> {
    let spec = session_spec();
    let mut p = raw::session_props(&SessionSpec { mode: spec.mode | MS_FLUSH, ..spec });
    raw::w32(&mut p, raw::OFF_FLUSH_TIMER, FLUSH_TIMER_MS);
    p
}

pub(crate) struct Inner {
    map: MapPtr,
    feed: Mutex<Feeder>,
    modules: Mutex<Modules>,
    running: AtomicBool,
    demo: AtomicBool,
    error: Mutex<String>,
    run: Mutex<Run>,
    game: AtomicU32,
    reorder_ms: AtomicU32,
}

/// The thread map's trace (one per process, `ThreadMapTrace::global()`).
pub struct ThreadMapTrace {
    inner: Arc<Inner>,
}

impl ThreadMapTrace {
    fn new() -> Self {
        let freq = qpc::frequency().max(1) as u64;
        let map = tm::tm_new(freq);
        let flush_ticks = (freq as i64 * FLUSH_MS / 1000).max(1);
        ThreadMapTrace {
            inner: Arc::new(Inner {
                map: MapPtr(map),
                feed: Mutex::new(Feeder::new(BATCH, flush_ticks)),
                modules: Mutex::new(Modules::default()),
                running: AtomicBool::new(false),
                demo: AtomicBool::new(false),
                error: Mutex::new(String::new()),
                run: Mutex::new(Run { trace: 0, thread: None, demo_stop: None, leftover: None }),
                game: AtomicU32::new(0),
                reorder_ms: AtomicU32::new(REORDER_MS),
            }),
        }
    }

    pub fn global() -> &'static ThreadMapTrace {
        static G: OnceLock<ThreadMapTrace> = OnceLock::new();
        G.get_or_init(ThreadMapTrace::new)
    }

    /// The tm_map the page reads (snapshot, series, layout). Never NULL unless the allocation failed.
    pub fn handle(&self) -> *mut TmMap {
        self.inner.map.0
    }

    pub fn running(&self) -> bool {
        self.inner.running.load(Ordering::SeqCst)
    }

    pub fn demo(&self) -> bool {
        self.inner.demo.load(Ordering::SeqCst)
    }

    pub fn error(&self) -> String {
        lock(&self.inner.error).clone()
    }

    /// The game pid for the "game" class and the time strip (0 = none). The demo keeps its own.
    pub fn set_game(&self, pid: u32) {
        if self.demo() {
            return;
        }
        if self.inner.game.swap(pid, Ordering::SeqCst) != pid {
            tm::tm_set_game(self.handle(), pid);
        }
    }

    pub fn game(&self) -> u32 {
        self.inner.game.load(Ordering::SeqCst)
    }

    pub fn reorder_ms(&self) -> u32 {
        self.inner.reorder_ms.load(Ordering::SeqCst)
    }

    /// Hands what is batched to the map (the page's 10 Hz poll: a quiet trace does not keep events back).
    pub fn flush_pending(&self) {
        let mut sink = TmSink(self.handle());
        lock(&self.inner.feed).flush(&mut sink);
    }

    /// Events lost by the session so far (0 for the demo or when it is not running).
    pub fn lost(&self) -> u32 {
        if self.running() && !self.demo() {
            raw::lost(NAME)
        } else {
            0
        }
    }

    /// Starts the kernel trace (or the demo feed). The history starts empty. true when it runs.
    pub fn start(&self, demo: bool) -> bool {
        if self.running() && self.demo() == demo {
            return true;
        }
        self.stop();
        let h = self.handle();
        if h.is_null() {
            *lock(&self.inner.error) = L::t("the session could not be opened").to_string();
            return false;
        }
        tm::tm_clear(h);
        lock(&self.inner.feed).clear();
        lock(&self.inner.modules).clear();
        lock(&self.inner.error).clear();
        self.inner.demo.store(demo, Ordering::SeqCst);
        let ok = if demo { self.start_demo() } else { self.start_session() };
        self.inner.running.store(ok, Ordering::SeqCst);
        if !ok {
            self.inner.demo.store(false, Ordering::SeqCst);
        }
        ok
    }

    fn start_session(&self) -> bool {
        let h = self.handle();
        self.inner.game.store(u32::MAX, Ordering::SeqCst); // set again at the next poll
        raw::stop_by_name(NAME);
        // A trace thread of an earlier session that did not end yet: give it a second, never run two side by side.
        let left = lock(&self.inner.run).leftover.take();
        let left = raw::join_or_keep(left, 1000);
        if left.is_some() {
            lock(&self.inner.run).leftover = left;
            *lock(&self.inner.error) = L::t("the previous trace is still closing, open the page again in a moment").to_string();
            return false;
        }
        let spec = session_spec();
        let mut p = session_props_ms();
        let (mut rc, mut s) = raw::start_props(NAME, &mut p);
        let mut reorder = REORDER_MS;
        if rc == 87 {
            // ERROR_INVALID_PARAMETER: no ms flush timer here; the 1 s one needs a longer reorder window.
            (rc, s) = raw::start(NAME, &spec);
            reorder = REORDER_SLOW_MS;
        }
        if rc != 0 {
            *lock(&self.inner.error) = if rc == 5 { L::t("no administrator rights").to_string() } else { lf!("error {0}", rc) };
            return false;
        }
        if raw::set_flags(s, FLAGS) != 0 {
            raw::stop_by_name(NAME);
            *lock(&self.inner.error) = L::t("the session could not be opened").to_string();
            return false;
        }
        self.inner.reorder_ms.store(reorder, Ordering::SeqCst);
        tm::tm_set_reorder_ms(h, reorder);
        let ctx = Arc::as_ptr(&self.inner) as *const core::ffi::c_void;
        let t = raw::open(NAME, on_event, ctx);
        if t == u64::MAX {
            raw::stop_by_name(NAME);
            *lock(&self.inner.error) = L::t("the session could not be opened").to_string();
            return false;
        }
        let th = raw::spawn_process("threadmap", 0, Some(L::n("Thread map")), t, self.inner.clone());
        let mut r = lock(&self.inner.run);
        r.trace = t;
        r.thread = th;
        true
    }

    fn start_demo(&self) -> bool {
        let h = self.handle();
        tm::tm_set_reorder_ms(h, REORDER_MS);
        self.inner.reorder_ms.store(REORDER_MS, Ordering::SeqCst);
        tm::tm_set_game(h, super::thread_map_demo::GAME_PID);
        self.inner.game.store(super::thread_map_demo::GAME_PID, Ordering::SeqCst);
        // 30 s of history before now, made here so the first snapshot already has it (screenshots), then live in
        // 50 ms steps on a thread of its own.
        let freq = qpc::frequency();
        let now = qpc::timestamp();
        let mut d = Demo::new(freq, now - 30 * freq);
        {
            let mut sink = TmSink(h);
            let mut f = lock(&self.inner.feed);
            d.run_until(now, &mut f, &mut sink);
            f.flush(&mut sink);
        }
        let stop = Arc::new(AtomicBool::new(false));
        let inner = self.inner.clone();
        let st = stop.clone();
        let th = std::thread::Builder::new()
            .name("threadmap-demo".into())
            .spawn(move || {
                let mut sink = TmSink(inner.map.0);
                while !st.load(Ordering::SeqCst) {
                    std::thread::sleep(std::time::Duration::from_millis(50));
                    let mut f = lock(&inner.feed);
                    d.run_until(qpc::timestamp(), &mut f, &mut sink);
                    f.flush(&mut sink);
                }
            })
            .ok();
        let mut r = lock(&self.inner.run);
        r.thread = th;
        r.demo_stop = Some(stop);
        true
    }

    /// Stops the trace or the demo (the page was left, the window hid, the app exits). The map keeps its history.
    /// true when its thread ended (also when nothing ran); a thread that did not end in 2 s is kept and logged.
    pub fn stop(&self) -> bool {
        if !self.running() {
            return true;
        }
        self.inner.running.store(false, Ordering::SeqCst);
        let (t, th, ds) = {
            let mut r = lock(&self.inner.run);
            let t = std::mem::take(&mut r.trace);
            (t, r.thread.take(), r.demo_stop.take())
        };
        if let Some(ds) = ds {
            ds.store(true, Ordering::SeqCst);
        } else {
            raw::stop_by_name(NAME);
            if t != 0 && t != u64::MAX {
                raw::close(t);
            }
        }
        let left = raw::join_or_keep(th, JOIN_MS);
        let ended = left.is_none();
        if !ended {
            lock(&self.inner.run).leftover = left;
            crate::base::app_log::AppLog::add_sev(lf!("Thread map: the trace thread did not end within {0} ms", JOIN_MS), crate::base::app_log::Sev::Warn);
        }
        self.flush_pending();
        self.inner.demo.store(false, Ordering::SeqCst);
        ended
    }

    pub fn stop_orphan() {
        raw::stop_by_name(NAME);
    }

    /// Tests: one decoded EVENT_RECORD through the same path as the callback.
    #[cfg(test)]
    pub(crate) fn inner(&self) -> &Inner {
        &self.inner
    }
}

impl Inner {
    /// One event (the callback after reading the header). Unknown events are ignored.
    pub(crate) fn dispatch(&self, provider: &[u8; 16], opcode: u8, version: u8, ts: i64, cpu: u32, header_tid: u32, u: &[u8]) {
        let mut sink = TmSink(self.map.0);
        if *provider == THREAD_GUID {
            match opcode {
                36 => {
                    if let Some(e) = parse_cswitch(u, ts, cpu) {
                        lock(&self.feed).push_cswitch(e, &mut sink);
                    }
                }
                50 => {
                    if let Some(e) = parse_ready(u, ts, cpu, header_tid) {
                        lock(&self.feed).push_ready(e, &mut sink);
                    }
                }
                1 | 3 => {
                    if let Some((pid, tid, addr, name)) = parse_thread(u) {
                        let m = lock(&self.modules).thread(ts, pid, tid, addr);
                        let mut f = lock(&self.feed);
                        f.flush(&mut sink);
                        sink.thread_start(ts, tid, pid, name.as_deref(), m.as_ref().map(|(n, _)| &**n), m.as_ref().map_or(0, |(_, o)| *o));
                    }
                }
                2 => {
                    if let Some(tid) = u32_at(u, 4) {
                        let mut f = lock(&self.feed);
                        f.flush(&mut sink);
                        sink.thread_end(ts, tid);
                    }
                }
                72 => {
                    if let Some((_, tid, name)) = parse_set_name(u) {
                        let mut f = lock(&self.feed);
                        f.flush(&mut sink);
                        sink.thread_name(ts, tid, &name);
                    }
                }
                _ => {}
            }
        } else if *provider == PROCESS_GUID {
            match opcode {
                1 | 3 => {
                    if let Some((pid, image)) = parse_process(u, version) {
                        let mut f = lock(&self.feed);
                        f.flush(&mut sink);
                        sink.process_start(ts, pid, &image);
                    }
                }
                2 => {
                    if let Some(pid) = parse_process_end(u) {
                        lock(&self.modules).process_end(pid);
                        let mut f = lock(&self.feed);
                        f.flush(&mut sink);
                        sink.process_end(ts, pid);
                    }
                }
                _ => {}
            }
        } else if *provider == IMAGE_GUID && (opcode == 10 || opcode == 3) {
            if let Some((pid, base, size, file)) = parse_image(u) {
                let late = lock(&self.modules).image(pid, base, size, &file);
                if !late.is_empty() {
                    let mut f = lock(&self.feed);
                    f.flush(&mut sink);
                    for (tid, ts0, m, off) in late {
                        sink.thread_start(ts0, tid, pid_for_late(pid), None, Some(&m), off);
                    }
                }
            }
        }
    }
}

/// The pid passed with a late module: kernel modules (pid 0) resolve system threads, which belong to pid 4; the map
/// keeps a thread's process when the start event repeats it, and an unknown pid only fills the module.
fn pid_for_late(pid: u32) -> u32 {
    if pid == 0 { 4 } else { pid }
}

unsafe extern "system" fn on_event(record: *mut core::ffi::c_void) {
    let p = record as *const u8;
    let r = raw::rec(p);
    if r.context.is_null() || r.user.is_null() {
        return;
    }
    let inner = &*(r.context as *const Inner);
    if !inner.running.load(Ordering::Relaxed) {
        return;
    }
    let header_tid: u32 = raw::rd(p, 8);
    let version: u8 = raw::rd(p, 42);
    // SAFETY: ETW passes UserDataLength readable bytes at UserData.
    let u = std::slice::from_raw_parts(r.user, r.len as usize);
    inner.dispatch(&r.provider, r.opcode, version, r.stamp, r.cpu as u32, header_tid, u);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct Rec {
        calls: Vec<String>,
        cs: Vec<CSwitch>,
        rd: Vec<ReadyThread>,
    }

    impl Sink for Rec {
        fn cswitch(&mut self, ev: &[CSwitch]) {
            self.calls.push(format!("cs{}", ev.len()));
            self.cs.extend_from_slice(ev);
        }
        fn ready(&mut self, ev: &[ReadyThread]) {
            self.calls.push(format!("rd{}", ev.len()));
            self.rd.extend_from_slice(ev);
        }
        fn link(&mut self, ev: &[LinkEvent]) {
            self.calls.push(format!("lk{}", ev.len()));
        }
        fn process_start(&mut self, _ts: i64, pid: u32, image: &str) {
            self.calls.push(format!("ps{pid}:{image}"));
        }
        fn process_end(&mut self, _ts: i64, pid: u32) {
            self.calls.push(format!("pe{pid}"));
        }
        fn thread_start(&mut self, _ts: i64, tid: u32, pid: u32, name: Option<&str>, module: Option<&str>, offset: u64) {
            self.calls.push(format!("ts{tid}/{pid}:{}:{}+{offset:x}", name.unwrap_or("-"), module.unwrap_or("-")));
        }
        fn thread_end(&mut self, _ts: i64, tid: u32) {
            self.calls.push(format!("te{tid}"));
        }
        fn thread_name(&mut self, _ts: i64, tid: u32, name: &str) {
            self.calls.push(format!("tn{tid}:{name}"));
        }
    }

    fn cs(ts: i64) -> CSwitch {
        CSwitch { ts, cpu: 1, new_tid: 8, old_tid: 4, ..Default::default() }
    }

    fn utf16(s: &str) -> Vec<u8> {
        s.encode_utf16().chain([0]).flat_map(|c| c.to_le_bytes()).collect()
    }

    #[test]
    fn cswitch_and_ready_decode_the_x64_layout() {
        let mut u = vec![0u8; 24];
        u[0..4].copy_from_slice(&1234u32.to_le_bytes());
        u[4..8].copy_from_slice(&5678u32.to_le_bytes());
        u[8] = 15;
        u[9] = (-2i8) as u8;
        u[12] = 28; // WrPushLock
        u[13] = 1;
        u[14] = 5; // Waiting
        u[16..20].copy_from_slice(&77u32.to_le_bytes());
        let e = parse_cswitch(&u, 99, 3).unwrap();
        assert_eq!((e.ts, e.cpu, e.new_tid, e.old_tid, e.new_priority, e.old_priority), (99, 3, 1234, 5678, 15, -2));
        assert_eq!((e.old_wait_reason, e.old_wait_mode, e.old_state, e.new_wait_time), (28, 1, 5, 77));
        assert!(parse_cswitch(&u[..20], 0, 0).is_none());

        let mut r = vec![0u8; 8];
        r[0..4].copy_from_slice(&1234u32.to_le_bytes());
        r[4] = 1;
        r[5] = 2;
        r[6] = 1; // readied from a DPC
        let e = parse_ready(&r, 5, 2, 600).unwrap();
        assert_eq!((e.target_tid, e.readier_tid, e.adjust_reason, e.adjust_increment, e.flags), (1234, 600, 1, 2, 1));
        assert_eq!(parse_ready(&r, 5, 2, u32::MAX).unwrap().readier_tid, READIER_UNKNOWN);
        assert_eq!(parse_ready(&r, 5, 2, 1234).unwrap().readier_tid, READIER_UNKNOWN); // never itself
        assert_eq!(parse_ready(&r, 5, 2, 0).unwrap().readier_tid, 0); // idle: kernel work
        assert!(parse_ready(&r[..4], 0, 0, 0).is_none());
    }

    fn process_event(version: u8, sid: &[u8], image: &str) -> Vec<u8> {
        let mut u = vec![0u8; if version >= 4 { 36 } else { 32 }];
        u[8..12].copy_from_slice(&2340u32.to_le_bytes());
        u.extend_from_slice(sid);
        u.extend_from_slice(image.as_bytes());
        u.push(0);
        u.extend_from_slice(&utf16("\"C:\\Games\\cs2.exe\" -novid"));
        u
    }

    #[test]
    fn process_events_skip_the_sid_and_read_the_image() {
        // A null SID (4 bytes) and a TOKEN_USER (16) + S-1-5-21-a-b-c-d (8 + 4*5).
        let mut sid = vec![0u8; 16];
        sid[0] = 0x40; // a pointer, not zero
        sid.extend_from_slice(&[1, 5, 0, 0, 0, 0, 0, 5]);
        sid.extend_from_slice(&[21, 0, 0, 0, 1, 0, 0, 0, 2, 0, 0, 0, 3, 0, 0, 0, 0xe9, 3, 0, 0]);
        for v in [3u8, 4] {
            assert_eq!(parse_process(&process_event(v, &[0, 0, 0, 0], "cs2.exe"), v), Some((2340, "cs2.exe".into())));
            assert_eq!(parse_process(&process_event(v, &sid, "Discord.exe"), v), Some((2340, "Discord.exe".into())));
        }
        // A wrong version byte still finds the name at the other offset.
        assert_eq!(parse_process(&process_event(4, &sid, "dwm.exe"), 3), Some((2340, "dwm.exe".into())));
        // Garbage: the pid, no name.
        assert_eq!(parse_process(&[0u8; 12], 4), Some((0, String::new())));
        let mut end = vec![0u8; 16];
        end[8..12].copy_from_slice(&88u32.to_le_bytes());
        assert_eq!(parse_process_end(&end), Some(88));
    }

    fn thread_event(pid: u32, tid: u32, start: u64, name: Option<&str>) -> Vec<u8> {
        let mut u = vec![0u8; 72];
        u[0..4].copy_from_slice(&pid.to_le_bytes());
        u[4..8].copy_from_slice(&tid.to_le_bytes());
        u[48..56].copy_from_slice(&start.to_le_bytes());
        if let Some(n) = name {
            u.extend_from_slice(&utf16(n));
        }
        u
    }

    #[test]
    fn thread_events_and_names() {
        assert_eq!(parse_thread(&thread_event(2340, 2400, 0x7ff6_0000_1234, None)), Some((2340, 2400, 0x7ff6_0000_1234, None)));
        assert_eq!(parse_thread(&thread_event(2340, 2400, 1, Some("MainThread"))), Some((2340, 2400, 1, Some("MainThread".into()))));
        assert_eq!(parse_thread(&thread_event(2340, 2400, 1, Some(""))).unwrap().3, None);
        let mut sn = vec![0u8; 8];
        sn[0..4].copy_from_slice(&2340u32.to_le_bytes());
        sn[4..8].copy_from_slice(&2404u32.to_le_bytes());
        sn.extend_from_slice(&utf16("RenderThread"));
        assert_eq!(parse_set_name(&sn), Some((2340, 2404, "RenderThread".into())));
    }

    fn image_event(pid: u32, base: u64, size: u64, path: &str, off: usize) -> Vec<u8> {
        let mut u = vec![0u8; off];
        u[0..8].copy_from_slice(&base.to_le_bytes());
        u[8..16].copy_from_slice(&size.to_le_bytes());
        u[16..20].copy_from_slice(&pid.to_le_bytes());
        u.extend_from_slice(&utf16(path));
        u
    }

    #[test]
    fn image_events_give_the_file_name() {
        let p = "\\Device\\HarddiskVolume3\\Games\\cs2\\bin\\engine2.dll";
        assert_eq!(parse_image(&image_event(2340, 0x7ff6_0000_0000, 0x10_0000, p, 56)), Some((2340, 0x7ff6_0000_0000, 0x10_0000, "engine2.dll".into())));
        assert_eq!(parse_image(&image_event(2340, 1, 2, p, 48)).unwrap().3, "engine2.dll");
        assert_eq!(parse_image(&image_event(2340, 1, 2, "x.dll", 56)), None); // not a kernel path
    }

    #[test]
    fn start_addresses_resolve_now_or_when_the_module_comes() {
        let mut m = Modules::default();
        assert_eq!(m.thread(1, 100, 104, 0x5010), None); // module not known yet: waits
        assert_eq!(m.thread(1, 100, 108, 0), None); // no address: not kept
        let late = m.image(100, 0x5000, 0x100, "game.dll");
        assert_eq!(late.len(), 1);
        assert_eq!((late[0].0, &*late[0].2, late[0].3), (104, "game.dll", 0x10));
        assert_eq!(m.thread(2, 100, 112, 0x5020).map(|(n, o)| (n.to_string(), o)), Some(("game.dll".into(), 0x20)));
        // Kernel threads resolve against pid 0's drivers.
        assert_eq!(m.thread(3, 4, 16, 0xFFFF_F800_0000_0040), None);
        let k = m.image(0, 0xFFFF_F800_0000_0000, 0x1000, "ntoskrnl.exe");
        assert_eq!(k.len(), 1);
        assert_eq!(m.waiting_total, 0);
        m.process_end(100);
        assert_eq!(m.thread(4, 100, 116, 0x5020), None);
    }

    #[test]
    fn batches_flush_when_full_after_50_ms_and_before_metadata_without_growing() {
        let mut r = Rec::default();
        let mut f = Feeder::new(4, 500);
        let caps = f.capacity();
        for i in 0..4 {
            f.push_cswitch(cs(i), &mut r);
        }
        assert_eq!(r.calls, vec!["cs4"]); // full
        f.push_cswitch(cs(10), &mut r);
        f.push_ready(ReadyThread { ts: 20, target_tid: 8, ..Default::default() }, &mut r);
        assert_eq!(f.pending(), 2);
        f.push_cswitch(cs(600), &mut r); // 50 ms (500 ticks here) after the last flush
        assert_eq!(r.calls, vec!["cs4", "rd1", "cs2"]);
        assert_eq!(f.pending(), 0);
        f.push_cswitch(cs(610), &mut r);
        f.flush(&mut r);
        f.flush(&mut r); // nothing left: no call
        assert_eq!(r.calls.len(), 4);
        assert_eq!(f.capacity(), caps);
        assert_eq!(r.cs.len(), 7);
        assert!(r.cs.windows(2).all(|w| w[0].ts <= w[1].ts));
    }

    /// The session asks for a 100 ms flush timer in ms (the ms flag set), a real-time private system logger; the
    /// fallback block is the 1 s one in seconds (no ms flag). Neither has FlushTimer 0 or an ms value in seconds.
    #[test]
    fn session_properties_flush_timer_and_mode() {
        let p = session_props_ms();
        let mode = raw::r32(&p, raw::OFF_LOG_FILE_MODE);
        assert_eq!(mode, 0x100 | 0x0200_0000 | 0x10);
        assert_eq!(raw::r32(&p, raw::OFF_FLUSH_TIMER), 100);
        let f = raw::session_props(&session_spec());
        assert_eq!(raw::r32(&f, raw::OFF_LOG_FILE_MODE), 0x100 | 0x0200_0000);
        assert_eq!(raw::r32(&f, raw::OFF_FLUSH_TIMER), 1);
        assert_eq!(raw::r32(&p, 48), 512); // BufferSize KB
        assert_eq!(raw::r32(&p, 72), 0); // EnableFlags go in with TraceSetInformation
    }

    /// Start and stop again and again: each stop ends its thread (the demo feed here; the real session fails on
    /// Linux and leaves nothing running).
    #[test]
    fn stop_ends_the_feed_thread_every_time() {
        let t = ThreadMapTrace::new();
        for _ in 0..3 {
            assert!(t.start(true));
            assert!(t.running() && t.demo());
            assert!(t.start(true)); // already running: no second thread
            assert!(t.stop());
            assert!(!t.running());
            let r = lock(&t.inner.run);
            assert!(r.thread.is_none() && r.leftover.is_none() && r.demo_stop.is_none());
        }
        assert!(t.stop()); // nothing runs: nothing to wait for
        if t.start(false) {
            // an elevated Windows host (CI runner): the real kernel trace starts; stop must end it too
            assert!(t.running() && !t.demo());
            assert!(t.stop());
            assert!(!t.running());
        } else {
            // Linux/Wine: no kernel trace
            assert!(!t.running());
            assert!(!t.error().is_empty());
        }
        assert!(lock(&t.inner.run).thread.is_none());
    }

    /// The callback path on a fake trace (no session): records go through dispatch into the map in feed order.
    #[test]
    fn dispatch_feeds_the_map_and_metadata_flushes_first() {
        let t = ThreadMapTrace::new();
        let i = t.inner();
        assert!(!t.handle().is_null());
        let f = qpc::frequency();
        tm::tm_set_game(t.handle(), 2340);
        i.dispatch(&PROCESS_GUID, 3, 4, f, 0, 0, &process_event(4, &[0, 0, 0, 0], "cs2.exe"));
        i.dispatch(&IMAGE_GUID, 3, 3, f, 0, 0, &image_event(2340, 0x7ff6_0000_0000, 0x10_0000, "\\Device\\X\\cs2\\engine2.dll", 56));
        i.dispatch(&THREAD_GUID, 3, 4, f, 0, 0, &thread_event(2340, 2400, 0x7ff6_0000_1000, Some("MainThread")));
        i.dispatch(&THREAD_GUID, 3, 4, f, 0, 0, &thread_event(2340, 2404, 0x7ff6_0000_2000, None));
        let tick = f / 1000; // 1 ms
        for k in 0..400i64 {
            let ts = f + k * tick;
            let mut rd = vec![0u8; 8];
            rd[0..4].copy_from_slice(&2404u32.to_le_bytes());
            i.dispatch(&THREAD_GUID, 50, 2, ts, 1, 2400, &rd);
            let mut c = vec![0u8; 24];
            c[0..4].copy_from_slice(&2404u32.to_le_bytes());
            c[14] = 2;
            i.dispatch(&THREAD_GUID, 36, 4, ts + tick / 10, 1, 0, &c);
            let mut c = vec![0u8; 24];
            c[4..8].copy_from_slice(&2404u32.to_le_bytes());
            c[12] = 6;
            c[14] = 5;
            i.dispatch(&THREAD_GUID, 36, 4, ts + tick / 2, 1, 0, &c);
        }
        t.flush_pending();
        tm::tm_flush(t.handle());
        let q = CString::new(r#"{"focus_pid":2340,"expand":[2340],"min_rate":0,"top_n":0,"merge":false}"#).unwrap();
        let s = tm::tm_snapshot_json(t.handle(), q.as_ptr());
        assert!(!s.is_null());
        let js = unsafe { std::ffi::CStr::from_ptr(s) }.to_string_lossy().into_owned();
        tm::tm_free_str(s);
        let v: serde_json::Value = serde_json::from_str(&js).unwrap();
        let nodes = v["nodes"].as_array().unwrap();
        let main = nodes.iter().find(|n| n["label"] == "MainThread").expect("MainThread");
        let other = nodes.iter().find(|n| n["label"].as_str().unwrap_or("").starts_with("engine2.dll+0x2000")).expect("module-named thread");
        assert_eq!(v["game_pid"], 2340);
        assert!(nodes.iter().any(|n| n["label"] == "cs2.exe" && n["class"] == "game"));
        assert!(v["links"].as_array().unwrap().iter().any(|l| l["from"] == main["id"] && l["to"] == other["id"] && l["kind"] == "wake"));
        assert!(other["cpu"].as_f64().unwrap() > 0.3);
        assert_eq!(other["core"], 1);
    }
}
