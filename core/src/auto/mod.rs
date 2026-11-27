//! auto: the one-button Auto mode and what runs with it (ports of AutoController.cs, Top5.cs, BurstGuard.cs,
//! Tuner.cs, Hud.cs, CheckStats.cs).
//!
//! # API overview
//!
//! - [`controller`]: `AutoController::new(Arc<Engine>, Arc<dyn GameProbe>, Option<Arc<dyn IFrameSource>>,
//!   Option<now>)`, shared as `Arc<AutoController>`; every method takes `&self`. Commands: `click`, `tick` (once a
//!   second), `shutdown`, `revert_everything(why)`, `standalone_background(on, game_mask)`, `set_top5(on)`,
//!   `recover_crashed_session(Option<&SessionData>)`. Reads: `state`, `status`, `on`, `button_enabled`, `game_pid`,
//!   `game_exe`, `mask_now`, `baseline_p99`, `applied_at`, `blocked`, `confidence`, `warm_rates`, `plan_view`,
//!   `last_game_running` (no lock), `protected(pid)`, `top5_mode`, `dynamic() -> Arc<Top5>`,
//!   `background_standalone`, `report`, `summary`; static `AutoController::is_system(exe)`. The C# public fields are
//!   setters: `set_save_session`, `set_autostart_line`, `set_self_test_problems`, `set_self_load_line`,
//!   `set_mask_filter` (tests), `set_faceit_allowed` (None = `Guard::faceit_cs2_allowed`), `set_timer_res`,
//!   `set_heat`, `set_soft` / `soft`, `set_burst` / `burst`, `set_watch_list`; the C# `Changed` event is
//!   `on_changed(handler)` (called on the changing thread, the controller may be read again from it).
//!   `GameProbe` is the C# `IGameProbe` with the 1.16.2 `modules_readable` (default true); `NativeProbe` wraps the
//!   native area's `IGameProbe` (`WinGameProbe`). `NoFrames` is the empty `IFrameSource` (the trait itself lives in
//!   `etw::frames`, re-exported here). `AutoState`.
//! - [`top5`]: `Top5Policy` (constants), `Top5Slot`, `Cand`, `Top5Plan` (pure: `set_cores`, `step`, `desired`,
//!   `wait_of`, `moves`, `slot_count`, `events`), and the runner `Top5::new(Arc<Engine>)` with `start`, `set_meta`,
//!   `tick`, `stop`, `dispose`, `view`, `log`, `running`, `exe`, `moves`, `threads`, `needs_meta`, and the hub hooks
//!   `set_wait_of` (ReadyProbe::wait_p99_ms) and `set_irq_shares` (EtwIrq::shares); 1.17.1: `set_drivers`
//!   (EtwIrq shares + graphics-driver cores), `set_all_threads` (setting "AllThreadsIdeal"), `all_view`.
//! - [`all_threads`] (1.17.1): `AllPlan` (pure: `step`, `confirm`, `cancel`, `reset`, `owned`, `tracked`, `events`),
//!   `AllCpus::new(topo, mask)`, `AllThread`, `AllPolicy`: ideal cores for every other game thread, run by `Top5::tick`.
//! - [`burst_guard`]: `BurstPolicy` (constants and pure rules, `never()`), `IBurstNative` (+ `NoBurstNative`,
//!   Windows `burst_native::WinBurstNative`), `BurstGuard::new(Option<native>, Option<now>)` with `tick`, `release`,
//!   `recover_after_crash`, `capped_count` and the C# fields as setters `set_on`, `set_soft`, `set_state_path`,
//!   `set_logical`, `set_log`, `set_game_hard_faults`.
//! - [`tuner`]: `TunerVariant`, `TunerPhase`, `Step`, `AbTuner` (`start`, `tick(now, &mut frames)`, `cancel`,
//!   `measure`, `decide`, `stats`, `table`, getters).
//! - [`hud`]: `FrameStats`, `CoreKind`, `HudMath` (`stats`, `percentile`, `is_hitch`, `buckets`, `align_end`, `nice`,
//!   `chart_top`, `classify`, `elapsed`, `spaced`).
//! - [`check_stats`]: `CheckResult`, `CheckStats::{from, report}`.

pub mod all_threads;
pub mod burst_guard;
#[cfg(windows)]
pub mod burst_native;
pub mod check_stats;
pub mod controller;
pub mod hud;
pub mod top5;
pub mod tuner;

#[cfg(test)]
mod tests;

pub use burst_guard::{BurstGuard, BurstPolicy, IBurstNative, NoBurstNative};
pub use check_stats::{CheckResult, CheckStats};
pub use controller::{AutoController, AutoState, GameProbe, NativeProbe, NoFrames};
pub use crate::etw::frames::IFrameSource;
pub use hud::{CoreKind, FrameStats, HudMath};
pub use all_threads::{AllCpus, AllEvent, AllPlan, AllPolicy, AllReason, AllThread};
pub use top5::{AllView, Cand, Top5, Top5Plan, Top5Policy, Top5Slot};
pub use tuner::{AbTuner, Step, TunerPhase, TunerVariant};
