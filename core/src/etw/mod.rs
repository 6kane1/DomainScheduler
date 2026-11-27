//! etw: Windows' own kernel and DXGI traces, read-only (ports of EtwIrq.cs, EtwFrames.cs, ReadyProbe.cs, BlackBox.cs,
//! CheckTrace.cs), and the automatic interrupt layout of the devices (IrqAuto.cs).
//!
//! # API overview
//!
//! Every session is a private real-time session on the QPC clock, started and stopped through the raw ETW API in
//! [`raw`] (Windows; on other platforms every call fails with error 50, so the logic is tested on Linux). The event
//! callbacks only copy a few fields into the object's state under one lock; the analysis is pure.
//!
//! - [`irq::EtwIrq`]: live DPC/ISR per processor. `start()`, `pause()`, `dispose()`, `available()`, `error()`,
//!   `shares()`, `noisy()` (for `topo::game_mask::NOISY_HOOK`), `busy_cores_if_ready(Some(&topo))` (for `BUSY_HOOK`),
//!   `busy_cores`, `gpu_cores`, `take_long()`; statics `stop_orphan()`, `stop_orphan_sched()`. Tests: `set_now`,
//!   `add`, `add_routine`, `set_drivers`, `mark_available_for_tests`.
//! - [`frames::EtwFrames`] implements [`frames::IFrameSource`] (`available`, `name`, `error`, `watch(pid)`,
//!   `intervals(from, to)`), plus `start()`, `times(from, &mut into)`, `dispose()`. `IFrameSource` belongs to
//!   AutoController.cs (auto area); it is declared here because auto builds on etw: auto re-exports it.
//! - [`ready_probe::ReadyProbe`] + [`ready_probe::ProbePolicy`]: `start(pid)`, `stop()`, `tick()`, `running()`,
//!   `game_pid()`, `error()`, `wait_p99_ms(tid)`, `hard_faults_per_sec()`; static `stop_orphan()`.
//! - [`black_box::BlackBox`] (+ `BlackBoxPolicy`, `BlackBoxAnalyzer`, `BbWait`, `BbDpc`, `BbCause`, `BbReport`,
//!   `Ring`): `start(pid, game)`, `stop()`, `tick() -> Option<BbReport>`, `reports()`, `set_folder`, `set_refresh_hz`,
//!   `monitor_hz()`, `stop_orphan()`.
//! - [`check_trace::CheckTrace`]: `new(pid)`, `start()`, `begin()`, `end()`, `snapshot()`, `lost()`, `dispose()`,
//!   `ticks_per_second()`.
//! - [`irq_auto::IrqAuto`] (+ `AutoKind`, `AutoDevice`): the pure rules (`excluded`, `is_storage`, `msi_allowed`,
//!   `power_saving_allowed`, `classify`, `is_integrated_gpu`, `mark_integrated`, `is_wifi`, `is_display_audio_device`,
//!   `is_display_endpoint`, `is_spdif`, `hid_role`, `parse_cppc`, `choose_nic_mode`, `assign`, `layout_in_place`,
//!   `allowed_entry`, `valid_irq_value`, `import_backup`, `device_of_entry`, `INSTANCE_RX`, `BACKUP_NAME`) and the
//!   Windows parts (`discover`, `plan_now`, `apply`, `revert_extras`, `redo_old_layout`, `cleanup_legacy`,
//!   `has_backup`, `battery_or_modern_standby`, `cppc`). Hooks: `CPPC_HOOK` (tests), `MARK_PENDING_HOOK`
//!   (`BootGuard.MarkPending` of the system area; unset = nothing is written).
//! - [`rx::Rx`]: the small .NET-compatible regex engine IrqAuto's patterns run on; [`stj`]: System.Text.Json-shaped
//!   output; [`qpc`]: `Stopwatch.GetTimestamp` / `Frequency`; [`drivers`]: kernel driver load addresses.
//! - [`thread_map::ThreadMapTrace`] (native-only, the Thread map page): `global()`, `start(demo)`, `stop()`,
//!   `handle()` (the `tm_map*` of the threadmap crate), `set_game`, `flush_pending`, `lost`, `error`, `stop_orphan()`;
//!   the decoders (`parse_*`), the batching `Feeder` and the `Sink` trait. [`thread_map_demo`]: the synthetic feed
//!   (DOS_TM_DEMO=1 / screenshots only).

pub mod black_box;
pub mod check_trace;
pub mod drivers;
pub mod frames;
pub mod irq;
pub mod irq_auto;
pub mod qpc;
pub mod raw;
pub mod ready_probe;
pub mod rx;
pub mod stj;
pub mod thread_map;
pub mod thread_map_demo;
#[cfg(windows)]
mod wmi;

pub use black_box::{BbCause, BbDpc, BbReport, BbWait, BlackBox, BlackBoxAnalyzer, BlackBoxPolicy, Ring};
pub use check_trace::CheckTrace;
pub use frames::{EtwFrames, IFrameSource};
pub use irq::EtwIrq;
pub use irq_auto::{AutoDevice, AutoKind, IrqAuto};
pub use ready_probe::{ProbePolicy, ReadyProbe};

#[cfg(all(test, windows))]
mod win_smoke {
    use super::*;

    /// Windows (Wine in the build): every session starts or fails with a reason, and stops without hanging; the device
    /// reader runs (read-only).
    #[test]
    fn sessions_start_or_fail_cleanly_and_stop() {
        let irq = EtwIrq::new();
        let ok = irq.start();
        assert!(ok || !irq.error().is_empty());
        irq.pause();
        let p = ReadyProbe::new();
        p.start(4);
        p.tick();
        p.stop();
        let b = BlackBox::new();
        b.start(4, "x.exe");
        assert!(b.tick().is_none());
        b.stop();
        let c = CheckTrace::new(4);
        let _ = c.start();
        c.begin();
        c.end();
        c.dispose();
        let f = EtwFrames::new();
        f.start();
        f.watch(4);
        f.watch(0);
        f.dispose();
        let _ = BlackBox::monitor_hz();
        let _ = IrqAuto::battery_or_modern_standby();
        let devs = IrqAuto::discover();
        assert!(devs.windows(2).all(|w| w[0].kind <= w[1].kind));
        EtwIrq::stop_orphan();
        EtwIrq::stop_orphan_sched();
        BlackBox::stop_orphan();
        ReadyProbe::stop_orphan();
    }
}
