//! Ports of the C# GPU tests: AmdOcTests (all), the GPU parts of Release1130Tests, Release1160Tests, SystemToolsTests,
//! FuzzTests and SecurityTests. Same inputs, same expected values.

use super::amd_oc::{AdlAdapter, AmdDomain, AmdOc, GpuOc, IAdl};
use super::fake_adl::{FakeAdl, OdnSet};
use super::nv_oc::{self, NvClock, NvOc};
use super::profiles::{GpuOcLogic, GpuProfiles, GpuRange, Start};
use crate::base::flags::Flags;
use crate::base::hooks::static_hooks;
use crate::base::settings::{BackendGuard, MemorySettingsBackend, Settings};
use crate::base::testrand::{iterations, Rand};
use crate::base::time::{DateTime, Kind};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};

fn temp_dir(tag: &str) -> PathBuf {
    static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    std::env::temp_dir().join(format!("{}-{}-{}-{}", tag, std::process::id(), n, DateTime::utc_now().ticks))
}

struct Dir(PathBuf);
impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn rd(b: &[u8], at: usize) -> i32 {
    i32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}
fn wr(b: &mut [u8], at: usize, v: i32) {
    b[at..at + 4].copy_from_slice(&v.to_le_bytes());
}

// ================================================================================================
// AmdOcTests
// ================================================================================================

/// The C# fixture: settings in memory, the gpu-oc flag open; Dispose puts the real driver back.
struct AmdFix {
    _dir: Dir,
    dir: PathBuf,
    _settings: BackendGuard,
    _hooks: MutexGuard<'static, ()>,
}

impl AmdFix {
    fn new() -> Self {
        let hooks = static_hooks();
        let settings = Settings::scoped_backend(Arc::new(MemorySettingsBackend::new()));
        Flags::set(Flags::GPU_OC, true).unwrap();
        let dir = temp_dir("ds-amd");
        AmdFix { _dir: Dir(dir.clone()), dir, _settings: settings, _hooks: hooks }
    }
}

impl Drop for AmdFix {
    fn drop(&mut self) {
        AmdOc::use_(None);
    }
}

fn use_(f: FakeAdl) -> Arc<FakeAdl> {
    let a = Arc::new(f);
    AmdOc::use_(Some(a.clone() as Arc<dyn IAdl>));
    a
}

fn ad(index: i32, bus: i32, vendor: i32, active: bool, sup: i32, ver: i32) -> AdlAdapter {
    AdlAdapter { index, bus, vendor, active, od_supported: sup, od_version: ver, ..Default::default() }
}

// ---- choosing the card ----

#[test]
fn picks_the_active_physical_amd_card_once_per_bus() {
    let all = vec![
        ad(0, 5, 0x10DE, true, 1, 8),  // NVIDIA
        ad(1, -1, 1002, true, 1, 8),   // no bus: virtual
        ad(2, 7, 1002, false, 1, 8),   // not active
        AdlAdapter { asic_known: true, asic_types: 2, ..ad(3, 9, 1002, true, 1, 8) }, // integrated
        ad(5, 3, 1002, true, 1, 7),
        ad(4, 3, 1002, true, 1, 7), // same card, lower index
    ];
    let (a, why) = AmdOc::pick(&all);
    assert_eq!("", why);
    let a = a.unwrap();
    assert_eq!(4, a.index);
    assert_eq!(7, a.od_version);
}

#[test]
fn older_or_missing_overdrive_is_refused_with_a_reason() {
    let _h = static_hooks();
    let (a, why) = AmdOc::pick(&[AdlAdapter { bus: 1, vendor: 1002, active: true, od_supported: 1, od_version: 6, ..Default::default() }]);
    assert!(a.is_none());
    assert!(why.contains('6'));
    let (a, why) = AmdOc::pick(&[AdlAdapter { bus: 1, vendor: 1002, active: true, od_supported: 0, od_version: 8, ..Default::default() }]);
    assert!(a.is_none());
    assert!(why.contains("no overclocking support"));
    let (a, why) = AmdOc::pick(&[]);
    assert!(a.is_none());
    assert!(why.contains("No active AMD"));
}

#[test]
fn version8_is_overdrive8_and_version7_is_overdrive_n() {
    let _f = AmdFix::new();
    use_(FakeAdl::rx6800(false));
    let s = AmdOc::read().unwrap();
    assert_eq!(8, s.version);
    assert_eq!("Overdrive8", s.kind());
    assert_eq!("AMD Radeon RX 6800 XT", s.name);
    use_(FakeAdl::rx580_default());
    let s = AmdOc::read().unwrap();
    assert_eq!(7, s.version);
    assert_eq!("OverdriveN", s.kind());
}

#[test]
fn a_missing_driver_or_no_overdrive_does_nothing() {
    let _f = AmdFix::new();
    let f = FakeAdl::new();
    f.lock().missing = true;
    let f = use_(f);
    assert!(AmdOc::read().is_none());
    assert!(AmdOc::error().contains("atiadlxx.dll"));
    assert!(AmdOc::apply(100, 100, 10, 0).is_none());
    assert!(!AmdOc::reset(true));
    assert_eq!(0, f.lock().writes);

    let f = FakeAdl::rx580_default();
    f.lock().adapters.iter_mut().for_each(|a| a.supported = 0);
    let f = use_(f);
    assert!(AmdOc::read().is_none());
    assert!(AmdOc::error().contains("no overclocking support"));
    assert!(AmdOc::apply(100, 100, 10, 0).is_none());
    assert_eq!(0, f.lock().writes);
}

#[test]
fn a_closed_flag_refuses_the_write() {
    let _fx = AmdFix::new();
    let f = use_(FakeAdl::rx580_default());
    Flags::set(Flags::GPU_OC, false).unwrap();
    assert!(AmdOc::apply(50, 0, 0, 0).is_none());
    assert!(AmdOc::error().contains("gpu-oc"));
    assert_eq!(0, f.lock().writes);
}

// ---- RX 580 (OverdriveN) ----

#[test]
fn rx580_ranges_come_from_the_top_state_the_driver_range_and_the_caps() {
    let _fx = AmdFix::new();
    use_(FakeAdl::rx580_default());
    let s = AmdOc::read().unwrap();
    assert_eq!(100, s.core_scale); // 10 kHz units
    assert_eq!((1366, 1366, 2000), (s.core.default, s.core.current, s.core.max));
    assert_eq!(1300, s.core.min); // never below the state under the top one
    assert_eq!((2000, 2000, 2250), (s.mem.default, s.mem.min, s.mem.max)); // no underclocking flag: not below stock
    assert_eq!((-50, 50, 0), (s.power.min, s.power.max, s.power.default));
    assert!(!s.volt.editable); // OverdriveN: voltages are never changed
    let clocks = AmdOc::clocks(&s);
    assert_eq!(GpuRange::new(-65, 300, 5), GpuOcLogic::core_range(Some(&clocks[0]))); // +634 allowed by the driver, our cap is +300
    assert_eq!(GpuRange::new(0, 250, 25), GpuOcLogic::mem_range(Some(&clocks[1])));
    assert_eq!(GpuRange::new(-50, 50, 1), GpuOcLogic::power_range(Some(&s.power)));
    assert_eq!(1366_000, clocks[0].base_khz);
}

