//! engine: the layer that changes processes and threads, and its rules (ports of Engine.cs, SysLayer.cs,
//! ThreadIni.cs, AutoLogic.cs, Guard.cs, Tables.cs, Session.cs, GamePower.cs, SoftClose.cs, BackgroundDeny.cs,
//! DefenderGameExclusion.cs).
//!
//! # API overview
//!
//! - [`engine`]: `Engine::new(Arc<dyn INative>)`; C# public fields are setters (`set_now`, `set_sleep`,
//!   `set_before_mutate`, `set_gate_hook`, `set_timer_guard`) and getters (`advice()`, `last_reports()`,
//!   `last_snap()`, `last_denied_share()`, `manual_calls()`, `set_calls()`, `reverts()`, `native()`). Methods in
//!   snake_case: `begin_pass`, `end_pass`, `name_of`, `processes_now`, `snapshot`, `apply`, `apply_auto`,
//!   `apply_all(&mut pick, aff_rules)`, `threads_now`, `gate`, `place_thread`, `release_thread`, `thread_pins`,
//!   `timer_request`, `timer_release`, `timer_requested`, `read_process`, `changed_pids`, `drop_pins`, `ideals`,
//!   `thread_manual`, `process_class_manual`, `process_boost`, `apply_background`, `revert`, `revert_all`,
//!   `revert_background`, `pending`, `is_background`, `background_count`, `prune`, `sizes`, `export`,
//!   `restore_session`; statics `prio/tprio/iop/mem` (the C# tables), `module_of`, `is_denied_module`,
//!   `thread_eligible`, `rule_hits`, `filter_tid`, `match_for`, `wildcard`, `keeps_timer`, `session_proc_valid`,
//!   `session_thread_valid`. Types `Target`, `ProcInfo`, `Wildcard`, `ReGate` (a re-entrant lock like C# `lock`).
//! - [`sys_layer`]: `SysLayer::new(Arc<Engine>)`, `configure`, `invalidate`, `pass(skip)`, `last_targets`, `passes`.
//! - [`thread_ini`]: `ThreadIni::{load_bundled, parse, self_test, self_test_of, exe_of, valid_prio, install_hooks}`,
//!   `IniResult`, `Row`, `TEXT_HOOK`.
//! - [`auto_logic`]: `AutoPolicy` (constants), `ThreadRole`, `Classified`, `MaskPlan`, `AutoLogic::{pick_mask,
//!   domains, has_single_domain, best_single_domain, harden, keeps_all_cores, all_cores, build_mask, total_rate,
//!   hot_threshold, classify, confidence, plan, role_prio, build_auto_preset, composition_change}`.
//! - [`guard`]: `Guard::{gate, gate_with, eligibility, is_denied, is_critical, is_anti_cheat, is_anti_cheat_process,
//!   faceit_running, faceit_cs2_allowed, set_faceit_cs2_allowed}`, `Op`, `NO_RUNNING`.
//! - [`tables`]: `Tables::{current, x3d, known_games, reload, load, merge, override_path, install_hooks}`, `Data`,
//!   `EMBEDDED_HOOK`, `OVERRIDE_PATH_HOOK`.
//! - [`session`]: `SessionProc`, `SessionThread`, `SessionData`, `SessionFile::{file_path, write, read, delete,
//!   to_json, from_json}`, `PATH_HOOK`.
//! - [`game_power`]: `GamePower::{valid_value, import_file, applied, restore}`, `Guid`.
//! - [`soft_close`]: `SoftClose::new()`, setters `set_on`, `set_list`, `set_close_window`, `set_now`; `begin`,
//!   `tick`, `asked`.
//! - [`background_deny`]: `BackgroundDeny::{contains, all}`.
//! - [`defender`]: `DefenderGameExclusion::{is_safe_path, remove_all, COMMAND, FILE_NAME}`.
//!
//! Start-up wiring (done by ffi, as C# does it implicitly): `Tables::install_hooks()` (CoreSplit reads the merged
//! X3D table) and `ThreadIni::install_hooks()` (the ReapplyMs default).

pub mod auto_logic;
pub mod background_deny;
pub mod defender;
#[allow(clippy::module_inception)]
pub mod engine;
pub mod game_power;
pub mod guard;
pub mod session;
pub mod soft_close;
pub mod sys_layer;
pub mod tables;
pub mod thread_ini;

pub use auto_logic::{AutoLogic, AutoPolicy, Classified, MaskPlan, ThreadRole};
pub use background_deny::BackgroundDeny;
pub use defender::DefenderGameExclusion;
pub use engine::{Engine, ProcInfo, Target};
pub use game_power::GamePower;
pub use guard::Guard;
pub use session::{SessionData, SessionFile, SessionProc, SessionThread};
pub use soft_close::SoftClose;
pub use sys_layer::SysLayer;
pub use tables::Tables;
pub use thread_ini::ThreadIni;
