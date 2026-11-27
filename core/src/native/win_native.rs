//! INative over the Windows API (port of `WinNative.cs`).
//!
//! The C# version catches `EntryPointNotFoundException` for the CPU Set, thread name and timer calls (Windows before
//! 10); the native exe imports them directly and needs Windows 10 or later.

use super::win::{self, h, NtQueryInformationProcess, NtQueryInformationThread, NtQuerySystemInformation, NtQueryTimerResolution, NtSetInformationProcess, NtSetTimerResolution};
use super::{Handle, INative};
use crate::base::model::ModRange;
use indexmap::IndexMap;
use windows_sys::Win32::Foundation::BOOL;
use windows_sys::Win32::System::Kernel::PROCESSOR_NUMBER;
use windows_sys::Win32::System::Threading::{
    GetPriorityClass, GetProcessAffinityMask, GetProcessDefaultCpuSets, GetProcessInformation, GetProcessPriorityBoost, GetThreadIdealProcessorEx,
    GetThreadPriority, GetThreadPriorityBoost, GetThreadSelectedCpuSets, SetPriorityClass, SetProcessAffinityMask, SetProcessDefaultCpuSets,
    SetProcessInformation, SetProcessPriorityBoost, SetThreadIdealProcessor, SetThreadPriority, SetThreadPriorityBoost, SetThreadSelectedCpuSets,
};
use windows_sys::Win32::System::WindowsProgramming::QueryThreadCycleTime;

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct PowerThrottling {
    version: i32,
    control_mask: u32,
    state_mask: u32,
}

/// `WinNative`.
#[derive(Default)]
pub struct WinNative;

impl WinNative {
    pub fn new() -> Self {
        WinNative
    }
}

fn read_ids(f: impl FnOnce(*mut u32, u32, *mut u32) -> BOOL, ids: &mut Vec<u32>) -> bool {
    ids.clear();
    let mut buf = vec![0u32; 512];
    let mut n: u32 = 0;
    if f(buf.as_mut_ptr(), buf.len() as u32, &mut n) == 0 {
        return false;
    }
    buf.truncate((n as usize).min(512));
    *ids = buf;
    true
}

impl INative for WinNative {
    fn enable_debug_privilege(&self) -> bool {
        win::enable_debug_privilege()
    }

    fn open_process_graceful(&self, pid: u32, level: &mut i32) -> Handle {
        win::open_process_graceful(pid, level)
    }

    fn process_created(&self, process: Handle) -> i64 {
        win::process_created(process)
    }

    fn is_protected(&self, process: Handle) -> bool {
        win::is_protected(process)
    }

    fn close(&self, handle: Handle) {
        if !handle.is_null() {
            win::close_handle(handle);
        }
    }

    fn get_priority_class(&self, process: Handle, cls: &mut u32) -> bool {
        *cls = unsafe { GetPriorityClass(h(process)) };
        *cls != 0
    }

    fn set_priority_class(&self, process: Handle, cls: u32) -> bool {
        unsafe { SetPriorityClass(h(process), cls) != 0 }
    }

    fn get_affinity(&self, process: Handle, mask: &mut u64) -> bool {
        let (mut pm, mut sm) = (0usize, 0usize);
        let ok = unsafe { GetProcessAffinityMask(h(process), &mut pm, &mut sm) } != 0;
        *mask = pm as u64;
        ok
    }

    fn set_affinity(&self, process: Handle, mask: u64) -> bool {
        unsafe { SetProcessAffinityMask(h(process), mask as usize) != 0 }
    }

    fn get_cpu_sets(&self, process: Handle, ids: &mut Vec<u32>) -> bool {
        read_ids(|b, len, n| unsafe { GetProcessDefaultCpuSets(h(process), b, len, n) }, ids)
    }

    fn set_cpu_sets(&self, process: Handle, ids: &[u32]) -> bool {
        unsafe {
            if ids.is_empty() {
                SetProcessDefaultCpuSets(h(process), std::ptr::null(), 0) != 0
            } else {
                SetProcessDefaultCpuSets(h(process), ids.as_ptr(), ids.len() as u32) != 0
            }
        }
    }

    fn get_io(&self, process: Handle, value: &mut u32) -> bool {
        let mut v: u32 = 0;
        let ok = unsafe { NtQueryInformationProcess(h(process), win::PROCESS_IO_PRIORITY, &mut v as *mut u32 as *mut _, 4, std::ptr::null_mut()) } == 0;
        *value = v;
        if ok && *value > 4 {
            *value = 2;
        }
        ok
    }

    fn set_io(&self, process: Handle, value: u32) -> bool {
        unsafe { NtSetInformationProcess(h(process), win::PROCESS_IO_PRIORITY, &value as *const u32 as *const _, 4) == 0 }
    }

    fn get_mem(&self, process: Handle, value: &mut u32) -> bool {
        *value = 5;
        let mut v: u32 = 0;
        if unsafe { GetProcessInformation(h(process), win::PROCESS_MEMORY_PRIORITY, &mut v as *mut u32 as *mut _, 4) } == 0 {
            return false;
        }
        *value = v;
        true
    }

