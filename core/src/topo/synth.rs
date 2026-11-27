//! Synthetic topologies of the acceptance list (port of the test helper `Synth.cs`).

use super::builder::TopoBuilder;
use super::model::Topo;
use crate::base::model::{CpuSet, L3Cache};

pub const MB: u64 = 1u64 << 20;

/// cores x (smt?2:1) logical processors; core c is LP 2c, 2c+1 with SMT; one LLC per ccd_cores cores.
/// `reported` 0 = the number of sets.
pub fn symmetric(cores: i32, smt: bool, ccd_cores: i32, sizes: &[u64], name: &str, reported: i32) -> Topo {
    let mut sets = Vec::new();
    let t = if smt { 2 } else { 1 };
    let mut lp = 0;
    for c in 0..cores {
        for _ in 0..t {
            sets.push(CpuSet { id: (256 + lp) as u32, lp: lp as u8, core: c as u8, llc: (c / ccd_cores * ccd_cores) as u8, ..Default::default() });
            lp += 1;
        }
    }
    let mut caches = Vec::new();
    for (i, size) in sizes.iter().enumerate() {
        let m = TopoBuilder::mask(sets.iter().filter(|s| s.core as i32 / ccd_cores == i as i32));
        caches.push(L3Cache { size: *size, masks: vec![(0, m)] });
    }
    let n = sets.len() as i32;
    TopoBuilder::build(sets, &caches, name, if reported > 0 { reported } else { n })
}

pub fn x7950x3d() -> Topo {
    symmetric(16, true, 8, &[96 * MB, 32 * MB], "AMD Ryzen 9 7950X3D 16-Core Processor", 0)
}
pub fn x7950x() -> Topo {
    symmetric(16, true, 8, &[32 * MB, 32 * MB], "AMD Ryzen 9 7950X 16-Core Processor", 0)
}
pub fn x7800x3d() -> Topo {
    symmetric(8, true, 8, &[96 * MB], "AMD Ryzen 7 7800X3D 8-Core Processor", 0)
}
pub fn i12400() -> Topo {
    symmetric(6, true, 6, &[18 * MB], "12th Gen Intel(R) Core(TM) i5-12400F", 0)
}
pub fn six() -> Topo {
    symmetric(6, false, 6, &[12 * MB], "6-core no SMT", 0)
}

/// 13900K: 8 P-cores with SMT (LP 0-15, class 1), 16 E-cores (LP 16-31, class 0), one L3.
pub fn i13900k() -> Topo {
    let mut sets = Vec::new();
    for c in 0..8u32 {
        sets.push(CpuSet { id: 256 + 2 * c, lp: (2 * c) as u8, core: (2 * c) as u8, eff: 1, ..Default::default() });
        sets.push(CpuSet { id: 257 + 2 * c, lp: (2 * c + 1) as u8, core: (2 * c) as u8, eff: 1, ..Default::default() });
    }
    for e in 0..16u32 {
        sets.push(CpuSet { id: 272 + e, lp: (16 + e) as u8, core: (16 + e) as u8, eff: 0, ..Default::default() });
    }
    let c3 = L3Cache { size: 36 * MB, masks: vec![(0, 0xFFFF_FFFF)] };
    TopoBuilder::build(sets, &[c3], "13th Gen Intel(R) Core(TM) i9-13900K", 32)
}
