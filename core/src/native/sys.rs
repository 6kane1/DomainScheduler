//! Process-wide facts the logic needs on every platform. On Windows they come from the Windows API; elsewhere (the
//! Linux test host) they are neutral: no CPU Sets, UTC, the invariant culture, the language from $LANG.

use crate::base::fmt::Culture;
use std::path::PathBuf;

#[cfg(windows)]
mod imp {
    use super::super::win::{self, from_wide, from_wide_ptr};
    use crate::base::fmt::Culture;
    use crate::base::time::FILETIME_EPOCH_TICKS;
    use std::path::PathBuf;
    use windows_sys::Win32::Foundation::{FILETIME, SYSTEMTIME};
    use windows_sys::Win32::Globalization::{
        GetLocaleInfoEx, GetUserPreferredUILanguages, LOCALE_S1159, LOCALE_S2359, LOCALE_SABBREVDAYNAME1, LOCALE_SABBREVMONTHNAME1, LOCALE_SDATE,
        LOCALE_SDAYNAME1, LOCALE_SDECIMAL, LOCALE_SMONTHNAME1, LOCALE_SNAME, LOCALE_SNEGATIVESIGN, LOCALE_SPERCENT, LOCALE_SSHORTDATE,
        LOCALE_SSHORTTIME, LOCALE_STHOUSAND, LOCALE_STIME, LOCALE_STIMEFORMAT, MUI_LANGUAGE_NAME,
    };
    use windows_sys::Win32::System::Com::CoTaskMemFree;
    use windows_sys::Win32::System::LibraryLoader::{SetDefaultDllDirectories, LOAD_LIBRARY_SEARCH_SYSTEM32};
    use windows_sys::Win32::System::SystemInformation::GetSystemDirectoryW;
    use windows_sys::Win32::System::Threading::{GetCurrentProcessId, GetCurrentThreadId};
    use windows_sys::Win32::System::Time::{FileTimeToSystemTime, GetTimeZoneInformationForYear, SystemTimeToFileTime, SystemTimeToTzSpecificLocalTime, TIME_ZONE_INFORMATION};
    use windows_sys::Win32::UI::Shell::{SHGetKnownFolderPath, FOLDERID_LocalAppData, FOLDERID_ProgramData};

    pub fn cpu_set_buffer() -> Vec<u8> {
        win::cpu_set_buffer()
    }
    pub fn cache_buffer() -> Vec<u8> {
        win::cache_buffer()
    }
    pub fn cpu_name() -> String {
        win::cpu_name()
    }
    pub fn active_lp_count() -> i32 {
        win::active_lp_count()
    }
    pub fn tick_count64() -> u64 {
        win::get_tick_count64()
    }
    pub fn current_thread_id() -> u32 {
        unsafe { GetCurrentThreadId() }
    }
    pub fn current_process_id() -> u32 {
        unsafe { GetCurrentProcessId() }
    }

