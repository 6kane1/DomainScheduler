//! Raw ETW plumbing shared by the sessions of this area (the C# classes each carry the same P/Invoke block:
//! `StartSession`, `StopByName`, `Props`, `Flush`, `Lost`, `Open`, the trace thread). The byte offsets into
//! EVENT_TRACE_PROPERTIES and EVENT_RECORD are the ones the C# code writes and reads (x64).
//! On other platforms every call fails with ERROR_NOT_SUPPORTED (50), so the logic compiles and tests on Linux.

/// `Guid.ToByteArray()`: the in-memory layout of a GUID (Data1..3 little-endian, Data4 as it is).
pub const fn guid_bytes(g: u128) -> [u8; 16] {
    let d1 = (g >> 96) as u32;
    let d2 = (g >> 80) as u16;
    let d3 = (g >> 64) as u16;
    let d4 = (g as u64).to_be_bytes();
    let a = d1.to_le_bytes();
    let b = d2.to_le_bytes();
    let c = d3.to_le_bytes();
    [a[0], a[1], a[2], a[3], b[0], b[1], c[0], c[1], d4[0], d4[1], d4[2], d4[3], d4[4], d4[5], d4[6], d4[7]]
}

pub const THREAD_GUID: [u8; 16] = guid_bytes(0x3d6fa8d1_fe05_11d0_9dda_00c04fd7ba7c);
pub const PAGE_FAULT_GUID: [u8; 16] = guid_bytes(0x3d6fa8d3_fe05_11d0_9dda_00c04fd7ba7c);
pub const PERF_INFO_GUID: [u8; 16] = guid_bytes(0xce1dbfb4_137e_4da6_87b0_3f59aa102cbc);
pub const DXGI_GUID: [u8; 16] = guid_bytes(0xCA11C036_0102_4A2D_A6AD_F03CFED5D3C9);
pub const DXGI_U128: u128 = 0xCA11C036_0102_4A2D_A6AD_F03CFED5D3C9;

/// The hasher of the per-event maps (thread ids, routine addresses): one multiply instead of SipHash. The keys come
/// from the kernel, not from an attacker, so a DoS-resistant hash buys nothing on the hot path.
#[derive(Clone, Copy, Default)]
pub struct FastHash(u64);

impl std::hash::Hasher for FastHash {
    fn finish(&self) -> u64 {
        // the product's high half is well mixed; fold it into the low bits the table indexes with (thread ids are
        // multiples of 4)
        self.0 ^ (self.0 >> 32)
    }
    fn write(&mut self, bytes: &[u8]) {
        for b in bytes {
            self.0 = (self.0.rotate_left(5) ^ *b as u64).wrapping_mul(0x51_7c_c1_b7_27_22_0a_95);
        }
    }
    fn write_u32(&mut self, v: u32) {
        self.write_u64(v as u64);
    }
    fn write_u64(&mut self, v: u64) {
        self.0 = (self.0.rotate_left(5) ^ v).wrapping_mul(0x51_7c_c1_b7_27_22_0a_95);
    }
    fn write_usize(&mut self, v: usize) {
        self.write_u64(v as u64);
    }
}

pub type FastBuild = std::hash::BuildHasherDefault<FastHash>;
pub type FastMap<K, V> = std::collections::HashMap<K, V, FastBuild>;

/// EVENT_TRACE_PROPERTIES + room for the logger name (`120 + 2048` bytes, as the C# code allocates).
pub const PROPS_SIZE: usize = 120 + 2048;

/// A zeroed properties block with BufferSize and LoggerNameOffset set (C# `Props`). 8-byte aligned.
pub fn props() -> Vec<u64> {
    let mut p = vec![0u64; PROPS_SIZE.div_ceil(8)];
    w32(&mut p, 0, PROPS_SIZE as u32);
    w32(&mut p, 116, 120);
    p
}

pub fn w32(p: &mut [u64], off: usize, v: u32) {
    let b = bytes_mut(p);
    b[off..off + 4].copy_from_slice(&v.to_le_bytes());
}

