//! Game / non-game split of the processor (section 4, port of `CoreSplit.cs`).

use super::builder::TopoBuilder;
use super::model::{SplitPlan, Topo};
use crate::base::ci::{self, CiMap, CiMapExt};
use crate::base::fmt::Arg;
use crate::base::hooks::Hook;
use crate::base::l::L;
use crate::lf;
use indexmap::IndexMap;
use std::sync::OnceLock;

/// `Tables.X3d` (dual-CCD X3D models: data, not code). `Tables.cs` is the engine area's: it installs this hook with
/// the merged table (embedded tables.json + the user file). Without the hook the embedded table is read here with the
/// same validation.
pub static X3D_HOOK: Hook<dyn Fn() -> CiMap<i32> + Send + Sync> = Hook::new();

const TABLES_JSON: &str = include_str!("../../res/lang/tables.json");

/// The "x3d" entries of the embedded tables.json, validated like `Tables.Merge` (sku ^\d{4}X3D$, 4..64 even cores).
fn embedded_x3d() -> CiMap<i32> {
    static V: OnceLock<CiMap<i32>> = OnceLock::new();
    V.get_or_init(|| {
        let mut d = CiMap::new();
        let Ok(serde_json::Value::Object(root)) = serde_json::from_str::<serde_json::Value>(TABLES_JSON.trim_start_matches('\u{feff}')) else {
            return d;
        };
        if let Some(serde_json::Value::Array(list)) = root.get("x3d") {
            for x in list {
                let sku = x.get("sku").and_then(|v| v.as_str()).map(|s| ci::upper(s.trim())).unwrap_or_default();
                let cores = x.get("cores").and_then(|v| v.as_i64()).filter(|n| *n >= i32::MIN as i64 && *n <= i32::MAX as i64).unwrap_or(0) as i32;
                let sc: Vec<char> = sku.chars().collect();
                let ok_sku = sc.len() == 7 && sc[..4].iter().all(|c| c.is_ascii_digit()) && sc[4..] == ['X', '3', 'D'];
                if !ok_sku || !(4..=64).contains(&cores) || cores % 2 != 0 {
                    continue;
                }
                d.ci_insert(&sku, cores);
            }
        }
        d
    })
    .clone()
}

fn x3d_dual() -> CiMap<i32> {
    match X3D_HOOK.get() {
        Some(f) => f(),
        None => embedded_x3d(),
    }
}

/// `\w` of .NET regex (letters, digits, connector punctuation, marks): enough for `\b`.
fn word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// `CoreSplit` (static class).
pub struct CoreSplit;

impl CoreSplit {
    pub fn bits(n: i32) -> u64 {
        if n < 64 { (1u64 << n) - 1 } else { u64::MAX }
    }

    /// `\b(\d{4}X3D)\b` (ignore case), upper-cased.
    pub fn sku_of(name: &str) -> String {
        let c: Vec<char> = name.chars().collect();
        for i in 0..c.len() {
            if i + 7 > c.len() {
                break;
            }
            if i > 0 && word(c[i - 1]) {
                continue;
            }
            if !c[i..i + 4].iter().all(|x| x.is_ascii_digit()) {
                continue;
            }
            let tail: String = c[i + 4..i + 7].iter().collect();
            if !ci::eq(&tail, "X3D") {
                continue;
            }
            if i + 7 < c.len() && word(c[i + 7]) {
                continue;
            }
            return ci::upper(&c[i..i + 7].iter().collect::<String>());
        }
        String::new()
    }

    /// `Core\(TM\)\s+Ultra|Core Ultra` (ignore case).
    pub fn is_core_ultra(name: &str) -> bool {
        let lower = ci::lower(name);
        if lower.contains("core ultra") {
            return true;
        }
        let mut from = 0;
        while let Some(p) = lower[from..].find("core(tm)") {
            let rest = &lower[from + p + 8..];
            let ws = rest.len() - rest.trim_start().len();
            if ws > 0 && rest[ws..].starts_with("ultra") {
                return true;
            }
            from += p + 8;
        }
        false
    }

    fn mb(b: u64) -> String {
        format!("{} MB", b >> 20)
    }

    fn stop(mut p: SplitPlan, code: &str, args: Vec<Arg>) -> SplitPlan {
        p.family = "none".into();
        p.code = code.into();
        p.args = args;
        p
    }

