//! Localization (port of `L.cs`): one JSON resource per language with an en -> translation map.
//! `L::t` translates with a fallback to the key, `L::n` marks a string for extraction without translating it,
//! `L::f` formats a translated string (see the `lf!` macro).
//!
//! The two resources are vendored copies of the C# project's files in core/res/lang (tools/sync-lang.sh refreshes them).

use super::fmt::{self, Arg, Culture};
use super::hooks::lock;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock, RwLock};

const RU_JSON: &str = include_str!("../../res/lang/ru.json");
const EN_JSON: &str = include_str!("../../res/lang/en.json");
/// Texts only the native version has (PLAN.md, owner's decision 4): merged over the shared ru.json. The C# ru.json
/// stays untouched (its tests reject keys the C# code does not use).
const RU_NATIVE_JSON: &str = include_str!("../../res/ru.native.json");

/// One parsed language resource.
pub struct LangData {
    pub map: HashMap<String, String>,
    pub fonts: Vec<String>,
}

fn parse(json: &str) -> LangData {
    let json = json.trim_start_matches('\u{feff}');
    let mut map = HashMap::new();
    let mut fonts = Vec::new();
    if let Ok(serde_json::Value::Object(root)) = serde_json::from_str::<serde_json::Value>(json) {
        if let Some(serde_json::Value::Array(f)) = root.get("fonts") {
            for x in f {
                if let Some(s) = x.as_str() {
                    if !s.trim().is_empty() {
                        fonts.push(s.to_string());
                    }
                }
            }
        }
        if let Some(serde_json::Value::Object(st)) = root.get("strings") {
            for (k, v) in st {
                map.insert(k.clone(), v.as_str().map(|s| s.to_string()).unwrap_or_else(|| k.clone()));
            }
        }
    }
    LangData { map, fonts }
}

/// The parsed resource of a language ("en" or "ru"), parsed once per run.
pub fn resource(lang: &str) -> Option<&'static LangData> {
    static RU: OnceLock<LangData> = OnceLock::new();
    static EN: OnceLock<LangData> = OnceLock::new();
    match lang {
        "ru" => Some(RU.get_or_init(|| {
            let mut d = parse(RU_JSON);
            d.map.extend(parse(RU_NATIVE_JSON).map);
            d
        })),
        "en" => Some(EN.get_or_init(|| parse(EN_JSON))),
        _ => None,
    }
}

struct State {
    data: Option<&'static LangData>,
    lang: &'static str,
}

static STATE: RwLock<State> = RwLock::new(State { data: None, lang: "en" });
type ChangedFn = dyn Fn() + Send + Sync;
static CHANGED: Mutex<Vec<Arc<ChangedFn>>> = Mutex::new(Vec::new());

/// `L` (static class).
pub struct L;

impl L {
    /// Languages with a resource, by code ("en" is the keys themselves). Only English and Russian.
    pub const AVAILABLE: [&'static str; 2] = ["en", "ru"];

    pub fn available() -> &'static [&'static str] {
        &Self::AVAILABLE
    }

