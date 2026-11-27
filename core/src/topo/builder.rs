//! Builds the processor model (section 3, port of `TopoBuilder.cs`) from the CPU Set and cache buffers.

use super::model::Topo;
use crate::base::hooks::{lock, Hook};
use crate::base::model::{CpuSet, L3Cache};
use crate::base::time::{DateTime, TimeSpan};
use crate::native::sys;
use indexmap::IndexMap;
use std::sync::{Arc, Mutex};

/// `Topo.LiveHook`: tests give the live topology (a fresh one per call, like `Synth.X7800X3D`).
pub static LIVE_HOOK: Hook<dyn Fn() -> Arc<Topo> + Send + Sync> = Hook::new();

struct Cache {
    live: Option<Arc<Topo>>,
    at: DateTime,
}

static GATE: Mutex<Cache> = Mutex::new(Cache { live: None, at: DateTime::MIN });

fn u16le(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}
fn i32le(b: &[u8], o: usize) -> i32 {
    i32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}
fn u32le(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}
fn u64le(b: &[u8], o: usize) -> u64 {
    u64::from_le_bytes(b[o..o + 8].try_into().unwrap_or([0; 8]))
}

/// `TopoBuilder` (static class).
pub struct TopoBuilder;

impl TopoBuilder {
    /// The live topology: the live hook in tests, otherwise read from Windows; cached for 30 s. None when Windows gave
    /// no CPU Sets (and nothing was cached).
    pub fn live() -> Option<Arc<Topo>> {
        if let Some(h) = LIVE_HOOK.get() {
            return Some(h());
        }
        let mut g = lock(&GATE);
        if let Some(t) = &g.live {
            if DateTime::utc_now() - g.at < TimeSpan::from_secs_i(30) {
                return Some(t.clone());
            }
        }
        let sets = Self::parse_cpu_sets(&sys::cpu_set_buffer());
        if sets.is_empty() {
            return g.live.clone();
        }
        // A cache read error is not fatal (the parser never fails: a broken buffer is an empty list).
        let caches = Self::parse_l3(&sys::cache_buffer());
        let t = Arc::new(Self::build(sets, &caches, &sys::cpu_name(), sys::active_lp_count()));
        g.live = Some(t.clone());
        g.at = DateTime::utc_now();
        Some(t)
    }

    /// Drops the cached topology (next live() reads again).
    pub fn invalidate() {
        lock(&GATE).live = None;
    }

    /// Walks a GetSystemCpuSetInformation buffer with step Size, Type 0 only.
    pub fn parse_cpu_sets(buf: &[u8]) -> Vec<CpuSet> {
        let mut list = Vec::new();
        let mut off: usize = 0;
        while off + 8 <= buf.len() {
            let size = i32le(buf, off);
            let ty = i32le(buf, off + 4);
            if size <= 0 || off + size as usize > buf.len() {
                break;
            }
            if ty == 0 && size >= 20 {
                list.push(CpuSet {
                    id: u32le(buf, off + 8),
                    group: u16le(buf, off + 12),
                    lp: buf[off + 14],
                    core: buf[off + 15],
                    llc: buf[off + 16],
                    numa: buf[off + 17],
                    eff: buf[off + 18],
                    flags: buf[off + 19],
                });
            }
            off += size as usize;
        }
        list
    }

    /// Walks a GetLogicalProcessorInformationEx(RelationCache) buffer: level 3 unified (or data) caches with their
    /// group masks. A broken buffer gives an empty list.
    pub fn parse_l3(buf: &[u8]) -> Vec<L3Cache> {
        let mut list = Vec::new();
        let len = buf.len();
        let mut off: usize = 0;
        while off + 8 <= len {
            let rel = i32le(buf, off);
            let size = i32le(buf, off + 4);
            if size <= 0 || off + size as usize > len {
                return Vec::new();
            }
            let end = off + size as usize;
            if rel == 2 && size >= 56 {
                let level = buf[off + 8];
                let csize = u32le(buf, off + 12);
                let ctype = i32le(buf, off + 16);
                if level == 3 && (ctype == 0 || ctype == 2) {
                    // CacheUnified, CacheData; Instruction/Trace skipped
                    let mut count = u16le(buf, off + 38) as usize;
                    if count == 0 {
                        count = 1; // before Windows 11 the count is 0 and one mask follows
                    }
                    let mut c = L3Cache { size: csize as u64, masks: Vec::new() };
                    for i in 0..count {
                        let m = off + 40 + i * 16;
                        if m + 16 > end {
                            return Vec::new();
                        }
                        c.masks.push((u16le(buf, m + 8), u64le(buf, m)));
                    }
                    list.push(c);
                }
            }
            off = end;
        }
        list
    }

