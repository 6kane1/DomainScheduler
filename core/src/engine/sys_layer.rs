//! The preset layer (sections 10, 11 and 14; port of `SysLayer.cs`): on every pass the running processes that have
//! an enabled preset get it through Engine::apply_all. Built-in presets come from the INI; a user preset applies only
//! to a process on the watchlist. A process is opened again only when its thread set changed, so a pass over an
//! unchanged system opens nothing.

use super::engine::{Engine, Target};
use super::guard::Guard;
use crate::base::ci::{CiMap, CiMapExt, CiSet, CiSetExt};
use crate::base::hooks::lock;
use crate::base::model::{ApplyOptions, Preset};
use indexmap::{IndexMap, IndexSet};
use std::sync::{Arc, Mutex};

struct St {
    presets: CiMap<Preset>,
    watch: CiSet,
    aff_rules: CiMap<String>,
    /// pid -> signature of the thread set that was handled
    done: IndexMap<u32, u64>,
    /// pids that cannot be opened or are protected
    refused: IndexSet<u32>,
    reverts: i64,
    enabled: bool,
    mask: String,
    last_targets: i32,
    passes: i32,
}

/// `SysLayer`.
pub struct SysLayer {
    e: Arc<Engine>,
    st: Mutex<St>,
}

impl SysLayer {
    pub fn new(engine: Arc<Engine>) -> Self {
        SysLayer {
            e: engine,
            st: Mutex::new(St {
                presets: CiMap::new(),
                watch: CiSet::new(),
                aff_rules: CiMap::new(),
                done: IndexMap::new(),
                refused: IndexSet::new(),
                reverts: -1,
                enabled: true,
                mask: "auto".into(),
                last_targets: 0,
                passes: 0,
            }),
        }
    }

    pub fn last_targets(&self) -> i32 {
        lock(&self.st).last_targets
    }

    pub fn passes(&self) -> i32 {
        lock(&self.st).passes
    }

    /// New presets, watchlist, mask rules or switches: everything is applied again on the next pass.
    pub fn configure<S: AsRef<str>>(&self, presets: &CiMap<Preset>, watch: &[S], aff_rules: &CiMap<String>, mask_name: &str, enabled: bool) {
        let mut st = lock(&self.st);
        st.presets = CiMap::new();
        for (k, v) in presets {
            st.presets.ci_insert(k.as_str(), v.clone());
        }
        st.watch = CiSet::new();
        for w in watch {
            st.watch.ci_add(w.as_ref());
        }
        st.aff_rules = CiMap::new();
        for (k, v) in aff_rules {
            st.aff_rules.ci_insert(k.as_str(), v.clone());
        }
        st.mask = if mask_name.is_empty() { "auto".into() } else { mask_name.to_string() };
        st.enabled = enabled;
        st.done.clear();
        st.refused.clear();
    }

    pub fn invalidate(&self) {
        lock(&self.st).done.clear();
    }

    fn signature(tids: &[u32]) -> u64 {
        let mut s = tids.to_vec();
        s.sort();
        let mut h: u64 = 1469598103934665603;
        for t in &s {
            h = (h ^ *t as u64).wrapping_mul(1099511628211);
        }
        h ^ tids.len() as u64
    }

    /// Whether the preset of this exe may be applied, and how.
    fn options_for(st: &St, p: &Preset, exe: &str) -> Option<ApplyOptions> {
        if !p.enabled {
            return None;
        }
        if Guard::is_denied(exe) {
            return None;
        }
        let watched = st.watch.ci_contains(exe);
        if !p.builtin && !watched {
            return None; // a user preset needs the watchlist
        }
        if p.builtin && !st.enabled {
            return None;
        }
        // Only a watched process gets a mask; system presets touch threads and boost only.
        Some(ApplyOptions {
            mask_name: st.mask.clone(),
            do_mask: watched && !p.builtin,
            do_aff_rules: watched && !p.builtin,
            do_threads: true,
            sys: p.builtin,
            game: watched && !p.builtin,
            ..Default::default()
        })
    }