    pub fn utc_offset_ticks(utc_ticks: i64) -> i64 {
        let ftv = utc_ticks - FILETIME_EPOCH_TICKS;
        if ftv < 0 {
            return 0;
        }
        let f = FILETIME { dwLowDateTime: ftv as u32, dwHighDateTime: (ftv >> 32) as u32 };
        // SAFETY: plain data structs filled by the API.
        unsafe {
            let mut st: SYSTEMTIME = std::mem::zeroed();
            if FileTimeToSystemTime(&f, &mut st) == 0 {
                return 0;
            }
            // The rules of the current time zone for that year (like .NET's adjustment rules).
            let mut tz: TIME_ZONE_INFORMATION = std::mem::zeroed();
            let tzp: *const TIME_ZONE_INFORMATION = if GetTimeZoneInformationForYear(st.wYear, std::ptr::null(), &mut tz) != 0 { &tz } else { std::ptr::null() };
            let mut lt: SYSTEMTIME = std::mem::zeroed();
            if SystemTimeToTzSpecificLocalTime(tzp, &st, &mut lt) == 0 {
                return 0;
            }
            let mut lf = FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 };
            if SystemTimeToFileTime(&lt, &mut lf) == 0 {
                return 0;
            }
            let l = ((lf.dwHighDateTime as i64) << 32) | lf.dwLowDateTime as i64;
            // SYSTEMTIME keeps milliseconds: compare with the instant cut to milliseconds.
            l - (ftv - ftv % 10_000)
        }
    }

    fn locale(lctype: u32) -> Option<String> {
        let mut buf = vec![0u16; 256];
        let n = unsafe { GetLocaleInfoEx(std::ptr::null(), lctype, buf.as_mut_ptr(), buf.len() as i32) };
        (n > 0).then(|| from_wide(&buf))
    }

    /// The user locale as a Culture (C# `CultureInfo.CurrentCulture` reads the same user settings).
    pub fn current_culture() -> Option<Culture> {
        let mut c = Culture::invariant();
        c.name = locale(LOCALE_SNAME)?;
        if let Some(v) = locale(LOCALE_SDECIMAL) {
            c.decimal = v;
        }
        if let Some(v) = locale(LOCALE_STHOUSAND) {
            c.group = v;
        }
        if let Some(v) = locale(LOCALE_SNEGATIVESIGN) {
            c.negative = v;
        }
        if let Some(v) = locale(LOCALE_SPERCENT) {
            c.percent = v;
        }
        if let Some(v) = locale(LOCALE_SSHORTTIME) {
            c.short_time = v;
        }
        if let Some(v) = locale(LOCALE_STIMEFORMAT) {
            c.long_time = v;
        }
        if let Some(v) = locale(LOCALE_SSHORTDATE) {
            c.short_date = v;
        }
        if let Some(v) = locale(LOCALE_S1159) {
            c.am = v;
        }
        if let Some(v) = locale(LOCALE_S2359) {
            c.pm = v;
        }
        if let Some(v) = locale(LOCALE_STIME) {
            c.time_sep = v;
        }
        if let Some(v) = locale(LOCALE_SDATE) {
            c.date_sep = v;
        }
        c.months = (0..12).filter_map(|i| locale(LOCALE_SMONTHNAME1 + i)).collect();
        c.months_abbr = (0..12).filter_map(|i| locale(LOCALE_SABBREVMONTHNAME1 + i)).collect();
        // LOCALE_SDAYNAME1 is Monday; .NET's DayOfWeek starts with Sunday.
        let days: Vec<String> = (0..7).filter_map(|i| locale(LOCALE_SDAYNAME1 + i)).collect();
        let abbr: Vec<String> = (0..7).filter_map(|i| locale(LOCALE_SABBREVDAYNAME1 + i)).collect();
        if days.len() == 7 {
            c.days = std::iter::once(days[6].clone()).chain(days[..6].iter().cloned()).collect();
        }
        if abbr.len() == 7 {
            c.days_abbr = std::iter::once(abbr[6].clone()).chain(abbr[..6].iter().cloned()).collect();
        }
        Some(c)
    }

    /// The two-letter code of the first preferred UI language ("ru-RU" -> "ru").
    pub fn ui_language_two_letter() -> String {
        let mut n: u32 = 0;
        let mut len: u32 = 0;
        unsafe { GetUserPreferredUILanguages(MUI_LANGUAGE_NAME, &mut n, std::ptr::null_mut(), &mut len) };
        if len == 0 {
            return "en".into();
        }
        let mut buf = vec![0u16; len as usize];
        if unsafe { GetUserPreferredUILanguages(MUI_LANGUAGE_NAME, &mut n, buf.as_mut_ptr(), &mut len) } == 0 {
            return "en".into();
        }
        let first = from_wide(&buf);
        first.split('-').next().unwrap_or("en").to_string()
    }

    fn known_folder(id: &windows_sys::core::GUID) -> PathBuf {
        let mut p: *mut u16 = std::ptr::null_mut();
        let hr = unsafe { SHGetKnownFolderPath(id, 0, std::ptr::null_mut(), &mut p) };
        let s = if hr >= 0 { unsafe { from_wide_ptr(p) } } else { String::new() };
        if !p.is_null() {
            unsafe { CoTaskMemFree(p as _) };
        }
        PathBuf::from(s)
    }

    pub fn common_app_data() -> PathBuf {
        known_folder(&FOLDERID_ProgramData)
    }
    pub fn local_app_data() -> PathBuf {
        known_folder(&FOLDERID_LocalAppData)
    }

    pub fn system_directory() -> PathBuf {
        let mut buf = vec![0u16; 520];
        let n = unsafe { GetSystemDirectoryW(buf.as_mut_ptr(), buf.len() as u32) };
        PathBuf::from(String::from_utf16_lossy(&buf[..n as usize]))
    }

    pub fn harden_dll_search() -> bool {
        unsafe { SetDefaultDllDirectories(LOAD_LIBRARY_SEARCH_SYSTEM32) != 0 }
    }

    pub fn is_wine() -> bool {
        use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
        let n = super::super::win::wide("ntdll.dll");
        let m = unsafe { GetModuleHandleW(n.as_ptr()) };
        !m.is_null() && unsafe { GetProcAddress(m, b"wine_get_version\0".as_ptr()) }.is_some()
    }
}