#[test]
fn rx580_apply_raises_only_the_top_state_and_keeps_every_voltage() {
    let _fx = AmdFix::new();
    let f = use_(FakeAdl::rx580_default());
    let low_sys: Vec<i32> = f.lock().sys_clk[..7].to_vec();
    let s = AmdOc::apply(50, 100, 20, -50).unwrap();
    assert_eq!("", AmdOc::error());
    assert_eq!((50, 100, 20), (s.core.offset(), s.mem.offset(), s.power.offset()));
    let st = f.lock();
    let sys: Vec<&OdnSet> = st.odn_sets.iter().filter(|x| !x.memory).collect();
    assert_eq!(1, sys.len());
    let sys = sys[0];
    assert_eq!(AmdOc::MODE_MANUAL, sys.mode);
    assert_eq!(low_sys, sys.clocks[..7].to_vec()); // lower states untouched
    assert_eq!(141_600, sys.clocks[7]);
    assert_eq!(vec![750, 769, 906, 1000, 1056, 1100, 1125, 1150], sys.vddc); // voltages as read
    assert!(sys.enabled.iter().all(|e| (0..=1).contains(e)));
    let mem: Vec<&OdnSet> = st.odn_sets.iter().filter(|x| x.memory).collect();
    assert_eq!(1, mem.len());
    assert_eq!(vec![30_000, 100_000, 210_000], mem[0].clocks);
    assert_eq!(vec![(AmdOc::MODE_MANUAL, 20)], st.power_sets);
}

#[test]
fn rx580_values_are_cut_to_the_caps_and_the_driver_range() {
    let _fx = AmdFix::new();
    let f = use_(FakeAdl::rx580_default());
    let s = AmdOc::apply(1000, 5000, 200, 0).unwrap();
    assert_eq!((300, 250, 50), (s.core.offset(), s.mem.offset(), s.power.offset()));
    assert_eq!(166_600, f.lock().sys_clk[7]);
    let s = AmdOc::apply(-500, -500, -200, 0).unwrap();
    assert_eq!((-66, 0, -50), (s.core.offset(), s.mem.offset(), s.power.offset()));
    assert_eq!(130_000, f.lock().sys_clk[7]); // equal to the state under it, never lower
}

#[test]
fn rx580_unchanged_values_are_not_written() {
    let _fx = AmdFix::new();
    let f = use_(FakeAdl::rx580_default());
    AmdOc::apply(0, 0, 0, 0);
    assert_eq!(0, f.lock().writes);
    AmdOc::apply(25, 0, 0, 0);
    assert_eq!(1, f.lock().odn_sets.len());
    assert!(f.lock().power_sets.is_empty());
}

#[test]
fn rx580_reset_uses_the_drivers_stock_table_also_after_a_failed_apply() {
    let _fx = AmdFix::new();
    let f = use_(FakeAdl::rx580_default());
    f.lock().fail_mem_set = -3;
    let s = AmdOc::apply(60, 100, 10, 0).unwrap();
    assert_ne!("", AmdOc::error()); // the memory write failed
    assert_eq!(60, s.core.offset()); // the core write went through
    f.lock().fail_mem_set = 0;
    assert!(AmdOc::reset(true));
    {
        let st = f.lock();
        assert_eq!(st.sys_def_clk, st.sys_clk);
        assert_eq!(st.mem_def_clk, st.mem_clk);
        assert_eq!(AmdOc::MODE_DEFAULT, st.odn_sets.last().unwrap().mode);
        assert_eq!((AmdOc::MODE_AUTO, 0), *st.power_sets.last().unwrap());
        assert_eq!(0, st.tdp);
    }
    // Nothing changed since: an unforced reset writes nothing.
    let writes = f.lock().writes;
    assert!(!AmdOc::reset(false));
    assert_eq!(writes, f.lock().writes);
}

#[test]
fn rx580_reset_works_in_a_new_session_without_any_remembered_state() {
    let _fx = AmdFix::new();
    let f = FakeAdl::rx580_default();
    f.lock().sys_clk[7] = 150_000; // set by an earlier run of the program
    f.lock().tdp = 30;
    let f = use_(f);
    assert!(AmdOc::reset(true));
    assert_eq!(136_600, f.lock().sys_clk[7]);
    assert_eq!(0, f.lock().tdp);
}

#[test]
fn overdrive_n_clocks_in_mhz_and_the_old_calls_work() {
    let _fx = AmdFix::new();
    assert_eq!(100, AmdOc::scale(200_000));
    assert_eq!(1, AmdOc::scale(2000));
    let f = use_(FakeAdl::rx580(false, 1));
    let s = AmdOc::read().unwrap();
    assert_eq!(1, s.core_scale);
    assert!(!s.x2);
    assert_eq!((1366, 1300), (s.core.default, s.core.min));
    AmdOc::apply(40, 0, 0, 0);
    assert_eq!(1406, f.lock().sys_clk[7]);
    assert_eq!(vec![300, 600, 900, 1145, 1215, 1257, 1300], f.lock().sys_clk[..7].to_vec());
}

#[test]
fn overdrive_n_is_not_offered_when_a_voltage_is_not_what_the_driver_reports() {
    let _fx = AmdFix::new();
    let f = FakeAdl::rx580_default();
    f.lock().sys_vddc[3] = 5000; // neither in the voltage range nor in the stock table
    f.lock().default_read_fails = true;
    let f = use_(f);
    let s = AmdOc::read().unwrap();
    assert!(!s.core.editable);
    AmdOc::apply(50, 0, 0, 0);
    assert!(!f.lock().odn_sets.iter().any(|x| !x.memory));
}

#[test]
fn overdrive_n_top_state_the_driver_locks_is_not_offered() {
    let _fx = AmdFix::new();
    let f = FakeAdl::rx580_default();
    f.lock().sys_control = Some(vec![7, 7, 7, 7, 7, 7, 7, 0]); // the top state's clock is not editable
    use_(f);
    let s = AmdOc::read().unwrap();
    assert!(!s.core.editable);
    assert_eq!(vec![NvOc::DOMAIN_MEMORY], AmdOc::clocks(&s).iter().map(|c| c.domain).collect::<Vec<_>>()); // the page says the core is locked
}

