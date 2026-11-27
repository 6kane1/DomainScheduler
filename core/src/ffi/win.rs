//! The few Win32 calls of the start-up and the hand-over that .NET made for App.xaml.cs, AutoCommand.cs,
//! UpdateService.cs and Hardware.cs (named events and the instance mutex, the message box, the beep, opening a link,
//! the own CPU time, the known folders, the current user's SID). Other platforms get neutral stand-ins so the logic
//! builds and is tested on Linux.

/// A kernel handle as a number (0 = none).
pub type H = isize;

pub const WAIT_OBJECT_0: u32 = 0;
pub const WAIT_ABANDONED: u32 = 0x80;
pub const WAIT_TIMEOUT: u32 = 258;
pub const INFINITE: u32 = 0xFFFF_FFFF;

#[cfg(windows)]
mod imp {
    use super::H;
    use crate::native::win::{from_wide, wide};
    use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, LocalFree, ERROR_ALREADY_EXISTS, HANDLE, HWND, LPARAM};
    use windows_sys::Win32::System::Threading::{
        CreateEventW, CreateMutexW, GetCurrentProcess, OpenEventW, ReleaseMutex, ResetEvent, SetEvent, SetPriorityClass,
        WaitForMultipleObjects, WaitForSingleObject, BELOW_NORMAL_PRIORITY_CLASS,
    };

    fn hd(h: H) -> HANDLE {
        h as HANDLE
    }

    /// `new EventWaitHandle(initial, manual ? ManualReset : AutoReset, name)`: creates or opens the event.
    pub fn create_event(manual: bool, initial: bool, name: Option<&str>) -> H {
        let w = name.map(wide);
        // SAFETY: plain Win32 call; the name is NUL-terminated.
        unsafe { CreateEventW(std::ptr::null(), manual as i32, initial as i32, w.as_ref().map_or(std::ptr::null(), |w| w.as_ptr())) as H }
    }

    /// `EventWaitHandle.OpenExisting(name)`: None when it does not exist (or access is denied).
    pub fn open_event(name: &str) -> Option<H> {
        let w = wide(name);
        // EVENT_MODIFY_STATE | SYNCHRONIZE
        // SAFETY: plain Win32 call.
        let h = unsafe { OpenEventW(0x0002 | 0x0010_0000, 0, w.as_ptr()) } as H;
        (h != 0).then_some(h)
    }

    pub fn set_event(h: H) -> bool {
        // SAFETY: a handle this module opened.
        unsafe { SetEvent(hd(h)) != 0 }
    }

    pub fn reset_event(h: H) -> bool {
        // SAFETY: a handle this module opened.
        unsafe { ResetEvent(hd(h)) != 0 }
    }

    /// WaitForSingleObject: WAIT_OBJECT_0, WAIT_ABANDONED, WAIT_TIMEOUT or WAIT_FAILED.
    pub fn wait(h: H, ms: u32) -> u32 {
        // SAFETY: a handle this module opened.
        unsafe { WaitForSingleObject(hd(h), ms) }
    }

    /// `WaitHandle.WaitAny`: the index of the signalled handle, None on failure.
    pub fn wait_any(hs: &[H], ms: u32) -> Option<usize> {
        let v: Vec<HANDLE> = hs.iter().map(|&h| hd(h)).collect();
        // SAFETY: handles this module opened.
        let r = unsafe { WaitForMultipleObjects(v.len() as u32, v.as_ptr(), 0, ms) };
        let n = v.len() as u32;
        if r < n {
            Some(r as usize)
        } else if (0x80..0x80 + n).contains(&r) {
            Some((r - 0x80) as usize)
        } else {
            None
        }
    }

    pub fn close(h: H) {
        if h != 0 {
            // SAFETY: a handle this module opened, closed once.
            unsafe { CloseHandle(hd(h)) };
        }
    }

    /// `new Mutex(initiallyOwned: true, name, out createdNew)`: (handle, createdNew). The calling thread owns it when
    /// createdNew is true.
    pub fn create_mutex(name: &str) -> (H, bool) {
        let w = wide(name);
        // SAFETY: plain Win32 call.
        unsafe {
            let h = CreateMutexW(std::ptr::null(), 1, w.as_ptr()) as H;
            let existed = GetLastError() == ERROR_ALREADY_EXISTS;
            (h, h != 0 && !existed)
        }
    }

    pub fn release_mutex(h: H) -> bool {
        // SAFETY: a handle this module opened.
        unsafe { ReleaseMutex(hd(h)) != 0 }
    }

    /// `Process.GetCurrentProcess().PriorityClass = BelowNormal`.
    pub fn below_normal() {
        // SAFETY: the pseudo handle of this process.
        unsafe { SetPriorityClass(GetCurrentProcess(), BELOW_NORMAL_PRIORITY_CLASS) };
    }

    /// The UI thread's message queue gets WM_QUIT (the app's loop ends: Application.Shutdown).
    pub fn post_quit(thread_id: u32) {
        use windows_sys::Win32::UI::WindowsAndMessaging::{PostThreadMessageW, WM_QUIT};
        if thread_id != 0 {
            // SAFETY: plain Win32 call.
            unsafe { PostThreadMessageW(thread_id, WM_QUIT, 0, 0) };
        }
    }

    unsafe extern "system" fn hide_one(hwnd: HWND, _l: LPARAM) -> i32 {
        use windows_sys::Win32::UI::WindowsAndMessaging::{IsWindowVisible, ShowWindow, SW_HIDE};
        if IsWindowVisible(hwnd) != 0 {
            ShowWindow(hwnd, SW_HIDE);
        }
        1
    }

    /// `_window.Hide()`: every visible top-level window of the UI thread.
    pub fn hide_windows(thread_id: u32) {
        use windows_sys::Win32::UI::WindowsAndMessaging::EnumThreadWindows;
        if thread_id != 0 {
            // SAFETY: the callback only hides windows of that thread.
            unsafe { EnumThreadWindows(thread_id, Some(hide_one), 0) };
        }
    }

    /// `MessageBox.Show(text, caption, YesNo, icon, defaultResult: No) == Yes` (no owner window).
    pub fn ask_yes_no(text: &str, caption: &str, warning: bool) -> bool {
        use windows_sys::Win32::UI::WindowsAndMessaging::{MessageBoxW, IDYES, MB_DEFBUTTON2, MB_ICONQUESTION, MB_ICONWARNING, MB_YESNO};
        let t = wide(text);
        let c = wide(caption);
        let icon = if warning { MB_ICONWARNING } else { MB_ICONQUESTION };
        // SAFETY: NUL-terminated strings.
        unsafe { MessageBoxW(std::ptr::null_mut(), t.as_ptr(), c.as_ptr(), MB_YESNO | icon | MB_DEFBUTTON2) == IDYES }
    }

    /// `Console.Beep(hz, ms)`.
    pub fn beep(hz: u32, ms: u32) {
        // SAFETY: plain Win32 call.
        unsafe { windows_sys::Win32::System::Diagnostics::Debug::Beep(hz, ms) };
    }

    /// `Process.Start(new ProcessStartInfo(target) { UseShellExecute = true })`; Err = the Win32 error text.
    pub fn shell_open(target: &str, args: Option<&str>) -> Result<(), String> {
        use windows_sys::Win32::UI::Shell::ShellExecuteW;
        let op = wide("open");
        let f = wide(target);
        let a = args.map(wide);
        // SAFETY: NUL-terminated strings.
        let r = unsafe {
            ShellExecuteW(std::ptr::null_mut(), op.as_ptr(), f.as_ptr(), a.as_ref().map_or(std::ptr::null(), |a| a.as_ptr()), std::ptr::null(), 1 /* SW_SHOWNORMAL */)
        } as isize;
        if r > 32 {
            Ok(())
        } else {
            Err(std::io::Error::from_raw_os_error(r as i32).to_string())
        }
    }

    /// `Process.TotalProcessorTime` of this process in 100 ns units.
    pub fn self_cpu_100ns() -> i64 {
        crate::native::win::get_process_times(crate::native::win::get_current_process()).map(|(_, _, k, u)| k + u).unwrap_or(0)
    }

    fn known_folder(id: &windows_sys::core::GUID) -> String {
        use windows_sys::Win32::System::Com::CoTaskMemFree;
        use windows_sys::Win32::UI::Shell::SHGetKnownFolderPath;
        let mut p: windows_sys::core::PWSTR = std::ptr::null_mut();
        // SAFETY: the returned buffer is freed with CoTaskMemFree.
        unsafe {
            if SHGetKnownFolderPath(id, 0, std::ptr::null_mut(), &mut p) < 0 || p.is_null() {
                if !p.is_null() {
                    CoTaskMemFree(p as *const _);
                }
                return String::new();
            }
            let mut n = 0;
            while *p.add(n) != 0 {
                n += 1;
            }
            let s = from_wide(std::slice::from_raw_parts(p, n));
            CoTaskMemFree(p as *const _);
            s
        }
    }

    /// `Environment.GetFolderPath(SpecialFolder.DesktopDirectory)`.
    pub fn desktop_dir() -> String {
        known_folder(&windows_sys::Win32::UI::Shell::FOLDERID_Desktop)
    }

    /// `Environment.GetFolderPath(SpecialFolder.ProgramFiles)`.
    pub fn program_files_dir() -> String {
        known_folder(&windows_sys::Win32::UI::Shell::FOLDERID_ProgramFiles)
    }

    /// `Environment.GetFolderPath(SpecialFolder.Windows)`.
    pub fn windows_dir() -> String {
        use windows_sys::Win32::System::SystemInformation::GetWindowsDirectoryW;
        let mut b = [0u16; 512];
        // SAFETY: the buffer size is passed.
        let n = unsafe { GetWindowsDirectoryW(b.as_mut_ptr(), b.len() as u32) } as usize;
        if n == 0 || n > b.len() {
            return String::new();
        }
        from_wide(&b[..n])
    }

    /// `WindowsIdentity.GetCurrent().User?.Value ?? ""`.
    pub fn user_sid() -> String {
        use windows_sys::Win32::Security::Authorization::ConvertSidToStringSidW;
        use windows_sys::Win32::Security::{GetTokenInformation, TokenUser, TOKEN_QUERY, TOKEN_USER};
        use windows_sys::Win32::System::Threading::OpenProcessToken;
        // SAFETY: the token is closed, the SID string freed with LocalFree; the buffer holds the TOKEN_USER.
        unsafe {
            let mut tok: HANDLE = std::ptr::null_mut();
            if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut tok) == 0 {
                return String::new();
            }
            let mut len = 0u32;
            GetTokenInformation(tok, TokenUser, std::ptr::null_mut(), 0, &mut len);
            let mut buf = vec![0u64; (len as usize).div_ceil(8).max(1)];
            let mut out = String::new();
            if GetTokenInformation(tok, TokenUser, buf.as_mut_ptr() as *mut _, len, &mut len) != 0 {
                let tu = &*(buf.as_ptr() as *const TOKEN_USER);
                let mut s: windows_sys::core::PWSTR = std::ptr::null_mut();
                if ConvertSidToStringSidW(tu.User.Sid, &mut s) != 0 && !s.is_null() {
                    let mut n = 0;
                    while *s.add(n) != 0 {
                        n += 1;
                    }
                    out = from_wide(std::slice::from_raw_parts(s, n));
                    LocalFree(s as *mut _);
                }
            }
            CloseHandle(tok);
            out
        }
    }

    /// `Environment.UserName`.
    pub fn user_name() -> String {
        std::env::var("USERNAME").unwrap_or_default()
    }
}

