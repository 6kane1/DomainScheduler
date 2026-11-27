//! Autostart (section 23), port of `Autostart.cs`: a Task Scheduler logon task with the highest privileges, created on
//! the first run without asking, read back after creation. The Run key is never used. Task Scheduler through COM
//! (Schedule.Service), schtasks.exe as the fallback, memory in tests.

use super::util::{guid_n, run_capture};
use crate::base::app_log::{AppLog, Sev};
use crate::base::ci;
use crate::base::hooks::Hook;
use crate::base::l::L;
use crate::base::settings::Settings;
use crate::base::state_dir::store_dir;
use crate::lf;
use crate::native::sys_exe::SysExe;
use std::sync::{Arc, RwLock};

/// Where the logon task lives (Task Scheduler COM, schtasks.exe, or memory in tests). Err = the C# exception message.
pub trait ITaskBackend: Send + Sync {
    /// The task's XML, None when there is no task.
    fn read(&self, name: &str) -> Result<Option<String>, String>;
    fn register(&self, name: &str, xml: &str) -> Result<(), String>;
    fn delete(&self, name: &str) -> Result<(), String>;
}

/// Task Scheduler 2.0 through COM (Schedule.Service).
#[derive(Default)]
pub struct ComTaskBackend;

#[cfg(not(windows))]
impl ITaskBackend for ComTaskBackend {
    fn read(&self, _name: &str) -> Result<Option<String>, String> {
        Err("Task Scheduler is not available".into())
    }
    fn register(&self, _name: &str, _xml: &str) -> Result<(), String> {
        Err("Task Scheduler is not available".into())
    }
    fn delete(&self, _name: &str) -> Result<(), String> {
        Err("Task Scheduler is not available".into())
    }
}

#[cfg(windows)]
mod com {
    //! The few Task Scheduler interfaces the program calls, by their vtable slots (taskschd.h order): IUnknown (3),
    //! IDispatch (4), then ITaskService { GetFolder 7, GetRunningTasks, NewTask, Connect 10 }, ITaskFolder { get_Name,
    //! get_Path, GetFolder, GetFolders, CreateFolder, DeleteFolder, GetTask 13, GetTasks, DeleteTask 15,
    //! RegisterTask 16 }, IRegisteredTask { get_Name, get_Path, get_State, get_Enabled, put_Enabled, Run, RunEx,
    //! GetInstances, get_LastRunTime, get_LastTaskResult, get_NumberOfMissedRuns, get_NextRunTime, get_Definition,
    //! get_Xml 20 }.
    use crate::native::win::wide;
    use std::ffi::c_void;
    use windows_sys::core::{GUID, HRESULT};
    use windows_sys::Win32::Foundation::{SysAllocString, SysFreeString, SysStringLen};
    use windows_sys::Win32::System::Com::{CLSIDFromProgID, CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_ALL, COINIT_APARTMENTTHREADED};
    use windows_sys::Win32::System::Variant::VARIANT;

    const IID_ITASK_SERVICE: GUID = GUID::from_u128(0x2faba4c7_4da9_4013_9697_20cc3fd40f85);
    const TASK_CREATE_OR_UPDATE: i32 = 6;
    const TASK_LOGON_INTERACTIVE_TOKEN: i32 = 3;
    const HRESULT_FILE_NOT_FOUND: HRESULT = 0x80070002u32 as i32;
    const HRESULT_PATH_NOT_FOUND: HRESULT = 0x80070003u32 as i32;

    pub fn hr_text(hr: HRESULT) -> String {
        let m = std::io::Error::from_raw_os_error(hr).to_string();
        let m = m.split(" (os error").next().unwrap_or("").trim().to_string();
        format!("{} (0x{:08X})", m, hr as u32)
    }

    /// A COM pointer, released on drop.
    struct Ptr(*mut c_void);
    impl Drop for Ptr {
        fn drop(&mut self) {
            if !self.0.is_null() {
                let release: extern "system" fn(*mut c_void) -> u32 = unsafe { slot(self.0, 2) };
                release(self.0);
            }
        }
    }

