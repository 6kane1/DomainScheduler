//! Windows calls (section 2, port of the static class `Native.cs`). Every handle is closed by its caller.
//! Windows only. The raw imports of ntdll that windows-sys does not carry are declared here.

#![allow(non_snake_case)]

use super::Handle;
use crate::base::model::ModRange;
use indexmap::IndexMap;
use std::sync::OnceLock;
use windows_sys::Win32::Foundation::{CloseHandle as Win32CloseHandle, GetLastError, LocalFree, BOOL, FILETIME, HANDLE, HMODULE, INVALID_HANDLE_VALUE, LUID};
use windows_sys::Win32::Security::{AdjustTokenPrivileges, LookupPrivilegeValueW, LUID_AND_ATTRIBUTES, TOKEN_PRIVILEGES};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, Thread32First, Thread32Next, PROCESSENTRY32W, THREADENTRY32,
};
use windows_sys::Win32::System::ProcessStatus::{K32EnumProcessModulesEx, K32GetModuleBaseNameW, K32GetModuleInformation, MODULEINFO};
use windows_sys::Win32::System::SystemInformation::{GetLogicalProcessorInformationEx, GetSystemCpuSetInformation, GetTickCount64 as Win32GetTickCount64};
use windows_sys::Win32::System::Threading::{
    GetActiveProcessorCount, GetCurrentProcess, GetProcessIdOfThread, GetProcessTimes, GetThreadDescription, GetThreadTimes, OpenProcess as Win32OpenProcess,
    OpenProcessToken, OpenThread as Win32OpenThread, QueryFullProcessImageNameW,
};

pub const P_FULL: u32 = 13840; // 0x3610
pub const P_NOVM: u32 = 13824; // 0x3600
pub const P_LIMITED: u32 = 12288; // 0x3000
pub const T_FULL: u32 = 3168; // 0x0C60
pub const T_LIMITED: u32 = 3072; // 0x0C00
pub const TH32CS_SNAPTHREAD: u32 = 4;
pub const TH32CS_SNAPPROCESS: u32 = 2;
pub const LIST_MODULES_ALL: u32 = 3;
pub const PROCESS_IO_PRIORITY: i32 = 33;
pub const THREAD_IO_PRIORITY: i32 = 22;
pub const PROCESS_MEMORY_PRIORITY: i32 = 0;
pub const THREAD_MEMORY_PRIORITY: i32 = 0;
pub const PROCESS_POWER_THROTTLING: i32 = 4;
pub const THREAD_QUERY_SET_WIN32_START_ADDRESS: i32 = 9;
pub const PROCESS_PROTECTION_INFORMATION: i32 = 61;
pub const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;

#[link(name = "ntdll")]
extern "system" {
    pub fn NtQuerySystemInformation(cls: i32, buffer: *mut core::ffi::c_void, length: u32, returned: *mut u32) -> i32;
    pub fn NtQueryTimerResolution(minimum: *mut u32, maximum: *mut u32, current: *mut u32) -> i32;
    pub fn NtSetTimerResolution(desired: u32, set: u8, actual: *mut u32) -> i32;
    pub fn NtQueryInformationProcess(process: HANDLE, cls: i32, info: *mut core::ffi::c_void, size: u32, ret: *mut u32) -> i32;
    pub fn NtSetInformationProcess(process: HANDLE, cls: i32, info: *const core::ffi::c_void, size: u32) -> i32;
    pub fn NtQueryInformationThread(thread: HANDLE, cls: i32, info: *mut core::ffi::c_void, size: u32, ret: *mut u32) -> i32;
    pub fn NtSetInformationThread(thread: HANDLE, cls: i32, info: *const core::ffi::c_void, size: u32) -> i32;
}

/// A NUL-terminated UTF-16 string.
pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// UTF-16 (up to the first NUL) to a String.
pub fn from_wide(w: &[u16]) -> String {
    let n = w.iter().position(|c| *c == 0).unwrap_or(w.len());
    String::from_utf16_lossy(&w[..n])
}

/// A NUL-terminated UTF-16 string at a pointer.
///
/// # Safety
/// `p` is null or points to a NUL-terminated UTF-16 string.
pub unsafe fn from_wide_ptr(p: *const u16) -> String {
    if p.is_null() {
        return String::new();
    }
    let mut n = 0;
    while *p.add(n) != 0 {
        n += 1;
    }
    String::from_utf16_lossy(std::slice::from_raw_parts(p, n))
}

