//! System-wide reserved cores (section 8, port of `Reservation.cs`): after a reboot Windows keeps ordinary threads
//! off them. Only the exact non-game half of a split processor may be reserved; never in the Auto path.

use super::core_split::CoreSplit;
use super::masks::Masks;
use super::model::Topo;
use crate::base::app_log::{AppLog, Sev};
use crate::base::hooks::Hook;
use crate::base::l::L;
use crate::base::state_dir::store_dir;
use crate::base::time::{DateTime, TimeSpan};
use crate::lf;
use std::path::PathBuf;
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::{Arc, RwLock};

/// The ReservedCpuSets value (8 bytes, little endian); Ok(None) from read = absent. Err: the registry failed
/// (C# throws).
pub trait IReservedRegistry: Send + Sync {
    fn read(&self) -> Result<Option<Vec<u8>>, String>;
    fn write(&self, value: &[u8]) -> Result<(), String>;
    fn delete(&self) -> Result<(), String>;
}

/// HKLM\SYSTEM\CurrentControlSet\Control\Session Manager\Kernel -> ReservedCpuSets.
#[derive(Default)]
pub struct WinReservedRegistry;

#[cfg(windows)]
impl IReservedRegistry for WinReservedRegistry {
    fn read(&self) -> Result<Option<Vec<u8>>, String> {
        use crate::native::reg::{Hive, Key};
        Ok(Key::open(Hive::LocalMachine, KEY_PATH, false).and_then(|k| k.get_binary(NAME)))
    }

    fn write(&self, value: &[u8]) -> Result<(), String> {
        use crate::native::reg::{Hive, Key, RegValue};
        let k = Key::open(Hive::LocalMachine, KEY_PATH, true).ok_or_else(|| L::t("The Session Manager\\Kernel key cannot be opened").to_string())?;
        k.set(NAME, &RegValue::Binary(value.to_vec()))
    }

    fn delete(&self) -> Result<(), String> {
        use crate::native::reg::{Hive, Key};
        match Key::open(Hive::LocalMachine, KEY_PATH, true) {
            Some(k) => k.delete_value(NAME, false),
            None => Ok(()),
        }
    }
}

#[cfg(not(windows))]
impl IReservedRegistry for WinReservedRegistry {
    fn read(&self) -> Result<Option<Vec<u8>>, String> {
        Err("Registry access is not supported on this platform".into())
    }
    fn write(&self, _value: &[u8]) -> Result<(), String> {
        Err("Registry access is not supported on this platform".into())
    }
    fn delete(&self) -> Result<(), String> {
        Err("Registry access is not supported on this platform".into())
    }
}

#[cfg_attr(not(windows), allow(dead_code))]
const KEY_PATH: &str = r"SYSTEM\CurrentControlSet\Control\Session Manager\Kernel";
#[cfg_attr(not(windows), allow(dead_code))]
const NAME: &str = "ReservedCpuSets";

static REG: RwLock<Option<Arc<dyn IReservedRegistry>>> = RwLock::new(None);
/// `Reservation.BootUtc` (tests).
pub static BOOT_UTC_HOOK: Hook<dyn Fn() -> DateTime + Send + Sync> = Hook::new();
/// `Reservation.BackupPathHook` (tests).
pub static BACKUP_PATH_HOOK: Hook<dyn Fn() -> PathBuf + Send + Sync> = Hook::new();
static APPLY_CALLS: AtomicI32 = AtomicI32::new(0);

#[derive(Clone, Debug, Default, PartialEq)]
pub struct State {
    /// absent | set | error
    pub kind: String,
    pub bytes: Option<Vec<u8>>,
    pub mask: u64,
    pub error: String,
    pub has_backup: bool,
    /// backed-up value: hex or "absent"
    pub original: String,
    pub time_known: bool,
    pub applied: DateTime,
    pub pending: bool,
}

fn to_mask(b: Option<&[u8]>) -> u64 {
    let Some(b) = b else { return 0 };
    if b.is_empty() {
        return 0;
    }
    let mut full = [0u8; 8];
    let n = b.len().min(8);
    full[..n].copy_from_slice(&b[..n]);
    u64::from_le_bytes(full)
}

fn hex_upper(b: &[u8]) -> String {
    b.iter().map(|x| format!("{:02X}", x)).collect()
}