pub fn r32(p: &[u64], off: usize) -> u32 {
    let b = bytes(p);
    u32::from_le_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]])
}

pub fn bytes(p: &[u64]) -> &[u8] {
    // SAFETY: u64 storage viewed as bytes; length in bytes is len*8.
    unsafe { std::slice::from_raw_parts(p.as_ptr() as *const u8, p.len() * 8) }
}

pub fn bytes_mut(p: &mut [u64]) -> &mut [u8] {
    // SAFETY: as above.
    unsafe { std::slice::from_raw_parts_mut(p.as_mut_ptr() as *mut u8, p.len() * 8) }
}

/// The session parameters the C# `StartSession` helpers write.
#[derive(Clone, Copy, Debug)]
pub struct SessionSpec {
    pub guid: [u8; 16],
    /// LogFileMode (0x100 REAL_TIME, 0x02000000 SYSTEM_LOGGER)
    pub mode: u32,
    pub buffer_kb: u32,
    pub min_buffers: u32,
    pub max_buffers: u32,
    /// EnableFlags (0: not written)
    pub flags: u32,
}

/// EVENT_TRACE_PROPERTIES offsets (x64) the sessions write: LogFileMode, FlushTimer.
pub const OFF_LOG_FILE_MODE: usize = 64;
pub const OFF_FLUSH_TIMER: usize = 68;
/// LogFileMode bits: EVENT_TRACE_USE_MS_FLUSH_TIMER (FlushTimer in ms, Windows 8+), EVENT_TRACE_REAL_TIME_MODE,
/// EVENT_TRACE_SYSTEM_LOGGER_MODE.
pub const MODE_MS_FLUSH_TIMER: u32 = 0x10;
pub const MODE_REAL_TIME: u32 = 0x100;
pub const MODE_SYSTEM_LOGGER: u32 = 0x0200_0000;

/// The properties block of a new session: Wnode.Guid, ClientContext 1 (QPC), WNODE_FLAG_TRACED_GUID, buffers,
/// mode, FlushTimer 1 s, EnableFlags.
pub fn session_props(s: &SessionSpec) -> Vec<u64> {
    let mut p = props();
    bytes_mut(&mut p)[24..40].copy_from_slice(&s.guid);
    w32(&mut p, 40, 1);
    w32(&mut p, 44, 0x20000);
    w32(&mut p, 48, s.buffer_kb);
    w32(&mut p, 52, s.min_buffers);
    w32(&mut p, 56, s.max_buffers);
    w32(&mut p, OFF_LOG_FILE_MODE, s.mode);
    // FlushTimer: 1 s (the field is in seconds unless the mode carries EVENT_TRACE_USE_MS_FLUSH_TIMER; 0 would mean
    // "no timer", a buffer then waits until it is full).
    w32(&mut p, OFF_FLUSH_TIMER, 1);
    w32(&mut p, 72, s.flags);
    p
}

/// The event callback: gets the EVENT_RECORD and the context passed to `open`.
pub type Callback = unsafe extern "system" fn(record: *mut core::ffi::c_void);

/// Reads a value at a byte offset of an event record (or its user data).
///
/// # Safety
/// `p + off` must point to `size_of::<T>()` readable bytes.
pub unsafe fn rd<T: Copy>(p: *const u8, off: usize) -> T {
    (p.add(off) as *const T).read_unaligned()
}

/// The fields of an EVENT_RECORD the sessions read (C# reads them by offset).
#[derive(Clone, Copy, Debug)]
pub struct Rec {
    pub flags: u16,
    pub pid: u32,
    pub stamp: i64,
    pub provider: [u8; 16],
    pub id: u16,
    pub opcode: u8,
    pub cpu: u16,
    pub len: u16,
    pub user: *const u8,
    pub context: *const core::ffi::c_void,
}

