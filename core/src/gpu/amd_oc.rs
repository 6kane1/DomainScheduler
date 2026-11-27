//! Port of `AmdOc.cs`: the IAdl interface, the AMD overclock (Overdrive8 / OverdriveN) and GpuOc (which maker).
//! The real ADL is in `adl_native.rs`.

use super::nv_oc::{NvClock, NvOc};
use super::{ascii, at, clampi, clampl, rd, wr};
use crate::base::flags::Flags;
use crate::base::hooks::{lock, Hook};
use crate::base::l::L;
use crate::lf;
use std::sync::{Arc, Mutex};

/// The AMD driver's display library (ADL, atiadlxx.dll) as the overclock needs it. Structures travel as byte blocks in
/// the driver's own layout (documented at each call) so the parsing and building in AmdOc can be tested with a fake.
/// A return value of 0 is ADL_OK; negative values are ADL error codes (-8 = not supported; the wrapper returns it too
/// when the driver does not export a function).
pub trait IAdl: Send + Sync {
    /// Loads atiadlxx.dll and creates a context. False with a reason when the library or a basic call is missing.
    fn open(&self, error: &mut String) -> bool;
    fn adapter_count(&self, count: &mut i32) -> i32;
    /// ADL2_Adapter_AdapterInfo_Get into count × AmdOc::ADAPTER_INFO_SIZE bytes.
    fn adapter_info(&self, buffer: &mut [u8]) -> i32;
    fn active(&self, adapter: i32, status: &mut i32) -> i32;
    fn overdrive_caps(&self, adapter: i32, supported: &mut i32, enabled: &mut i32, version: &mut i32) -> i32;
    fn asic_type(&self, adapter: i32, types: &mut i32, valids: &mut i32) -> i32;
    /// ADL2_Overdrive8_Init_SettingX2_Get: the capability bits and count × 16 bytes (featureID, min, max, default).
    fn od8_init(&self, adapter: i32, caps: &mut i32, list: &mut Option<Vec<u8>>) -> i32;
    /// ADL2_Overdrive8_Current_SettingX2_Get: one value per setting.
    fn od8_current(&self, adapter: i32, values: &mut Option<Vec<i32>>) -> i32;
    /// ADL2_Overdrive8_Setting_Set with an ADLOD8SetSetting block and room for the ADLOD8CurrentSetting answer.
    fn od8_set(&self, adapter: i32, set: &[u8], current: &mut [u8]) -> i32;
    /// ADL2_OverdriveN_CapabilitiesX2_Get (x2 = true, 208 bytes) or ADL2_OverdriveN_Capabilities_Get (164 bytes).
    fn odn_caps(&self, adapter: i32, caps: &mut Option<Vec<u8>>, x2: &mut bool) -> i32;
    /// ADL2_OverdriveN_{System|Memory}Clocks[X2]_{Get|Set} on an ADLODNPerformanceLevels[X2] block.
    fn odn_clocks(&self, adapter: i32, memory: bool, set: bool, x2: bool, levels: &mut [u8]) -> i32;
    /// ADL2_OverdriveN_PowerLimit_{Get|Set} on an ADLODNPowerLimitSetting block (12 bytes).
    fn odn_power(&self, adapter: i32, set: bool, power: &mut [u8]) -> i32;
    fn odn_temperature(&self, adapter: i32, value: &mut i32) -> i32;
    /// ADL2_New_QueryPMLogData_Get into an ADLPMLogDataOutput block (4 + 256 × 8 bytes).
    fn pm_log(&self, adapter: i32, output: &mut [u8]) -> i32;
}

/// One ADLODNParameterRange as AmdOc reads it: the C# tuple `(int Min, int Max, int Step, int Def)`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AdlRange {
    pub min: i32,
    pub max: i32,
    pub step: i32,
    pub def: i32,
}

impl AdlRange {
    pub const fn new(min: i32, max: i32, step: i32, def: i32) -> Self {
        AdlRange { min, max, step, def }
    }
}

/// One adjustable value of an AMD card, in the driver's absolute terms: MHz, percent or millivolts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AmdDomain {
    pub editable: bool,
    /// What may be written: the driver's range, raised to the clock of the state below and cut by our own rules.
    pub min: i32,
    pub max: i32,
    /// The driver's stock value (the offsets in the profiles count from it) and the value in force now.
    pub default: i32,
    pub current: i32,
    /// The driver's step in the same unit (1 when it reports none).
    pub step: i32,
}

impl Default for AmdDomain {
    fn default() -> Self {
        AmdDomain { editable: false, min: 0, max: 0, default: 0, current: 0, step: 1 }
    }
}

impl AmdDomain {
    pub fn offset(&self) -> i32 {
        self.current.wrapping_sub(self.default)
    }
}

/// What the driver reports for the chosen AMD card.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AmdState {
    /// 8 = Overdrive8 (RX 5000 and newer, Vega 20), 7 = OverdriveN (Polaris RX 400/500, Vega 10).
    pub version: i32,
    pub adapter: i32,
    pub name: String,
    pub core: AmdDomain,
    pub mem: AmdDomain,
    pub power: AmdDomain,
    pub volt: AmdDomain,
    // Overdrive8: the capability bits and the per-setting tables (index = ADLOD8SettingId).
    pub caps: i32,
    pub min: Vec<i32>,
    pub max: Vec<i32>,
    pub def: Vec<i32>,
    pub cur: Vec<i32>,
    // OverdriveN: the blocks as read (written back with only the top clock changed), the unit and the flags.
    pub x2: bool,
    pub flags: i32,
    pub core_scale: i32,
    pub mem_scale: i32,
    pub power_default: i32,
    pub sys_levels: Option<Vec<u8>>,
    pub mem_levels: Option<Vec<u8>>,
    pub sys_default: Option<Vec<u8>>,
    pub mem_default: Option<Vec<u8>>,
    pub power_raw: Option<Vec<u8>>,
    pub sys_count: i32,
    pub mem_count: i32,
}

impl AmdState {
    /// `new AmdState()` (CoreScale = MemScale = 1).
    pub fn new() -> Self {
        AmdState { core_scale: 1, mem_scale: 1, ..Default::default() }
    }

    pub fn kind(&self) -> &'static str {
        if self.version == 8 {
            "Overdrive8"
        } else {
            "OverdriveN"
        }
    }
}

