//! KWAIT_REASON and KTHREAD_STATE numbers as the kernel writes them into the CSwitch event
//! (OldThreadWaitReason, OldThreadState).
//!
//! Source: phnt `ntkeapi.h` (github.com/winsiderss/phnt, the System Informer headers), which matches the WDK `wdm.h`
//! enum `_KWAIT_REASON` (Windows 10/11: WrIoRing, WrMdlCache and WrRcu were added at the end) and `_KTHREAD_STATE`.
//! Checked 2026-11-27.

pub const EXECUTIVE: u8 = 0;
pub const FREE_PAGE: u8 = 1;
pub const PAGE_IN: u8 = 2;
pub const POOL_ALLOCATION: u8 = 3;
pub const DELAY_EXECUTION: u8 = 4;
pub const SUSPENDED: u8 = 5;
pub const USER_REQUEST: u8 = 6;
pub const WR_EXECUTIVE: u8 = 7;
pub const WR_FREE_PAGE: u8 = 8;
pub const WR_PAGE_IN: u8 = 9;
pub const WR_POOL_ALLOCATION: u8 = 10;
pub const WR_DELAY_EXECUTION: u8 = 11;
pub const WR_SUSPENDED: u8 = 12;
pub const WR_USER_REQUEST: u8 = 13;
pub const WR_EVENT_PAIR: u8 = 14;
pub const WR_QUEUE: u8 = 15;
pub const WR_LPC_RECEIVE: u8 = 16;
pub const WR_LPC_REPLY: u8 = 17;
pub const WR_VIRTUAL_MEMORY: u8 = 18;
pub const WR_PAGE_OUT: u8 = 19;
pub const WR_RENDEZVOUS: u8 = 20;
pub const WR_KEYED_EVENT: u8 = 21;
pub const WR_TERMINATED: u8 = 22;
pub const WR_PROCESS_IN_SWAP: u8 = 23;
pub const WR_CPU_RATE_CONTROL: u8 = 24;
pub const WR_CALLOUT_STACK: u8 = 25;
pub const WR_KERNEL: u8 = 26;
pub const WR_RESOURCE: u8 = 27;
pub const WR_PUSH_LOCK: u8 = 28;
pub const WR_MUTEX: u8 = 29;
pub const WR_QUANTUM_END: u8 = 30;
pub const WR_DISPATCH_INT: u8 = 31;
pub const WR_PREEMPTED: u8 = 32;
pub const WR_YIELD_EXECUTION: u8 = 33;
pub const WR_FAST_MUTEX: u8 = 34;
pub const WR_GUARDED_MUTEX: u8 = 35;
pub const WR_RUNDOWN: u8 = 36;
pub const WR_ALERT_BY_THREAD_ID: u8 = 37;
pub const WR_DEFERRED_PREEMPT: u8 = 38;
pub const WR_PHYSICAL_FAULT: u8 = 39;
pub const WR_IO_RING: u8 = 40;
pub const WR_MDL_CACHE: u8 = 41;
pub const WR_RCU: u8 = 42;

const NAMES: [&str; 43] = [
    "Executive", "FreePage", "PageIn", "PoolAllocation", "DelayExecution", "Suspended", "UserRequest",
    "WrExecutive", "WrFreePage", "WrPageIn", "WrPoolAllocation", "WrDelayExecution", "WrSuspended",
    "WrUserRequest", "WrEventPair", "WrQueue", "WrLpcReceive", "WrLpcReply", "WrVirtualMemory", "WrPageOut",
    "WrRendezvous", "WrKeyedEvent", "WrTerminated", "WrProcessInSwap", "WrCpuRateControl", "WrCalloutStack",
    "WrKernel", "WrResource", "WrPushLock", "WrMutex", "WrQuantumEnd", "WrDispatchInt", "WrPreempted",
    "WrYieldExecution", "WrFastMutex", "WrGuardedMutex", "WrRundown", "WrAlertByThreadId", "WrDeferredPreempt",
    "WrPhysicalFault", "WrIoRing", "WrMdlCache", "WrRcu",
];

/// Name of a wait reason ("WrPushLock"); "Unknown" outside the table.
pub fn reason_name(r: u8) -> &'static str {
    NAMES.get(r as usize).copied().unwrap_or("Unknown")
}

/// Waits on a lock or another synchronization primitive that some other thread holds and releases.
/// - WrResource (ERESOURCE), WrPushLock, WrMutex (KMUTEX), WrFastMutex, WrGuardedMutex: kernel locks.
/// - WrAlertByThreadId: user-mode SRWLOCK, CRITICAL_SECTION, CONDITION_VARIABLE and WaitOnAddress since Windows 8
///   (NtWaitForAlertByThreadId); the releasing thread readies the waiter directly.
/// - WrKeyedEvent: the older keyed-event path of the same user-mode locks.
///
/// Not included: Executive/WrExecutive (the generic reason drivers pass to KeWaitForSingleObject for any event or
/// timer, so most kernel waits would show as "locks"), UserRequest/WrUserRequest (WaitForSingleObject on any handle:
/// events, mutants, semaphores, processes cannot be told apart from the reason alone).
pub fn is_lock(r: u8) -> bool {
    matches!(r, WR_KEYED_EVENT | WR_RESOURCE | WR_PUSH_LOCK | WR_MUTEX | WR_FAST_MUTEX | WR_GUARDED_MUTEX | WR_ALERT_BY_THREAD_ID)
}

/// LPC/ALPC waits: WrLpcReceive (a server waiting for a request), WrLpcReply (a client waiting for the answer).
pub fn is_ipc(r: u8) -> bool {
    matches!(r, WR_LPC_RECEIVE | WR_LPC_REPLY)
}

/// Waits that are a sleep, not waiting for another thread: Sleep()/NtDelayExecution and suspension.
pub fn is_sleep(r: u8) -> bool {
    matches!(r, DELAY_EXECUTION | WR_DELAY_EXECUTION | SUSPENDED | WR_SUSPENDED)
}

/// KTHREAD_STATE (CSwitch OldThreadState).
pub mod state {
    pub const INITIALIZED: u8 = 0;
    pub const READY: u8 = 1;
    pub const RUNNING: u8 = 2;
    pub const STANDBY: u8 = 3;
    pub const TERMINATED: u8 = 4;
    pub const WAITING: u8 = 5;
    pub const TRANSITION: u8 = 6;
    pub const DEFERRED_READY: u8 = 7;
    pub const GATE_WAIT_OBSOLETE: u8 = 8;
    pub const WAITING_FOR_PROCESS_IN_SWAP: u8 = 9;
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn table_matches_constants() {
        assert_eq!(reason_name(WR_PUSH_LOCK), "WrPushLock");
        assert_eq!(reason_name(WR_ALERT_BY_THREAD_ID), "WrAlertByThreadId");
        assert_eq!(reason_name(WR_LPC_REPLY), "WrLpcReply");
        assert_eq!(reason_name(WR_RCU), "WrRcu");
        assert_eq!(reason_name(200), "Unknown");
        assert!(is_lock(WR_MUTEX) && !is_lock(EXECUTIVE) && !is_lock(WR_USER_REQUEST));
        assert!(is_ipc(16) && is_ipc(17) && !is_ipc(15));
        assert!(is_sleep(DELAY_EXECUTION) && !is_sleep(WR_QUEUE));
    }
}
