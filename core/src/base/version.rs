//! The program version and `AssemblyInfo.cs`.
//!
//! The version has one source: `version` in core/Cargo.toml ([package]), which cargo hands to the build as
//! CARGO_PKG_VERSION. The release script (release/hub/domainos.py, set_toolbox_version) writes it there and into
//! Cargo.lock. Test builds carry a pre-release tail ("1.17.1-test1"), as the C# `<Version>` did; a "+build" tail is cut
//! off by `full()` (C# `AppVersion.Full`).
//!
//! `AssemblyInfo.cs` holds one attribute: every DllImport is loaded from System32 only (never from the program's
//! folder, the current folder or PATH, where another program could put a file with the same name). The native exe
//! gets the same with `native::sys::harden_dll_search()` (SetDefaultDllDirectories(LOAD_LIBRARY_SEARCH_SYSTEM32)),
//! which the start-up code calls first; libraries loaded at run time (nvapi64, atiadlxx) are loaded with
//! LOAD_LIBRARY_SEARCH_SYSTEM32.

/// The version as built ("1.17.0", a test build "1.17.1-test1").
pub const INFORMATIONAL_VERSION: &str = env!("CARGO_PKG_VERSION");

/// `AppVersion.Full` before the test override: the version without a "+build" tail.
pub fn full() -> &'static str {
    match INFORMATIONAL_VERSION.find('+') {
        Some(p) => &INFORMATIONAL_VERSION[..p],
        None => INFORMATIONAL_VERSION,
    }
}

#[cfg(test)]
mod tests {
    /// The version is the one written in core/Cargo.toml, and it has the shape the update feeds compare
    /// (major.minor.patch, optionally -testN or -testN.M).
    #[test]
    fn version_comes_from_cargo_toml() {
        let toml = include_str!("../../Cargo.toml");
        let pkg = toml.split("[package]").nth(1).unwrap();
        let v = pkg.lines().find_map(|l| l.trim().strip_prefix("version = ")).unwrap().trim_matches('"');
        assert_eq!(super::INFORMATIONAL_VERSION, v);
        assert_eq!(super::full(), v.split('+').next().unwrap());
        let (core, tail) = match super::full().split_once("-test") {
            Some((c, t)) => (c, Some(t)),
            None => (super::full(), None),
        };
        let parts: Vec<&str> = core.split('.').collect();
        assert_eq!(parts.len(), 3, "major.minor.patch: {core}");
        assert!(parts.iter().all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit())), "{core}");
        if let Some(t) = tail {
            assert!(!t.is_empty() && t.split('.').all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit())), "{t}");
        }
    }

    /// The lock file names the same version (builds use cargo --locked).
    #[test]
    fn cargo_lock_has_the_same_version() {
        let lock = include_str!("../../Cargo.lock");
        let want = format!("name = \"dos_core\"\nversion = \"{}\"", super::INFORMATIONAL_VERSION);
        assert!(lock.replace("\r\n", "\n").contains(&want), "Cargo.lock: dos_core is not {}", super::INFORMATIONAL_VERSION);
    }
}
