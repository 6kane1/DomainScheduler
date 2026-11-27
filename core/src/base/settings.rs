//! Application settings (section 9, port of `Settings.cs`): string values behind a backend (the registry in the
//! product, memory in tests). The registry backend itself (`RegistrySettingsBackend`, HKCU / HKLM) lives in
//! `native::settings_backend`, because it calls Windows.
//!
//! In the product (Auto, section 17) only the UI language, the window geometry and the autostart flag are read;
//! every read is counted so tests can prove the Auto path reads nothing else.

use super::ci::{self, CiMap, CiMapExt, CiSet, CiSetExt};
use super::flags::Flags;
use super::hooks::{lock, Hook};
use super::l::L;
use super::time::DateTime;
use crate::lf;
use std::collections::HashMap;
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::{Arc, Mutex, OnceLock, RwLock};

/// String storage behind Settings (the registry, or memory in tests).
pub trait ISettingsBackend: Send + Sync {
    /// The value when it exists (maybe empty).
    fn try_get(&self, sub: &str, name: &str) -> Option<String>;
    /// Err: it could not be written (C# throws).
    fn set(&self, sub: &str, name: &str, value: &str) -> Result<(), String>;
    fn delete(&self, sub: &str, name: &str) -> Result<(), String>;
    fn names(&self, sub: &str) -> Vec<String>;
}

/// In-memory backend (tests).
#[derive(Default)]
pub struct MemorySettingsBackend {
    v: Mutex<CiMap<String>>,
}

impl MemorySettingsBackend {
    pub fn new() -> Self {
        Self::default()
    }
}

impl ISettingsBackend for MemorySettingsBackend {
    fn try_get(&self, sub: &str, name: &str) -> Option<String> {
        lock(&self.v).ci_get(&format!("{}\\{}", sub, name)).cloned()
    }
    fn set(&self, sub: &str, name: &str, value: &str) -> Result<(), String> {
        lock(&self.v).ci_insert(&format!("{}\\{}", sub, name), value.to_string());
        Ok(())
    }
    fn delete(&self, sub: &str, name: &str) -> Result<(), String> {
        lock(&self.v).ci_remove(&format!("{}\\{}", sub, name));
        Ok(())
    }
    fn names(&self, sub: &str) -> Vec<String> {
        let prefix = format!("{}\\", sub);
        lock(&self.v)
            .keys()
            .filter(|k| ci::starts_with(k.as_str(), &prefix))
            .map(|k| k.as_str().chars().skip(prefix.chars().count()).collect())
            .collect()
    }
}

static BACKEND: RwLock<Option<Arc<dyn ISettingsBackend>>> = RwLock::new(None);

fn default_backend() -> Arc<dyn ISettingsBackend> {
    #[cfg(windows)]
    {
        Arc::new(crate::native::settings_backend::RegistrySettingsBackend)
    }
    #[cfg(not(windows))]
    {
        // Not Windows (tests): no registry; a memory store in its place.
        Arc::new(MemorySettingsBackend::new())
    }
}

/// Puts the previous backend back when dropped (tests: `let _b = Settings::scoped_backend(...)`).
pub struct BackendGuard(Option<Arc<dyn ISettingsBackend>>);

impl Drop for BackendGuard {
    fn drop(&mut self) {
        *BACKEND.write().unwrap_or_else(|e| e.into_inner()) = self.0.take();
    }
}

/// The ReapplyMs default: ThreadIni.LoadBundled().Sleep. The engine area may install its own reader; without one the
/// [SETTINGS] sleep of the bundled threads.ini is read here with the same rule.
pub static REAPPLY_DEFAULT_HOOK: Hook<dyn Fn() -> i32 + Send + Sync> = Hook::new();

const THREADS_INI: &str = include_str!("../../res/lang/threads.ini");

/// The [SETTINGS] sleep=<ms> of the bundled threads.ini (ThreadIni.Parse: only that key; default 10000; at least 1000).
fn bundled_sleep() -> i32 {
    static V: OnceLock<i32> = OnceLock::new();
    *V.get_or_init(|| {
        let mut sleep = 10000;
        let mut sec = String::new();
        for raw in THREADS_INI.split('\n') {
            let line = raw.trim().trim_start_matches('\u{feff}');
            if line.is_empty() || line.starts_with(';') || line.starts_with('#') {
                continue;
            }
            if line.starts_with('[') && line.ends_with(']') {
                sec = line[1..line.len() - 1].trim().to_string();
                continue;
            }
            let (key, val) = match line.find('=') {
                Some(p) => (line[..p].trim(), line[p + 1..].trim()),
                None => (line, ""),
            };
            if ci::eq(&sec, "SETTINGS") && ci::eq(key, "sleep") {
                let v = val.split(';').next().unwrap_or("").trim();
                if !v.is_empty() && v.chars().all(|c| c.is_ascii_digit()) {
                    if let Ok(ms) = v.parse::<i32>() {
                        if ms > 0 {
                            sleep = ms.max(1000);
                        }
                    }
                }
            }
        }
        sleep
    })
}

