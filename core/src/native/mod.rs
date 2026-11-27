//! native: every Windows call of the logic (ports of INative.cs, WinNative.cs, Native.cs, NativeHome.cs, SysExe.cs,
//! SelfThreads.cs, WinProbe.cs, the registry part of Settings.cs and the Windows part of StateDir.cs).
//!
//! # API overview
//!
//! - [`Handle`]: a Windows handle (C# `IntPtr`), `Handle::NULL`, `is_null()`.
//! - [`INative`]: the calls the engine makes, the same methods as `INative.cs` in snake_case. C# `out` parameters
//!   stay out parameters (`&mut`), so a port reads like the C# code and gets the same values on failure
//!   (e.g. `get_mem` leaves 5). `Send + Sync`; the product implementation is [`win_native::WinNative`] (Windows),
//!   the test one is [`fake::FakeNative`] (`cfg(test)` / feature "testsupport", port of `FakeNative.cs`).
//! - [`IGameProbe`]: the probe AutoController uses to find the game (defined in `AutoController.cs` in C#; it lives
//!   here because its product implementation, `win_probe::WinGameProbe`, is a Windows one).
//! - [`sys`]: process-wide facts that exist on every platform (Linux gives neutral values for the tests):
//!   `cpu_set_buffer()`, `cache_buffer()`, `cpu_name()`, `active_lp_count()`, `tick_count64()`,
//!   `current_thread_id()`, `current_process_id()`, `utc_offset_ticks()`, `current_culture()`,
//!   `ui_language_two_letter()`, `common_app_data()`, `local_app_data()`, `system_directory()`, `harden_dll_search()`.
//! - [`win`] (Windows): the static `Native` helpers (`open_process_graceful`, `open_thread_of`,
//!   `enable_debug_privilege`, `image_name`, `image_path`, `modules`, `is_protected`, `threads_by_pid`, `processes`,
//!   `thread_description`, `process_created`, `get_process_times` ...), the access constants, the ntdll imports,
//!   `wide()` / `from_wide()`.
//! - [`reg`] (Windows): a small safe registry API (`Key::open`, `Key::create`, `get`, `get_string`, `set`,
//!   `delete_value`, `value_names`, `subkey_names`, `RegValue`) for every area that reads or writes the registry.
//! - [`settings_backend`] (Windows): `RegistrySettingsBackend` (HKCU\Software\DomainScheduler and
//!   HKLM\SOFTWARE\Domain OS\Toolbox), `migrate_to_machine`, `migrate_from_legacy`.
//! - `acl` (Windows): the permission check of the state folder (`StateDir::ensure` calls it).
//! - [`native_home`]: `NativeHome` (`should_relaunch`, `waits_for_child`, `relaunch_if_needed`, `clean_old`).
//! - [`sys_exe`]: `SysExe` (`system32`, `power_shell_path`, `modules_path`, `unsafe_var`, `start`, `power_shell`) and
//!   `ProcessStartInfo` (file, arguments, the full environment block; `command()` gives a `std::process::Command`).
//! - [`self_threads`]: `SelfThreads` (`register`, `ended`, `now`, `shares`, `others_shares`, `sample_tick`, `work_shares`).
//! - [`thread_sample`]: what an unnamed thread of this process runs (`sample`, `work_module`), for the self-check.

pub mod native_home;
pub mod self_threads;
pub mod sys;
pub mod sys_exe;
pub mod thread_sample;

#[cfg(windows)]
pub mod acl;
#[cfg(windows)]
pub mod reg;
#[cfg(windows)]
pub mod settings_backend;
#[cfg(windows)]
pub mod win;
#[cfg(windows)]
pub mod win_native;
#[cfg(windows)]
pub mod win_probe;

#[cfg(any(test, feature = "testsupport"))]
pub mod fake;

use crate::base::model::ModRange;
use indexmap::{IndexMap, IndexSet};

/// A Windows handle (C# `IntPtr`). The fake hands out small numbers.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Handle(pub isize);

impl Handle {
    pub const NULL: Handle = Handle(0);
    pub fn is_null(self) -> bool {
        self.0 == 0
    }
}

