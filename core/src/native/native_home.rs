//! Where the program's own native libraries are unpacked (1.15.0 Test 1.3, port of `NativeHome.cs`).
//!
//! The C# Toolbox is one .NET single-file exe whose native libraries travel inside it; the .NET host unpacks them on
//! start, by default into %TEMP%\.net\DomainScheduler\<build id>, reuses a folder that already exists there and only
//! checks that each file is present, not what is in it. %TEMP% belongs to the user: a program running without
//! administrator rights could put its own DLL there under the expected name, and the next start with administrator
//! rights would load it. So the C# program starts itself again with DOTNET_BUNDLE_EXTRACT_BASE_DIR pointing to
//! "native" in the admin-only state folder (and without the variables that make .NET load a profiler or a startup
//! hook), waits until the new copy has reached its own start, and ends. For a start that has to report back (update
//! hand-over, --revert-devices, --uninstall, --factory-reset) it waits for the new copy to finish and returns its exit
//! code. If the new copy cannot start, this copy goes on by itself as before: the program is never left unable to
//! start.
//!
//! The native exe is not a .NET bundle: nothing is unpacked, so [`BUNDLED`] is false and `relaunch_if_needed` only
//! does the second-copy part (a native exe started by an older C# copy with the marker set still answers it).
//! `clean_old` still removes the unpacked folders a C# build left in the state folder, except the one in use.

use crate::base::app_log::{AppLog, Sev};
use crate::base::ci;
use crate::base::state_dir::{store_dir, StateDir};
use std::path::PathBuf;

/// Whether this exe is a single-file bundle whose host unpacks libraries (C#: `Assembly.Location` is empty).
pub const BUNDLED: bool = false;

/// `NativeHome` (static class).
pub struct NativeHome;

impl NativeHome {
    pub const EXTRACT_ENV: &'static str = "DOTNET_BUNDLE_EXTRACT_BASE_DIR";
    /// Set for the second copy: the PID of the first one (its "reached the start" event carries that number).
    pub const MARKER_ENV: &'static str = "DOMAINOS_TOOLBOX_PARENT";

    pub fn ready_name(parent_pid: &str) -> String {
        format!("Local\\DomainScheduler.Relaunched.{}", parent_pid)
    }

    /// The folder for the unpacked libraries (in the checked state folder; Err when that cannot be used).
    pub fn dir() -> Result<PathBuf, String> {
        Ok(store_dir()?.join("native"))
    }

    /// Whether to start again with the folder set (pure).
    pub fn should_relaunch(windows: bool, bundled: bool, current: Option<&str>, wanted: &str, marker: Option<&str>) -> bool {
        windows && bundled && !wanted.is_empty() && marker.map(|m| m.is_empty()).unwrap_or(true) && !Self::same_path(current, wanted)
    }

    fn same_path(a: Option<&str>, b: &str) -> bool {
        let Some(a) = a else { return false };
        let t = |s: &str| s.trim().trim_end_matches(['\\', '/']).to_string();
        !a.is_empty() && !b.is_empty() && ci::eq(&t(a), &t(b))
    }

    /// Starts whose caller waits for the result: the first copy waits for the second one and returns its exit code.
    pub fn waits_for_child<S: AsRef<str>>(args: &[S]) -> bool {
        args.iter().any(|a| matches!(a.as_ref(), "--after-update" | "--revert-devices" | "--uninstall" | "--factory-reset"))
    }

