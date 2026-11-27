//! The shared data types (port of `Model.cs`). Field names are the C# ones in snake_case; C# `int` stays `i32`,
//! `uint` `u32`, `ulong` `u64`, `long` `i64`, `byte` `u8`. A C# `string` that may be null is a `String` ("" for null)
//! unless the difference matters (then `Option`).
//!
//! `Topo` (with its lazily computed `SplitPlan`) is defined in `topo::model` and re-exported here, because it needs
//! `CoreSplit`.

use super::fmt::Arg;
use super::time::DateTime;

pub use crate::topo::model::{SplitPlan, Topo};

/// One entry of GetSystemCpuSetInformation (Type 0).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct CpuSet {
    /// System ID for the CPU Set APIs: NOT the logical processor index.
    pub id: u32,
    pub group: u16,
    pub lp: u8,
    pub core: u8,
    pub llc: u8,
    pub numa: u8,
    pub eff: u8,
    /// bit0 Parked, bit1 Allocated, bit2 ToTarget, bit3 RealTime
    pub flags: u8,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct L3Cache {
    pub size: u64,
    /// (Group, Mask)
    pub masks: Vec<(u16, u64)>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ModRange {
    pub base: u64,
    pub size: u64,
    pub name: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MaskSpec {
    pub base: String,
    pub ccd: i32,
    pub no_smt: bool,
    pub no_core0: bool,
}

impl Default for MaskSpec {
    fn default() -> Self {
        MaskSpec { base: "auto".into(), ccd: -1, no_smt: false, no_core0: false }
    }
}

impl std::fmt::Display for MaskSpec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut s = if self.base == "ccd" { format!("ccd{}", self.ccd) } else { self.base.clone() };
        if self.no_smt {
            s.push_str("+nosmt");
        }
        if self.no_core0 {
            s.push_str("+nocore0");
        }
        f.write_str(&s)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct MaskResult {
    pub name: String,
    pub hex: String,
    pub lps: String,
    pub spec: String,
    pub mask: u64,
    pub set_ids: Vec<u32>,
    pub count: i32,
    pub domains: i32,
    pub portable: bool,
    pub satisfied: bool,
    pub notes: Vec<String>,
}

impl Default for MaskResult {
    fn default() -> Self {
        MaskResult {
            name: String::new(),
            hex: String::new(),
            lps: String::new(),
            spec: String::new(),
            mask: 0,
            set_ids: Vec::new(),
            count: 0,
            domains: 0,
            portable: true,
            satisfied: true,
            notes: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MaskOption {
    pub key: String,
    pub label: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct GameMaskInfo {
    /// C# null when the info was not built from a rule (ForRule, ForLive): "" here.
    pub base_rule: String,
    pub mode: String,
    pub base: u64,
    pub add: u64,
    pub drop: u64,
    pub manual: u64,
    pub final_: u64,
    pub irq: u64,
    pub irq_drop: u64,
    pub irq_ignored: bool,
    pub edits_stale: bool,
    pub notes: Vec<String>,
    pub irq_devices: Vec<(String, u64)>,
    pub irq_wide: Vec<(String, u64)>,
}

impl GameMaskInfo {
    /// Computed, not a field.
    pub fn adjusted(&self) -> bool {
        self.final_ != self.base
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ThreadRule {
    pub match_: String,
    pub pattern: String,
    pub prio: String,
    pub ideal: String,
    pub boost: String,
    pub tid: String,
    pub enabled: bool,
    /// Section 22.4: after the ideal processor the thread is also limited to that processor's CPU Set.
    pub pin: bool,
}

impl Default for ThreadRule {
    fn default() -> Self {
        ThreadRule {
            match_: "module".into(),
            pattern: String::new(),
            prio: String::new(),
            ideal: String::new(),
            boost: String::new(),
            tid: String::new(),
            enabled: true,
            pin: false,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Preset {
    pub exe: String,
    pub class: String,
    pub io: String,
    pub mem: String,
    pub eco: String,
    pub boost: String,
    pub enabled: bool,
    pub builtin: bool,
    /// Heat policy (section 22.6): "pin" | "ideal" | "none" for the hottest thread and for the next ones.
    pub hot_first: String,
    pub hot_others: String,
    pub threads: Vec<ThreadRule>,
}

impl Default for Preset {
    fn default() -> Self {
        Preset {
            exe: String::new(),
            class: "normal".into(),
            io: "normal".into(),
            mem: "normal".into(),
            eco: "off".into(),
            boost: String::new(),
            enabled: true,
            builtin: false,
            hot_first: "none".into(),
            hot_others: "none".into(),
            threads: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ApplyOptions {
    /// C# null by default: "" here (Masks.Resolve treats both the same).
    pub mask_name: String,
    pub bg_mask_name: String,
    pub do_mask: bool,
    pub do_threads: bool,
    pub do_background: bool,
    pub bg_affinity: bool,
    pub do_aff_rules: bool,
    pub warmup: bool,
    pub sys: bool,
    pub game: bool,
    pub allow_elevated_priority: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ApplyReport {
    pub pid: i32,
    pub handle_level: i32,
    pub named_hits: i32,
    pub last_threads: i32,
    pub exe: String,
    pub mask_hex: String,
    pub mask_lps: String,
    pub cycle_fallback_tids: Vec<u32>,
    pub failures: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ThreadRow {
    pub tid: u32,
    pub rate: u64,
    pub prio: i32,
    pub sym: String,
    pub desc: String,
    pub born: i64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ThreadSnap {
    pub ok: bool,
    pub level: i32,
    pub rows: Vec<ThreadRow>,
    pub cycle_fallback_tids: Vec<u32>,
}

/// The process state before the program changed it (internal to the engine in C#).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ProcBefore {
    pub prio: u32,
    pub aff: u64,
    pub io: u32,
    pub mem: u32,
    pub eco_ctl: u32,
    pub eco_state: u32,
    pub boost_disabled: bool,
    pub sets: Vec<u32>,
    pub created: i64,
    pub have: bool,
    pub prio_set: bool,
    pub aff_set: bool,
    pub sets_set: bool,
    pub io_set: bool,
    pub mem_set: bool,
    pub eco_set: bool,
    pub boost_set: bool,
}

/// The thread state before the program changed it (internal to the engine in C#).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ThrBefore {
    pub prio: i32,
    pub boost_disabled: bool,
    pub prio_set: bool,
    pub boost_set: bool,
    pub sets_set: bool,
    pub ideal_set: bool,
    /// The ideal processor before the dynamic mode moved the thread.
    pub ideal: u32,
    /// The thread's selected CPU Sets before the program touched them.
    pub sets: Vec<u32>,
    pub born: i64,
    /// 1.17.1: only the ideal processor was taken (the all-threads layer of Dynamic Top-5): priority, boost and CPU
    /// Sets were never read nor changed, and the ideal processor goes back only while it is still the value the
    /// program set.
    pub ideal_only: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ThrPrevE {
    pub c: u64,
    pub t: DateTime,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ThrMetaE {
    pub sym: String,
    pub desc: String,
    pub born: i64,
    pub seen: DateTime,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ModCacheE {
    pub m: Vec<ModRange>,
    pub t: DateTime,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Pn {
    pub id: i32,
    pub name: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct IrqConfig {
    pub msi: bool,
    pub msg_limit: i32,
    pub assign: Option<u64>,
    pub priority: Option<i32>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PciDevice {
    pub name: String,
    pub pdo: String,
    pub service: String,
    pub driver_key: String,
    /// Display, USB, Net, ...
    pub class: String,
    pub dev_inst: u32,
    pub location: String,
}

/// Formats a C# `object[]` argument list for the split texts (kept as `Arg` so numbers format in the culture).
pub type Args = Vec<Arg>;