fn from_hex(s: &str) -> Result<Vec<u8>, String> {
    if s.len() % 2 != 0 || !s.bytes().all(|c| c.is_ascii_hexdigit()) {
        return Err("The input is not a valid hex string".into());
    }
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).map_err(|e| e.to_string())).collect()
}

/// `Reservation` (static class).
pub struct Reservation;

impl Reservation {
    pub const BACKUP_NAME: &'static str = "reserved-cpusets.bak";

    /// `Reservation.Reg`.
    pub fn reg() -> Arc<dyn IReservedRegistry> {
        if let Some(r) = REG.read().unwrap_or_else(|e| e.into_inner()).as_ref() {
            return r.clone();
        }
        Arc::new(WinReservedRegistry)
    }

    /// `Reservation.Reg = r` (None: the Windows registry again).
    pub fn set_reg(r: Option<Arc<dyn IReservedRegistry>>) {
        *REG.write().unwrap_or_else(|e| e.into_inner()) = r;
    }

    pub fn boot_utc() -> DateTime {
        match BOOT_UTC_HOOK.get() {
            Some(f) => f(),
            None => DateTime::utc_now() - TimeSpan::from_ms(crate::native::sys::tick_count64() as f64),
        }
    }

    pub fn backup_path() -> Result<PathBuf, String> {
        if let Some(h) = BACKUP_PATH_HOOK.get() {
            return Ok(h());
        }
        Ok(store_dir()?.join(Self::BACKUP_NAME))
    }

    pub fn unmix(mask: u64, res: u64, prefer_reserved: bool) -> u64 {
        if res == 0 {
            return mask;
        }
        let inn = mask & res;
        let out = mask & !res;
        if prefer_reserved {
            if inn != 0 { inn } else { mask }
        } else if out != 0 {
            out
        } else {
            mask
        }
    }

    pub fn mixes(m: u64, res: u64) -> bool {
        res != 0 && (m & res) != 0 && (m & !res) != 0
    }

    /// The reserved processors now (0: none or unreadable).
    pub fn reserved() -> u64 {
        match Self::reg().read() {
            Ok(b) => to_mask(b.as_deref()),
            Err(_) => 0,
        }
    }

    /// Why a mask must not be reserved; empty = it may.
    pub fn refusals(t: Option<&Topo>, mask: u64) -> Vec<String> {
        let mut r = Vec::new();
        let Some(t) = t else {
            r.push(L::t("The processor topology is not known").to_string());
            return r;
        };
        if t.groups > 1 {
            r.push(L::t("More than one processor group").to_string());
        }
        if t.logical_count > 64 {
            r.push(L::t("More than 64 logical processors").to_string());
        }
        if mask == 0 {
            r.push(L::t("The mask is empty").to_string());
        }
        if (mask & 1) != 0 {
            r.push(L::t("Processor 0 is never reserved").to_string());
        }
        let sp = t.split();
        if !sp.can_split() {
            r.push(lf!("The processor does not split: {0}", sp.why()));
        }
        if !sp.is_split_family() {
            r.push(L::t("Only X3D with two dies and Intel hybrid are split").to_string());
        }
        if mask != sp.non_game {
            r.push(lf!("Only the whole non-game half ({0}) can be reserved", Masks::ranges(sp.non_game)));
        }
        if (mask & sp.game) != 0 {
            r.push(L::t("The mask holds game cores").to_string());
        }
        if !CoreSplit::smt_whole(t, mask) {
            r.push(L::t("The mask takes only one hardware thread of a core").to_string());
        }
        r
    }

    /// The saved original may be written back (pure): "absent" or the bytes of a ReservedCpuSets value (1-32 bytes,
    /// hex): the value this program read before it reserved anything.
    pub fn valid_original(orig: &str) -> bool {
        if orig == "absent" {
            return true;
        }
        // Uri.IsHexDigit: ASCII hex digits only.
        !orig.is_empty() && orig.chars().count() <= 64 && orig.chars().count() % 2 == 0 && orig.chars().all(|c| c.is_ascii_hexdigit())
    }

    /// Takes over the backup of an older version: the text when its original passes `valid_original`, else None.
    pub fn import_backup(text: &str, dropped: Option<&mut Vec<String>>) -> Option<String> {
        let trimmed = text.trim();
        let orig = trimmed.split('|').next().unwrap_or("");
        if Self::valid_original(orig) {
            return Some(trimmed.to_string());
        }
        if let Some(d) = dropped {
            d.push(if orig.chars().count() > 40 { orig.chars().take(40).collect() } else { orig.to_string() });
        }
        None
    }

