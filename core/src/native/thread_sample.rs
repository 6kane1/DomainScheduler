//! What the program's unnamed threads run (native 1.17.0, self-check): thread-pool workers (start ntdll
//! TppWorkerThread) carry no name, and their start address says nothing about the work they are given. A few times a
//! minute each busy unregistered thread of this process is stopped for a few microseconds: its instruction pointer and
//! the top of its stack are copied into a buffer allocated beforehand, and it runs on. The copy is then read for
//! addresses inside the loaded modules; the first module that is not the system's thread plumbing (ntdll, kernel32,
//! kernelbase) is the one that gave the thread its work ("d3d10warp.dll", "sechost.dll", "DomainScheduler.exe").
//!
//! Safe by construction: only threads of this process, never the calling one; between SuspendThread and ResumeThread
//! only system calls run (GetThreadContext, ReadProcessMemory into the preallocated buffer): no allocation, no lock
//! the stopped thread could hold. Read-only; nothing of the thread is changed.

use crate::base::model::ModRange;

/// Words of stack copied per sample (16 KB).
pub const STACK_WORDS: usize = 2048;

/// Modules that run every thread and every pool callback: never "the work".
const PLUMBING: [&str; 4] = ["ntdll.dll", "kernel32.dll", "kernelbase.dll", "win32u.dll"];

/// The module that gave the thread its work: the instruction pointer's module, else the first module address found
/// on the stack copy (top first), skipping the system's thread plumbing. None when only plumbing is seen (an idle
/// pool worker waiting for work).
pub fn work_module<'a>(rip: u64, stack: &[u64], mods: &'a [ModRange]) -> Option<&'a str> {
    let find = |a: u64| mods.iter().find(|m| a >= m.base && a - m.base < m.size);
    let plumbing = |m: &ModRange| PLUMBING.iter().any(|p| m.name.eq_ignore_ascii_case(p));
    std::iter::once(rip).chain(stack.iter().copied()).filter_map(find).find(|m| !plumbing(m)).map(|m| m.name.as_str())
}

/// One sample of a thread: (instruction pointer, words copied into `stack`); None when it could not be read.
#[cfg(windows)]
pub fn sample(tid: u32, stack: &mut [u64; STACK_WORDS]) -> Option<(u64, usize)> {
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::System::Diagnostics::Debug::{GetThreadContext, ReadProcessMemory, CONTEXT};
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetCurrentThreadId, GetProcessIdOfThread, OpenThread, ResumeThread, SuspendThread};
    const ACCESS: u32 = 0x0002 /* SUSPEND_RESUME */ | 0x0008 /* GET_CONTEXT */ | 0x0040 /* QUERY_INFORMATION */;
    const CONTEXT_CONTROL: u32 = 0x0010_0001; // AMD64: Rip, Rsp, segments, flags
    // SAFETY: plain Win32 calls on a thread of this process; the buffers are ours and sized for the reads.
    unsafe {
        if tid == GetCurrentThreadId() {
            return None;
        }
        let th: HANDLE = OpenThread(ACCESS, 0, tid);
        if th.is_null() {
            return None;
        }
        if GetProcessIdOfThread(th) != std::process::id() {
            CloseHandle(th);
            return None;
        }
        // Stack base from the TEB (NT_TIB.StackBase at +8), read before the thread stops.
        #[repr(C)]
        struct Tbi {
            exit: i32,
            teb: u64,
            client: [u64; 2],
            affinity: u64,
            prio: i32,
            base_prio: i32,
        }
        let mut tbi: Tbi = std::mem::zeroed();
        let mut base: u64 = 0;
        let me = GetCurrentProcess();
        let ok = super::win::NtQueryInformationThread(th, 0, &mut tbi as *mut Tbi as *mut _, std::mem::size_of::<Tbi>() as u32, std::ptr::null_mut()) == 0
            && tbi.teb != 0
            && ReadProcessMemory(me, (tbi.teb + 8) as *const _, &mut base as *mut u64 as *mut _, 8, std::ptr::null_mut()) != 0;
        if !ok || SuspendThread(th) == u32::MAX {
            CloseHandle(th);
            return None;
        }
        let mut ctx: CONTEXT = std::mem::zeroed();
        ctx.ContextFlags = CONTEXT_CONTROL;
        let mut words = 0usize;
        let got = GetThreadContext(th, &mut ctx) != 0;
        if got && ctx.Rsp != 0 && base > ctx.Rsp {
            let want = (((base - ctx.Rsp) / 8) as usize).min(STACK_WORDS);
            let mut read = 0usize;
            if ReadProcessMemory(me, ctx.Rsp as *const _, stack.as_mut_ptr() as *mut _, want * 8, &mut read) != 0 {
                words = read / 8;
            }
        }
        ResumeThread(th);
        CloseHandle(th);
        got.then_some((ctx.Rip, words))
    }
}

#[cfg(not(windows))]
pub fn sample(_tid: u32, _stack: &mut [u64; STACK_WORDS]) -> Option<(u64, usize)> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_work_is_the_first_module_past_the_thread_plumbing() {
        let m = |b: u64, n: &str| ModRange { base: b, size: 0x1000, name: n.into() };
        let mods = vec![m(0x1000, "ntdll.dll"), m(0x3000, "KERNEL32.DLL"), m(0x5000, "d3d10warp.dll"), m(0x7000, "DomainScheduler.exe")];
        // pool worker inside WARP: rip in ntdll (a system call), WARP on the stack, then the pool and the thread start
        assert_eq!(work_module(0x1010, &[0x42, 0x5100, 0x1200, 0x3010], &mods), Some("d3d10warp.dll"));
        // rip in our own code
        assert_eq!(work_module(0x7010, &[0x5100], &mods), Some("DomainScheduler.exe"));
        // idle worker: only plumbing
        assert_eq!(work_module(0x1010, &[0x1200, 0x3010, 0x99], &mods), None);
        assert_eq!(work_module(0, &[], &mods), None);
    }
}
