//! Taking over the data of versions before 1.15.0 Test 1.3 (port of `LegacyImport.cs`). They kept everything in the
//! user's own folder (%LOCALAPPDATA%\DomainScheduler), which any program of that user can change without administrator
//! rights. Some of it is still needed: the backups that put back the device and registry changes made then.
//!
//! At every start, each file still there is read once: only entries that pass the same checks the restores use
//! (exactly what the program itself could have written) are copied into the admin-only state folder, the rest is named
//! in the log and dropped; then the old file is renamed "*.migrated" so it is never read again. A backup that already
//! exists in the state folder wins (it holds the values from before the first change). The GPU profile marked "apply at
//! start" is taken over without that mark: the program asks once, in its own window, whether to keep applying it
//! (GpuProfiles.Confirm). Nothing is written to the registry or to Windows here; this only moves files.
//!
//! The importers of other areas' files are hooks (`deps`); while one is not set its file is left as it is (not read,
//! not renamed), so it is taken over at a later start.

use super::boot_guard::{BootGuard, State};
use super::deps;
use super::devices::Devices;
use super::store::Store;
use super::tweaks::Tweaks;
use super::util::{create_parent, guid_n, move_overwrite, parse_stj_date, read_all_text, stj_date, write_all_text};
use crate::base::app_log::Sev;
use crate::base::ci;
use crate::base::l::L;
use crate::base::time::DateTime;
use crate::lf;
use crate::topo::reservation::Reservation;
use std::path::{Path, PathBuf};

/// The importer of one file: (old content, content already in the state folder or None, dropped) -> new content, or
/// None for nothing. The outer None: the importer is not available in this build (see `deps`).
type Importer = fn(&str, Option<&str>, &mut Vec<String>) -> Option<Result<Option<String>, String>>;

const MAX_BYTES: u64 = 4 << 20;

fn hook(h: &crate::base::hooks::Hook<deps::ImportFn>, t: &str, e: Option<&str>, d: &mut Vec<String>) -> Option<Result<Option<String>, String>> {
    h.get().map(|f| f(t, e, d))
}

fn files() -> Vec<(&'static str, Importer)> {
    vec![
        (deps::names::IRQ_AUTO_BACKUP, |t, e, d| hook(&deps::IRQ_AUTO_IMPORT_BACKUP, t, e, d)),
        (Devices::BACKUP_NAME, |t, e, d| Some(Devices::import_backup(t, e, Some(d)))),
        (Tweaks::BACKUP_NAME, |t, e, d| Some(Tweaks::import_backup(t, e, Some(d)))),
        (Reservation::BACKUP_NAME, |t, e, d| Some(Ok(if e.is_some() { None } else { Reservation::import_backup(t, Some(d)) }))),
        (deps::names::GAME_POWER, |t, e, d| {
            let f = deps::GAME_POWER_IMPORT_FILE.get()?;
            Some(if e.is_some() { Ok(None) } else { f(t, d) })
        }),
        (deps::names::DEFENDER_EXCLUSIONS, import_exclusions),
        (BootGuard::FILE_NAME, |t, e, _d| {
            Some(if e.is_some() {
                Ok(None)
            } else {
                match serde_json::from_str::<Option<State>>(t) {
                    Ok(Some(s)) => serde_json::to_string(&s).map(Some).map_err(|x| x.to_string()),
                    Ok(None) => Err("empty".into()),
                    Err(x) => Err(x.to_string()),
                }
            })
        }),
        ("session.json", |t, e, d| hook(&deps::SESSION_IMPORT, t, e, d)),
        (deps::names::GPU_PROFILES, |t, e, d| hook(&deps::GPU_IMPORT, t, e, d)),
        ("presets.json", import_presets),
        ("tables.json", |t, e, _d| {
            let f = deps::TABLES_CHECK.get()?;
            Some(if e.is_some() { Ok(None) } else { f(t).map(|_| Some(t.to_string())) })
        }),
        ("devices-state.json", |t, e, _d| Some(if e.is_some() { Ok(None) } else { import_device_state(t).map(Some) })),
    ]
}

