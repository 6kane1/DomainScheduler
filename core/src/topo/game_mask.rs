//! The game's core mask with the IRQ exclusion (section 6, port of `GameMask.cs`): processors that a device's
//! interrupts are pinned to can be left out of the mask, but never more than half of it.

use super::builder::TopoBuilder;
use super::masks::Masks;
use super::model::Topo;
use crate::base::hooks::{lock, Hook};
use crate::base::model::GameMaskInfo;
use crate::base::time::{DateTime, TimeSpan};
use crate::lf;
use indexmap::{IndexMap, IndexSet};
use std::sync::{Mutex, RwLock};

pub type PinList = Vec<(String, u64)>;

/// `GameMask.IrqHook`: test hook for the pinned interrupts.
pub static IRQ_HOOK: Hook<dyn Fn() -> PinList + Send + Sync> = Hook::new();
/// `GameMask.IrqSource`: the real source (Devices.PinnedIrqs), set by the Windows layer.
pub static IRQ_SOURCE: Hook<dyn Fn() -> PinList + Send + Sync> = Hook::new();
/// `GameMask.NoisyHook`, section 22.9: processors that are busy with DPC/ISR right now (unset = the live source is off).
pub static NOISY_HOOK: Hook<dyn Fn() -> u64 + Send + Sync> = Hook::new();
/// `GameMask.BusyHook`: live interrupt cores for Auto (whole physical cores, measured while the game runs). Returns
/// None while the live source is off or not ready yet: Auto then keeps the registry-pin rule.
pub static BUSY_HOOK: Hook<dyn Fn(&Topo) -> Option<u64> + Send + Sync> = Hook::new();

static MODE: RwLock<Option<String>> = RwLock::new(None);

struct IrqCache {
    irq: PinList,
    at: DateTime,
}

static GATE: Mutex<IrqCache> = Mutex::new(IrqCache { irq: Vec::new(), at: DateTime::MIN });

/// Manual edits of a mask, bound to the hardware signature they were made on (`GameMask.Edits`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Edits {
    pub add: u64,
    pub drop: u64,
    pub sig: String,
}

/// `GameMask` (static class).
pub struct GameMask;

fn bit(m: u64, lp: u8) -> bool {
    // C# masks the shift count to 6 bits.
    (m.wrapping_shr(lp as u32) & 1) != 0
}

impl GameMask {
    pub const OFF: &'static str = "Off";
    pub const THREAD_MODE: &'static str = "Thread";
    pub const CORE_MODE: &'static str = "Core";
    pub const LIVE_MODE: &'static str = "Live";
    pub const WIDE_NOTE: &'static str = "wide-irq: partial";
    pub const SMALL_NOTE: &'static str = "irq: several devices on a small processor, only their processors cut";

    /// `GameMask.Mode`: the IRQ refinement mode of preset masks (the Masks page; Auto has its own policy).
    pub fn mode() -> String {
        MODE.read().unwrap_or_else(|e| e.into_inner()).clone().unwrap_or_else(|| Self::CORE_MODE.to_string())
    }

    pub fn set_mode(m: &str) {
        *MODE.write().unwrap_or_else(|e| e.into_inner()) = Some(m.to_string());
    }

    /// Pinned interrupts (device, mask), cached for 30 s.
    pub fn pinned() -> PinList {
        if let Some(h) = IRQ_HOOK.get() {
            return h();
        }
        let mut g = lock(&GATE);
        if DateTime::utc_now() - g.at < TimeSpan::from_secs_i(30) {
            return g.irq.clone();
        }
        g.at = DateTime::utc_now();
        // C# catches an exception of the source (empty list); a source here cannot throw.
        g.irq = IRQ_SOURCE.get().map(|f| f()).unwrap_or_default();
        g.irq.clone()
    }

    pub fn invalidate_irq() {
        lock(&GATE).at = DateTime::MIN;
    }

