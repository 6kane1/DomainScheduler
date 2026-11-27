//! Tables as data (section 22.11, port of `Tables.cs`): known games and dual-CCD X3D models come from the embedded
//! tables.json merged with an optional user file next to the presets (the user file wins). A broken user file is
//! ignored with an error; the code itself knows no model or game.

use super::guard::Guard;
use crate::base::app_log::{AppLog, Sev};
use crate::base::ci::{self, CiMap, CiMapExt};
use crate::base::hooks::{lock, Hook};
use crate::base::l::L;
use crate::lf;
use serde_json::Value;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

const TABLES_JSON: &str = include_str!("../../res/lang/tables.json");

#[derive(Clone, Debug, Default)]
pub struct Data {
    /// exe -> label
    pub known_games: CiMap<String>,
    /// sku -> cores
    pub x3d: CiMap<i32>,
    pub override_used: bool,
    pub error: String,
}

/// `Tables.EmbeddedHook` (tests replace the embedded text).
pub static EMBEDDED_HOOK: Hook<dyn Fn() -> String + Send + Sync> = Hook::new();
/// `Tables.OverridePathHook`.
pub static OVERRIDE_PATH_HOOK: Hook<dyn Fn() -> PathBuf + Send + Sync> = Hook::new();

static GATE: Mutex<Option<Arc<Data>>> = Mutex::new(None);

/// `Path.GetInvalidFileNameChars()` of Windows.
fn invalid_file_name_char(c: char) -> bool {
    (c as u32) < 32 || matches!(c, '"' | '<' | '>' | '|' | ':' | '*' | '?' | '\\' | '/')
}

/// `^\d{4}X3D$` (IgnoreCase) on an upper-cased, trimmed sku.
fn sku_ok(sku: &str) -> bool {
    let c: Vec<char> = sku.chars().collect();
    c.len() == 7 && c[..4].iter().all(|x| x.is_ascii_digit()) && ci::eq(&c[4..].iter().collect::<String>(), "X3D")
}

/// The JSON reader options of the C# side: comments are skipped and trailing commas allowed (serde_json knows
/// neither, so they are taken out first; strings are left as they are).
pub(crate) fn relax_json(s: &str) -> String {
    drop_trailing_commas(&strip_comments(s))
}

/// Takes `//` and `/* */` comments out (strings are left as they are).
fn strip_comments(s: &str) -> String {
    let b: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    let mut in_str = false;
    while i < b.len() {
        let c = b[i];
        if in_str {
            out.push(c);
            if c == '\\' && i + 1 < b.len() {
                out.push(b[i + 1]);
                i += 2;
                continue;
            }
            if c == '"' {
                in_str = false;
            }
            i += 1;
            continue;
        }
        if c == '"' {
            in_str = true;
        } else if c == '/' && i + 1 < b.len() && b[i + 1] == '/' {
            while i < b.len() && b[i] != '\n' {
                i += 1;
            }
            continue;
        } else if c == '/' && i + 1 < b.len() && b[i + 1] == '*' {
            i += 2;
            while i + 1 < b.len() && !(b[i] == '*' && b[i + 1] == '/') {
                i += 1;
            }
            i += 2;
            out.push(' ');
            continue;
        }
        out.push(c);
        i += 1;
    }
    out
}

/// Takes out a comma whose next significant char closes an object or a list.
fn drop_trailing_commas(s: &str) -> String {
    let b: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    let mut in_str = false;
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if in_str {
            out.push(c);
            if c == '\\' && i + 1 < b.len() {
                out.push(b[i + 1]);
                i += 2;
                continue;
            }
            if c == '"' {
                in_str = false;
            }
            i += 1;
            continue;
        }
        if c == '"' {
            in_str = true;
        } else if c == ',' {
            let mut j = i + 1;
            while j < b.len() && b[j].is_whitespace() {
                j += 1;
            }
            if j < b.len() && (b[j] == '}' || b[j] == ']') {
                i += 1;
                continue;
            }
        }
        out.push(c);
        i += 1;
    }
    out
}

/// `Tables` (static class).
pub struct Tables;

