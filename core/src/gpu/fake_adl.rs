//! Port of `FakeAdl.cs` (tests): an in-memory AMD driver (ADL) for the AMD overclock tests: adapters, an Overdrive8
//! table and an OverdriveN state table, all answered in the byte layouts of the ADL SDK. Like the real driver it
//! refuses values outside its ranges. The state is behind a mutex (`f.lock()`), so a test keeps an `Arc<FakeAdl>`
//! next to the one AmdOc uses.

use super::amd_oc::{AdlRange, AmdOc, IAdl};
use super::{ascii_bytes, rd, wr};
use crate::base::hooks::lock;
use std::sync::{Mutex, MutexGuard};

#[derive(Clone, Debug)]
pub struct Adapter {
    pub index: i32,
    pub bus: i32,
    pub vendor: i32,
    pub supported: i32,
    pub enabled: i32,
    pub version: i32,
    pub asic: i32,
    pub name: String,
    pub active: bool,
    pub asic_missing: bool,
}

impl Default for Adapter {
    fn default() -> Self {
        Adapter { index: 0, bus: 0, vendor: 1002, supported: 1, enabled: 1, version: 8, asic: 1, name: "AMD Radeon".into(), active: true, asic_missing: false }
    }
}

/// One OverdriveN levels write the fake saw: (Memory, Mode, Clocks, Vddc, Enabled).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OdnSet {
    pub memory: bool,
    pub mode: i32,
    pub clocks: Vec<i32>,
    pub vddc: Vec<i32>,
    pub enabled: Vec<i32>,
}

#[derive(Clone, Debug)]
pub struct FakeAdlState {
    pub missing: bool,
    pub adapters: Vec<Adapter>,
    pub calls: Vec<String>,
    pub writes: i32,

    // ---- Overdrive8 ----
    pub od8_caps: i32,
    pub min: Vec<i32>,
    pub max: Vec<i32>,
    pub def: Vec<i32>,
    pub cur: Vec<i32>,
    pub od8_sets: Vec<Vec<(i32, i32, bool)>>,
    pub od8_set_rc: i32,

    // ---- OverdriveN ----
    pub x2: bool,
    pub odn_flags: i32,
    pub eng: AdlRange,
    pub mem_r: AdlRange,
    pub vddc: AdlRange,
    pub pow: AdlRange,
    pub sys_clk: Vec<i32>,
    pub sys_vddc: Vec<i32>,
    pub sys_def_clk: Vec<i32>,
    pub mem_clk: Vec<i32>,
    pub mem_vddc: Vec<i32>,
    pub mem_def_clk: Vec<i32>,
    pub sys_enabled: Option<Vec<i32>>,
    pub mem_enabled: Option<Vec<i32>>,
    pub sys_control: Option<Vec<i32>>,
    pub mem_control: Option<Vec<i32>>,
    pub tdp: i32,
    pub tdp_mode: i32,
    pub max_temp: i32,
    pub fail_sys_set: i32,
    pub fail_mem_set: i32,
    pub fail_power_set: i32,
    pub default_read_fails: bool,
    pub odn_sets: Vec<OdnSet>,
    pub power_sets: Vec<(i32, i32)>,
}

impl Default for FakeAdlState {
    fn default() -> Self {
        let n = AmdOc::OD8_COUNT as usize;
        FakeAdlState {
            missing: false,
            adapters: Vec::new(),
            calls: Vec::new(),
            writes: 0,
            od8_caps: 0,
            min: vec![0; n],
            max: vec![0; n],
            def: vec![0; n],
            cur: vec![0; n],
            od8_sets: Vec::new(),
            od8_set_rc: 0,
            x2: true,
            odn_flags: 0,
            eng: AdlRange::default(),
            mem_r: AdlRange::default(),
            vddc: AdlRange::default(),
            pow: AdlRange::default(),
            sys_clk: Vec::new(),
            sys_vddc: Vec::new(),
            sys_def_clk: Vec::new(),
            mem_clk: Vec::new(),
            mem_vddc: Vec::new(),
            mem_def_clk: Vec::new(),
            sys_enabled: None,
            mem_enabled: None,
            sys_control: None,
            mem_control: None,
            tdp: 0,
            tdp_mode: 0,
            max_temp: 85,
            fail_sys_set: 0,
            fail_mem_set: 0,
            fail_power_set: 0,
            default_read_fails: false,
            odn_sets: Vec::new(),
            power_sets: Vec::new(),
        }
    }
}

