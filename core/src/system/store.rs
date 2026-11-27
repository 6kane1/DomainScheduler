//! Presets and profiles on disk (section 9), port of `Store.cs`. JSON in the same shape as System.Text.Json writes
//! it (the same property names and order). Not used by the Auto path.

use super::util::{ci_cmp, create_parent, move_overwrite, read_all_text, to_json, to_json_indented, write_all_text, Ordered};
use crate::base::ci::{self, CiMap, CiMapExt};
use crate::base::model::{Preset, ThreadRule};
use crate::base::state_dir::{store_dir, store_dir_shown};
use crate::base::time::DateTime;
use crate::lf;
use serde::Serialize;
use serde_json::{Map, Value};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI32, Ordering};

static LOAD_CALLS: AtomicI32 = AtomicI32::new(0);
static LIST_PROFILES_CALLS: AtomicI32 = AtomicI32::new(0);

/// What `Store::load` read: the migration notes, the presets (by exe, case-insensitive) and the background classes.
#[derive(Default)]
pub struct Loaded {
    pub notes: Vec<String>,
    pub presets: CiMap<Preset>,
    pub bg: CiMap<String>,
}

#[derive(Serialize)]
struct ThreadJson<'a> {
    #[serde(rename = "match")]
    match_: &'a str,
    pattern: &'a str,
    prio: &'a str,
    ideal: &'a str,
    boost: &'a str,
    tid: &'a str,
    enabled: bool,
    pin: bool,
}

#[derive(Serialize)]
struct PresetJson<'a> {
    exe: &'a str,
    class: &'a str,
    io: &'a str,
    mem: &'a str,
    eco: &'a str,
    boost: &'a str,
    enabled: bool,
    builtin: bool,
    #[serde(rename = "hotFirst")]
    hot_first: &'a str,
    #[serde(rename = "hotOthers")]
    hot_others: &'a str,
    threads: Vec<ThreadJson<'a>>,
}

fn preset_json(p: &Preset) -> PresetJson<'_> {
    PresetJson {
        exe: &p.exe,
        class: &p.class,
        io: &p.io,
        mem: &p.mem,
        eco: &p.eco,
        boost: &p.boost,
        enabled: p.enabled,
        builtin: p.builtin,
        hot_first: &p.hot_first,
        hot_others: &p.hot_others,
        threads: p
            .threads
            .iter()
            .map(|t| ThreadJson {
                match_: &t.match_,
                pattern: &t.pattern,
                prio: &t.prio,
                ideal: &t.ideal,
                boost: &t.boost,
                tid: &t.tid,
                enabled: t.enabled,
                pin: t.pin,
            })
            .collect(),
    }
}

#[derive(Serialize)]
struct RootJson<'a> {
    version: i32,
    #[serde(rename = "savedUtc")]
    saved_utc: String,
    presets: Vec<PresetJson<'a>>,
    background: Ordered<'a, String>,
}

/// `JsonValue.TryGetValue(out string)`: only a JSON string.
fn s(d: &Map<String, Value>, key: &str, def: &str) -> String {
    match d.get(key) {
        Some(Value::String(v)) => v.clone(),
        _ => def.to_string(),
    }
}

/// `JsonValue.TryGetValue(out int)`: a JSON number that is a whole 32-bit value.
fn int_of(v: Option<&Value>) -> Option<i32> {
    match v {
        Some(Value::Number(n)) => n.as_i64().and_then(|x| i32::try_from(x).ok()),
        _ => None,
    }
}

fn b(d: &Map<String, Value>, key: &str, def: bool) -> bool {
    match d.get(key) {
        Some(Value::Bool(v)) => *v,
        _ => def,
    }
}

fn heat(v: String) -> String {
    if v == "pin" || v == "ideal" { v } else { "none".into() }
}