impl Tables {
    /// `Tables.OverridePath`: `tables.json` next to presets.json (the data folder). Err when the folder cannot be used.
    pub fn override_path() -> Result<PathBuf, String> {
        if let Some(h) = OVERRIDE_PATH_HOOK.get() {
            return Ok(h());
        }
        Ok(crate::base::state_dir::store_dir()?.join("tables.json"))
    }

    pub fn current() -> Arc<Data> {
        let mut g = lock(&GATE);
        if g.is_none() {
            *g = Some(Arc::new(Self::load(true)));
        }
        g.as_ref().unwrap().clone()
    }

    pub fn x3d() -> CiMap<i32> {
        Self::current().x3d.clone()
    }

    pub fn known_games() -> CiMap<String> {
        Self::current().known_games.clone()
    }

    pub fn reload() {
        let d = Self::load(true);
        *lock(&GATE) = Some(Arc::new(d));
    }

    /// Plugs the merged X3D table into CoreSplit (C# `CoreSplit.X3dDual => Tables.X3d`). Called once at start.
    pub fn install_hooks() {
        crate::topo::core_split::X3D_HOOK.set(Some(Arc::new(Self::x3d)));
    }

    fn embedded() -> String {
        if let Some(h) = EMBEDDED_HOOK.get() {
            return h();
        }
        TABLES_JSON.to_string()
    }

    /// Parses and validates one file into the data; returns how many entries were rejected. Err = the C# exception.
    pub fn merge(d: &mut Data, json: &str) -> Result<i32, String> {
        let mut bad = 0;
        let text = relax_json(json.trim_start_matches('\u{feff}'));
        let root: Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
        let Value::Object(root) = root else {
            return Err("root is not an object".into());
        };
        if let Some(games) = root.get("knownGames") {
            let Value::Array(games) = games else {
                return Err("knownGames is not a list".into());
            };
            for g in games {
                let s = |k: &str| g.as_object().and_then(|o| o.get(k)).and_then(|v| v.as_str()).map(|v| v.trim().to_string()).unwrap_or_default();
                let mut exe = s("exe");
                let label = s("label");
                if exe.is_empty() || exe.chars().any(invalid_file_name_char) {
                    bad += 1;
                    continue;
                }
                if !ci::ends_with(&exe, ".exe") {
                    exe.push_str(".exe");
                }
                if Guard::is_denied(&exe) {
                    bad += 1; // a denylisted program is never a known game
                    continue;
                }
                let l = if !label.is_empty() { label } else { exe.clone() };
                d.known_games.ci_insert(&exe, l);
            }
        }
        if let Some(x3d) = root.get("x3d") {
            let Value::Array(x3d) = x3d else {
                return Err("x3d is not a list".into());
            };
            for x in x3d {
                let o = x.as_object();
                let sku = o.and_then(|o| o.get("sku")).and_then(|v| v.as_str()).map(|v| ci::upper(v.trim())).unwrap_or_default();
                let cores = o
                    .and_then(|o| o.get("cores"))
                    .and_then(|v| v.as_i64())
                    .filter(|n| *n >= i32::MIN as i64 && *n <= i32::MAX as i64)
                    .unwrap_or(0) as i32;
                if !sku_ok(&sku) || !(4..=64).contains(&cores) || cores % 2 != 0 {
                    bad += 1;
                    continue;
                }
                d.x3d.ci_insert(&sku, cores);
            }
        }
        Ok(bad)
    }