// ---- RX 6000/7000 (Overdrive8) ----

#[test]
fn rx6800_ranges_come_from_the_driver_table() {
    let _fx = AmdFix::new();
    use_(FakeAdl::rx6800(false));
    let s = AmdOc::read().unwrap();
    assert_eq!((2250, 500, 2800), (s.core.default, s.core.min, s.core.max));
    let clocks = AmdOc::clocks(&s);
    assert_eq!(GpuRange::new(-300, 300, 5), GpuOcLogic::core_range(Some(&clocks[0])));
    assert_eq!(GpuRange::new(0, 150, 25), GpuOcLogic::mem_range(Some(&clocks[1])));
    assert_eq!(GpuRange::new(-10, 15, 1), GpuOcLogic::power_range(Some(&s.power)));
    assert!(!s.volt.editable);
}

#[test]
fn rx6800_apply_switches_to_manual_then_writes_only_what_changed() {
    let _fx = AmdFix::new();
    let f = use_(FakeAdl::rx6800(false));
    let s = AmdOc::apply(100, 100, 15, 0).unwrap();
    assert_eq!("", AmdOc::error());
    assert_eq!((100, 100, 15), (s.core.offset(), s.mem.offset(), s.power.offset()));
    assert_eq!(2, f.lock().od8_sets.len());
    assert_eq!(vec![(AmdOc::POWER_MODE, AmdOc::POWER_MODE_MANUAL, false)], f.lock().od8_sets[0]);
    assert_eq!(vec![(0, 2350, false), (1, 500, false), (8, 1100, false), (9, 15, false)], f.lock().od8_sets[1]);
    // Only the core again: the mode is already manual, memory and power are not sent.
    AmdOc::apply(50, 100, 15, 0);
    assert_eq!(vec![(0, 2300, false), (1, 500, false)], f.lock().od8_sets[2]);
}

#[test]
fn rx6800_reset_sends_the_reset_flag_with_the_defaults() {
    let _fx = AmdFix::new();
    let f = use_(FakeAdl::rx6800(false));
    AmdOc::apply(100, 100, 15, 0);
    assert!(AmdOc::reset(true));
    let st = f.lock();
    let reset = st.od8_sets.last().unwrap();
    assert!(reset.iter().all(|x| x.2));
    assert_eq!(vec![0, 1, 8, 9, 36], reset.iter().map(|x| x.0).collect::<Vec<_>>());
    assert_eq!((2250, 1000, 0, 0), (st.cur[0], st.cur[8], st.cur[9], st.cur[36]));
}

#[test]
fn rx6800_a_refused_apply_leaves_a_working_reset() {
    let _fx = AmdFix::new();
    let f = use_(FakeAdl::rx6800(false));
    f.lock().od8_set_rc = -3;
    assert!(AmdOc::apply(100, 0, 0, 0).is_some());
    assert!(AmdOc::error().contains("refused"));
    f.lock().od8_set_rc = 0;
    f.lock().cur[0] = 2400; // whatever is in force, reset brings stock
    assert!(AmdOc::reset(true));
    assert_eq!(2250, f.lock().cur[0]);
}

#[test]
fn rx7900_voltage_offset_only_goes_down() {
    let _fx = AmdFix::new();
    let f = use_(FakeAdl::rx6800(true));
    let s = AmdOc::read().unwrap();
    assert!(s.volt.editable);
    assert_eq!(0, s.volt.max); // the driver allows +50 mV: never offered
    assert_eq!(GpuRange::new(-200, 0, 5), GpuOcLogic::volt_mv_range(Some(&s.volt)));
    AmdOc::apply(0, 0, 0, 30);
    assert_eq!(0, f.lock().writes); // a raise is not written at all
    let s = AmdOc::apply(0, 0, 0, -50).unwrap();
    assert_eq!(-50, s.volt.offset());
    let s = AmdOc::apply(0, 0, 0, -900).unwrap();
    assert_eq!(-200, s.volt.offset());
    assert!(AmdOc::reset(true));
    assert_eq!(0, f.lock().cur[AmdOc::OD_VOLTAGE as usize]);
}

#[test]
fn implausible_tables_are_not_offered() {
    let _fx = AmdFix::new();
    let f = FakeAdl::rx6800(false);
    f.lock().od8(AmdOc::GFX_MAX, 500, 2800, 3500, None); // stock outside its own range
    f.lock().od8(AmdOc::UCLK_MAX, 0, 90_000, 1000, None); // a memory clock no card has
    f.lock().od8(AmdOc::POWER_PCT, 10, -10, 0, None); // a reversed range
    let f = use_(f);
    assert!(AmdOc::read().is_none());
    assert!(AmdOc::error().contains("no adjustable"));
    assert!(AmdOc::apply(100, 100, 10, 0).is_none());
    assert_eq!(0, f.lock().writes);
}

#[test]
fn targets_stay_on_the_drivers_step_rounded_towards_stock() {
    let d = AmdDomain { editable: true, min: 300, max: 2000, default: 1366, current: 1366, step: 5 };
    assert_eq!(1366, AmdOc::target(&d, 3, -300, 300)); // one grid step (1365/1370) would overshoot: stock
    assert_eq!(1370, AmdOc::target(&d, 7, -300, 300));
    assert_eq!(1360, AmdOc::target(&d, -7, -300, 300));
    assert_eq!(1665, AmdOc::target(&d, 1000, -300, 300));
    assert_eq!(1366, AmdOc::target(&d, 0, -300, 300));
}

#[test]
fn temperature_comes_from_the_right_sensor() {
    let _fx = AmdFix::new();
    use_(FakeAdl::rx580_default());
    assert!(AmdOc::read().is_some());
    assert_eq!(65, AmdOc::temperature()); // OverdriveN: millidegrees
    use_(FakeAdl::rx6800(false));
    assert!(AmdOc::read().is_some());
    assert_eq!(60, AmdOc::temperature()); // Overdrive8: PMLog edge sensor
}

// ---- profiles and the start ----

