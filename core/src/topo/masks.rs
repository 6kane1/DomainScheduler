//! Named and numeric core masks (section 5, port of `Masks.cs`).

use super::builder::TopoBuilder;
use super::core_split::CoreSplit;
use super::model::Topo;
use crate::base::ci;
use crate::base::l::L;
use crate::base::model::{CpuSet, MaskResult, MaskSpec};
use crate::lf;
use indexmap::IndexMap;

fn alias(t: &str) -> Option<&'static str> {
    // Case-insensitive aliases; t is already lower-case.
    Some(match t {
        "auto" | "a" => "auto",
        "all" => "all",
        "game" | "g" => "game",
        "nongame" | "n" => "nongame",
        "vcache" | "v" => "vcache",
        "pcores" | "p" => "pcores",
        "ecores" | "e" => "ecores",
        _ => return None,
    })
}

fn tokens(text: &str) -> Vec<&str> {
    text.split([',', ';', '+']).map(|x| x.trim()).filter(|x| !x.is_empty()).collect()
}

fn hex(m: u64) -> String {
    format!("0x{:x}", m)
}

/// GroupBy(Core) in first-seen order, then the lowest LP of each core (OrderBy(LP).First()).
fn one_per_core<'a>(src: impl IntoIterator<Item = &'a CpuSet>) -> Vec<CpuSet> {
    let mut g: IndexMap<u8, CpuSet> = IndexMap::new();
    for s in src {
        match g.get_mut(&s.core) {
            Some(cur) if s.lp < cur.lp => *cur = *s,
            Some(_) => {}
            None => {
                g.insert(s.core, *s);
            }
        }
    }
    g.into_values().collect()
}

/// `Masks.LimitResult`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LimitResult {
    pub kept: u64,
    pub dropped: u64,
    pub ok: bool,
    pub note: String,
}

/// `Masks` (static class).
pub struct Masks;

impl Masks {
    /// A named spec ("vcache+nosmt", "ccd1;nocore0", "g,nosmt"), None when the text is not one.
    pub fn try_parse_spec(text: &str) -> Option<MaskSpec> {
        let tok = tokens(text);
        if tok.is_empty() {
            return None;
        }
        let mut s = MaskSpec::default();
        let mut have_base = false;
        for raw in tok {
            let t = ci::lower(raw);
            if t == "nosmt" || t == "ns" {
                s.no_smt = true;
                continue;
            }
            if t == "nocore0" || t == "nc0" {
                s.no_core0 = true;
                continue;
            }
            if have_base {
                return None;
            }
            if let Some(b) = alias(&t) {
                s.base = b.into();
                have_base = true;
                continue;
            }
            let num = if let Some(r) = t.strip_prefix("ccd") { Some(r) } else { t.strip_prefix('c') };
            if let Some(num) = num {
                if !num.is_empty() && num.bytes().all(|b| b.is_ascii_digit()) {
                    if let Ok(n) = num.parse::<i32>() {
                        s.base = "ccd".into();
                        s.ccd = n;
                        have_base = true;
                        continue;
                    }
                }
            }
            return None;
        }
        Some(s)
    }

    pub fn to_spec(text: &str) -> Option<String> {
        Self::try_parse_spec(text).map(|s| s.to_string())
    }

    /// 0xFF, 255 or "0,2,4"; false when the text is none of these.
    pub fn parse(text: &str, mask: &mut u64) -> bool {
        *mask = 0;
        let text = text.trim();
        if text.is_empty() {
            return false;
        }
        if text.len() >= 2 && ci::starts_with(text, "0x") {
            let h = &text[2..];
            // NumberStyles.HexNumber: hex digits with leading/trailing white space allowed.
            let h = h.trim_matches(|c: char| matches!(c, ' ' | '\t' | '\n' | '\r' | '\u{b}' | '\u{c}'));
            if h.is_empty() || !h.bytes().all(|b| b.is_ascii_hexdigit()) {
                return false;
            }
            return match u64::from_str_radix(h, 16) {
                Ok(v) => {
                    *mask = v;
                    true
                }
                Err(_) => false,
            };
        }
        if text.contains(',') || text.contains(' ') {
            for p in text.split([',', ' ']).filter(|p| !p.is_empty()) {
                let lp = if p.bytes().all(|b| b.is_ascii_digit()) { p.parse::<i32>().ok() } else { None };
                match lp {
                    Some(lp) if lp <= 63 => *mask |= 1u64 << lp,
                    _ => {
                        *mask = 0;
                        return false;
                    }
                }
            }
            return *mask != 0;
        }
        if text.bytes().all(|b| b.is_ascii_digit()) {
            if let Ok(v) = text.parse::<u64>() {
                *mask = v;
                return true;
            }
        }
        false
    }

