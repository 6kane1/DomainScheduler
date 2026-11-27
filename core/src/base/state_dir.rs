//! The state folder (1.15.0 Test 1.3, port of `StateDir.cs`): %ProgramData%\Domain OS Toolbox. The program runs with
//! administrator rights and acts on what is kept here (backups it restores at start, the session it recovers, the
//! boot guard, update downloads), so only SYSTEM and the Administrators group may change anything in it. Users may
//! read it (the log and a downloaded playbook have to open in Explorer); they cannot create, change, rename or delete
//! anything.
//!
//! The folder sits directly under ProgramData, where a standard user may create folders but may not delete or rename
//! anyone else's. Before the first use in a run its owner and permissions are checked:
//!  - missing: created with the protected permissions in the same call (no moment in which anybody else can open it);
//!  - owned by SYSTEM / Administrators / TrustedInstaller and nobody else can write: used; extra read-only entries
//!    (Explorer's "Continue" button adds one) are reset, the contents stay trusted;
//!  - anything else (another owner, a write entry for anybody else, inherited permissions, a link or a file with
//!    that name): somebody else could have put files there, so the whole thing is renamed to
//!    "Domain OS Toolbox.untrusted-<time>", never read, and a fresh folder is created.
//! When none of that works the folder is not used at all: every load returns "nothing saved" and every save fails.
//!
//! Also here: `Store.Dir` / `Store.DirShown` / `Store.DirHook` (the data folder every area uses). `Store.cs` belongs to
//! the system area; its `dir()` must call [`store_dir`] so there is one hook.

use super::hooks::{lock, Hook};
use crate::lf;
use std::path::PathBuf;
use std::sync::Mutex;

/// One entry of a folder's permission list (enough to judge it).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ace {
    pub sid: String,
    pub mask: i32,
    pub allow: bool,
    pub flags: i32,
    pub inherited: bool,
}