#[test]
fn amd_profiles_keep_their_values_and_maker() {
    let fx = AmdFix::new();
    std::fs::create_dir_all(&fx.dir).unwrap();
    let file = fx.dir.join("gpu-profiles.json");
    let mut g = GpuProfiles::new();
    assert!(g.put("Radeon", 50, 100, 30, 12, -40, Some(GpuOc::AMD)));
    assert!(g.put("Clamp", 0, 0, 0, 500, 80, Some(GpuOc::AMD)));
    g.save(Some(&file)).unwrap();
    let back = GpuProfiles::load(Some(&file));
    let p = back.find("Radeon").unwrap();
    assert_eq!((50, 100, 30, 12, -40, "amd"), (p.core_mhz, p.mem_mhz, p.volt_pct, p.power_pct, p.volt_mv, p.vendor.as_str()));
    let c = back.find("Clamp").unwrap();
    assert_eq!((100, 0), (c.power_pct, c.volt_mv)); // raising the voltage is never stored
    // A file of 1.15.0 Test 1.4: no new fields, an NVIDIA profile.
    std::fs::write(&file, "{\"Profiles\":[{\"Name\":\"Old\",\"CoreMHz\":100,\"MemMHz\":500,\"VoltPct\":10}],\"AtStart\":\"Old\"}").unwrap();
    let all = GpuProfiles::load(Some(&file));
    let old = all.find("Old").unwrap();
    assert_eq!(("", "nvidia", 0, 0), (old.vendor.as_str(), old.maker(), old.power_pct, old.volt_mv));
}

#[test]
fn crash_guard_and_start_work_the_same_for_amd() {
    let mut g = GpuProfiles::new();
    g.put("Power only", 0, 0, 0, 10, 0, Some(GpuOc::AMD));
    assert!(!g.find("Power only").unwrap().is_stock()); // a power limit alone is an overclock to re-apply
    g.set_at_start(Some("Power only"));
    assert_eq!(Start::Apply, GpuOcLogic::decide_start(true, Some(&g), false).0);
    assert_eq!(Start::SkipCrash, GpuOcLogic::decide_start(true, Some(&g), true).0); // unexpected shutdown since the apply
    assert_eq!(Start::Nothing, GpuOcLogic::decide_start(false, Some(&g), false).0); // flag closed
    g.put("Undervolt", 0, 0, 0, 0, -30, Some(GpuOc::AMD));
    g.set_at_start(Some("Undervolt"));
    assert_eq!(Start::SkipCrash, GpuOcLogic::decide_start(true, Some(&g), true).0);
}

#[test]
fn profiles_are_applied_at_start_only_on_their_makers_card() {
    let mut g = GpuProfiles::new();
    g.put("Radeon", 50, 0, 0, 0, 0, Some(GpuOc::AMD));
    g.put("GeForce", 50, 0, 10, 0, 0, Some(GpuOc::NVIDIA));
    g.put3("Old", 50, 0, 10);
    assert!(GpuOcLogic::fits(g.find("Radeon"), GpuOc::AMD));
    assert!(!GpuOcLogic::fits(g.find("Radeon"), GpuOc::NVIDIA));
    assert!(GpuOcLogic::fits(g.find("GeForce"), GpuOc::NVIDIA));
    assert!(!GpuOcLogic::fits(g.find("Old"), GpuOc::AMD));
    assert!(GpuOcLogic::fits(g.find("Old"), GpuOc::NVIDIA));
    assert!(!GpuOcLogic::fits(g.find("Radeon"), ""));
}

#[test]
fn od8_set_block_has_the_sdk_layout() {
    let b = AmdOc::build_od8_set(77, &[(0, 2350, false), (36, 3, true), (200, 1, false)]);
    assert_eq!(4 + 77 * 12, b.len());
    assert_eq!(77, rd(&b, 0));
    assert_eq!((2350, 1, 0), (rd(&b, 4), rd(&b, 8), rd(&b, 12)));
    let at = 4 + 36 * 12;
    assert_eq!((3, 1, 1), (rd(&b, at), rd(&b, at + 4), rd(&b, at + 8)));
    assert_eq!(140, AmdOc::levels_bytes(true, 8)); // ADLODNPerformanceLevelsX2: 3 ints + 8 × 16 bytes
    assert_eq!(48, AmdOc::levels_bytes(false, 3)); // ADLODNPerformanceLevels: 3 ints + 3 × 12 bytes
    assert_eq!(1572, AmdOc::ADAPTER_INFO_SIZE); // AdapterInfo on Windows
}

// ================================================================================================
// Release1160Tests (GPU crash memory)
// ================================================================================================

fn crash() -> DateTime {
    DateTime::from_parts(2026, 11, 19, 17, 10, 0, Kind::Utc).unwrap()
}

#[test]
fn crashed_values_are_recognised_again() {
    let mut g = GpuProfiles::new();
    g.put("Saved values", 105, 1500, 100, 0, 0, Some(GpuOc::NVIDIA));
    let p = g.find("Saved values").cloned();
    g.record_crash(p.as_ref(), crash());
    // the same values, under the same or another name
    assert!(GpuOcLogic::crashed_before(g.find("Saved values"), Some(&g.crashes)).is_some());
    g.put("Copy", 105, 1500, 100, 0, 0, Some(GpuOc::NVIDIA));
    assert_eq!(crash(), GpuOcLogic::crashed_before(g.find("Copy"), Some(&g.crashes)).unwrap().utc);
    // higher in everything: still the crashed values or more
    g.put("Higher", 120, 1600, 100, 0, 0, Some(GpuOc::NVIDIA));
    assert!(GpuOcLogic::crashed_before(g.find("Higher"), Some(&g.crashes)).is_some());
}

#[test]
fn lower_or_other_maker_is_not_warned() {
    let mut g = GpuProfiles::new();
    g.put("Saved values", 105, 1500, 100, 0, 0, Some(GpuOc::NVIDIA));
    let p = g.find("Saved values").cloned();
    g.record_crash(p.as_ref(), crash());
    g.put("Softer memory", 105, 1000, 100, 0, 0, Some(GpuOc::NVIDIA));
    assert!(GpuOcLogic::crashed_before(g.find("Softer memory"), Some(&g.crashes)).is_none());
    g.put("Softer core", 60, 1500, 100, 0, 0, Some(GpuOc::NVIDIA));
    assert!(GpuOcLogic::crashed_before(g.find("Softer core"), Some(&g.crashes)).is_none());
    g.put("Less voltage", 105, 1500, 50, 0, 0, Some(GpuOc::NVIDIA));
    assert!(GpuOcLogic::crashed_before(g.find("Less voltage"), Some(&g.crashes)).is_none());
    g.put("Radeon", 105, 1500, 0, 0, 0, Some(GpuOc::AMD));
    assert!(GpuOcLogic::crashed_before(g.find("Radeon"), Some(&g.crashes)).is_none());
    g.put("Stock", 0, 0, 0, 0, 0, Some(GpuOc::NVIDIA));
    assert!(GpuOcLogic::crashed_before(g.find("Stock"), Some(&g.crashes)).is_none());
    assert!(GpuOcLogic::crashed_before(g.find("Stock"), None).is_none());
}