/// One adapter as ADL lists it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AdlAdapter {
    pub index: i32,
    pub bus: i32,
    pub vendor: i32,
    pub name: String,
    pub active: bool,
    pub od_supported: i32,
    pub od_enabled: i32,
    pub od_version: i32,
    pub asic_known: bool,
    pub asic_types: i32,
}

/// One requested Overdrive8 setting: (Id, Value, Reset).
pub type Od8Item = (i32, i32, bool);

/// The static fields of AmdOc (behind `_gate`).
struct State {
    native: Option<Arc<dyn IAdl>>,
    tried: bool,
    changed: bool,
    adapter: Option<AdlAdapter>,
    error: String,
}

static GATE: Mutex<State> = Mutex::new(State { native: None, tried: false, changed: false, adapter: None, error: String::new() });

/// Overclocking for AMD Radeon cards through the AMD driver's own interface (ADL), the counterpart of NvOc:
///  - Overdrive8 (version 8 in ADL2_Overdrive_Caps: RX 5000/6000/7000/9000): the maximum GPU clock, the maximum memory
///    clock, the power limit in percent and (where the driver offers it) the voltage offset — only lowering;
///  - OverdriveN (version 7: Polaris RX 470/480/570/580/590, Vega 56/64): the clock of the TOP performance state of
///    the GPU and of the memory and the power limit; lower states and every voltage are written back as read;
///  - every value inside the range the driver reports, on its step, and inside the same caps as NVIDIA
///    (+300 MHz core, +1500 MHz memory, never below −300 MHz); the profiles store offsets from the driver's stock
///    values (read from the driver every time), so a profile means the same on both makers;
///  - every write is read back; "Reset to stock" uses the driver's own defaults (Overdrive8: the reset flag with the
///    default values; OverdriveN: the default state table read from the driver, power limit back to automatic) and
///    does not depend on anything this program remembers, so it also works after a failed or partial apply.
/// Only active adapters of a physical AMD card are used (one per PCI bus, integrated Radeon graphics left out).
pub struct AmdOc;

impl AmdOc {
    pub const ADAPTER_INFO_SIZE: i32 = 1572;
    pub const NAME_AT: i32 = 280;
    pub const NAME_LEN: i32 = 256;
    pub const BUS_AT: i32 = 264;
    pub const VENDOR_AT: i32 = 276;
    pub const OD8_SINGLE_SIZE: i32 = 16;
    pub const OD8_SET_ENTRY: i32 = 12;
    /// OD8_COUNT of the ADL SDK headers this code follows; the driver's own count is used when it reports one.
    pub const OD8_COUNT: i32 = 77;
    // ADLOD8SettingId
    pub const GFX_MAX: i32 = 0;
    pub const GFX_MIN: i32 = 1;
    pub const UCLK_MAX: i32 = 8;
    pub const POWER_PCT: i32 = 9;
    pub const UCLK_MIN: i32 = 34;
    pub const POWER_MODE: i32 = 36;
    pub const OD_VOLTAGE: i32 = 37;
    // ADLOD8FeatureControl bits
    pub const CAP_GFX_LIMITS: i32 = 1 << 0;
    pub const CAP_GFX_CURVE: i32 = 1 << 1;
    pub const CAP_UCLK_MAX: i32 = 1 << 2;
    pub const CAP_POWER: i32 = 1 << 3;
    pub const CAP_POWER_MODE: i32 = 1 << 16;
    pub const CAP_VOLTAGE: i32 = 1 << 17;
    /// OD8_OPTIMZED_POWER_MODE value "manual" (from the SDK sample): the driver takes manual values only in it.
    pub const POWER_MODE_MANUAL: i32 = 3;
    // ADLODNFeatureControl bits, ADLODNDPMMaskType, ADLODNControlType
    pub const ODN_SCLK: i32 = 1 << 0;
    pub const ODN_MCLK: i32 = 1 << 1;
    pub const ODN_POWER: i32 = 1 << 9;
    pub const ODN_MCLK_UNDERCLOCK: i32 = 1 << 14;
    pub const DPM_CLOCK: i32 = 1 << 0;
    pub const MODE_CURRENT: i32 = 0;
    pub const MODE_DEFAULT: i32 = 1;
    pub const MODE_AUTO: i32 = 2;
    pub const MODE_MANUAL: i32 = 3;
    pub const ODN_RANGE: i32 = 20;
    pub const ODN_CAPS_X2: i32 = 208;
    pub const ODN_CAPS: i32 = 164;
    pub const POWER_SIZE: i32 = 12;
    pub const PM_LOG_SIZE: i32 = 4 + 256 * 8;
    pub const PM_EDGE: i32 = 8;

    /// `AmdOc.Error`.
    pub fn error() -> String {
        lock(&GATE).error.clone()
    }

    /// `AmdOc.Name`.
    pub fn name() -> String {
        lock(&GATE).adapter.as_ref().map(|a| a.name.clone()).unwrap_or_default()
    }

    /// The driver interface in use (the real one unless a test put in a fake).
    pub fn native() -> Arc<dyn IAdl> {
        native_in(&mut lock(&GATE))
    }

    /// Test hook: a fake driver (None: the real one again); forgets the chosen card.
    pub fn use_(native: Option<Arc<dyn IAdl>>) {
        let st = &mut *lock(&GATE);
        st.native = native;
        st.tried = false;
        st.changed = false;
        st.adapter = None;
        st.error = String::new();
    }

    // ---- choosing the card ---------------------------------------------------------------------------

    /// The adapters of the AdapterInfo block (count × 1572 bytes): index, bus, vendor and name.
    pub fn parse_adapters(info: Option<&[u8]>, count: i32) -> Vec<AdlAdapter> {
        let mut list = Vec::new();
        let Some(info) = info else { return list };
        let size = Self::ADAPTER_INFO_SIZE as usize;
        let mut i = 0usize;
        while (i as i64) < count as i64 && (i + 1) * size <= info.len() {
            let at = i * size;
            let raw = &info[at + Self::NAME_AT as usize..at + (Self::NAME_AT + Self::NAME_LEN) as usize];
            let name = ascii(raw);
            let name = match name.find('\0') {
                Some(z) => &name[..z],
                None => &name[..],
            };
            let at = at as i32;
            list.push(AdlAdapter {
                index: rd(info, at + 4).unwrap_or(0),
                bus: rd(info, at + Self::BUS_AT).unwrap_or(0),
                vendor: rd(info, at + Self::VENDOR_AT).unwrap_or(0),
                name: name.trim().to_string(),
                ..Default::default()
            });
            i += 1;
        }
        list
    }