static READS: Mutex<Option<CiMap<i32>>> = Mutex::new(None);
static LOAD_AFF_RULES_CALLS: AtomicI32 = AtomicI32::new(0);

/// Values kept in HKLM (administrators only): they make the program act by itself with administrator rights.
/// The whole Flags subkey goes there too.
pub const MACHINE_KEYS: [&str; 7] = ["GpuOcAtStart", "GpuOcCoreKHz", "GpuOcMemKHz", "GpuOcVoltPct", "GpuOcAppliedUtc", "SafeDeviceLayout112", "LegacyRawMouseChecked"];

/// Keys the product reads (section 17.1).
pub const PRODUCT_KEYS: [&str; 4] = ["Language", "WindowLeft", "WindowTop", "Autostart"];

/// `Settings` (static class).
pub struct Settings;

impl Settings {
    pub const AFFINITY_SUB: &'static str = "Affinity";

    /// The current backend.
    pub fn backend() -> Arc<dyn ISettingsBackend> {
        if let Some(b) = BACKEND.read().unwrap_or_else(|e| e.into_inner()).as_ref() {
            return b.clone();
        }
        let mut w = BACKEND.write().unwrap_or_else(|e| e.into_inner());
        w.get_or_insert_with(default_backend).clone()
    }

    /// `Settings.Backend = b`.
    pub fn set_backend(b: Arc<dyn ISettingsBackend>) {
        *BACKEND.write().unwrap_or_else(|e| e.into_inner()) = Some(b);
    }

    /// Sets the backend until the guard is dropped (tests).
    pub fn scoped_backend(b: Arc<dyn ISettingsBackend>) -> BackendGuard {
        let old = BACKEND.write().unwrap_or_else(|e| e.into_inner()).replace(b);
        BackendGuard(old)
    }

    pub fn machine_keys() -> CiSet {
        ci::set_of(MACHINE_KEYS)
    }

    pub fn is_machine(sub: &str, name: Option<&str>) -> bool {
        ci::eq(sub, Flags::SUB) || (sub.is_empty() && name.map(|n| MACHINE_KEYS.iter().any(|k| ci::eq(k, n))).unwrap_or(false))
    }

    /// Whether a value read from HKCU may be copied to HKLM (pure). Flags: a known flag, "true"/"false". The one-shot
    /// markers: only "true" (a planted "false" would make an old clean-up run again). The GPU values: integers in the
    /// range the program writes and a time stamp. GpuOcAtStart is never taken: an overclock applied at start has to be
    /// chosen again in the program (it cannot be told from one planted by another program).
    pub fn machine_value_valid(sub: &str, name: &str, value: &str) -> bool {
        if ci::eq(sub, Flags::SUB) {
            return Flags::FLAG_KEYS.contains(&name) && (value == "true" || value == "false");
        }
        if !sub.is_empty() {
            return false;
        }
        let int = |lo: i64, hi: i64| parse_int_leading_sign(value).map(|v| v >= lo && v <= hi).unwrap_or(false);
        match name {
            "SafeDeviceLayout112" | "LegacyRawMouseChecked" => value == "true",
            "GpuOcCoreKHz" => int(-1_000_000, 1_000_000),
            "GpuOcMemKHz" => int(-3_000_000, 3_000_000),
            "GpuOcVoltPct" => int(0, 100),
            "GpuOcAppliedUtc" => value.is_empty() || DateTime::parse_iso(value, false).is_some(),
            _ => false,
        }
    }

