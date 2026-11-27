//! Anti-cheat and protection rules (port of `Guard.cs`): a game under a kernel anti-cheat is never opened (not even
//! for reading), anti-cheat threads are never touched.

use crate::base::ci::{self, CiMap, CiMapExt, CiSet, CiSetExt};
use crate::base::l::L;
use crate::lf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op {
    Process,
    Thread,
}

/// Games whose kernel anti-cheat forbids touching them: exe -> anti-cheat.
fn ac_games() -> &'static CiMap<&'static str> {
    static V: OnceLock<CiMap<&'static str>> = OnceLock::new();
    V.get_or_init(|| {
        let mut d = CiMap::new();
        for (k, v) in [
            ("VALORANT-Win64-Shipping.exe", "Vanguard"),
            ("League of Legends.exe", "Vanguard"),
            ("FortniteClient-Win64-Shipping.exe", "Easy Anti-Cheat"),
            ("r5apex.exe", "Easy Anti-Cheat"),
            ("r5apex_dx12.exe", "Easy Anti-Cheat"),
            ("RustClient.exe", "Easy Anti-Cheat"),
            ("EscapeFromTarkov.exe", "BattlEye"),
            ("RainbowSix.exe", "BattlEye"),
            ("RainbowSix_BE.exe", "BattlEye"),
            ("TslGame.exe", "BattlEye"),
            ("DayZ_x64.exe", "BattlEye"),
            ("cod.exe", "Ricochet"),
            ("destiny2.exe", "BattlEye"),
            ("Overwatch.exe", "Defense Matrix"),
            ("GTA5.exe", "BattlEye"),
            ("eldenring.exe", "Easy Anti-Cheat"),
            ("FallGuys_client_game.exe", "Easy Anti-Cheat"),
            ("XDefiant.exe", "Ricochet"),
            ("thefinals.exe", "Easy Anti-Cheat"),
            ("deadlock.exe", "Valve"),
        ] {
            d.ci_insert(k, v);
        }
        d
    })
}

/// Anti-cheat clients that protect a game they run with (CS2 on FACEIT / ESEA).
fn ac_clients() -> &'static CiMap<&'static str> {
    static V: OnceLock<CiMap<&'static str>> = OnceLock::new();
    V.get_or_init(|| {
        let mut d = CiMap::new();
        for (k, v) in [
            ("faceit.exe", "FACEIT"),
            ("FACEITService.exe", "FACEIT"),
            ("faceitclient.exe", "FACEIT"),
            ("ESEAClient.exe", "ESEA"),
            ("eseaclient.exe", "ESEA"),
            ("vgc.exe", "Vanguard"),
            ("vgtray.exe", "Vanguard"),
        ] {
            d.ci_insert(k, v);
        }
        d
    })
}

/// Games that a running anti-cheat client protects.
fn client_protected() -> &'static CiSet {
    static V: OnceLock<CiSet> = OnceLock::new();
    V.get_or_init(|| ci::set_of(["cs2.exe", "csgo.exe"]))
}

const AC_MODULES: [&str; 19] = [
    "easyanticheat", "eac_", "eosac", "beclient", "battleye", "bedaisy", "vgc", "vgk", "vanguard", "faceit", "gameguard", "xigncode",
    "nprotect", "ricochet", "mhyprot", "acebase", "ace-", "anticheat", "esea",
];

/// System-critical processes: never modified (read-only diagnostics only).
fn critical() -> &'static CiSet {
    static V: OnceLock<CiSet> = OnceLock::new();
    V.get_or_init(|| {
        ci::set_of([
            "System", "Registry", "Idle", "[System Process]", "Secure System", "Memory Compression", "smss.exe", "csrss.exe", "wininit.exe",
            "services.exe", "lsass.exe", "MsMpEng.exe", "MpDefenderCoreService.exe", "NisSrv.exe", "fontdrvhost.exe",
        ])
    })
}

/// 1.16.2: CS2 may be tuned while the FACEIT client runs. FACEIT support confirmed in writing on 2026-11-25 (ticket of
/// 2026-11-21: priority class, thread priorities and preferred processors, affinity, IFEO CpuPriorityClass for
/// cs2.exe) that their anti-cheat team is fine with it. Only FACEIT and only cs2.exe; set from the "CS2 under FACEIT"
/// switch in Settings. ESEA, Vanguard and every other anti-cheat stay untouched; anti-cheat processes and threads of
/// anti-cheat modules inside the game are never touched either way. (`Guard.FaceitCs2Allowed`, volatile in C#.)
static FACEIT_CS2_ALLOWED: AtomicBool = AtomicBool::new(false);

/// `Guard` (static class).
pub struct Guard;

impl Guard {
    pub fn faceit_cs2_allowed() -> bool {
        FACEIT_CS2_ALLOWED.load(Ordering::SeqCst)
    }

    pub fn set_faceit_cs2_allowed(on: bool) {
        FACEIT_CS2_ALLOWED.store(on, Ordering::SeqCst);
    }

    /// The FACEIT client runs (one of its processes is in the list).
    pub fn faceit_running<S: AsRef<str>>(running_exes: Option<&[S]>) -> bool {
        match running_exes {
            None => false,
            Some(r) => r.iter().any(|x| ac_clients().ci_get(x.as_ref()).map(|c| *c == "FACEIT").unwrap_or(false)),
        }
    }

    pub fn is_critical(exe: &str) -> bool {
        critical().ci_contains(exe)
    }