/// Reads the header fields of an EVENT_RECORD.
///
/// # Safety
/// `r` must be the EVENT_RECORD pointer ETW passes to the callback.
pub unsafe fn rec(r: *const u8) -> Rec {
    let flags: u16 = rd(r, 4);
    let cpu = if flags & 0x200 != 0 { rd::<u16>(r, 80) } else { rd::<u8>(r, 80) as u16 };
    Rec {
        flags,
        pid: rd(r, 12),
        stamp: rd(r, 16),
        provider: rd(r, 24),
        id: rd(r, 40),
        opcode: rd(r, 45),
        cpu,
        len: rd(r, 86),
        user: rd::<*const u8>(r, 96),
        context: rd::<*const core::ffi::c_void>(r, 104),
    }
}

#[cfg(windows)]
mod imp {
    use super::*;
    use crate::native::win::wide;
    use windows_sys::core::GUID;
    use windows_sys::Win32::System::Diagnostics::Etw::{
        CloseTrace, ControlTraceW, EnableTraceEx2, OpenTraceW, ProcessTrace, StartTraceW, TraceSetInformation, CONTROLTRACE_HANDLE,
        ENABLE_TRACE_PARAMETERS, EVENT_TRACE_LOGFILEW, EVENT_TRACE_PROPERTIES, PROCESSTRACE_HANDLE,
    };

    /// `StartTraceW(out handle, name, props)`: (error code, session handle).
    pub fn start(name: &str, s: &SessionSpec) -> (u32, u64) {
        start_props(name, &mut session_props(s))
    }

    /// `StartTraceW` with a properties block the caller prepared (e.g. a FlushTimer in ms).
    pub fn start_props(name: &str, p: &mut [u64]) -> (u32, u64) {
        let n = wide(name);
        let mut h = CONTROLTRACE_HANDLE { Value: 0 };
        // SAFETY: p is a zeroed, 8-aligned EVENT_TRACE_PROPERTIES with room for the name.
        let rc = unsafe { StartTraceW(&mut h, n.as_ptr(), p.as_mut_ptr() as *mut EVENT_TRACE_PROPERTIES) };
        (rc, h.Value)
    }

    fn control(name: &str, p: &mut [u64], code: u32) -> u32 {
        let n = wide(name);
        // SAFETY: as above; handle 0 = by name.
        unsafe { ControlTraceW(CONTROLTRACE_HANDLE { Value: 0 }, n.as_ptr(), p.as_mut_ptr() as *mut EVENT_TRACE_PROPERTIES, code) }
    }

    /// `ControlTraceW(0, name, props, STOP)`.
    pub fn stop_by_name(name: &str) {
        let mut p = props();
        control(name, &mut p, 1);
    }

    /// `ControlTraceW(0, name, props, FLUSH)`.
    pub fn flush(name: &str) {
        let mut p = props();
        control(name, &mut p, 3);
    }

    /// EventsLost + RealTimeBuffersLost of a session (0 when the query fails).
    pub fn lost(name: &str) -> u32 {
        let mut p = props();
        if control(name, &mut p, 0) == 0 { r32(&p, 88).wrapping_add(r32(&p, 100)) } else { 0 }
    }

    /// `TraceSetInformation(handle, TraceSystemTraceEnableFlagsInfo, flags, 4)`.
    pub fn set_flags(session: u64, flags: u32) -> u32 {
        let v = flags;
        // SAFETY: 4 readable bytes.
        unsafe { TraceSetInformation(CONTROLTRACE_HANDLE { Value: session }, 4, &v as *const u32 as *const core::ffi::c_void, 4) }
    }

    /// `EnableTraceEx2(session, provider, ENABLE, level, any, all, 0, params)`; `filters` = (type, bytes).
    pub fn enable(session: u64, provider: u128, level: u8, any: u64, filters: &[(u32, Vec<u8>)]) -> u32 {
        use windows_sys::Win32::System::Diagnostics::Etw::EVENT_FILTER_DESCRIPTOR;
        let g = GUID::from_u128(provider);
        if filters.is_empty() {
            // SAFETY: plain arguments.
            return unsafe { EnableTraceEx2(CONTROLTRACE_HANDLE { Value: session }, &g, 1, level, any, 0, 0, std::ptr::null()) };
        }
        let mut descs: Vec<EVENT_FILTER_DESCRIPTOR> =
            filters.iter().map(|(t, b)| EVENT_FILTER_DESCRIPTOR { Ptr: b.as_ptr() as u64, Size: b.len() as u32, Type: *t }).collect();
        let params = ENABLE_TRACE_PARAMETERS {
            Version: 2,
            EnableProperty: 0,
            ControlFlags: 0,
            SourceId: GUID::from_u128(0),
            EnableFilterDesc: descs.as_mut_ptr(),
            FilterDescCount: descs.len() as u32,
        };
        // SAFETY: descs and their buffers live until the call returns.
        unsafe { EnableTraceEx2(CONTROLTRACE_HANDLE { Value: session }, &g, 1, level, any, 0, 0, &params) }
    }

