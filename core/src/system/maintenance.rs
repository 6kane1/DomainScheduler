//! Factory reset / uninstall (port of `Maintenance.cs`): the logon task, the settings (HKCU and HKLM) and the data
//! folders go away.

use super::autostart::Autostart;
use super::deps;
use super::devices::{irq_auto_has_backup, Devices};
use super::store::Store;
use super::tweaks::Tweaks;
use crate::base::ci;
use crate::base::l::L;
use crate::lf;

/// `Maintenance` (static class).
pub struct Maintenance;

impl Maintenance {
    /// Restores a crashed session first (nothing may stay changed), then removes everything the program created.
    /// restore_crashed: C# `engine` + `SessionFile.Read()`: it reads the session and, when it did not end cleanly,
    /// restores it through the engine and returns the number of values (None: no crashed session or no engine).
    pub fn factory_reset(restore_crashed: Option<&dyn Fn() -> Option<i32>>) -> Vec<String> {
        let mut log = Vec::new();
        if let Some(n) = restore_crashed.and_then(|f| f()) {
            log.push(lf!("Restored after a crash: {0} values", n));
        }
        // Their saved values live in the data folder, which goes away below: put them back first.
        for h in [&deps::GAME_POWER_RESTORE, &deps::DEFENDER_REMOVE_ALL] {
            if let Some(f) = h.get() {
                match f() {
                    Ok(p) if !p.is_empty() => log.push(p),
                    Ok(_) => {}
                    Err(e) => log.push(e),
                }
            }
        }
        // Protection switches of older versions and the device interrupt changes: their backups are in the data folder too.
        log.extend(Tweaks::revert_retired());
        if irq_auto_has_backup() || Devices::has_backup() {
            let d = Devices::revert_all();
            log.push(match d {
                None => L::t("Device interrupt changes reverted; restart Windows").to_string(),
                Some(d) => lf!("Devices: {0}", d),
            });
        }
        Autostart::task_delete();
        log.push(lf!("Autostart task {0} removed", Autostart::TASK_NAME));
        match Self::remove_settings() {
            Ok(()) => log.push(L::t("Settings removed").to_string()),
            Err(e) => log.push(lf!("Settings: {0}", e)),
        }
        match Self::remove_data() {
            Ok(()) => log.push(L::t("Data folder removed").to_string()),
            Err(e) => log.push(lf!("Data folder: {0}", e)),
        }
        log
    }

    fn remove_settings() -> Result<(), String> {
        #[cfg(windows)]
        {
            use crate::native::reg::{Hive, Key};
            use crate::native::settings_backend::{MACHINE_ROOT, ROOT};
            if let Some(k) = Key::open(Hive::CurrentUser, "", true) {
                k.delete_subkey_tree(ROOT)?;
            }
            if let Some(k) = Key::open(Hive::LocalMachine, "", true) {
                k.delete_subkey_tree(MACHINE_ROOT)?;
            }
        }
        Ok(())
    }

    fn remove_data() -> Result<(), String> {
        // The state folder's contents; "native" holds the program's own unpacked libraries, in use while it runs.
        let dir = Store::dir()?;
        for e in std::fs::read_dir(&dir).map_err(|e| e.to_string())?.flatten() {
            let p = e.path();
            if p.is_dir() {
                if !ci::eq(&e.file_name().to_string_lossy(), "native") {
                    std::fs::remove_dir_all(&p).map_err(|e| e.to_string())?;
                }
            }
        }
        for e in std::fs::read_dir(&dir).map_err(|e| e.to_string())?.flatten() {
            let p = e.path();
            if !p.is_dir() {
                std::fs::remove_file(&p).map_err(|e| e.to_string())?;
            }
        }
        // The user's folder of versions before 1.15.0 Test 1.3 (already taken over and renamed *.migrated).
        let legacy = Store::legacy_dir();
        if legacy.is_dir() {
            std::fs::remove_dir_all(&legacy).map_err(|e| e.to_string())?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::base::hooks::static_hooks;
    use crate::base::settings::{MemorySettingsBackend, Settings};
    use crate::base::state_dir::STORE_DIR_HOOK;
    use std::sync::Arc;

    /// LatencyDefaultsTests: the factory reset puts the game power values back (GamePower.Restore()) before the data
    /// folder goes; here: restore, the hooks' lines, the task, the folder (the "native" folder stays).
    #[test]
    fn factory_reset_restores_first_and_keeps_native() {
        let _g = static_hooks();
        let _b = Settings::scoped_backend(Arc::new(MemorySettingsBackend::new()));
        let state = std::env::temp_dir().join(format!("ds-reset-{}", super::super::util::guid_n()));
        std::fs::create_dir_all(state.join("native")).unwrap();
        std::fs::create_dir_all(state.join("profiles")).unwrap();
        std::fs::write(state.join("presets.json"), "{}").unwrap();
        let s2 = state.clone();
        STORE_DIR_HOOK.set(Some(Arc::new(move || s2.clone())));
        deps::GAME_POWER_RESTORE.set(Some(Arc::new(|| Ok("power back".to_string()))));
        let mem = Arc::new(crate::system::autostart::MemTasks::default());
        Autostart::set_primary(Some(mem.clone()));
        Autostart::set_fallback(Some(mem.clone()));
        assert!(!Store::legacy_dir().exists(), "a real legacy folder exists here; the test would remove it");
        let restore = || Some(3);
        let log = Maintenance::factory_reset(Some(&restore));
        deps::GAME_POWER_RESTORE.set(None);
        Autostart::set_primary(None);
        Autostart::set_fallback(None);
        STORE_DIR_HOOK.set(None);
        assert_eq!(log[0], "Restored after a crash: 3 values");
        assert_eq!(log[1], "power back");
        assert!(log.contains(&"Autostart task DomainScheduler removed".to_string()));
        assert_eq!(log.last().unwrap(), "Data folder removed");
        assert!(state.join("native").is_dir());
        assert!(!state.join("profiles").exists() && !state.join("presets.json").exists());
        let _ = std::fs::remove_dir_all(&state);
    }
}