pub fn h(x: Handle) -> HANDLE {
    x.0 as HANDLE
}

pub fn hnd(x: HANDLE) -> Handle {
    Handle(x as isize)
}

/// `GetLastError()` (C# `Marshal.GetLastWin32Error()`).
pub fn last_error() -> i32 {
    unsafe { GetLastError() as i32 }
}

pub fn close_handle(x: Handle) -> bool {
    !x.is_null() && unsafe { Win32CloseHandle(h(x)) != 0 }
}

pub fn get_current_process() -> Handle {
    hnd(unsafe { GetCurrentProcess() })
}

pub fn open_process(access: u32, pid: u32) -> Handle {
    hnd(unsafe { Win32OpenProcess(access, 0, pid) })
}

pub fn open_thread(access: u32, tid: u32) -> Handle {
    hnd(unsafe { Win32OpenThread(access, 0, tid) })
}

pub fn get_tick_count64() -> u64 {
    unsafe { Win32GetTickCount64() }
}

fn ft(f: &FILETIME) -> i64 {
    ((f.dwHighDateTime as i64) << 32) | f.dwLowDateTime as i64
}

/// GetProcessTimes: (created, exit, kernel, user) FILETIMEs.
pub fn get_process_times(process: Handle) -> Option<(i64, i64, i64, i64)> {
    let z = FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 };
    let (mut c, mut e, mut k, mut u) = (z, z, z, z);
    let ok = unsafe { GetProcessTimes(h(process), &mut c, &mut e, &mut k, &mut u) } != 0;
    ok.then(|| (ft(&c), ft(&e), ft(&k), ft(&u)))
}

/// GetThreadTimes: (created, exit, kernel, user) FILETIMEs.
pub fn get_thread_times(thread: Handle) -> Option<(i64, i64, i64, i64)> {
    let z = FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 };
    let (mut c, mut e, mut k, mut u) = (z, z, z, z);
    let ok = unsafe { GetThreadTimes(h(thread), &mut c, &mut e, &mut k, &mut u) } != 0;
    ok.then(|| (ft(&c), ft(&e), ft(&k), ft(&u)))
}

// ---- helpers -------------------------------------------------------------------------------

/// Opens a process with the most rights it allows: 2 full, 1 without VM read, 0 limited, -1 refused.
pub fn open_process_graceful(pid: u32, level: &mut i32) -> Handle {
    let x = open_process(P_FULL, pid);
    if !x.is_null() {
        *level = 2;
        return x;
    }
    let x = open_process(P_NOVM, pid);
    if !x.is_null() {
        *level = 1;
        return x;
    }
    let x = open_process(P_LIMITED, pid);
    if !x.is_null() {
        *level = 0;
        return x;
    }
    *level = -1;
    Handle::NULL
}

/// Opens a thread and checks that it belongs to pid; NULL otherwise.
pub fn open_thread_of(pid: u32, tid: u32) -> Handle {
    let mut x = open_thread(T_FULL, tid);
    if x.is_null() {
        x = open_thread(T_LIMITED, tid);
    }
    if x.is_null() {
        return Handle::NULL;
    }
    if unsafe { GetProcessIdOfThread(h(x)) } == pid {
        x
    } else {
        close_handle(x);
        Handle::NULL
    }
}

/// Turns SeDebugPrivilege on for this process (token 0x28, Attributes 2, len 16).
pub fn enable_debug_privilege() -> bool {
    let mut token: HANDLE = std::ptr::null_mut();
    if unsafe { OpenProcessToken(GetCurrentProcess(), 0x28, &mut token) } == 0 {
        return false;
    }
    let r = (|| {
        let mut luid = LUID { LowPart: 0, HighPart: 0 };
        let name = wide("SeDebugPrivilege");
        if unsafe { LookupPrivilegeValueW(std::ptr::null(), name.as_ptr(), &mut luid) } == 0 {
            return false;
        }
        let tp = TOKEN_PRIVILEGES { PrivilegeCount: 1, Privileges: [LUID_AND_ATTRIBUTES { Luid: luid, Attributes: 2 }] };
        if unsafe { AdjustTokenPrivileges(token, 0, &tp, 16, std::ptr::null_mut(), std::ptr::null_mut()) } == 0 {
            return false;
        }
        last_error() == 0
    })();
    close_handle(hnd(token));
    r
}