    /// `OpenTraceW` of a real-time session with EVENT_RECORD callbacks and raw (QPC) time stamps; u64::MAX on failure.
    pub fn open(name: &str, cb: Callback, context: *const core::ffi::c_void) -> u64 {
        let mut n = wide(name);
        // SAFETY: EVENT_TRACE_LOGFILEW is plain data; all-zero is valid.
        let mut lf: EVENT_TRACE_LOGFILEW = unsafe { std::mem::zeroed() };
        lf.LoggerName = n.as_mut_ptr();
        lf.Anonymous1.ProcessTraceMode = 0x100 | 0x10000000 | 0x1000;
        // SAFETY: the callback has the EVENT_RECORD_CALLBACK signature (one pointer argument, system ABI).
        lf.Anonymous2.EventRecordCallback = Some(unsafe { std::mem::transmute::<Callback, unsafe extern "system" fn(*mut windows_sys::Win32::System::Diagnostics::Etw::EVENT_RECORD)>(cb) });
        lf.Context = context as *mut core::ffi::c_void;
        // SAFETY: lf is valid for the call.
        unsafe { OpenTraceW(&mut lf) }.Value
    }

    /// `ProcessTrace(new[] { trace }, 1, null, null)` (blocks until the session ends); its result code.
    pub fn process(trace: u64) -> u32 {
        let h = PROCESSTRACE_HANDLE { Value: trace };
        // SAFETY: one handle.
        unsafe { ProcessTrace(&h, 1, std::ptr::null(), std::ptr::null()) }
    }

    pub fn close(trace: u64) {
        // SAFETY: a handle from OpenTraceW.
        unsafe { CloseTrace(PROCESSTRACE_HANDLE { Value: trace }) };
    }

    /// `Thread.Priority` of the calling thread: -1 BelowNormal, 1 AboveNormal.
    pub fn set_priority(p: i32) {
        use windows_sys::Win32::System::Threading::{GetCurrentThread, SetThreadPriority};
        // SAFETY: pseudo handle of this thread.
        unsafe { SetThreadPriority(GetCurrentThread(), p) };
    }
}

#[cfg(not(windows))]
mod imp {
    use super::*;
    pub fn start(_name: &str, _s: &SessionSpec) -> (u32, u64) {
        (50, 0)
    }
    pub fn start_props(_name: &str, _p: &mut [u64]) -> (u32, u64) {
        (50, 0)
    }
    pub fn stop_by_name(_name: &str) {}
    pub fn flush(_name: &str) {}
    pub fn lost(_name: &str) -> u32 {
        0
    }
    pub fn set_flags(_session: u64, _flags: u32) -> u32 {
        50
    }
    pub fn enable(_session: u64, _provider: u128, _level: u8, _any: u64, _filters: &[(u32, Vec<u8>)]) -> u32 {
        50
    }
    pub fn open(_name: &str, _cb: Callback, _context: *const core::ffi::c_void) -> u64 {
        u64::MAX
    }
    pub fn process(_trace: u64) -> u32 {
        0
    }
    pub fn close(_trace: u64) {}
    pub fn set_priority(_p: i32) {}
}

pub use imp::*;

/// A pointer that may cross to the trace thread (the context of the callback).
pub struct SendPtr(pub *const core::ffi::c_void);
// SAFETY: the pointee is an `Arc`-owned, thread-safe object the thread keeps alive.
unsafe impl Send for SendPtr {}