    pub fn detect(t: &Topo) -> SplitPlan {
        let mut p = SplitPlan { name: t.name.clone(), sku: Self::sku_of(&t.name), ..Default::default() };
        if t.groups > 1 {
            return Self::stop(p, "groups", vec![t.groups.into()]);
        }
        let known = t.llc_groups.iter().all(|l| t.l3_by_llc.get(l).map(|v| *v != 0).unwrap_or(false));
        if t.llc_groups.len() > 2 && known {
            return Self::stop(p, "domains", vec![(t.llc_groups.len() as i32).into()]);
        }
        if !known {
            return Self::stop(p, "nocache", vec![]);
        }
        p.probe = t.logical_count;
        p.reported = if t.reported > 0 { t.reported } else { t.logical_count };
        if p.probe != p.reported {
            let a = vec![p.probe.into(), p.reported.into()];
            return Self::stop(p, "probe", a);
        }
        if p.probe > 64 || p.reported > 64 {
            return Self::stop(p, "64+", vec![]);
        }

        let list2: Vec<_> = t.sets.iter().filter(|s| s.group == 0).copied().collect();
        // GroupBy(Core) in first-seen order
        let mut cores: IndexMap<u8, Vec<crate::base::model::CpuSet>> = IndexMap::new();
        for s in &list2 {
            cores.entry(s.core).or_default().push(*s);
        }
        p.all = TopoBuilder::mask(&list2);
        p.smt_on = cores.values().any(|g| g.len() > 1);
        p.cores = t.physical_count;
        let mixed_smt = p.smt_on && cores.values().any(|g| g.len() == 1);

        // A. Hybrid: split by EfficiencyClass, not by cache.
        if t.eff_classes.len() > 1 {
            let top = *t.eff_classes.iter().max().unwrap_or(&0);
            p.family = "hybrid".into();
            p.top_eff = top;
            p.game = TopoBuilder::mask(list2.iter().filter(|s| s.eff == top));
            p.non_game = p.all & !p.game;
            p.p_cores = cores.values().filter(|c| c[0].eff == top).count() as i32;
            p.e_cores = cores.len() as i32 - p.p_cores;
            p.expected = p.reported;
            p.code = if t.llc_groups.len() > 1 { "split".into() } else { "single".into() };
            return Self::validate(t, p);
        }

        // B. Symmetric, exactly two L3 domains.
        if t.llc_groups.len() == 2 {
            if mixed_smt {
                return Self::stop(p, "mixed-smt", vec![]);
            }
            let s0 = t.l3_by_llc[&t.llc_groups[0]];
            let s1 = t.l3_by_llc[&t.llc_groups[1]];
            if s0 == s1 {
                let a = vec![Self::mb(s0).into(), Self::mb(s1).into(), t.name.clone().into()];
                return Self::stop(p, "equal", a);
            }
            let big = if s0 > s1 { t.llc_groups[0] } else { t.llc_groups[1] };
            p.game_l3 = s0.max(s1);
            p.non_game_l3 = s0.min(s1);
            p.game = TopoBuilder::mask(list2.iter().filter(|s| s.llc == big)) & p.all;
            p.non_game = p.all & !p.game;
            let f = if p.smt_on { 2 } else { 1 };
            if !p.sku.is_empty() {
                if let Some(c) = x3d_dual().ci_get(&p.sku).copied() {
                    p.expected = c * f;
                    if p.reported != p.expected {
                        let a = vec![p.sku.clone().into(), p.expected.into(), p.smt_on.into(), p.reported.into()];
                        return Self::stop(p, "sku", a);
                    }
                    let lower = Self::bits(c / 2 * f); // 7900X3D -> 0xFFF, 7950X3D -> 0xFFFF
                    p.family = "x3d".into();
                    p.upper_half = p.non_game == lower;
                }
            }
            p.code = if t.llc_groups.len() > 1 { "split".into() } else { "single".into() };
            return Self::validate(t, p);
        }

        // One domain: nothing to split.
        p.family = "none".into();
        p.code = "single".into();
        p.game = 0;
        p.non_game = 0;
        p
    }

    pub fn validate(t: &Topo, mut p: SplitPlan) -> SplitPlan {
        if p.game == 0 {
            p.problems.push("empty".into());
        }
        if p.non_game == 0 {
            p.problems.push("nongame-empty".into());
        }
        if (p.game | p.non_game) != p.all {
            p.problems.push("coverage".into());
        }
        if (p.game & p.non_game) != 0 {
            p.problems.push("overlap".into());
        }
        if !Self::smt_whole(t, p.game) {
            p.problems.push("smt-game".into());
        }
        if !Self::smt_whole(t, p.non_game) {
            p.problems.push("smt-nongame".into());
        }
        p
    }

    pub fn smt_whole(t: &Topo, mask: u64) -> bool {
        let mut cores: IndexMap<u8, u64> = IndexMap::new();
        for s in t.sets.iter().filter(|s| s.group == 0) {
            let m = cores.entry(s.core).or_insert(0);
            if s.lp < 64 {
                *m |= 1u64 << s.lp;
            }
        }
        for core in cores.values() {
            let inter = core & mask;
            if inter != 0 && inter != *core {
                return false;
            }
        }
        true
    }