    /// What `RegistrySettingsBackend::migrate_to_machine` copies (pure): `read(sub)` gives the HKCU values of a
    /// subkey (in their order), `machine_has(sub, name)` says whether HKLM has the value already.
    pub fn machine_import(
        read: &dyn Fn(&str) -> Vec<(String, String)>,
        machine_has: &dyn Fn(&str, &str) -> bool,
        mut log: Option<&mut Vec<String>>,
    ) -> Vec<(String, String, String)> {
        let mut plan = Vec::new();
        for sub in ["", Flags::SUB] {
            for (name, value) in read(sub) {
                if !Self::is_machine(sub, Some(&name)) {
                    continue;
                }
                if !Self::machine_value_valid(sub, &name, &value) {
                    if let Some(l) = log.as_deref_mut() {
                        let shown = if sub.is_empty() { name.clone() } else { format!("{}\\{}", sub, name) };
                        l.push(lf!("Settings: {0} was not taken over from the user settings (not a value the program writes)", shown));
                    }
                    continue;
                }
                if machine_has(sub, &name) {
                    continue;
                }
                plan.push((sub.to_string(), name, value));
            }
        }
        plan
    }

    pub fn product_keys() -> CiSet {
        ci::set_of(PRODUCT_KEYS)
    }

    /// The built-in default of a key, or None (no default: "").
    fn default_of(name: &str) -> Option<String> {
        let n = ci::lower(name);
        Some(match n.as_str() {
            "maskname" => Self::default_mask(),
            "irqmode" => "Off".into(), // 1.16.0: owner's setting (manual mask rules do not step around pinned devices)
            "domask" | "dothreads" | "dobackground" | "bgaffinity" => "true".into(),
            "reapplyms" => match REAPPLY_DEFAULT_HOOK.get() {
                Some(f) => f().to_string(),
                None => bundled_sleep().to_string(),
            },
            "syspresets" | "bgon" => "true".into(),
            "top5on" => "true".into(), // no longer read since 1.16.0 Test 1.9: Top-5 is always on
            "gpuoccorekhz" | "gpuocmemkhz" | "gpuocvoltpct" => "0".into(),
            "gpuocatstart" => "false".into(),
            "gpuocappliedutc" => "".into(),
            "softcloseon" => "false".into(),
            "closablelist" => "".into(),
            "timerreson" => "true".into(),
            "etwirq" => "false".into(),
            "watchlist" => "cs2.exe".into(),
            "masknosmt" | "masknocore0" => "false".into(),
            "gameclass" => "above".into(),
            "allowelevatedpriority" => "false".into(),
            "autostart" => "true".into(),
            "advanced" => "true".into(), // 1.16.0: the advanced layout by default (owner's setting)
            "language" => L::detect(),
            _ => return None,
        })
    }

    fn default_mask() -> String {
        match crate::topo::builder::TopoBuilder::live() {
            None => "auto".into(),
            Some(t) => {
                if t.split().can_split() && t.split().family == "x3d" {
                    "vcache".into()
                } else if t.hybrid() {
                    "pcores".into()
                } else {
                    "auto".into()
                }
            }
        }
    }

    fn count(name: &str) {
        let mut g = lock(&READS);
        let m = g.get_or_insert_with(CiMap::new);
        let v = m.ci_get(name).copied().unwrap_or(0) + 1;
        m.ci_insert(name, v);
    }

    /// Reads per key since the last reset (tests).
    pub fn reads() -> HashMap<String, i32> {
        lock(&READS).as_ref().map(|m| m.iter().map(|(k, v)| (k.0.clone(), *v)).collect()).unwrap_or_default()
    }

    pub fn reset_reads() {
        *lock(&READS) = None;
    }

    /// `Get(name)`: the non-empty stored value, else the default, else "".
    pub fn get(name: &str) -> String {
        Self::get_or(name, "")
    }

    /// `Get(name, fallback)`: the non-empty stored value, else the fallback (when not empty), else the default, else "".
    pub fn get_or(name: &str, fallback: &str) -> String {
        Self::count(name);
        if let Some(v) = Self::backend().try_get("", name) {
            if !v.is_empty() {
                return v;
            }
        }
        if !fallback.is_empty() {
            return fallback.to_string();
        }
        Self::default_of(name).unwrap_or_default()
    }

    /// `GetBool(name)` (fallback false).
    pub fn get_bool(name: &str) -> bool {
        Self::get_bool_or(name, false)
    }

    pub fn get_bool_or(name: &str, fallback: bool) -> bool {
        let s = Self::get(name);
        if s.is_empty() { fallback } else { ci::eq(&s, "true") }
    }

    pub fn get_int(name: &str, fallback: i32) -> i32 {
        parse_int_integer_style(&Self::get_or(name, &fallback.to_string())).unwrap_or(fallback)
    }