    /// The built-in denylist: system-critical and integrity-sensitive processes.
    pub fn is_denied(exe: &str) -> bool {
        Self::is_critical(exe) || Self::is_anti_cheat_process(exe) || ac_games().ci_contains(exe)
    }

    /// Section 10: a process may be changed only when it is on the watchlist and not on the denylist. Returns the
    /// reason it may not, None when it may. `watch` None = C# null.
    pub fn eligibility<S: AsRef<str>>(exe: &str, watch: Option<&CiSet>, running_exes: Option<&[S]>) -> Option<String> {
        if exe.is_empty() {
            return Some(L::t("No process").to_string());
        }
        if let Some(why) = Self::gate(exe, Op::Process, false, running_exes) {
            return Some(why);
        }
        match watch {
            Some(w) if w.ci_contains(exe) => None,
            _ => Some(lf!("{0} is not on the watchlist", exe)),
        }
    }

    /// Anti-cheat processes and services: never opened, never lowered.
    pub fn is_anti_cheat_process(exe: &str) -> bool {
        ac_clients().ci_contains(exe) || AC_MODULES.iter().any(|m| ci::contains(exe, m))
    }

    pub fn is_anti_cheat(module: &str) -> bool {
        !module.is_empty() && AC_MODULES.iter().any(|m| ci::contains(module, m))
    }

    /// Why the program must not touch a process; None = it may. `running_exes` are the images running now (an
    /// anti-cheat client protects the games it runs with). Uses the shared FACEIT switch.
    pub fn gate<S: AsRef<str>>(exe: &str, op: Op, is_protected: bool, running_exes: Option<&[S]>) -> Option<String> {
        Self::gate_with(exe, op, is_protected, running_exes, Self::faceit_cs2_allowed())
    }

    /// The same with the FACEIT switch given explicitly (tests do not touch the shared flag).
    pub fn gate_with<S: AsRef<str>>(exe: &str, _op: Op, is_protected: bool, running_exes: Option<&[S]>, faceit_cs2_allowed: bool) -> Option<String> {
        if exe.is_empty() {
            return None;
        }
        if is_protected {
            return Some(lf!("{0} is a protected process", exe));
        }
        if Self::is_critical(exe) {
            return Some(lf!("{0} is a system-critical process", exe));
        }
        if let Some(ac) = ac_games().ci_get(exe) {
            return Some(lf!("{0} runs under {1}", exe, *ac));
        }
        if Self::is_anti_cheat_process(exe) {
            return Some(lf!("{0} is an anti-cheat", exe));
        }
        if client_protected().ci_contains(exe) {
            if let Some(running) = running_exes {
                for r in running {
                    if let Some(client) = ac_clients().ci_get(r.as_ref()) {
                        if *client == "FACEIT" && faceit_cs2_allowed && ci::eq(exe, "cs2.exe") {
                            continue;
                        }
                        return Some(lf!("{0} runs under {1}", exe, *client));
                    }
                }
            }
        }
        None
    }
}

/// `None::<&[&str]>` for the C# `runningExes = null` default.
pub const NO_RUNNING: Option<&[&str]> = None;

#[cfg(test)]
mod tests {
    use super::*;

    /// SpecTests.EligibilityNeedsTheWatchlistAndRespectsTheDenylist
    #[test]
    fn eligibility_needs_the_watchlist_and_respects_the_denylist() {
        let watch = ci::set_of(["cs2.exe", "lsass.exe", "VALORANT-Win64-Shipping.exe"]);
        assert!(Guard::eligibility("cs2.exe", Some(&watch), NO_RUNNING).is_none());
        assert!(Guard::eligibility("other.exe", Some(&watch), NO_RUNNING).is_some());
        assert!(Guard::eligibility("lsass.exe", Some(&watch), NO_RUNNING).is_some());
        assert!(Guard::eligibility("VALORANT-Win64-Shipping.exe", Some(&watch), NO_RUNNING).is_some());
        assert!(Guard::eligibility("cs2.exe", Some(&watch), Some(&["faceit.exe"][..])).is_some());
        assert!(Guard::is_denied("csrss.exe"));
        assert!(!Guard::is_denied("dwm.exe"));
    }

    /// SecurityTests.Cs2UnderFaceitIsTunedOnlyWithTheSwitchAndNoOtherAntiCheatIsOpened
    #[test]
    fn cs2_under_faceit_is_tuned_only_with_the_switch_and_no_other_anti_cheat_is_opened() {
        let p = Op::Process;
        assert!(Guard::gate_with("cs2.exe", p, false, Some(&["faceit.exe"][..]), false).is_some());
        assert!(Guard::gate_with("cs2.exe", p, false, Some(&["faceit.exe", "FACEITService.exe"][..]), true).is_none());
        assert!(Guard::gate_with("cs2.exe", p, false, Some(&["faceit.exe", "eseaclient.exe"][..]), true).is_some());
        assert!(Guard::gate_with("cs2.exe", p, false, Some(&["faceit.exe", "vgc.exe"][..]), true).is_some());
        assert!(Guard::gate_with("csgo.exe", p, false, Some(&["faceit.exe"][..]), true).is_some());
        assert!(Guard::gate_with("faceit.exe", p, false, NO_RUNNING, true).is_some());
        assert!(Guard::gate_with("VALORANT-Win64-Shipping.exe", p, false, NO_RUNNING, true).is_some());
    }
}