#[cfg(not(windows))]
mod imp {
    use crate::base::fmt::Culture;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU32, Ordering};

    pub fn cpu_set_buffer() -> Vec<u8> {
        Vec::new()
    }
    pub fn cache_buffer() -> Vec<u8> {
        Vec::new()
    }
    pub fn cpu_name() -> String {
        String::new()
    }
    pub fn active_lp_count() -> i32 {
        std::thread::available_parallelism().map(|n| n.get() as i32).unwrap_or(1)
    }
    pub fn tick_count64() -> u64 {
        use std::sync::OnceLock;
        static START: OnceLock<std::time::Instant> = OnceLock::new();
        START.get_or_init(std::time::Instant::now).elapsed().as_millis() as u64
    }
    pub fn current_thread_id() -> u32 {
        // A small number per thread (Linux has no GetCurrentThreadId; tests only).
        static NEXT: AtomicU32 = AtomicU32::new(1);
        thread_local!(static ID: u32 = NEXT.fetch_add(1, Ordering::Relaxed));
        ID.with(|i| *i)
    }
    pub fn current_process_id() -> u32 {
        std::process::id()
    }
    pub fn utc_offset_ticks(_utc_ticks: i64) -> i64 {
        0
    }
    pub fn current_culture() -> Option<Culture> {
        None
    }
    pub fn ui_language_two_letter() -> String {
        // .NET on Unix: LC_ALL, LC_MESSAGES, LANG; "C"/"POSIX" is the invariant culture ("iv").
        for v in ["LC_ALL", "LC_MESSAGES", "LANG"] {
            if let Ok(s) = std::env::var(v) {
                if !s.is_empty() {
                    if s == "C" || s == "POSIX" || s.starts_with("C.") {
                        return "iv".into();
                    }
                    return s.split(['_', '.', '-', '@']).next().unwrap_or("").to_string();
                }
            }
        }
        "iv".into()
    }
    pub fn common_app_data() -> PathBuf {
        // Not Windows: tests only (.NET maps LocalApplicationData to ~/.local/share there).
        local_app_data()
    }
    pub fn local_app_data() -> PathBuf {
        if let Ok(x) = std::env::var("XDG_DATA_HOME") {
            if !x.is_empty() {
                return PathBuf::from(x);
            }
        }
        PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".local").join("share")
    }
    pub fn system_directory() -> PathBuf {
        PathBuf::new()
    }
    pub fn harden_dll_search() -> bool {
        true
    }
    pub fn is_wine() -> bool {
        false
    }
}

/// The raw GetSystemCpuSetInformation buffer (empty when unknown).
pub fn cpu_set_buffer() -> Vec<u8> {
    imp::cpu_set_buffer()
}
/// The raw GetLogicalProcessorInformationEx(RelationCache) buffer (empty when unknown).
pub fn cache_buffer() -> Vec<u8> {
    imp::cache_buffer()
}
/// ProcessorNameString of processor 0 (cached).
pub fn cpu_name() -> String {
    imp::cpu_name()
}
/// GetActiveProcessorCount(ALL_PROCESSOR_GROUPS).
pub fn active_lp_count() -> i32 {
    imp::active_lp_count()
}
/// GetTickCount64: milliseconds since boot.
pub fn tick_count64() -> u64 {
    imp::tick_count64()
}
pub fn current_thread_id() -> u32 {
    imp::current_thread_id()
}
pub fn current_process_id() -> u32 {
    imp::current_process_id()
}
/// Local time minus UTC at that UTC instant, in ticks (0 on Linux).
pub fn utc_offset_ticks(utc_ticks: i64) -> i64 {
    imp::utc_offset_ticks(utc_ticks)
}
/// The user's number and time formats (None: the invariant culture).
pub fn current_culture() -> Option<Culture> {
    imp::current_culture()
}
/// `CultureInfo.CurrentUICulture.TwoLetterISOLanguageName`.
pub fn ui_language_two_letter() -> String {
    imp::ui_language_two_letter()
}
/// `Environment.GetFolderPath(CommonApplicationData)` (%ProgramData%).
pub fn common_app_data() -> PathBuf {
    imp::common_app_data()
}
/// `Environment.GetFolderPath(LocalApplicationData)`.
pub fn local_app_data() -> PathBuf {
    imp::local_app_data()
}
/// `Environment.SystemDirectory` (C:\Windows\System32).
pub fn system_directory() -> PathBuf {
    imp::system_directory()
}
/// Running under Wine (tests skip what only real Windows does, such as owner and permission changes).
pub fn is_wine() -> bool {
    imp::is_wine()
}
/// `[assembly: DefaultDllImportSearchPaths(System32)]`: every library loaded later comes from System32 only.
pub fn harden_dll_search() -> bool {
    imp::harden_dll_search()
}