#[cfg(not(windows))]
mod imp {
    //! Stand-ins: events and mutexes exist only inside this process (enough for the tests), nothing is shown.
    use super::H;
    use std::collections::BTreeMap as HashMap;
    use std::sync::{Condvar, Mutex};

    struct Ev {
        manual: bool,
        set: bool,
        name: Option<String>,
    }

    struct Table {
        next: H,
        evs: HashMap<H, Ev>,
    }

    static T: Mutex<Table> = Mutex::new(Table { next: 1, evs: HashMap::new() });
    static CV: Condvar = Condvar::new();

    fn lock() -> std::sync::MutexGuard<'static, Table> {
        T.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn create_event(manual: bool, initial: bool, name: Option<&str>) -> H {
        let mut t = lock();
        if let Some(n) = name {
            if let Some((h, _)) = t.evs.iter().find(|(_, e)| e.name.as_deref() == Some(n)) {
                return *h;
            }
        }
        let h = t.next;
        t.next += 1;
        t.evs.insert(h, Ev { manual, set: initial, name: name.map(str::to_string) });
        h
    }

    pub fn open_event(name: &str) -> Option<H> {
        lock().evs.iter().find(|(_, e)| e.name.as_deref() == Some(name)).map(|(h, _)| *h)
    }

    pub fn set_event(h: H) -> bool {
        let mut t = lock();
        let ok = t.evs.get_mut(&h).map(|e| e.set = true).is_some();
        CV.notify_all();
        ok
    }