    /// The card to work on: an active adapter of a physical AMD GPU (vendor 1002, a PCI bus), one per bus, not an
    /// integrated Radeon, whose driver reports Overdrive8 or OverdriveN. The lowest bus wins. None with a reason otherwise.
    pub fn pick(all: &[AdlAdapter]) -> (Option<AdlAdapter>, String) {
        // GroupBy(Bus) in the order of first appearance, the lowest index of each group (OrderBy is stable).
        let mut groups: Vec<(i32, AdlAdapter)> = Vec::new();
        for a in all.iter().filter(|a| (a.vendor == 1002 || a.vendor == 0x1002) && a.bus > -1 && a.active) {
            match groups.iter_mut().find(|g| g.0 == a.bus) {
                Some(g) => {
                    if a.index < g.1.index {
                        g.1 = a.clone();
                    }
                }
                None => groups.push((a.bus, a.clone())),
            }
        }
        let mut amd: Vec<AdlAdapter> = groups
            .into_iter()
            .map(|g| g.1)
            .filter(|a| !a.asic_known || (a.asic_types & 1) != 0 || (a.asic_types & 2) == 0) // ADL_ASIC_DISCRETE / _INTEGRATED
            .collect();
        amd.sort_by_key(|a| a.bus); // stable, like OrderBy
        if amd.is_empty() {
            return (None, L::t("No active AMD Radeon graphics card was found").to_string());
        }
        if let Some(od) = amd.iter().find(|a| a.od_supported != 0 && (a.od_version == 7 || a.od_version == 8)) {
            return (Some(od.clone()), String::new());
        }
        let first = &amd[0];
        (
            None,
            if first.od_supported == 0 {
                L::t("The AMD driver reports no overclocking support for this card").to_string()
            } else {
                lf!("The AMD driver offers an older overclocking interface (version {0}) that is not supported", first.od_version)
            },
        )
    }

    /// Finds the AMD card. False (with Error) when there is none or the driver does not answer.
    pub fn open() -> bool {
        open_in(&mut lock(&GATE))
    }

    // ---- the pure part: ranges, targets, blocks -------------------------------------------------------

    /// A domain as the NVIDIA clock shape the sliders use: stock, offset and the allowed offsets in kHz.
    pub fn as_clock(d: &AmdDomain, domain: i32) -> NvClock {
        NvClock {
            domain,
            editable: d.editable,
            base_khz: d.default.wrapping_mul(1000),
            delta_khz: d.current.wrapping_sub(d.default).wrapping_mul(1000),
            min_delta_khz: 0i32.min(d.min.wrapping_sub(d.default)).wrapping_mul(1000),
            max_delta_khz: 0i32.max(d.max.wrapping_sub(d.default)).wrapping_mul(1000),
        }
    }

    /// The adjustable core and memory clocks in the NVIDIA shape (domains 0 and 4; a clock the driver does not let us
    /// change is left out, so the page says so instead of showing it).
    pub fn clocks(s: &AmdState) -> Vec<NvClock> {
        [(&s.core, NvOc::DOMAIN_CORE), (&s.mem, NvOc::DOMAIN_MEMORY)]
            .into_iter()
            .filter(|x| x.0.editable)
            .map(|x| Self::as_clock(x.0, x.1))
            .collect()
    }

    /// The absolute value that will be written for an offset: the offset cut by our caps (clocks: NvOc's), the result
    /// inside the domain's range and on the driver's step counted from the range's low end (rounded inward).
    pub fn target(d: &AmdDomain, offset: i32, cap_lo: i32, cap_hi: i32) -> i32 {
        let off = clampl(offset as i64, 0i32.min(cap_lo) as i64, 0i32.max(cap_hi) as i64);
        let (lo, hi) = (d.min.min(d.default), d.max.max(d.default));
        let v = clampl(d.default as i64 + off, lo as i64, hi as i64) as i32;
        if v == d.default {
            return v; // stock is always a valid value, whatever the grid
        }
        let step = 1i32.max(d.step);
        // On the driver's grid (counted from the low end of the range), rounded towards stock: never further than asked.
        let mut r = if v > d.default {
            lo.wrapping_add(v.wrapping_sub(lo) / step * step)
        } else {
            lo.wrapping_add(v.wrapping_sub(lo).wrapping_add(step - 1) / step * step)
        };
        if v > d.default && r < d.default {
            r = d.default;
        }
        if v < d.default && r > d.default {
            r = d.default;
        }
        clampi(r, lo, hi)
    }

    pub fn core_target(d: &AmdDomain, off_mhz: i32) -> i32 {
        Self::target(d, off_mhz, NvOc::FLOOR_KHZ / 1000, NvOc::CAP_CORE_KHZ / 1000)
    }
    pub fn mem_target(d: &AmdDomain, off_mhz: i32) -> i32 {
        Self::target(d, off_mhz, NvOc::FLOOR_KHZ / 1000, NvOc::CAP_MEMORY_KHZ / 1000)
    }
    pub fn power_target(d: &AmdDomain, off_pct: i32) -> i32 {
        Self::target(d, off_pct, i32::MIN / 2, i32::MAX / 2)
    }
    /// The voltage offset only goes down (undervolt): raising it is never written.
    pub fn volt_target(d: &AmdDomain, off_mv: i32) -> i32 {
        Self::target(d, 0i32.min(off_mv), i32::MIN / 2, 0)
    }