impl FakeAdlState {
    pub fn od8(&mut self, id: i32, min: i32, max: i32, def: i32, cur: Option<i32>) {
        let i = id as usize;
        self.min[i] = min;
        self.max[i] = max;
        self.def[i] = def;
        self.cur[i] = cur.unwrap_or(def);
    }

    fn a(&self, index: i32) -> &Adapter {
        self.adapters.iter().find(|a| a.index == index).expect("Sequence contains no matching element")
    }
}

#[derive(Default)]
pub struct FakeAdl {
    state: Mutex<FakeAdlState>,
}

impl FakeAdl {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_state(s: FakeAdlState) -> Self {
        FakeAdl { state: Mutex::new(s) }
    }

    pub fn lock(&self) -> MutexGuard<'_, FakeAdlState> {
        lock(&self.state)
    }

    // ---- cards ----------------------------------------------------------------------------------------

    /// An RX 580 8 GB as Wattman shows it (OverdriveN, X2 calls, clocks in 10 kHz units): 8 core states up to 1366 MHz,
    /// 3 memory states up to 2000 MHz, core range 300–2000 MHz, memory 300–2250 MHz, power limit −50…+50 %, no
    /// memory underclocking.
    pub fn rx580(x2: bool, scale: i32) -> FakeAdl {
        let mut f = FakeAdlState { x2, ..Default::default() };
        f.adapters.push(Adapter { index: 0, bus: 1, version: 7, name: "Radeon RX 580 Series".into(), ..Default::default() });
        // second output of the same card
        f.adapters.push(Adapter { index: 1, bus: 1, version: 7, name: "Radeon RX 580 Series".into(), active: false, ..Default::default() });
        f.odn_flags = AmdOc::ODN_SCLK | AmdOc::ODN_MCLK | (1 << 2) | (1 << 3) | AmdOc::ODN_POWER;
        f.eng = AdlRange::new(300 * scale, 2000 * scale, scale, 1366 * scale);
        f.mem_r = AdlRange::new(300 * scale, 2250 * scale, scale, 2000 * scale);
        f.vddc = AdlRange::new(750, 1200, 1, 1150);
        f.pow = AdlRange::new(-50, 50, 1, 0);
        f.sys_def_clk = [300, 600, 900, 1145, 1215, 1257, 1300, 1366].iter().map(|x| x * scale).collect();
        f.sys_clk = f.sys_def_clk.clone();
        f.sys_vddc = vec![750, 769, 906, 1000, 1056, 1100, 1125, 1150];
        f.sys_enabled = Some(vec![1, 1, 1, 1, 1, 1, 1, 2]); // a driver may report "enabled" as another non-zero value
        f.mem_def_clk = [300, 1000, 2000].iter().map(|x| x * scale).collect();
        f.mem_clk = f.mem_def_clk.clone();
        f.mem_vddc = vec![750, 800, 950];
        FakeAdl::from_state(f)
    }

    /// `FakeAdl.Rx580()` with its defaults (x2, 10 kHz units).
    pub fn rx580_default() -> FakeAdl {
        Self::rx580(true, 100)
    }

    /// An RX 6800 XT-like Overdrive8 table: max GPU clock 500–2800 MHz (stock 2250), min clock 500, max memory clock
    /// 1000–1150 MHz (stock 1000), power limit −10…+15 %, tuning mode 0–3 (stock 0). voltage: with an offset range when asked.
    pub fn rx6800(voltage: bool) -> FakeAdl {
        let mut f = FakeAdlState::default();
        f.adapters.push(Adapter { index: 0, bus: 3, version: 8, name: "AMD Radeon RX 6800 XT".into(), ..Default::default() });
        f.od8_caps = AmdOc::CAP_GFX_LIMITS | AmdOc::CAP_UCLK_MAX | AmdOc::CAP_POWER | AmdOc::CAP_POWER_MODE | if voltage { AmdOc::CAP_VOLTAGE } else { 0 };
        f.od8(AmdOc::GFX_MAX, 500, 2800, 2250, None);
        f.od8(AmdOc::GFX_MIN, 500, 2800, 500, None);
        f.od8(AmdOc::UCLK_MAX, 1000, 1150, 1000, None);
        f.od8(AmdOc::POWER_PCT, -10, 15, 0, None);
        f.od8(AmdOc::POWER_MODE, 0, 3, 0, None);
        if voltage {
            f.od8(AmdOc::OD_VOLTAGE, -200, 50, 0, None);
        }
        FakeAdl::from_state(f)
    }
}