    /// The function in vtable slot i of a COM object.
    unsafe fn slot<F: Copy>(obj: *mut c_void, i: usize) -> F {
        let vtbl = *(obj as *const *const usize);
        let f = *vtbl.add(i);
        std::mem::transmute_copy::<usize, F>(&f)
    }

    struct Bstr(*const u16);
    impl Bstr {
        fn new(s: &str) -> Bstr {
            let w = wide(s);
            Bstr(unsafe { SysAllocString(w.as_ptr()) })
        }
    }
    impl Drop for Bstr {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe { SysFreeString(self.0) };
            }
        }
    }

    fn empty() -> VARIANT {
        unsafe { std::mem::zeroed() } // VT_EMPTY
    }

    /// Connects to Task Scheduler and runs f with the root folder (COM is set up on this thread for the call).
    fn with_folder<T>(f: impl FnOnce(*mut c_void) -> Result<T, String>) -> Result<T, String> {
        let init = unsafe { CoInitializeEx(std::ptr::null(), COINIT_APARTMENTTHREADED as u32) };
        let r = (|| -> Result<T, String> {
            let prog = wide("Schedule.Service");
            let mut clsid: GUID = unsafe { std::mem::zeroed() };
            if unsafe { CLSIDFromProgID(prog.as_ptr(), &mut clsid) } < 0 {
                return Err("Task Scheduler is not available".into());
            }
            let mut svc: *mut c_void = std::ptr::null_mut();
            let hr = unsafe { CoCreateInstance(&clsid, std::ptr::null_mut(), CLSCTX_ALL, &IID_ITASK_SERVICE, &mut svc) };
            if hr < 0 {
                return Err(hr_text(hr));
            }
            let svc = Ptr(svc);
            let connect: extern "system" fn(*mut c_void, VARIANT, VARIANT, VARIANT, VARIANT) -> HRESULT = unsafe { slot(svc.0, 10) };
            let hr = connect(svc.0, empty(), empty(), empty(), empty());
            if hr < 0 {
                return Err(hr_text(hr));
            }
            let get_folder: extern "system" fn(*mut c_void, *const u16, *mut *mut c_void) -> HRESULT = unsafe { slot(svc.0, 7) };
            let root = Bstr::new("\\");
            let mut folder: *mut c_void = std::ptr::null_mut();
            let hr = get_folder(svc.0, root.0, &mut folder);
            if hr < 0 {
                return Err(hr_text(hr));
            }
            let folder = Ptr(folder);
            f(folder.0)
        })();
        if init >= 0 {
            unsafe { CoUninitialize() };
        }
        r
    }

    pub fn read(name: &str) -> Result<Option<String>, String> {
        with_folder(|folder| {
            let get_task: extern "system" fn(*mut c_void, *const u16, *mut *mut c_void) -> HRESULT = unsafe { slot(folder, 13) };
            let n = Bstr::new(name);
            let mut task: *mut c_void = std::ptr::null_mut();
            // COMException / FileNotFoundException: no task.
            if get_task(folder, n.0, &mut task) < 0 || task.is_null() {
                return Ok(None);
            }
            let task = Ptr(task);
            let get_xml: extern "system" fn(*mut c_void, *mut *const u16) -> HRESULT = unsafe { slot(task.0, 20) };
            let mut xml: *const u16 = std::ptr::null();
            if get_xml(task.0, &mut xml) < 0 || xml.is_null() {
                return Ok(None);
            }
            let x = Bstr(xml);
            let len = unsafe { SysStringLen(x.0) } as usize;
            Ok(Some(String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(x.0, len) })))
        })
    }

    pub fn register(name: &str, xml: &str) -> Result<(), String> {
        with_folder(|folder| {
            type Reg = extern "system" fn(*mut c_void, *const u16, *const u16, i32, VARIANT, VARIANT, i32, VARIANT, *mut *mut c_void) -> HRESULT;
            let register: Reg = unsafe { slot(folder, 16) };
            let (n, x) = (Bstr::new(name), Bstr::new(xml));
            let mut task: *mut c_void = std::ptr::null_mut();
            let hr = register(folder, n.0, x.0, TASK_CREATE_OR_UPDATE, empty(), empty(), TASK_LOGON_INTERACTIVE_TOKEN, empty(), &mut task);
            let _task = Ptr(task);
            if hr < 0 {
                return Err(hr_text(hr));
            }
            Ok(())
        })
    }

    pub fn delete(name: &str) -> Result<(), String> {
        with_folder(|folder| {
            let delete_task: extern "system" fn(*mut c_void, *const u16, i32) -> HRESULT = unsafe { slot(folder, 15) };
            let n = Bstr::new(name);
            let hr = delete_task(folder, n.0, 0);
            // COMException / FileNotFoundException are swallowed as in C#.
            let _ = (hr, HRESULT_FILE_NOT_FOUND, HRESULT_PATH_NOT_FOUND);
            Ok(())
        })
    }
}