fn preset_from(o: &Map<String, Value>) -> Preset {
    let mut p = Preset {
        exe: s(o, "exe", ""),
        class: s(o, "class", "normal"),
        io: s(o, "io", "normal"),
        mem: s(o, "mem", "normal"),
        eco: s(o, "eco", "off"),
        boost: s(o, "boost", ""),
        enabled: b(o, "enabled", true),
        builtin: b(o, "builtin", false),
        hot_first: heat(s(o, "hotFirst", "none")),
        hot_others: heat(s(o, "hotOthers", "none")),
        threads: Vec::new(),
    };
    if let Some(Value::Array(arr)) = o.get("threads") {
        for t in arr.iter().filter_map(|x| x.as_object()) {
            p.threads.push(ThreadRule {
                match_: s(t, "match", "module"),
                pattern: s(t, "pattern", ""),
                prio: s(t, "prio", ""),
                ideal: s(t, "ideal", ""),
                boost: s(t, "boost", ""),
                tid: s(t, "tid", ""),
                enabled: b(t, "enabled", true),
                pin: b(t, "pin", false),
            });
        }
    }
    p
}

/// `Store` (static class).
pub struct Store;

impl Store {
    pub const VERSION: i32 = 1;

    /// The data folder: the admin-only state folder (`StateDir`), checked before its first use. Everything the program
    /// keeps lives here: presets, backups, the session, the boot guard, the log, update downloads. Err when the folder
    /// cannot be made safe (nothing is then read from it or written to it).
    pub fn dir() -> Result<PathBuf, String> {
        store_dir()
    }

    /// The data folder for display (never fails, nothing is checked).
    pub fn dir_shown() -> PathBuf {
        store_dir_shown()
    }

    /// Where versions before 1.15.0 Test 1.3 kept their data (the user's own folder): only read once by `LegacyImport`.
    pub fn legacy_dir() -> PathBuf {
        crate::native::sys::local_app_data().join("DomainScheduler")
    }

    pub fn profiles_dir() -> Result<PathBuf, String> {
        Ok(Self::dir()?.join("profiles"))
    }

    pub fn presets_path() -> Result<PathBuf, String> {
        Ok(Self::dir()?.join("presets.json"))
    }

    /// `Store.LoadCalls`.
    pub fn load_calls() -> i32 {
        LOAD_CALLS.load(Ordering::SeqCst)
    }

    /// `Store.ListProfilesCalls`.
    pub fn list_profiles_calls() -> i32 {
        LIST_PROFILES_CALLS.load(Ordering::SeqCst)
    }

    /// Saves the presets (ordered by exe), the background classes and flags. path None: `presets_path()`.
    pub fn save(presets: &CiMap<Preset>, bg: &CiMap<String>, path: Option<&Path>) -> Result<(), String> {
        let mut list: Vec<&Preset> = presets.values().collect();
        list.sort_by(|a, b| ci_cmp(&a.exe, &b.exe));
        let mut bgl: Vec<(&str, &String)> = bg.iter().map(|(k, v)| (k.as_str(), v)).collect();
        bgl.sort_by(|a, b| ci_cmp(a.0, b.0));
        let root = RootJson {
            version: Self::VERSION,
            saved_utc: DateTime::utc_now().to_o(),
            presets: list.into_iter().map(preset_json).collect(),
            background: Ordered(bgl),
        };
        let path = match path {
            Some(p) => p.to_path_buf(),
            None => Self::presets_path()?,
        };
        create_parent(&path).map_err(|e| e.to_string())?;
        let tmp = PathBuf::from(format!("{}.tmp", path.display()));
        write_all_text(&tmp, &to_json_indented(&root)).map_err(|e| e.to_string())?;
        move_overwrite(&tmp, &path).map_err(|e| e.to_string())
    }

