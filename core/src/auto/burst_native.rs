//! The real Windows calls of the burst guard (`WinBurstNative` of `BurstGuard.cs`). The native area's helpers are
//! used for the handles; the job, working-set and privilege calls are made here (see the report).

use super::burst_guard::IBurstNative;
use crate::native::win::{close_handle, get_current_process, get_process_times, h, hnd, last_error, open_process, wide};
use crate::native::Handle;
use std::sync::atomic::{AtomicBool, Ordering};
use windows_sys::Win32::Foundation::{HANDLE, LUID};
use windows_sys::Win32::Security::{AdjustTokenPrivileges, LookupPrivilegeValueW, LUID_AND_ATTRIBUTES, TOKEN_PRIVILEGES};
use windows_sys::Win32::System::JobObjects::{AssignProcessToJobObject, CreateJobObjectW, OpenJobObjectW, SetInformationJobObject};
use windows_sys::Win32::System::Memory::{GetProcessWorkingSetSizeEx, SetProcessWorkingSetSizeEx};
use windows_sys::Win32::System::ProcessStatus::{K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
use windows_sys::Win32::System::Threading::OpenProcessToken;

const PROCESS_SET_QUOTA: u32 = 0x0100;
const PROCESS_TERMINATE: u32 = 0x0001;
const PROCESS_QUERY_LIMITED: u32 = 0x1000;
const JOB_ALL: u32 = 0x1F001F;
const JOB_OBJECT_EXTENDED_LIMIT_INFORMATION: i32 = 9;
const JOB_OBJECT_CPU_RATE_CONTROL_INFORMATION: i32 = 15;
const JOB_OBJECT_LIMIT_SILENT_BREAKAWAY_OK: u32 = 0x1000;
const CPU_RATE_ENABLE: u32 = 0x1;
const CPU_RATE_WEIGHT_BASED: u32 = 0x2;
const CPU_RATE_HARD_CAP: u32 = 0x4;
const QUOTA_LIMITS_HARDWS_MIN_ENABLE: u32 = 0x1;
const QUOTA_LIMITS_HARDWS_MIN_DISABLE: u32 = 0x2;
const QUOTA_LIMITS_HARDWS_MAX_DISABLE: u32 = 0x8;

static PRIV: AtomicBool = AtomicBool::new(false);

fn privileges() {
    if PRIV.swap(true, Ordering::SeqCst) {
        return;
    }
    let mut t: HANDLE = std::ptr::null_mut();
    // TOKEN_ADJUST_PRIVILEGES | TOKEN_QUERY = 0x28
    if unsafe { OpenProcessToken(h(get_current_process()), 0x28, &mut t) } == 0 {
        return;
    }
    for name in ["SeIncreaseWorkingSetPrivilege", "SeIncreaseQuotaPrivilege"] {
        let w = wide(name);
        let mut luid = LUID { LowPart: 0, HighPart: 0 };
        if unsafe { LookupPrivilegeValueW(std::ptr::null(), w.as_ptr(), &mut luid) } != 0 {
            let tp = TOKEN_PRIVILEGES { PrivilegeCount: 1, Privileges: [LUID_AND_ATTRIBUTES { Luid: luid, Attributes: 2 }] };
            unsafe { AdjustTokenPrivileges(t, 0, &tp, 16, std::ptr::null_mut(), std::ptr::null_mut()) };
        }
    }
    close_handle(hnd(t));
}

fn cpu_rate_info(flags: u32, rate: u32) -> [u8; 8] {
    let mut b = [0u8; 8];
    b[..4].copy_from_slice(&flags.to_le_bytes());
    b[4..].copy_from_slice(&rate.to_le_bytes());
    b
}

fn set_info(job: Handle, cls: i32, data: &[u8]) -> bool {
    unsafe { SetInformationJobObject(h(job), cls, data.as_ptr() as *const core::ffi::c_void, data.len() as u32) != 0 }
}

/// `WinBurstNative`.
pub struct WinBurstNative;

impl IBurstNative for WinBurstNative {
    fn cpu_time(&self, pid: u32, created: &mut i64, cpu100ns: &mut i64) -> bool {
        *created = 0;
        *cpu100ns = 0;
        let x = open_process(PROCESS_QUERY_LIMITED, pid);
        if x.is_null() {
            return false;
        }
        let r = match get_process_times(x) {
            Some((c, _, k, u)) => {
                *created = c;
                *cpu100ns = k + u;
                true
            }
            None => false,
        };
        close_handle(x);
        r
    }

    fn cap(&self, pid: u32, job_name: &str, cpu_rate: u32, weight: i32, job: &mut Handle, error: &mut i32) -> bool {
        *job = Handle::NULL;
        *error = 0;
        // the two rights AssignProcessToJobObject needs
        let p = open_process(PROCESS_SET_QUOTA | PROCESS_TERMINATE, pid);
        if p.is_null() {
            *error = last_error();
            return false;
        }
        let r = (|| {
            let name = wide(job_name);
            let j = hnd(unsafe { CreateJobObjectW(std::ptr::null(), name.as_ptr()) });
            if j.is_null() {
                *error = last_error();
                return false;
            }
            // JOBOBJECT_EXTENDED_LIMIT_INFORMATION (x64, 144 bytes): BasicLimitInformation.LimitFlags at offset 16.
            let mut ext = [0u8; 144];
            ext[16..20].copy_from_slice(&JOB_OBJECT_LIMIT_SILENT_BREAKAWAY_OK.to_le_bytes());
            // The union after ControlFlags holds CpuRate (hard cap) or Weight (weight-based), both a DWORD at offset 4.
            let rate = if weight > 0 {
                cpu_rate_info(CPU_RATE_ENABLE | CPU_RATE_WEIGHT_BASED, weight.clamp(1, 9) as u32)
            } else {
                cpu_rate_info(CPU_RATE_ENABLE | CPU_RATE_HARD_CAP, cpu_rate)
            };
            if !set_info(j, JOB_OBJECT_EXTENDED_LIMIT_INFORMATION, &ext)
                || !set_info(j, JOB_OBJECT_CPU_RATE_CONTROL_INFORMATION, &rate)
                || unsafe { AssignProcessToJobObject(h(j), h(p)) } == 0
            {
                *error = last_error();
                self.lift(j);
                close_handle(j);
                return false;
            }
            *job = j;
            true
        })();
        close_handle(p);
        r
    }

    fn lift(&self, job: Handle) {
        if job.is_null() {
            return;
        }
        let off = cpu_rate_info(0, 0);
        set_info(job, JOB_OBJECT_CPU_RATE_CONTROL_INFORMATION, &off);
    }

    fn lift_by_name(&self, job_name: &str) -> bool {
        let name = wide(job_name);
        let j = hnd(unsafe { OpenJobObjectW(JOB_ALL, 0, name.as_ptr()) });
        if j.is_null() {
            return false;
        }
        self.lift(j);
        close_handle(j);
        true
    }

    fn close(&self, job: Handle) {
        if !job.is_null() {
            close_handle(job);
        }
    }

    fn memory(&self, avail: &mut u64, total: &mut u64) -> bool {
        // SAFETY: MEMORYSTATUSEX is plain data; zero is a valid start value.
        let mut m: MEMORYSTATUSEX = unsafe { std::mem::zeroed() };
        m.dwLength = std::mem::size_of::<MEMORYSTATUSEX>() as u32;
        let ok = unsafe { GlobalMemoryStatusEx(&mut m) } != 0;
        *avail = m.ullAvailPhys;
        *total = m.ullTotalPhys;
        ok
    }

    fn working_set(&self, pid: u32, ws: &mut u64, min: &mut u64, max: &mut u64) -> bool {
        *ws = 0;
        *min = 0;
        *max = 0;
        let x = open_process(PROCESS_QUERY_LIMITED, pid);
        if x.is_null() {
            return false;
        }
        let r = (|| {
            // PROCESS_MEMORY_COUNTERS (x64, 72 bytes): WorkingSetSize at offset 16.
            // SAFETY: plain data.
            let mut c: PROCESS_MEMORY_COUNTERS = unsafe { std::mem::zeroed() };
            c.cb = 72;
            if unsafe { K32GetProcessMemoryInfo(h(x), &mut c, 72) } == 0 {
                return false;
            }
            *ws = c.WorkingSetSize as u64;
            let (mut mn, mut mx, mut flags) = (0usize, 0usize, 0u32);
            if unsafe { GetProcessWorkingSetSizeEx(h(x), &mut mn, &mut mx, &mut flags) } == 0 {
                return false;
            }
            *min = mn as u64;
            *max = mx as u64;
            true
        })();
        close_handle(x);
        r
    }

    fn set_working_set_min(&self, pid: u32, min: u64, max: u64, hard: bool) -> bool {
        privileges();
        let x = open_process(PROCESS_SET_QUOTA | PROCESS_QUERY_LIMITED, pid);
        if x.is_null() {
            return false;
        }
        let flags = (if hard { QUOTA_LIMITS_HARDWS_MIN_ENABLE } else { QUOTA_LIMITS_HARDWS_MIN_DISABLE }) | QUOTA_LIMITS_HARDWS_MAX_DISABLE;
        let r = unsafe { SetProcessWorkingSetSizeEx(h(x), min as usize, max.max(min) as usize, flags) } != 0;
        close_handle(x);
        r
    }
}