    pub fn load(log: bool) -> Data {
        let mut d = Data::default();
        let mut bad = 0;
        match Self::merge(&mut d, &Self::embedded()) {
            Ok(n) => bad = n,
            Err(e) => {
                d.error = e.clone();
                if log {
                    AppLog::add_sev(lf!("Built-in tables: {0}", e), Sev::Error);
                }
            }
        }
        let over = (|| -> Result<Option<(Data, i32)>, String> {
            let path = Self::override_path()?;
            if !path.is_file() {
                return Ok(None);
            }
            let mut merged = Data::default();
            for (k, v) in &d.known_games {
                merged.known_games.ci_insert(k.as_str(), v.clone());
            }
            for (k, v) in &d.x3d {
                merged.x3d.ci_insert(k.as_str(), *v);
            }
            let text = std::fs::read(&path).map_err(|e| e.to_string())?;
            let text = String::from_utf8_lossy(&text).to_string();
            let n = Self::merge(&mut merged, &text)?;
            merged.override_used = true;
            Ok(Some((merged, n)))
        })();
        match over {
            Ok(Some((merged, n))) => {
                bad += n;
                d = merged;
            }
            Ok(None) => {}
            Err(e) => {
                d.error = e.clone(); // d is still the embedded data
                if log {
                    AppLog::add_sev(lf!("tables.json override is broken ({0}): the built-in tables are used", e), Sev::Error);
                }
            }
        }
        if log {
            let with = if d.override_used { L::t(", with the user file").to_string() } else { String::new() };
            let rej = if bad > 0 { lf!(", {0} entries rejected", bad) } else { String::new() };
            AppLog::add_sev(
                lf!("Tables: {0} known games, {1} X3D models{2}{3}", d.known_games.len(), d.x3d.len(), with, rej),
                if bad > 0 { Sev::Warn } else { Sev::Info },
            );
        }
        d
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::base::hooks::static_hooks;

    fn logged(sev: Sev, part: &str) -> bool {
        AppLog::tail(500).iter().any(|e| e.sev == sev && e.msg.contains(part))
    }

    /// RoadmapTests.TablesMergeTheUserFileAndSurviveABrokenOne
    #[test]
    fn tables_merge_the_user_file_and_survive_a_broken_one() {
        let _g = static_hooks();
        AppLog::clear();
        let dir = std::env::temp_dir().join(format!("ds-tab-{}-{}", std::process::id(), crate::native::sys::tick_count64()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("tables.json");
        let p2 = path.clone();
        OVERRIDE_PATH_HOOK.set(Some(Arc::new(move || p2.clone())));
        Tables::reload();
        assert_eq!(16, *Tables::x3d().ci_get("7950X3D").unwrap());
        assert!(Tables::known_games().ci_contains("cs2.exe"));
        assert!(!Tables::current().override_used);
        let games = Tables::known_games().len();

        std::fs::write(&path, r#"{"knownGames":[{"exe":"mygame","label":"My game"},{"exe":"cs2.exe","label":"CS"},{"exe":"lsass.exe","label":"x"},{"exe":""}],"x3d":[{"sku":"9955X3D","cores":16},{"sku":"7950X3D","cores":32},{"sku":"junk","cores":16},{"sku":"1234X3D","cores":7}]}"#).unwrap();
        Tables::reload();
        assert!(Tables::current().override_used);
        let kg = Tables::known_games();
        assert_eq!("My game", kg.ci_get("mygame.exe").unwrap());
        assert_eq!("CS", kg.ci_get("cs2.exe").unwrap());
        assert!(!kg.ci_contains("lsass.exe"));
        assert_eq!(games + 1, kg.len());
        let x = Tables::x3d();
        assert_eq!(16, *x.ci_get("9955X3D").unwrap());
        assert_eq!(32, *x.ci_get("7950X3D").unwrap());
        assert!(!x.ci_contains("JUNK"));
        assert!(!x.ci_contains("1234X3D"));
        assert!(logged(Sev::Warn, "entries rejected"));

        AppLog::clear();
        std::fs::write(&path, "{ this is not json").unwrap();
        Tables::reload();
        assert!(!Tables::current().override_used);
        assert_eq!(16, *Tables::x3d().ci_get("7950X3D").unwrap());
        assert_eq!(games, Tables::known_games().len());
        assert!(logged(Sev::Error, "tables.json override is broken"));

        OVERRIDE_PATH_HOOK.set(None);
        EMBEDDED_HOOK.set(None);
        *lock(&GATE) = None;
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn relaxed_json_takes_comments_and_trailing_commas() {
        let mut d = Data::default();
        let n = Tables::merge(&mut d, "{ // c\n \"x3d\": [ {\"sku\":\"7950x3d\", \"cores\": 16, }, /* x */ ], \"knownGames\": [{\"exe\":\"a//b\"}] }").unwrap();
        assert_eq!(1, n); // "a//b" has an invalid file name char
        assert_eq!(16, *d.x3d.ci_get("7950X3D").unwrap());
    }
}