#[cfg(windows)]
impl ITaskBackend for ComTaskBackend {
    fn read(&self, name: &str) -> Result<Option<String>, String> {
        com::read(name)
    }
    fn register(&self, name: &str, xml: &str) -> Result<(), String> {
        com::register(name, xml)
    }
    fn delete(&self, name: &str) -> Result<(), String> {
        com::delete(name)
    }
}

/// schtasks.exe: the fallback when COM fails.
#[derive(Default)]
pub struct SchtasksBackend;

impl SchtasksBackend {
    fn run(args: &[&str]) -> Result<(i32, String), String> {
        let mut psi = SysExe::start("schtasks.exe", true);
        for a in args {
            psi.arg(a);
        }
        let r = run_capture(&psi, 15000).map_err(|e| e.to_string())?;
        Ok((r.code.unwrap_or(-1), r.stdout))
    }
}

impl ITaskBackend for SchtasksBackend {
    fn read(&self, name: &str) -> Result<Option<String>, String> {
        let (code, output) = Self::run(&["/Query", "/TN", name, "/XML"])?;
        Ok(if code == 0 && output.contains("<Task") { Some(output) } else { None })
    }

    fn register(&self, name: &str, xml: &str) -> Result<(), String> {
        // The task definition is handed over in the admin-only state folder, not in %TEMP% (which any program of the
        // user can write to: a definition swapped there before schtasks reads it would be registered with full rights).
        let tmp = store_dir()?.join(format!("task-{}.xml", guid_n()));
        let r = (|| -> Result<(), String> {
            use std::io::Write;
            let mut o = std::fs::OpenOptions::new();
            o.write(true).create_new(true);
            #[cfg(windows)]
            {
                use std::os::windows::fs::OpenOptionsExt;
                o.share_mode(0);
            }
            {
                let mut fs = o.open(&tmp).map_err(|e| e.to_string())?;
                // Encoding.Unicode with its byte order mark.
                let mut bytes = vec![0xFFu8, 0xFE];
                for u in xml.encode_utf16() {
                    bytes.extend_from_slice(&u.to_le_bytes());
                }
                fs.write_all(&bytes).map_err(|e| e.to_string())?;
            }
            let tmp_s = tmp.to_string_lossy().to_string();
            let (code, output) = Self::run(&["/Create", "/TN", name, "/XML", &tmp_s, "/F"])?;
            if code != 0 {
                return Err(format!("schtasks: {}", output.trim()));
            }
            Ok(())
        })();
        let _ = std::fs::remove_file(&tmp);
        r
    }

    fn delete(&self, name: &str) -> Result<(), String> {
        Self::run(&["/Delete", "/TN", name, "/F"]).map(|_| ())
    }
}