    pub fn limit(t: &Topo, mask: u64) -> LimitResult {
        let full = t.full_mask();
        let mut r = LimitResult { kept: mask & full, dropped: mask & !full, ok: false, note: String::new() };
        r.ok = r.kept != 0;
        if r.dropped != 0 {
            r.note = lf!("Processors {0} do not exist and were dropped", Self::ranges(r.dropped));
        }
        r
    }

    pub fn from_value(t: &Topo, mask: u64) -> MaskResult {
        let list: Vec<CpuSet> = t.sets.iter().filter(|x| x.group == 0 && x.lp < 64 && ((mask >> x.lp) & 1) != 0).copied().collect();
        let h = hex(mask);
        let mut by_lp = list.clone();
        by_lp.sort_by_key(|x| x.lp);
        let mut dom: Vec<u8> = list.iter().map(|x| x.llc).collect();
        dom.sort();
        dom.dedup();
        MaskResult {
            mask,
            hex: h.clone(),
            lps: by_lp.iter().map(|x| x.lp.to_string()).collect::<Vec<_>>().join(","),
            set_ids: list.iter().map(|x| x.id).collect(),
            count: list.len() as i32,
            domains: dom.len() as i32,
            portable: false,
            satisfied: !list.is_empty(),
            name: h.clone(),
            spec: h,
            notes: Vec::new(),
        }
    }

    /// A spec or a number to a mask.
    pub fn resolve(t: &Topo, text: &str) -> MaskResult {
        if let Some(spec) = Self::try_parse_spec(text) {
            return Self::build_spec(t, &spec);
        }
        let mut v = 0u64;
        if !Self::parse(text, &mut v) {
            let mut r = Self::build_spec(t, &MaskSpec::default());
            r.satisfied = false;
            r.notes.push(lf!("Unknown mask: {0}", text));
            return r;
        }
        let lim = Self::limit(t, v);
        if !lim.ok {
            let mut r = Self::build_spec(t, &MaskSpec::default());
            r.satisfied = false;
            r.notes.push(lim.note.clone());
            r.notes.push("empty".into());
            return r;
        }
        let mut res = Self::from_value(t, lim.kept);
        if !lim.note.is_empty() {
            res.notes.push(lim.note);
        }
        res
    }