    /// Each L3 domain gets the size of the largest cache covering one of its processors.
    pub fn l3_map(sets: &[CpuSet], caches: &[L3Cache]) -> IndexMap<u8, u64> {
        let mut d: IndexMap<u8, u64> = IndexMap::new();
        for s in sets {
            for c in caches {
                if c.masks.iter().any(|m| m.0 == s.group && s.lp < 64 && ((m.1 >> s.lp) & 1) != 0) {
                    match d.get(&s.llc) {
                        Some(cur) if c.size <= *cur => {}
                        _ => {
                            d.insert(s.llc, c.size); // the MAXIMUM
                        }
                    }
                }
            }
        }
        d
    }

    /// The domain with the extra cache: two or more domains, sizes known, largest >= 2x smallest, only one of that size.
    pub fn detect_big_cache(t: &Topo) -> Option<u8> {
        if t.llc_groups.len() < 2 {
            return None;
        }
        if t.llc_groups.iter().any(|l| !t.l3_by_llc.contains_key(l)) {
            return None;
        }
        let sz: Vec<u64> = t.llc_groups.iter().map(|l| t.l3_by_llc[l]).collect();
        let max = *sz.iter().max()?;
        let min = *sz.iter().min()?;
        if min == 0 || max < 2 * min {
            return None;
        }
        let big: Vec<u8> = t.llc_groups.iter().copied().filter(|l| t.l3_by_llc[l] == max).collect();
        if big.len() == 1 { Some(big[0]) } else { None }
    }

    /// The model from CPU Sets and caches.
    pub fn build(sets: Vec<CpuSet>, caches: &[L3Cache], name: &str, reported: i32) -> Topo {
        let mut s = sets;
        s.sort_by(|a, b| a.group.cmp(&b.group).then(a.lp.cmp(&b.lp))); // stable, like OrderBy.ThenBy
        let mut phys: Vec<(u16, u8)> = s.iter().map(|x| (x.group, x.core)).collect();
        phys.sort();
        phys.dedup();
        let mut groups: Vec<u16> = s.iter().map(|x| x.group).collect();
        groups.sort();
        groups.dedup();
        let mut eff: Vec<u8> = s.iter().map(|x| x.eff).collect();
        eff.sort();
        eff.dedup();
        let mut llc: Vec<u8> = s.iter().map(|x| x.llc).collect();
        llc.sort();
        llc.dedup();
        let mut t = Topo {
            logical_count: s.len() as i32,
            physical_count: phys.len() as i32,
            groups: groups.len() as i32,
            reported,
            name: name.to_string(),
            eff_classes: eff,
            llc_groups: llc,
            ..Default::default()
        };
        t.smt = t.logical_count > t.physical_count;
        t.l3_by_llc = Self::l3_map(&s, caches);
        t.sets = s;
        t.big_cache_llc = Self::detect_big_cache(&t);
        t
    }