/// Tasks in memory (tests; AddendumTests.MemTasks).
#[cfg(any(test, feature = "testsupport"))]
#[derive(Default)]
pub struct MemTasks {
    pub tasks: std::sync::Mutex<std::collections::HashMap<String, String>>,
    pub fail: std::sync::atomic::AtomicBool,
}

#[cfg(any(test, feature = "testsupport"))]
impl ITaskBackend for MemTasks {
    fn read(&self, name: &str) -> Result<Option<String>, String> {
        Ok(crate::base::hooks::lock(&self.tasks).get(name).cloned())
    }
    fn register(&self, name: &str, xml: &str) -> Result<(), String> {
        if self.fail.load(std::sync::atomic::Ordering::SeqCst) {
            return Err("COM down".into());
        }
        crate::base::hooks::lock(&self.tasks).insert(name.to_string(), xml.to_string());
        Ok(())
    }
    fn delete(&self, name: &str) -> Result<(), String> {
        crate::base::hooks::lock(&self.tasks).remove(name);
        Ok(())
    }
}

static PRIMARY: RwLock<Option<Arc<dyn ITaskBackend>>> = RwLock::new(None);
static FALLBACK: RwLock<Option<Arc<dyn ITaskBackend>>> = RwLock::new(None);
/// Tests: the account the task runs as ("DOMAIN\user").
pub static USER_HOOK: Hook<dyn Fn() -> String + Send + Sync> = Hook::new();

/// `SecurityElement.Escape`.
pub fn xml_escape(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '<' => o.push_str("&lt;"),
            '>' => o.push_str("&gt;"),
            '"' => o.push_str("&quot;"),
            '\'' => o.push_str("&apos;"),
            '&' => o.push_str("&amp;"),
            c => o.push(c),
        }
    }
    o
}

/// `Environment.UserDomainName + "\\" + Environment.UserName`.
fn current_user() -> String {
    #[cfg(windows)]
    {
        use crate::native::win::{from_wide, wide};
        use windows_sys::Win32::Security::LookupAccountNameW;
        use windows_sys::Win32::System::WindowsProgramming::GetUserNameW;
        let mut buf = vec![0u16; 257];
        let mut n = buf.len() as u32;
        let user = if unsafe { GetUserNameW(buf.as_mut_ptr(), &mut n) } != 0 { from_wide(&buf) } else { std::env::var("USERNAME").unwrap_or_default() };
        let mut sid = vec![0u8; 256];
        let mut sid_len = sid.len() as u32;
        let mut dom = vec![0u16; 256];
        let mut dom_len = dom.len() as u32;
        let mut use_: i32 = 0;
        let w = wide(&user);
        let domain = if unsafe { LookupAccountNameW(std::ptr::null(), w.as_ptr(), sid.as_mut_ptr() as *mut _, &mut sid_len, dom.as_mut_ptr(), &mut dom_len, &mut use_) } != 0 {
            from_wide(&dom)
        } else {
            std::env::var("USERDOMAIN").unwrap_or_default()
        };
        format!("{}\\{}", domain, user)
    }
    #[cfg(not(windows))]
    {
        format!("{}\\{}", std::env::var("HOSTNAME").unwrap_or_default(), std::env::var("USER").unwrap_or_default())
    }
}

/// `Autostart` (static class).
pub struct Autostart;

impl Autostart {
    pub const TASK_NAME: &'static str = "DomainScheduler";
    pub const ARG: &'static str = "--autostart";

    pub fn primary() -> Arc<dyn ITaskBackend> {
        PRIMARY.read().unwrap_or_else(|e| e.into_inner()).clone().unwrap_or_else(|| Arc::new(ComTaskBackend))
    }

    pub fn fallback() -> Arc<dyn ITaskBackend> {
        FALLBACK.read().unwrap_or_else(|e| e.into_inner()).clone().unwrap_or_else(|| Arc::new(SchtasksBackend))
    }

