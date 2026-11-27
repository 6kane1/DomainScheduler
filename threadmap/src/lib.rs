//! Thread map (Domain OS Toolbox, Advanced page): a graph of processes that expand into their threads, with the
//! links between them (who wakes whom, who takes whose core, who waits on whose lock, who calls whom), over a 30 s
//! history the user can scrub.
//!
//! Pure logic, no Windows calls. The etw area records the NT kernel logger (CSwitch, ReadyThread, process/thread
//! start/end/rundown) and feeds the structs of `events`; the app reads JSON snapshots and layout positions through
//! the C ABI in `ffi` (header `threadmap.h`).
//!
//! Flow: `ThreadMap::push_*` (any thread order across cpus) -> `pump()` (sorts inside the reorder window, updates
//! thread states, derives links, fills the 100 ms buckets) -> `snapshot(&Query)` (sums a bucket range, aggregates,
//! filters) -> `Layout::sync` + `Layout::step(dt)` (positions by node id).

pub mod events;
pub mod ffi;
pub mod fx;
pub mod graph;
pub mod history;
pub mod kwait;
pub mod layout;
pub mod map;

pub use events::{CSwitch, LinkEvent, LinkKind, ProcessInfo, ReadyThread, ThreadInfo, READIER_UNKNOWN, READY_FLAG_DPC};
pub use graph::{Cut, Link, Member, Node, Query, Snapshot, Window, ID_GROUP, ID_PROCESS, ID_THREAD};
pub use history::{BUCKETS, BUCKET_MS};
pub use layout::{Layout, Pos};
pub use map::{Config, RunState, Series, Stats, ThreadMap, PID_KERNEL, PID_UNKNOWN};
