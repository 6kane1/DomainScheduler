//! IGameProbe over the Windows API (port of `WinProbe.cs`).

use super::win::{self, close_handle, h, open_process, NtQuerySystemInformation, PROCESS_QUERY_LIMITED_INFORMATION};
use crate::base::app_log::{AppLog, Sev};
use super::IGameProbe;
use crate::base::ci;
use crate::base::hooks::lock;
use crate::base::time::{DateTime, TimeSpan};
use indexmap::IndexSet;
use std::collections::HashMap;
use std::sync::Mutex;
use windows_sys::Win32::Foundation::{BOOL, HWND, LPARAM};
use windows_sys::Win32::System::RemoteDesktop::ProcessIdToSessionId;
use windows_sys::Win32::UI::WindowsAndMessaging::{EnumWindows, GetForegroundWindow, GetWindow, GetWindowThreadProcessId, IsWindowVisible};

const PROCESS_VM_READ: u32 = 0x0010;
const GW_OWNER: u32 = 4;
const GRAPHICS_MODULES: [&str; 4] = ["d3d11.dll", "d3d12.dll", "vulkan-1.dll", "opengl32.dll"];

/// `WinGameProbe`.
pub struct WinGameProbe {
    graphics: Mutex<HashMap<(u32, i64), (bool, DateTime)>>,
    self_pid: u32,
}

impl Default for WinGameProbe {
    fn default() -> Self {
        Self::new()
    }
}

impl WinGameProbe {
    pub fn new() -> Self {
        WinGameProbe { graphics: Mutex::new(HashMap::new()), self_pid: std::process::id() }
    }
}

/// 1.17.2: creation time, CPU time (kernel + user, 100 ns) and session of a process from the system's process list
/// (NtQuerySystemInformation, SystemProcessInformation = 5). Needs no handle to the process: a kernel anti-cheat
/// (FACEIT) can refuse even PROCESS_QUERY_LIMITED_INFORMATION after a match starts, and then Toolbox took the running
/// game for ended and could not find it again. Read only. x64 layout: CreateTime 0x20, UserTime 0x28, KernelTime 0x30,
/// UniqueProcessId 0x50, SessionId 0x64.
pub(crate) fn system_process_times(pid: u32) -> Option<(i64, i64, u32)> {
    let mut size: u32 = 1 << 20;
    for _ in 0..4 {
        let mut buf = vec![0u64; (size as usize).div_ceil(8)];
        let mut got: u32 = 0;
        let st = unsafe { NtQuerySystemInformation(5, buf.as_mut_ptr() as *mut _, size, &mut got) };
        if st == 0xC0000004u32 as i32 {
            size = got.max(size).saturating_mul(2);
            continue;
        }
        if st != 0 {
            return None;
        }
        let b = unsafe { std::slice::from_raw_parts(buf.as_ptr() as *const u8, size as usize) };
        let rd = |o: usize, n: usize| -> Option<&[u8]> { b.get(o..o + n) };
        let mut off = 0usize;
        loop {
            let next = u32::from_le_bytes(rd(off, 4)?.try_into().ok()?) as usize;
            let id = u64::from_le_bytes(rd(off + 0x50, 8)?.try_into().ok()?) as u32;
            if id == pid {
                let created = i64::from_le_bytes(rd(off + 0x20, 8)?.try_into().ok()?);
                let user = i64::from_le_bytes(rd(off + 0x28, 8)?.try_into().ok()?);
                let kernel = i64::from_le_bytes(rd(off + 0x30, 8)?.try_into().ok()?);
                let session = u32::from_le_bytes(rd(off + 0x64, 4)?.try_into().ok()?);
                return Some((created, user + kernel, session));
            }
            if next == 0 {
                return None;
            }
            off += next;
        }
    }
    None
}