    /// Loads presets and background classes; the notes say what was migrated (missing fields get defaults).
    pub fn load(path: Option<&Path>) -> Result<Loaded, String> {
        LOAD_CALLS.fetch_add(1, Ordering::SeqCst);
        let mut r = Loaded::default();
        let path = match path {
            Some(p) => p.to_path_buf(),
            None => Self::presets_path()?,
        };
        if !path.is_file() {
            return Ok(r);
        }
        let text = read_all_text(&path).map_err(|e| e.to_string())?;
        let root = match serde_json::from_str::<Value>(&text) {
            Ok(Value::Object(o)) => o,
            Ok(_) => return Ok(r),
            Err(e) => {
                r.notes.push(lf!("presets.json is damaged: {0}", e.to_string()));
                return Ok(r);
            }
        };
        let ver = int_of(root.get("version")).unwrap_or(0);
        if ver < Self::VERSION {
            r.notes.push(lf!("presets.json version {0} migrated to {1}", ver, Self::VERSION));
        }
        if let Some(Value::Array(arr)) = root.get("presets") {
            for o in arr.iter().filter_map(|x| x.as_object()) {
                let p = preset_from(o);
                if !p.exe.is_empty() {
                    let exe = p.exe.clone();
                    r.presets.ci_insert(&exe, p);
                }
            }
        }
        if let Some(Value::Object(bo)) = root.get("background") {
            for (k, v) in bo {
                if let Value::String(s) = v {
                    r.bg.ci_insert(k, s.clone());
                }
            }
        }
        Ok(r)
    }

    /// Reconciles the stored presets with the built-in ones: a stored copy flagged Builtin is replaced by the shipped
    /// text (built-ins are never edited in place), a user preset with the same exe stays and overrides the built-in,
    /// missing built-ins are added. Returns what changed.
    pub fn mark_builtin(presets: &mut CiMap<Preset>, builtins: &[Preset]) -> Vec<String> {
        let mut notes = Vec::new();
        // C# ToDictionary throws on a duplicate exe; the port keeps the first one.
        let mut shipped: CiMap<&Preset> = CiMap::new();
        for b in builtins {
            if !shipped.ci_contains(&b.exe) {
                shipped.ci_insert(&b.exe, b);
            }
        }
        let keys: Vec<String> = presets.keys().map(|k| k.0.clone()).collect();
        for key in keys {
            let builtin = presets.ci_get(&key).map(|p| p.builtin).unwrap_or(false);
            if !builtin {
                continue;
            }
            if !shipped.ci_contains(&key) {
                presets.ci_remove(&key);
                notes.push(lf!("{0}: built-in preset no longer shipped, removed", key.as_str()));
            }
        }
        for b in shipped.values() {
            let existing = presets.ci_get(&b.exe).cloned();
            if let Some(p) = &existing {
                if !p.builtin {
                    notes.push(lf!("{0}: your preset overrides the built-in one", b.exe.as_str()));
                    continue;
                }
            }
            let had = existing.is_some();
            let mut c = (*b).clone();
            c.builtin = true;
            if let Some(p) = &existing {
                c.enabled = p.enabled;
            }
            if !had {
                notes.push(lf!("{0}: built-in preset added", b.exe.as_str()));
            } else if !Self::same(existing.as_ref(), Some(&c)) {
                notes.push(lf!("{0}: built-in preset updated", b.exe.as_str()));
            }
            presets.ci_insert(&b.exe, c);
        }
        notes
    }

    /// Two presets say the same (the dirty check of the Presets page and the reconciliation).
    pub fn same(a: Option<&Preset>, b: Option<&Preset>) -> bool {
        match (a, b) {
            (Some(a), Some(b)) => to_json(&preset_json(a)) == to_json(&preset_json(b)),
            _ => false,
        }
    }

    /// Saves the current presets under a profile name.
    pub fn save_profile(name: &str, presets: &CiMap<Preset>, bg: &CiMap<String>) -> Result<PathBuf, String> {
        let path = Self::profile_path(name)?;
        Self::save(presets, bg, Some(&path))?;
        Ok(path)
    }

