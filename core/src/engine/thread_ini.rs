//! The built-in system presets (section 14, port of `ThreadIni.cs`): an embedded INI with [SETTINGS], [DISABLEBOOST]
//! and one section per process. Parsed into presets with Builtin = true; the sys layer applies them independently of
//! Auto.

use super::engine::Engine;
use super::guard::Guard;
use crate::base::ci::{self, CiMap, CiMapExt};
use crate::base::hooks::Hook;
use crate::base::model::{Preset, ThreadRule};
use crate::base::settings::{parse_int_leading_sign, parse_int_none};
use crate::lf;
use std::sync::{Arc, OnceLock};

const THREADS_INI: &str = include_str!("../../res/lang/threads.ini");

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Row {
    pub process: String,
    pub skipped: bool,
    pub note: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct IniResult {
    pub rows: Vec<Row>,
    pub presets: CiMap<Preset>,
    pub sleep: i32,
}

impl Default for IniResult {
    fn default() -> Self {
        IniResult { rows: Vec::new(), presets: CiMap::new(), sleep: ThreadIni::DEFAULT_SLEEP }
    }
}

/// `ThreadIni.TextHook` (test hook: replaces the embedded text).
pub static TEXT_HOOK: Hook<dyn Fn() -> String + Send + Sync> = Hook::new();

/// `ThreadIni` (static class).
pub struct ThreadIni;

impl ThreadIni {
    pub const DEFAULT_SLEEP: i32 = 10000;

    pub fn load_bundled() -> Arc<IniResult> {
        if let Some(h) = TEXT_HOOK.get() {
            return Arc::new(Self::parse(&h()));
        }
        static CACHE: OnceLock<Arc<IniResult>> = OnceLock::new();
        CACHE.get_or_init(|| Arc::new(Self::parse(THREADS_INI))).clone()
    }

    /// Settings' ReapplyMs default reads the bundled sleep through this (C# `ThreadIni.LoadBundled().Sleep`).
    pub fn install_hooks() {
        crate::base::settings::REAPPLY_DEFAULT_HOOK.set(Some(Arc::new(|| ThreadIni::load_bundled().sleep)));
    }

    pub fn exe_of(name: &str) -> String {
        if ci::ends_with(name, ".exe") { name.to_string() } else { format!("{}.exe", name) }
    }

    fn new_preset(exe: &str) -> Preset {
        Preset {
            exe: exe.to_string(),
            class: String::new(),
            io: String::new(),
            mem: String::new(),
            eco: String::new(),
            boost: String::new(),
            builtin: true,
            ..Default::default()
        }
    }

    fn is_list(sec: &str) -> bool {
        ci::eq(sec, "SETTINGS") || ci::eq(sec, "DISABLEBOOST")
    }

    pub fn parse(text: &str) -> IniResult {
        let mut r = IniResult::default();
        // rows by exe (case-insensitive) -> index into r.rows
        let mut rows: CiMap<usize> = CiMap::new();
        let mut sec = String::new();
        let mut row: Option<usize> = None;
        for raw in text.split('\n') {
            let line = raw.trim().trim_start_matches('\u{feff}');
            let first = match line.chars().next() {
                None => continue,
                Some(c) => c,
            };
            if first == ';' || first == '#' {
                continue;
            }
            if first == '[' && line.ends_with(']') && line.len() >= 2 {
                sec = line[1..line.len() - 1].trim().to_string();
                row = None;
                if sec.is_empty() || Self::is_list(&sec) {
                    continue;
                }
                let exe = Self::exe_of(&sec);
                match rows.ci_get(&exe) {
                    Some(i) => row = Some(*i),
                    None => {
                        r.rows.push(Row { process: exe.clone(), ..Default::default() });
                        rows.ci_insert(&exe, r.rows.len() - 1);
                        row = Some(r.rows.len() - 1);
                    }
                }
                if !r.presets.ci_contains(&exe) {
                    r.presets.ci_insert(&exe, Self::new_preset(&exe));
                }
                continue;
            }
            let eq = line.find('=');
            let key = match eq {
                None => line,
                Some(i) => &line[..i],
            }
            .trim();
            let val = match eq {
                None => "",
                Some(i) => line[i + 1..].trim(),
            };
            if key.is_empty() {
                continue;
            }
            if ci::eq(&sec, "SETTINGS") {
                if ci::eq(key, "sleep") {
                    if let Some(ms) = parse_int_none(val.split(';').next().unwrap_or("").trim()) {
                        if ms > 0 {
                            r.sleep = ms.max(1000);
                        }
                    }
                }
                continue;
            }
            if ci::eq(&sec, "DISABLEBOOST") {
                let exe = Self::exe_of(key);
                if !r.presets.ci_contains(&exe) {
                    r.presets.ci_insert(&exe, Self::new_preset(&exe));
                }
                r.presets.ci_get_mut(&exe).unwrap().boost = "off".into();
                continue;
            }
            let Some(ri) = row else { continue };
            let process = r.rows[ri].process.clone();
            let kind = ci::lower(key);
            if kind != "module" && kind != "description" {
                r.rows[ri].note = lf!("unknown key: {0}", key);
                continue;
            }
            let f: Vec<&str> = val.split(',').map(|x| x.trim()).collect();
            if f.len() < 2 || f[0].is_empty() {
                r.rows[ri].note = lf!("broken rule: {0}", line);
                continue;
            }
            let mut rule = ThreadRule {
                match_: if kind == "module" { "module".into() } else { "desc".into() },
                pattern: f[0].to_string(),
                prio: f[1].to_string(),
                enabled: true,
                ..Default::default()
            };
            for extra in f.iter().skip(2) {
                let x = ci::lower(&extra.replace(' ', ""));
                if x == "disableboost" {
                    rule.boost = "off".into();
                } else if x.starts_with("tid=") {
                    rule.tid = x[4..].to_string();
                } else {
                    r.rows[ri].note = lf!("unknown option: {0}", *extra);
                }
            }
            r.presets.ci_get_mut(&process).unwrap().threads.push(rule);
        }
        r
    }

    pub fn valid_prio(p: &str) -> bool {
        parse_int_leading_sign(p).map(|n| Engine::tprio_contains_value(n as i32)).unwrap_or(false)
    }

    fn valid_tid(t: &str) -> bool {
        t.is_empty() || t == "first" || t == "last" || parse_int_none(t).is_some()
    }

    /// Checks the built-in presets; an empty list means OK. Problems never block the run.
    pub fn self_test() -> Vec<String> {
        Self::self_test_of(&Self::load_bundled())
    }

    pub fn self_test_of(r: &IniResult) -> Vec<String> {
        let mut bad = Vec::new();
        for row in &r.rows {
            if row.skipped && row.note.is_empty() {
                bad.push(lf!("{0}: skipped without a note", &row.process));
            } else if !row.note.is_empty() {
                bad.push(format!("{}: {}", row.process, row.note));
            }
        }
        for p in r.presets.values() {
            if Guard::is_denied(&p.exe) {
                bad.push(lf!("{0}: a denylisted process is among the presets", &p.exe));
            }
            if p.threads.is_empty() && p.boost.is_empty() {
                bad.push(lf!("{0}: no rules", &p.exe));
            }
            if !matches!(p.boost.as_str(), "" | "on" | "off") {
                bad.push(lf!("{0}: bad boost", &p.exe));
            }
            for t in &p.threads {
                let id = format!("{}/{}", p.exe, t.pattern);
                if t.pattern.trim().is_empty() {
                    bad.push(lf!("{0}: empty pattern", &p.exe));
                }
                if !Self::valid_prio(&t.prio) {
                    bad.push(lf!("{0}: bad priority", &id));
                }
                if !matches!(t.boost.as_str(), "" | "on" | "off") {
                    bad.push(lf!("{0}: bad boost", &id));
                }
                let ideal_ok = t.ideal.is_empty() || t.ideal == "auto" || parse_int_none(&t.ideal).map(|n| (0..=63).contains(&n)).unwrap_or(false);
                if !ideal_ok {
                    bad.push(lf!("{0}: bad ideal processor", &id));
                }
                if !matches!(t.match_.as_str(), "module" | "desc" | "tid") {
                    bad.push(lf!("{0}: bad match", &id));
                }
                if !Self::valid_tid(&t.tid) {
                    bad.push(lf!("{0}: bad tid filter", &id));
                }
            }
        }
        bad
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::base::hooks::static_hooks;
    use crate::base::settings::{MemorySettingsBackend, Settings};
    use crate::base::testrand::{iterations, Rand};

    /// AddendumTests.BundledPresetsPassSelfTestAndBrokenOnesAreReported
    #[test]
    fn bundled_presets_pass_self_test_and_broken_ones_are_reported() {
        let _g = static_hooks();
        TEXT_HOOK.set(None);
        assert!(ThreadIni::self_test().is_empty(), "{:?}", ThreadIni::self_test());
        let bundled = ThreadIni::load_bundled();
        for name in [
            "spotify", "discord", "steam", "steamwebhelper", "dwm", "searchapp", "winlogon", "audiodg", "sihost", "taskhostw", "dllhost",
            "startmenuexperiencehost", "shellexperiencehost", "textinputhost", "conhost", "explorer", "ctfmon", "ayugram", "winws", "domainscheduler",
        ] {
            assert!(bundled.presets.ci_contains(&format!("{}.exe", name)), "{}", name);
        }
        assert_eq!(20, bundled.presets.len());
        assert!(bundled.presets.values().all(|p| p.builtin));
        assert_eq!("off", bundled.presets.ci_get("spotify.exe").unwrap().boost);
        assert_eq!("", bundled.presets.ci_get("dwm.exe").unwrap().boost);
        assert_eq!(10000, bundled.sleep);
        let _b = Settings::scoped_backend(Arc::new(MemorySettingsBackend::new()));
        ThreadIni::install_hooks();
        assert_eq!("10000", Settings::get("ReapplyMs"));
        crate::base::settings::REAPPLY_DEFAULT_HOOK.set(None);

        let r = ThreadIni::parse("[SETTINGS]\nsleep=3000\n[DISABLEBOOST]\nfoo\n[foo]\nmodule=a.dll,-1,tid=last,disableboost\ndescription=*Render*,2,tid=3\n");
        assert_eq!(3000, r.sleep);
        let foo = r.presets.ci_get("foo.exe").unwrap();
        assert_eq!("off", foo.boost);
        let t0 = &foo.threads[0];
        assert_eq!(("module", "a.dll", "-1", "last", "off"), (t0.match_.as_str(), t0.pattern.as_str(), t0.prio.as_str(), t0.tid.as_str(), t0.boost.as_str()));
        let t1 = &foo.threads[1];
        assert_eq!(("desc", "*Render*", "2", "3", ""), (t1.match_.as_str(), t1.pattern.as_str(), t1.prio.as_str(), t1.tid.as_str(), t1.boost.as_str()));
        assert!(ThreadIni::self_test_of(&r).is_empty());

        TEXT_HOOK.set(Some(Arc::new(|| "[bad]\nmodule=x.dll,7\nmodule=y.dll,1,tid=soon\nwhat=1\n[empty]\n[csrss]\nmodule=csrss.exe,1\n".to_string())));
        let bad = ThreadIni::self_test();
        TEXT_HOOK.set(None);
        assert!(bad.iter().any(|b| b.contains("bad priority")));
        assert!(bad.iter().any(|b| b.contains("bad tid")));
        assert!(bad.iter().any(|b| b.contains("unknown key")));
        assert!(bad.iter().any(|b| b.contains("empty.exe")));
        assert!(bad.iter().any(|b| b.contains("csrss.exe") && b.contains("denylisted")));
    }

    /// FuzzTests.TextParsersNeverThrow (the ThreadIni part)
    #[test]
    fn text_parsers_never_throw() {
        let mut r = Rand::new(1604);
        for _ in 0..iterations() {
            let s = r.text(400);
            ThreadIni::parse(&s);
            ThreadIni::parse(&format!("[cs2.exe]\n{}", s));
        }
    }
}