/// Logs once per process that it cannot be opened or read and the system list is used instead.
fn note_unopenable(pid: u32, err: u32) {
    static SEEN: Mutex<Option<std::collections::HashSet<u32>>> = Mutex::new(None);
    let mut g = lock(&SEEN);
    if g.get_or_insert_with(Default::default).insert(pid) {
        AppLog::add_sev(
            format!("PID {pid}: the process cannot be opened or read (error {err}, an anti-cheat?); its CPU time is read from the system's process list"),
            Sev::Info,
        );
    }
}

unsafe extern "system" fn enum_cb(hwnd: HWND, param: LPARAM) -> BOOL {
    // SAFETY: param is the &mut IndexSet passed by window_pids for the duration of EnumWindows.
    let set = &mut *(param as *mut IndexSet<u32>);
    if IsWindowVisible(hwnd) != 0 && GetWindow(hwnd, GW_OWNER).is_null() {
        let mut pid: u32 = 0;
        GetWindowThreadProcessId(hwnd, &mut pid);
        if pid != 0 {
            set.insert(pid);
        }
    }
    1
}

impl IGameProbe for WinGameProbe {
    fn self_pid(&self) -> u32 {
        self.self_pid
    }

    fn foreground_pid(&self) -> u32 {
        let w = unsafe { GetForegroundWindow() };
        if w.is_null() {
            return 0;
        }
        let mut pid: u32 = 0;
        unsafe { GetWindowThreadProcessId(w, &mut pid) };
        pid
    }

    fn window_pids(&self) -> IndexSet<u32> {
        let mut set: IndexSet<u32> = IndexSet::new();
        unsafe { EnumWindows(Some(enum_cb), &mut set as *mut IndexSet<u32> as LPARAM) };
        set
    }

    /// A graphics API module is loaded; a positive answer is kept, a negative one is checked again after 10 s.
    fn has_graphics_module(&self, pid: u32) -> bool {
        let (mut created, mut cpu) = (0i64, 0i64);
        if !self.cpu_time(pid, &mut created, &mut cpu) {
            return false;
        }
        if let Some(c) = lock(&self.graphics).get(&(pid, created)) {
            if c.0 || DateTime::utc_now() - c.1 < TimeSpan::from_secs_i(10) {
                return c.0;
            }
        }
        let mut has = false;
        let x = open_process(PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_VM_READ, pid);
        if !x.is_null() {
            has = win::modules(x).iter().any(|m| GRAPHICS_MODULES.iter().any(|g| ci::eq(g, &m.name)));
            close_handle(x);
        }
        let mut g = lock(&self.graphics);
        if g.len() > 512 {
            g.clear();
        }
        g.insert((pid, created), (has, DateTime::utc_now()));
        has
    }

    fn cpu_time(&self, pid: u32, created: &mut i64, cpu100ns: &mut i64) -> bool {
        *created = 0;
        *cpu100ns = 0;
        let x = open_process(PROCESS_QUERY_LIMITED_INFORMATION, pid);
        if x.is_null() {
            let err = unsafe { windows_sys::Win32::Foundation::GetLastError() };
            return match system_process_times(pid) {
                Some((c, cpu, _)) => {
                    note_unopenable(pid, err);
                    *created = c;
                    *cpu100ns = cpu;
                    true
                }
                None => false,
            };
        }
        let r = match win::get_process_times(x) {
            Some((c, _, k, u)) => {
                *created = c;
                *cpu100ns = k + u;
                true
            }
            // 1.17.2: the open worked but the handle has no rights left (a kernel anti-cheat strips them from handles
            // to its game): the same numbers from the system's process list.
            None => {
                let err = unsafe { windows_sys::Win32::Foundation::GetLastError() };
                match system_process_times(pid) {
                    Some((c, cpu, _)) => {
                        note_unopenable(pid, err);
                        *created = c;
                        *cpu100ns = cpu;
                        true
                    }
                    None => false,
                }
            }
        };
        close_handle(x);
        let _ = h;
        r
    }

    fn session_of(&self, pid: u32) -> u32 {
        let mut s: u32 = 0;
        if unsafe { ProcessIdToSessionId(pid, &mut s) } != 0 {
            return s;
        }
        system_process_times(pid).map(|x| x.2).unwrap_or(u32::MAX)
    }
}