    /// Human-readable text of a split code.
    pub fn text(code: &str, args: &[Arg]) -> String {
        let a = |i: usize| -> Arg { args.get(i).cloned().unwrap_or_else(|| Arg::from("?")) };
        match code {
            "split" => L::t("The processor splits into game and non-game cores").to_string(),
            "single" => L::t("One shared L3 and one core class: nothing to split. Priorities, the IRQ exclusion, the background and EcoQoS are what helps; no FPS gain is promised").to_string(),
            "groups" => lf!("More than one processor group ({0}): the split is not supported", a(0)),
            "domains" => lf!("More than two L3 domains ({0}): the split needs exactly two", a(0)),
            "nocache" => L::t("Windows did not report the L3 size of every domain: no split by cache").to_string(),
            "probe" => lf!("CPU Sets show {0} logical processors, Windows reports {1}: some are hidden, the split is off", a(0), a(1)),
            "64+" => L::t("More than 64 logical processors: the split is not supported").to_string(),
            "equal" => lf!("Two L3 domains of the same size ({0} and {1}, {2}): not an X3D, no split by cache; ccd0/ccd1 still work", a(0), a(1), a(2)),
            "mixed-smt" => L::t("SMT is on for some cores and off for others: no symmetric split").to_string(),
            "sku" => lf!("{0} should have {1} logical processors (SMT {2}), {3} are visible: the split is off", a(0), a(1), a(2), a(3)),
            _ => code.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::topo::synth::{self, MB};

    /// SplitTests.X3DSplitsByTheLargerCache
    #[test]
    fn x3d_splits_by_the_larger_cache() {
        let t = synth::x7950x3d();
        let sp = t.split();
        assert_eq!("x3d", sp.family);
        assert!(sp.can_split());
        assert_eq!(0xFFFF, sp.game);
        assert_eq!(0xFFFF_0000, sp.non_game);
        assert!(!sp.upper_half);
        assert_eq!(32, sp.expected);
        let up = synth::symmetric(16, true, 8, &[32 * MB, 96 * MB], "AMD Ryzen 9 9950X3D 16-Core Processor", 0);
        assert!(up.split().upper_half);
        let hidden = synth::symmetric(12, true, 6, &[96 * MB, 32 * MB], "AMD Ryzen 9 7950X3D", 0);
        assert_eq!("sku", hidden.split().code);
        assert!(!hidden.split().can_split());
    }

    /// SplitTests.Others
    #[test]
    fn others() {
        let x = synth::x7950x();
        assert_eq!("equal", x.split().code);
        assert!(!x.split().can_split());
        let h = synth::i13900k();
        let hs = h.split();
        assert_eq!("hybrid", hs.family);
        assert!(hs.can_split());
        assert_eq!(0xFFFF, hs.game);
        assert_eq!(8, hs.p_cores);
        assert_eq!(16, hs.e_cores);
        for t in [synth::i12400(), synth::six(), synth::x7800x3d()] {
            assert_eq!("single", t.split().code);
            assert!(!t.split().can_split());
            assert!(!t.split().why().is_empty());
        }
        let probe = synth::symmetric(6, true, 6, &[18 * MB], "x", 16);
        assert_eq!("probe", probe.split().code);
    }

    /// SplitTests.SmtWhole
    #[test]
    fn smt_whole() {
        let t = synth::x7950x3d();
        assert!(CoreSplit::smt_whole(&t, 0xF));
        assert!(!CoreSplit::smt_whole(&t, 0x1));
    }

    #[test]
    fn sku_and_ultra_like_the_regexes() {
        let _g = crate::base::hooks::static_hooks(); // the texts are compared in English
        assert_eq!(CoreSplit::sku_of("AMD Ryzen 9 7950x3d 16-Core Processor"), "7950X3D");
        assert_eq!(CoreSplit::sku_of("AMD Ryzen 9 17950X3D"), "");
        assert_eq!(CoreSplit::sku_of("AMD Ryzen 9 7950X3DX"), "");
        assert_eq!(CoreSplit::sku_of("7800X3D"), "7800X3D");
        assert!(CoreSplit::is_core_ultra("Intel(R) Core(TM) Ultra 9 285K"));
        assert!(CoreSplit::is_core_ultra("Intel Core Ultra 7 155H"));
        assert!(!CoreSplit::is_core_ultra("Intel(R) Core(TM) i9-13900K"));
        assert_eq!(embedded_x3d().ci_get("7950x3d"), Some(&16));
        assert_eq!(embedded_x3d().len(), 4);
        let t = synth::symmetric(16, true, 8, &[96 * MB, 32 * MB], "AMD Ryzen 9 7950X3D", 0);
        assert!(t.split().why().contains("game"));
        assert_eq!(
            CoreSplit::text("sku", &["7950X3D".into(), 32.into(), true.into(), 24.into()]),
            "7950X3D should have 32 logical processors (SMT True), 24 are visible: the split is off"
        );
    }
}
