//! The automatic interrupt layout (port of `IrqLayout.cs`), the same rules as DEVICE-TWEAKER's AUTO mode (its
//! AutoAffinityPlanner, re-written, not copied):
//!  - a unit is one physical core and only its first logical processor is used;
//!  - only P-cores of the target CCD take part: with two CCDs the one without 3D V-Cache (when its L3 is at least
//!    32 MB smaller and the other at least 1.5x bigger), else the CCD with the best-ranked core, else the first;
//!  - order of cores: the CPPC preferred-core rank Windows logs at boot (rank, then performance, then position);
//!    without a ranking, by position;
//!  - core 0 is avoided when there are enough other cores;
//!  - order of placement: mouse, game controller, graphics card (two neighbouring cores, preferring no core 0, one
//!    CCD, one CCX, then the highest pair), network card (away from the input CCX when a CCD has several), keyboard
//!    (shares the mouse core through its SMT sibling when no core is left);
//!  - after the graphics card, the network card and the keyboard (after the input only when there is no graphics
//!    card) the next free core other than core 0 stays empty, as long as cores are left over;
//!  - a final check takes back any slot that shares a core with the graphics card, a network card that shares an
//!    input core, and a graphics card without two distinct P-cores.

use super::model::Topo;
use crate::base::l::L;
use crate::lf;
use indexmap::{IndexMap, IndexSet};
use std::collections::HashMap;

/// `AutoLogic.SmallCpuCores` (engine area): no device cores on 6 cores or fewer.
const SMALL_CPU_CORES: usize = 6;

/// What a device is for the automatic interrupt layout (declaration order = placement order).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum IrqRole {
    Mouse,
    Controller,
    Gpu,
    Net,
    Keyboard,
}

/// A device that takes part in the layout, with the role that decides its place.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IrqSlot {
    /// device instance id
    pub key: String,
    pub name: String,
    pub role: IrqRole,
    /// The result: the processors its interrupts go to (0 = no core: the device keeps the base settings).
    pub mask: u64,
    /// Placed, but taken back by the final check (shares a core it must not share).
    pub blocked: bool,
    pub why: String,
}

impl IrqSlot {
    pub fn new(key: &str, name: &str, role: IrqRole) -> Self {
        IrqSlot { key: key.into(), name: name.into(), role, mask: 0, blocked: false, why: String::new() }
    }
}

#[derive(Clone, Debug)]
struct Unit {
    /// core index
    id: i32,
    /// first logical processor
    lp: i32,
    sibling: i32,
    ccx: i32,
    ccd: i32,
    core0: bool,
    e: bool,
    used: bool,
    rank: i32,
    rating: i32,
}

/// `IrqLayout` (static class).
pub struct IrqLayout;

impl IrqLayout {
    /// Dense CPPC ranks (1 = best) per logical processor, or None when the ranking is missing or flat.
    pub fn ranks(t: Option<&Topo>, perf: Option<&HashMap<i32, i32>>) -> Option<HashMap<i32, i32>> {
        let (t, perf) = (t?, perf?);
        let lps: Vec<i32> = t.sets.iter().filter(|s| s.group == 0 && s.lp < 64).map(|s| s.lp as i32).collect();
        if lps.is_empty() || lps.iter().any(|lp| !perf.contains_key(lp)) {
            return None;
        }
        let mut values: Vec<i32> = Vec::new();
        for lp in &lps {
            let v = perf[lp];
            if !values.contains(&v) {
                values.push(v);
            }
        }
        values.sort_by(|a, b| b.cmp(a));
        if values.len() < 2 {
            return None;
        }
        let mut d = HashMap::new();
        for lp in lps {
            let r = values.iter().position(|v| *v == perf[&lp]).unwrap_or(0) as i32 + 1;
            d.insert(lp, r);
        }
        Some(d)
    }