/// The raw GetSystemCpuSetInformation buffer (two-phase), empty on failure.
pub fn cpu_set_buffer() -> Vec<u8> {
    let mut len: u32 = 0;
    unsafe { GetSystemCpuSetInformation(std::ptr::null_mut(), 0, &mut len, GetCurrentProcess(), 0) };
    if len == 0 || len > 1 << 22 {
        return Vec::new();
    }
    // u64 storage keeps the buffer aligned for the API.
    let mut buf = vec![0u64; (len as usize).div_ceil(8)];
    let mut got: u32 = 0;
    if unsafe { GetSystemCpuSetInformation(buf.as_mut_ptr() as *mut _, len, &mut got, GetCurrentProcess(), 0) } == 0 {
        return Vec::new();
    }
    let n = len.min(if got == 0 { len } else { got }) as usize;
    let bytes: Vec<u8> = buf.iter().flat_map(|w| w.to_le_bytes()).collect();
    bytes[..n].to_vec()
}

/// The raw GetLogicalProcessorInformationEx(RelationCache) buffer (two-phase), empty on failure.
pub fn cache_buffer() -> Vec<u8> {
    let mut len: u32 = 0;
    unsafe { GetLogicalProcessorInformationEx(2, std::ptr::null_mut(), &mut len) };
    if len == 0 || len > 1 << 22 {
        return Vec::new();
    }
    let mut buf = vec![0u64; (len as usize).div_ceil(8)];
    if unsafe { GetLogicalProcessorInformationEx(2, buf.as_mut_ptr() as *mut _, &mut len) } == 0 {
        return Vec::new();
    }
    let bytes: Vec<u8> = buf.iter().flat_map(|w| w.to_le_bytes()).collect();
    bytes[..len as usize].to_vec()
}

/// ProcessorNameString of processor 0, cached for the life of the process.
pub fn cpu_name() -> String {
    static NAME: OnceLock<String> = OnceLock::new();
    NAME.get_or_init(|| {
        super::reg::Key::open(super::reg::Hive::LocalMachine, r"HARDWARE\DESCRIPTION\System\CentralProcessor\0", false)
            .and_then(|k| k.get_string("ProcessorNameString"))
            .map(|s| s.trim().to_string())
            .unwrap_or_default()
    })
    .clone()
}

/// GetActiveProcessorCount(ALL_PROCESSOR_GROUPS), fallback Environment.ProcessorCount.
pub fn active_lp_count() -> i32 {
    let n = unsafe { GetActiveProcessorCount(0xFFFF) };
    if n > 0 {
        return n as i32;
    }
    std::thread::available_parallelism().map(|n| n.get() as i32).unwrap_or(1)
}

/// The thread's start address (NtQueryInformationThread ThreadQuerySetWin32StartAddress), 0 when unknown.
pub fn thread_start_address(thread: Handle) -> u64 {
    let mut a: u64 = 0;
    let r = unsafe { NtQueryInformationThread(h(thread), THREAD_QUERY_SET_WIN32_START_ADDRESS, &mut a as *mut u64 as *mut _, 8, std::ptr::null_mut()) };
    if r == 0 { a } else { 0 }
}

/// The thread's name (GetThreadDescription, freed with LocalFree).
pub fn thread_description(thread: Handle) -> String {
    let mut p: *mut u16 = std::ptr::null_mut();
    let hr = unsafe { GetThreadDescription(h(thread), &mut p) };
    let s = if hr < 0 || p.is_null() { String::new() } else { unsafe { from_wide_ptr(p) } };
    if !p.is_null() {
        unsafe { LocalFree(p as _) };
    }
    s
}

/// Process creation FILETIME, 0 when unknown.
pub fn process_created(process: Handle) -> i64 {
    get_process_times(process).map(|t| t.0).unwrap_or(0)
}

/// Thread creation FILETIME, 0 when unknown.
pub fn thread_created(thread: Handle) -> i64 {
    get_thread_times(thread).map(|t| t.0).unwrap_or(0)
}

fn full_image(pid: u32) -> Option<String> {
    let x = open_process(PROCESS_QUERY_LIMITED_INFORMATION, pid);
    if x.is_null() {
        return None;
    }
    let mut buf = vec![0u16; 1024];
    let mut size = buf.len() as u32;
    let ok = unsafe { QueryFullProcessImageNameW(h(x), 0, buf.as_mut_ptr(), &mut size) } != 0;
    close_handle(x);
    ok.then(|| String::from_utf16_lossy(&buf[..size as usize]))
}

