//! Input events. They mirror the NT kernel logger events Toolbox already records; the etw area decodes the raw
//! records and fills these. The `#[repr(C)]` ones are passed through the C ABI as they are (see threadmap.h).
//!
//! Timestamps are raw QPC ticks (EVENT_RECORD.EventHeader.TimeStamp with the session's QPC clock); the frequency is
//! given once in `Config::qpc_freq`.

/// Thread/CSwitch (opcode 36, class CSwitch v4). Field names follow the MOF class.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CSwitch {
    pub ts: i64,
    /// Processor number of the switch (EVENT_RECORD BufferContext.ProcessorIndex).
    pub cpu: u32,
    pub new_tid: u32,
    pub old_tid: u32,
    pub new_priority: i8,
    pub old_priority: i8,
    /// KWAIT_REASON, see `kwait`.
    pub old_wait_reason: u8,
    /// 0 KernelMode, 1 UserMode.
    pub old_wait_mode: u8,
    /// KTHREAD_STATE, see `kwait::state`.
    pub old_state: u8,
    pub reserved: [u8; 3],
    /// NewThreadWaitTime (ticks of the scheduler clock; kept for the UI, not used by the logic).
    pub new_wait_time: u32,
    pub reserved2: u32,
}

/// `ReadyThread::readier_tid` when the etw area does not know the readying thread: the thread running on `cpu`
/// (tracked from CSwitch) is used, like WPA's "Readying Thread".
pub const READIER_UNKNOWN: u32 = u32::MAX;
/// ReadyThread Flag 0x1: readied from a DPC (Microsoft Learn, ReadyThread class). The thread on the cpu is only the
/// interrupted one, so the readier becomes the "DPC/ISR" node.
pub const READY_FLAG_DPC: u8 = 0x1;

/// Thread/ReadyThread (opcode 50, class ReadyThread: TThreadId, AdjustReason, AdjustIncrement, Flag).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ReadyThread {
    pub ts: i64,
    pub cpu: u32,
    /// The thread that made the target ready: the event header's ThreadId when the etw area trusts it, else
    /// `READIER_UNKNOWN`. 0 (the idle thread) means kernel work on an idle cpu (timer, DPC) and goes to "DPC/ISR".
    pub readier_tid: u32,
    /// TThreadId.
    pub target_tid: u32,
    pub adjust_reason: i8,
    pub adjust_increment: i8,
    /// Flag (READY_FLAG_DPC = 0x1, 0x2 kernel stack swapped out, 0x4 process swapped out).
    pub flags: u8,
    pub reserved: u8,
}

/// Link kinds. The number is stable (C ABI, JSON uses the name).
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum LinkKind {
    /// readier -> target: A made B ready (passes work to B).
    Wake = 0,
    /// attacker -> victim: A held the cpu B needed.
    Preempt = 1,
    /// waiter -> owner: B waited on a lock and A released it.
    Lock = 2,
    /// client -> server: LPC/ALPC, pipe, file request.
    Ipc = 3,
}

pub const LINK_KINDS: usize = 4;

impl LinkKind {
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(Self::Wake),
            1 => Some(Self::Preempt),
            2 => Some(Self::Lock),
            3 => Some(Self::Ipc),
            _ => None,
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Self::Wake => "wake",
            Self::Preempt => "preempt",
            Self::Lock => "lock",
            Self::Ipc => "ipc",
        }
    }
}

/// Where a link came from (kept as a bit mask per link; shown by the UI as a hint).
pub mod via {
    /// Derived from CSwitch/ReadyThread (wake, preempt, lock).
    pub const SCHED: u8 = 0;
    /// Derived from an LPC wait reason (WrLpcReceive/WrLpcReply).
    pub const LPC_WAIT: u8 = 1;
    pub const ALPC: u8 = 2;
    pub const PIPE: u8 = 3;
    pub const FILE: u8 = 4;
    pub const OTHER: u8 = 5;
    pub const NAMES: [&str; 6] = ["sched", "lpc-wait", "alpc", "pipe", "file", "other"];
}

/// A link that another event source already resolved to two threads: ALPC send/receive pairs (client tid, server
/// tid), named pipe or file requests between processes. New sources only need a new `via` value.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LinkEvent {
    pub ts: i64,
    pub from_tid: u32,
    pub to_tid: u32,
    /// LinkKind as u8 (normally Ipc for these sources).
    pub kind: u8,
    /// `via::*`.
    pub via: u8,
    pub reserved: [u8; 2],
    /// How many messages this event stands for (0 counts as 1).
    pub count: u32,
}

/// Process/Start or DCStart (ImageFileName).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ProcessInfo {
    pub ts: i64,
    pub pid: u32,
    pub image: String,
}

/// Thread/Start or DCStart, plus what the etw area resolves: SetThreadDescription name and start module+offset.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ThreadInfo {
    pub ts: i64,
    pub tid: u32,
    pub pid: u32,
    /// SetThreadDescription name ("MainThread", "Worker 3").
    pub description: Option<String>,
    /// Module of Win32StartAddr ("engine2.dll") and the offset inside it.
    pub start_module: Option<String>,
    pub start_offset: u64,
}
