//! The boot guard of the device interrupt changes (port of `BootGuard.cs`). They take effect at the next start of
//! Windows, and a bad one can stop Windows from starting. Before anything is written, a "pending" marker is saved with
//! the time and the current boot. A later boot confirms the changes once the program has run in it for
//! `CONFIRM_UPTIME` of uptime. At the start of the program the changes are reverted when
//!  - Windows went down unexpectedly since they were written (System log: Kernel-Power 41 or EventLog 6008), or
//!  - a boot after the change ended before the program could confirm it (it saw that boot, never confirmed it).
//!
//! "--revert-devices" reverts them without the window (also usable from Safe Mode).

use super::util::{create_parent, move_overwrite, parse_stj_date, read_all_text, run_capture, stj_date, write_all_text};
use crate::base::hooks::Hook;
use crate::base::l::L;
use crate::base::state_dir::store_dir;
use crate::base::time::{DateTime, TimeSpan};
use crate::lf;
use crate::native::sys_exe::SysExe;
use crate::topo::reservation::Reservation;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::path::PathBuf;

/// DateTime fields as System.Text.Json writes and reads them.
pub(crate) mod stj_dt {
    use super::*;
    pub fn serialize<S: Serializer>(d: &DateTime, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&stj_date(*d))
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<DateTime, D::Error> {
        let s = String::deserialize(d)?;
        parse_stj_date(&s).ok_or_else(|| serde::de::Error::custom(format!("The JSON value could not be converted to System.DateTime: {}", s)))
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct State {
    #[serde(rename = "Pending")]
    pub pending: bool,
    #[serde(rename = "AppliedUtc", with = "stj_dt")]
    pub applied_utc: DateTime,
    #[serde(rename = "AppliedBootUtc", with = "stj_dt")]
    pub applied_boot_utc: DateTime,
    /// A later boot in which the program ran but has not confirmed yet (default = none).
    #[serde(rename = "SeenBootUtc", with = "stj_dt")]
    pub seen_boot_utc: DateTime,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    Nothing,
    Wait,
    Revert,
}

/// What the home screen's "Optimize devices" button shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HomeDevices {
    Offer,
    BlockedByGame,
    AppliedRestartNeeded,
    AppliedActive,
}

pub static PATH_HOOK: Hook<dyn Fn() -> PathBuf + Send + Sync> = Hook::new();
/// Tests: the unexpected-shutdown check (applied time -> seen).
pub static CRASH_HOOK: Hook<dyn Fn(DateTime) -> bool + Send + Sync> = Hook::new();

/// `BootGuard` (static class).
pub struct BootGuard;

impl BootGuard {
    pub const CONFIRM_UPTIME: TimeSpan = TimeSpan::from_secs_i(120);
    /// Boot times are computed from the uptime; two readings of one boot differ by milliseconds.
    const SAME_BOOT_TOLERANCE: TimeSpan = TimeSpan::from_secs_i(90);
    pub const FILE_NAME: &'static str = "device-boot-guard.json";

    pub fn file_path() -> Result<PathBuf, String> {
        if let Some(h) = PATH_HOOK.get() {
            return Ok(h());
        }
        Ok(store_dir()?.join(Self::FILE_NAME))
    }

    pub fn same_boot(a: DateTime, b: DateTime) -> bool {
        a != DateTime::MIN && b != DateTime::MIN && (a.ticks - b.ticks).abs() <= Self::SAME_BOOT_TOLERANCE.ticks()
    }

    /// The decision at the start of the program (pure).
    pub fn decide(s: Option<&State>, boot_utc: DateTime, crash_since_apply: bool) -> Verdict {
        let Some(s) = s.filter(|s| s.pending) else { return Verdict::Nothing };
        if crash_since_apply {
            return Verdict::Revert;
        }
        // A boot after the change in which the program ran, but which ended before the confirmation.
        if s.seen_boot_utc != DateTime::MIN && !Self::same_boot(s.seen_boot_utc, boot_utc) && !Self::same_boot(s.seen_boot_utc, s.applied_boot_utc) {
            return Verdict::Revert;
        }
        Verdict::Wait
    }

    /// The changes are confirmed in a boot after the one they were written in, after 2 minutes of uptime (pure).
    pub fn can_confirm(s: Option<&State>, boot_utc: DateTime, uptime: TimeSpan) -> bool {
        matches!(s, Some(s) if s.pending && !Self::same_boot(boot_utc, s.applied_boot_utc) && uptime >= Self::CONFIRM_UPTIME)
    }

