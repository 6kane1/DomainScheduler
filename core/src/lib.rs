//! dos_core: the logic of Domain OS Toolbox, ported from the C# DomainScheduler.Lib with the SAME behaviour.
//! Areas (one folder each, see ../PLAN.md): base (shared types), native (Windows calls behind a trait),
//! topo (processor topology, masks), engine, auto (Auto, Top-5, burst guard), etw (kernel traces),
//! system (settings, store, updater, tweaks, devices), gpu (overclocking), ffi (the C ABI for the C++ app).
pub mod base;
pub mod native;
pub mod topo;
pub mod engine;
pub mod auto;
pub mod etw;
pub mod system;
pub mod gpu;
pub mod ffi;