    /// A bit mask of the sets' logical processors (indexes >= 64 dropped).
    pub fn mask<'a>(sets: impl IntoIterator<Item = &'a CpuSet>) -> u64 {
        let mut m = 0u64;
        for s in sets {
            if s.lp < 64 {
                m |= 1u64 << s.lp;
            }
        }
        m
    }

    pub fn pop_count(m: u64) -> i32 {
        m.count_ones() as i32
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::base::hooks::static_hooks;
    use crate::topo::synth::{self, MB};

    /// TopoTests.ParsesCpuSetBuffer
    #[test]
    fn parses_cpu_set_buffer() {
        let mut buf = vec![0u8; 96];
        let mut put = |off: usize, ty: i32, id: u32, lp: u8, core: u8, llc: u8, eff: u8| {
            buf[off..off + 4].copy_from_slice(&32i32.to_le_bytes());
            buf[off + 4..off + 8].copy_from_slice(&ty.to_le_bytes());
            buf[off + 8..off + 12].copy_from_slice(&id.to_le_bytes());
            buf[off + 14] = lp;
            buf[off + 15] = core;
            buf[off + 16] = llc;
            buf[off + 18] = eff;
        };
        put(0, 0, 256, 0, 0, 0, 1);
        put(32, 7, 0, 0, 0, 0, 0); // another type: skipped
        put(64, 0, 257, 1, 0, 0, 1);
        let sets = TopoBuilder::parse_cpu_sets(&buf);
        assert_eq!(2, sets.len());
        assert_eq!(257, sets[1].id);
        assert_eq!(1, sets[1].lp);
        assert!(TopoBuilder::parse_cpu_sets(&[1, 2, 3]).is_empty());
    }

    fn cache_record(level: u8, ty: i32, size: u32, groups: u16, rec_size: usize) -> Vec<u8> {
        let mut b = vec![0u8; rec_size];
        b[0..4].copy_from_slice(&2i32.to_le_bytes());
        b[4..8].copy_from_slice(&(rec_size as i32).to_le_bytes());
        b[8] = level;
        b[12..16].copy_from_slice(&size.to_le_bytes());
        b[16..20].copy_from_slice(&ty.to_le_bytes());
        b[38..40].copy_from_slice(&groups.to_le_bytes());
        b[40..48].copy_from_slice(&0xFFFFu64.to_le_bytes());
        b
    }

    /// TopoTests.ParsesL3AndRejectsBrokenBuffers
    #[test]
    fn parses_l3_and_rejects_broken_buffers() {
        let l3 = cache_record(3, 0, (32 * MB) as u32, 1, 56);
        let l2 = cache_record(2, 0, 1 << 20, 1, 56);
        let instr = cache_record(3, 1, 1 << 20, 1, 56);
        let all: Vec<u8> = l3.iter().chain(l2.iter()).chain(instr.iter()).copied().collect();
        let caches = TopoBuilder::parse_l3(&all);
        assert_eq!(1, caches.len());
        assert_eq!(32 * MB, caches[0].size);
        assert_eq!(0xFFFF, caches[0].masks[0].1);
        // A record size past the end, or masks past the record: nothing, no exception.
        let mut broken = l3.clone();
        broken[4..8].copy_from_slice(&500i32.to_le_bytes());
        assert!(TopoBuilder::parse_l3(&broken).is_empty());
        assert!(TopoBuilder::parse_l3(&cache_record(3, 0, 1, 3, 56)).is_empty());
    }

    /// TopoTests.L3MapTakesTheMaximum
    #[test]
    fn l3_map_takes_the_maximum() {
        let sets = vec![CpuSet { lp: 0, llc: 0, ..Default::default() }, CpuSet { lp: 1, llc: 0, ..Default::default() }];
        let a = L3Cache { size: 32 * MB, masks: vec![(0, 1)] };
        let b = L3Cache { size: 96 * MB, masks: vec![(0, 2)] };
        assert_eq!(96 * MB, TopoBuilder::l3_map(&sets, &[a, b])[&0]);
    }

    /// TopoTests.Counts
    #[test]
    fn counts() {
        let t = synth::x7950x3d();
        assert_eq!(32, t.logical_count);
        assert_eq!(16, t.physical_count);
        assert!(t.smt);
        assert_eq!(1, t.groups);
        assert_eq!(2, t.llc_groups.len());
        assert!(!t.l3_by_llc.is_empty());
        let h = synth::i13900k();
        assert!(h.hybrid());
        assert_eq!(24, h.physical_count);
        assert_eq!(0xFFFF_FFFF, h.full_mask());
    }

    /// TopoTests.DetectBigCache
    #[test]
    fn detect_big_cache() {
        assert_eq!(Some(0), TopoBuilder::detect_big_cache(&synth::x7950x3d()));
        assert_eq!(None, TopoBuilder::detect_big_cache(&synth::x7950x()));
        assert_eq!(None, TopoBuilder::detect_big_cache(&synth::x7800x3d()));
        let mut missing = synth::x7950x3d();
        let l = missing.llc_groups[1];
        missing.l3_by_llc.shift_remove(&l);
        assert_eq!(None, TopoBuilder::detect_big_cache(&missing));
    }

    /// TopoTests.LiveHookIsUsed
    #[test]
    fn live_hook_is_used() {
        let _g = static_hooks();
        LIVE_HOOK.set(Some(Arc::new(|| Arc::new(synth::i13900k()))));
        assert!(TopoBuilder::live().unwrap().hybrid());
        LIVE_HOOK.set(None);
    }

    /// FuzzTests.InterruptAndCpuSetBuffersNeverThrow (the CPU Set half; Devices.ParseIrq is the system area's)
    #[test]
    fn cpu_set_and_cache_buffers_never_panic() {
        let mut r = crate::base::testrand::Rand::new(1603);
        for _ in 0..crate::base::testrand::iterations() {
            TopoBuilder::parse_cpu_sets(&r.bytes(2000));
            TopoBuilder::parse_l3(&r.bytes(2000));
        }
        assert!(TopoBuilder::parse_cpu_sets(&[]).is_empty());
    }
}