    /// `Autostart.Primary = b` (None: COM again).
    pub fn set_primary(b: Option<Arc<dyn ITaskBackend>>) {
        *PRIMARY.write().unwrap_or_else(|e| e.into_inner()) = b;
    }

    /// `Autostart.Fallback = b` (None: schtasks.exe again).
    pub fn set_fallback(b: Option<Arc<dyn ITaskBackend>>) {
        *FALLBACK.write().unwrap_or_else(|e| e.into_inner()) = b;
    }

    fn user() -> String {
        match USER_HOOK.get() {
            Some(h) => h(),
            None => current_user(),
        }
    }

    pub fn xml(exe_path: &str) -> String {
        let user = xml_escape(&Self::user());
        String::new()
            + "<?xml version=\"1.0\" encoding=\"UTF-16\"?>\n"
            + "<Task version=\"1.2\" xmlns=\"http://schemas.microsoft.com/windows/2004/02/mit/task\">\n"
            + "  <RegistrationInfo><Description>DomainScheduler at logon</Description></RegistrationInfo>\n"
            + "  <Triggers><LogonTrigger><Enabled>true</Enabled><UserId>"
            + &user
            + "</UserId></LogonTrigger></Triggers>\n"
            + "  <Principals><Principal id=\"Author\"><UserId>"
            + &user
            + "</UserId><LogonType>InteractiveToken</LogonType><RunLevel>HighestAvailable</RunLevel></Principal></Principals>\n"
            + "  <Settings><MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy><DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>"
            + "<StopIfGoingOnBatteries>false</StopIfGoingOnBatteries><ExecutionTimeLimit>PT0S</ExecutionTimeLimit><Enabled>true</Enabled><Priority>5</Priority></Settings>\n"
            + "  <Actions Context=\"Author\"><Exec><Command>"
            + &xml_escape(exe_path)
            + "</Command><Arguments>"
            + Self::ARG
            + "</Arguments></Exec></Actions>\n"
            + "</Task>"
    }

    fn read_any() -> Option<String> {
        if let Ok(Some(x)) = Self::primary().read(Self::TASK_NAME) {
            return Some(x);
        }
        Self::fallback().read(Self::TASK_NAME).ok().flatten()
    }

    pub fn task_exists() -> bool {
        Self::read_any().is_some()
    }

    /// The task exists and is enabled.
    pub fn task_enabled() -> bool {
        let Some(x) = Self::read_any() else { return false };
        match x.rfind("<Settings>") {
            None => true,
            Some(i) => !ci::contains(&x[i..], "<Enabled>false</Enabled>"),
        }
    }

    fn matches(xml: Option<&str>, exe_path: &str) -> bool {
        matches!(xml, Some(x) if ci::contains(x, &xml_escape(exe_path)) && x.contains("HighestAvailable"))
    }

    /// Creates (or updates) the task and checks it by reading it back.
    pub fn task_create(exe_path: &str) -> Result<(), String> {
        let xml = Self::xml(exe_path);
        let p = Self::primary();
        let first = match p.register(Self::TASK_NAME, &xml).and_then(|_| p.read(Self::TASK_NAME)) {
            Ok(back) if Self::matches(back.as_deref(), exe_path) => return Ok(()),
            Ok(_) => "the task read back differs".to_string(),
            Err(e) => e,
        };
        AppLog::add_sev(lf!("Autostart through COM failed ({0}): schtasks.exe is used", first), Sev::Warn);
        let f = Self::fallback();
        f.register(Self::TASK_NAME, &xml)?;
        if !Self::matches(f.read(Self::TASK_NAME)?.as_deref(), exe_path) {
            return Err(L::t("The autostart task read back differs").to_string());
        }
        Ok(())
    }

    pub fn task_delete() {
        let _ = Self::primary().delete(Self::TASK_NAME);
        let f = Self::fallback();
        if let Ok(Some(_)) = f.read(Self::TASK_NAME) {
            let _ = f.delete(Self::TASK_NAME);
        }
    }

