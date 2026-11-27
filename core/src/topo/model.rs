//! `Topo` and `SplitPlan` (Model.cs): the processor model and its game / non-game split.

use super::core_split::CoreSplit;
use crate::base::fmt::Arg;
use crate::base::l::L;
use crate::base::model::CpuSet;
use crate::lf;
use indexmap::IndexMap;
use std::sync::OnceLock;

#[derive(Clone, Debug, Default)]
pub struct Topo {
    pub sets: Vec<CpuSet>,
    pub logical_count: i32,
    pub physical_count: i32,
    pub groups: i32,
    pub reported: i32,
    pub smt: bool,
    /// distinct, ascending
    pub eff_classes: Vec<u8>,
    /// distinct, ascending
    pub llc_groups: Vec<u8>,
    pub l3_by_llc: IndexMap<u8, u64>,
    pub big_cache_llc: Option<u8>,
    pub name: String,
    pub(crate) split: OnceLock<SplitPlan>,
}

impl Topo {
    pub fn hybrid(&self) -> bool {
        self.eff_classes.len() > 1
    }

    /// `Topo.Split`: detected on first use and kept (like the C# lazy field).
    pub fn split(&self) -> &SplitPlan {
        self.split.get_or_init(|| CoreSplit::detect(self))
    }

    pub fn full_mask(&self) -> u64 {
        if self.logical_count >= 64 { u64::MAX } else { (1u64 << self.logical_count) - 1 }
    }

    /// `Sets.Min(s => s.Core)` (C# throws on an empty list; 0 here).
    pub fn first_core(&self) -> u8 {
        self.sets.iter().map(|s| s.core).min().unwrap_or(0)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct SplitPlan {
    pub family: String,
    pub name: String,
    pub sku: String,
    pub code: String,
    pub args: Vec<Arg>,
    pub game: u64,
    pub non_game: u64,
    pub all: u64,
    pub game_l3: u64,
    pub non_game_l3: u64,
    pub probe: i32,
    pub reported: i32,
    pub expected: i32,
    pub cores: i32,
    pub p_cores: i32,
    pub e_cores: i32,
    pub top_eff: u8,
    pub smt_on: bool,
    pub upper_half: bool,
    pub problems: Vec<String>,
}

impl Default for SplitPlan {
    fn default() -> Self {
        SplitPlan {
            family: "none".into(),
            name: String::new(),
            sku: String::new(),
            code: String::new(),
            args: Vec::new(),
            game: 0,
            non_game: 0,
            all: 0,
            game_l3: 0,
            non_game_l3: 0,
            probe: 0,
            reported: 0,
            expected: 0,
            cores: 0,
            p_cores: 0,
            e_cores: 0,
            top_eff: 0,
            smt_on: false,
            upper_half: false,
            problems: Vec::new(),
        }
    }
}

impl SplitPlan {
    pub fn is_split_family(&self) -> bool {
        self.family == "x3d" || self.family == "hybrid"
    }

    pub fn can_split(&self) -> bool {
        self.is_split_family() && self.problems.is_empty() && self.game != 0 && self.non_game != 0
    }

    pub fn label(&self) -> String {
        match self.family.as_str() {
            "x3d" => self.sku.clone(),
            "hybrid" => L::t("Hybrid (P+E)").to_string(),
            _ => self.family.clone(),
        }
    }

    pub fn why(&self) -> String {
        if !self.problems.is_empty() {
            lf!("Split problems: {0}", self.problems.join(", "))
        } else {
            CoreSplit::text(&self.code, &self.args)
        }
    }
}