    /// The Overdrive8 domains from the driver's tables (index = setting id). Err: an index outside the tables
    /// (C#: IndexOutOfRangeException).
    pub fn fill_od8(s: &mut AmdState) -> Result<(), String> {
        let n = s.cur.len().min(s.def.len()) as i32;
        let has = |s: &AmdState, id: i32| -> Result<bool, String> { Ok(id < n && at(&s.min, id)? <= at(&s.max, id)?) };
        let in_ = |s: &AmdState, id: i32, v: i32| -> Result<bool, String> { Ok(has(s, id)? && v >= at(&s.min, id)? && v <= at(&s.max, id)?) };
        let dom = |s: &AmdState, id: i32, cap: i32, lo: i32, hi: i32| -> Result<AmdDomain, String> {
            let mut d = AmdDomain::default();
            if (s.caps & cap) == 0 || !has(s, id)? {
                return Ok(d);
            }
            d.min = at(&s.min, id)?;
            d.max = at(&s.max, id)?;
            d.default = at(&s.def, id)?;
            d.current = at(&s.cur, id)?;
            // Plausible values only: the stock value inside the reported range, and the range inside what a card can be.
            d.editable = d.default >= d.min && d.default <= d.max && d.min >= lo && d.max <= hi;
            Ok(d)
        };
        s.core = dom(s, Self::GFX_MAX, Self::CAP_GFX_LIMITS, 100, 10_000)?;
        s.mem = dom(s, Self::UCLK_MAX, Self::CAP_UCLK_MAX, 50, 20_000)?;
        s.power = dom(s, Self::POWER_PCT, Self::CAP_POWER, -100, 300)?;
        s.volt = dom(s, Self::OD_VOLTAGE, Self::CAP_VOLTAGE, -1000, 1000)?;
        // The maximum clock stays at or above the minimum clock in force.
        if s.core.editable {
            let v = at(&s.cur, Self::GFX_MIN)?;
            if in_(s, Self::GFX_MIN, v)? {
                s.core.min = s.core.min.max(v).min(s.core.default);
            }
        }
        if s.mem.editable {
            let v = at(&s.cur, Self::UCLK_MIN)?;
            if in_(s, Self::UCLK_MIN, v)? {
                s.mem.min = s.mem.min.max(v).min(s.mem.default);
            }
        }
        if s.volt.editable {
            s.volt.max = s.volt.max.min(s.volt.default);
        }
        Ok(())
    }

    /// An ADLOD8SetSetting block: count, then per setting (value, requested, reset).
    pub fn build_od8_set(count: i32, items: &[Od8Item]) -> Vec<u8> {
        let mut b = vec![0u8; (4 + count.max(1) as i64 * Self::OD8_SET_ENTRY as i64) as usize];
        let _ = wr(&mut b, 0, count);
        for &(id, value, reset) in items {
            if id < 0 || id >= count {
                continue;
            }
            let at = 4 + id * Self::OD8_SET_ENTRY;
            let _ = wr(&mut b, at, value);
            let _ = wr(&mut b, at + 4, 1);
            let _ = wr(&mut b, at + 8, if reset { 1 } else { 0 });
        }
        b
    }

    /// The settings one Overdrive8 apply requests: each changed domain with its target, and the minimum clocks that pair
    /// with a changed maximum with their values in force (as the SDK sample does). Unchanged domains are not sent.
    pub fn od8_items(s: &AmdState, core: i32, mem: i32, power: i32, volt: i32) -> Result<Vec<Od8Item>, String> {
        let mut list = Vec::new();
        let n = s.cur.len() as i32;
        let keep = |id: i32| -> Result<bool, String> {
            Ok(id < n && (id as usize) < s.min.len() && at(&s.cur, id)? >= at(&s.min, id)? && at(&s.cur, id)? <= at(&s.max, id)?)
        };
        if s.core.editable && core != s.core.current {
            list.push((Self::GFX_MAX, core, false));
            if keep(Self::GFX_MIN)? {
                list.push((Self::GFX_MIN, at(&s.cur, Self::GFX_MIN)?, false));
            }
        }
        if s.mem.editable && mem != s.mem.current {
            list.push((Self::UCLK_MAX, mem, false));
            if keep(Self::UCLK_MIN)? && at(&s.max, Self::UCLK_MIN)? > 0 {
                list.push((Self::UCLK_MIN, at(&s.cur, Self::UCLK_MIN)?, false));
            }
        }
        if s.power.editable && power != s.power.current {
            list.push((Self::POWER_PCT, power, false));
        }
        if s.volt.editable && volt != s.volt.current {
            list.push((Self::OD_VOLTAGE, volt, false));
        }
        Ok(list)
    }

    /// Overdrive8 back to stock: every setting this program writes, with the reset flag and the default value.
    pub fn od8_reset_items(s: &AmdState) -> Result<Vec<Od8Item>, String> {
        let mut list = Vec::new();
        let n = s.def.len().min(s.cur.len()) as i32;
        let mut add = |id: i32, cap: i32| -> Result<(), String> {
            if (s.caps & cap) != 0 && id < n {
                list.push((id, at(&s.def, id)?, true));
            }
            Ok(())
        };
        add(Self::GFX_MAX, Self::CAP_GFX_LIMITS)?;
        add(Self::GFX_MIN, Self::CAP_GFX_LIMITS)?;
        add(Self::UCLK_MAX, Self::CAP_UCLK_MAX)?;
        if Self::UCLK_MIN < n && at(&s.max, Self::UCLK_MIN)? > 0 {
            add(Self::UCLK_MIN, Self::CAP_UCLK_MAX)?;
        }
        add(Self::POWER_PCT, Self::CAP_POWER)?;
        add(Self::OD_VOLTAGE, Self::CAP_VOLTAGE)?;
        add(Self::POWER_MODE, Self::CAP_POWER_MODE)?;
        Ok(list)
    }

    /// The power-mode switch to "manual" Overdrive8 needs before manual values count; None when not needed.
    pub fn od8_manual_mode(s: &AmdState) -> Result<Option<Vec<Od8Item>>, String> {
        if (s.caps & Self::CAP_POWER_MODE) == 0 || Self::POWER_MODE >= s.cur.len() as i32 || at(&s.cur, Self::POWER_MODE)? == Self::POWER_MODE_MANUAL {
            return Ok(None);
        }
        if Self::POWER_MODE_MANUAL < at(&s.min, Self::POWER_MODE)? || Self::POWER_MODE_MANUAL > at(&s.max, Self::POWER_MODE)? {
            return Ok(None);
        }
        Ok(Some(vec![(Self::POWER_MODE, Self::POWER_MODE_MANUAL, false)]))
    }