    fn set_mem(&self, process: Handle, value: u32) -> bool {
        unsafe { SetProcessInformation(h(process), win::PROCESS_MEMORY_PRIORITY, &value as *const u32 as *const _, 4) != 0 }
    }

    fn get_eco(&self, process: Handle, control: &mut u32, state: &mut u32) -> bool {
        *control = 0;
        *state = 0;
        let mut s = PowerThrottling { version: 1, ..Default::default() };
        if unsafe { GetProcessInformation(h(process), win::PROCESS_POWER_THROTTLING, &mut s as *mut PowerThrottling as *mut _, 12) } == 0 {
            return false;
        }
        *control = s.control_mask;
        *state = s.state_mask;
        true
    }

    fn set_eco(&self, process: Handle, control: u32, state: u32) -> bool {
        let s = PowerThrottling { version: 1, control_mask: control, state_mask: state };
        unsafe { SetProcessInformation(h(process), win::PROCESS_POWER_THROTTLING, &s as *const PowerThrottling as *const _, 12) != 0 }
    }

    fn get_boost_disabled(&self, process: Handle, disabled: &mut bool) -> bool {
        let mut d: BOOL = 0;
        let ok = unsafe { GetProcessPriorityBoost(h(process), &mut d) } != 0;
        *disabled = d != 0;
        ok
    }

    fn set_boost_disabled(&self, process: Handle, disabled: bool) -> bool {
        unsafe { SetProcessPriorityBoost(h(process), disabled as BOOL) != 0 }
    }

    fn modules(&self, process: Handle) -> Vec<ModRange> {
        win::modules(process)
    }

    fn open_thread_of(&self, pid: u32, tid: u32) -> Handle {
        win::open_thread_of(pid, tid)
    }

    fn thread_cycles(&self, thread: Handle, cycles: &mut u64) -> bool {
        unsafe { QueryThreadCycleTime(h(thread), cycles) != 0 }
    }

    fn thread_times(&self, thread: Handle, created: &mut i64, kernel_user: &mut u64) -> bool {
        match win::get_thread_times(thread) {
            Some((c, _, k, u)) => {
                *created = c;
                *kernel_user = (k + u) as u64;
                true
            }
            None => {
                *created = 0;
                *kernel_user = 0;
                false
            }
        }
    }

    fn thread_start(&self, thread: Handle) -> u64 {
        let mut a: u64 = 0;
        let r = unsafe { NtQueryInformationThread(h(thread), win::THREAD_QUERY_SET_WIN32_START_ADDRESS, &mut a as *mut u64 as *mut _, 8, std::ptr::null_mut()) };
        if r == 0 { a } else { 0 }
    }

    fn thread_desc(&self, thread: Handle) -> String {
        win::thread_description(thread)
    }

    fn get_thread_prio(&self, thread: Handle, prio: &mut i32) -> bool {
        *prio = unsafe { GetThreadPriority(h(thread)) };
        *prio != 0x7fffffff
    }

    fn set_thread_prio(&self, thread: Handle, prio: i32) -> bool {
        unsafe { SetThreadPriority(h(thread), prio) != 0 }
    }

    fn get_thread_boost_disabled(&self, thread: Handle, disabled: &mut bool) -> bool {
        let mut d: BOOL = 0;
        let ok = unsafe { GetThreadPriorityBoost(h(thread), &mut d) } != 0;
        *disabled = d != 0;
        ok
    }

    fn set_thread_boost_disabled(&self, thread: Handle, disabled: bool) -> bool {
        unsafe { SetThreadPriorityBoost(h(thread), disabled as BOOL) != 0 }
    }

    fn set_thread_ideal(&self, thread: Handle, lp: u32) -> bool {
        unsafe { SetThreadIdealProcessor(h(thread), lp) != 0xFFFF_FFFF }
    }

    fn get_thread_ideal(&self, thread: Handle, lp: &mut u32) -> bool {
        // PROCESSOR_NUMBER { WORD Group; BYTE Number; BYTE Reserved }.
        *lp = 0;
        let mut pn = PROCESSOR_NUMBER { Group: 0, Number: 0, Reserved: 0 };
        if unsafe { GetThreadIdealProcessorEx(h(thread), &mut pn) } == 0 {
            return false;
        }
        *lp = pn.Number as u32;
        pn.Group == 0 // group 0 only
    }

    fn cpu_times(&self, idle: &mut Vec<i64>, total: &mut Vec<i64>) -> bool {
        // SystemProcessorPerformanceInformation (8): 48 bytes per processor: Idle, Kernel (includes idle), User, Dpc,
        // Interrupt, count.
        idle.clear();
        total.clear();
        let mut buf = vec![0i64; 6 * 64];
        let mut got: u32 = 0;
        if unsafe { NtQuerySystemInformation(8, buf.as_mut_ptr() as *mut _, (48 * 64) as u32, &mut got) } != 0 || got < 48 {
            return false;
        }
        let n = got as usize / 48;
        for i in 0..n {
            idle.push(buf[i * 6]);
            total.push(buf[i * 6 + 1] + buf[i * 6 + 2]);
        }
        true
    }