    pub fn set(name: &str, value: &str) -> Result<(), String> {
        Self::backend().set("", name, value)
    }

    pub fn reset(name: &str) -> Result<(), String> {
        Self::backend().delete("", name)
    }

    /// The stored language code when there is a resource for it, else the detected one.
    pub fn language() -> String {
        let code = Self::backend().try_get("", "Language").map(|v| ci::lower(v.trim())).unwrap_or_default();
        if L::AVAILABLE.contains(&code.as_str()) { code } else { L::detect() }
    }

    /// The watchlist: the only processes whose scheduling the user asked to change.
    pub fn watch() -> Vec<String> {
        Self::watch_list(&Self::get("WatchList"))
    }

    pub fn set_watch<S: AsRef<str>>(exes: &[S]) -> Result<(), String> {
        let joined = exes.iter().map(|s| s.as_ref()).collect::<Vec<_>>().join(",");
        Self::set("WatchList", &Self::watch_list(&joined).join(", "))
    }

    pub fn set_bool(name: &str, value: bool) -> Result<(), String> {
        Self::set(name, if value { "true" } else { "false" })
    }

    /// The value exists (even empty): one-shot defaults hang on this.
    pub fn try_stored(name: &str) -> bool {
        Self::backend().try_get("", name).is_some()
    }

    /// The value exists and is not empty.
    pub fn stored(name: &str) -> bool {
        Self::backend().try_get("", name).map(|v| !v.is_empty()).unwrap_or(false)
    }

    /// "game, other.exe; game" -> ["game.exe", "other.exe"].
    pub fn watch_list(text: &str) -> Vec<String> {
        let mut seen = CiSet::new();
        let mut list = Vec::new();
        for raw in text.split([',', ';', '+']) {
            let raw = raw.trim();
            if raw.is_empty() {
                continue;
            }
            let exe = if ci::ends_with(raw, ".exe") { raw.to_string() } else { format!("{}.exe", raw) };
            if seen.ci_add(&exe) {
                list.push(exe);
            }
        }
        list
    }

    /// How often `load_aff_rules` ran (tests).
    pub fn load_aff_rules_calls() -> i32 {
        LOAD_AFF_RULES_CALLS.load(Ordering::SeqCst)
    }

    /// Per-game mask rules (subkey Affinity, value name = game.exe).
    pub fn load_aff_rules() -> CiMap<String> {
        LOAD_AFF_RULES_CALLS.fetch_add(1, Ordering::SeqCst);
        let b = Self::backend();
        let mut d = CiMap::new();
        for n in b.names(Self::AFFINITY_SUB) {
            if let Some(v) = b.try_get(Self::AFFINITY_SUB, &n) {
                if !v.is_empty() {
                    d.ci_insert(&n, v);
                }
            }
        }
        d
    }

    pub fn save_aff_rule(exe: &str, rule: &str) -> Result<(), String> {
        if rule.is_empty() {
            Self::backend().delete(Self::AFFINITY_SUB, exe)
        } else {
            Self::backend().set(Self::AFFINITY_SUB, exe, rule)
        }
    }
}

/// `int.TryParse(s, NumberStyles.AllowLeadingSign, InvariantCulture)`: an optional sign and digits, nothing else.
pub fn parse_int_leading_sign(s: &str) -> Option<i64> {
    let (neg, digits) = match s.as_bytes().first() {
        Some(b'-') => (true, &s[1..]),
        Some(b'+') => (false, &s[1..]),
        _ => (false, s),
    };
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let v: i64 = digits.parse::<i64>().ok()?;
    let v = if neg { -v } else { v };
    if v < i32::MIN as i64 || v > i32::MAX as i64 {
        return None;
    }
    Some(v)
}

/// `int.TryParse(s, NumberStyles.Integer, InvariantCulture, out int)`: whitespace around, an optional sign, digits.
pub fn parse_int_integer_style(s: &str) -> Option<i32> {
    let t = s.trim_matches(|c: char| matches!(c, ' ' | '\t' | '\n' | '\r' | '\u{b}' | '\u{c}'));
    parse_int_leading_sign(t).map(|v| v as i32)
}