    pub fn copy_profile(from: &str, to: &str) -> Result<bool, String> {
        let src = Self::profile_path(from)?;
        let dst = Self::profile_path(to)?;
        if !src.is_file() || ci::eq(&src.to_string_lossy(), &dst.to_string_lossy()) {
            return Ok(false);
        }
        std::fs::copy(&src, &dst).map_err(|e| e.to_string())?;
        Ok(true)
    }

    pub fn delete_profile(name: &str) -> Result<bool, String> {
        let path = Self::profile_path(name)?;
        if !path.is_file() {
            return Ok(false);
        }
        std::fs::remove_file(&path).map_err(|e| e.to_string())?;
        Ok(true)
    }

    /// The characters Windows refuses in a file name and control characters become '_'; empty gives "profile".
    pub fn sanitize(name: &str) -> String {
        const BAD: [char; 9] = ['\\', '/', ':', '*', '?', '"', '<', '>', '|'];
        let s: String = name.chars().map(|c| if BAD.contains(&c) || c.is_control() { '_' } else { c }).collect();
        let s = s.trim().to_string();
        if s.is_empty() { "profile".into() } else { s }
    }

    pub fn list_profiles() -> Result<Vec<String>, String> {
        LIST_PROFILES_CALLS.fetch_add(1, Ordering::SeqCst);
        let dir = Self::profiles_dir()?;
        if !dir.is_dir() {
            return Ok(Vec::new());
        }
        let mut list = Vec::new();
        for e in std::fs::read_dir(&dir).map_err(|e| e.to_string())?.flatten() {
            let p = e.path();
            if !p.is_file() {
                continue;
            }
            if p.extension().map(|x| ci::eq(&x.to_string_lossy(), "json")).unwrap_or(false) {
                if let Some(stem) = p.file_stem() {
                    list.push(stem.to_string_lossy().to_string());
                }
            }
        }
        list.sort_by(|a, b| ci_cmp(a, b));
        Ok(list)
    }

    pub fn profile_path(name: &str) -> Result<PathBuf, String> {
        Ok(Self::profiles_dir()?.join(format!("{}.json", Self::sanitize(name))))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::base::hooks::static_hooks;
    use crate::base::settings::{MemorySettingsBackend, Settings};
    use crate::base::state_dir::STORE_DIR_HOOK;
    use std::sync::Arc;

    pub(crate) fn temp_dir(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!("{}-{}", tag, super::super::util::guid_n()))
    }

    fn map(list: Vec<Preset>) -> CiMap<Preset> {
        let mut m = CiMap::new();
        for p in list {
            let e = p.exe.clone();
            m.ci_insert(&e, p);
        }
        m
    }