    /// First run: the task is created without asking (once; a task the user removed is not recreated).
    pub fn ensure_on_first_run(exe_path: &str) {
        if Settings::try_stored("Autostart") {
            return;
        }
        let r = (|| -> Result<(), String> {
            if !Self::task_exists() {
                Self::task_create(exe_path)?;
            }
            Settings::set("Autostart", "true")?;
            AppLog::add(lf!("Autostart task {0} created", Self::TASK_NAME));
            Ok(())
        })();
        if let Err(e) = r {
            AppLog::add_sev(lf!("Autostart task not created: {0}", e), Sev::Error);
        }
    }

    pub fn report_line() -> String {
        lf!("Autostart: task {0} ({1})", Self::TASK_NAME, if Self::task_enabled() { L::t("on") } else { L::t("off") })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::base::hooks::static_hooks;
    use crate::base::settings::MemorySettingsBackend;
    use std::sync::atomic::Ordering;

    /// AddendumTests.AutostartTaskIsCreatedOnceAndReadBack
    #[test]
    fn autostart_task_is_created_once_and_read_back() {
        let _g = static_hooks();
        let _b = Settings::scoped_backend(Arc::new(MemorySettingsBackend::new()));
        let com = Arc::new(MemTasks::default());
        let sch = Arc::new(MemTasks::default());
        Autostart::set_primary(Some(com.clone()));
        Autostart::set_fallback(Some(sch.clone()));
        USER_HOOK.set(Some(Arc::new(|| "PC\\player".to_string())));

        Autostart::ensure_on_first_run(r"C:\Tools\DomainScheduler.exe");
        assert!(Autostart::task_exists());
        let x = com.tasks.lock().unwrap()[Autostart::TASK_NAME].clone();
        assert!(x.contains("HighestAvailable"));
        assert!(x.contains("--autostart"));
        assert!(x.contains("<UserId>PC\\player</UserId>"));
        assert!(Autostart::task_enabled());
        // The user removed it: a second start does not recreate it; exit does not remove it.
        com.tasks.lock().unwrap().clear();
        Autostart::ensure_on_first_run(r"C:\Tools\DomainScheduler.exe");
        assert!(!Autostart::task_exists());
        // COM failing: schtasks takes over.
        com.fail.store(true, Ordering::SeqCst);
        Autostart::task_create(r"C:\Tools\DomainScheduler.exe").unwrap();
        assert!(sch.tasks.lock().unwrap().contains_key(Autostart::TASK_NAME));
        Autostart::task_delete();
        assert!(!Autostart::task_exists());
        assert!(Autostart::report_line().contains("(off)"));

        Autostart::set_primary(None);
        Autostart::set_fallback(None);
        USER_HOOK.set(None);
    }

    /// The COM backend for real (Windows; only with DS_TASK_TEST=1, e.g. under Wine: it registers and removes a task).
    #[cfg(windows)]
    #[test]
    fn com_backend_registers_reads_and_deletes() {
        if std::env::var("DS_TASK_TEST").ok().as_deref() != Some("1") {
            return;
        }
        let name = "DomainSchedulerPortTest";
        let com = ComTaskBackend;
        let _ = com.delete(name);
        assert_eq!(com.read(name).unwrap(), None);
        USER_HOOK.set(None);
        let xml = Autostart::xml(r"C:\Tools\DomainScheduler.exe");
        com.register(name, &xml).unwrap();
        let back = com.read(name).unwrap().unwrap();
        assert!(back.contains("DomainScheduler at logon"), "{}", back); // Wine drops the Exec part when it reads back
        com.delete(name).unwrap();
        assert_eq!(com.read(name).unwrap(), None);
    }

    #[test]
    fn escapes_like_security_element() {
        assert_eq!(xml_escape(r#"C:\A&B\<x>"'.exe"#), "C:\\A&amp;B\\&lt;x&gt;&quot;&apos;.exe");
    }
}