/// The image name of a process ("cs2.exe"), "" when unknown.
pub fn image_name(pid: u32) -> String {
    full_image(pid).map(|p| file_name(&p)).unwrap_or_default()
}

/// The full image path of a process (for its icon), "" when unknown. Read-only access.
pub fn image_path(pid: u32) -> String {
    full_image(pid).unwrap_or_default()
}

/// `Path.GetFileName` for a Windows path.
pub fn file_name(p: &str) -> String {
    p.rsplit(['\\', '/']).next().unwrap_or("").to_string()
}

/// Modules of a process (up to 2048), names up to 260 characters.
pub fn modules(process: Handle) -> Vec<ModRange> {
    let mut list = Vec::new();
    let mut mods: Vec<HMODULE> = vec![std::ptr::null_mut(); 2048];
    let mut needed: u32 = 0;
    let cb = (mods.len() * std::mem::size_of::<HMODULE>()) as u32;
    if unsafe { K32EnumProcessModulesEx(h(process), mods.as_mut_ptr(), cb, &mut needed, LIST_MODULES_ALL) } == 0 {
        return list;
    }
    let n = mods.len().min(needed as usize / std::mem::size_of::<HMODULE>());
    let mut name = [0u16; 260];
    for m in mods.iter().take(n) {
        let mut mi = MODULEINFO { lpBaseOfDll: std::ptr::null_mut(), SizeOfImage: 0, EntryPoint: std::ptr::null_mut() };
        if unsafe { K32GetModuleInformation(h(process), *m, &mut mi, std::mem::size_of::<MODULEINFO>() as u32) } == 0 {
            continue;
        }
        let len = unsafe { K32GetModuleBaseNameW(h(process), *m, name.as_mut_ptr(), name.len() as u32) };
        if len == 0 {
            continue;
        }
        list.push(ModRange { base: mi.lpBaseOfDll as u64, size: mi.SizeOfImage as u64, name: String::from_utf16_lossy(&name[..len as usize]) });
    }
    list
}

/// Protected process (PPL / PP).
pub fn is_protected(process: Handle) -> bool {
    let mut prot: u8 = 0;
    unsafe { NtQueryInformationProcess(h(process), PROCESS_PROTECTION_INFORMATION, &mut prot as *mut u8 as *mut _, 1, std::ptr::null_mut()) == 0 && prot != 0 }
}

/// Every thread of every process, from one CreateToolhelp32Snapshot.
pub fn threads_by_pid() -> IndexMap<u32, Vec<u32>> {
    let mut d: IndexMap<u32, Vec<u32>> = IndexMap::new();
    let snap = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) };
    if snap.is_null() || snap == INVALID_HANDLE_VALUE {
        return d;
    }
    let size = std::mem::size_of::<THREADENTRY32>() as u32;
    let mut e = THREADENTRY32 { dwSize: size, cntUsage: 0, th32ThreadID: 0, th32OwnerProcessID: 0, tpBasePri: 0, tpDeltaPri: 0, dwFlags: 0 };
    if unsafe { Thread32First(snap, &mut e) } != 0 {
        loop {
            d.entry(e.th32OwnerProcessID).or_default().push(e.th32ThreadID);
            e.dwSize = size;
            if unsafe { Thread32Next(snap, &mut e) } == 0 {
                break;
            }
        }
    }
    close_handle(hnd(snap));
    d
}

/// Every process: pid -> (image name, parent pid), from one snapshot.
pub fn processes() -> IndexMap<u32, (String, u32)> {
    let mut d = IndexMap::new();
    let snap = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if snap.is_null() || snap == INVALID_HANDLE_VALUE {
        return d;
    }
    let size = std::mem::size_of::<PROCESSENTRY32W>() as u32;
    // SAFETY: PROCESSENTRY32W is plain data; all-zero is a valid value.
    let mut e: PROCESSENTRY32W = unsafe { std::mem::zeroed() };
    e.dwSize = size;
    if unsafe { Process32FirstW(snap, &mut e) } != 0 {
        loop {
            d.insert(e.th32ProcessID, (from_wide(&e.szExeFile), e.th32ParentProcessID));
            e.dwSize = size;
            if unsafe { Process32NextW(snap, &mut e) } == 0 {
                break;
            }
        }
    }
    close_handle(hnd(snap));
    d
}

/// BOOL to bool.
pub fn b(x: BOOL) -> bool {
    x != 0
}