impl Ace {
    pub fn new(sid: &str, mask: i32, allow: bool, flags: i32, inherited: bool) -> Ace {
        Ace { sid: sid.to_string(), mask, allow, flags, inherited }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Trust {
    Trusted,
    ResetPermissions,
    Untrusted,
}

pub const SYSTEM_SID: &str = "S-1-5-18";
pub const ADMINS_SID: &str = "S-1-5-32-544";
pub const USERS_SID: &str = "S-1-5-32-545";
pub const TRUSTED_INSTALLER_SID: &str = "S-1-5-80-956008885-3418522649-1831038044-1853292631-2271478464";

struct State {
    ready: Option<PathBuf>,
    error: Option<String>,
    notes: Vec<String>,
}

static GATE: Mutex<State> = Mutex::new(State { ready: None, error: None, notes: Vec::new() });

/// `Store.DirHook` (tests point the data folder somewhere else).
pub static STORE_DIR_HOOK: Hook<dyn Fn() -> PathBuf + Send + Sync> = Hook::new();

/// `Store.Dir`: the data folder, checked before its first use. Err when it cannot be made safe (nothing is then read
/// from it or written to it).
pub fn store_dir() -> Result<PathBuf, String> {
    if let Some(h) = STORE_DIR_HOOK.get() {
        return Ok(h());
    }
    StateDir::ensure()
}

/// `Store.DirShown`: the data folder for display (never fails, nothing is checked).
pub fn store_dir_shown() -> PathBuf {
    if let Some(h) = STORE_DIR_HOOK.get() {
        return h();
    }
    StateDir::path_only()
}

/// `StateDir` (static class).
pub struct StateDir;

impl StateDir {
    pub const FOLDER_NAME: &'static str = "Domain OS Toolbox";

    /// Rights that change something: write/append data (= add file / add folder), extended attributes, delete a
    /// child, attributes, delete, permissions, owner, and the generic write/all bits (inherit-only entries).
    pub const WRITE_MASK: i32 = 0x2 | 0x4 | 0x10 | 0x40 | 0x100 | 0x10000 | 0x40000 | 0x80000 | 0x2000000 | 0x10000000 | 0x40000000;
    pub const FULL_CONTROL: i32 = 0x1F01FF;
    pub const READ_AND_EXECUTE: i32 = 0x1200A9;
    /// OBJECT_INHERIT_ACE | CONTAINER_INHERIT_ACE
    pub const INHERIT_BOTH: i32 = 0x1 | 0x2;

    /// Where the folder is (nothing is checked).
    pub fn path_only() -> PathBuf {
        crate::native::sys::common_app_data().join(Self::FOLDER_NAME)
    }

    /// Why the folder cannot be used in this run (None when it can or was not tried yet).
    pub fn error() -> Option<String> {
        lock(&GATE).error.clone()
    }

    /// What the check did (moved an untrusted folder away, reset the permissions): read once for the log.
    pub fn take_notes() -> Vec<String> {
        std::mem::take(&mut lock(&GATE).notes)
    }

    /// The folder, checked and made safe (once per run). Err when it cannot be made safe: then nothing in it is read
    /// or written in this run.
    pub fn ensure() -> Result<PathBuf, String> {
        let mut g = lock(&GATE);
        if let Some(r) = &g.ready {
            return Ok(r.clone());
        }
        if let Some(e) = &g.error {
            return Err(e.clone());
        }
        let p = Self::path_only();
        let r: Result<(), String> = {
            #[cfg(windows)]
            {
                let mut notes = Vec::new();
                let r = crate::native::acl::secure(&p, &mut notes);
                g.notes.extend(notes);
                r
            }
            #[cfg(not(windows))]
            {
                std::fs::create_dir_all(&p).map_err(|e| e.to_string())
            }
        };
        match r {
            Ok(()) => {
                g.ready = Some(p.clone());
                Ok(p)
            }
            Err(e) => {
                let msg = lf!("The state folder {0} is not safe to use: {1}", p.display().to_string(), e);
                g.error = Some(msg.clone());
                Err(msg)
            }
        }
    }

    /// The judgement of a folder's owner and permissions (pure). Untrusted: another owner, no permission list, or an
    /// allow entry that lets anybody but SYSTEM, Administrators or TrustedInstaller change something.
    /// ResetPermissions: safe, but not exactly ours (not protected from inheritance, extra or missing read entries).
    pub fn judge(owner_sid: Option<&str>, dacl_present: bool, dacl_protected: bool, aces: Option<&[Ace]>) -> Trust {
        let Some(aces) = aces else { return Trust::Untrusted };
        if !owner_sid.map(is_trusted_sid).unwrap_or(false) || !dacl_present {
            return Trust::Untrusted;
        }
        for a in aces {
            if a.allow && !is_trusted_sid(&a.sid) && (a.mask & Self::WRITE_MASK) != 0 {
                return Trust::Untrusted;
            }
        }
        if !dacl_protected || aces.iter().any(|a| a.inherited) {
            return Trust::ResetPermissions;
        }
        let mut want: Vec<(String, i32, bool, i32)> = Self::canonical().into_iter().map(|a| (a.sid, a.mask, a.allow, a.flags)).collect();
        let mut got: Vec<(String, i32, bool, i32)> = aces.iter().map(|a| (a.sid.clone(), a.mask, a.allow, a.flags & Self::INHERIT_BOTH)).collect();
        // OrderBy(Sid, Ordinal) is a stable sort by the SID only.
        want.sort_by(|a, b| a.0.cmp(&b.0));
        got.sort_by(|a, b| a.0.cmp(&b.0));
        if want == got { Trust::Trusted } else { Trust::ResetPermissions }
    }

    /// The permission list the folder gets: SYSTEM and Administrators full control, Users read; inherited by
    /// everything inside.
    pub fn canonical() -> Vec<Ace> {
        vec![
            Ace::new(SYSTEM_SID, Self::FULL_CONTROL, true, Self::INHERIT_BOTH, false),
            Ace::new(ADMINS_SID, Self::FULL_CONTROL, true, Self::INHERIT_BOTH, false),
            Ace::new(USERS_SID, Self::READ_AND_EXECUTE, true, Self::INHERIT_BOTH, false),
        ]
    }

    /// Tests: forget the check of this run.
    pub fn reset_for_tests() {
        let mut g = lock(&GATE);
        g.ready = None;
        g.error = None;
        g.notes.clear();
    }
}

fn is_trusted_sid(sid: &str) -> bool {
    sid == SYSTEM_SID || sid == ADMINS_SID || sid == TRUSTED_INSTALLER_SID
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::base::testrand::Rand;

    /// SecurityTests.StateFolderPermissionsAreJudgedStrictly
    #[test]
    fn state_folder_permissions_are_judged_strictly() {
        const SYS: &str = "S-1-5-18";
        const ADM: &str = "S-1-5-32-544";
        const USERS: &str = "S-1-5-32-545";
        const ME: &str = "S-1-5-21-1-2-3-1001";
        const CREATOR: &str = "S-1-3-0";
        let ours = StateDir::canonical();
        let with = |a: Ace| {
            let mut v = ours.clone();
            v.push(a);
            v
        };
        let j = |o: &str, p: bool, pr: bool, a: &[Ace]| StateDir::judge(Some(o), p, pr, Some(a));
        assert_eq!(Trust::Trusted, j(ADM, true, true, &ours));
        assert_eq!(Trust::Trusted, j(SYS, true, true, &ours));
        assert_eq!(Trust::Untrusted, j(ME, true, true, &ours)); // another owner
        assert_eq!(Trust::Untrusted, j(ADM, false, true, &ours)); // no list = everybody
        assert_eq!(Trust::Untrusted, j(ADM, true, true, &with(Ace::new(USERS, 0x2 | 0x4, true, 3, true)))); // ProgramData's inherited "create files"
        assert_eq!(Trust::Untrusted, j(ADM, true, true, &with(Ace::new(CREATOR, 0x10000000, true, 0xB, false))));
        assert_eq!(Trust::Untrusted, j(ADM, true, true, &with(Ace::new(ME, 0x40000, true, 0, false)))); // may change the permissions
        assert_eq!(Trust::ResetPermissions, j(ADM, true, true, &with(Ace::new(ME, 0x1200A9, true, 3, false)))); // Explorer's read entry
        assert_eq!(Trust::ResetPermissions, j(ADM, true, false, &ours)); // not protected from inheritance
        let deny_only: Vec<Ace> = with(Ace::new(ME, 0x40000, false, 0, false)).into_iter().filter(|a| a.allow).collect();
        assert_eq!(Trust::Trusted, j(ADM, true, true, &deny_only));
    }

    /// FuzzTests.FolderAnyoneElseCanWriteIsNeverTrusted
    #[test]
    fn folder_anyone_else_can_write_is_never_trusted() {
        let sids = [
            "S-1-5-18",
            "S-1-5-32-544",
            "S-1-5-80-956008885-3418522649-1831038044-1853292631-2271478464",
            "S-1-5-32-545",
            "S-1-1-0",
            "S-1-5-11",
            "S-1-5-21-1-2-3-1001",
            "S-1-5-4",
            "?",
        ];
        let trusted_sids = &sids[..3];
        let mut r = Rand::new(1610);
        let mut trusted = 0;
        for _ in 0..crate::base::testrand::iterations() * 3 {
            let owner = sids[r.next(sids.len() as i32) as usize];
            let mut aces = Vec::new();
            let n = r.next(6);
            let canonical = r.next(4) == 0;
            if canonical {
                aces.extend(StateDir::canonical());
            }
            for _ in 0..n {
                let mask = match r.next(3) {
                    0 => 0x1200A9,
                    1 => StateDir::WRITE_MASK,
                    _ => r.next_i32(),
                };
                aces.push(Ace::new(sids[r.next(sids.len() as i32) as usize], mask, r.next(3) > 0, r.next(4), r.next(5) == 0));
            }
            let present = r.next(8) > 0;
            let prot = r.next(2) == 0;
            let t = StateDir::judge(Some(owner), present, prot, Some(&aces));
            let someone_else_writes = aces.iter().any(|a| a.allow && !trusted_sids.contains(&a.sid.as_str()) && (a.mask & StateDir::WRITE_MASK) != 0);
            if !trusted_sids.contains(&owner) || !present || someone_else_writes {
                assert_eq!(Trust::Untrusted, t);
            }
            if t == Trust::Trusted {
                trusted += 1;
                assert!(prot);
                assert!(!aces.iter().any(|a| a.inherited));
            }
        }
        let _ = trusted;
        assert_eq!(Trust::Trusted, StateDir::judge(Some("S-1-5-32-544"), true, true, Some(&StateDir::canonical())));
    }
}
