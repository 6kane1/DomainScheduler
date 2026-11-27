//! Feature flags (section 22.12, port of `Flags.cs`): the subkey Flags of the settings, one bool per key, all closed
//! by default except "core", "devices-write" and "gpu-oc" (open since 1.16.0). A closed flag hides the control in the
//! UI and makes the engine refuse the action with a warning.

use super::app_log::{AppLog, Sev};
use super::ci;
use super::hooks::lock;
use super::settings::Settings;
use crate::lf;
use std::collections::HashSet;
use std::sync::Mutex;

static WARNED: Mutex<Option<HashSet<String>>> = Mutex::new(None);

/// `Flags` (static class).
pub struct Flags;

impl Flags {
    pub const SUB: &'static str = "Flags";
    pub const CORE: &'static str = "core";
    pub const DEVICES_WRITE: &'static str = "devices-write";
    pub const SOFT_CLOSE: &'static str = "softclose";
    pub const ETW_IRQ: &'static str = "etwirq";
    pub const TIMER_RES: &'static str = "timerres";
    pub const GPU_OC: &'static str = "gpu-oc";
    pub const FLAG_KEYS: [&'static str; 6] = [Self::CORE, Self::DEVICES_WRITE, Self::SOFT_CLOSE, Self::ETW_IRQ, Self::TIMER_RES, Self::GPU_OC];

    pub fn open(key: &str) -> bool {
        if !Self::FLAG_KEYS.contains(&key) {
            return false;
        }
        if let Some(v) = Settings::backend().try_get(Self::SUB, key) {
            if !v.is_empty() {
                return ci::eq(&v, "true");
            }
        }
        // 1.16.0: the owner's defaults: the device layout and GPU overclocking pages are open (both still ask before
        // writing anything); the other extended actions stay closed.
        key == Self::CORE || key == Self::DEVICES_WRITE || key == Self::GPU_OC
    }

    /// Opens or closes a flag. Err: the settings could not be written (C# throws).
    pub fn set(key: &str, open: bool) -> Result<(), String> {
        if !Self::FLAG_KEYS.contains(&key) {
            return Ok(());
        }
        Settings::backend().set(Self::SUB, key, if open { "true" } else { "false" })?;
        if let Some(w) = lock(&WARNED).as_mut() {
            w.remove(key);
        }
        AppLog::add(if open { lf!("Feature flag {0}: open", key) } else { lf!("Feature flag {0}: closed", key) });
        Ok(())
    }

    /// True when the action must be refused; the refusal is logged (once per flag until it changes).
    pub fn refuse(key: &str, action: &str) -> bool {
        if Self::open(key) {
            return false;
        }
        let first = lock(&WARNED).get_or_insert_with(HashSet::new).insert(key.to_string());
        if first {
            AppLog::add_sev(lf!("{0}: refused, the feature flag {1} is closed", action, key), Sev::Warn);
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::base::hooks::static_hooks;
    use crate::base::settings::MemorySettingsBackend;
    use std::sync::Arc;

    /// RoadmapTests.ClosedFlagsRefuseTheirActionsAndOnlyCoreDevicesAndGpuAreOpenByDefault (the flag part; the Devices
    /// and Engine refusals are ported with their areas).
    #[test]
    fn closed_flags_refuse_and_only_core_devices_and_gpu_are_open_by_default() {
        let _g = static_hooks();
        let _b = Settings::scoped_backend(Arc::new(MemorySettingsBackend::new()));
        AppLog::clear();
        assert_eq!(Flags::FLAG_KEYS, ["core", "devices-write", "softclose", "etwirq", "timerres", "gpu-oc"]);
        for k in [Flags::CORE, Flags::DEVICES_WRITE, Flags::GPU_OC] {
            assert!(Flags::open(k));
        }
        for k in [Flags::SOFT_CLOSE, Flags::ETW_IRQ, Flags::TIMER_RES] {
            assert!(!Flags::open(k));
        }
        assert!(!Flags::open("no-such-flag"));
        Flags::set(Flags::DEVICES_WRITE, false).unwrap();
        assert!(!Flags::open(Flags::DEVICES_WRITE));
        assert!(Flags::refuse(Flags::DEVICES_WRITE, "Device layout"));
        assert!(AppLog::tail(500).iter().any(|e| e.sev == Sev::Warn && e.msg.contains("devices-write")));
        let warns = AppLog::tail(500).iter().filter(|e| e.sev == Sev::Warn).count();
        assert!(Flags::refuse(Flags::DEVICES_WRITE, "Device layout"));
        assert_eq!(warns, AppLog::tail(500).iter().filter(|e| e.sev == Sev::Warn).count()); // once per flag
        Flags::set(Flags::TIMER_RES, true).unwrap();
        assert!(Flags::open(Flags::TIMER_RES));
        assert!(!Flags::refuse(Flags::TIMER_RES, "Timer"));
        Flags::set(Flags::TIMER_RES, false).unwrap();
        assert!(!Flags::open(Flags::TIMER_RES));
    }
}