    pub fn whole_cores(t: &Topo, m: u64) -> u64 {
        let mut groups: IndexMap<u8, u64> = IndexMap::new();
        let mut hit: IndexSet<u8> = IndexSet::new();
        for s in t.sets.iter().filter(|s| s.group == 0) {
            let g = groups.entry(s.core).or_insert(0);
            if s.lp < 64 {
                *g |= 1u64 << s.lp;
                if bit(m, s.lp) {
                    hit.insert(s.core);
                }
            }
        }
        groups.iter().filter(|(c, _)| hit.contains(*c)).fold(0, |a, (_, m)| a | m)
    }

    pub fn dedicated(t: &Topo, pin: u64) -> bool {
        if pin == 0 {
            return false;
        }
        let cores: IndexSet<u8> = t.sets.iter().filter(|s| s.group == 0 && s.lp < 64 && bit(pin, s.lp)).map(|s| s.core).collect();
        cores.len() == 1
    }

    /// `Adjust(t, base, add, drop, irq, mode, out irqDrop, out irqIgnored)` (no wide pins, no notes).
    pub fn adjust(t: &Topo, base_m: u64, add: u64, drop: u64, irq: u64, mode: &str, irq_drop: &mut u64, irq_ignored: &mut bool) -> u64 {
        Self::adjust_wide(t, base_m, add, drop, irq, 0, mode, irq_drop, irq_ignored, None)
    }

    /// Section 22.8: wide = pins that cover more than one core. They never cut whole cores: only the pinned processors
    /// themselves leave the mask (the "Thread" behaviour) and the note "wide-irq: partial" is added. Dedicated pins keep
    /// the mode's behaviour. The "never more than half of the mask" guard covers all cuts together.
    #[allow(clippy::too_many_arguments)]
    pub fn adjust_wide(
        t: &Topo,
        base_m: u64,
        add: u64,
        drop: u64,
        irq: u64,
        wide: u64,
        mode: &str,
        irq_drop: &mut u64,
        irq_ignored: &mut bool,
        mut notes: Option<&mut Vec<String>>,
    ) -> u64 {
        *irq_drop = 0;
        *irq_ignored = false;
        let full = t.full_mask();
        let mut m = (base_m | add) & !drop & full;
        if m == 0 {
            m = base_m & full;
        }
        if m == 0 {
            m = full;
        }
        if mode == Self::OFF || (irq | wide) == 0 {
            return m;
        }
        let wide_cut = wide & m;
        let mut cut = ((if mode == Self::CORE_MODE { Self::whole_cores(t, irq) } else { irq }) & m) | wide_cut;
        // 8 cores or fewer: cutting whole cores for several devices costs the game too much (3 of 8 cores was seen on
        // a 7800X3D). More than one core's worth -> only the pinned processors themselves leave; their SMT siblings stay.
        if mode == Self::CORE_MODE {
            let all_cores: IndexSet<u8> = t.sets.iter().map(|s| s.core).collect();
            let cut_cores: IndexSet<u8> = t.sets.iter().filter(|s| s.lp < 64 && (cut & (1u64 << s.lp)) != 0).map(|s| s.core).collect();
            if all_cores.len() <= 8 && cut_cores.len() > 1 {
                cut = (irq & m) | wide_cut;
                if let Some(n) = notes.as_deref_mut() {
                    if !n.iter().any(|x| x == Self::SMALL_NOTE) {
                        n.push(Self::SMALL_NOTE.to_string());
                    }
                }
            }
        }
        if wide_cut != 0 {
            if let Some(n) = notes.as_deref_mut() {
                if !n.iter().any(|x| x == Self::WIDE_NOTE) {
                    n.push(Self::WIDE_NOTE.to_string());
                }
            }
        }
        if cut == 0 {
            return m;
        }
        let kept = m & !cut;
        if kept == 0 || TopoBuilder::pop_count(kept) * 2 < TopoBuilder::pop_count(m) {
            *irq_ignored = true;
            return m;
        }
        *irq_drop = cut;
        kept
    }