    /// One ADLODNParameterRange of a capabilities block: (min, max, step, default).
    pub fn odn_range_at(caps: &[u8], x2: bool, k: i32) -> Result<AdlRange, String> {
        let at = (if x2 { 8 } else { 4 }) + k * Self::ODN_RANGE;
        Ok(AdlRange::new(rd(caps, at + 4)?, rd(caps, at + 8)?, rd(caps, at + 12)?, rd(caps, at + 16)?))
    }

    pub fn level_size(x2: bool) -> i32 {
        if x2 {
            16
        } else {
            12
        }
    }
    pub fn levels_bytes(x2: bool, count: i32) -> i32 {
        12 + Self::level_size(x2) * count
    }

    /// An empty ADLODNPerformanceLevels[X2] block asking for the given mode.
    pub fn new_levels(x2: bool, count: i32, mode: i32) -> Vec<u8> {
        let len = Self::levels_bytes(x2, count).max(0);
        let mut b = vec![0u8; len as usize];
        let _ = wr(&mut b, 0, len);
        let _ = wr(&mut b, 4, mode);
        let _ = wr(&mut b, 8, count);
        b
    }

    pub fn clock(levels: &[u8], x2: bool, i: i32) -> Result<i32, String> {
        rd(levels, 12 + i * Self::level_size(x2))
    }
    pub fn vddc(levels: &[u8], x2: bool, i: i32) -> Result<i32, String> {
        rd(levels, 12 + i * Self::level_size(x2) + 4)
    }
    pub fn control(levels: &[u8], x2: bool, i: i32) -> Result<i32, String> {
        if x2 {
            rd(levels, 12 + i * 16 + 12)
        } else {
            Ok(Self::DPM_CLOCK)
        }
    }

    /// The unit of OverdriveN clocks: values above 10000 are 10 kHz units (Wattman's own), else MHz.
    pub fn scale(range_max: i32) -> i32 {
        if range_max > 10_000 {
            100
        } else {
            1
        }
    }

    /// One OverdriveN clock domain: the top state of the levels, the range of the capabilities.
    #[allow(clippy::too_many_arguments)]
    pub fn odn_clock(cur: Option<&[u8]>, def: Option<&[u8]>, count: i32, x2: bool, range: AdlRange, scale: i32, feature: bool, floor_at_stock: bool) -> Result<AmdDomain, String> {
        let mut d = AmdDomain::default();
        let Some(cur) = cur else { return Ok(d) };
        if count < 1 {
            return Ok(d);
        }
        let top = count - 1;
        d.min = range.min / scale;
        d.max = range.max / scale;
        d.step = 1i32.max(range.step / scale);
        d.current = Self::clock(cur, x2, top)? / scale;
        let mut stock = match def {
            Some(def) => Self::clock(def, x2, top)?,
            None => 0,
        };
        if stock < range.min || stock > range.max {
            stock = range.def; // the range's own default when the table is empty
        }
        d.default = stock / scale;
        let plausible = range.min > 0 && range.min < range.max && stock >= range.min && stock <= range.max && d.max <= 20_000 && d.default >= 100;
        d.editable = feature && plausible && (Self::control(cur, x2, top)? & Self::DPM_CLOCK) != 0;
        if !d.editable {
            return Ok(d);
        }
        // The top state never goes below the state under it; memory never below stock unless the driver allows underclocking.
        if top > 0 {
            d.min = d.min.max(Self::clock(cur, x2, top - 1)? / scale);
        }
        if floor_at_stock {
            d.min = d.min.max(d.default);
        }
        d.min = d.min.min(d.default);
        Ok(d)
    }

    /// The levels block to write for a new top clock: the block as read, mode "manual", each state's enable flag as
    /// 0/1 (as the SDK sample does), the top clock changed; every other clock and every voltage as read.
    pub fn odn_write(cur: &[u8], x2: bool, count: i32, top_clock_raw: i32) -> Result<Vec<u8>, String> {
        let mut b = cur.to_vec();
        wr(&mut b, 4, Self::MODE_MANUAL)?;
        for i in 0..count {
            let at = 12 + i * Self::level_size(x2) + 8;
            let v = rd(&b, at)?;
            wr(&mut b, at, if v > 0 { 1 } else { 0 })?;
        }
        wr(&mut b, 12 + (count - 1) * Self::level_size(x2), top_clock_raw)?;
        Ok(b)
    }

    /// The voltages written back must be what the driver reports: inside its voltage range or equal to its stock table.
    /// False when a state's voltage is neither (then nothing is written).
    pub fn voltages_safe(cur: &[u8], def: Option<&[u8]>, x2: bool, count: i32, vddc: AdlRange) -> Result<bool, String> {
        for i in 0..count {
            let v = Self::vddc(cur, x2, i)?;
            let in_range = vddc.min <= vddc.max && v >= vddc.min && v <= vddc.max;
            let stock = match def {
                Some(def) => Self::vddc(def, x2, i)? == v,
                None => false,
            };
            if !in_range && !stock {
                return Ok(false);
            }
        }
        Ok(true)
    }

    // ---- reading ------------------------------------------------------------------------------------

    /// The clocks, power limit and voltage as the driver reports them now; None (with Error) when they cannot be read.
    pub fn read() -> Option<AmdState> {
        read_in(&mut lock(&GATE))
    }

    /// ADL status codes in words.
    pub fn status_text(rc: i32) -> String {
        match rc {
            -8 | -22 => L::t("this graphics card or driver does not support it").to_string(),
            -23 => L::t("administrator rights are needed").to_string(),
            -10 => L::t("the graphics card is disabled").to_string(),
            -3 | -4 => L::t("the driver refused the values").to_string(),
            _ => lf!("driver code {0}", rc),
        }
    }

    // ---- writing ------------------------------------------------------------------------------------

