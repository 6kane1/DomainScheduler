//! Soft closing of background programs (section 22.3, port of `SoftClose.cs`). Only programs the user listed himself
//! are asked to close, with Process.CloseMainWindow; a program that is still there after 5 seconds is left alone with
//! a warning. TerminateProcess is never called. Closing cannot be undone, so it is not part of the crash-safe session.

use super::guard::Guard;
use crate::base::app_log::{AppLog, Sev};
use crate::base::ci::{self, CiSet, CiSetExt};
use crate::base::flags::Flags;
use crate::base::hooks::lock;
use crate::base::l::L;
use crate::base::time::{DateTime, TimeSpan};
use crate::lf;
use indexmap::{IndexMap, IndexSet};
use std::sync::{Arc, Mutex, OnceLock};

/// `AutoController.IsSystem` (the auto area's list, copied here so SoftClose does not depend on that area; see the
/// report). Keep both lists the same.
pub(crate) fn auto_is_system(exe: &str) -> bool {
    static V: OnceLock<CiSet> = OnceLock::new();
    V.get_or_init(|| {
        ci::set_of([
            "System", "Registry", "smss.exe", "csrss.exe", "wininit.exe", "winlogon.exe", "services.exe", "lsass.exe", "dwm.exe",
            "audiodg.exe", "fontdrvhost.exe", "MsMpEng.exe", "SearchIndexer.exe", "spoolsv.exe", "taskhostw.exe",
        ])
    })
    .ci_contains(exe)
}

/// Asks a process to close its main window: Ok(false) when it has none, Err = the C# exception text.
pub type CloseFn = dyn Fn(u32) -> Result<bool, String> + Send + Sync;
pub type NowFn = dyn Fn() -> DateTime + Send + Sync;

struct State {
    pending: Vec<(u32, String, DateTime)>,
    /// a program is asked once, not at every game start
    asked_pids: IndexSet<u32>,
    asked: i32,
}

/// `SoftClose` (an instance class). The settings fields (`On`, `List`) and the hooks are set through the setters.
pub struct SoftClose {
    gate: Mutex<State>,
    close_window: Mutex<Arc<CloseFn>>,
    now: Mutex<Arc<NowFn>>,
    on: Mutex<bool>,
    list: Mutex<CiSet>,
}

impl Default for SoftClose {
    fn default() -> Self {
        Self::new()
    }
}

impl SoftClose {
    pub const WAIT_SECONDS: i32 = 5;

    pub fn new() -> Self {
        SoftClose {
            gate: Mutex::new(State { pending: Vec::new(), asked_pids: IndexSet::new(), asked: 0 }),
            close_window: Mutex::new(Arc::new(close_main_window)),
            now: Mutex::new(Arc::new(DateTime::utc_now)),
            on: Mutex::new(false),
            list: Mutex::new(CiSet::new()),
        }
    }

    pub fn set_close_window(&self, f: Arc<CloseFn>) {
        *lock(&self.close_window) = f;
    }
    pub fn set_now(&self, f: Arc<NowFn>) {
        *lock(&self.now) = f;
    }
    /// The switch SoftCloseOn (set by the settings).
    pub fn set_on(&self, on: bool) {
        *lock(&self.on) = on;
    }
    pub fn on(&self) -> bool {
        *lock(&self.on)
    }
    /// The user's ClosableList (set by the settings).
    pub fn set_list(&self, list: CiSet) {
        *lock(&self.list) = list;
    }
    pub fn list(&self) -> CiSet {
        lock(&self.list).clone()
    }
    pub fn asked(&self) -> i32 {
        lock(&self.gate).asked
    }

    fn now_(&self) -> DateTime {
        let f = lock(&self.now).clone();
        f()
    }