    /// One pass; skip = the process Auto is laying out itself (0 = none). Returns how many processes were applied.
    pub fn pass(&self, skip: u32) -> i32 {
        let mut st = lock(&self.st);
        st.passes += 1;
        if self.e.reverts() != st.reverts {
            st.done.clear(); // something was reverted: what this layer set may be gone
            st.reverts = self.e.reverts();
        }
        let aff = st.aff_rules.clone();
        let reports = {
            let st = &mut *st;
            self.e.apply_all(
                &mut |procs, tids| {
                    let mut list = Vec::new();
                    st.done.retain(|p, _| procs.contains_key(p));
                    st.refused.retain(|p| procs.contains_key(p));
                    for (pid, pr) in procs {
                        let pid = *pid;
                        if pid == 0 || pid == 4 || pid == skip || st.refused.contains(&pid) {
                            continue;
                        }
                        let preset = match st.presets.ci_get(&pr.0) {
                            Some(p) => p.clone(),
                            None => {
                                // A watched process with only a mask rule: an empty preset carries the mask.
                                if !st.watch.ci_contains(&pr.0) || !st.aff_rules.ci_contains(&pr.0) {
                                    continue;
                                }
                                Preset {
                                    exe: pr.0.clone(),
                                    class: String::new(),
                                    io: String::new(),
                                    mem: String::new(),
                                    eco: String::new(),
                                    boost: String::new(),
                                    ..Default::default()
                                }
                            }
                        };
                        let Some(o) = Self::options_for(st, &preset, &pr.0) else { continue };
                        let sig = Self::signature(tids.get(&pid).map(|v| v.as_slice()).unwrap_or(&[]));
                        if st.done.get(&pid) == Some(&sig) {
                            continue;
                        }
                        st.done.insert(pid, sig);
                        list.push(Target { pid, preset, options: o });
                    }
                    list
                },
                Some(&aff),
            )
        };
        for r in &reports {
            if r.handle_level < 0 || (!r.failures.is_empty() && r.named_hits == 0 && r.last_threads == 0) {
                st.refused.insert(r.pid as u32);
            }
        }
        st.reverts = self.e.reverts();
        st.last_targets = reports.len() as i32;
        reports.len() as i32
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::base::hooks::static_hooks;
    use crate::base::model::{ModRange, ThreadRule};
    use crate::base::settings::{MemorySettingsBackend, Settings};
    use crate::engine::guard::{Op, NO_RUNNING};
    use crate::engine::thread_ini::ThreadIni;
    use crate::native::fake::{FakeNative, P, T};
    use crate::topo::builder::LIVE_HOOK;
    use crate::topo::game_mask::IRQ_HOOK;
    use crate::topo::reservation::{FakeReserved, Reservation};
    use crate::topo::synth;

    pub(crate) fn system() -> Arc<FakeNative> {
        let f = FakeNative::cs2(500);
        {
            let mut s = f.lock();
            s.procs.insert(
                600,
                P {
                    name: "discord.exe".into(),
                    created: 5,
                    mods: vec![
                        ModRange { base: 0x1000, size: 0x1000, name: "discord_voice.node".into() },
                        ModRange { base: 0x9000, size: 0x1000, name: "Discord.exe".into() },
                    ],
                    ..Default::default()
                },
            );
            s.threads.insert(601, T { pid: 600, born: 1, start: 0x1010, ..Default::default() });
            s.threads.insert(602, T { pid: 600, born: 2, start: 0x9010, desc: "Compositor".into(), ..Default::default() });
            s.procs.insert(700, P::named("csrss.exe", 6));
            s.threads.insert(701, T { pid: 700, born: 1, ..Default::default() });
        }
        Arc::new(f)
    }

    pub(crate) fn hooks_on() -> crate::base::settings::BackendGuard {
        let t = Arc::new(synth::x7800x3d());
        LIVE_HOOK.set(Some(Arc::new(move || t.clone())));
        IRQ_HOOK.set(Some(Arc::new(Vec::new)));
        Reservation::set_reg(Some(Arc::new(FakeReserved::default())));
        Settings::scoped_backend(Arc::new(MemorySettingsBackend::new()))
    }

    pub(crate) fn hooks_off() {
        LIVE_HOOK.set(None);
        IRQ_HOOK.set(None);
        Reservation::set_reg(None);
    }

    /// SpecTests.SysLayerAppliesBuiltinsOnceAndUserPresetsOnlyOnTheWatchlist
    #[test]
    fn sys_layer_applies_builtins_once_and_user_presets_only_on_the_watchlist() {
        let _g = static_hooks();
        let _b = hooks_on();
        let f = system();
        let e = Arc::new(Engine::new(f.clone()));
        e.set_gate_hook(Some(Arc::new(|_pid, exe: &str| Guard::gate(exe, Op::Process, false, NO_RUNNING))));
        // Store.MarkBuiltin on an empty store: every shipped preset is copied in as a built-in.
        let mut presets: CiMap<Preset> = CiMap::new();
        for b in ThreadIni::load_bundled().presets.values() {
            let mut c = b.clone();
            c.builtin = true;
            presets.ci_insert(&b.exe, c);
        }
        presets.ci_insert(
            "cs2.exe",
            Preset {
                exe: "cs2.exe".into(),
                class: "above".into(),
                threads: vec![ThreadRule { match_: "desc".into(), pattern: "MainThrd".into(), prio: "above".into(), ..Default::default() }],
                ..Default::default()
            },
        );
        presets.ci_insert("csrss.exe", Preset { exe: "csrss.exe".into(), class: "above".into(), ..Default::default() });
        let sys = SysLayer::new(e.clone());
        let none: [&str; 0] = [];

        // cs2 is not on the watchlist: its user preset does nothing; discord gets the built-in one.
        sys.configure(&presets, &none, &CiMap::new(), "auto", true);
        assert_eq!(1, sys.pass(0));
        assert_eq!(1, f.lock().threads[&601].prio);
        assert_eq!(-1, f.lock().threads[&602].prio);
        assert!(f.lock().procs[&600].boost_disabled);
        assert_eq!(0, f.lock().threads[&100].prio);
        assert!(!f.lock().opened_pids.contains(&500));
        assert!(!f.lock().opened_pids.contains(&700)); // denylisted: never opened

        // Nothing changed in the system: the second pass opens nothing and writes nothing.
        let (opens, writes) = {
            let s = f.lock();
            (s.process_opens, s.writes)
        };
        assert_eq!(0, sys.pass(0));
        assert_eq!(opens, f.lock().process_opens);
        assert_eq!(writes, f.lock().writes);

        // On the watchlist the user preset applies; the process Auto owns is skipped.
        sys.configure(&presets, &["cs2.exe", "csrss.exe"], &CiMap::new(), "auto", true);
        sys.pass(500);
        assert_eq!(32, f.lock().procs[&500].cls);
        sys.pass(0);
        assert_eq!(32768, f.lock().procs[&500].cls);
        assert_eq!(1, f.lock().threads[&100].prio);
        assert!(!f.lock().opened_pids.contains(&700));

        // A revert drops the cache: the next pass puts the presets back.
        e.revert_all();
        assert_eq!(0, f.lock().threads[&601].prio);
        assert!(!f.lock().procs[&600].boost_disabled);
        sys.pass(0);
        assert_eq!(1, f.lock().threads[&601].prio);

        // The switch turns the built-ins off.
        e.revert_all();
        sys.configure(&presets, &none, &CiMap::new(), "auto", false);
        assert_eq!(0, sys.pass(0));
        hooks_off();
    }
}
