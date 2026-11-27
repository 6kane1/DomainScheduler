//! Clean-up only (port of `DefenderGameExclusion.cs`). Earlier test builds could exclude the game's exe from Microsoft
//! Defender scanning while it ran and kept the paths they added in defender-exclusions.txt. The feature is gone;
//! RemoveAll still runs at start and at exit so an exclusion left behind by such a build (Toolbox killed, PC crashed)
//! is taken off. Nothing is ever added.
//!
//! The list lives in the admin-only state folder (an older version's list is taken over once, see LegacyImport), and
//! every line is checked again before use: only an absolute "X:\...\name.exe" path made of letters, digits and a few
//! plain punctuation marks passes; no quote of any kind (PowerShell also takes the typographic single and double
//! quotes as quotes), no $, backtick, ;, |, &, <, > or line break. The path never becomes part of a command: a fixed
//! PowerShell command reads it from an environment variable.

use crate::base::app_log::{AppLog, Sev};
use crate::base::ci::{self, CiSet, CiSetExt};
use crate::base::hooks::lock;
use crate::lf;
use crate::native::sys_exe::SysExe;
use std::io::Read;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};

static GATE: Mutex<()> = Mutex::new(());

/// One char of a path part: `[\p{Lu}\p{Ll}\p{Lt}\p{Lo}\p{Nd} _.()\[\]+,!#@%=~-]`. Rust has no general-category
/// tables; letters are its Alphabetic property and digits its Numeric one (see the report: this also admits Lm, Nl,
/// No and alphabetic marks; none of them is a quote or a command character).
fn part_char(c: char) -> bool {
    c.is_alphabetic() || c.is_numeric() || matches!(c, ' ' | '_' | '.' | '(' | ')' | '[' | ']' | '+' | ',' | '!' | '#' | '@' | '%' | '=' | '~' | '-')
}

/// `[A-Za-z]` with RegexOptions.IgnoreCase (the invariant case table also maps the Kelvin sign to k and the long s
/// to s).
fn drive_char(c: char) -> bool {
    c.is_ascii_alphabetic() || c == '\u{212A}' || c == '\u{17F}'
}

/// The C# regex `^[A-Za-z]:\\(?:[P]+\\)*[P]+\.exe$` (IgnoreCase; `$` also matches before one final line break).
fn safe_regex(line: &str) -> bool {
    let s = line.strip_suffix('\n').unwrap_or(line);
    let mut it = s.chars();
    match it.next() {
        Some(c) if drive_char(c) => {}
        _ => return false,
    }
    if it.next() != Some(':') || it.next() != Some('\\') {
        return false;
    }
    let rest: String = it.collect();
    let parts: Vec<&str> = rest.split('\\').collect();
    if parts.iter().any(|p| p.is_empty() || !p.chars().all(part_char)) {
        return false;
    }
    let last = parts[parts.len() - 1];
    // `[P]+\.exe`: at least one char before ".exe"
    ci::ends_with(last, ".exe") && last.chars().count() > 4
}

/// `DefenderGameExclusion` (static class).
pub struct DefenderGameExclusion;

impl DefenderGameExclusion {
    pub const FILE_NAME: &'static str = "defender-exclusions.txt";
    /// The fixed command: the path comes only from $env:DOMAINOS_EXCLUSION.
    pub const COMMAND: &'static str = "$ErrorActionPreference='Stop'; Remove-MpPreference -ExclusionProcess $env:DOMAINOS_EXCLUSION";

    fn file() -> Result<PathBuf, String> {
        Ok(crate::base::state_dir::store_dir()?.join(Self::FILE_NAME))
    }

    /// A line of the list may be used (pure): see the module summary.
    pub fn is_safe_path(line: &str) -> bool {
        line.encode_utf16().count() <= 260 && safe_regex(line) && !line.split('\\').any(|p| p == "." || p == "..")
    }

    /// Takes off every exclusion an earlier build added (listed in defender-exclusions.txt).
    pub fn remove_all() -> String {
        let _g = lock(&GATE);
        let Ok(file) = Self::file() else { return String::new() };
        let (paths, refused): (Vec<String>, Vec<String>) = {
            if !file.is_file() {
                return String::new();
            }
            let Ok(bytes) = std::fs::read(&file) else { return String::new() };
            let text = String::from_utf8_lossy(&bytes).to_string();
            let text = text.trim_start_matches('\u{feff}');
            let mut seen = CiSet::new();
            let mut all = Vec::new();
            // File.ReadAllLines: lines end at \r\n, \n or \r
            for l in text.split("\r\n").flat_map(|x| x.split('\n')).flat_map(|x| x.split('\r')) {
                let l = l.trim();
                if !l.is_empty() && seen.ci_add(l) {
                    all.push(l.to_string());
                }
            }
            (all.iter().filter(|l| Self::is_safe_path(l)).cloned().collect(), all.iter().filter(|l| !Self::is_safe_path(l)).cloned().collect())
        };
        if !refused.is_empty() {
            let shown: Vec<String> = refused
                .iter()
                .take(5)
                .map(|r| if r.encode_utf16().count() > 120 { format!("{}…", utf16_prefix(r, 120)) } else { r.clone() })
                .collect();
            AppLog::add_sev(
                lf!(
                    "Defender: lines of the exclusion list that are not plain program paths were not used (remove such an exclusion by hand in Windows Security if it is there): {0}",
                    shown.join(" | ")
                ),
                Sev::Warn,
            );
        }
        if paths.is_empty() {
            let _ = std::fs::remove_file(&file);
            return String::new();
        }
        let mut left = Vec::new();
        for p in &paths {
            let (ok, _) = Self::ps(p);
            if !ok {
                left.push(p.clone());
            }
        }
        let _ = if left.is_empty() { std::fs::remove_file(&file) } else { std::fs::write(&file, left.iter().map(|l| format!("{}\r\n", l)).collect::<String>()) };
        if left.is_empty() {
            let names: Vec<String> = paths.iter().map(|p| p.rsplit(['\\', '/']).next().unwrap_or(p).to_string()).collect();
            lf!("Defender: exclusion taken off ({0})", names.join(", "))
        } else {
            lf!("Defender: could not take off {0}; it is retried at the next start", left.join(", "))
        }
    }

