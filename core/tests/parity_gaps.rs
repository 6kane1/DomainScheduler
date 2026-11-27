//! Integration area: C# tests that no area had ported with the C# inputs (found in the parity review).
//! Run with `cargo test --features testsupport --test parity_gaps` (build.sh runs it).
#![cfg(feature = "testsupport")]

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use dos_core::base::hooks::static_hooks;
use dos_core::base::time::DateTime;
use dos_core::engine::auto_logic::AutoLogic;
use dos_core::topo::game_mask::{BUSY_HOOK, IRQ_HOOK};
use dos_core::topo::reservation::{IReservedRegistry, Reservation, BACKUP_PATH_HOOK, BOOT_UTC_HOOK};
use dos_core::topo::synth;

/// `FakeReserved` of SystemLayerTests.cs, with its `Lie` switch (the crate's FakeReserved has none).
#[derive(Default)]
struct FakeReserved {
    value: Mutex<Option<Vec<u8>>>,
    lie: Mutex<bool>,
}

impl IReservedRegistry for FakeReserved {
    fn read(&self) -> Result<Option<Vec<u8>>, String> {
        Ok(self.value.lock().unwrap().clone())
    }
    fn write(&self, v: &[u8]) -> Result<(), String> {
        *self.value.lock().unwrap() = Some(if *self.lie.lock().unwrap() { vec![0u8; 8] } else { v.to_vec() });
        Ok(())
    }
    fn delete(&self) -> Result<(), String> {
        *self.value.lock().unwrap() = None;
        Ok(())
    }
}

/// The ReservationTests fixture: a fake registry and a backup file in a fresh temporary folder.
struct Fixture {
    reg: Arc<FakeReserved>,
    dir: PathBuf,
    backup: PathBuf,
}

impl Fixture {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("ds-res-{}-{}", tag, std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let reg = Arc::new(FakeReserved::default());
        Reservation::set_reg(Some(reg.clone()));
        let backup = dir.join("reserved.bak");
        let b2 = backup.clone();
        BACKUP_PATH_HOOK.set(Some(Arc::new(move || b2.clone())));
        Fixture { reg, dir, backup }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        Reservation::set_reg(None);
        BACKUP_PATH_HOOK.set(None);
        BOOT_UTC_HOOK.set(None);
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// ReservationTests.Refusals
#[test]
fn reservation_refusals() {
    let _g = static_hooks();
    let _f = Fixture::new("ref");
    let x = synth::x7950x3d();
    assert!(Reservation::refusals(Some(&x), 0xFFFF0000).is_empty());
    assert!(!Reservation::refusals(Some(&x), 0xFFFF0001).is_empty()); // CPU0
    assert!(!Reservation::refusals(Some(&x), 0x10000).is_empty()); // half an SMT core, not the whole half
    assert!(!Reservation::refusals(Some(&x), 0xFFFF).is_empty()); // the game half
    assert!(!Reservation::refusals(Some(&x), 0).is_empty());
    assert!(!Reservation::refusals(Some(&synth::x7950x()), 0xFFFF0000).is_empty());
    assert!(!Reservation::refusals(Some(&synth::x7800x3d()), 0xFF00).is_empty());
    assert!(Reservation::refusals(Some(&synth::i13900k()), 0xFFFF0000).is_empty());
    assert!(!Reservation::refusals(None, 1).is_empty());
}

/// ReservationTests.UnmixAndMixes
#[test]
fn reservation_unmix_and_mixes() {
    assert_eq!(0xFFFFu64, Reservation::unmix(0xFFFFFFFF, 0xFFFF0000, false));
    assert_eq!(0xFFFF0000u64, Reservation::unmix(0xFFFFFFFF, 0xFFFF0000, true));
    assert_eq!(0xFF0000u64, Reservation::unmix(0xFF0000, 0xFFFF0000, false));
    assert!(Reservation::mixes(0xFFFFFFFF, 0xFFFF0000));
    assert!(!Reservation::mixes(0xFFFF, 0xFFFF0000));
    assert!(!Reservation::mixes(0xFF, 0));
}

/// ReservationTests.WriteBackupReadBackPendingRevert
#[test]
fn reservation_write_backup_read_back_pending_revert() {
    let _g = static_hooks();
    let f = Fixture::new("wb");
    *f.reg.value.lock().unwrap() = Some(vec![1, 2, 0, 0, 0, 0, 0, 0]);
    BOOT_UTC_HOOK.set(Some(Arc::new(|| DateTime::utc_now().add_hours(-1.0))));
    assert_eq!(None, Reservation::apply_non_game(Some(&synth::x7950x3d())));
    assert!(f.backup.exists());
    assert_eq!(Some(0xFFFF0000u64.to_le_bytes().to_vec()), *f.reg.value.lock().unwrap());
    let st = Reservation::read();
    assert!(st.pending);
    assert!(st.has_backup);
    assert_eq!("0102000000000000", st.original);
    BOOT_UTC_HOOK.set(Some(Arc::new(|| DateTime::utc_now().add_seconds(5.0)))); // rebooted since
    assert!(!Reservation::read().pending);
    assert_eq!(None, Reservation::revert());
    assert_eq!(Some(vec![1, 2, 0, 0, 0, 0, 0, 0]), *f.reg.value.lock().unwrap());
    assert!(!f.backup.exists());
    // A value that does not read back is reported.
    *f.reg.lie.lock().unwrap() = true;
    assert!(Reservation::apply_non_game(Some(&synth::x7950x3d())).is_some());
    // No split: refused before anything is written.
    *f.reg.lie.lock().unwrap() = false;
    *f.reg.value.lock().unwrap() = None;
    assert!(Reservation::apply_non_game(Some(&synth::x7800x3d())).is_some());
    assert_eq!(None, *f.reg.value.lock().unwrap());
}

/// RoadmapTests.AutoUsesLiveCoresWhenReadyAndThePinsBefore
#[test]
fn auto_uses_live_cores_when_ready_and_the_pins_before() {
    let _g = static_hooks();
    let x = synth::x7800x3d();
    IRQ_HOOK.set(Some(Arc::new(|| vec![("GPU".to_string(), 0x5000), ("Mouse".to_string(), 0x4)])));
    BUSY_HOOK.set(None);
    let pins = AutoLogic::build_mask(&x, 0);
    BUSY_HOOK.set(Some(Arc::new(|_| None))); // live on, not ready yet: same as the pins
    assert_eq!(pins.mask, AutoLogic::build_mask(&x, 0).mask);
    BUSY_HOOK.set(Some(Arc::new(|_| Some(0xF000)))); // ready: whole cores 6 and 7 out, the pins no longer used
    assert_eq!(0x0FFFu64, AutoLogic::build_mask(&x, 0).mask);
    BUSY_HOOK.set(None);
    IRQ_HOOK.set(None);
}