    pub fn reset_event(h: H) -> bool {
        lock().evs.get_mut(&h).map(|e| e.set = false).is_some()
    }

    pub fn wait(h: H, ms: u32) -> u32 {
        wait_any(&[h], ms).map_or(super::WAIT_TIMEOUT, |_| super::WAIT_OBJECT_0)
    }

    pub fn wait_any(hs: &[H], ms: u32) -> Option<usize> {
        let end = std::time::Instant::now() + std::time::Duration::from_millis(if ms == super::INFINITE { 1 << 40 } else { ms as u64 });
        let mut t = lock();
        loop {
            for (i, h) in hs.iter().enumerate() {
                match t.evs.get_mut(h) {
                    Some(e) if e.set => {
                        if !e.manual {
                            e.set = false;
                        }
                        return Some(i);
                    }
                    None => return None,
                    _ => {}
                }
            }
            let now = std::time::Instant::now();
            if now >= end {
                return None;
            }
            t = CV.wait_timeout(t, end - now).unwrap_or_else(|e| e.into_inner()).0;
        }
    }

    /// Named events stay (another "handle" of this process may still use them, like a kernel object would).
    pub fn close(h: H) {
        let mut t = lock();
        if t.evs.get(&h).map(|e| e.name.is_none()).unwrap_or(false) {
            t.evs.remove(&h);
        }
    }

    pub fn create_mutex(_name: &str) -> (H, bool) {
        (create_event(true, false, None), true)
    }
    pub fn release_mutex(_h: H) -> bool {
        true
    }
    pub fn below_normal() {}
    pub fn post_quit(_thread_id: u32) {}
    pub fn hide_windows(_thread_id: u32) {}
    pub fn ask_yes_no(_text: &str, _caption: &str, _warning: bool) -> bool {
        false
    }
    pub fn beep(_hz: u32, _ms: u32) {}
    pub fn shell_open(_target: &str, _args: Option<&str>) -> Result<(), String> {
        Err("not supported on this platform".into())
    }
    pub fn self_cpu_100ns() -> i64 {
        0
    }
    pub fn desktop_dir() -> String {
        std::env::var("HOME").map(|h| format!("{}/Desktop", h)).unwrap_or_default()
    }
    pub fn program_files_dir() -> String {
        String::new()
    }
    pub fn windows_dir() -> String {
        String::new()
    }
    pub fn user_sid() -> String {
        String::new()
    }
    pub fn user_name() -> String {
        std::env::var("USER").unwrap_or_default()
    }
}

pub use imp::*;