#[test]
fn crash_memory_survives_save_load_and_is_capped() {
    let dir = Dir(temp_dir("ds-1160"));
    let file = dir.0.join("gpu-profiles.json");
    let mut g = GpuProfiles::new();
    g.put("P", 100, 1000, 0, 0, 0, Some(GpuOc::NVIDIA));
    for i in 0..GpuProfiles::MAX_CRASHES + 5 {
        let p = g.find("P").cloned();
        g.record_crash(p.as_ref(), crash().add_days(i as f64));
    }
    assert_eq!(GpuProfiles::MAX_CRASHES, g.crashes.len());
    assert_eq!(crash().add_days(5.0), g.crashes[0].utc); // the oldest ones went first
    g.save(Some(&file)).unwrap();
    let back = GpuProfiles::load(Some(&file));
    assert_eq!(GpuProfiles::MAX_CRASHES, back.crashes.len());
    assert_eq!(1000, back.crashes.last().unwrap().mem_mhz);
    assert_eq!(crash().add_days((GpuProfiles::MAX_CRASHES + 4) as f64), GpuOcLogic::crashed_before(back.find("P"), Some(&back.crashes)).unwrap().utc); // the newest crash
}

#[test]
fn old_file_without_crashes_loads() {
    let dir = Dir(temp_dir("ds-1160"));
    std::fs::create_dir_all(&dir.0).unwrap();
    let file = dir.0.join("gpu-profiles.json");
    std::fs::write(&file, "{\"Profiles\":[{\"Name\":\"A\",\"CoreMHz\":50,\"MemMHz\":500}],\"AtStart\":\"A\"}").unwrap();
    let g = GpuProfiles::load(Some(&file));
    assert!(g.crashes.is_empty());
    assert_eq!("A", g.at_start);
}

// ================================================================================================
// Release1130Tests (GPU slider ranges, the profile store and the start decision)
// ================================================================================================

#[test]
fn slider_ranges_follow_the_driver_and_the_caps() {
    // The driver allows −1000…+1000 MHz on the core: NvOc's caps cut it to −300…+300.
    let core = NvClock { domain: NvOc::DOMAIN_CORE, editable: true, min_delta_khz: -1_000_000, max_delta_khz: 1_000_000, ..Default::default() };
    assert_eq!(GpuRange::new(-300, 300, 5), GpuOcLogic::core_range(Some(&core)));
    // A narrower driver range wins, pulled inward onto the 25 MHz grid.
    let mem = NvClock { domain: NvOc::DOMAIN_MEMORY, editable: true, min_delta_khz: -510_000, max_delta_khz: 1_010_000, ..Default::default() };
    assert_eq!(GpuRange::new(-300, 1000, 25), GpuOcLogic::mem_range(Some(&mem)));
    let mem2 = NvClock { domain: NvOc::DOMAIN_MEMORY, editable: true, min_delta_khz: -240_000, max_delta_khz: 990_000, ..Default::default() };
    assert_eq!(GpuRange::new(-225, 975, 25), GpuOcLogic::mem_range(Some(&mem2)));
    // Nothing reported: the fixed ranges, still under the caps.
    assert_eq!(GpuRange::new(-200, 300, 5), GpuOcLogic::core_range(None));
    assert_eq!(GpuRange::new(-300, 1500, 25), GpuOcLogic::mem_range(None));
    assert_eq!(GpuRange::new(0, 100, 1), GpuOcLogic::VOLT);
}

#[test]
fn snap_clamps_and_rounds_to_the_step() {
    let r = GpuRange::new(-225, 975, 25);
    assert_eq!(0, r.snap(0.0));
    assert_eq!(25, r.snap(13.0));
    assert_eq!(0, r.snap(12.0));
    assert_eq!(-225, r.snap(-5000.0));
    assert_eq!(975, r.snap(2000.0));
    assert_eq!(975, r.snap(974.0));
    assert_eq!(0, r.snap(f64::NAN));
    let c = GpuRange::new(-300, 300, 5);
    assert_eq!(105, c.snap(103.0));
    assert_eq!(-105, c.snap(-103.0));
    assert_eq!(55, GpuOcLogic::VOLT.snap(55.4));
    assert_eq!(0, GpuOcLogic::VOLT.snap(-3.0));
}

#[test]
fn profile_store_saves_renames_deletes_and_keeps_the_start_mark() {
    let dir = Dir(temp_dir("ds-gpu"));
    let file_p = dir.0.join("gpu-profiles.json");
    let mut g = GpuProfiles::new();
    assert!(g.put3("  Daily  ", 100, 500, 0));
    assert!(g.put3("Bench", 150, 1000, 30));
    assert!(!g.put3("   ", 1, 1, 1));
    assert!(!g.put3(&"x".repeat(GpuProfiles::MAX_NAME + 1), 1, 1, 1));
    assert!(g.put3("daily", 105, 525, 150)); // same name in another case: overwritten, voltage clamped
    assert_eq!(2, g.profiles.len());
    let d = g.find("Daily").unwrap();
    assert_eq!((105, 525, 100), (d.core_mhz, d.mem_mhz, d.volt_pct));

    assert!(g.set_at_start(Some("bench")));
    assert_eq!("Bench", g.at_start);
    assert!(!g.set_at_start(Some("nope")));
    assert!(!g.rename("Bench", "DAILY")); // taken
    assert!(!g.rename("Bench", ""));
    assert!(g.rename("Bench", "Bench 2"));
    assert_eq!("Bench 2", g.at_start); // the mark follows the rename
    assert!(g.rename("Bench 2", "bench 2")); // only the case changes

    g.save(Some(&file_p)).unwrap();
    let mut back = GpuProfiles::load(Some(&file_p));
    assert_eq!(2, back.profiles.len());
    assert_eq!(Some("bench 2"), back.start_profile().map(|p| p.name.as_str()));
    assert_eq!(150, back.start_profile().unwrap().core_mhz);

    assert!(back.delete("BENCH 2"));
    assert_eq!("", back.at_start); // the mark goes with the profile
    assert!(!back.delete("BENCH 2"));
    assert_eq!(1, back.profiles.len());
}

#[test]
fn broken_or_missing_file_gives_an_empty_store() {
    let dir = Dir(temp_dir("ds-gpu"));
    let file_p = dir.0.join("gpu-profiles.json");
    assert!(GpuProfiles::load(Some(&file_p)).profiles.is_empty());
    std::fs::create_dir_all(&dir.0).unwrap();
    std::fs::write(&file_p, "{ not json").unwrap();
    assert!(GpuProfiles::load(Some(&file_p)).profiles.is_empty());
    std::fs::write(&file_p, "{\"Profiles\":[{\"Name\":\"A\",\"CoreMHz\":50},{\"Name\":\"a\"},{\"Name\":\"\"},null],\"AtStart\":\"ghost\"}").unwrap();
    let g = GpuProfiles::load(Some(&file_p));
    assert_eq!(1, g.profiles.len());
    assert_eq!(50, g.profiles[0].core_mhz);
    assert_eq!("", g.at_start); // points at no profile: dropped
}