    pub fn build_spec(t: &Topo, s: &MaskSpec) -> MaskResult {
        let mut ok = true;
        let mut notes: Vec<String> = Vec::new();
        let mut fail = |why: String, ok: &mut bool| -> Vec<CpuSet> {
            *ok = false;
            notes.push(why);
            Vec::new()
        };
        let top = t.eff_classes.iter().copied().max().unwrap_or(0);
        let all = || t.sets.clone();
        let sp = t.split();
        let src: Vec<CpuSet> = match s.base.as_str() {
            "all" | "auto" => all(),
            "vcache" => match t.big_cache_llc {
                Some(v) => t.sets.iter().filter(|x| x.llc == v).copied().collect(),
                None => fail("no-vcache".into(), &mut ok),
            },
            "game" => {
                if sp.can_split() {
                    t.sets.iter().filter(|x| x.lp < 64 && (sp.game >> x.lp & 1) != 0).copied().collect()
                } else {
                    fail(sp.why(), &mut ok)
                }
            }
            "nongame" => {
                if sp.can_split() {
                    t.sets.iter().filter(|x| x.lp < 64 && (sp.non_game >> x.lp & 1) != 0).copied().collect()
                } else if let Some(w) = t.big_cache_llc {
                    t.sets.iter().filter(|x| x.llc != w).copied().collect()
                } else {
                    fail("no-split".into(), &mut ok)
                }
            }
            "pcores" => {
                if t.hybrid() {
                    t.sets.iter().filter(|x| x.eff == top).copied().collect()
                } else {
                    fail("not-hybrid".into(), &mut ok)
                }
            }
            "ecores" => {
                if t.hybrid() {
                    t.sets.iter().filter(|x| x.eff < top).copied().collect()
                } else {
                    fail("not-hybrid".into(), &mut ok)
                }
            }
            "ccd" if s.ccd >= 0 && (s.ccd as usize) < t.llc_groups.len() => {
                let l = t.llc_groups[s.ccd as usize];
                t.sets.iter().filter(|x| x.llc == l).copied().collect()
            }
            _ => fail("bad-spec".into(), &mut ok),
        };
        let mut src = src;
        if s.no_smt {
            src = one_per_core(&src);
        }
        if s.no_core0 {
            let fc = t.first_core();
            src.retain(|x| x.core != fc);
        }
        let mut list = src;
        if list.is_empty() {
            ok = false;
            notes.push("empty".into());
            list = t.sets.clone();
        }
        if s.base == "vcache" && t.big_cache_llc.is_some() && sp.can_split() && TopoBuilder::mask(&list) != sp.game && !s.no_smt && !s.no_core0 {
            notes.push("vcache-split-mismatch".into());
        }
        let m = TopoBuilder::mask(&list);
        let mut by_lp = list.clone();
        by_lp.sort_by_key(|x| x.lp);
        let mut dom: Vec<u8> = list.iter().map(|x| x.llc).collect();
        dom.sort();
        dom.dedup();
        MaskResult {
            mask: m,
            hex: hex(m),
            lps: by_lp.iter().map(|x| x.lp.to_string()).collect::<Vec<_>>().join(","),
            set_ids: list.iter().map(|x| x.id).collect(),
            count: list.len() as i32,
            domains: dom.len() as i32,
            portable: true,
            satisfied: ok,
            notes,
            name: s.to_string(),
            spec: s.to_string(),
        }
    }

    /// "0-7,16-23".
    pub fn ranges(mask: u64) -> String {
        let mut parts = Vec::new();
        let mut i = 0;
        while i < 64 {
            if (mask >> i) & 1 == 0 {
                i += 1;
                continue;
            }
            let mut j = i;
            while j + 1 < 64 && (mask >> (j + 1)) & 1 != 0 {
                j += 1;
            }
            parts.push(if j > i { format!("{}-{}", i, j) } else { i.to_string() });
            i = j + 1;
        }
        parts.join(",")
    }

    /// Ideal-processor candidates: one per physical core (lowest LP), P-cores only on hybrid; SMT siblings only when
    /// fewer than 6.
    pub fn ideal_candidates(t: &Topo, skip_cpu0: bool, allow_mask: u64) -> Vec<CpuSet> {
        let mut src: Vec<CpuSet> = t.sets.clone();
        if allow_mask != 0 {
            let f: Vec<CpuSet> = src.iter().filter(|s| s.lp < 64 && (allow_mask >> s.lp & 1) != 0).copied().collect();
            if !f.is_empty() {
                src = f;
            }
        }
        if t.eff_classes.len() > 1 {
            let mx = t.sets.iter().map(|s| s.eff).max().unwrap_or(0);
            src.retain(|s| s.eff == mx);
        }
        // Section 22.7: on a dual-CCD X3D the big-cache domain goes first, whichever half of the processor numbers it
        // is (so UpperHalf does not change which processors are picked).
        let big = if t.split().family == "x3d" { t.big_cache_llc } else { None };
        let mut list = one_per_core(&src);
        // OrderBy(big domain first).ThenBy(LP): a stable sort by the pair.
        list.sort_by_key(|s| (if big.is_some() && Some(s.llc) != big { 1 } else { 0 }, s.lp));
        if skip_cpu0 {
            list.retain(|s| s.lp != 0);
        }
        if list.is_empty() {
            list = t.sets.clone();
            list.sort_by_key(|s| s.lp);
        }
        if list.len() < 6 {
            let mut rest = src.clone();
            rest.sort_by_key(|s| s.lp);
            let extra: Vec<CpuSet> = rest.into_iter().filter(|s| !list.iter().any(|x| x.lp == s.lp)).collect();
            list.extend(extra);
        }
        list
    }

