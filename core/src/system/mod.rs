//! system: the program's own state and the system changes it makes by hand (ports of Store.cs, Updater.cs,
//! Autostart.cs, BootGuard.cs, LegacyImport.cs, Tweaks.cs, Devices.cs, Maintenance.cs).
//!
//! # API overview
//!
//! - [`store`]: `Store::{dir, dir_shown, legacy_dir, profiles_dir, presets_path, save, load -> Loaded, mark_builtin,
//!   same, save_profile, copy_profile, delete_profile, sanitize, list_profiles, profile_path, load_calls,
//!   list_profiles_calls}`.
//! - [`updater`]: `AppVersion` (full, channel, channel_for, is_test, display, describe, override_full), `TesterKey`
//!   (current, parse, format, open, seal; `SealError`), `UpdateVersion` (parse -> `V`, compare), `UpdateFile`,
//!   `UpdateFeed`, `UpdateCheck`, `TrustedKey`, `UpdError`, `Updater` (consts FEED_BASE, GITHUB_API, PUBLIC_KEY,
//!   BACKUP_PUBLIC_KEY, REVOKED_KEY_IDS, MAX_FEED_BYTES, MAX_DOWNLOAD_BYTES; trusted_keys, key_id, verify, verify_any,
//!   verify_with, dir, version_rx, installed_playbook, parse_feed, choose, check, key_refused, try_key, split_signed,
//!   sha256, sha256_file, download_bytes, read_all, check_bytes, safe_file_name, write_new, swap, swap_back,
//!   swap_to_previous, undo_swap_to_previous; hook `RETRY_SLEEP`). The C# async methods are blocking here.
//! - [`http`]: the WinHTTP client of the updater (`Client`, `HttpError`, `Transport`, test hook `TRANSPORT_HOOK`).
//! - [`autostart`]: `ITaskBackend` (read/register/delete), `ComTaskBackend` (Windows, Task Scheduler COM),
//!   `SchtasksBackend`, `MemTasks` (tests), `Autostart::{TASK_NAME, ARG, xml, task_exists, task_enabled, task_create,
//!   task_delete, ensure_on_first_run, report_line, set_primary, set_fallback}`, hook `USER_HOOK`.
//! - [`boot_guard`]: `BootGuard::{CONFIRM_UPTIME, FILE_NAME, file_path, same_boot, decide, can_confirm,
//!   restart_pending, home_state, pending_applied_utc, load, clear, mark_pending, on_start, confirm_if_due,
//!   unexpected_shutdown_since}`, `State`, `Verdict`, `HomeDevices`, hooks `PATH_HOOK`, `CRASH_HOOK`.
//! - [`legacy_import`]: `LegacyImport::run(legacy_dir, account) -> Vec<(String, Sev)>`.
//! - [`tweaks`]: `IRegStore`, `RegError`, `WinRegStore` (Windows; bcdedit for the "BCD" pseudo hive),
//!   `MemoryRegStore`, `RegChange`, `Tweak`, `TweakState`, `Tweaks::{catalog, withdrawn, retired, pin_release,
//!   revert_retired, state, acceptable, import_backup, has_backup, apply, revert, backup_path, reg, set_reg,
//!   BACKUP_NAME}`, hook `BACKUP_PATH_HOOK`.
//! - [`devices`]: `IrqKind`, `IrqResource`, `DeviceFacts`, `Devices::{BACKUP_NAME, backup_path, has_backup, list,
//!   list_pci_names, read_irq, pinned_irqs, allocated, resources, parse_irq, resource, state_path, facts,
//!   valid_backup_device, import_backup, write, write_with, write_calls, is_storage, revert_all}`, hooks
//!   `LIST_HOOK`, `ALLOC_HOOK`, `RESOURCE_HOOK`, `READ_HOOK`, `STATE_PATH_HOOK`.
//! - [`maintenance`]: `Maintenance::factory_reset(restore_crashed)`.
//! - [`deps`]: hooks to the parts of other areas that LegacyImport, Devices and Maintenance call (IrqAuto, GamePower,
//!   DefenderGameExclusion, the session, GpuProfiles, Tables). The `ffi` area sets them at start (see deps.rs).

pub mod autostart;
pub mod boot_guard;
pub mod deps;
pub mod devices;
pub mod http;
pub mod legacy_import;
pub mod maintenance;
pub mod store;
pub mod tweaks;
pub mod updater;
pub mod util;

pub use autostart::{Autostart, ITaskBackend};
pub use boot_guard::BootGuard;
pub use devices::Devices;
pub use legacy_import::LegacyImport;
pub use maintenance::Maintenance;
pub use store::Store;
pub use tweaks::{Tweak, TweakState, Tweaks};
pub use updater::{AppVersion, TesterKey, UpdateCheck, UpdateFeed, UpdateFile, UpdateVersion, Updater};