    /// The base mask without the measured busy interrupt cores (both SMT threads), never more than half of it.
    pub fn for_live(t: &Topo, base_mask: u64, busy: u64) -> GameMaskInfo {
        let mut info = GameMaskInfo { mode: Self::LIVE_MODE.into(), base: base_mask, manual: base_mask, ..Default::default() };
        for lp in 0..64u32 {
            if (busy >> lp) & 1 != 0 {
                info.irq_devices.push((lf!("live DPC/ISR on processor {0}", lp), 1u64 << lp));
            }
        }
        info.irq = busy & t.full_mask();
        let (mut d, mut ig) = (0u64, false);
        info.final_ = Self::adjust_wide(t, base_mask, 0, 0, info.irq, 0, Self::THREAD_MODE, &mut d, &mut ig, Some(&mut info.notes));
        info.irq_drop = d;
        info.irq_ignored = ig;
        info
    }

    fn wide_mask(t: &Topo, info: &GameMaskInfo) -> u64 {
        // A pin that covers every processor is no pin at all: there is nothing to step around.
        let full = t.full_mask();
        info.irq_wide.iter().filter(|p| (p.1 & full) != full).fold(0u64, |a, p| a | p.1) & full
    }

    /// Every pin; the ones spread over several cores also in irq_wide.
    pub fn info(t: &Topo) -> GameMaskInfo {
        let mut info = GameMaskInfo::default();
        let noisy = NOISY_HOOK.get().map(|f| f()).unwrap_or(0) & t.full_mask();
        if noisy != 0 {
            for lp in 0..64u32 {
                if (noisy >> lp) & 1 != 0 {
                    info.irq_devices.push((lf!("live DPC/ISR on processor {0}", lp), 1u64 << lp));
                    info.irq |= 1u64 << lp;
                }
            }
        }
        for p in Self::pinned() {
            if Self::dedicated(t, p.1) {
                info.irq |= p.1;
                info.irq_devices.push(p);
            } else {
                info.irq_wide.push(p);
            }
        }
        info
    }

    /// The base mask with the IRQ exclusion of dedicated pins only.
    pub fn for_rule(t: &Topo, base_mask: u64, mode: &str) -> GameMaskInfo {
        let mut info = Self::info(t);
        info.mode = mode.to_string();
        info.base = base_mask;
        info.manual = base_mask;
        let wide = Self::wide_mask(t, &info);
        let (mut d, mut ig) = (0u64, false);
        let irq = info.irq;
        info.final_ = Self::adjust_wide(t, base_mask, 0, 0, irq, wide, mode, &mut d, &mut ig, Some(&mut info.notes));
        info.irq_drop = d;
        info.irq_ignored = ig;
        info
    }

    pub fn cpu_sig(t: &Topo) -> String {
        format!("{}|{}|{}|{}", t.logical_count, t.physical_count, t.llc_groups.len(), if t.hybrid() { 1 } else { 0 })
    }

    /// The full mask of a rule: base spec, manual edits (dropped on another processor), IRQ exclusion.
    pub fn build(t: &Topo, base_rule: &str, edits: Option<&Edits>, mode: &str) -> GameMaskInfo {
        let r = Masks::resolve(t, base_rule);
        let mut info = Self::info(t);
        info.base_rule = base_rule.to_string();
        info.mode = mode.to_string();
        info.base = r.mask;
        let mut add = edits.map(|e| e.add).unwrap_or(0);
        let mut drop = edits.map(|e| e.drop).unwrap_or(0);
        if (add != 0 || drop != 0) && edits.map(|e| e.sig.as_str()).unwrap_or("") != Self::cpu_sig(t) {
            add = 0;
            drop = 0;
            info.edits_stale = true;
        }
        info.add = add;
        info.drop = drop;
        let (mut d0, mut i0) = (0u64, false);
        info.manual = Self::adjust(t, info.base, add, drop, 0, Self::OFF, &mut d0, &mut i0);
        let wide = Self::wide_mask(t, &info);
        let (mut d, mut ig) = (0u64, false);
        let (base, irq) = (info.base, info.irq);
        info.final_ = Self::adjust_wide(t, base, add, drop, irq, wide, mode, &mut d, &mut ig, Some(&mut info.notes));
        info.irq_drop = d;
        info.irq_ignored = ig;
        info
    }