#[test]
fn start_decision_keeps_the_crash_guard() {
    let mut g = GpuProfiles::new();
    g.put3("Daily", 100, 500, 0);
    g.put3("Stock", 0, 0, 0);
    assert_eq!(Start::Nothing, GpuOcLogic::decide_start(true, Some(&g), false).0); // nothing marked
    g.set_at_start(Some("Daily"));
    assert_eq!(Start::Apply, GpuOcLogic::decide_start(true, Some(&g), false).0);
    assert_eq!("Daily", GpuOcLogic::decide_start(true, Some(&g), false).1.unwrap().name);
    assert_eq!(Start::SkipCrash, GpuOcLogic::decide_start(true, Some(&g), true).0); // last session ended < 10 min after an apply
    assert_eq!(Start::Nothing, GpuOcLogic::decide_start(false, Some(&g), false).0); // the feature flag is closed
    assert_eq!(Start::Nothing, GpuOcLogic::decide_start(false, Some(&g), true).0);
    g.set_at_start(Some("Stock"));
    assert_eq!(Start::Nothing, GpuOcLogic::decide_start(true, Some(&g), true).0); // all zero: nothing to apply
    assert_eq!(Start::Nothing, GpuOcLogic::decide_start(true, None, false).0);
}

#[test]
fn old_start_values_become_a_profile_once() {
    let mut g = GpuProfiles::new();
    assert!(!GpuOcLogic::migrate(Some(&mut g), true, true, 100_000, 0, 0, "Saved"));
    assert!(!GpuOcLogic::migrate(Some(&mut g), false, false, 100_000, 0, 0, "Saved"));
    assert!(!GpuOcLogic::migrate(Some(&mut g), false, true, 0, 0, 0, "Saved"));
    assert!(GpuOcLogic::migrate(Some(&mut g), false, true, 120_000, 500_000, 20, "Saved"));
    assert_eq!("Saved", g.at_start);
    let s = g.start_profile().unwrap();
    assert_eq!((120, 500, 20), (s.core_mhz, s.mem_mhz, s.volt_pct));
}

#[test]
fn marking_a_profile_clears_the_crash_notice() {
    let mut g = GpuProfiles::new();
    g.put3("Daily", 100, 0, 0);
    g.skipped = "Daily".into();
    g.rename("Daily", "Main");
    assert_eq!("Main", g.skipped);
    g.set_at_start(Some("Main"));
    assert_eq!("", g.skipped);
}

// ================================================================================================
// SystemToolsTests (the byte work of the NVIDIA clock and voltage interface)
// ================================================================================================

fn table(version: i32, core_delta: i32, mem_delta: i32) -> Vec<u8> {
    let (core_max, mem_max) = (250_000, 2_000_000);
    let mut b = vec![0u8; if version == NvOc::VERSION { NvOc::SIZE } else { NvOc::SIZE_V2 } as usize];
    wr(&mut b, 0, version);
    wr(&mut b, 4, 1);
    wr(&mut b, 8, 1);
    wr(&mut b, 12, 2);
    let (c0, c1) = (20 + 8, 20 + 8 + 44);
    wr(&mut b, c0, 0);
    wr(&mut b, c0 + 4, 1);
    wr(&mut b, c0 + 8, 1);
    wr(&mut b, c0 + 12, core_delta);
    wr(&mut b, c0 + 16, -500_000);
    wr(&mut b, c0 + 20, core_max);
    wr(&mut b, c0 + 24, 300_000);
    wr(&mut b, c0 + 28, 2_600_000);
    wr(&mut b, c1, 4);
    wr(&mut b, c1 + 4, 0);
    wr(&mut b, c1 + 8, 1);
    wr(&mut b, c1 + 12, mem_delta);
    wr(&mut b, c1 + 16, -500_000);
    wr(&mut b, c1 + 20, mem_max);
    wr(&mut b, c1 + 24, 9_000_000);
    b
}

#[test]
fn clock_table_is_parsed_checked_and_built_to_the_drivers_layout() {
    assert_eq!(7316, NvOc::SIZE);
    assert_eq!(0x11C94, NvOc::VERSION);
    assert_eq!(0x21CF8, NvOc::VERSION2);
    assert_eq!(0x31CF8, NvOc::VERSION3);
    assert_eq!(0x10028, NvOc::VOLT_VERSION);

    let clocks = NvOc::parse(Some(&table(NvOc::VERSION3, 50_000, 0))).unwrap();
    assert_eq!(2, clocks.len());
    assert_eq!((0, 2_600_000, 50_000, true), (clocks[0].domain, clocks[0].base_khz, clocks[0].delta_khz, clocks[0].editable)); // a range clock: its maximum
    assert_eq!((4, 9_000_000), (clocks[1].domain, clocks[1].base_khz));
    assert!(NvOc::parse(Some(&table(NvOc::VERSION, 0, 0))).is_some());

    // Tables that do not look like one are refused, so nothing is ever written from a misread.
    assert!(NvOc::parse(Some(&[0u8; 100])).is_none());
    let mut bad = table(NvOc::VERSION3, 0, 0);
    wr(&mut bad, 0, 0x12345);
    assert!(NvOc::parse(Some(&bad)).is_none());
    let mut bad = table(NvOc::VERSION3, 0, 0);
    wr(&mut bad, 12, 40); // 40 clocks: impossible
    assert!(NvOc::parse(Some(&bad)).is_none());
    let mut bad = table(NvOc::VERSION3, 0, 0);
    wr(&mut bad, 20 + 8 + 28, 5); // a 5 kHz core clock: impossible
    assert!(NvOc::parse(Some(&bad)).is_none());

    // The request never leaves the driver's range nor our cap.
    assert_eq!(250_000, NvOc::clamp(&clocks[0], 900_000)); // the driver's maximum is lower than the cap
    assert_eq!(NvOc::CAP_MEMORY_KHZ, NvOc::clamp(&clocks[1], 5_000_000)); // the cap is lower than the driver's maximum
    assert_eq!(NvOc::FLOOR_KHZ, NvOc::clamp(&clocks[0], -900_000));

    let set = NvOc::build_set(NvOc::DOMAIN_MEMORY, 125_000, NvOc::VERSION3);
    assert_eq!(NvOc::SIZE_V2 as usize, set.len());
    assert_eq!(NvOc::VERSION3, rd(&set, 0));
    assert_eq!((1, 1, 0), (rd(&set, 8), rd(&set, 12), rd(&set, 16)));
    assert_eq!((4, 125_000), (rd(&set, 28), rd(&set, 40)));
    assert!(set[44..].iter().all(|&x| x == 0)); // nothing after the one delta is filled in
    let volt = NvOc::build_volt(250);
    assert_eq!((0x10028, 100), (rd(&volt, 0), rd(&volt, 4)));
    assert_eq!(40, volt.len());
}