fn i(b: &[u8], at: i32) -> i32 {
    rd(b, at).expect("FakeAdl read inside the block")
}

fn w(b: &mut [u8], at: i32, v: i32) {
    wr(b, at, v).expect("FakeAdl write inside the block")
}

impl IAdl for FakeAdl {
    fn open(&self, error: &mut String) -> bool {
        let mut s = self.lock();
        s.calls.push("Open".into());
        *error = if s.missing { "No AMD driver is installed (its interface atiadlxx.dll was not found)".into() } else { String::new() };
        !s.missing
    }

    fn adapter_count(&self, count: &mut i32) -> i32 {
        *count = self.lock().adapters.len() as i32;
        0
    }

    fn adapter_info(&self, buffer: &mut [u8]) -> i32 {
        let s = self.lock();
        for (k, a) in s.adapters.iter().enumerate() {
            let at = k as i32 * AmdOc::ADAPTER_INFO_SIZE;
            w(buffer, at, AmdOc::ADAPTER_INFO_SIZE);
            w(buffer, at + 4, a.index);
            w(buffer, at + AmdOc::BUS_AT, a.bus);
            w(buffer, at + AmdOc::VENDOR_AT, a.vendor);
            let name = ascii_bytes(&a.name);
            let from = (at + AmdOc::NAME_AT) as usize;
            buffer[from..from + name.len()].copy_from_slice(&name);
        }
        0
    }

    fn active(&self, adapter: i32, status: &mut i32) -> i32 {
        *status = if self.lock().a(adapter).active { 1 } else { 0 };
        0
    }

    fn overdrive_caps(&self, adapter: i32, supported: &mut i32, enabled: &mut i32, version: &mut i32) -> i32 {
        let s = self.lock();
        let a = s.a(adapter);
        *supported = a.supported;
        *enabled = a.enabled;
        *version = a.version;
        0
    }

    fn asic_type(&self, adapter: i32, types: &mut i32, valids: &mut i32) -> i32 {
        let s = self.lock();
        let a = s.a(adapter);
        *types = a.asic;
        *valids = 0xFF;
        if a.asic_missing {
            -8
        } else {
            0
        }
    }

    fn od8_init(&self, _adapter: i32, caps: &mut i32, list: &mut Option<Vec<u8>>) -> i32 {
        let s = self.lock();
        *caps = s.od8_caps;
        let mut l = vec![0u8; s.min.len() * AmdOc::OD8_SINGLE_SIZE as usize];
        for k in 0..s.min.len() {
            let k32 = k as i32;
            w(&mut l, k32 * 16 + 4, s.min[k]);
            w(&mut l, k32 * 16 + 8, s.max[k]);
            w(&mut l, k32 * 16 + 12, s.def[k]);
        }
        *list = Some(l);
        0
    }

    fn od8_current(&self, _adapter: i32, values: &mut Option<Vec<i32>>) -> i32 {
        *values = Some(self.lock().cur.clone());
        0
    }

    fn od8_set(&self, _adapter: i32, set: &[u8], current: &mut [u8]) -> i32 {
        let mut s = self.lock();
        s.writes += 1;
        let count = i(set, 0);
        let mut items = Vec::new();
        for id in 0..count {
            let at = 4 + id * AmdOc::OD8_SET_ENTRY;
            if i(set, at + 4) == 0 {
                continue;
            }
            items.push((id, i(set, at), i(set, at + 8) != 0));
        }
        s.od8_sets.push(items.clone());
        if s.od8_set_rc != 0 {
            return s.od8_set_rc;
        }
        for &(id, v, reset) in &items {
            if !reset && (v < s.min[id as usize] || v > s.max[id as usize]) {
                return -3; // out of range: nothing is taken
            }
        }
        for &(id, v, reset) in &items {
            s.cur[id as usize] = if reset { s.def[id as usize] } else { v };
        }
        w(current, 0, count);
        let mut k = 0;
        while k < count && (4 + k * 4 + 4) as usize <= current.len() {
            let v = s.cur[k as usize];
            w(current, 4 + k * 4, v);
            k += 1;
        }
        0
    }