/// `JsonSerializer.Serialize(JsonSerializer.Deserialize<Dictionary<string, DateTime>>(t) ?? new())`.
fn import_device_state(t: &str) -> Result<String, String> {
    let v: serde_json::Value = serde_json::from_str(t).map_err(|e| e.to_string())?;
    let mut o = serde_json::Map::new();
    match v {
        serde_json::Value::Null => {}
        serde_json::Value::Object(m) => {
            for (k, x) in m {
                let s = x.as_str().ok_or_else(|| "The JSON value could not be converted to System.DateTime.".to_string())?;
                let d: DateTime = parse_stj_date(s).ok_or_else(|| "The JSON value could not be converted to System.DateTime.".to_string())?;
                o.insert(k, serde_json::Value::String(stj_date(d)));
            }
        }
        _ => return Err("The JSON value could not be converted to System.Collections.Generic.Dictionary.".into()),
    }
    Ok(serde_json::Value::Object(o).to_string())
}

/// FileAttributes.ReparsePoint (a link, a junction, any reparse point).
fn is_reparse(p: &Path) -> bool {
    match std::fs::symlink_metadata(p) {
        Ok(m) => {
            #[cfg(windows)]
            {
                use std::os::windows::fs::MetadataExt;
                m.file_attributes() & 0x400 != 0
            }
            #[cfg(not(windows))]
            {
                m.file_type().is_symlink()
            }
        }
        Err(_) => false,
    }
}

fn with_suffix(p: &Path, suffix: &str) -> PathBuf {
    let mut s = p.as_os_str().to_os_string();
    s.push(suffix);
    PathBuf::from(s)
}

/// `Path.GetFullPath(p).TrimEnd('\\', '/')`.
fn full(p: &Path) -> String {
    let abs = if p.is_absolute() { p.to_path_buf() } else { std::env::current_dir().map(|d| d.join(p)).unwrap_or_else(|_| p.to_path_buf()) };
    abs.to_string_lossy().trim_end_matches(['\\', '/']).to_string()
}

/// `LegacyImport` (static class).
pub struct LegacyImport;

impl LegacyImport {
    /// Runs the take-over from legacy_dir into the state folder (`Store::dir`). Returns lines for the log. account only
    /// names where the files came from in the log. Err: the state folder cannot be used.
    pub fn run(legacy_dir: &Path, _account: &str) -> Result<Vec<(String, Sev)>, String> {
        let mut log = Vec::new();
        if legacy_dir.as_os_str().is_empty() || !legacy_dir.is_dir() {
            return Ok(log);
        }
        if is_reparse(legacy_dir) {
            log.push((lf!("The old data folder {0} is a link: nothing was taken over from it", legacy_dir.to_string_lossy().as_ref()), Sev::Warn));
            return Ok(log);
        }
        let dest = Store::dir()?;
        if ci::eq(&full(&dest), &full(legacy_dir)) {
            return Ok(log);
        }
        for (name, import) in files() {
            Self::one(legacy_dir, &dest, name, name, import, &mut log);
        }
        let profiles = legacy_dir.join("profiles");
        if profiles.is_dir() && !is_reparse(&profiles) {
            let mut list: Vec<PathBuf> = std::fs::read_dir(&profiles)
                .map(|rd| rd.flatten().map(|e| e.path()).filter(|p| p.is_file() && p.extension().map(|x| ci::eq(&x.to_string_lossy(), "json")).unwrap_or(false)).collect())
                .unwrap_or_default();
            list.sort();
            for f in list {
                let file_name = f.file_name().map(|x| x.to_string_lossy().to_string()).unwrap_or_default();
                let stem = f.file_stem().map(|x| x.to_string_lossy().to_string()).unwrap_or_default();
                let name = format!("profiles{}{}", std::path::MAIN_SEPARATOR, file_name);
                let dest_name = format!("profiles{}{}.json", std::path::MAIN_SEPARATOR, Store::sanitize(&stem));
                Self::one(legacy_dir, &dest, &name, &dest_name, import_presets, &mut log);
            }
        }
        Ok(log)
    }