    /// The current language code.
    pub fn lang() -> &'static str {
        STATE.read().unwrap_or_else(|e| e.into_inner()).lang
    }

    /// The languages' own names (never translated).
    pub fn name_of(code: &str) -> &'static str {
        match code {
            "ru" => "Русский",
            _ => "English",
        }
    }

    /// All fonts of the current language, in order (the UI uses them as a fallback chain).
    pub fn fonts() -> Vec<String> {
        let st = STATE.read().unwrap_or_else(|e| e.into_inner());
        st.data.map(|d| d.fonts.iter().filter(|f| !f.trim().is_empty()).cloned().collect()).unwrap_or_default()
    }

    /// The first non-empty font of the current language ("" = the system default).
    pub fn family() -> String {
        Self::fonts().into_iter().next().unwrap_or_default()
    }

    /// Subscribes to `L.Changed` (raised after `load`: the UI rebuilds its texts).
    pub fn on_changed(f: Arc<ChangedFn>) {
        lock(&CHANGED).push(f);
    }

    pub fn load(lang: &str) {
        let lang = if lang.trim().is_empty() { Self::detect() } else { super::ci::lower(lang.trim()) };
        // A removed language (zh before 1.11.0) or an unknown code.
        let code: &'static str = Self::AVAILABLE.iter().copied().find(|c| *c == lang).unwrap_or("en");
        let data = resource(code);
        {
            let mut st = STATE.write().unwrap_or_else(|e| e.into_inner());
            st.data = data;
            st.lang = if data.is_some() { code } else { "en" };
        }
        let subs: Vec<Arc<ChangedFn>> = lock(&CHANGED).clone();
        for s in subs {
            s();
        }
    }

    /// The UI language of Windows when there is a resource for it, else English.
    pub fn detect() -> String {
        let two = super::ci::lower(&crate::native::sys::ui_language_two_letter());
        if Self::AVAILABLE.contains(&two.as_str()) { two } else { "en".into() }
    }

    /// `L.T(en)`: the translation, or the key itself.
    pub fn t<'a>(en: &'a str) -> &'a str {
        let st = STATE.read().unwrap_or_else(|e| e.into_inner());
        if let Some(d) = st.data {
            if let Some(t) = d.map.get(en) {
                if !t.is_empty() {
                    let s: &'static str = t.as_str();
                    return s;
                }
            }
        }
        en
    }

    /// `L.N(en)`: marks a text for the translation catalog, returns it as it is.
    pub fn n(en: &str) -> &str {
        en
    }

    /// `L.F(en, args)`: the translated format in the current culture; a broken translation falls back to the English
    /// text in the invariant culture.
    pub fn f(en: &str, args: &[Arg]) -> String {
        let f = Self::t(en);
        match fmt::format(&Culture::current(), f, args) {
            Ok(s) => s,
            // C# formats the English text again and lets that exception go up; the port returns the text unformatted.
            Err(_) => fmt::format(&Culture::invariant(), en, args).unwrap_or_else(|_| en.to_string()),
        }
    }
}