    /// Writes the offsets (from the driver's stock values: core and memory MHz, power limit percent, voltage mV, only
    /// downwards) and reads back. Returns the state in force after the write, or None; Error says what failed or differs.
    pub fn apply(core_off_mhz: i32, mem_off_mhz: i32, power_off_pct: i32, volt_off_mv: i32) -> Option<AmdState> {
        let st = &mut *lock(&GATE);
        if Flags::refuse(Flags::GPU_OC, L::t("GPU overclock")) {
            st.error = L::t("The feature flag gpu-oc is closed").to_string();
            return None;
        }
        let s = read_in(st)?;
        let core = if s.core.editable { Self::core_target(&s.core, core_off_mhz) } else { s.core.current };
        let mem = if s.mem.editable { Self::mem_target(&s.mem, mem_off_mhz) } else { s.mem.current };
        let power = if s.power.editable { Self::power_target(&s.power, power_off_pct) } else { s.power.current };
        let volt = if s.volt.editable { Self::volt_target(&s.volt, volt_off_mv) } else { s.volt.current };
        let mut errors: Vec<String> = Vec::new();
        let r = if s.version == 8 { write_od8(st, &s, core, mem, power, volt, &mut errors) } else { write_odn(st, &s, core, mem, power, &mut errors) };
        if let Err(e) = r {
            errors.push(e);
        }
        let Some(after) = read_in(st) else {
            if !errors.is_empty() {
                st.error = errors.join("; ");
            }
            return None;
        };
        if errors.is_empty() {
            let same = (!s.core.editable || after.core.current == core)
                && (!s.mem.editable || after.mem.current == mem)
                && (!s.power.editable || after.power.current == power)
                && (!s.volt.editable || after.volt.current == volt);
            if !same {
                errors.push(L::t("The values read back differ from the request").to_string());
            }
        }
        st.error = errors.join("; ");
        Some(after)
    }

    /// Clocks, power limit and voltage back to the driver's stock values. Only on the user's "Reset" (never by itself at
    /// exit). Reads everything fresh from the driver, so it works after a failed apply and after a restart of the program.
    pub fn reset(force: bool) -> bool {
        let st = &mut *lock(&GATE);
        if !st.changed && !force {
            return false;
        }
        if !open_in(st) {
            return false;
        }
        let n = native_in(st);
        let adapter = st.adapter.clone().unwrap_or_default();
        let mut ok = true;
        let mut errors: Vec<String> = Vec::new();
        let r: Result<(), String> = (|| {
            if adapter.od_version == 8 {
                let mut caps = 0;
                let mut list = None;
                let rc = n.od8_init(adapter.index, &mut caps, &mut list);
                let mut cur = None;
                let rc2 = n.od8_current(adapter.index, &mut cur);
                match (rc, rc2, list, cur) {
                    (0, 0, Some(list), Some(cur)) => {
                        let count = ((list.len() as i32) / Self::OD8_SINGLE_SIZE).min(cur.len() as i32);
                        let mut s = AmdState::new();
                        s.caps = caps;
                        s.cur = cur[..count.max(0) as usize].to_vec();
                        let (mn, mx, df) = od8_tables(&list, count)?;
                        s.min = mn;
                        s.max = mx;
                        s.def = df;
                        let items = Self::od8_reset_items(&s)?;
                        let mut current = vec![0u8; (4 + count * 4) as usize];
                        let rs = n.od8_set(adapter.index, &Self::build_od8_set(count, &items), &mut current);
                        if rs != 0 {
                            errors.push(Self::status_text(rs));
                            ok = false;
                        }
                    }
                    _ => {
                        errors.push(Self::status_text(if rc != 0 { rc } else { rc2 }));
                        ok = false;
                    }
                }
            } else {
                let mut caps = None;
                let mut x2 = false;
                let rc = n.odn_caps(adapter.index, &mut caps, &mut x2);
                match (rc, caps) {
                    (0, Some(caps)) => {
                        let levels = clampi(rd(&caps, 0)?, 1, 16);
                        for memory in [false, true] {
                            // The driver's own stock table, written back as it is (SDK sample: Get with Default, then Set).
                            let mut def = Self::new_levels(x2, levels, Self::MODE_DEFAULT);
                            let mut r = n.odn_clocks(adapter.index, memory, false, x2, &mut def);
                            if r == 0 {
                                wr(&mut def, 4, Self::MODE_DEFAULT)?;
                                r = n.odn_clocks(adapter.index, memory, true, x2, &mut def);
                            }
                            if r != 0 {
                                errors.push(format!("{}: {}", if memory { L::t("Memory clock") } else { L::t("Core clock") }, Self::status_text(r)));
                                ok = false;
                            }
                        }
                        let pow = Self::odn_range_at(&caps, x2, 3)?;
                        let mut p = vec![0u8; Self::POWER_SIZE as usize];
                        if (!x2 || (rd(&caps, 4)? & Self::ODN_POWER) != 0) && n.odn_power(adapter.index, false, &mut p) == 0 {
                            wr(&mut p, 0, Self::MODE_AUTO)?;
                            if pow.def >= pow.min && pow.def <= pow.max {
                                wr(&mut p, 4, pow.def)?;
                            }
                            let r = n.odn_power(adapter.index, true, &mut p);
                            if r != 0 {
                                errors.push(format!("{}: {}", L::t("Power limit"), Self::status_text(r)));
                                ok = false;
                            }
                        }
                    }
                    (rc, _) => {
                        errors.push(Self::status_text(rc));
                        ok = false;
                    }
                }
            }
            Ok(())
        })();
        if let Err(e) = r {
            errors.push(e);
            ok = false;
        }
        st.changed = false;
        st.error = errors.join("; ");
        ok
    }

    /// The GPU (edge) temperature in °C, or -1 when the driver does not say.
    pub fn temperature() -> i32 {
        let st = &mut *lock(&GATE);
        let Some(adapter) = st.adapter.clone() else { return -1 };
        let n = native_in(st);
        let t;
        if adapter.od_version == 8 {
            let mut b = vec![0u8; Self::PM_LOG_SIZE as usize];
            let _ = wr(&mut b, 0, Self::PM_LOG_SIZE);
            if n.pm_log(adapter.index, &mut b) != 0 || rd(&b, 4 + Self::PM_EDGE * 8).unwrap_or(0) == 0 {
                return -1;
            }
            t = rd(&b, 4 + Self::PM_EDGE * 8 + 4).unwrap_or(-1);
        } else {
            let mut v = 0;
            if n.odn_temperature(adapter.index, &mut v) != 0 {
                return -1;
            }
            t = if v > 1000 { v / 1000 } else { v }; // OverdriveN reports millidegrees
        }
        if t > 0 && t < 130 {
            t
        } else {
            -1
        }
    }
}

fn native_in(st: &mut State) -> Arc<dyn IAdl> {
    st.native.get_or_insert_with(|| Arc::new(super::adl_native::AdlNative::new()) as Arc<dyn IAdl>).clone()
}