    /// Where background work goes next to a game mask (not the plain complement).
    pub fn bg_mask(t: &Topo, game: u64) -> u64 {
        // 1) whole free cores
        let mut groups: IndexMap<u8, Vec<CpuSet>> = IndexMap::new();
        for s in &t.sets {
            groups.entry(s.core).or_default().push(*s);
        }
        let free = groups.values().filter(|g| g.iter().all(|s| s.lp >= 64 || (game >> s.lp & 1) == 0)).flatten();
        let m = TopoBuilder::mask(free);
        if m != 0 {
            return m;
        }
        // 2) free LPs of the first core
        let fc = t.first_core();
        let m = TopoBuilder::mask(t.sets.iter().filter(|s| s.core == fc && s.lp < 64 && (game >> s.lp & 1) == 0));
        if m != 0 {
            return m;
        }
        // 3) the complement
        let m = t.full_mask() ^ game;
        if m != 0 {
            return m;
        }
        // 4) the first core
        let m = TopoBuilder::mask(t.sets.iter().filter(|s| s.core == fc));
        if m != 0 { m } else { 1 }
    }

    /// What to tell the user about a mask on this processor (empty: nothing).
    pub fn advice(t: &Topo, r: &MaskResult) -> Vec<String> {
        let mut a: Vec<String> = r.notes.iter().map(|n| Self::note_text(n)).collect();
        if CoreSplit::is_core_ultra(&t.name) {
            a.push(L::t("Core Ultra: a third core class (LP E-cores) may exist; the game gets only the top class").to_string());
        }
        if t.hybrid() {
            a.push(L::t("Intel hybrid: Thread Director may ignore the ideal processor; the CPU Sets and EcoQoS off matter more").to_string());
        }
        if t.big_cache_llc.is_some() {
            a.push(L::t("The V-Cache die runs at a lower clock: a CPU-bound game may do better on the other die").to_string());
        }
        a
    }