/// `int.TryParse(s, NumberStyles.None, InvariantCulture, out int)`: digits only.
pub fn parse_int_none(s: &str) -> Option<i32> {
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    s.parse::<i32>().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::base::hooks::static_hooks;

    /// SecurityTests.MachineSettingsTakeOverOnlyKnownValidValues
    #[test]
    fn machine_settings_take_over_only_known_valid_values() {
        assert!(Settings::is_machine(Flags::SUB, Some("gpu-oc")));
        assert!(Settings::is_machine("", Some("SafeDeviceLayout112")));
        assert!(Settings::is_machine("", Some("GpuOcAtStart")));
        assert!(!Settings::is_machine("", Some("Language")));
        assert!(!Settings::is_machine("", Some("WatchList")));

        let user: HashMap<&str, Vec<(String, String)>> = HashMap::from([
            (
                "",
                vec![
                    ("Language".into(), "ru".into()),
                    ("SafeDeviceLayout112".into(), "true".into()),
                    ("LegacyRawMouseChecked".into(), "false".into()),
                    ("GpuOcAtStart".into(), "true".into()),
                    ("GpuOcCoreKHz".into(), "300000".into()),
                    ("GpuOcVoltPct".into(), "500".into()),
                ],
            ),
            (
                Flags::SUB,
                vec![
                    ("devices-write".into(), "true".into()),
                    ("gpu-oc".into(), "maybe".into()),
                    ("evil".into(), "true".into()),
                    ("etwirq".into(), "false".into()),
                ],
            ),
        ]);
        let mut log = Vec::new();
        let plan = Settings::machine_import(&|sub| user[sub].clone(), &|_sub, name| name == "etwirq", Some(&mut log));
        let has = |s: &str, n: &str, v: &str| plan.iter().any(|p| p.0 == s && p.1 == n && p.2 == v);
        assert!(has("", "SafeDeviceLayout112", "true"));
        assert!(has("", "GpuOcCoreKHz", "300000"));
        assert!(has(Flags::SUB, "devices-write", "true"));
        assert!(!plan.iter().any(|p| ["Language", "LegacyRawMouseChecked", "GpuOcAtStart", "GpuOcVoltPct", "gpu-oc", "evil", "etwirq"].contains(&p.1.as_str())));
        assert!(log.len() >= 4);
    }

    /// LatencyDefaultsTests.TheLatencyExperimentsHaveNoSettingAndNoToggle (the default-table half: no default is left;
    /// the source scans are of the C# sources).
    #[test]
    fn the_latency_experiments_have_no_default() {
        let _g = static_hooks();
        let _b = Settings::scoped_backend(Arc::new(MemorySettingsBackend::new()));
        for key in ["GameNoIdle", "SoftBackground", "HighBoost"] {
            assert!(!Settings::get_bool(key));
        }
    }

    #[test]
    fn defaults_reads_and_lists() {
        let _g = static_hooks();
        let _b = Settings::scoped_backend(Arc::new(MemorySettingsBackend::new()));
        Settings::reset_reads();
        assert_eq!(Settings::get("IrqMode"), "Off");
        assert_eq!(Settings::get("ReapplyMs"), "10000");
        assert_eq!(Settings::get_int("ReapplyMs", 5), 5); // C#: the fallback text wins over the default
        assert_eq!(Settings::get_int("ReapplyMs", 0), 0);
        assert_eq!(Settings::get("WatchList"), "cs2.exe");
        assert_eq!(Settings::get_or("Nothing", "fb"), "fb");
        assert_eq!(Settings::get("Nothing"), "");
        assert!(Settings::get_bool("Autostart"));
        assert_eq!(Settings::reads()["IrqMode"], 1);
        assert_eq!(Settings::watch_list("game, other.exe; GAME.exe + game"), vec!["game.exe", "other.exe"]);
        Settings::set_watch(&["a", "b.exe", "A.EXE"]).unwrap();
        assert_eq!(Settings::get("WatchList"), "a.exe, b.exe");
        assert!(!Settings::try_stored("X"));
        Settings::set("X", "").unwrap();
        assert!(Settings::try_stored("X"));
        assert!(!Settings::stored("X"));
        Settings::save_aff_rule("cs2.exe", "vcache").unwrap();
        Settings::save_aff_rule("x.exe", "0xff").unwrap();
        Settings::save_aff_rule("x.exe", "").unwrap();
        let r = Settings::load_aff_rules();
        assert_eq!(r.len(), 1);
        assert_eq!(r.ci_get("CS2.EXE").unwrap(), "vcache");
        Settings::set("GetInt", " -12 ").unwrap();
        assert_eq!(Settings::get_int("GetInt", 3), -12);
        Settings::set("GetInt", "x").unwrap();
        assert_eq!(Settings::get_int("GetInt", 3), 3);
    }
}