    fn ps(path: &str) -> (bool, String) {
        let psi = SysExe::power_shell(Self::COMMAND, &[("DOMAINOS_EXCLUSION", path)]);
        let mut child = match psi.command().spawn() {
            Ok(c) => c,
            Err(e) => return (false, e.to_string()),
        };
        let mut err_pipe = child.stderr.take();
        let err_reader = std::thread::spawn(move || {
            let mut s = String::new();
            if let Some(p) = err_pipe.as_mut() {
                let _ = p.read_to_string(&mut s);
            }
            s
        });
        let mut o = String::new();
        if let Some(mut p) = child.stdout.take() {
            let _ = p.read_to_string(&mut o);
        }
        let start = Instant::now();
        let status = loop {
            match child.try_wait() {
                Ok(Some(s)) => break Some(s),
                Ok(None) if start.elapsed() < Duration::from_millis(15000) => std::thread::sleep(Duration::from_millis(20)),
                Ok(None) => break None,
                Err(e) => return (false, e.to_string()),
            }
        };
        let Some(status) = status else { return (false, "timeout".into()) };
        let e = err_reader.join().unwrap_or_default();
        if status.code() == Some(0) {
            (true, o)
        } else {
            let t = if !e.is_empty() { e } else { o };
            (false, t.trim().split('\n').next().unwrap_or("").to_string())
        }
    }
}

/// `s[..n]` counted in UTF-16 units (C# string indexing).
fn utf16_prefix(s: &str, n: usize) -> String {
    let u: Vec<u16> = s.encode_utf16().take(n).collect();
    String::from_utf16_lossy(&u)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::base::hooks::static_hooks;
    use crate::base::state_dir::STORE_DIR_HOOK;
    use std::sync::Arc;

    /// SecurityTests.DefenderListRefusesInjectionAndNeverBuildsACommandFromIt
    #[test]
    fn defender_list_refuses_injection_and_never_builds_a_command_from_it() {
        let _g = static_hooks();
        assert!(DefenderGameExclusion::is_safe_path(r"C:\Games\Counter-Strike Global Offensive\game\bin\win64\cs2.exe"));
        assert!(DefenderGameExclusion::is_safe_path(r"D:\SteamLibrary\steamapps\common\Игра (2024)\game.exe"));
        let payloads = [
            "C:\\Games\\a\u{2018}; Start-Process calc; \u{2018}.exe",
            "C:\\Games\\a\u{2019}+$(calc)+\u{2019}.exe",
            "C:\\Games\\a\u{201A};calc;\u{201B}.exe",
            "C:\\Games\\a\u{201C};calc;\u{201D}.exe",
            "C:\\Games\\a';calc;'.exe",
            "C:\\Games\\$(calc).exe",
            "C:\\Games\\a`;calc.exe",
            "C:\\Games\\a|calc.exe",
            "C:\\Games\\a&calc.exe",
            "C:\\Games\\a>x.exe",
            "C:\\Games\\a\ncalc.exe",
            "C:\\Games\\..\\Windows\\a.exe",
            "\\\\server\\share\\a.exe",
            "C:\\Games\\game.bat",
        ];
        for p in payloads {
            assert!(!DefenderGameExclusion::is_safe_path(p), "{}", p);
        }

        // The path only travels in an environment variable; the command is fixed.
        let payload = payloads[0];
        let psi = SysExe::power_shell(DefenderGameExclusion::COMMAND, &[("DOMAINOS_EXCLUSION", payload)]);
        assert!(!psi.arguments.iter().any(|a| a.contains("calc")));
        assert_eq!(payload, psi.environment.iter().find(|(k, _)| k.as_str() == "DOMAINOS_EXCLUSION").unwrap().1);
        assert!(psi.arguments.iter().any(|a| a == "-NoProfile"));
        assert!(psi.arguments.iter().any(|a| a == "-NonInteractive"));
        assert_eq!(&SysExe::modules_path(), psi.environment.iter().find(|(k, _)| ci::eq(k.as_str(), "PSModulePath")).unwrap().1);
        assert!(psi.file_name.ends_with(r"WindowsPowerShell\v1.0\powershell.exe"));

        // A planted list with only the payload: nothing is started, the file goes, the log says so.
        let dir = std::env::temp_dir().join(format!("ds-state-def-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let d2 = dir.clone();
        STORE_DIR_HOOK.set(Some(Arc::new(move || d2.clone())));
        AppLog::clear();
        std::fs::write(dir.join(DefenderGameExclusion::FILE_NAME), format!("{}\n", payload)).unwrap();
        assert_eq!("", DefenderGameExclusion::remove_all());
        assert!(!dir.join(DefenderGameExclusion::FILE_NAME).exists());
        assert!(AppLog::tail(500).iter().any(|e| e.sev == Sev::Warn && e.msg.contains("Defender")));
        STORE_DIR_HOOK.set(None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