    /// Device changes were written in this boot: they start working after a restart of Windows.
    pub fn restart_pending() -> bool {
        matches!(Self::load(), Some(s) if s.pending && Self::same_boot(Reservation::boot_utc(), s.applied_boot_utc))
    }

    /// The home button's state (pure). applied: the device layout's backup exists. applied_utc: when the last device
    /// change was written (the boot guard's marker; default = no unconfirmed change). Windows has restarted since the
    /// change when the current boot started after it: the same boot time the boot guard compares.
    pub fn home_state(applied: bool, game_running: bool, applied_utc: DateTime, boot_utc: DateTime) -> HomeDevices {
        if !applied {
            return if game_running { HomeDevices::BlockedByGame } else { HomeDevices::Offer };
        }
        if applied_utc != DateTime::MIN && applied_utc > boot_utc { HomeDevices::AppliedRestartNeeded } else { HomeDevices::AppliedActive }
    }

    /// When the unconfirmed device change was written (default when there is none).
    pub fn pending_applied_utc() -> DateTime {
        match Self::load() {
            Some(s) if s.pending => s.applied_utc,
            _ => DateTime::MIN,
        }
    }

    pub fn load() -> Option<State> {
        let p = Self::file_path().ok()?;
        if !p.is_file() {
            return None;
        }
        let text = read_all_text(&p).ok()?;
        serde_json::from_str::<Option<State>>(&text).ok().flatten()
    }