    /// A game became active: every listed program is asked once. The denylist, system processes, the game (and
    /// everything on the watchlist) and this program are excluded whatever the list says.
    pub fn begin(&self, procs: &IndexMap<u32, (String, u32)>, games: Option<&[u32]>, watch: Option<&CiSet>, self_pid: u32) -> i32 {
        let mut st = lock(&self.gate);
        st.asked_pids.retain(|p| procs.contains_key(p));
        let list = self.list();
        if !self.on() || list.is_empty() {
            return 0;
        }
        if Flags::refuse(Flags::SOFT_CLOSE, L::t("Soft close")) {
            return 0;
        }
        let close = lock(&self.close_window).clone();
        let mut n = 0;
        for (pid, p) in procs {
            let (pid, name) = (*pid, p.0.as_str());
            if !list.ci_contains(name) {
                continue;
            }
            if pid == 0 || pid == 4 || pid == self_pid || games.map(|g| g.contains(&pid)).unwrap_or(false) {
                continue;
            }
            if Guard::is_denied(name) || auto_is_system(name) || watch.map(|w| w.ci_contains(name)).unwrap_or(false) {
                continue;
            }
            if !st.asked_pids.insert(pid) {
                continue;
            }
            let asked = match close(pid) {
                Ok(a) => a,
                Err(e) => {
                    AppLog::add_sev(lf!("Soft close: {0} (PID {1}) could not be asked: {2}", name, pid, e), Sev::Warn);
                    continue;
                }
            };
            if !asked {
                AppLog::add_sev(lf!("Soft close: {0} (PID {1}) has no window to close, left running", name, pid), Sev::Warn);
                continue;
            }
            AppLog::add(lf!("Soft close: {0} (PID {1}) was asked to close", name, pid));
            let deadline = self.now_() + TimeSpan::from_secs(Self::WAIT_SECONDS as f64);
            st.pending.push((pid, name.to_string(), deadline));
            n += 1;
        }
        st.asked += n;
        n
    }

    /// Every second: after 5 s a program that is still running is left alone (never forced).
    pub fn tick(&self, alive: &dyn Fn(u32) -> bool) {
        let mut st = lock(&self.gate);
        if st.pending.is_empty() {
            return;
        }
        let now = self.now_();
        for x in st.pending.clone() {
            let lives = alive(x.0);
            if lives && now < x.2 {
                continue;
            }
            if let Some(i) = st.pending.iter().position(|y| *y == x) {
                st.pending.remove(i);
            }
            if lives {
                AppLog::add_sev(lf!("Soft close: {0} (PID {1}) is still running after {2} s, left alone", &x.1, x.0, Self::WAIT_SECONDS), Sev::Warn);
            } else {
                AppLog::add(lf!("Soft close: {0} (PID {1}) closed", &x.1, x.0));
            }
        }
    }
}

/// `Process.GetProcessById(pid).CloseMainWindow()`: the first visible, unowned top-level window of the process gets
/// WM_CLOSE; false when there is none or it is disabled. Err when the process is not running.
#[cfg(windows)]
pub fn close_main_window(pid: u32) -> Result<bool, String> {
    use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, BOOL, HWND, LPARAM};
    use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetWindow, GetWindowLongW, GetWindowThreadProcessId, IsWindowVisible, PostMessageW, GWL_STYLE, GW_OWNER, WM_CLOSE, WS_DISABLED,
    };
    // SAFETY: plain Win32 calls with values owned here; the callback only writes into the struct passed as lParam.
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if h.is_null() {
            if GetLastError() == 87 {
                return Err(format!("Process with an Id of {} is not running.", pid));
            }
        } else {
            CloseHandle(h);
        }
        struct Find {
            pid: u32,
            hwnd: HWND,
        }
        unsafe extern "system" fn cb(w: HWND, l: LPARAM) -> BOOL {
            let f = &mut *(l as *mut Find);
            let mut p = 0u32;
            GetWindowThreadProcessId(w, &mut p);
            if p == f.pid && GetWindow(w, GW_OWNER).is_null() && IsWindowVisible(w) != 0 {
                f.hwnd = w;
                return 0;
            }
            1
        }
        let mut f = Find { pid, hwnd: std::ptr::null_mut() };
        EnumWindows(Some(cb), &mut f as *mut Find as LPARAM);
        if f.hwnd.is_null() {
            return Ok(false);
        }
        if (GetWindowLongW(f.hwnd, GWL_STYLE) as u32 & WS_DISABLED) != 0 {
            return Ok(false);
        }
        PostMessageW(f.hwnd, WM_CLOSE, 0, 0);
        Ok(true)
    }
}

