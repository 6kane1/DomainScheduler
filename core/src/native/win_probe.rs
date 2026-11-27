//! IGameProbe over the Windows API (port of `WinProbe.cs`).

use super::win::{self, close_handle, h, open_process, PROCESS_QUERY_LIMITED_INFORMATION};
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
            return false;
        }
        let r = match win::get_process_times(x) {
            Some((c, _, k, u)) => {
                *created = c;
                *cpu100ns = k + u;
                true
            }
            None => false,
        };
        close_handle(x);
        let _ = h;
        r
    }

    fn session_of(&self, pid: u32) -> u32 {
        let mut s: u32 = 0;
        if unsafe { ProcessIdToSessionId(pid, &mut s) } != 0 { s } else { u32::MAX }
    }
}