    fn save(s: &State) -> Result<(), String> {
        let p = Self::file_path()?;
        create_parent(&p).map_err(|e| e.to_string())?;
        let tmp = PathBuf::from(format!("{}.tmp", p.display()));
        write_all_text(&tmp, &serde_json::to_string(s).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
        move_overwrite(&tmp, &p).map_err(|e| e.to_string())
    }

    pub fn clear() {
        if let Ok(p) = Self::file_path() {
            let _ = std::fs::remove_file(p);
        }
    }

    /// Called before any device interrupt value is written. A failure to save the marker stops the write.
    pub fn mark_pending() -> Result<(), String> {
        let s = match Self::load() {
            Some(mut s) if s.pending => {
                // Another change on top of an unconfirmed one: the new time counts, the boot stays the current one.
                s.applied_utc = DateTime::utc_now();
                s.applied_boot_utc = Reservation::boot_utc();
                s.seen_boot_utc = DateTime::MIN;
                s
            }
            _ => State { pending: true, applied_utc: DateTime::utc_now(), applied_boot_utc: Reservation::boot_utc(), seen_boot_utc: DateTime::MIN },
        };
        Self::save(&s)
    }

    /// At the start of the program: reverts the device changes when the decision says so, else remembers this boot.
    /// revert does the work (`Devices::revert_all`); returns lines for the log.
    pub fn on_start(revert: &dyn Fn() -> Option<String>) -> Vec<String> {
        let mut log = Vec::new();
        let Some(mut s) = Self::load().filter(|s| s.pending) else { return log };
        let boot = Reservation::boot_utc();
        let crash = Self::unexpected_shutdown_since(s.applied_utc);
        let v = Self::decide(Some(&s), boot, crash);
        if v == Verdict::Revert {
            let err = revert();
            log.push(
                if crash {
                    L::t("Windows went down unexpectedly after the device interrupt changes: they were reverted. Restart Windows.")
                } else {
                    L::t("Windows restarted before the device interrupt changes were confirmed: they were reverted. Restart Windows.")
                }
                .to_string(),
            );
            if let Some(e) = err {
                log.push(lf!("Devices: {0}", e));
            }
            Self::clear();
            return log;
        }
        if v == Verdict::Wait && !Self::same_boot(boot, s.applied_boot_utc) && !Self::same_boot(boot, s.seen_boot_utc) {
            s.seen_boot_utc = boot;
            let _ = Self::save(&s);
        }
        log
    }

    /// Clears the marker when the change has been confirmed in this boot. Returns a log line or None.
    pub fn confirm_if_due(uptime: TimeSpan) -> Option<String> {
        let s = Self::load();
        if !Self::can_confirm(s.as_ref(), Reservation::boot_utc(), uptime) {
            return None;
        }
        Self::clear();
        Some(L::t("Windows started fine with the device interrupt changes: they are confirmed").to_string())
    }

    /// Kernel-Power 41, EventLog 6008 or a bug check (WER 1001, blue screen) in the System log after the given time.
    pub fn unexpected_shutdown_since(utc: DateTime) -> bool {
        if let Some(h) = CRASH_HOOK.get() {
            return h(utc);
        }
        let since = utc.to_universal().format("yyyy-MM-ddTHH:mm:ss.fffZ", &crate::base::fmt::Culture::invariant());
        let q = format!(
            "*[System[((Provider[@Name='Microsoft-Windows-Kernel-Power'] and EventID=41) or (Provider[@Name='EventLog'] and EventID=6008) or (Provider[@Name='Microsoft-Windows-WER-SystemErrorReporting'] and EventID=1001)) and TimeCreated[@SystemTime>='{}']]]",
            since
        );
        let mut psi = SysExe::start("wevtutil.exe", false);
        psi.redirect_standard_output = true;
        for a in ["qe", "System", &format!("/q:{}", q), "/c:1", "/f:xml"] {
            psi.arg(a);
        }
        match run_capture(&psi, 15000) {
            Ok(r) if r.code.is_some() => r.stdout.contains("<Event"),
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::base::hooks::static_hooks;
    use crate::base::time::Kind;
    use crate::topo::reservation::BOOT_UTC_HOOK;
    use std::sync::atomic::{AtomicBool, AtomicI32, AtomicI64, Ordering};
    use std::sync::Arc;

    fn boot1() -> DateTime {
        DateTime::from_parts(2026, 11, 18, 10, 0, 0, Kind::Utc).unwrap()
    }
    fn boot2() -> DateTime {
        boot1().add_hours(2.0)
    }
    fn boot3() -> DateTime {
        boot1().add_hours(3.0)
    }

    fn pending(seen: DateTime) -> State {
        State { pending: true, applied_utc: boot1().add_minutes(30.0), applied_boot_utc: boot1(), seen_boot_utc: seen }
    }

    /// SafetyTests.BootGuardDecision
    #[test]
    fn boot_guard_decision() {
        assert_eq!(BootGuard::decide(None, boot2(), false), Verdict::Nothing);
        assert_eq!(BootGuard::decide(Some(&State::default()), boot2(), true), Verdict::Nothing);
        // Same boot as the change (the program was restarted, Windows was not): wait.
        assert_eq!(BootGuard::decide(Some(&pending(DateTime::MIN)), boot1().add_seconds(2.0), false), Verdict::Wait);
        // First boot after the change, no crash: wait for the 2-minute confirmation.
        assert_eq!(BootGuard::decide(Some(&pending(DateTime::MIN)), boot2(), false), Verdict::Wait);
        // The program started again in that same boot: still wait.
        assert_eq!(BootGuard::decide(Some(&pending(boot2())), boot2().add_ms(30.0), false), Verdict::Wait);
        // Windows went down unexpectedly since the change: revert.
        assert_eq!(BootGuard::decide(Some(&pending(DateTime::MIN)), boot2(), true), Verdict::Revert);
        // A boot after the change ended before it was confirmed: revert.
        assert_eq!(BootGuard::decide(Some(&pending(boot2())), boot3(), false), Verdict::Revert);
    }

    /// SafetyTests.BootGuardConfirmsOnlyInALaterBootAfterTwoMinutes
    #[test]
    fn boot_guard_confirms_only_in_a_later_boot_after_two_minutes() {
        let s = pending(DateTime::MIN);
        assert!(!BootGuard::can_confirm(Some(&s), boot1(), TimeSpan::from_hours(1.0))); // not restarted yet: nothing is active
        assert!(!BootGuard::can_confirm(Some(&s), boot2(), TimeSpan::from_secs(50.0))); // too early
        assert!(BootGuard::can_confirm(Some(&s), boot2(), TimeSpan::from_minutes(2.0)));
        assert!(!BootGuard::can_confirm(Some(&State::default()), boot2(), TimeSpan::from_hours(1.0)));
    }

    /// SafetyTests.BootGuardRevertsAtTheNextStartAfterACrash
    #[test]
    fn boot_guard_reverts_at_the_next_start_after_a_crash() {
        let _g = static_hooks();
        let dir = std::env::temp_dir().join(format!("ds-guard-{}", super::super::util::guid_n()));
        let d2 = dir.clone();
        PATH_HOOK.set(Some(Arc::new(move || d2.join("guard.json"))));
        let boot = Arc::new(AtomicI64::new(boot1().ticks));
        let b2 = boot.clone();
        BOOT_UTC_HOOK.set(Some(Arc::new(move || DateTime::utc(b2.load(Ordering::SeqCst)))));
        let crash = Arc::new(AtomicBool::new(false));
        let c2 = crash.clone();
        CRASH_HOOK.set(Some(Arc::new(move |_| c2.load(Ordering::SeqCst))));
        let reverts = AtomicI32::new(0);
        let revert = || -> Option<String> {
            reverts.fetch_add(1, Ordering::SeqCst);
            None
        };
        let set_boot = |d: DateTime| boot.store(d.ticks, Ordering::SeqCst);

        BootGuard::mark_pending().unwrap();
        assert!(BootGuard::load().unwrap().pending);
        assert!(BootGuard::on_start(&revert).is_empty()); // same boot: waits
        set_boot(boot2());
        assert!(BootGuard::on_start(&revert).is_empty()); // the next boot is seen
        assert_eq!(BootGuard::load().unwrap().seen_boot_utc, boot2());
        set_boot(boot3()); // that boot never confirmed
        assert_eq!(BootGuard::on_start(&revert).len(), 1);
        assert_eq!(reverts.load(Ordering::SeqCst), 1);
        assert!(BootGuard::load().is_none());

        set_boot(boot1());
        BootGuard::mark_pending().unwrap();
        set_boot(boot2());
        crash.store(true, Ordering::SeqCst); // Kernel-Power 41 since the change
        assert_eq!(BootGuard::on_start(&revert).len(), 1);
        assert_eq!(reverts.load(Ordering::SeqCst), 2);

        crash.store(false, Ordering::SeqCst);
        set_boot(boot1());
        BootGuard::mark_pending().unwrap();
        set_boot(boot2());
        assert!(BootGuard::confirm_if_due(TimeSpan::from_secs(30.0)).is_none());
        assert!(BootGuard::confirm_if_due(TimeSpan::from_minutes(3.0)).is_some());
        assert!(BootGuard::load().is_none());
        assert!(BootGuard::on_start(&revert).is_empty());
        assert_eq!(reverts.load(Ordering::SeqCst), 2);

        PATH_HOOK.set(None);
        CRASH_HOOK.set(None);
        BOOT_UTC_HOOK.set(None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Release1130Tests.HomeDeviceStateMatchesApplyAndBoot
    #[test]
    fn home_device_state_matches_apply_and_boot() {
        let boot = DateTime::from_parts(2026, 11, 18, 8, 0, 0, Kind::Utc).unwrap();
        let d = DateTime::MIN;
        assert_eq!(BootGuard::home_state(false, false, d, boot), HomeDevices::Offer);
        assert_eq!(BootGuard::home_state(false, true, d, boot), HomeDevices::BlockedByGame);
        // Applied in this boot: restart needed, a running game does not change that.
        assert_eq!(BootGuard::home_state(true, false, boot.add_minutes(30.0), boot), HomeDevices::AppliedRestartNeeded);
        assert_eq!(BootGuard::home_state(true, true, boot.add_minutes(30.0), boot), HomeDevices::AppliedRestartNeeded);
        // Applied before this boot (Windows restarted since), or confirmed (no unconfirmed change): active, no note.
        assert_eq!(BootGuard::home_state(true, false, boot.add_minutes(-5.0), boot), HomeDevices::AppliedActive);
        assert_eq!(BootGuard::home_state(true, true, boot.add_minutes(-5.0), boot), HomeDevices::AppliedActive);
        assert_eq!(BootGuard::home_state(true, false, d, boot), HomeDevices::AppliedActive);
    }

    /// The marker file reads what the C# version wrote (System.Text.Json dates, the default date for "none").
    #[test]
    fn reads_the_csharp_marker() {
        let s: State = serde_json::from_str(
            r#"{"Pending":true,"AppliedUtc":"2026-11-18T10:30:00.1234567Z","AppliedBootUtc":"2026-11-18T10:00:00Z","SeenBootUtc":"0001-01-01T00:00:00"}"#,
        )
        .unwrap();
        assert!(s.pending);
        assert_eq!(s.applied_boot_utc, boot1());
        assert_eq!(s.seen_boot_utc, DateTime::MIN);
        assert_eq!(
            serde_json::to_string(&s).unwrap(),
            r#"{"Pending":true,"AppliedUtc":"2026-11-18T10:30:00.1234567Z","AppliedBootUtc":"2026-11-18T10:00:00Z","SeenBootUtc":"0001-01-01T00:00:00"}"#
        );
    }
}
