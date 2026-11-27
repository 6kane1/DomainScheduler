//! The calls LegacyImport, Devices and Maintenance make into other areas (C#: direct static calls into `IrqAuto`,
//! `GamePower`, `DefenderGameExclusion`, `Engine`/`SessionFile`, `GpuProfiles`, `Tables`). Those areas are ported by
//! other agents, so the system area reaches them through these hooks; the `ffi` area sets every hook at start, before
//! `LegacyImport::run`, `BootGuard::on_start` and the first `Devices`/`Maintenance` call (App.xaml.cs order).
//!
//! A hook that is not set means "that part is not there": LegacyImport leaves the file untouched (it is read again at
//! the next start, nothing is lost), Devices skips the extra revert, Maintenance skips that restore.
//!
//! The pure checks Devices needs from IrqAuto (`InstanceRx`, `ValidIrqValue`, `IsStorage`) and `IrqAuto.HasBackup` are
//! private copies in devices.rs (same text as IrqAuto.cs), so they need no hook.

use crate::base::hooks::Hook;

/// An importer of LegacyImport: (old text, text already in the state folder or None, dropped entries) -> the new
/// text, None for "nothing to take over", Err for "not taken over" (the C# exception message).
pub type ImportFn = dyn Fn(&str, Option<&str>, &mut Vec<String>) -> Result<Option<String>, String> + Send + Sync;

/// `IrqAuto.ImportBackup(text, existing, dropped)` (irq-auto-backup.json).
pub static IRQ_AUTO_IMPORT_BACKUP: Hook<ImportFn> = Hook::new();
/// `GamePower.ImportFile(text, dropped)` (game-power.json; called only when the state folder has none).
pub static GAME_POWER_IMPORT_FILE: Hook<dyn Fn(&str, &mut Vec<String>) -> Result<Option<String>, String> + Send + Sync> = Hook::new();
/// `DefenderGameExclusion.IsSafePath(line)`.
pub static DEFENDER_IS_SAFE_PATH: Hook<dyn Fn(&str) -> bool + Send + Sync> = Hook::new();
/// `LegacyImport.ImportSession` (session.json). Its C# body, for the engine/ffi owner:
/// existing != null -> null; s = Deserialize<SessionData>(text); s == null || s.CleanShutdown -> null; keep only
/// `Engine.SessionProcValid` procs and `Engine.SessionThreadValid` threads; dropped gets L.F("{0} processes", n) /
/// L.F("{0} threads", n) for what was removed; returns Serialize(s).
pub static SESSION_IMPORT: Hook<ImportFn> = Hook::new();
/// `LegacyImport.ImportGpu` (gpu-profiles.json). Its C# body, for the gpu/ffi owner: existing != null -> null; the text
/// is written to a scratch file in the state folder ("import-<guid>.tmp") and read with `GpuProfiles.Load` (names
/// cleaned, voltage clamped, duplicates dropped), the scratch file deleted; no profiles -> null; AtStart != "" ->
/// Confirm = AtStart, AtStart = ""; returns Serialize(g, WriteIndented).
pub static GPU_IMPORT: Hook<ImportFn> = Hook::new();
/// `Tables.Merge(new Tables.Data(), text)` (tables.json): Err when the text is refused.
pub static TABLES_CHECK: Hook<dyn Fn(&str) -> Result<(), String> + Send + Sync> = Hook::new();
/// `IrqAuto.RevertExtras()` (part of "Restore all devices"; errors are swallowed by the caller).
pub static IRQ_AUTO_REVERT_EXTRAS: Hook<dyn Fn() -> Result<(), String> + Send + Sync> = Hook::new();
/// `GamePower.Restore()`: a log line ("" for nothing), Err = the exception message.
pub static GAME_POWER_RESTORE: Hook<dyn Fn() -> Result<String, String> + Send + Sync> = Hook::new();
/// `DefenderGameExclusion.RemoveAll()`: a log line ("" for nothing), Err = the exception message.
pub static DEFENDER_REMOVE_ALL: Hook<dyn Fn() -> Result<String, String> + Send + Sync> = Hook::new();

/// The file names of the other areas' state files (the same constants as in C#).
pub mod names {
    /// `IrqAuto.BackupName`
    pub const IRQ_AUTO_BACKUP: &str = "irq-auto-backup.json";
    /// `GamePower.FileName`
    pub const GAME_POWER: &str = "game-power.json";
    /// `DefenderGameExclusion.FileName`
    pub const DEFENDER_EXCLUSIONS: &str = "defender-exclusions.txt";
    /// `GpuProfiles.FileName`
    pub const GPU_PROFILES: &str = "gpu-profiles.json";
}