#[cfg(not(windows))]
pub fn close_main_window(pid: u32) -> Result<bool, String> {
    Err(format!("Process with an Id of {} is not running.", pid))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::base::hooks::static_hooks;
    use crate::base::settings::{MemorySettingsBackend, Settings};
    use crate::native::fake::{FakeNative, P};
    use crate::native::INative;

    fn logged(sev: Sev, part: &str) -> bool {
        AppLog::tail(500).iter().any(|e| e.sev == sev && e.msg.contains(part))
    }

    /// RoadmapTests.SoftCloseOnlyAsksListedProgramsAndNeverForces
    #[test]
    fn soft_close_only_asks_listed_programs_and_never_forces() {
        let _g = static_hooks();
        let _b = Settings::scoped_backend(Arc::new(MemorySettingsBackend::new()));
        AppLog::clear();
        let f = FakeNative::cs2(500);
        {
            let mut s = f.lock();
            s.procs.insert(800, P::named("GCC.exe", 5));
            s.procs.insert(801, P::named("notepad.exe", 6));
            s.procs.insert(802, P::named("lsass.exe", 7));
            s.procs.insert(803, P::named("dwm.exe", 8));
            s.procs.insert(1, P::named("DomainScheduler.exe", 1));
        }
        let watch = ci::set_of(["cs2.exe"]);
        let now = Arc::new(Mutex::new(DateTime::from_parts(2026, 1, 1, 12, 0, 0, crate::base::time::Kind::Utc).unwrap()));
        let asked = Arc::new(Mutex::new(Vec::<u32>::new()));
        let soft = SoftClose::new();
        let n2 = now.clone();
        soft.set_now(Arc::new(move || *lock(&n2)));
        let a2 = asked.clone();
        soft.set_close_window(Arc::new(move |pid| {
            lock(&a2).push(pid);
            Ok(true)
        }));
        soft.set_on(true);
        soft.set_list(ci::set_of(["notepad.exe", "lsass.exe", "dwm.exe", "cs2.exe", "DomainScheduler.exe"]));
        let procs = f.processes();

        assert_eq!(0, soft.begin(&procs, Some(&[500]), Some(&watch), 1)); // the flag is closed: refused with a warning
        assert!(lock(&asked).is_empty());
        assert!(logged(Sev::Warn, "softclose"));

        Flags::set(Flags::SOFT_CLOSE, true).unwrap();
        assert_eq!(1, soft.begin(&procs, Some(&[500]), Some(&watch), 1));
        assert_eq!(vec![801u32], *lock(&asked));
        assert!(logged(Sev::Info, "notepad.exe (PID 801) was asked to close"));

        *lock(&now) += TimeSpan::from_secs(3.0);
        soft.tick(&|_| true);
        assert!(!logged(Sev::Warn, "still running"));
        *lock(&now) += TimeSpan::from_secs(3.0);
        soft.tick(&|_| true);
        assert!(logged(Sev::Warn, "still running"));
        assert_eq!(1, lock(&asked).len());
        assert!(f.lock().procs.contains_key(&801));

        // Nothing in the sources can kill a process.
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut stack = vec![root];
        while let Some(d) = stack.pop() {
            for e in std::fs::read_dir(&d).unwrap().flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                } else if p.extension().map(|x| x == "rs").unwrap_or(false) {
                    let code: String = std::fs::read_to_string(&p).unwrap().lines().filter(|l| !l.trim_start().starts_with("//")).collect::<Vec<_>>().join("\n");
                    let kill = ["Terminate", "Process("].concat();
                    let kill2 = [".ki", "ll("].concat();
                    assert!(!code.contains(&kill) && !code.contains(&kill2), "{:?}", p);
                }
            }
        }
    }
}
