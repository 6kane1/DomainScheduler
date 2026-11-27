//! The pure decisions of the Auto mode (sections 12.5-12.7, 17.2; port of `AutoLogic.cs` with `AutoPolicy`,
//! `ThreadRole` and `Classified`).

use super::engine::Engine;
use super::guard::Guard;
use crate::base::ci;
use crate::base::l::L;
use crate::base::model::{CpuSet, GameMaskInfo, MaskResult, Preset, ThreadRow, ThreadRule, ThreadSnap};
use crate::topo::builder::TopoBuilder;
use crate::topo::game_mask::{GameMask, BUSY_HOOK};
use crate::topo::masks::Masks;
use crate::topo::model::Topo;
use crate::topo::reservation::Reservation;
use indexmap::{IndexMap, IndexSet};
use std::collections::HashMap;

/// Every Auto parameter (section 12.3): constants, never read from the UI or the registry.
pub struct AutoPolicy;

impl AutoPolicy {
    pub const ENTER_STABLE_MS: i32 = 5000;
    pub const EXIT_STABLE_MS: i32 = 15000;
    pub const REAPPLY_MS: i32 = 5000;
    pub const MIN_GAME_LP: i32 = 4;
    pub const OS_RESERVED_CORES: i32 = 1;
    pub const MIN_MASK_FRACTION: f64 = 0.5;
    pub const HOT_RATE_FRACTION: f64 = 0.05;
    pub const COVER_TARGET: f64 = 0.80;
    pub const CONFIDENCE_FLOOR: f64 = 0.60;
    pub const GAME_CLASS: &'static str = "above";
    pub const BG_CLASS: &'static str = "below";
    pub const ARMING_TIMEOUT_MS: i32 = 8000;
    pub const BASELINE_MS: i32 = 5000;
    pub const WARMUP_MS: i32 = 700;
    pub const WARMUP_ROUNDS: i32 = 3;
    pub const IRQ_MODE: &'static str = GameMask::CORE_MODE;
    /// of one core
    pub const GAME_CPU_SHARE: f64 = 0.15;
    /// thread composition change that rebuilds the plan
    pub const PLAN_CHANGE_FRACTION: f64 = 0.25;
    pub const DENIED_SHARE_LIMIT: f64 = 0.30;
    pub const THREAD_DROP_FACTOR: i32 = 3;
    pub const MAX_APPLY_ERRORS: i32 = 3;
    pub const MIN_FRAMES_FOR_BASELINE: i32 = 200;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum ThreadRole {
    #[default]
    Main,
    Render,
    Audio,
    Worker,
    Aux,
    Skip,
}

impl ThreadRole {
    /// Main, Render, Audio or Worker (the roles that get a slot).
    pub fn is_slot(self) -> bool {
        matches!(self, ThreadRole::Main | ThreadRole::Render | ThreadRole::Audio | ThreadRole::Worker)
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Classified {
    pub row: ThreadRow,
    pub role: ThreadRole,
    pub lp: u32,
    pub core: u8,
    pub llc: u8,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MaskPlan {
    pub spec: String,
    /// C# null until resolved.
    pub resolved: Option<MaskResult>,
    pub irq: Option<GameMaskInfo>,
    /// 0: no mask is applied
    pub mask: u64,
    pub satisfied: bool,
    pub notes: Vec<String>,
}

impl Default for MaskPlan {
    fn default() -> Self {
        MaskPlan { spec: String::new(), resolved: None, irq: None, mask: 0, satisfied: false, notes: Vec::new() }
    }
}

/// `AutoLogic` (static class).
pub struct AutoLogic;

impl AutoLogic {
    /// 1.16.2: the mask is used only above this many physical cores (owner: "only on some CPUs with more than 8 cores").
    pub const MASK_MIN_CORES: i32 = 8;
    /// kept for the device layout (IrqLayout): no device cores on 6 cores or fewer
    pub const SMALL_CPU_CORES: i32 = 6;

    // ---- mask (12.5) ------------------------------------------------------------------------------

    pub fn pick_mask(t: &Topo) -> &'static str {
        if t.split().can_split() && t.split().family == "x3d" {
            "vcache"
        } else if t.hybrid() {
            "pcores"
        } else {
            "auto"
        }
    }

    pub fn domains(t: &Topo, m: u64) -> i32 {
        let s: IndexSet<u8> = t.sets.iter().filter(|s| s.lp < 64 && ((m >> s.lp) & 1) != 0).map(|s| s.llc).collect();
        s.len() as i32
    }

    fn groups_by_llc(t: &Topo) -> IndexMap<u8, Vec<&CpuSet>> {
        let mut g: IndexMap<u8, Vec<&CpuSet>> = IndexMap::new();
        for s in &t.sets {
            g.entry(s.llc).or_default().push(s);
        }
        g
    }

    /// A single L3 domain can hold the game (at least MinGameLp processors).
    pub fn has_single_domain(t: &Topo) -> bool {
        Self::groups_by_llc(t).values().any(|g| g.len() as i32 >= AutoPolicy::MIN_GAME_LP)
    }

    /// The best single domain: the largest L3, then the most processors, then the lowest index.
    pub fn best_single_domain(t: &Topo) -> u64 {
        let mut g: Vec<(u8, Vec<&CpuSet>)> = Self::groups_by_llc(t).into_iter().filter(|(_, g)| g.len() as i32 >= AutoPolicy::MIN_GAME_LP).collect();
        g.sort_by(|a, b| {
            let la = t.l3_by_llc.get(&a.0).copied().unwrap_or(0);
            let lb = t.l3_by_llc.get(&b.0).copied().unwrap_or(0);
            lb.cmp(&la).then(b.1.len().cmp(&a.1.len())).then(a.0.cmp(&b.0))
        });
        // C# First() throws on an empty list; callers check HasSingleDomain first.
        g.first().map(|(_, sets)| TopoBuilder::mask(sets.iter().copied())).unwrap_or(0)
    }

    pub fn harden(t: &Topo, p: u64) -> u64 {
        let full = t.full_mask();
        if p == 0 || TopoBuilder::pop_count(p) < AutoPolicy::MIN_GAME_LP {
            return full;
        }
        if (TopoBuilder::pop_count(p) as f64) < TopoBuilder::pop_count(full) as f64 * AutoPolicy::MIN_MASK_FRACTION {
            return full;
        }
        if Self::domains(t, p) > 1 && Self::has_single_domain(t) {
            return Self::best_single_domain(t);
        }
        p
    }

    /// 1.16.1 Test 4: a processor with ONE cache domain and no E-cores keeps every processor for the game, at any core
    /// count (6 or fewer before). The mask only helps where it separates something: two CCDs (the V-Cache die) or P-
    /// and E-cores. Measurements (owner's 7800X3D, CS2 deathmatch, 22.11.2026, mask off/on alternated in one session,
    /// 3 runs each): without the mask the game's main threads never waited for a core longer than 0.5 ms (13 times per
    /// 30 s with it), the longest wait fell from 940 to 202 us, the longest frame from 14.6 to 11.3 ms; 1% / 0.1% low
    /// equal, runs far steadier. Earlier: a tester's i5-10400F (6 cores) lost 6% of its 1% low with the mask.
    /// Setting "AutoMask": auto (this rule), on (always the mask), off (never a mask); the Core mask button sets on/off.
    pub fn keeps_all_cores(t: Option<&Topo>, setting: &str) -> bool {
        let setting = ci::lower(setting.trim());
        if setting == "off" {
            return true;
        }
        let Some(t) = t else { return false };
        if setting == "on" {
            return false;
        }
        // 1.16.2: the mask only on processors with MORE than 8 cores of two kinds (P- and E-cores, or two CCDs /
        // caches). 8 cores or fewer (a Ryzen 3700X with two CCX caches, a laptop i5 with 4 P + 4 E) keep every core.
        if t.physical_count > 0 && t.physical_count <= Self::MASK_MIN_CORES {
            return true;
        }
        !t.hybrid() && t.physical_count > 0 && t.llc_groups.len() <= 1
    }

    pub fn all_cores(t: Option<&Topo>, mp: MaskPlan) -> MaskPlan {
        let Some(t) = t else { return mp };
        if !mp.satisfied {
            return mp;
        }
        MaskPlan {
            spec: format!("{} (all processors)", mp.spec),
            resolved: mp.resolved,
            irq: mp.irq,
            mask: t.full_mask(),
            satisfied: true,
            notes: Vec::new(),
        }
    }

    /// PickMask -> Resolve -> ForRule(IRQ, Core) -> Unmix(false) -> Harden; unsatisfied -> no mask.
    pub fn build_mask(t: &Topo, reserved: u64) -> MaskPlan {
        let mut mp = MaskPlan { spec: Self::pick_mask(t).to_string(), ..Default::default() };
        let resolved = Masks::resolve(t, &mp.spec);
        mp.satisfied = resolved.satisfied;
        if !mp.satisfied {
            mp.notes.extend(resolved.notes.iter().map(|n| Masks::note_text(n)));
            mp.resolved = Some(resolved);
            return mp;
        }
        let busy = BUSY_HOOK.get().and_then(|h| h(t));
        let irq = match busy {
            Some(b) => GameMask::for_live(t, resolved.mask, b),
            None => GameMask::for_rule(t, resolved.mask, AutoPolicy::IRQ_MODE),
        };
        let m = Reservation::unmix(irq.final_, reserved, false);
        mp.mask = Self::harden(t, m);
        if irq.irq_ignored {
            mp.notes.push(L::t("IRQ exclusion ignored: it would cut more than half of the mask").to_string());
        }
        mp.resolved = Some(resolved);
        mp.irq = Some(irq);
        mp
    }

    // ---- threads (12.6) ---------------------------------------------------------------------------

    fn hint(d: &str, m: &str, words: &[&str]) -> bool {
        words.iter().any(|w| d.contains(w) || m.contains(w))
    }

    pub fn total_rate(s: &ThreadSnap) -> u64 {
        s.rows.iter().fold(0u64, |a, r| a.wrapping_add(r.rate))
    }

    pub fn hot_threshold(s: &ThreadSnap) -> u64 {
        (Self::total_rate(s) as f64 * AutoPolicy::HOT_RATE_FRACTION) as u64
    }

    pub fn classify(r: &ThreadRow, s: &ThreadSnap) -> ThreadRole {
        if Guard::is_anti_cheat(Engine::module_of(&r.sym)) {
            return ThreadRole::Skip;
        }
        let d = ci::lower(&r.desc);
        let m = ci::lower(Engine::module_of(&r.sym));
        if Self::hint(&d, &m, &["audio", "wasapi", "xaudio", "fmod", "wwise", "dsound"]) {
            return ThreadRole::Audio;
        }
        if Self::hint(&d, &m, &["render", "gfx", "d3d", "vulkan", "opengl", "nvapi", "atidxx"]) {
            return ThreadRole::Render;
        }
        if !s.rows.is_empty() && Some(r.tid) == s.rows.iter().map(|x| x.tid).min() {
            return ThreadRole::Main;
        }
        if r.rate > Self::hot_threshold(s) {
            return ThreadRole::Worker;
        }
        ThreadRole::Aux
    }

    pub fn confidence(s: &ThreadSnap, plan: &[Classified]) -> f64 {
        let n = (s.rows.len().max(1)) as f64;
        let s1 = s.rows.iter().filter(|r| !r.desc.is_empty()).count() as f64 / n;
        let s2 = s.rows.iter().filter(|r| !Engine::module_of(&r.sym).is_empty()).count() as f64 / n;
        let s3 = if s.cycle_fallback_tids.is_empty() { 1.0 } else { 0.5 };
        let s4 = if plan.iter().any(|x| x.role == ThreadRole::Main) { 1.0 } else { 0.0 };
        let s5 = if s.rows.iter().filter(|r| r.rate > 0).count() >= 2 { 1.0 } else { 0.0 };
        0.25 * s1 + 0.25 * s2 + 0.15 * s3 + 0.20 * s4 + 0.15 * s5
    }

    fn order(r: ThreadRole) -> i32 {
        match r {
            ThreadRole::Main => 0,
            ThreadRole::Render => 1,
            ThreadRole::Audio => 2,
            _ => 3,
        }
    }

    /// The pin plan (12.7): roles Main/Render/Audio/Worker, at most cores-1 threads, one thread per physical core;
    /// Main and Render on different cores of one L3 domain; workers only until their rate covers CoverTarget of the
    /// process; earlier LPs are kept (sticky). `previous`: tid -> LP of the earlier plan.
    pub fn plan(t: &Topo, s: &ThreadSnap, mask: u64, previous: Option<&HashMap<u32, u32>>) -> Vec<Classified> {
        let cores = Masks::ideal_candidates(t, true, mask);
        let budget = (cores.len() as i32 - AutoPolicy::OS_RESERVED_CORES).max(1) as usize;
        let classified: Vec<Classified> = s.rows.iter().map(|r| Classified { row: r.clone(), role: Self::classify(r, s), ..Default::default() }).collect();
        let mut pick: Vec<Classified> = classified.into_iter().filter(|x| x.role.is_slot()).collect();
        // OrderBy(role order).ThenByDescending(rate): a stable sort
        pick.sort_by(|a, b| Self::order(a.role).cmp(&Self::order(b.role)).then(b.row.rate.cmp(&a.row.rate)));
        let total = Self::total_rate(s);
        let mut covered: u64 = 0;
        let mut plan: Vec<Classified> = Vec::new();
        for c in pick {
            if plan.len() >= budget {
                break;
            }
            if c.role == ThreadRole::Worker && total > 0 && covered as f64 >= total as f64 * AutoPolicy::COVER_TARGET {
                continue; // cold ones are not added
            }
            covered = covered.wrapping_add(c.row.rate);
            plan.push(c);
        }
        // Processors: one thread per physical core; Main and Render on different cores of one L3 domain; a thread
        // keeps the processor of the previous plan when it can (sticky).
        let mut used_cores: IndexSet<u8> = IndexSet::new();
        let mut done = vec![false; plan.len()];
        if let Some(prev) = previous {
            for (i, c) in plan.iter_mut().enumerate() {
                let Some(lp) = prev.get(&c.row.tid) else { continue };
                let cs = cores.iter().find(|x| x.lp as u32 == *lp).copied().unwrap_or_default();
                if cores.iter().any(|x| x.lp as u32 == *lp) && used_cores.insert(cs.core) {
                    Self::assign(c, &cs);
                    done[i] = true;
                }
            }
        }
        let mut main_llc: Option<u8> = plan.iter().enumerate().filter(|(i, c)| c.role == ThreadRole::Main && done[*i]).map(|(_, c)| c.llc).next();
        for i in 0..plan.len() {
            if done[i] {
                continue;
            }
            let mut free: Vec<CpuSet> = cores.iter().filter(|x| !used_cores.contains(&x.core)).copied().collect();
            if plan[i].role == ThreadRole::Render {
                if let Some(ml) = main_llc {
                    let same: Vec<CpuSet> = free.iter().filter(|x| x.llc == ml).copied().collect();
                    if !same.is_empty() {
                        free = same;
                    }
                }
            }
            if free.is_empty() {
                break;
            }
            let pick_core = free[0];
            used_cores.insert(pick_core.core);
            Self::assign(&mut plan[i], &pick_core);
            done[i] = true;
            if plan[i].role == ThreadRole::Main {
                main_llc = Some(pick_core.llc);
            }
        }
        plan.into_iter().zip(done).filter(|(_, d)| *d).map(|(c, _)| c).collect()
    }

    fn assign(c: &mut Classified, s: &CpuSet) {
        c.lp = s.lp as u32;
        c.core = s.core;
        c.llc = s.llc;
    }

    pub fn role_prio(r: ThreadRole) -> &'static str {
        match r {
            ThreadRole::Main | ThreadRole::Render | ThreadRole::Audio => "above",
            _ => "",
        }
    }

    /// The synthesized preset (17.2): numeric ideal processors, Match = tid. `hot_first` (22.6): policy of the first
    /// slot of the plan ("pin" | "ideal" | "none"); `hot_others`: policy of the other slots. "none" keeps the plan as
    /// it is. (C# defaults: "none", "none".)
    pub fn build_auto_preset(exe: &str, plan: &[Classified], pins: bool, hot_first: &str, hot_others: &str) -> Preset {
        let mut p = Preset {
            exe: exe.to_string(),
            class: AutoPolicy::GAME_CLASS.into(),
            io: "normal".into(),
            mem: "normal".into(),
            eco: "off".into(),
            boost: String::new(),
            ..Default::default()
        };
        for (i, c) in plan.iter().enumerate() {
            // The roles give the order; a set heat field overrides what the slot gets.
            let heat = if i == 0 { hot_first } else { hot_others };
            let hard = heat == "pin";
            p.threads.push(ThreadRule {
                match_: "tid".into(),
                pattern: c.row.tid.to_string(),
                prio: Self::role_prio(c.role).into(),
                ideal: if pins { c.lp.to_string() } else { String::new() },
                // Hard limits only with pins, i.e. only when the confidence passed the floor (the guard of 22.4).
                pin: pins && hard && c.role.is_slot(),
                boost: if matches!(c.role, ThreadRole::Main | ThreadRole::Audio) { "on".into() } else { "off".into() },
                enabled: true,
                ..Default::default()
            });
        }
        p
    }

    /// Share of the planned thread set that changed (ended or new heavy threads).
    pub fn composition_change(planned: &[u32], now: &[u32]) -> f64 {
        if planned.is_empty() {
            return if !now.is_empty() { 1.0 } else { 0.0 };
        }
        let a: IndexSet<u32> = planned.iter().copied().collect();
        let b: IndexSet<u32> = now.iter().copied().collect();
        let gone = a.iter().filter(|x| !b.contains(*x)).count();
        let added = b.iter().filter(|x| !a.contains(*x)).count();
        (gone + added) as f64 / a.len().max(1) as f64
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::base::hooks::static_hooks;
    use crate::base::model::L3Cache;
    use crate::topo::builder::LIVE_HOOK;
    use crate::topo::game_mask::IRQ_HOOK;
    use crate::topo::synth;
    use std::sync::Arc;

    fn row(tid: u32, desc: &str, rate: u64) -> ThreadRow {
        ThreadRow { tid, desc: desc.into(), rate, ..Default::default() }
    }

    /// AutoLogicTests.HardenKeepsAtLeastHalfAndMinLp
    #[test]
    fn harden_keeps_at_least_half_and_min_lp() {
        let _g = static_hooks();
        let x = synth::x7950x3d();
        assert_eq!(x.full_mask(), AutoLogic::harden(&x, 0x7));
        assert_eq!(x.full_mask(), AutoLogic::harden(&x, 0xFF));
        assert_eq!(0xFFFF, AutoLogic::harden(&x, 0xFFFF));
        assert_eq!(0xFFFF, AutoLogic::harden(&x, x.full_mask()));
        let e = synth::x7950x();
        assert_eq!(0xFFFF, AutoLogic::harden(&e, e.full_mask()));
        let s = synth::x7800x3d();
        assert_eq!(s.full_mask(), AutoLogic::harden(&s, s.full_mask()));
        for t in [x, e, s, synth::i13900k(), synth::i12400(), synth::six()] {
            let t = Arc::new(t);
            let t2 = t.clone();
            LIVE_HOOK.set(Some(Arc::new(move || t2.clone())));
            IRQ_HOOK.set(Some(Arc::new(Vec::new)));
            let mp = AutoLogic::build_mask(&t, 0);
            if mp.satisfied {
                assert!(TopoBuilder::pop_count(mp.mask) >= 4);
                assert!(TopoBuilder::pop_count(mp.mask) * 2 >= TopoBuilder::pop_count(t.full_mask()));
            }
            LIVE_HOOK.set(None);
            IRQ_HOOK.set(None);
        }
    }

    /// AutoLogicTests.PickMaskByProcessor
    #[test]
    fn pick_mask_by_processor() {
        assert_eq!("vcache", AutoLogic::pick_mask(&synth::x7950x3d()));
        assert_eq!("pcores", AutoLogic::pick_mask(&synth::i13900k()));
        assert_eq!("auto", AutoLogic::pick_mask(&synth::x7800x3d()));
        assert_eq!("auto", AutoLogic::pick_mask(&synth::x7950x()));
    }

    /// AutoLogicTests.ClassifyRoles
    #[test]
    fn classify_roles() {
        let s = ThreadSnap {
            rows: vec![
                row(10, "MainThrd", 100),
                row(11, "D3D11RenderThread", 50),
                row(12, "AudioMixer", 5),
                row(13, "GlobPool/P0", 40),
                row(14, "idle", 1),
                ThreadRow { tid: 15, sym: "EasyAntiCheat_EOS.dll+0x10".into(), rate: 30, ..Default::default() },
            ],
            ..Default::default()
        };
        assert_eq!(ThreadRole::Main, AutoLogic::classify(&s.rows[0], &s));
        assert_eq!(ThreadRole::Render, AutoLogic::classify(&s.rows[1], &s));
        assert_eq!(ThreadRole::Audio, AutoLogic::classify(&s.rows[2], &s));
        assert_eq!(ThreadRole::Worker, AutoLogic::classify(&s.rows[3], &s));
        assert_eq!(ThreadRole::Aux, AutoLogic::classify(&s.rows[4], &s));
        assert_eq!(ThreadRole::Skip, AutoLogic::classify(&s.rows[5], &s));
    }

    /// AutoLogicTests.PlanIsStickyAndRebuildsOnlyOnBigChanges
    #[test]
    fn plan_is_sticky_and_rebuilds_only_on_big_changes() {
        let t = synth::x7950x3d();
        let s = ThreadSnap {
            rows: (0..10)
                .map(|i| ThreadRow {
                    tid: 100 + i,
                    desc: if i == 0 { "Main".into() } else { format!("Worker{}", i) },
                    sym: "game.exe+0x1".into(),
                    rate: (1000 - i as u64 * 50),
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        };
        let p1 = AutoLogic::plan(&t, &s, 0xFFFF, None);
        assert!(p1.len() <= 6);
        let prev: HashMap<u32, u32> = p1.iter().map(|c| (c.row.tid, c.lp)).collect();
        let p2 = AutoLogic::plan(&t, &s, 0xFFFF, Some(&prev));
        assert!(p2.iter().all(|c| prev[&c.row.tid] == c.lp));
        assert!((AutoLogic::composition_change(&[1, 2, 3, 4, 5], &[1, 2, 3, 4]) - 0.2).abs() < 0.0005);
        assert_eq!(0.0, AutoLogic::composition_change(&[1, 2], &[2, 1]));
    }

    /// SmallCpuMaskTests.SixCoresOrFewerKeepAllCores
    #[test]
    fn six_cores_or_fewer_keep_all_cores() {
        assert!(AutoLogic::keeps_all_cores(Some(&synth::x7800x3d()), "auto"));
        assert!(AutoLogic::keeps_all_cores(Some(&synth::i12400()), "auto"));
        assert!(AutoLogic::keeps_all_cores(Some(&synth::six()), ""));
    }

    /// SmallCpuMaskTests.BigOrHybridProcessorsKeepTheMask
    #[test]
    fn big_or_hybrid_processors_keep_the_mask() {
        assert!(!AutoLogic::keeps_all_cores(Some(&synth::x7950x3d()), "auto"));
        assert!(!AutoLogic::keeps_all_cores(Some(&synth::x7950x()), "auto"));
        assert!(!AutoLogic::keeps_all_cores(Some(&synth::i13900k()), "auto"));
    }

    /// SmallCpuMaskTests.SettingOverridesTheRule
    #[test]
    fn setting_overrides_the_rule() {
        assert!(!AutoLogic::keeps_all_cores(Some(&synth::i12400()), "on"));
        assert!(AutoLogic::keeps_all_cores(Some(&synth::x7800x3d()), "off"));
        assert!(AutoLogic::keeps_all_cores(Some(&synth::x7950x3d()), "off"));
        assert!(AutoLogic::keeps_all_cores(Some(&synth::x7800x3d()), " OFF "));
        assert!(!AutoLogic::keeps_all_cores(None, "auto"));
    }

    /// SmallCpuMaskTests.AllCoresGivesTheFullMaskAndKeepsUnsatisfiedPlans
    #[test]
    fn all_cores_gives_the_full_mask_and_keeps_unsatisfied_plans() {
        let t = synth::i12400();
        let mp = MaskPlan { spec: "auto".into(), mask: 0x0FF, satisfied: true, ..Default::default() };
        let all = AutoLogic::all_cores(Some(&t), mp);
        assert_eq!(t.full_mask(), all.mask);
        assert_eq!(0xFFF, all.mask);
        assert!(all.satisfied);
        let no = MaskPlan { spec: "auto".into(), satisfied: false, ..Default::default() };
        assert_eq!(no.clone(), AutoLogic::all_cores(Some(&t), no));
    }

    /// SmallCpuMaskTests.EightCoresOrFewerKeepEveryCoreEvenWithTwoCachesOrECores
    #[test]
    fn eight_cores_or_fewer_keep_every_core_even_with_two_caches_or_e_cores() {
        assert!(AutoLogic::keeps_all_cores(
            Some(&synth::symmetric(8, true, 4, &[16 << 20, 16 << 20], "AMD Ryzen 7 3700X 8-Core Processor", 0)),
            "auto"
        ));
        let mut sets = Vec::new();
        for c in 0..4u32 {
            sets.push(CpuSet { id: 256 + 2 * c, lp: (2 * c) as u8, core: (2 * c) as u8, eff: 1, ..Default::default() });
            sets.push(CpuSet { id: 257 + 2 * c, lp: (2 * c + 1) as u8, core: (2 * c) as u8, eff: 1, ..Default::default() });
        }
        for e in 0..4u32 {
            sets.push(CpuSet { id: 264 + e, lp: (8 + e) as u8, core: (8 + e) as u8, eff: 0, ..Default::default() });
        }
        let l3 = L3Cache { size: 12 << 20, masks: vec![(0, 0xFFF)] };
        let laptop = TopoBuilder::build(sets, &[l3], "12th Gen Intel(R) Core(TM) i5-12450H", 12);
        assert!(laptop.hybrid());
        assert!(AutoLogic::keeps_all_cores(Some(&laptop), "auto"));
        assert!(!AutoLogic::keeps_all_cores(Some(&laptop), "on"));
        assert!(!AutoLogic::keeps_all_cores(Some(&synth::i13900k()), "auto"));
    }

    fn plan3() -> Vec<Classified> {
        vec![
            Classified { row: ThreadRow { tid: 100, born: 1000, ..Default::default() }, role: ThreadRole::Main, lp: 2, ..Default::default() },
            Classified { row: ThreadRow { tid: 101, born: 1010, ..Default::default() }, role: ThreadRole::Render, lp: 4, ..Default::default() },
            Classified { row: ThreadRow { tid: 103, born: 1030, ..Default::default() }, role: ThreadRole::Worker, lp: 6, ..Default::default() },
        ]
    }

    /// LatencyDefaultsTests.KeyGameThreadsGetPlusOne
    #[test]
    fn key_game_threads_get_plus_one() {
        let plan = vec![
            Classified { row: ThreadRow { tid: 100, born: 1000, ..Default::default() }, role: ThreadRole::Main, lp: 2, ..Default::default() },
            Classified { row: ThreadRow { tid: 101, born: 1001, ..Default::default() }, role: ThreadRole::Render, lp: 4, ..Default::default() },
        ];
        let preset = AutoLogic::build_auto_preset("cs2.exe", &plan, true, "none", "none");
        assert_eq!("above", preset.threads[0].prio);
        assert_eq!("above", preset.threads[1].prio);
    }

    /// RoadmapTests.HeatNoneNoneIsTheReferenceAndPinOverridesTheFirstSlot (the preset part; the engine part is in
    /// engine.rs)
    #[test]
    fn heat_pin_overrides_the_first_slot() {
        let plan = plan3();
        let hot = AutoLogic::build_auto_preset("cs2.exe", &plan, true, "pin", "ideal");
        assert_eq!(vec![true, false, false], hot.threads.iter().map(|t| t.pin).collect::<Vec<_>>());
        assert!(AutoLogic::build_auto_preset("cs2.exe", &plan, false, "pin", "pin").threads.iter().all(|t| !t.pin));
    }
}