/// The Windows calls the engine makes (WinNative in the product, a fake in tests).
pub trait INative: Send + Sync {
    fn enable_debug_privilege(&self) -> bool;
    fn open_process_graceful(&self, pid: u32, level: &mut i32) -> Handle;
    fn process_created(&self, process: Handle) -> i64;
    fn is_protected(&self, process: Handle) -> bool;
    fn close(&self, handle: Handle);

    fn get_priority_class(&self, process: Handle, cls: &mut u32) -> bool;
    fn set_priority_class(&self, process: Handle, cls: u32) -> bool;
    fn get_affinity(&self, process: Handle, mask: &mut u64) -> bool;
    fn set_affinity(&self, process: Handle, mask: u64) -> bool;
    /// Empty: none.
    fn get_cpu_sets(&self, process: Handle, ids: &mut Vec<u32>) -> bool;
    /// Empty: clear.
    fn set_cpu_sets(&self, process: Handle, ids: &[u32]) -> bool;
    fn get_io(&self, process: Handle, value: &mut u32) -> bool;
    fn set_io(&self, process: Handle, value: u32) -> bool;
    fn get_mem(&self, process: Handle, value: &mut u32) -> bool;
    fn set_mem(&self, process: Handle, value: u32) -> bool;
    fn get_eco(&self, process: Handle, control: &mut u32, state: &mut u32) -> bool;
    fn set_eco(&self, process: Handle, control: u32, state: u32) -> bool;
    fn get_boost_disabled(&self, process: Handle, disabled: &mut bool) -> bool;
    fn set_boost_disabled(&self, process: Handle, disabled: bool) -> bool;
    fn modules(&self, process: Handle) -> Vec<ModRange>;

    fn open_thread_of(&self, pid: u32, tid: u32) -> Handle;
    fn thread_cycles(&self, thread: Handle, cycles: &mut u64) -> bool;
    fn thread_times(&self, thread: Handle, created: &mut i64, kernel_user: &mut u64) -> bool;
    fn thread_start(&self, thread: Handle) -> u64;
    fn thread_desc(&self, thread: Handle) -> String;
    fn get_thread_prio(&self, thread: Handle, prio: &mut i32) -> bool;
    fn set_thread_prio(&self, thread: Handle, prio: i32) -> bool;
    fn get_thread_boost_disabled(&self, thread: Handle, disabled: &mut bool) -> bool;
    fn set_thread_boost_disabled(&self, thread: Handle, disabled: bool) -> bool;
    fn set_thread_ideal(&self, thread: Handle, lp: u32) -> bool;
    fn get_thread_ideal(&self, thread: Handle, lp: &mut u32) -> bool;
    /// Per logical processor (group 0): idle and total time in 100 ns units since boot.
    fn cpu_times(&self, idle: &mut Vec<i64>, total: &mut Vec<i64>) -> bool;
    /// Empty: none.
    fn get_thread_cpu_sets(&self, thread: Handle, ids: &mut Vec<u32>) -> bool;
    /// Empty: clear.
    fn set_thread_cpu_sets(&self, thread: Handle, ids: &[u32]) -> bool;

    /// System timer resolution in 100 ns units: the coarsest, the finest and the current one.
    fn timer_query(&self, coarsest: &mut u32, finest: &mut u32, current: &mut u32) -> bool;
    /// Requests (set = true) or releases (set = false) a timer resolution for this process.
    fn timer_set(&self, resolution: u32, set: bool, actual: &mut u32) -> bool;

    /// Every thread of every process (one toolhelp snapshot), in snapshot order.
    fn threads_by_pid(&self) -> IndexMap<u32, Vec<u32>>;
    /// Every process: image name and parent, in snapshot order.
    fn processes(&self) -> IndexMap<u32, (String, u32)>;
    /// The last Win32 error of the calling thread.
    fn last_error(&self) -> i32;
}

/// The game probe of AutoController (C# `IGameProbe`).
pub trait IGameProbe: Send + Sync {
    fn self_pid(&self) -> u32;
    fn foreground_pid(&self) -> u32;
    /// Processes owning a visible, unowned top-level window (a main window).
    fn window_pids(&self) -> IndexSet<u32>;
    /// d3d11/d3d12/vulkan-1/opengl32 is loaded (opens the process for reading).
    fn has_graphics_module(&self, pid: u32) -> bool;
    fn cpu_time(&self, pid: u32, created: &mut i64, cpu100ns: &mut i64) -> bool;
    fn session_of(&self, pid: u32) -> u32;
}