    fn read_backup() -> (bool, String, DateTime, DateTime) {
        let r = (|| -> Result<Option<(String, DateTime, DateTime)>, String> {
            let p = Self::backup_path()?;
            if !p.is_file() {
                return Ok(None);
            }
            let text = std::fs::read_to_string(&p).map_err(|e| e.to_string())?;
            let text = text.trim_start_matches('\u{feff}');
            let parts: Vec<&str> = text.trim().split('|').collect();
            let at = parts.get(1).and_then(|s| DateTime::parse_iso(s, true)).unwrap_or_default();
            let applied = parts.get(2).and_then(|s| DateTime::parse_iso(s, true)).unwrap_or_default();
            Ok(Some((parts[0].to_string(), at, applied)))
        })();
        match r {
            Ok(Some((o, a, b))) => (true, o, a, b),
            _ => (false, String::new(), DateTime::MIN, DateTime::MIN),
        }
    }

    fn write_backup(original: &str, backup_at: DateTime, applied: DateTime) -> Result<(), String> {
        let p = Self::backup_path()?;
        if let Some(d) = p.parent() {
            std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
        }
        std::fs::write(&p, format!("{}|{}|{}", original, backup_at.to_o(), applied.to_o())).map_err(|e| e.to_string())
    }

    pub fn read() -> State {
        let mut s = State::default();
        match Self::reg().read() {
            Ok(b) => {
                s.kind = if b.is_none() { "absent".into() } else { "set".into() };
                s.mask = to_mask(b.as_deref());
                s.bytes = b;
            }
            Err(e) => {
                s.kind = "error".into();
                s.error = e;
            }
        }
        let (ok, orig, _, applied) = Self::read_backup();
        s.has_backup = ok;
        s.original = orig;
        s.time_known = ok && applied != DateTime::MIN;
        s.applied = applied;
        s.pending = s.time_known && applied > Self::boot_utc();
        s
    }

    /// How often `apply_non_game` ran (tests: Auto never calls it).
    pub fn apply_calls() -> i32 {
        APPLY_CALLS.load(Ordering::SeqCst)
    }

    /// Reserves the non-game half: refusals -> backup -> write -> byte read-back. Error text or None.
    pub fn apply_non_game(t: Option<&Topo>) -> Option<String> {
        APPLY_CALLS.fetch_add(1, Ordering::SeqCst);
        let mask = t.map(|t| t.split().non_game).unwrap_or(0);
        let refusals = Self::refusals(t, mask);
        if !refusals.is_empty() {
            return Some(refusals.join("; "));
        }
        let r = (|| -> Result<Option<String>, String> {
            let (had, mut orig, mut at, _) = Self::read_backup();
            let reg = Self::reg();
            if !had {
                let cur = reg.read()?;
                orig = match cur {
                    None => "absent".into(),
                    Some(b) => hex_upper(&b),
                };
                at = DateTime::utc_now();
            }
            let want = mask.to_le_bytes();
            Self::write_backup(&orig, at, DateTime::utc_now())?; // the backup exists before the registry is touched
            reg.write(&want)?;
            let got = reg.read()?;
            if got.as_deref() != Some(&want[..]) {
                return Ok(Some(L::t("The value read back differs: the reservation is not confirmed").to_string()));
            }
            Ok(None)
        })();
        match r {
            Ok(x) => x,
            Err(e) => Some(e),
        }
    }

    /// Puts the backed-up value back (or deletes the value) and removes the backup.
    pub fn revert() -> Option<String> {
        let (ok, mut orig, _, _) = Self::read_backup();
        if !ok {
            return Some(L::t("No backup: the program did not reserve cores").to_string());
        }
        let r = (|| -> Result<(), String> {
            if !Self::valid_original(&orig) {
                // Not a value this program could have saved: Windows' default (no reservation) instead of the file's text.
                AppLog::add_sev(L::t("Core reservation: the saved original is not a valid value; the reservation is removed instead"), Sev::Warn);
                orig = "absent".into();
            }
            let reg = Self::reg();
            if orig == "absent" {
                reg.delete()?;
            } else {
                reg.write(&from_hex(&orig)?)?;
            }
            std::fs::remove_file(Self::backup_path()?).or_else(|e| if e.kind() == std::io::ErrorKind::NotFound { Ok(()) } else { Err(e.to_string()) })?;
            Ok(())
        })();
        r.err()
    }
}

