//! base: the shared types and services every area builds on (ports of Model.cs, L.cs, AppLog.cs, Flags.cs,
//! Settings.cs, StateDir.cs, AssemblyInfo.cs). Pure logic; Windows calls go through `crate::native`.
//!
//! # API overview
//!
//! C# static classes are unit structs with associated functions (`L::t`, `AppLog::add`, `Settings::get`,
//! `Flags::open`, `StateDir::ensure`); C# instance data types are plain structs with pub snake_case fields.
//!
//! - [`model`]: `CpuSet`, `L3Cache`, `ModRange`, `MaskSpec`, `MaskResult`, `MaskOption`, `GameMaskInfo`, `ThreadRule`,
//!   `Preset`, `ApplyOptions`, `ApplyReport`, `ThreadRow`, `ThreadSnap`, `ProcBefore`, `ThrBefore`, `ThrPrevE`,
//!   `ThrMetaE`, `ModCacheE`, `Pn`, `IrqConfig`, `PciDevice`, and (re-exported from `topo::model`) `Topo`,
//!   `SplitPlan`. C# keyword-named fields get a trailing underscore: `ThreadRule::match_`, `GameMaskInfo::final_`.
//! - [`l`]: `L::load(lang)`, `L::t(en) -> &str`, `L::n(en)`, `L::f(en, &[Arg])`, the macro
//!   `lf!("text {0}", a, b)` (= `L.F`), `L::lang()`, `L::detect()`, `L::fonts()`, `L::family()`, `L::on_changed(f)`.
//! - [`fmt`]: .NET formatting. `Arg` (a C# `object`: `From` for strings, numbers, bool, `DateTime`),
//!   `format(&Culture, fmt, &[Arg])` (= `string.Format`), `num(v, "0.0", &culture)` / `int(v, "x", &culture)`
//!   (= `v.ToString(fmt, culture)`), `Culture::current()` / `Culture::invariant()`.
//! - [`time`]: `DateTime` (ticks + kind; `utc_now()`, `to_local()`, `to_o()`, `format(pattern, &culture)`,
//!   `parse_iso`, `from_file_time_utc`) and `TimeSpan` (`from_ms`, `from_secs`, `total_ms()` ...): the C# `Now` hooks
//!   (`Func<DateTime>`) become `Arc<dyn Fn() -> DateTime + Send + Sync>`.
//! - [`ci`]: `OrdinalIgnoreCase` helpers: `ci::eq`, `starts_with`, `ends_with`, `contains`, `upper`, `lower`;
//!   `CiMap<V>` / `CiSet` (insertion-ordered, case-insensitive; `ci_get`, `ci_insert`, `ci_add` ...).
//!   Plain C# `Dictionary`/`HashSet` with ordinal keys: use `indexmap::IndexMap` / `IndexSet` where the order of
//!   enumeration matters (C# enumerates in insertion order).
//! - [`app_log`]: `AppLog::add(msg)`, `warn`, `error`, `add_sev(msg, Sev)`, `tail(n)`, `clear()`, `flush()`,
//!   `version()`, `on_error_added(f)`, `describe(&err)`, `describe_panic(payload)`; hooks `NOW_HOOK`, `PATH_HOOK`.
//! - [`flags`]: `Flags::open(key)`, `set(key, open)`, `refuse(key, action)`, the key constants.
//! - [`settings`]: trait `ISettingsBackend` (try_get / set / delete / names), `MemorySettingsBackend`,
//!   `Settings::{get, get_or, get_bool, get_bool_or, get_int, set, set_bool, reset, language, watch, set_watch,
//!   watch_list, try_stored, stored, load_aff_rules, save_aff_rule, is_machine, machine_value_valid, machine_import,
//!   reads, reset_reads, backend, set_backend, scoped_backend}`; `REAPPLY_DEFAULT_HOOK`. The registry backend is
//!   `native::settings_backend::RegistrySettingsBackend` (Windows only; the default backend there).
//! - [`state_dir`]: `StateDir::{ensure, path_only, error, take_notes, judge, canonical}`, and `store_dir()` /
//!   `store_dir_shown()` / `STORE_DIR_HOOK` = C# `Store.Dir` / `Store.DirShown` / `Store.DirHook`.
//! - [`version`]: `INFORMATIONAL_VERSION`, `full()`.
//! - [`hooks`]: `Hook<F>` (a settable static `Arc<F>`), `lock(&mutex)` (poison-free), and `static_hooks()`: the
//!   xunit collection "static-hooks"; every test that sets a static hook or the settings backend holds it.
//! - `testrand` (tests / feature "testsupport"): `Rand` and `iterations()` for the ported fuzz tests.

pub mod app_log;
pub mod ci;
pub mod flags;
pub mod fmt;
pub mod hooks;
pub mod l;
pub mod model;
pub mod settings;
pub mod state_dir;
#[cfg(any(test, feature = "testsupport"))]
pub mod testrand;
pub mod time;
pub mod version;

pub use app_log::{AppLog, Sev};
pub use ci::{CiMap, CiMapExt, CiSet, CiSetExt, CiString};
pub use flags::Flags;
pub use fmt::Arg;
pub use l::L;
pub use model::*;
pub use settings::{ISettingsBackend, MemorySettingsBackend, Settings};
pub use state_dir::StateDir;
pub use time::{DateTime, TimeSpan};
