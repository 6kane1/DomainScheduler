//! The settings in the registry (only strings), port of `RegistrySettingsBackend` (Settings.cs). Two places
//! (1.15.0 Test 1.3):
//!  - HKCU\Software\DomainScheduler: preferences of the window (language, position, charts, the watchlist...), which
//!    only change how the program looks or which games it may handle;
//!  - HKLM\SOFTWARE\Domain OS\Toolbox: the feature flags and every value that makes the program, with administrator
//!    rights, change something by itself at start (the GPU overclock at start, the one-shot device clean-ups).
//!    Only administrators can write there, so a program running without rights cannot switch them.
//! `Settings::is_machine` decides which goes where.

use super::reg::{Hive, Key, RegValue};
use crate::base::settings::{ISettingsBackend, Settings};
use crate::base::time::DateTime;

pub const ROOT: &str = r"Software\DomainScheduler";
pub const LEGACY_ROOT: &str = r"Software\DomainScheduler.Legacy";
pub const MACHINE_ROOT: &str = r"SOFTWARE\Domain OS\Toolbox";

fn path(sub: &str) -> String {
    if sub.is_empty() { ROOT.to_string() } else { format!("{}\\{}", ROOT, sub) }
}

fn machine_path(sub: &str) -> String {
    if sub.is_empty() { MACHINE_ROOT.to_string() } else { format!("{}\\{}", MACHINE_ROOT, sub) }
}

fn place(sub: &str, name: Option<&str>) -> (Hive, String) {
    if Settings::is_machine(sub, name) { (Hive::LocalMachine, machine_path(sub)) } else { (Hive::CurrentUser, path(sub)) }
}

/// `RegistrySettingsBackend`.
#[derive(Default)]
pub struct RegistrySettingsBackend;

impl ISettingsBackend for RegistrySettingsBackend {
    fn try_get(&self, sub: &str, name: &str) -> Option<String> {
        let (hive, p) = place(sub, Some(name));
        let k = Key::open(hive, &p, false)?;
        k.get(name).map(|v| v.to_csharp_string())
    }

    fn set(&self, sub: &str, name: &str, value: &str) -> Result<(), String> {
        let (hive, p) = place(sub, Some(name));
        let k = Key::create(hive, &p)?;
        k.set(name, &RegValue::Sz(value.to_string()))
    }

    fn delete(&self, sub: &str, name: &str) -> Result<(), String> {
        let (hive, p) = place(sub, Some(name));
        match Key::open(hive, &p, true) {
            Some(k) => k.delete_value(name, false),
            None => Ok(()),
        }
    }

    fn names(&self, sub: &str) -> Vec<String> {
        let (hive, p) = place(sub, None);
        Key::open(hive, &p, false).map(|k| k.value_names()).unwrap_or_default()
    }
}

impl RegistrySettingsBackend {
    /// Once per Windows account: the values that moved to HKLM are copied over from this account's HKCU. Only known
    /// names with a valid value (`Settings::machine_value_valid`) are taken, and only where HKLM has none yet; the
    /// HKCU copies stay (an older version reads them) but are never read again. Returns lines for the log.
    pub fn migrate_to_machine(account: &str) -> Result<Vec<String>, String> {
        let mut log = Vec::new();
        let lm = Key::create(Hive::LocalMachine, MACHINE_ROOT)?;
        let marker = format!("ImportedFrom-{}", account);
        if lm.get(&marker).is_some() {
            return Ok(log);
        }
        let cu = Key::open(Hive::CurrentUser, ROOT, false);
        let read = |sub: &str| -> Vec<(String, String)> {
            let Some(cu) = cu.as_ref() else { return Vec::new() };
            let key = if sub.is_empty() { None } else { cu.open_sub(sub, false) };
            let k = if sub.is_empty() { Some(cu) } else { key.as_ref() };
            let Some(k) = k else { return Vec::new() };
            // ToDictionary(StringComparer.OrdinalIgnoreCase): value names are unique in a key anyway.
            k.value_names().into_iter().map(|n| {
                let v = k.get(&n).map(|v| v.to_csharp_string()).unwrap_or_default();
                (n, v)
            }).collect()
        };
        let has = |sub: &str, name: &str| -> bool { Key::open(Hive::LocalMachine, &machine_path(sub), false).and_then(|k| k.get(name)).is_some() };
        let plan = Settings::machine_import(&read, &has, Some(&mut log));
        for (sub, name, value) in plan {
            let k = Key::create(Hive::LocalMachine, &machine_path(&sub))?;
            k.set(&name, &RegValue::Sz(value))?;
        }
        lm.set(&marker, &RegValue::Sz(DateTime::utc_now().to_o()))?;
        Ok(log)
    }

    /// Copies the legacy root (values with their kinds, subkeys recursively) once.
    pub fn migrate_from_legacy() -> Result<i32, String> {
        let Some(old) = Key::open(Hive::CurrentUser, LEGACY_ROOT, false) else { return Ok(0) };
        let cur = Key::create(Hive::CurrentUser, ROOT)?;
        if let Some(RegValue::Sz(m)) = cur.get("Migrated") {
            if m == "true" {
                return Ok(0);
            }
        }
        let n = copy(&old, &cur)?;
        cur.set("Migrated", &RegValue::Sz("true".into()))?;
        Ok(n)
    }
}

fn copy(from: &Key, to: &Key) -> Result<i32, String> {
    let mut n = 0;
    for name in from.value_names() {
        if to.get(&name).is_some() {
            continue;
        }
        if let Some(v) = from.get(&name) {
            to.set(&name, &v)?;
            n += 1;
        }
    }
    for sub in from.subkey_names() {
        if let Some(f) = from.open_sub(&sub, false) {
            let t = to.create_sub(&sub)?;
            n += copy(&f, &t)?;
        }
    }
    Ok(n)
}