/// Clears the NvOc hooks at the end (the C# Dispose).
struct NvFix {
    _settings: BackendGuard,
    _hooks: MutexGuard<'static, ()>,
}

impl Drop for NvFix {
    fn drop(&mut self) {
        nv_oc::GET_HOOK.set(None);
        nv_oc::SET_HOOK.set(None);
        nv_oc::VOLT_GET_HOOK.set(None);
        nv_oc::VOLT_SET_HOOK.set(None);
    }
}

#[test]
fn offsets_and_voltage_go_through_the_flag_are_clamped_and_read_back() {
    let hooks = static_hooks();
    let _fx = NvFix { _settings: Settings::scoped_backend(Arc::new(MemorySettingsBackend::new())), _hooks: hooks };
    nv_oc::reset_for_tests();
    #[derive(Default)]
    struct S {
        core: i32,
        mem: i32,
        volt: i32,
        sets: i32,
    }
    let s = Arc::new(Mutex::new(S::default()));
    let g = s.clone();
    nv_oc::GET_HOOK.set(Some(Arc::new(move || {
        let s = g.lock().unwrap();
        Some(table(NvOc::VERSION3, s.core, s.mem))
    })));
    let g = s.clone();
    nv_oc::SET_HOOK.set(Some(Arc::new(move |t: &[u8]| {
        let mut s = g.lock().unwrap();
        s.sets += 1;
        let (domain, delta) = (rd(t, 28), rd(t, 40));
        if domain == 0 {
            s.core = delta;
        } else {
            s.mem = delta;
        }
        0
    })));
    let g = s.clone();
    nv_oc::VOLT_GET_HOOK.set(Some(Arc::new(move || Some(NvOc::build_volt(g.lock().unwrap().volt)))));
    let g = s.clone();
    nv_oc::VOLT_SET_HOOK.set(Some(Arc::new(move |b: &[u8]| {
        let mut s = g.lock().unwrap();
        s.sets += 1;
        s.volt = rd(b, 4);
        0
    })));

    Flags::set(Flags::GPU_OC, false).unwrap(); // open by default since 1.16.0: close it for this check
    assert_eq!(None, NvOc::set(NvOc::DOMAIN_CORE, 100_000)); // the flag is closed
    assert_eq!(None, NvOc::set_voltage(50));
    assert_eq!(0, s.lock().unwrap().sets);

    Flags::set(Flags::GPU_OC, true).unwrap();
    assert_eq!(Some(100_000), NvOc::set(NvOc::DOMAIN_CORE, 100_000));
    assert_eq!(Some(250_000), NvOc::set(NvOc::DOMAIN_CORE, 999_000)); // clamped to the driver's range
    assert_eq!(Some(NvOc::CAP_MEMORY_KHZ), NvOc::set(NvOc::DOMAIN_MEMORY, 9_000_000));
    assert_eq!(Some(100), NvOc::set_voltage(400));
    assert_eq!(Some(35), NvOc::set_voltage(35));
    {
        let s = s.lock().unwrap();
        assert_eq!((250_000, NvOc::CAP_MEMORY_KHZ, 35), (s.core, s.mem, s.volt));
    }

    nv_oc::SET_HOOK.set(Some(Arc::new(|_t: &[u8]| -104))); // the card says "not supported"
    assert_eq!(None, NvOc::set(NvOc::DOMAIN_CORE, 50_000));
    assert!(NvOc::error().contains("does not support"));

    let g = s.clone();
    nv_oc::SET_HOOK.set(Some(Arc::new(move |t: &[u8]| {
        let mut s = g.lock().unwrap();
        if rd(t, 28) == 0 {
            s.core = 0;
        } else {
            s.mem = 0;
        }
        0
    })));
    assert!(NvOc::reset(true)); // only when asked: everything back to stock
    let s = s.lock().unwrap();
    assert_eq!((0, 0, 0), (s.core, s.mem, s.volt));
}

// ================================================================================================
// FuzzTests (driver byte tables, the profile file)
// ================================================================================================

#[test]
fn nvidia_clock_table_never_throws_and_stays_in_bounds() {
    let mut r = Rand::new(1601);
    for _ in 0..iterations() {
        let mut t = r.bytes(NvOc::SIZE_V2 + 200);
        if t.len() >= 16 && r.next(2) == 0 {
            // a plausible header: the parser gets past the version check and has to cope with the body
            let v = match r.next(3) {
                0 => NvOc::VERSION,
                1 => NvOc::VERSION2,
                _ => NvOc::VERSION3,
            };
            wr(&mut t, 0, v);
            let p = r.range(-2, 20);
            wr(&mut t, 8, p);
            let c = r.range(-2, 12);
            wr(&mut t, 12, c);
        }
        let Some(clocks) = NvOc::parse(Some(&t)) else { continue };
        assert!(clocks.len() <= 8);
    }
}

#[test]
fn amd_adapter_table_never_reads_past_its_end() {
    let mut r = Rand::new(1602);
    for _ in 0..iterations() {
        let info = r.bytes(AmdOc::ADAPTER_INFO_SIZE * 4 + 50);
        let count = r.range(-3, 10);
        let list = AmdOc::parse_adapters(Some(&info), count);
        assert!(list.len() as i32 <= 0.max(count));
        assert!(list.len() * AmdOc::ADAPTER_INFO_SIZE as usize <= info.len());
    }
    assert!(AmdOc::parse_adapters(None, 5).is_empty());
}