/// `FakeReserved` of the C# tests (SystemLayerTests.cs): the value in memory.
#[cfg(any(test, feature = "testsupport"))]
#[derive(Default)]
pub struct FakeReserved {
    pub value: std::sync::Mutex<Option<Vec<u8>>>,
    pub writes: AtomicI32,
}

#[cfg(any(test, feature = "testsupport"))]
impl IReservedRegistry for FakeReserved {
    fn read(&self) -> Result<Option<Vec<u8>>, String> {
        Ok(crate::base::hooks::lock(&self.value).clone())
    }
    fn write(&self, value: &[u8]) -> Result<(), String> {
        self.writes.fetch_add(1, Ordering::SeqCst);
        *crate::base::hooks::lock(&self.value) = Some(value.to_vec());
        Ok(())
    }
    fn delete(&self) -> Result<(), String> {
        *crate::base::hooks::lock(&self.value) = None;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::base::hooks::static_hooks;
    use crate::base::testrand::{iterations, Rand};
    use crate::topo::synth;

    /// SecurityTests.ReservationAndPowerRestoreOnlyTheirOwnValues (the reservation half; GamePower is the engine's)
    #[test]
    fn reservation_restores_only_its_own_values() {
        assert!(Reservation::valid_original("absent"));
        assert!(Reservation::valid_original("0102000000000000"));
        assert!(!Reservation::valid_original("zz"));
        assert!(!Reservation::valid_original(&"0".repeat(200)));
        assert_eq!(None, Reservation::import_backup("evil|2026-01-01T00:00:00Z", Some(&mut Vec::new())));
    }

    /// FuzzTests.ReservedCpuSetOriginalAcceptsOnlyItsFormat
    #[test]
    fn reserved_cpu_set_original_accepts_only_its_format() {
        let mut r = Rand::new(1609);
        for _ in 0..iterations() {
            let s = r.text(80);
            if Reservation::valid_original(&s) {
                let n = s.chars().count();
                assert!(s == "absent" || (n > 0 && n <= 64 && n % 2 == 0 && s.chars().all(|c| c.is_ascii_hexdigit())), "{}", s);
            }
            let mut dropped = Vec::new();
            let imported = Reservation::import_backup(&s, Some(&mut dropped));
            assert!(imported.map(|i| Reservation::valid_original(i.split('|').next().unwrap())).unwrap_or(true));
        }
    }

    /// The apply / read / revert cycle with a fake registry and a temporary backup file (SystemLayerTests' FakeReserved).
    #[test]
    fn apply_read_and_revert_round_trip() {
        let _g = static_hooks();
        let dir = std::env::temp_dir().join(format!("ds-res-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join(Reservation::BACKUP_NAME);
        let f2 = file.clone();
        BACKUP_PATH_HOOK.set(Some(Arc::new(move || f2.clone())));
        let fake = Arc::new(FakeReserved::default());
        Reservation::set_reg(Some(fake.clone()));
        let x = synth::x7950x3d();
        assert_eq!(None, Reservation::apply_non_game(Some(&x)));
        assert_eq!(0xFFFF_0000, Reservation::reserved());
        let st = Reservation::read();
        assert_eq!("set", st.kind);
        assert!(st.has_backup);
        assert_eq!("absent", st.original);
        assert!(st.time_known);
        // A refused mask: the X3D without a split, or no topology.
        assert!(Reservation::apply_non_game(Some(&synth::x7800x3d())).is_some());
        assert!(Reservation::apply_non_game(None).unwrap().contains("not known"));
        assert_eq!(None, Reservation::revert());
        assert_eq!(None, *fake.value.lock().unwrap());
        assert!(!file.exists());
        assert!(Reservation::revert().is_some());
        // A planted original is never written back.
        std::fs::write(&file, "evil|2026-01-01T00:00:00Z|2026-01-01T00:00:00Z").unwrap();
        *fake.value.lock().unwrap() = Some(vec![1, 2]);
        assert_eq!(None, Reservation::revert());
        assert_eq!(None, *fake.value.lock().unwrap());
        Reservation::set_reg(None);
        BACKUP_PATH_HOOK.set(None);
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(0x0F, Reservation::unmix(0xFF, 0x0F, true));
        assert_eq!(0xF0, Reservation::unmix(0xFF, 0x0F, false));
        assert!(Reservation::mixes(0xFF, 0x0F) && !Reservation::mixes(0x0F, 0x0F));
    }
}