/// `L.F("text {0} {1}", a, b)` -> `lf!("text {0} {1}", a, b)`. Arguments are anything `Into<Arg>` (strings, numbers,
/// bool, DateTime).
#[macro_export]
macro_rules! lf {
    ($en:expr $(, $a:expr)* $(,)?) => {
        $crate::base::l::L::f($en, &[$($crate::base::fmt::Arg::from($a)),*])
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::base::hooks::static_hooks;
    use crate::base::settings::{MemorySettingsBackend, Settings};

    /// SpecTests.RussianAndEnglishOnlyLoadWithTheirFontsAndFallBackToTheKey
    #[test]
    fn russian_and_english_only_load_with_their_fonts_and_fall_back_to_the_key() {
        let _g = static_hooks();
        let _b = Settings::scoped_backend(Arc::new(MemorySettingsBackend::new()));
        assert_eq!(L::available(), &["en", "ru"]);
        for code in L::available() {
            L::load(code);
            assert_eq!(*code, L::lang());
            assert_ne!("", L::family());
            assert_eq!("no such key", L::t("no such key"));
            assert_eq!("x", L::n("x"));
        }
        L::load("ru");
        assert_ne!("Turn on", L::t("Turn on"));
        L::load("zh"); // removed in 1.11.0
        assert_eq!("en", L::lang());
        assert!(resource("zh").is_none());
        L::load("xx");
        assert_eq!("en", L::lang());
        Settings::set("Language", "ru").unwrap();
        assert_eq!("ru", Settings::language());
        Settings::set("Language", "klingon").unwrap();
        assert_eq!(L::detect(), Settings::language());
        Settings::set("Language", "zh").unwrap(); // a saved removed language: English, or Russian on a Russian Windows
        assert_eq!(L::detect(), Settings::language());
        assert!(["en", "ru"].contains(&L::detect().as_str()));
    }

    /// LangTests.RussianStatusesMatchTheSpec
    #[test]
    fn russian_statuses_match_the_spec() {
        let _g = static_hooks();
        L::load("ru");
        assert_eq!("ru", L::lang());
        assert_eq!("Жду игру…", L::t("Waiting for the game…"));
        assert_eq!("Игра: cs2.exe · ускорение включено", lf!("Game: {0} · boost on", "cs2.exe"));
        assert_eq!("Античит: игра не изменена, фон приглушён", L::t("Anti-cheat: the game is not changed, the background is muted"));
        assert_eq!("Ошибка. Всё возвращено как было. Откройте отчёт.", L::t("Error. Everything was put back. Open the Report."));
        assert_eq!("Раздел Session Manager\\Kernel не открыть", L::t("The Session Manager\\Kernel key cannot be opened"));
        assert_eq!("untranslated", L::t("untranslated"));
        L::load("en");
    }

    fn placeholders(s: &str) -> Vec<String> {
        // \{\d+[^}]*\}
        let mut out = Vec::new();
        let b: Vec<char> = s.chars().collect();
        let mut i = 0;
        while i < b.len() {
            if b[i] == '{' && i + 1 < b.len() && b[i + 1].is_ascii_digit() {
                if let Some(end) = b[i..].iter().position(|c| *c == '}') {
                    out.push(b[i..i + end + 1].iter().collect());
                    i += end + 1;
                    continue;
                }
            }
            i += 1;
        }
        out.sort();
        out
    }

    /// TranslationTests.RussianEntriesKeepThePlaceholdersAndAreUsed (the placeholder half; "used in the sources" is
    /// checked against the C# sources, which stay the single source of the texts).
    #[test]
    fn russian_entries_keep_the_placeholders() {
        let ru = resource("ru").unwrap();
        assert!(ru.map.len() > 500);
        for (en, t) in &ru.map {
            assert_eq!(placeholders(en), placeholders(t), "placeholders differ: {} => {}", en, t);
        }
    }

    /// Native-only texts (core/res/ru.native.json) are merged over the shared ru.json, and every text the
    /// native-only Thread map page and its feed hand to L has a Russian entry.
    #[test]
    fn native_texts_are_merged_and_cover_the_thread_map() {
        let ru = resource("ru").unwrap();
        let native = parse(RU_NATIVE_JSON);
        assert!(native.map.len() > 40);
        for (k, v) in &native.map {
            assert_eq!(ru.map.get(k), Some(v));
            assert_eq!(placeholders(k), placeholders(v), "placeholders differ: {} => {}", k, v);
        }
        let root = env!("CARGO_MANIFEST_DIR");
        let page = std::fs::read_to_string(format!("{root}/../app/src/pages/thread_map.cpp")).unwrap();
        let mut keys = Vec::new();
        for pat in ["L::T(\"", "L::F(\""] {
            let mut from = 0;
            while let Some(p) = page[from..].find(pat) {
                let start = from + p + pat.len();
                let end = start + page[start..].find('"').unwrap();
                keys.push(page[start..end].to_string());
                from = end;
            }
        }
        keys.extend(["wakes", "takes the core", "waits for a lock", "calls"].map(String::from));
        for f in ["etw/thread_map.rs", "ffi/thread_map.rs"] {
            keys.extend(literals(&std::fs::read_to_string(format!("{root}/src/{f}")).unwrap()));
        }
        assert!(keys.len() > 50, "only {} texts found: the scan is broken", keys.len());
        let missing: Vec<&String> = keys.iter().filter(|k| !ru.map.get(*k).is_some_and(|v| !v.trim().is_empty())).collect();
        assert!(missing.is_empty(), "no Russian text for: {missing:?}");
    }

    /// TranslationTests.OnlyRussianAndEnglish
    #[test]
    fn only_russian_and_english() {
        assert_eq!(L::available(), &["en", "ru"]);
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/res/lang");
        let mut files: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|n| n.ends_with(".json") && n != "tables.json")
            .collect();
        files.sort();
        assert_eq!(files, vec!["en.json", "ru.json"]);
        assert_eq!("English", L::name_of("zh"));
    }

    /// TranslationTests.EveryLiteralTextHasARussianEntry, for the Rust sources of base, native and topo: every literal
    /// handed to L::t / L::n / lf! has a Russian entry (byte-for-byte the same English key as in C#).
    #[test]
    fn every_literal_text_of_the_foundation_has_a_russian_entry() {
        let ru = resource("ru").unwrap();
        let root = concat!(env!("CARGO_MANIFEST_DIR"), "/src");
        let mut missing = Vec::new();
        let mut found = 0;
        for area in ["base", "native", "topo"] {
            let dir = format!("{}/{}", root, area);
            for e in std::fs::read_dir(&dir).unwrap().flatten() {
                let path = e.path();
                if path.extension().map(|x| x != "rs").unwrap_or(true) {
                    continue;
                }
                let text = std::fs::read_to_string(&path).unwrap();
                // The product code only: the tests use made-up keys on purpose.
                let text = match text.find("#[cfg(test)]\nmod tests") {
                    Some(p) => text[..p].to_string(),
                    None => text,
                };
                // Comments are not product code either.
                let text: String = text.lines().filter(|l| !l.trim_start().starts_with("//")).collect::<Vec<_>>().join("\n");
                for lit in literals(&text) {
                    found += 1;
                    let format_only = !strip_placeholders(&lit).chars().any(|c| c.is_alphabetic());
                    if !format_only && !ru.map.get(&lit).map(|v| !v.trim().is_empty()).unwrap_or(false) {
                        missing.push(format!("{}: {}", path.file_name().unwrap().to_string_lossy(), lit));
                    }
                }
            }
        }
        assert!(found > 50, "only {} texts found: the scan is broken", found);
        assert!(missing.is_empty(), "no Russian text for:\n{}", missing.join("\n"));
    }

    fn strip_placeholders(s: &str) -> String {
        let mut out = String::new();
        let mut depth = 0;
        for c in s.chars() {
            match c {
                '{' => depth += 1,
                '}' if depth > 0 => depth -= 1,
                _ if depth == 0 => out.push(c),
                _ => {}
            }
        }
        out
    }

    /// The string literals right after `L::t(`, `L::n(` and `lf!(` (normal "..." literals with escapes).
    pub(crate) fn literals(text: &str) -> Vec<String> {
        let mut out = Vec::new();
        for pat in ["L::t(\"", "L::n(\"", "lf!(\""] {
            let mut from = 0;
            while let Some(p) = text[from..].find(pat) {
                let start = from + p + pat.len();
                let mut s = String::new();
                let mut chars = text[start..].chars();
                let mut ok = false;
                while let Some(c) = chars.next() {
                    match c {
                        '"' => {
                            ok = true;
                            break;
                        }
                        '\\' => match chars.next() {
                            Some('n') => s.push('\n'),
                            Some('t') => s.push('\t'),
                            Some('\\') => s.push('\\'),
                            Some('"') => s.push('"'),
                            Some('\'') => s.push('\''),
                            Some('\n') => {
                                // a line continuation: skip the leading whitespace of the next line
                                let rest: String = chars.clone().collect();
                                let skip = rest.len() - rest.trim_start().len();
                                for _ in 0..rest[..skip].chars().count() {
                                    chars.next();
                                }
                            }
                            Some(x) => s.push(x),
                            None => break,
                        },
                        x => s.push(x),
                    }
                }
                from = start;
                if ok && !s.is_empty() && !s.contains("L::") && s != "(" {
                    out.push(s);
                }
            }
        }
        out
    }
}