    fn one(from: &Path, to: &Path, name: &str, dest_name: &str, import: Importer, log: &mut Vec<(String, Sev)>) {
        let src = from.join(name);
        let dst = to.join(dest_name);
        if !src.is_file() {
            return;
        }
        let mut dropped = Vec::new();
        let r = (|| -> Result<bool, String> {
            if is_reparse(&src) {
                return Err(L::t("it is a link").to_string());
            }
            let len = std::fs::metadata(&src).map_err(|e| e.to_string())?.len();
            if len > MAX_BYTES {
                return Err(L::t("it is too large").to_string());
            }
            let text = read_all_text(&src).map_err(|e| e.to_string())?;
            let existing = if dst.is_file() { Some(read_all_text(&dst).map_err(|e| e.to_string())?) } else { None };
            let Some(result) = import(&text, existing.as_deref(), &mut dropped) else { return Ok(false) };
            match result? {
                Some(result) => {
                    create_parent(&dst).map_err(|e| e.to_string())?;
                    let tmp = with_suffix(&dst, ".tmp");
                    write_all_text(&tmp, &result).map_err(|e| e.to_string())?;
                    move_overwrite(&tmp, &dst).map_err(|e| e.to_string())?;
                    log.push((lf!("Taken over from the old data folder: {0}", name), Sev::Info));
                }
                None => {
                    if existing.is_some() {
                        log.push((lf!("Not taken over from the old data folder (the state folder already has it): {0}", name), Sev::Info));
                    }
                }
            }
            if !dropped.is_empty() {
                let first: Vec<&str> = dropped.iter().take(8).map(|s| s.as_str()).collect();
                let list = format!("{}{}", first.join(", "), if dropped.len() > 8 { " …" } else { "" });
                log.push((lf!("{0}: entries that are not values the program writes were dropped: {1}", name, list), Sev::Warn));
            }
            Ok(true)
        })();
        match r {
            // The importer of this file is not in this build: the file stays as it is (read at a later start).
            Ok(false) => return,
            Ok(true) => {}
            Err(e) => log.push((lf!("{0} from the old data folder was not taken over: {1}", name, e), Sev::Warn)),
        }
        // Renamed in any case (read once): an unreadable or refused file is never tried again either.
        let mut target = with_suffix(&src, ".migrated");
        if target.exists() {
            target = with_suffix(&src, &format!(".{}.migrated", DateTime::utc_now().format("yyyyMMddHHmmss", &crate::base::fmt::Culture::invariant())));
        }
        let moved = if target.exists() { Err("The file already exists.".to_string()) } else { std::fs::rename(&src, &target).map_err(|e| e.to_string()) };
        if let Err(e) = moved {
            log.push((lf!("{0}: the old file could not be renamed ({1}); it is read again at the next start", name, e), Sev::Warn));
        }
    }
}

fn import_exclusions(text: &str, existing: Option<&str>, dropped: &mut Vec<String>) -> Option<Result<Option<String>, String>> {
    let safe = deps::DEFENDER_IS_SAFE_PATH.get()?;
    let lines: Vec<String> = existing.unwrap_or("").split('\n').chain(text.split('\n')).map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect();
    let mut keep: Vec<String> = Vec::new();
    for l in lines {
        if safe(&l) {
            if !keep.iter().any(|k| ci::eq(k, &l)) {
                keep.push(l);
            }
        } else {
            let n = l.encode_utf16().count();
            dropped.push(if n > 80 { format!("{}…", String::from_utf16_lossy(&l.encode_utf16().take(80).collect::<Vec<u16>>())) } else { l });
        }
    }
    Some(Ok(if keep.is_empty() { None } else { Some(format!("{}\r\n", keep.join("\r\n"))) }))
}

/// A scratch file inside the state folder (never in %TEMP%, which the user's programs can change).
fn scratch() -> Result<PathBuf, String> {
    Ok(Store::dir()?.join(format!("import-{}.tmp", guid_n())))
}