fn open_in(st: &mut State) -> bool {
    if st.adapter.is_some() {
        return true;
    }
    if st.tried {
        return false;
    }
    st.tried = true;
    let n = native_in(st);
    let mut why = String::new();
    if !n.open(&mut why) {
        st.error = why;
        return false;
    }
    let mut count = 0;
    if n.adapter_count(&mut count) != 0 || !(1..=256).contains(&count) {
        st.error = L::t("No active AMD Radeon graphics card was found").to_string();
        return false;
    }
    let mut info = vec![0u8; (count * AmdOc::ADAPTER_INFO_SIZE) as usize];
    if n.adapter_info(&mut info) != 0 {
        st.error = L::t("The AMD driver did not list its graphics cards").to_string();
        return false;
    }
    let mut all = AmdOc::parse_adapters(Some(&info), count);
    for a in all.iter_mut() {
        if !(a.vendor == 1002 || a.vendor == 0x1002) || a.bus < 0 {
            continue;
        }
        let mut status = 0;
        a.active = n.active(a.index, &mut status) == 0 && status != 0;
        if !a.active {
            continue;
        }
        let (mut sup, mut en, mut ver) = (0, 0, 0);
        if n.overdrive_caps(a.index, &mut sup, &mut en, &mut ver) == 0 {
            a.od_supported = sup;
            a.od_enabled = en;
            a.od_version = ver;
        }
        let (mut types, mut valids) = (0, 0);
        if n.asic_type(a.index, &mut types, &mut valids) == 0 {
            a.asic_known = true;
            a.asic_types = if valids != 0 { types & valids } else { types };
        }
    }
    let (pick, reason) = AmdOc::pick(&all);
    let Some(pick) = pick else {
        st.error = reason;
        return false;
    };
    st.adapter = Some(pick);
    st.error = String::new();
    true
}

/// The Min/Max/Def tables of an Overdrive8 init list (count entries of 16 bytes: featureID, min, max, default).
fn od8_tables(list: &[u8], count: i32) -> Result<(Vec<i32>, Vec<i32>, Vec<i32>), String> {
    let c = count.max(0) as usize;
    let (mut mn, mut mx, mut df) = (vec![0; c], vec![0; c], vec![0; c]);
    for i in 0..count {
        mn[i as usize] = rd(list, i * 16 + 4)?;
        mx[i as usize] = rd(list, i * 16 + 8)?;
        df[i as usize] = rd(list, i * 16 + 12)?;
    }
    Ok((mn, mx, df))
}

fn read_in(st: &mut State) -> Option<AmdState> {
    if !open_in(st) {
        return None;
    }
    let adapter = st.adapter.clone().unwrap_or_default();
    let mut s = AmdState::new();
    s.version = adapter.od_version;
    s.adapter = adapter.index;
    s.name = adapter.name.clone();
    let ok = if s.version == 8 { read_od8(st, &mut s) } else { read_odn(st, &mut s) };
    match ok {
        Ok(false) => None,
        Err(e) => {
            st.error = e;
            None
        }
        Ok(true) => {
            if !s.core.editable && !s.mem.editable && !s.power.editable && !s.volt.editable {
                st.error = L::t("The AMD driver reports no adjustable clocks or limits for this card").to_string();
                return None;
            }
            Some(s)
        }
    }
}

fn read_od8(st: &mut State, s: &mut AmdState) -> Result<bool, String> {
    let n = native_in(st);
    let mut caps = 0;
    let mut list = None;
    let rc = n.od8_init(s.adapter, &mut caps, &mut list);
    let list = match list {
        Some(l) if rc == 0 && l.len() >= AmdOc::OD8_SINGLE_SIZE as usize => l,
        _ => {
            st.error = lf!("The AMD driver did not report its overclocking ranges: {0}", AmdOc::status_text(rc));
            return Ok(false);
        }
    };
    let mut cur = None;
    let rc = n.od8_current(s.adapter, &mut cur);
    let cur = match cur {
        Some(c) if rc == 0 && !c.is_empty() => c,
        _ => {
            st.error = lf!("The AMD driver did not report its current settings: {0}", AmdOc::status_text(rc));
            return Ok(false);
        }
    };
    let count = ((list.len() as i32) / AmdOc::OD8_SINGLE_SIZE).min(cur.len() as i32);
    s.caps = caps;
    let (mn, mx, df) = od8_tables(&list, count)?;
    s.min = mn;
    s.max = mx;
    s.def = df;
    s.cur = cur[..count as usize].to_vec();
    AmdOc::fill_od8(s)?;
    Ok(true)
}