    pub fn note_text(note: &str) -> String {
        match note {
            "no-vcache" => L::t("No V-Cache die (needs two L3 domains, one at least twice as large)").to_string(),
            "no-split" => L::t("The processor has no game / non-game halves").to_string(),
            "not-hybrid" => L::t("Not a hybrid processor: no P- and E-cores").to_string(),
            "bad-spec" => L::t("Unknown mask").to_string(),
            "empty" => L::t("The mask left no processor: all were taken").to_string(),
            "vcache-split-mismatch" => L::t("The V-Cache detection and the split plan give different masks").to_string(),
            _ => note.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::topo::synth::{self, MB};

    fn p(text: &str) -> (bool, u64) {
        let mut m = 0;
        let ok = Masks::parse(text, &mut m);
        (ok, m)
    }

    /// MaskTests.SpecRoundTrip
    #[test]
    fn spec_round_trip() {
        assert_eq!(Some("ccd1+nosmt".to_string()), Masks::to_spec("ccd1+nosmt"));
        assert_eq!("ccd1+nosmt", Masks::try_parse_spec(&Masks::to_spec("ccd1+nosmt").unwrap()).unwrap().to_string());
        assert_eq!(Some("vcache+nocore0".to_string()), Masks::to_spec("v; nocore0"));
        assert_eq!(Some("game+nosmt".to_string()), Masks::to_spec("g,nosmt"));
        assert_eq!(None, Masks::to_spec("0xff"));
        assert_eq!(None, Masks::to_spec("vcache+game"));
    }

    /// MaskTests.NumericForms
    #[test]
    fn numeric_forms() {
        assert_eq!((true, 0xFF), p("0xFF"));
        assert_eq!((true, 255), p("255"));
        assert_eq!((true, 0x15), p("0,2,4"));
        let t = synth::i12400();
        let r = Masks::resolve(&t, "0x3F000");
        assert!(!r.satisfied);
        let r2 = Masks::resolve(&t, "0x3F");
        assert!(!r2.portable);
        assert_eq!("0x3f", r2.spec);
        let lim = Masks::limit(&t, 0x1FFF);
        assert_eq!(0xFFF, lim.kept);
        assert_eq!(0x1000, lim.dropped);
        assert!(!lim.note.is_empty());
    }

    /// MaskTests.Specs
    #[test]
    fn specs() {
        let x = synth::x7950x3d();
        let v = Masks::resolve(&x, "vcache");
        assert!(v.satisfied);
        assert_eq!(0xFFFF, v.mask);
        assert_eq!(1, v.domains);
        assert_eq!(256, v.set_ids[0]); // CPU Set IDs, not LP indexes
        assert_eq!(0x5555, Masks::resolve(&x, "vcache+nosmt").mask);
        assert_eq!(0xFFFF_0000, Masks::resolve(&x, "ccd1").mask);
        assert_eq!(0xFFFF_0000, Masks::resolve(&x, "nongame").mask);
        assert_eq!(0xFFFF_FFFC, Masks::resolve(&x, "all+nocore0").mask);
        let h = synth::i13900k();
        let hv = Masks::resolve(&h, "vcache");
        assert!(!hv.satisfied);
        assert_eq!(h.full_mask(), hv.mask);
        assert_eq!(0xFFFF, Masks::resolve(&h, "pcores").mask);
        assert_eq!(0xFFFF_0000, Masks::resolve(&h, "ecores").mask);
        assert!(!Masks::resolve(&synth::i12400(), "game").satisfied);
        assert!(!Masks::resolve(&synth::i12400(), "ccd1").satisfied);
        assert_eq!("0-7,16-23", Masks::ranges(0xFF00FF));
    }

    /// MaskTests.IdealCandidatesOnePerCore
    #[test]
    fn ideal_candidates_one_per_core() {
        for t in [synth::x7950x3d(), synth::i13900k(), synth::x7800x3d(), synth::i12400(), synth::six()] {
            let c = Masks::ideal_candidates(&t, false, 0);
            if c.len() >= 6 {
                let mut cores: Vec<u8> = c.iter().map(|x| x.core).collect();
                cores.sort();
                cores.dedup();
                assert_eq!(c.len(), cores.len());
            }
        }
        let x = Masks::ideal_candidates(&synth::x7950x3d(), false, 0xFFFF);
        assert_eq!(8, x.len());
        assert_eq!(vec![0u8, 2, 4, 6, 8, 10, 12, 14], x.iter().map(|s| s.lp).collect::<Vec<_>>());
        let h = Masks::ideal_candidates(&synth::i13900k(), false, 0);
        assert_eq!(8, h.len());
        assert!(h.iter().all(|s| s.eff == 1));
        let four = synth::symmetric(4, true, 4, &[8 * MB], "4c", 0);
        let f = Masks::ideal_candidates(&four, false, 0);
        assert_eq!(8, f.len());
        assert_eq!(1, f[4].lp); // SMT siblings appended at the end
        assert_ne!(0, Masks::ideal_candidates(&synth::six(), true, 0)[0].lp);
    }

    /// MaskTests.BgMaskOrder
    #[test]
    fn bg_mask_order() {
        let x = synth::x7950x3d();
        assert_eq!(0xFFFF_0000, Masks::bg_mask(&x, 0xFFFF));
        assert_eq!(0x2, Masks::bg_mask(&x, 0xFFFF_FFFD));
        assert_eq!(0x3, Masks::bg_mask(&x, 0xFFFF_FFFF));
    }

    /// RoadmapTests.X3dCandidatesStartWithTheBigCacheDomainWhicheverHalfItIs
    #[test]
    fn x3d_candidates_start_with_the_big_cache_domain_whichever_half_it_is() {
        for x in [synth::x7950x3d(), synth::symmetric(16, true, 8, &[32 * MB, 96 * MB], "AMD Ryzen 9 9950X3D 16-Core Processor", 0)] {
            assert_eq!("x3d", x.split().family);
            let cand = Masks::ideal_candidates(&x, false, 0);
            let big = x.big_cache_llc.unwrap();
            let first_other = cand.iter().position(|c| c.llc != big).unwrap();
            assert!(first_other > 0);
            assert!(cand[..first_other].iter().all(|c| c.llc == big));
            assert!(cand[first_other..].iter().all(|c| c.llc != big));
        }
    }

    /// FuzzTests.TextParsersNeverThrow (the mask parsers; the other parsers are their areas')
    #[test]
    fn mask_text_parsers_never_panic() {
        let mut r = crate::base::testrand::Rand::new(1604);
        let t = synth::x7950x3d();
        for _ in 0..crate::base::testrand::iterations() {
            let s = r.text(400);
            let mut m = 0;
            Masks::parse(&s, &mut m);
            Masks::try_parse_spec(&s);
            Masks::resolve(&t, &s);
        }
    }
}
