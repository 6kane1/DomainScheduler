//! topo: the processor model and everything computed from it (ports of TopoBuilder.cs, Masks.cs, CoreSplit.cs,
//! GameMask.cs, Reservation.cs, IrqLayout.cs, the Topo/SplitPlan part of Model.cs, and the test helper Synth.cs).
//!
//! # API overview
//!
//! - [`model`]: `Topo` (fields `sets`, `logical_count`, `physical_count`, `groups`, `reported`, `smt`, `eff_classes`,
//!   `llc_groups`, `l3_by_llc`, `big_cache_llc`, `name`; methods `hybrid()`, `split()`, `full_mask()`, `first_core()`)
//!   and `SplitPlan` (`can_split()`, `is_split_family()`, `label()`, `why()`). Also re-exported from `base`.
//! - [`builder`]: `TopoBuilder::{live, invalidate, parse_cpu_sets, parse_l3, l3_map, detect_big_cache, build, mask,
//!   pop_count}`; `LIVE_HOOK` (= `Topo.LiveHook`). `live()` returns `Option<Arc<Topo>>` (cached 30 s).
//! - [`masks`]: `Masks::{try_parse_spec, to_spec, parse, limit, from_value, resolve, build_spec, ranges,
//!   ideal_candidates, bg_mask, advice, note_text}`, `LimitResult`.
//! - [`core_split`]: `CoreSplit::{detect, validate, smt_whole, text, sku_of, is_core_ultra, bits}`; `X3D_HOOK` (the
//!   engine's `Tables.X3d` plugs in here; without it the embedded tables.json is read).
//! - [`game_mask`]: `GameMask::{mode, set_mode, pinned, invalidate_irq, whole_cores, dedicated, adjust, adjust_wide,
//!   for_live, info, for_rule, cpu_sig, build, rule, manual_rule, background}`, `Edits`; hooks `IRQ_HOOK`,
//!   `IRQ_SOURCE`, `NOISY_HOOK`, `BUSY_HOOK`.
//! - [`reservation`]: `Reservation::{reg, set_reg, boot_utc, backup_path, unmix, mixes, reserved, refusals,
//!   valid_original, import_backup, read, apply_calls, apply_non_game, revert}`, trait `IReservedRegistry`,
//!   `WinReservedRegistry`, `State`, hooks `BOOT_UTC_HOOK`, `BACKUP_PATH_HOOK`; `FakeReserved` for tests.
//! - [`irq_layout`]: `IrqLayout::{ranks, plan}`, `IrqRole`, `IrqSlot`.
//! - `synth` (tests / feature "testsupport"): `symmetric`, `x7950x3d`, `x7950x`, `x7800x3d`, `i12400`, `six`,
//!   `i13900k`, `MB`.

pub mod builder;
pub mod core_split;
pub mod game_mask;
pub mod irq_layout;
pub mod masks;
pub mod model;
pub mod reservation;
#[cfg(any(test, feature = "testsupport"))]
pub mod synth;

pub use builder::TopoBuilder;
pub use core_split::CoreSplit;
pub use game_mask::GameMask;
pub use irq_layout::{IrqLayout, IrqRole, IrqSlot};
pub use masks::Masks;
pub use model::{SplitPlan, Topo};
pub use reservation::Reservation;