    fn hex(m: u64) -> String {
        format!("0x{:x}", m)
    }

    /// What is stored for the rule: the hex when it differs from the base, otherwise the spec.
    pub fn rule(info: &GameMaskInfo) -> String {
        if info.final_ != info.base { Self::hex(info.final_) } else { info.base_rule.clone() }
    }

    pub fn manual_rule(info: &GameMaskInfo) -> String {
        if info.manual != info.base { Self::hex(info.manual) } else { info.base_rule.clone() }
    }

    /// Where background programs go next to this game mask.
    pub fn background(t: &Topo, info: &GameMaskInfo) -> u64 {
        Masks::bg_mask(t, Masks::resolve(t, &Self::manual_rule(info)).mask)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::base::hooks::static_hooks;
    use crate::topo::synth;
    use std::sync::Arc;

    fn adj(t: &Topo, b: u64, a: u64, d: u64, irq: u64, mode: &str) -> (u64, u64, bool) {
        let (mut drop, mut ign) = (0, false);
        let m = GameMask::adjust(t, b, a, d, irq, mode, &mut drop, &mut ign);
        (m, drop, ign)
    }

    /// GameMaskTests.AdjustAndGuard
    #[test]
    fn adjust_and_guard() {
        let x = synth::x7950x3d();
        let (m, drop, ign) = adj(&x, 0xFFFF, 0, 0, 0x10, GameMask::CORE_MODE);
        assert_eq!(0xFFCF, m);
        assert_eq!(0x30, drop);
        assert!(!ign);
        assert_eq!(0xFFEF, adj(&x, 0xFFFF, 0, 0, 0x10, GameMask::THREAD_MODE).0);
        // A cut of more than half the mask is ignored and the mask stays as it is.
        let (m, drop, ign) = adj(&x, 0xFF, 0, 0, 0x3F, GameMask::CORE_MODE);
        assert_eq!(0xFF, m);
        assert_eq!(0, drop);
        assert!(ign);
        assert_eq!(0x1FE, adj(&x, 0xFF, 0x100, 0x1, 0, GameMask::OFF).0);
    }

    /// GameMaskTests.DedicatedPinsOnly
    #[test]
    fn dedicated_pins_only() {
        let _g = static_hooks();
        let x = synth::x7950x3d();
        assert!(GameMask::dedicated(&x, 0x30));
        assert!(!GameMask::dedicated(&x, 0x50));
        IRQ_HOOK.set(Some(Arc::new(|| vec![("GPU".to_string(), 0x30), ("NIC".to_string(), 0xF0)])));
        let info = GameMask::for_rule(&x, 0xFFFF, GameMask::CORE_MODE);
        IRQ_HOOK.set(None);
        assert_eq!(0x30, info.irq);
        assert_eq!(1, info.irq_wide.len());
        assert_eq!(0xFF0F, info.final_); // 22.8: the wide pin takes out only its own processors
        assert!(info.notes.iter().any(|n| n == GameMask::WIDE_NOTE));
        assert!(info.adjusted());
    }

    /// GameMaskTests.EditsFromAnotherProcessorAreStale
    #[test]
    fn edits_from_another_processor_are_stale() {
        let _g = static_hooks();
        let x = synth::x7950x3d();
        IRQ_HOOK.set(Some(Arc::new(Vec::new)));
        let stale = GameMask::build(&x, "vcache", Some(&Edits { add: 0x10000, drop: 0, sig: "8|8|1|0".into() }), GameMask::OFF);
        assert!(stale.edits_stale);
        assert_eq!(0xFFFF, stale.final_);
        assert_eq!("vcache", GameMask::rule(&stale));
        let ok = GameMask::build(&x, "vcache", Some(&Edits { add: 0x10000, drop: 0, sig: GameMask::cpu_sig(&x) }), GameMask::OFF);
        assert_eq!(0x1FFFF, ok.final_);
        assert_eq!("0x1ffff", GameMask::rule(&ok));
        assert_eq!(0xFFFC_0000, GameMask::background(&x, &ok)); // core 8 (LP 16,17) is partly the game's: whole free cores only
        IRQ_HOOK.set(None);
    }

    /// RoadmapTests.WideIrqNeverCutsWholeCoresAndTheHalfGuardCountsEverything
    #[test]
    fn wide_irq_never_cuts_whole_cores_and_the_half_guard_counts_everything() {
        let x = synth::x7950x3d();
        let mut notes = Vec::new();
        let (mut drop, mut ignored) = (0, false);
        // Dedicated pin on LP 4 (core mode: LPs 4-5), wide pin on LPs 8 and 10 (two cores): only those two.
        let m = GameMask::adjust_wide(&x, 0xFFFF, 0, 0, 0x10, 0x500, GameMask::CORE_MODE, &mut drop, &mut ignored, Some(&mut notes));
        assert_eq!(0xFFFF & !0x30 & !0x500, m);
        assert_eq!(0x530, drop);
        assert!(!ignored);
        assert!(notes.iter().any(|n| n == "wide-irq: partial"));
        // Without a wide pin: no note, the old behaviour.
        notes.clear();
        assert_eq!(0xFFCF, GameMask::adjust_wide(&x, 0xFFFF, 0, 0, 0x10, 0, GameMask::CORE_MODE, &mut drop, &mut ignored, Some(&mut notes)));
        assert!(notes.is_empty());
        assert_eq!(0xFFCF, adj(&x, 0xFFFF, 0, 0, 0x10, GameMask::CORE_MODE).0);
        // Together they would take more than half: everything is ignored, the mask is unchanged.
        assert_eq!(0xFFFF, GameMask::adjust_wide(&x, 0xFFFF, 0, 0, 0x10, 0xFF00, GameMask::CORE_MODE, &mut drop, &mut ignored, Some(&mut notes)));
        assert!(ignored);
        assert_eq!(0, drop);
    }

    /// RoadmapTests.LiveMaskStepsAroundWholeBusyCoresButNeverMoreThanHalf
    #[test]
    fn live_mask_steps_around_whole_busy_cores_but_never_more_than_half() {
        let x = synth::x7800x3d();
        let live = GameMask::for_live(&x, 0xFFFF, 0xF000);
        assert_eq!(0x0FFF, live.final_);
        assert_eq!(0xF000, live.irq_drop);
        let too_much = GameMask::for_live(&x, 0xFFFF, 0xFFF0);
        assert!(too_much.irq_ignored);
        assert_eq!(0xFFFF, too_much.final_);
    }

    /// RoadmapTests.LiveDpcIsrChangesTheMaskOnlyWhenItIsOn (the GameMask half: the noisy processors come from a hook
    /// here; EtwIrq.Noisy, which feeds it in the product, is the etw area's)
    #[test]
    fn live_dpc_isr_changes_the_mask_only_when_it_is_on() {
        let _g = static_hooks();
        let x = synth::x7950x3d();
        IRQ_HOOK.set(Some(Arc::new(|| vec![("GPU".to_string(), 0x30)])));
        NOISY_HOOK.set(None);
        let registry_only = GameMask::for_rule(&x, 0xFFFF, GameMask::CORE_MODE);
        let off = GameMask::for_rule(&x, 0xFFFF, GameMask::CORE_MODE);
        assert_eq!(registry_only.final_, off.final_);
        assert_eq!(registry_only.irq, off.irq);
        NOISY_HOOK.set(Some(Arc::new(|| 0x100))); // processor 8 is noisy
        let on = GameMask::for_rule(&x, 0xFFFF, GameMask::CORE_MODE);
        assert_eq!(registry_only.final_ & !0x300, on.final_); // the whole core of processor 8 is stepped around
        NOISY_HOOK.set(Some(Arc::new(|| 0))); // the noise is over
        assert_eq!(registry_only.final_, GameMask::for_rule(&x, 0xFFFF, GameMask::CORE_MODE).final_);
        NOISY_HOOK.set(None);
        IRQ_HOOK.set(None);
    }
}