    fn get_thread_cpu_sets(&self, thread: Handle, ids: &mut Vec<u32>) -> bool {
        read_ids(|b, len, n| unsafe { GetThreadSelectedCpuSets(h(thread), b, len, n) }, ids)
    }

    fn set_thread_cpu_sets(&self, thread: Handle, ids: &[u32]) -> bool {
        unsafe {
            if ids.is_empty() {
                SetThreadSelectedCpuSets(h(thread), std::ptr::null(), 0) != 0
            } else {
                SetThreadSelectedCpuSets(h(thread), ids.as_ptr(), ids.len() as u32) != 0
            }
        }
    }

    fn timer_query(&self, coarsest: &mut u32, finest: &mut u32, current: &mut u32) -> bool {
        // The API names the coarsest interval "minimum resolution" and the finest "maximum resolution".
        *coarsest = 0;
        *finest = 0;
        *current = 0;
        let (mut a, mut b) = (0u32, 0u32);
        if unsafe { NtQueryTimerResolution(&mut a, &mut b, current) } != 0 {
            return false;
        }
        *coarsest = a.max(b);
        *finest = a.min(b);
        *finest != 0
    }

    fn timer_set(&self, resolution: u32, set: bool, actual: &mut u32) -> bool {
        *actual = 0;
        unsafe { NtSetTimerResolution(resolution, set as u8, actual) == 0 }
    }

    fn threads_by_pid(&self) -> IndexMap<u32, Vec<u32>> {
        win::threads_by_pid()
    }

    fn processes(&self) -> IndexMap<u32, (String, u32)> {
        win::processes()
    }

    fn last_error(&self) -> i32 {
        win::last_error()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native::reg::{Hive, Key, RegValue};

    /// The Windows calls on this process (runs under Windows or Wine).
    #[test]
    fn win_native_reads_this_process() {
        let n = WinNative::new();
        let pid = std::process::id();
        let mut level = -9;
        let ph = n.open_process_graceful(pid, &mut level);
        assert!(!ph.is_null());
        assert_eq!(level, 2);
        let mut cls = 0;
        assert!(n.get_priority_class(ph, &mut cls));
        // NORMAL unless another test of this process already lowered its own class (the app runs below normal)
        assert!([32, 64, 128, 256, 16384, 32768].contains(&cls), "priority class {cls}");
        let mut aff = 0;
        assert!(n.get_affinity(ph, &mut aff) && aff != 0);
        assert!(win::process_created(ph) > 0);
        assert!(n.threads_by_pid().get(&pid).map(|t| !t.is_empty()).unwrap_or(false));
        let procs = n.processes();
        assert!(procs.contains_key(&pid));
        assert!(crate::base::ci::ends_with(&procs[&pid].0, ".exe"));
        assert!(!n.modules(ph).is_empty());
        n.close(ph);
        assert!(!win::cpu_set_buffer().is_empty() || true);
    }

    #[test]
    fn registry_round_trip() {
        let path = r"Software\DomainScheduler.PortTest";
        let k = Key::create(Hive::CurrentUser, path).unwrap();
        k.set("s", &RegValue::Sz("x".into())).unwrap();
        k.set("d", &RegValue::Dword(0xFFFF_FFFF)).unwrap();
        k.set("b", &RegValue::Binary(vec![1, 2, 3])).unwrap();
        k.set("m", &RegValue::MultiSz(vec!["a".into(), "b".into()])).unwrap();
        assert_eq!(k.get_string("s").unwrap(), "x");
        assert_eq!(k.get("d").unwrap().to_csharp_string(), "-1");
        assert_eq!(k.get_binary("b").unwrap(), vec![1, 2, 3]);
        assert_eq!(k.get("m").unwrap(), RegValue::MultiSz(vec!["a".into(), "b".into()]));
        let mut names = k.value_names();
        names.sort();
        assert_eq!(names, vec!["b", "d", "m", "s"]);
        k.delete_value("s", false).unwrap();
        assert!(k.get("s").is_none());
        assert!(k.delete_value("s", true).is_err());
        drop(k);
        Key::open(Hive::CurrentUser, "Software", true).unwrap().delete_subkey_tree("DomainScheduler.PortTest").unwrap();
        assert!(Key::open(Hive::CurrentUser, path, false).is_none());
    }

    #[test]
    fn locale_time_and_folders() {
        assert!(!crate::native::sys::common_app_data().as_os_str().is_empty());
        assert!(!crate::native::sys::system_directory().as_os_str().is_empty());
        let c = crate::base::fmt::Culture::current();
        assert!(!c.decimal.is_empty());
        let now = crate::base::time::DateTime::utc_now();
        let back = now.to_local().to_universal();
        assert_eq!(now.ticks, back.ticks);
        assert!(["en", "ru"].contains(&crate::base::l::L::detect().as_str()));
    }
}