fn read_odn(st: &mut State, s: &mut AmdState) -> Result<bool, String> {
    let n = native_in(st);
    let mut caps = None;
    let mut x2 = false;
    let rc = n.odn_caps(s.adapter, &mut caps, &mut x2);
    let caps = match caps {
        Some(c) if rc == 0 && c.len() >= (if x2 { AmdOc::ODN_CAPS_X2 } else { AmdOc::ODN_CAPS }) as usize => c,
        _ => {
            st.error = lf!("The AMD driver did not report its overclocking ranges: {0}", AmdOc::status_text(rc));
            return Ok(false);
        }
    };
    let levels = rd(&caps, 0)?;
    if !(1..=16).contains(&levels) {
        st.error = L::t("The AMD driver's clock table is not in the expected form: overclocking is not offered for this card").to_string();
        return Ok(false);
    }
    s.x2 = x2;
    s.flags = if x2 { rd(&caps, 4)? } else { AmdOc::ODN_SCLK | AmdOc::ODN_MCLK | AmdOc::ODN_POWER };
    let eng = AmdOc::odn_range_at(&caps, x2, 0)?;
    let mem = AmdOc::odn_range_at(&caps, x2, 1)?;
    let vddc = AmdOc::odn_range_at(&caps, x2, 2)?;
    let pow = AmdOc::odn_range_at(&caps, x2, 3)?;
    s.core_scale = AmdOc::scale(eng.max);
    s.mem_scale = AmdOc::scale(mem.max);

    let levels_of = |memory: bool| -> Result<(Option<Vec<u8>>, Option<Vec<u8>>, i32), String> {
        let mut cur = AmdOc::new_levels(x2, levels, AmdOc::MODE_CURRENT);
        if n.odn_clocks(s.adapter, memory, false, x2, &mut cur) != 0 {
            return Ok((None, None, 0));
        }
        let c = clampi(rd(&cur, 8)?, 0, levels);
        let mut def = AmdOc::new_levels(x2, levels, AmdOc::MODE_DEFAULT);
        let def = if n.odn_clocks(s.adapter, memory, false, x2, &mut def) != 0 || rd(&def, 8)? != c { None } else { Some(def) };
        Ok((Some(cur), def, c))
    };
    let sys = levels_of(false)?;
    let mm = levels_of(true)?;
    s.sys_levels = sys.0.clone();
    s.sys_default = sys.1.clone();
    s.sys_count = sys.2;
    s.mem_levels = mm.0.clone();
    s.mem_default = mm.1.clone();
    s.mem_count = mm.2;
    let underclock = x2 && (s.flags & AmdOc::ODN_MCLK_UNDERCLOCK) != 0;
    s.core = AmdOc::odn_clock(sys.0.as_deref(), sys.1.as_deref(), sys.2, x2, eng, s.core_scale, (s.flags & AmdOc::ODN_SCLK) != 0, false)?;
    s.mem = AmdOc::odn_clock(mm.0.as_deref(), mm.1.as_deref(), mm.2, x2, mem, s.mem_scale, (s.flags & AmdOc::ODN_MCLK) != 0, !underclock)?;
    // Nothing is written when the voltages that would go back with the clocks are not what the driver reports.
    if s.core.editable && !AmdOc::voltages_safe(sys.0.as_deref().unwrap_or(&[]), sys.1.as_deref(), x2, sys.2, vddc)? {
        s.core.editable = false;
    }
    if s.mem.editable && !AmdOc::voltages_safe(mm.0.as_deref().unwrap_or(&[]), mm.1.as_deref(), x2, mm.2, vddc)? {
        s.mem.editable = false;
    }

    let mut p = vec![0u8; AmdOc::POWER_SIZE as usize];
    if (s.flags & AmdOc::ODN_POWER) != 0 && n.odn_power(s.adapter, false, &mut p) == 0 && pow.min <= pow.max && pow.min >= -100 && pow.max <= 300 {
        let current = rd(&p, 4)?;
        s.power_raw = Some(p);
        s.power_default = pow.def;
        s.power = AmdDomain {
            min: pow.min,
            max: pow.max,
            step: 1i32.max(pow.step),
            current,
            default: if pow.def >= pow.min && pow.def <= pow.max { pow.def } else { 0 },
            editable: false,
        };
        s.power.editable = s.power.default >= pow.min && s.power.default <= pow.max;
    }
    Ok(true)
}

fn write_od8(st: &mut State, s: &AmdState, core: i32, mem: i32, power: i32, volt: i32, errors: &mut Vec<String>) -> Result<(), String> {
    let items = AmdOc::od8_items(s, core, mem, power, volt)?;
    if items.is_empty() {
        return Ok(());
    }
    let n = native_in(st);
    let count = s.cur.len() as i32;
    st.changed = true;
    if let Some(mode) = AmdOc::od8_manual_mode(s)? {
        let mut current = vec![0u8; (4 + count * 4) as usize];
        let rm = n.od8_set(s.adapter, &AmdOc::build_od8_set(count, &mode), &mut current);
        if rm != 0 {
            errors.push(lf!("The driver refused the manual tuning mode: {0}", AmdOc::status_text(rm)));
            return Ok(());
        }
    }
    let mut current = vec![0u8; (4 + count * 4) as usize];
    let rc = n.od8_set(s.adapter, &AmdOc::build_od8_set(count, &items), &mut current);
    if rc != 0 {
        errors.push(lf!("The driver refused the values: {0}", AmdOc::status_text(rc)));
    }
    Ok(())
}

fn write_odn(st: &mut State, s: &AmdState, core: i32, mem: i32, power: i32, errors: &mut Vec<String>) -> Result<(), String> {
    let n = native_in(st);
    if s.core.editable && core != s.core.current {
        st.changed = true;
        let mut b = AmdOc::odn_write(s.sys_levels.as_deref().unwrap_or(&[]), s.x2, s.sys_count, core.wrapping_mul(s.core_scale))?;
        let rc = n.odn_clocks(s.adapter, false, true, s.x2, &mut b);
        if rc != 0 {
            errors.push(lf!("Core clock: {0}", AmdOc::status_text(rc)));
        }
    }
    if s.mem.editable && mem != s.mem.current {
        st.changed = true;
        let mut b = AmdOc::odn_write(s.mem_levels.as_deref().unwrap_or(&[]), s.x2, s.mem_count, mem.wrapping_mul(s.mem_scale))?;
        let rc = n.odn_clocks(s.adapter, true, true, s.x2, &mut b);
        if rc != 0 {
            errors.push(lf!("Memory clock: {0}", AmdOc::status_text(rc)));
        }
    }
    if s.power.editable && power != s.power.current {
        if let Some(raw) = &s.power_raw {
            st.changed = true;
            let mut p = raw.clone();
            wr(&mut p, 0, AmdOc::MODE_MANUAL)?;
            wr(&mut p, 4, power)?;
            let rc = n.odn_power(s.adapter, true, &mut p);
            if rc != 0 {
                errors.push(lf!("Power limit: {0}", AmdOc::status_text(rc)));
            }
        }
    }
    Ok(())
}

/// `GpuOc.DetectHook` (test hook: replaces the detection).
pub static DETECT_HOOK: Hook<dyn Fn() -> Option<String> + Send + Sync> = Hook::new();

/// Which maker's overclocking is in use: NVIDIA first (as before), AMD when no NVIDIA card answers.
pub struct GpuOc;

impl GpuOc {
    pub const NVIDIA: &'static str = "nvidia";
    pub const AMD: &'static str = "amd";

    /// "nvidia", "amd" or "" (no card whose driver offers overclocking).
    pub fn detect() -> String {
        if let Some(h) = DETECT_HOOK.get() {
            return h().unwrap_or_default();
        }
        if NvOc::open() {
            return Self::NVIDIA.into();
        }
        if AmdOc::open() {
            return Self::AMD.into();
        }
        String::new()
    }

    pub fn error(vendor: &str) -> String {
        if vendor == Self::AMD {
            AmdOc::error()
        } else {
            NvOc::error()
        }
    }
}