/// Starts the thread that runs `ProcessTrace` (C#: background thread, name, priority, SelfThreads name).
/// `keep` stays alive on the thread until ProcessTrace returns (it owns what the callback context points to).
/// When the session ends, the thread's processor time stays under its SelfThreads name (`SelfThreads::ended`), and
/// a ProcessTrace that failed (not 0 and not ERROR_CANCELLED) is logged once per session.
pub fn spawn_process<K: Send + 'static>(thread_name: &str, priority: i32, self_name: Option<&'static str>, trace: u64, keep: K) -> Option<std::thread::JoinHandle<()>> {
    let thread = thread_name.to_string();
    std::thread::Builder::new()
        .name(thread_name.to_string())
        .spawn(move || {
            set_priority(priority);
            if let Some(n) = self_name {
                crate::native::self_threads::SelfThreads::register(n);
            }
            let rc = process(trace);
            drop(keep);
            if rc != 0 && rc != 1223 {
                crate::base::app_log::AppLog::add_sev(format!("ETW {thread}: ProcessTrace {rc}"), crate::base::app_log::Sev::Warn);
            }
            if let Some(n) = self_name {
                crate::native::self_threads::SelfThreads::ended(n);
            }
        })
        .ok()
}

/// `Thread.Join(ms)`: waits at most ms for the thread; a thread still running is left behind (it is a background
/// thread in C#).
pub fn join_timeout(h: Option<std::thread::JoinHandle<()>>, ms: u64) {
    let _ = join_or_keep(h, ms);
}

/// Waits at most ms for the thread and joins it; a thread still running after that is handed back (the caller keeps
/// it and must not start a second consumer of the same session meanwhile).
pub fn join_or_keep(h: Option<std::thread::JoinHandle<()>>, ms: u64) -> Option<std::thread::JoinHandle<()>> {
    let h = h?;
    let until = std::time::Instant::now() + std::time::Duration::from_millis(ms);
    while !h.is_finished() {
        if std::time::Instant::now() >= until {
            return Some(h);
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let _ = h.join();
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guid_layout_is_the_dotnet_one() {
        // new Guid("3d6fa8d1-fe05-11d0-9dda-00c04fd7ba7c").ToByteArray()
        assert_eq!(THREAD_GUID, [0xd1, 0xa8, 0x6f, 0x3d, 0x05, 0xfe, 0xd0, 0x11, 0x9d, 0xda, 0x00, 0xc0, 0x4f, 0xd7, 0xba, 0x7c]);
        let p = session_props(&SessionSpec { guid: THREAD_GUID, mode: 0x100, buffer_kb: 64, min_buffers: 16, max_buffers: 128, flags: 3 });
        assert_eq!(r32(&p, 0), PROPS_SIZE as u32);
        assert_eq!(r32(&p, 116), 120);
        assert_eq!(r32(&p, 44), 0x20000);
        assert_eq!(r32(&p, 72), 3);
        assert_eq!(bytes(&p)[24], 0xd1);
        // Every session of this area: FlushTimer 1 (seconds: no ms flag), never 0.
        assert_eq!(r32(&p, OFF_FLUSH_TIMER), 1);
        assert_eq!(r32(&p, OFF_LOG_FILE_MODE), 0x100);
        assert_eq!(r32(&p, OFF_LOG_FILE_MODE) & MODE_MS_FLUSH_TIMER, 0);
    }

    #[test]
    fn join_or_keep_joins_a_finished_thread_and_hands_back_a_running_one() {
        let done = std::thread::spawn(|| {});
        assert!(join_or_keep(Some(done), 2000).is_none());
        let (tx, rx) = std::sync::mpsc::channel::<()>();
        let busy = std::thread::spawn(move || {
            let _ = rx.recv();
        });
        let kept = join_or_keep(Some(busy), 20).expect("still running: handed back");
        drop(tx);
        assert!(join_or_keep(Some(kept), 2000).is_none());
        assert!(join_or_keep(None, 0).is_none());
    }
}
