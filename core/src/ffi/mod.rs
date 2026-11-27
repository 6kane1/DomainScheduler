//! ffi: the C ABI for the C++ app (app/src/dos_core.h) and the parts of the C# app project that are logic:
//! Hub.cs (the long-lived objects and their wiring), App.xaml.cs (start-up, command line, single instance, crash-safe
//! start, clean exit, update hand-over), AutoCommand.cs, UpdateService.cs, Optimizer.cs, Hardware.cs, and what every
//! page decides (pages/).
//!
//! # API overview
//!
//! - [`abi`]: the `extern "C"` functions (`dos_app_start`, `dos_get`, `dos_do`, `dos_start`, `dos_job_poll` ...).
//! - [`app`]: `start(args, &mut show)` (= App.OnStartup), `exit_app`/`clean_up`, `install_update`, `roll_back`,
//!   `factory_reset`, `install_hooks` (the start-up wiring of the other areas: system::deps, IrqAuto's boot guard,
//!   Tables / ThreadIni hooks), `app()` / `hub()` (the running instance).
//! - [`hub::Hub`]: `configure`, `live_tick`, `tuner_*`, `load_presets`, `mask_info`, `gpu_oc_startup`, `self_line` ...
//! - [`optimizer::Optimizer`]: the worker (`queue`, `run`, `run_now`) and the 1 s tick.
//! - [`auto_command::AutoCommand`]: `--auto on|off|status` (`send`) and the running side (`new`, `mirror`).
//! - [`update_service::UpdateService`]: update checks, downloads, the card's data.
//! - [`hardware`]: `detect()` (WMI), the makers' links, `open_link`, `run`.
//! - [`state`]: the state version / event, tray notices, error entries, jobs, the quit request.
//! - [`pages`]: `get` / `do_` / `start` by name (the JSON contract is in dos_core.h).
//! - [`thread_map`]: `dos_tm_open` / `dos_tm_close` / `dos_tm_status` of the native-only Thread map page.
//! - [`win`], [`wmi`]: the Win32 calls of this area (named events, the instance mutex, message box, beep, shell
//!   open, known folders, WMI queries); stand-ins elsewhere.

pub mod abi;
pub mod app;
pub mod auto_command;
pub mod hardware;
pub mod hub;
pub mod optimizer;
pub mod pages;
pub mod state;
pub mod thread_map;
pub mod update_service;
pub mod win;
pub mod wmi;

#[cfg(test)]
mod tests;