    fn odn_caps(&self, _adapter: i32, caps: &mut Option<Vec<u8>>, x2: &mut bool) -> i32 {
        let s = self.lock();
        *x2 = s.x2;
        let mut c = vec![0u8; if s.x2 { AmdOc::ODN_CAPS_X2 } else { AmdOc::ODN_CAPS } as usize];
        w(&mut c, 0, s.sys_clk.len() as i32);
        let bas = if s.x2 { 8 } else { 4 };
        if s.x2 {
            w(&mut c, 4, s.odn_flags);
        }
        let ranges = [s.eng, s.mem_r, s.vddc, s.pow];
        for (k, r) in ranges.iter().enumerate() {
            let at = bas + k as i32 * AmdOc::ODN_RANGE;
            w(&mut c, at + 4, r.min);
            w(&mut c, at + 8, r.max);
            w(&mut c, at + 12, r.step);
            w(&mut c, at + 16, r.def);
        }
        *caps = Some(c);
        0
    }

    fn odn_clocks(&self, _adapter: i32, memory: bool, set: bool, x2: bool, levels: &mut [u8]) -> i32 {
        let mut s = self.lock();
        let size = if x2 { 16 } else { 12 };
        let clk = if memory { s.mem_clk.clone() } else { s.sys_clk.clone() };
        let vdd = if memory { s.mem_vddc.clone() } else { s.sys_vddc.clone() };
        let def = if memory { s.mem_def_clk.clone() } else { s.sys_def_clk.clone() };
        let en = (if memory { s.mem_enabled.clone() } else { s.sys_enabled.clone() }).unwrap_or_else(|| vec![1; clk.len()]);
        let ctl = (if memory { s.mem_control.clone() } else { s.sys_control.clone() }).unwrap_or_else(|| vec![7; clk.len()]);
        let range = if memory { s.mem_r } else { s.eng };
        if !set {
            let mode = i(levels, 4);
            if mode == AmdOc::MODE_DEFAULT && s.default_read_fails {
                return -8;
            }
            w(levels, 8, clk.len() as i32);
            for k in 0..clk.len() {
                let at = 12 + k as i32 * size;
                w(levels, at, if mode == AmdOc::MODE_DEFAULT { def[k] } else { clk[k] });
                w(levels, at + 4, vdd[k]);
                w(levels, at + 8, en[k]);
                if x2 {
                    w(levels, at + 12, ctl[k]);
                }
            }
            return 0;
        }
        s.writes += 1;
        let (n, m) = (i(levels, 8), i(levels, 4));
        let mut c = vec![0; n as usize];
        let mut v = vec![0; n as usize];
        let mut e = vec![0; n as usize];
        for k in 0..n {
            c[k as usize] = i(levels, 12 + k * size);
            v[k as usize] = i(levels, 12 + k * size + 4);
            e[k as usize] = i(levels, 12 + k * size + 8);
        }
        s.odn_sets.push(OdnSet { memory, mode: m, clocks: c.clone(), vddc: v, enabled: e });
        let fail = if memory { s.fail_mem_set } else { s.fail_sys_set };
        if fail != 0 {
            return fail;
        }
        if c.iter().any(|&x| x < range.min || x > range.max) {
            return -3;
        }
        if m == AmdOc::MODE_DEFAULT {
            c = def.clone();
        }
        let target = if memory { &mut s.mem_clk } else { &mut s.sys_clk };
        target[..n as usize].copy_from_slice(&c[..n as usize]);
        0
    }

    fn odn_power(&self, _adapter: i32, set: bool, power: &mut [u8]) -> i32 {
        let mut s = self.lock();
        if !set {
            w(power, 0, s.tdp_mode);
            w(power, 4, s.tdp);
            w(power, 8, s.max_temp);
            return 0;
        }
        s.writes += 1;
        let (mode, tdp) = (i(power, 0), i(power, 4));
        s.power_sets.push((mode, tdp));
        if s.fail_power_set != 0 {
            return s.fail_power_set;
        }
        if mode == AmdOc::MODE_AUTO {
            s.tdp_mode = mode;
            s.tdp = s.pow.def;
            return 0;
        }
        if tdp < s.pow.min || tdp > s.pow.max {
            return -3;
        }
        s.tdp_mode = mode;
        s.tdp = tdp;
        0
    }

    fn odn_temperature(&self, _adapter: i32, value: &mut i32) -> i32 {
        *value = 65000;
        0
    }

    fn pm_log(&self, _adapter: i32, output: &mut [u8]) -> i32 {
        w(output, 4 + AmdOc::PM_EDGE * 8, 1);
        w(output, 4 + AmdOc::PM_EDGE * 8 + 4, 60);
        0
    }
}