    /// `ccx_pairs`: AMD family 17h (Zen / Zen 2): two L3 groups (CCX) form one CCD.
    pub fn plan(t: Option<&Topo>, devices: Vec<IrqSlot>, perf: Option<&HashMap<i32, i32>>, ccx_pairs: bool) -> Vec<IrqSlot> {
        let mut slots = devices;
        slots.sort_by_key(|s| s.role); // stable: equal roles keep their order
        for s in slots.iter_mut() {
            s.mask = 0;
            s.blocked = false;
            s.why.clear();
        }
        let t = match t {
            Some(t) if !t.sets.is_empty() && !slots.is_empty() => t,
            _ => return slots,
        };

        // ---- units -------------------------------------------------------------------------------------------
        let ranks = Self::ranks(Some(t), perf);
        let top_eff = t.sets.iter().map(|s| s.eff).max().unwrap_or(0);
        let hybrid = t.sets.iter().map(|s| s.eff).collect::<IndexSet<u8>>().len() > 1;
        let mut llc_order: Vec<u8> = t.sets.iter().map(|s| s.llc).collect::<IndexSet<u8>>().into_iter().collect();
        llc_order.sort();
        let mut groups: IndexMap<u8, Vec<crate::base::model::CpuSet>> = IndexMap::new();
        for s in t.sets.iter().filter(|s| s.group == 0 && s.lp < 64) {
            groups.entry(s.core).or_default().push(*s);
        }
        let mut all: Vec<Unit> = groups
            .iter()
            .map(|(core, g)| {
                let mut lps: Vec<i32> = g.iter().map(|s| s.lp as i32).collect();
                lps.sort();
                let ccx = llc_order.iter().position(|l| *l == g[0].llc).map(|x| x as i32).unwrap_or(-1);
                Unit {
                    id: *core as i32,
                    lp: lps[0],
                    sibling: if lps.len() > 1 { lps[1] } else { -1 },
                    ccx,
                    ccd: if ccx_pairs { ccx / 2 } else { ccx },
                    core0: lps.contains(&0),
                    e: hybrid && g[0].eff != top_eff,
                    used: false,
                    rank: match &ranks {
                        None => i32::MAX,
                        Some(r) => lps.iter().map(|lp| r[lp]).min().unwrap_or(i32::MAX),
                    },
                    rating: match perf {
                        None => 0,
                        Some(p) => lps.iter().map(|lp| p.get(lp).copied().unwrap_or(0)).max().unwrap_or(0),
                    },
                }
            })
            .collect();
        let p: Vec<usize> = (0..all.len()).filter(|i| !all[*i].e).collect();
        if p.is_empty() {
            for s in slots.iter_mut() {
                s.why = L::t("Left alone: no performance cores were found").to_string();
            }
            return slots;
        }
        // The target CCD.
        let mut ccds: Vec<i32> = p.iter().map(|i| all[*i].ccd).collect::<IndexSet<i32>>().into_iter().collect();
        ccds.sort();
        // 1.16.0-test1.7: a processor with 6 cores or fewer (one CCD, no E-cores) gives no device a core of its own;
        // 8 cores and more keep the layout as before (owner's decision). On a tester's i5-10400F (6 cores) the cores
        // taken from the game cost 6% of the 1% low. The devices keep high interrupt priority; Windows places their
        // interrupts.
        if !hybrid && ccds.len() == 1 && p.len() <= SMALL_CPU_CORES {
            for s in slots.iter_mut() {
                s.why = lf!("No core of its own: a processor with {0} cores gives every core to the game; Windows places the interrupts", p.len() as i32);
            }
            return slots;
        }
        let mut target: i32 = -1;
        if ccds.len() >= 2 {
            let l3 = |ccd: i32| -> u64 {
                let llcs: IndexSet<u8> = all.iter().filter(|u| u.ccd == ccd).map(|u| llc_order[u.ccx as usize]).collect();
                llcs.iter().map(|l| t.l3_by_llc.get(l).copied().unwrap_or(0) as u128).sum::<u128>() as u64
            };
            if ccds.len() == 2 {
                let (a, b) = (l3(ccds[0]), l3(ccds[1]));
                let (big, small) = (a.max(b), a.min(b));
                if small > 0 && big - small >= 32u64 << 20 && big.wrapping_mul(2) >= small.wrapping_mul(3) {
                    target = if a < b { ccds[0] } else { ccds[1] };
                }
            }
            if target < 0 {
                target = if ranks.is_some() {
                    let mut by: Vec<usize> = p.clone();
                    by.sort_by_key(|i| (all[*i].rank, all[*i].ccd));
                    all[by[0]].ccd
                } else {
                    ccds[0]
                };
            }
        }
        let mut pref: Vec<usize> = if target >= 0 { p.iter().copied().filter(|i| all[*i].ccd == target).collect() } else { p.clone() };
        if pref.is_empty() {
            pref = p.clone();
        }
        if ranks.is_some() {
            pref.sort_by(|x, y| {
                let (a, b) = (&all[*x], &all[*y]);
                a.rank.cmp(&b.rank).then(b.rating.cmp(&a.rating)).then(a.lp.cmp(&b.lp))
            });
        } else {
            pref.sort_by_key(|i| all[*i].lp);
        }

        // ---- budget ------------------------------------------------------------------------------------------
        let required: usize = slots.iter().map(|s| if s.role == IrqRole::Gpu { 2 } else { 1 }).sum();
        let non_core0 = pref.iter().filter(|i| !all[**i].core0).count();
        let avoid_core0 = non_core0 >= required.min(pref.len());
        let eligible = if avoid_core0 { non_core0 } else { pref.len() };
        let mut spare: i64 = eligible as i64 - eligible.min(required) as i64;
        let has_gpu = slots.iter().any(|s| s.role == IrqRole::Gpu);
        let ccx_visible = {
            let mut by_ccd: IndexMap<i32, IndexSet<i32>> = IndexMap::new();
            for i in &pref {
                by_ccd.entry(all[*i].ccd).or_default().insert(all[*i].ccx);
            }
            by_ccd.values().any(|s| s.len() > 1)
        };

        fn next(all: &[Unit], order: &[usize], avoid_core0: bool) -> Option<usize> {
            let mut free = order.iter().copied().filter(|i| !all[*i].used);
            if avoid_core0 {
                let first_free: Vec<usize> = free.collect();
                first_free.iter().copied().find(|i| !all[*i].core0).or_else(|| first_free.first().copied())
            } else {
                free.next()
            }
        }
        fn spacer(all: &mut [Unit], pref: &[usize], spare: &mut i64) {
            if *spare <= 0 {
                return;
            }
            let Some(u) = pref.iter().copied().find(|i| !all[*i].used && !all[*i].core0) else { return };
            all[u].used = true;
            *spare -= 1;
        }
        fn take(all: &mut [Unit], s: &mut IrqSlot, units: &mut Vec<usize>, u: usize) {
            all[u].used = true;
            s.mask |= 1u64 << all[u].lp;
            units.push(u);
        }

        // ---- placement ---------------------------------------------------------------------------------------
        let mut input: Option<usize> = None;
        let mut units_of: Vec<Option<Vec<usize>>> = vec![None; slots.len()];
        for (si, s) in slots.iter_mut().enumerate() {
            match s.role {
                IrqRole::Mouse | IrqRole::Controller => {
                    if let Some(one) = next(&all, &pref, avoid_core0) {
                        input.get_or_insert(one);
                        s.why = if s.role == IrqRole::Mouse {
                            L::t("Mouse controller: a core of its own, the best one").to_string()
                        } else {
                            L::t("Game controller: a core of its own").to_string()
                        };
                        take(&mut all, s, units_of[si].get_or_insert_with(Vec::new), one);
                        if !has_gpu {
                            spacer(&mut all, &pref, &mut spare);
                        }
                    }
                }
                IrqRole::Gpu => {
                    let mut line: Vec<usize> = pref.iter().copied().filter(|i| !all[*i].e).collect();
                    line.sort_by_key(|i| all[*i].id);
                    let mut best: Option<(usize, usize)> = None;
                    let mut best_score = (-1i32, -1i32, -1i32, -1i32, -1i32);
                    for k in 0..line.len().saturating_sub(1) {
                        let (a, b) = (&all[line[k]], &all[line[k + 1]]);
                        if a.used || b.used {
                            continue;
                        }
                        let score = (
                            if avoid_core0 && (a.core0 || b.core0) { 0 } else { 1 },
                            if a.ccd == b.ccd { 1 } else { 0 },
                            if !ccx_visible || a.ccx == b.ccx { 1 } else { 0 },
                            a.lp.max(b.lp),
                            a.lp.min(b.lp),
                        );
                        if score > best_score {
                            best_score = score;
                            best = Some((line[k], line[k + 1]));
                        }
                    }
                    match best {
                        None => s.why = L::t("No two neighbouring free cores: the graphics card keeps the base settings").to_string(),
                        Some((a, b)) => {
                            s.why = L::t("Graphics card: two neighbouring cores of its own").to_string();
                            let units = units_of[si].get_or_insert_with(Vec::new);
                            take(&mut all, s, units, a);
                            take(&mut all, s, units, b);
                            spacer(&mut all, &pref, &mut spare);
                        }
                    }
                }
                IrqRole::Net => {
                    let order: Vec<usize> = match input {
                        Some(inp) if ccx_visible => {
                            let ic = all[inp].ccx;
                            pref.iter().copied().filter(|i| all[*i].ccx != ic).chain(pref.iter().copied().filter(|i| all[*i].ccx == ic)).collect()
                        }
                        _ => pref.clone(),
                    };
                    if let Some(one) = next(&all, &order, avoid_core0) {
                        s.why = L::t("Network card: a core of its own; its receive queue (RSS) goes to the same core").to_string();
                        take(&mut all, s, units_of[si].get_or_insert_with(Vec::new), one);
                        spacer(&mut all, &pref, &mut spare);
                    }
                }
                IrqRole::Keyboard => {
                    if let Some(one) = next(&all, &pref, avoid_core0) {
                        s.why = L::t("Keyboard controller: a core of its own").to_string();
                        take(&mut all, s, units_of[si].get_or_insert_with(Vec::new), one);
                        spacer(&mut all, &pref, &mut spare);
                    } else if let Some(inp) = input {
                        let u = &all[inp];
                        s.mask = 1u64 << (if u.sibling >= 0 { u.sibling } else { u.lp });
                        units_of[si] = Some(vec![inp]);
                        s.why = L::t("Keyboard controller: no free core left, shares the mouse core through its second thread").to_string();
                    }
                }
            }
            if s.mask == 0 && s.why.is_empty() {
                s.why = L::t("No free core: the device keeps the base settings").to_string();
            }
        }

        // ---- final check -------------------------------------------------------------------------------------
        let gpu_units: IndexSet<usize> = slots
            .iter()
            .enumerate()
            .filter(|(i, s)| s.role == IrqRole::Gpu && units_of[*i].is_some())
            .flat_map(|(i, _)| units_of[i].clone().unwrap_or_default())
            .collect();
        let input_units: IndexSet<usize> = slots
            .iter()
            .enumerate()
            .filter(|(i, s)| matches!(s.role, IrqRole::Mouse | IrqRole::Controller | IrqRole::Keyboard) && units_of[*i].is_some())
            .flat_map(|(i, _)| units_of[i].clone().unwrap_or_default())
            .collect();
        for (i, s) in slots.iter_mut().enumerate() {
            let Some(us) = &units_of[i] else { continue };
            let distinct: IndexSet<usize> = us.iter().copied().collect();
            let why: Option<&str> = if s.role == IrqRole::Gpu && (distinct.len() < 2 || us.iter().any(|u| all[*u].e)) {
                Some(L::t("needs two distinct performance cores"))
            } else if s.role != IrqRole::Gpu && us.iter().any(|u| gpu_units.contains(u)) {
                Some(L::t("would share a core with the graphics card"))
            } else if s.role == IrqRole::Net && us.iter().any(|u| input_units.contains(u)) {
                Some(L::t("would share a core with an input device"))
            } else {
                None
            };
            let Some(why) = why else { continue };
            s.mask = 0;
            s.blocked = true;
            s.why = lf!("Taken back: {0}; the device keeps the base settings", why);
        }
        slots
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::topo::synth::{self, MB};

    fn s(r: IrqRole) -> IrqSlot {
        let n = format!("{:?}", r);
        IrqSlot::new(&n, &n, r)
    }

    fn plan(t: &Topo, perf: Option<&HashMap<i32, i32>>, slots: Vec<IrqSlot>) -> HashMap<String, u64> {
        IrqLayout::plan(Some(t), slots, perf, false).into_iter().map(|s| (s.name, s.mask)).collect()
    }

    /// IrqLayoutTests.Ryzen7800X3D_mouse_gpu_nic_get_the_layout_of_the_original
    #[test]
    fn ryzen7800x3d_mouse_gpu_nic_get_the_layout_of_the_original() {
        let m = plan(&synth::x7800x3d(), None, vec![s(IrqRole::Gpu), s(IrqRole::Net), s(IrqRole::Mouse)]);
        assert_eq!(0x4, m["Mouse"]); // LP2
        assert_eq!(0x5000, m["Gpu"]); // LP12 + LP14
        assert_eq!(0x40, m["Net"]); // LP6 (LP4 stays empty after the GPU)
    }

    /// IrqLayoutTests.A_separate_keyboard_controller_takes_the_next_core_after_the_nic_spacer
    #[test]
    fn a_separate_keyboard_controller_takes_the_next_core_after_the_nic_spacer() {
        let m = plan(&synth::x7800x3d(), None, vec![s(IrqRole::Mouse), s(IrqRole::Gpu), s(IrqRole::Net), s(IrqRole::Keyboard)]);
        assert_eq!(0x400, m["Keyboard"]); // LP10
        assert_eq!(0x40, m["Net"]);
    }

    /// IrqLayoutTests.Six_cores_or_fewer_give_no_device_a_core_of_its_own
    #[test]
    fn six_cores_or_fewer_give_no_device_a_core_of_its_own() {
        let _g = crate::base::hooks::static_hooks(); // the texts are compared in English
        // 1.16.0-test1.7: 6 cores or fewer, one CCD, no E-cores: every core stays with the game (8+ keep the layout).
        for t in [synth::i12400(), synth::six(), synth::symmetric(4, true, 4, &[8 * MB], "4 cores", 0)] {
            let slots = IrqLayout::plan(Some(&t), vec![s(IrqRole::Mouse), s(IrqRole::Gpu), s(IrqRole::Net), s(IrqRole::Keyboard)], None, false);
            assert!(slots.iter().all(|x| x.mask == 0));
            assert!(slots.iter().all(|x| x.why.contains("No core of its own")));
        }
    }

    /// IrqLayoutTests.Ryzen7950X3D_devices_go_to_the_ccd_without_vcache
    #[test]
    fn ryzen7950x3d_devices_go_to_the_ccd_without_vcache() {
        let m = plan(&synth::x7950x3d(), None, vec![s(IrqRole::Mouse), s(IrqRole::Gpu), s(IrqRole::Net)]);
        assert_eq!(1u64 << 16, m["Mouse"]);
        assert_eq!((1u64 << 28) | (1u64 << 30), m["Gpu"]);
        assert_eq!(1u64 << 20, m["Net"]);
    }

    /// IrqLayoutTests.Intel_hybrid_uses_only_p_cores
    #[test]
    fn intel_hybrid_uses_only_p_cores() {
        let m = plan(&synth::i13900k(), None, vec![s(IrqRole::Mouse), s(IrqRole::Gpu), s(IrqRole::Net)]);
        assert_eq!(0x4, m["Mouse"]);
        assert_eq!(0x5000, m["Gpu"]);
        assert_eq!(0x40, m["Net"]);
        assert!(m.values().all(|v| *v < (1u64 << 16)));
    }

    /// IrqLayoutTests.Cppc_ranking_puts_the_mouse_on_the_best_core
    #[test]
    fn cppc_ranking_puts_the_mouse_on_the_best_core() {
        let perf: HashMap<i32, i32> = (0..16).map(|lp| (lp, if lp == 6 || lp == 7 { 100 } else { 90 })).collect();
        let m = plan(&synth::x7800x3d(), Some(&perf), vec![s(IrqRole::Mouse), s(IrqRole::Gpu), s(IrqRole::Net)]);
        assert_eq!(0x40, m["Mouse"]); // LP6, the best-ranked core
        assert_eq!(0x5000, m["Gpu"]);
        assert_eq!(0x10, m["Net"]); // LP4 (LP2 is the spacer after the GPU)
    }

    /// IrqLayoutTests.A_flat_ranking_is_ignored
    #[test]
    fn a_flat_ranking_is_ignored() {
        let perf: HashMap<i32, i32> = (0..16).map(|lp| (lp, 100)).collect();
        assert!(IrqLayout::ranks(Some(&synth::x7800x3d()), Some(&perf)).is_none());
    }
}