    /// SettingsStoreTests.StoreRoundTrip
    #[test]
    fn store_round_trip() {
        let _g = static_hooks();
        let _b = Settings::scoped_backend(Arc::new(MemorySettingsBackend::new()));
        let dir = temp_dir("ds-store");
        let d2 = dir.clone();
        STORE_DIR_HOOK.set(Some(Arc::new(move || d2.clone())));
        let presets = map(vec![Preset {
            exe: "cs2.exe".into(),
            class: "above".into(),
            threads: vec![ThreadRule { match_: "desc".into(), pattern: "MainThrd".into(), ideal: "auto".into(), ..Default::default() }],
            ..Default::default()
        }]);
        let mut bg = CiMap::new();
        bg.ci_insert("discord.exe", "below".to_string());
        Store::save(&presets, &bg, None).unwrap();
        let mut r = Store::load(None).unwrap();
        assert!(r.notes.is_empty());
        assert_eq!(r.presets.ci_get("cs2.exe").unwrap().class, "above");
        assert_eq!(r.presets.ci_get("cs2.exe").unwrap().threads[0].pattern, "MainThrd");
        assert_eq!(r.bg.ci_get("discord.exe").unwrap(), "below");
        assert_eq!(Store::sanitize("a/b:c"), "a_b_c");
        assert!(!Store::mark_builtin(&mut r.presets, &[Preset { exe: "other.exe".into(), ..Default::default() }]).is_empty());
        assert!(!r.presets.ci_get("cs2.exe").unwrap().builtin);
        assert!(r.presets.ci_get("other.exe").unwrap().builtin);
        STORE_DIR_HOOK.set(None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// SpecTests.BuiltinsAreReplacedByTheShippedTextAndUserCopiesOverrideThem
    #[test]
    fn builtins_are_replaced_by_the_shipped_text_and_user_copies_override_them() {
        let shipped = vec![
            Preset {
                exe: "a.exe".into(),
                class: "".into(),
                threads: vec![ThreadRule { pattern: "x.dll".into(), prio: "1".into(), ..Default::default() }],
                ..Default::default()
            },
            Preset { exe: "b.exe".into(), class: "".into(), ..Default::default() },
        ];
        let mut presets = map(vec![
            Preset { exe: "a.exe".into(), builtin: true, enabled: false, class: "high".into(), ..Default::default() },
            Preset { exe: "b.exe".into(), class: "below".into(), ..Default::default() },
            Preset { exe: "gone.exe".into(), builtin: true, ..Default::default() },
        ]);
        let notes = Store::mark_builtin(&mut presets, &shipped);
        assert_eq!(notes.len(), 3);
        assert_eq!(presets.ci_get("a.exe").unwrap().class, "");
        assert!(!presets.ci_get("a.exe").unwrap().enabled); // the switch is the user's
        assert!(presets.ci_get("a.exe").unwrap().builtin);
        assert!(!presets.ci_get("b.exe").unwrap().builtin);
        assert_eq!(presets.ci_get("b.exe").unwrap().class, "below");
        assert!(!presets.ci_contains("gone.exe"));
        assert!(Store::mark_builtin(&mut presets, &shipped).iter().all(|n| n.contains("overrides")));
    }

    /// SpecTests.ProfilesRoundTrip
    #[test]
    fn profiles_round_trip() {
        let _g = static_hooks();
        let dir = temp_dir("ds-prof");
        let d2 = dir.clone();
        STORE_DIR_HOOK.set(Some(Arc::new(move || d2.clone())));
        let presets = map(vec![Preset {
            exe: "x.exe".into(),
            class: "above".into(),
            threads: vec![ThreadRule { match_: "desc".into(), pattern: "Main".into(), prio: "1".into(), tid: "first".into(), ..Default::default() }],
            ..Default::default()
        }]);
        Store::save_profile("one", &presets, &CiMap::new()).unwrap();
        assert!(Store::copy_profile("one", "two").unwrap());
        assert_eq!(Store::list_profiles().unwrap(), vec!["one".to_string(), "two".to_string()]);
        let back = Store::load(Some(&Store::profile_path("two").unwrap())).unwrap();
        assert!(Store::same(presets.ci_get("x.exe"), back.presets.ci_get("x.exe")));
        assert!(Store::delete_profile("one").unwrap());
        assert!(!Store::delete_profile("one").unwrap());
        STORE_DIR_HOOK.set(None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn damaged_and_old_files_give_notes() {
        let dir = temp_dir("ds-store-bad");
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("presets.json");
        std::fs::write(&p, "{ not json").unwrap();
        let r = Store::load(Some(&p)).unwrap();
        assert_eq!(r.notes.len(), 1);
        assert!(r.notes[0].starts_with("presets.json is damaged: "));
        std::fs::write(&p, r#"{"presets":[{"exe":"a.exe","hotFirst":"weird","threads":[{"pin":true},5]}],"background":{"x.exe":"idle","y.exe":3}}"#).unwrap();
        let r = Store::load(Some(&p)).unwrap();
        assert_eq!(r.notes, vec!["presets.json version 0 migrated to 1".to_string()]);
        let a = r.presets.ci_get("A.EXE").unwrap();
        assert_eq!((a.class.as_str(), a.hot_first.as_str(), a.threads.len(), a.threads[0].pin, a.threads[0].match_.as_str()), ("normal", "none", 1, true, "module"));
        assert_eq!(r.bg.len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