    /// Called first in main. None: go on in this process. Some(code): this process is done, exit with it.
    pub fn relaunch_if_needed<S: AsRef<str>>(args: &[S]) -> Option<i32> {
        let marker = std::env::var(Self::MARKER_ENV).ok().filter(|m| !m.is_empty());
        if let Some(m) = marker {
            // The second copy: tell the first one it got here, and keep the marker away from our own children.
            #[cfg(windows)]
            imp::signal(&Self::ready_name(&m));
            #[cfg(not(windows))]
            let _ = m;
            std::env::remove_var(Self::MARKER_ENV);
            return None;
        }
        if !cfg!(windows) {
            return None;
        }
        let bundled = BUNDLED;
        if !bundled {
            return None;
        }
        let wanted = match Self::dir().and_then(|d| std::fs::create_dir_all(&d).map(|_| d).map_err(|e| e.to_string())) {
            Ok(d) => d,
            Err(_) => return None, // no safe folder: the default one, as before
        };
        for n in StateDir::take_notes() {
            AppLog::add_sev(n, Sev::Warn);
        }
        let current = std::env::var(Self::EXTRACT_ENV).ok();
        if !Self::should_relaunch(true, bundled, current.as_deref(), &wanted.to_string_lossy(), None) {
            return None;
        }
        #[cfg(windows)]
        {
            imp::relaunch(args, &wanted)
        }
        #[cfg(not(windows))]
        {
            let _ = args;
            None
        }
    }

    /// Removes unpacked folders of other builds (after an update the old one is no longer needed). The folder in use
    /// is the one a loaded module of this process comes from; when none is found nothing is removed.
    pub fn clean_old() {
        #[cfg(windows)]
        imp::clean_old();
    }
}

#[cfg(windows)]
mod imp {
    use super::NativeHome;
    use crate::base::app_log::{AppLog, Sev};
    use crate::base::ci;
    use crate::lf;
    use crate::native::sys_exe::{ProcessStartInfo, SysExe};
    use crate::native::win::{self, wide};
    use crate::base::ci::CiMapExt;
    use std::path::Path;
    use windows_sys::Win32::Foundation::{CloseHandle, HMODULE, WAIT_OBJECT_0};
    use windows_sys::Win32::System::ProcessStatus::{K32EnumProcessModulesEx, K32GetModuleFileNameExW};
    use windows_sys::Win32::System::Threading::{CreateEventW, GetCurrentProcess, OpenEventW, SetEvent, WaitForSingleObject};
    use windows_sys::Win32::UI::WindowsAndMessaging::AllowSetForegroundWindow;

    const EVENT_MODIFY_STATE: u32 = 0x0002;

    pub fn signal(name: &str) {
        let w = wide(name);
        let ev = unsafe { OpenEventW(EVENT_MODIFY_STATE, 0, w.as_ptr()) };
        if !ev.is_null() {
            unsafe {
                SetEvent(ev);
                CloseHandle(ev);
            }
        }
    }

    pub fn relaunch<S: AsRef<str>>(args: &[S], wanted: &Path) -> Option<i32> {
        let self_exe = std::env::current_exe().ok()?;
        let mut psi = ProcessStartInfo::new(&self_exe.to_string_lossy());
        psi.working_directory = std::env::current_dir().ok();
        for a in args {
            psi.arg(a.as_ref());
        }
        let bad: Vec<String> = psi.environment.keys().filter(|k| SysExe::unsafe_var(k.as_str())).map(|k| k.0.clone()).collect();
        for k in bad {
            psi.environment.ci_remove(&k);
        }
        let pid = std::process::id().to_string();
        psi.environment.ci_insert(NativeHome::EXTRACT_ENV, wanted.to_string_lossy().to_string());
        psi.environment.ci_insert(NativeHome::MARKER_ENV, pid.clone());
        let name = wide(&NativeHome::ready_name(&pid));
        let ready = unsafe { CreateEventW(std::ptr::null(), 1, 0, name.as_ptr()) };
        if ready.is_null() {
            return None;
        }
        let r = (|| {
            let mut child = psi.command().spawn().ok()?;
            unsafe { AllowSetForegroundWindow(child.id()) };
            let wait = |ms: u32| unsafe { WaitForSingleObject(ready, ms) == WAIT_OBJECT_0 };
            let mut reached = false;
            let mut exited = false;
            for _ in 0..300 {
                if reached {
                    break;
                }
                reached = wait(200);
                if !reached {
                    if let Ok(Some(_)) = child.try_wait() {
                        exited = true;
                        reached = wait(0);
                        break;
                    }
                }
            }
            if !reached && (exited || matches!(child.try_wait(), Ok(Some(_)))) {
                let code = child.wait().ok().and_then(|s| s.code()).unwrap_or(-1);
                if code == 0 {
                    return Some(0);
                }
                // The host could not start it there: go on here, with the default folder.
                AppLog::add_sev(
                    lf!("The program's libraries could not be unpacked into the state folder (exit code {0}); the default temporary folder is used this time", code),
                    Sev::Warn,
                );
                return None;
            }
            if !NativeHome::waits_for_child(args) {
                return Some(0);
            }
            child.wait().ok().and_then(|s| s.code()).or(Some(-1))
        })();
        unsafe { CloseHandle(ready) };
        r
    }