#[test]
fn gpu_profile_file_of_any_content_loads_inside_the_limits() {
    let dir = Dir(temp_dir("ds-fuzz"));
    std::fs::create_dir_all(&dir.0).unwrap();
    let mut r = Rand::new(1608);
    let file = dir.0.join("gpu-profiles.json");
    let mut good = GpuProfiles::new();
    good.put("Daily", 100, 1000, 50, 10, -50, Some(GpuOc::NVIDIA));
    good.put("Radeon", 50, 200, 0, -20, -100, Some(GpuOc::AMD));
    good.set_at_start(Some("Daily"));
    let p = good.find("Daily").cloned();
    good.record_crash(p.as_ref(), DateTime::utc_now());
    good.save(Some(&file)).unwrap();
    let seed = std::fs::read(&file).unwrap();
    for _ in 0..iterations() {
        let bytes = if r.next(4) == 0 { r.text(400).into_bytes() } else { r.mutate(&seed) };
        std::fs::write(&file, bytes).unwrap();
        let g = GpuProfiles::load(Some(&file)); // never panics
        assert!(g.crashes.len() <= GpuProfiles::MAX_CRASHES);
        for p in &g.profiles {
            assert!(GpuProfiles::clean_name(Some(&p.name)).is_some());
            assert!((0..=100).contains(&p.volt_pct));
            assert!((-100..=100).contains(&p.power_pct));
            assert!((-500..=0).contains(&p.volt_mv)); // an AMD voltage offset is only ever lowered
        }
        assert!(g.at_start.is_empty() || g.find(&g.at_start).is_some());
        let mut names: Vec<String> = g.profiles.iter().map(|p| crate::base::ci::upper(&p.name)).collect();
        names.sort();
        names.dedup();
        assert_eq!(g.profiles.len(), names.len());
    }
}

// ================================================================================================
// SecurityTests
// ================================================================================================

#[test]
fn old_start_mark_waits_for_answer_and_any_choice_answers_it() {
    let mut g = GpuProfiles::new();
    g.put3("Max", 300, 1500, 50);
    g.put3("Daily", 100, 500, 0);
    g.confirm = "max".into();
    assert_eq!(Some("Max"), g.confirm_profile().map(|p| p.name.as_str()));
    let (what, _) = GpuOcLogic::decide_start(true, Some(&g), false);
    assert_eq!(Start::Nothing, what); // not applied before the yes
    assert!(g.rename("Max", "Max 2"));
    assert_eq!("Max 2", g.confirm); // follows a rename
    assert!(g.set_at_start(Some("Max 2"))); // yes
    assert_eq!("", g.confirm);
    assert_eq!(Start::Apply, GpuOcLogic::decide_start(true, Some(&g), false).0);
    assert_eq!(Start::SkipCrash, GpuOcLogic::decide_start(true, Some(&g), true).0); // crash guard still holds

    g.set_at_start(None);
    g.confirm = "Daily".into();
    assert!(g.set_at_start(None)); // no
    assert!(g.confirm_profile().is_none());
    g.confirm = "Daily".into();
    assert!(g.delete("Daily"));
    assert_eq!("", g.confirm); // goes with the profile

    let dir = Dir(temp_dir("gp"));
    std::fs::create_dir_all(&dir.0).unwrap();
    let f = dir.0.join("gp.json");
    std::fs::write(&f, "{\"Profiles\":[{\"Name\":\"A\"}],\"Confirm\":\"ghost\"}").unwrap();
    assert_eq!("", GpuProfiles::load(Some(&f)).confirm); // points at no profile: dropped
    std::fs::write(&f, "{\"Profiles\":[{\"Name\":\"A\"},{\"Name\":\"B\"}],\"AtStart\":\"A\",\"Confirm\":\"B\"}").unwrap();
    let l = GpuProfiles::load(Some(&f));
    assert_eq!("A", l.at_start);
    assert!(l.confirm_profile().is_none()); // a mark already chosen wins: nothing to ask
}

// ================================================================================================
// Port-specific checks: the file is the one System.Text.Json writes and reads
// ================================================================================================

#[test]
fn json_has_the_system_text_json_shape() {
    let mut g = GpuProfiles::new();
    g.put("Dé<1>", 100, 500, 10, 0, 0, Some(GpuOc::NVIDIA));
    g.set_at_start(Some("Dé<1>"));
    g.record_crash(g.find("Dé<1>").cloned().as_ref(), crash());
    let nl = if cfg!(windows) { "\r\n" } else { "\n" };
    let expect = [
        "{",
        "  \"Profiles\": [",
        "    {",
        "      \"Name\": \"D\\u00E9\\u003C1\\u003E\",",
        "      \"CoreMHz\": 100,",
        "      \"MemMHz\": 500,",
        "      \"VoltPct\": 10,",
        "      \"PowerPct\": 0,",
        "      \"VoltMv\": 0,",
        "      \"Vendor\": \"nvidia\",",
        "      \"IsStock\": false,",
        "      \"Maker\": \"nvidia\"",
        "    }",
        "  ],",
        "  \"AtStart\": \"D\\u00E9\\u003C1\\u003E\",",
        "  \"Skipped\": \"\",",
        "  \"SkippedUtc\": \"0001-01-01T00:00:00\",",
        "  \"Crashes\": [",
        "    {",
        "      \"Name\": \"D\\u00E9\\u003C1\\u003E\",",
        "      \"CoreMHz\": 100,",
        "      \"MemMHz\": 500,",
        "      \"VoltPct\": 10,",
        "      \"PowerPct\": 0,",
        "      \"Vendor\": \"nvidia\",",
        "      \"Utc\": \"2026-11-19T17:10:00Z\"",
        "    }",
        "  ],",
        "  \"Confirm\": \"\",",
        "  \"StartProfile\": {",
        "    \"Name\": \"D\\u00E9\\u003C1\\u003E\",",
        "    \"CoreMHz\": 100,",
        "    \"MemMHz\": 500,",
        "    \"VoltPct\": 10,",
        "    \"PowerPct\": 0,",
        "    \"VoltMv\": 0,",
        "    \"Vendor\": \"nvidia\",",
        "    \"IsStock\": false,",
        "    \"Maker\": \"nvidia\"",
        "  },",
        "  \"ConfirmProfile\": null",
        "}",
    ]
    .join(nl);
    assert_eq!(expect, g.to_json());
    let back = GpuProfiles::parse(&g.to_json());
    assert_eq!(g, back);
    // Empty lists, case-sensitive names, a null for a number fails the whole file, a later duplicate wins.
    assert!(GpuProfiles::new().to_json().contains("\"Profiles\": [],"));
    assert!(GpuProfiles::parse("{\"profiles\":[{\"Name\":\"A\"}]}").profiles.is_empty());
    assert!(GpuProfiles::parse("{\"Profiles\":[{\"Name\":\"A\",\"CoreMHz\":null}]}").profiles.is_empty());
    assert!(GpuProfiles::parse("{\"Profiles\":[{\"Name\":\"A\",\"CoreMHz\":1.5}]}").profiles.is_empty());
    assert_eq!(7, GpuProfiles::parse("{\"Profiles\":[{\"Name\":\"A\",\"CoreMHz\":1,\"CoreMHz\":7}]}").profiles[0].core_mhz);
    assert_eq!(1, GpuProfiles::parse("{\"Profiles\":[{\"Name\":\"A\",\"IsStock\":true,\"Maker\":3}]}").profiles.len());
    assert!(GpuProfiles::parse("null").profiles.is_empty());
}