fn import_presets(text: &str, existing: Option<&str>, _dropped: &mut Vec<String>) -> Option<Result<Option<String>, String>> {
    if existing.is_some() {
        return Some(Ok(None));
    }
    Some((|| -> Result<Option<String>, String> {
        let tmp = scratch()?;
        let r = (|| -> Result<Option<String>, String> {
            write_all_text(&tmp, text).map_err(|e| e.to_string())?;
            let l = Store::load(Some(&tmp))?; // parsed with the same rules as a normal load
            if l.presets.is_empty() && l.bg.is_empty() {
                return Ok(None);
            }
            Store::save(&l.presets, &l.bg, Some(&tmp))?;
            read_all_text(&tmp).map(Some).map_err(|e| e.to_string())
        })();
        let _ = std::fs::remove_file(&tmp);
        let _ = std::fs::remove_file(with_suffix(&tmp, ".tmp"));
        r
    })())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::base::hooks::static_hooks;
    use crate::base::settings::{MemorySettingsBackend, Settings};
    use crate::base::state_dir::STORE_DIR_HOOK;
    use std::sync::Arc;

    /// SecurityTests.MigrationTakesOverValidEntriesDropsPlantedOnesAndRenamesTheOldFiles: the take-over itself (read
    /// once, renamed, a broken backup refused and named, dropped entries named). The IrqAuto, Defender, GPU and session
    /// checks belong to their areas; here their importers are stand-ins with the same contract.
    #[test]
    fn migration_takes_over_valid_entries_drops_planted_ones_and_renames_the_old_files() {
        let _g = static_hooks();
        let _b = Settings::scoped_backend(Arc::new(MemorySettingsBackend::new()));
        let state = std::env::temp_dir().join(format!("ds-state-{}", guid_n()));
        let legacy = std::env::temp_dir().join(format!("ds-legacy-{}", guid_n()));
        std::fs::create_dir_all(&state).unwrap();
        std::fs::create_dir_all(&legacy).unwrap();
        let s2 = state.clone();
        STORE_DIR_HOOK.set(Some(Arc::new(move || s2.clone())));
        deps::IRQ_AUTO_IMPORT_BACKUP.set(Some(Arc::new(|t: &str, _e: Option<&str>, d: &mut Vec<String>| {
            if t.contains("evil") {
                d.push("ImagePath".into());
            }
            Ok(Some("{\"Reg\":{\"HKLM|x|DevicePolicy\":\"d:0\"}}".into()))
        })));
        deps::DEFENDER_IS_SAFE_PATH.set(Some(Arc::new(|l: &str| !l.contains(';'))));

        std::fs::write(legacy.join(deps::names::IRQ_AUTO_BACKUP), "{\"Reg\":{\"evil\":\"s:x\"}}").unwrap();
        std::fs::write(legacy.join(deps::names::DEFENDER_EXCLUSIONS), "C:\\Games\\cs2.exe\nC:\\Games\\a\u{2018}; Start-Process calc; \u{2018}.exe\n").unwrap();
        std::fs::write(legacy.join(Tweaks::BACKUP_NAME), "{ not json").unwrap();
        std::fs::write(legacy.join("presets.json"), "{\"presets\":[{\"exe\":\"a.exe\"}]}").unwrap();
        std::fs::write(legacy.join(deps::names::GPU_PROFILES), "{}").unwrap(); // no GPU importer in this test: left alone

        let log = LegacyImport::run(&legacy, "").unwrap();

        let irq = std::fs::read_to_string(state.join(deps::names::IRQ_AUTO_BACKUP)).unwrap();
        assert!(irq.contains("DevicePolicy") && !irq.contains("evil"));
        let excl = std::fs::read_to_string(state.join(deps::names::DEFENDER_EXCLUSIONS)).unwrap();
        assert!(excl.contains("cs2.exe") && !excl.contains("calc"));
        assert!(!state.join(Tweaks::BACKUP_NAME).exists()); // broken: not taken over
        assert!(Store::load(Some(&state.join("presets.json"))).unwrap().presets.ci_contains_key("a.exe"));
        for n in [deps::names::IRQ_AUTO_BACKUP, deps::names::DEFENDER_EXCLUSIONS, Tweaks::BACKUP_NAME, "presets.json"] {
            assert!(!legacy.join(n).exists(), "{}", n);
            assert!(with_suffix(&legacy.join(n), ".migrated").exists(), "{}", n);
        }
        assert!(legacy.join(deps::names::GPU_PROFILES).exists());
        assert!(log.iter().any(|(t, s)| *s == Sev::Warn && t.contains("ImagePath")));
        assert!(log.iter().any(|(t, s)| *s == Sev::Warn && t.contains(Tweaks::BACKUP_NAME)));
        assert!(LegacyImport::run(&legacy, "").unwrap().is_empty()); // read once

        deps::IRQ_AUTO_IMPORT_BACKUP.set(None);
        deps::DEFENDER_IS_SAFE_PATH.set(None);
        STORE_DIR_HOOK.set(None);
        let _ = std::fs::remove_dir_all(&state);
        let _ = std::fs::remove_dir_all(&legacy);
    }

    trait CiKey {
        fn ci_contains_key(&self, k: &str) -> bool;
    }
    impl<V> CiKey for crate::base::ci::CiMap<V> {
        fn ci_contains_key(&self, k: &str) -> bool {
            use crate::base::ci::CiMapExt;
            self.ci_contains(k)
        }
    }
}