    pub fn clean_old() {
        let Ok(dir) = NativeHome::dir() else { return };
        let exe = std::env::current_exe().ok().and_then(|p| p.file_stem().map(|s| s.to_string_lossy().to_string())).unwrap_or_default();
        let root = dir.join(&exe);
        if exe.is_empty() || !root.is_dir() {
            return;
        }
        let root_s = root.to_string_lossy().to_string();
        let mut in_use: Option<String> = None;
        let mut mods: Vec<HMODULE> = vec![std::ptr::null_mut(); 1024];
        let mut needed: u32 = 0;
        let cb = (mods.len() * std::mem::size_of::<HMODULE>()) as u32;
        let proc_ = unsafe { GetCurrentProcess() };
        if unsafe { K32EnumProcessModulesEx(proc_, mods.as_mut_ptr(), cb, &mut needed, win::LIST_MODULES_ALL) } != 0 {
            let n = mods.len().min(needed as usize / std::mem::size_of::<HMODULE>());
            for m in mods.iter().take(n) {
                let mut buf = vec![0u16; 1024];
                let len = unsafe { K32GetModuleFileNameExW(proc_, *m, buf.as_mut_ptr(), buf.len() as u32) };
                let f = String::from_utf16_lossy(&buf[..len as usize]);
                let prefix = format!("{}\\", root_s);
                if !ci::starts_with(&f, &prefix) {
                    continue;
                }
                let rest: String = f.chars().skip(prefix.chars().count()).collect();
                in_use = Some(rest.split('\\').next().unwrap_or("").to_string());
                break;
            }
        }
        let Some(in_use) = in_use else { return };
        if let Ok(rd) = std::fs::read_dir(&root) {
            for d in rd.flatten() {
                if d.path().is_dir() && !ci::eq(&d.file_name().to_string_lossy(), &in_use) {
                    let _ = std::fs::remove_dir_all(d.path());
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// SecurityTests.RelaunchOnlyWhenTheHostUnpacksElsewhere
    #[test]
    fn relaunch_only_when_the_host_unpacks_elsewhere() {
        let want = r"C:\ProgramData\Domain OS Toolbox\native";
        assert!(NativeHome::should_relaunch(true, true, None, want, None));
        assert!(NativeHome::should_relaunch(true, true, Some(r"C:\Users\x\AppData\Local\Temp\evil"), want, None));
        assert!(!NativeHome::should_relaunch(true, true, Some(&format!("{}\\", want)), want, None));
        assert!(!NativeHome::should_relaunch(true, true, None, want, Some("1234"))); // already the second copy: never a loop
        assert!(!NativeHome::should_relaunch(true, false, None, want, None)); // not a single-file build
        assert!(!NativeHome::should_relaunch(false, true, None, want, None));
        assert!(NativeHome::waits_for_child(&["--revert-devices"]));
        assert!(NativeHome::waits_for_child(&["--after-update"]));
        assert!(!NativeHome::waits_for_child(&["--autostart"]));
    }
}
