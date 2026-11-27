//! DevicesPage.cs, GpuPage.cs, DriversPage.cs and PowerPage.cs.

use super::{b, did, hex, i, s, task};
use crate::base::app_log::{AppLog, Sev};
use crate::base::ci;
use crate::base::fmt::{int, Culture};
use crate::base::flags::Flags;
use crate::base::hooks::lock;
use crate::base::l::L;
use crate::base::model::{IrqConfig, PciDevice};
use crate::base::settings::Settings;
use crate::base::time::DateTime;
use crate::etw::rx::Rx;
use crate::etw::{AutoDevice, AutoKind, IrqAuto};
use crate::ffi::hardware::{self, Facts};
use crate::ffi::{app, hub, win};
use crate::gpu::{AmdOc, AmdState, GpuOc, GpuOcLogic, GpuProfile, GpuProfiles, GpuRange, NvClock, NvOc};
use crate::lf;
use crate::native::sys_exe::SysExe;
use crate::system::Devices;
use crate::topo::{GameMask, IrqRole, Masks, TopoBuilder};
use serde_json::{json, Value};
use std::sync::Mutex;

/// IrqAuto.Apply's result as the page uses it: an error (C# exception) is logged by BusyTask and gives no lines.
pub fn apply_lines(r: Result<Vec<(String, bool)>, String>) -> Vec<(String, bool)> {
    match r {
        Ok(l) => l,
        Err(e) => {
            AppLog::add_sev(lf!("Error: {0}", e), Sev::Error);
            Vec::new()
        }
    }
}

fn cut(name: &str, max: usize) -> String {
    let u: Vec<u16> = name.encode_utf16().collect();
    if u.len() > max {
        String::from_utf16_lossy(&u[..max - 1]) + "…"
    } else {
        name.to_string()
    }
}

// ---- devices (DevicesPage.cs) ------------------------------------------------------------------------------------

struct DevPage {
    plan: Vec<AutoDevice>,
    rows: Vec<(PciDevice, IrqConfig)>,
    pins: Vec<(String, u64)>,
}

static DEV: Mutex<DevPage> = Mutex::new(DevPage { plan: Vec::new(), rows: Vec::new(), pins: Vec::new() });

fn role_text(r: IrqRole) -> &'static str {
    match r {
        IrqRole::Mouse => L::t("Mouse"),
        IrqRole::Controller => L::t("Game controller"),
        IrqRole::Gpu => L::t("Graphics card"),
        IrqRole::Net => L::t("Network"),
        IrqRole::Keyboard => L::t("Keyboard"),
    }
}

fn kind_text(k: AutoKind) -> &'static str {
    match k {
        AutoKind::Gpu => L::t("Graphics"),
        AutoKind::Audio => L::t("Audio"),
        AutoKind::Net => L::t("Network"),
        _ => L::t("USB"),
    }
}

/// The core a device of the plan gets, as written in the registry (0 = Windows decides).
fn want(d: &AutoDevice) -> u64 {
    d.slot.as_ref().map(|s| s.mask).unwrap_or(0)
}

/// One line of the confirmation: "Mouse · USB controller …: core 2".
fn plan_line(d: &AutoDevice) -> String {
    let role = match &d.slot {
        Some(s) => role_text(s.role),
        None => kind_text(d.kind),
    };
    let wh = match &d.slot {
        Some(s) if s.mask != 0 => lf!("processor {0}", Masks::ranges(s.mask)),
        _ if d.kind == AutoKind::Net && d.wifi => L::t("not touched").to_string(),
        _ => L::t("Windows decides").to_string(),
    };
    let name = cut(&d.name, 48);
    format!("{}{}:  {}", if role.is_empty() { String::new() } else { format!("{} · ", role) }, name, wh)
}

fn devices_plan() -> u64 {
    task(|| {
        let plan = IrqAuto::plan_now();
        let now: Vec<(Option<u64>, u64)> = plan.iter().map(|d| (Devices::read_irq(&d.id).assign, want(d))).collect();
        let has_backup = IrqAuto::has_backup();
        let in_place = has_backup && IrqAuto::layout_in_place(&now);
        let own = plan.iter().filter(|d| d.slot.as_ref().map(|s| s.mask != 0).unwrap_or(false)).count();
        let lines: Vec<String> = plan.iter().map(plan_line).collect();
        let confirm = format!(
            "{}\n\n{}\n\n{}",
            L::t("These devices get these processors:"),
            lines.join("\n"),
            L::t("Works after a restart of Windows. Every original value is backed up; if Windows does not start, everything is undone by itself.")
        );
        let empty = plan.is_empty();
        lock(&DEV).plan = plan;
        json!({
            "error": Value::Null, "empty": empty, "in_place": in_place,
            "in_place_text": format!("✓  {}", lf!("The layout is in place: {0} devices have cores of their own.", own as i32)),
            "flag_open": Flags::open("devices-write"), "has_backup": has_backup,
            "confirm_title": L::t("Apply the layout"), "confirm_text": confirm, "lines": lines,
        })
    })
}

fn devices_apply_plan() -> u64 {
    let plan = lock(&DEV).plan.clone();
    task(move || {
        let lines = apply_lines(IrqAuto::apply(&plan));
        GameMask::invalidate_irq();
        for (m, ok) in &lines {
            did(m, if *ok { Sev::Info } else { Sev::Error });
        }
        did(L::t("Restart Windows: the new interrupt cores start working after the restart"), Sev::Info);
        json!({})
    })
}

fn devices_restore(from: String) -> u64 {
    task(move || {
        let err = Devices::revert_all();
        GameMask::invalidate_irq();
        match err {
            None => did(if from == "device" { L::t("Device policies restored from the backup") } else { L::t("Device policies restored from the backup; restart Windows") }, Sev::Info),
            Some(e) => did(lf!("Devices: {0}", e), Sev::Error),
        }
        json!({})
    })
}

fn irq_prio_text(p: Option<i32>) -> &'static str {
    match p {
        Some(1) => L::t("low"),
        Some(2) => L::t("normal"),
        Some(3) => L::t("high"),
        _ => "",
    }
}

fn devices_list() -> u64 {
    task(|| {
        let mut list: Vec<(PciDevice, IrqConfig)> = Devices::list().into_iter().map(|d| {
            let c = Devices::read_irq(&d.pdo);
            (d, c)
        }).collect();
        list.sort_by(|a, b| ci::upper(&a.0.name).cmp(&ci::upper(&b.0.name)));
        let pins: Vec<(String, u64)> = list.iter().filter(|r| r.1.assign.unwrap_or(0) != 0).map(|r| (r.0.name.clone(), r.1.assign.unwrap_or(0))).collect();
        let rows: Vec<Value> = list
            .iter()
            .map(|(d, c)| {
                let a = c.assign.unwrap_or(0);
                json!({"name": d.name, "msi": c.msi, "pinned": if a != 0 { Masks::ranges(a) } else { String::new() }, "prio": irq_prio_text(c.priority), "assign": hex(a)})
            })
            .collect();
        let mut g = lock(&DEV);
        g.rows = list;
        g.pins = pins;
        json!({"rows": rows})
    })
}

fn fmt_g(t: DateTime) -> String {
    t.to_local().format("g", &Culture::current())
}

fn device_facts(index: usize) -> u64 {
    let (row, pins) = {
        let g = lock(&DEV);
        (g.rows.get(index).cloned(), g.pins.clone())
    };
    let Some((dev, cfg)) = row else { return 0 };
    task(move || {
        let f = Devices::facts(&dev, Some(&pins));
        let topo = TopoBuilder::live();
        let others = f.others.iter().fold(0u64, |a, p| a | p.1);
        let game = app::hub().map(|h| h.auto.game_pid() != 0).unwrap_or(false);
        let (write, note) = if !Flags::open("devices-write") {
            (1, L::t("Writing device policies is switched off (feature flag devices-write)."))
        } else if game {
            (2, L::t("Not while a game runs: the change restarts the device. Close the game and select the device again."))
        } else if Devices::is_storage(Some(&dev)) {
            (3, L::t("Storage controllers are never changed: a wrong value there can stop Windows from starting"))
        } else if !cfg.msi {
            (4, L::t("Skipped: the device does not use MSI, a pin would not work reliably."))
        } else {
            (0, "")
        };
        let mut lp_options = vec![json!({"key": "", "label": L::t("no pin")})];
        if let Some(t) = &topo {
            let mut sets: Vec<_> = t.sets.iter().filter(|s| s.group == 0).collect();
            sets.sort_by_key(|s| s.lp);
            for s in sets {
                lp_options.push(json!({"key": s.lp.to_string(), "label": lf!("Processor {0}", s.lp as i32)}));
            }
        }
        json!({
            "index": index, "name": dev.name, "driver": f.driver_file, "location": dev.location,
            "boot": if f.boot == DateTime::MIN { String::new() } else { fmt_g(f.boot) },
            "restarted": if f.restarted == DateTime::MIN { L::t("never").to_string() } else { fmt_g(f.restarted) },
            "resources": f.resources.iter().map(|r| lf!("Vector {0} · affinity {1} · group {2} · {3} · {4}", r.vector, hex(r.affinity), r.group as i32, if r.message { "MSI" } else { L::t("line") }, format!("{:?}", r.kind))).collect::<Vec<_>>(),
            "mine": hex(cfg.assign.unwrap_or(0)), "others": hex(others), "topo_ok": topo.is_some(),
            "write": write, "write_note": note, "lp_options": lp_options,
            "prio_options": [{"key": "", "label": L::t("Priority: keep")}, {"key": "1", "label": L::t("low")}, {"key": "2", "label": L::t("normal")}, {"key": "3", "label": L::t("high")}],
        })
    })
}

fn device_write(a: &Value) -> u64 {
    let index = i(a, "index") as usize;
    let Some((dev, cfg)) = lock(&DEV).rows.get(index).cloned() else { return 0 };
    let mut w = IrqConfig { msi: cfg.msi, msg_limit: cfg.msg_limit, priority: cfg.priority, assign: None };
    if let Ok(lp) = s(a, "lp").trim().parse::<i32>() {
        w.assign = Some(1u64.wrapping_shl(lp as u32));
    }
    if let Ok(p) = s(a, "prio").trim().parse::<i32>() {
        w.priority = Some(p);
    }
    task(move || {
        let err = Devices::write(Some(&dev), &w);
        GameMask::invalidate_irq();
        match err {
            None => did(lf!("{0}: policy written, read back, device restarted", &dev.name), Sev::Info),
            Some(e) => did(lf!("{0}: {1}", &dev.name, e), Sev::Error),
        }
        json!({})
    })
}

// ---- gpu (GpuPage.cs) ------------------------------------------------------------------------------------------

struct GpuPage {
    loaded: bool,
    vendor: String,
    amd: Option<AmdState>,
    clocks: Option<Vec<NvClock>>,
    has_volt: bool,
    core: i32,
    mem: i32,
    volt: i32,
    power: i32,
    volt_mv: i32,
    core_range: GpuRange,
    mem_range: GpuRange,
    power_range: GpuRange,
    volt_mv_range: GpuRange,
    store: GpuProfiles,
    selected: String,
    facts: Option<Facts>,
}

static GPU: Mutex<Option<GpuPage>> = Mutex::new(None);

fn signed(v: i32) -> String {
    int(v as i64, "+0;-0;0", &Culture::invariant())
}

fn mhz(v: i32) -> String {
    format!("{} MHz", signed(v))
}

impl GpuPage {
    fn new() -> GpuPage {
        GpuPage {
            loaded: false,
            vendor: String::new(),
            amd: None,
            clocks: None,
            has_volt: false,
            core: 0,
            mem: 0,
            volt: 0,
            power: 0,
            volt_mv: 0,
            core_range: GpuOcLogic::CORE_FALLBACK,
            mem_range: GpuOcLogic::MEM_FALLBACK,
            power_range: GpuRange::new(0, 0, 1),
            volt_mv_range: GpuRange::new(0, 0, 5),
            store: GpuProfiles::new(),
            selected: String::new(),
            facts: None,
        }
    }
    fn amd(&self) -> bool {
        self.vendor == GpuOc::AMD
    }
    fn oc_error(&self) -> String {
        GpuOc::error(&self.vendor)
    }
    fn clock(&self, domain: i32) -> Option<&NvClock> {
        self.clocks.as_ref().and_then(|c| c.iter().find(|x| x.domain == domain))
    }
    /// Saves the profile file; false (with a log line) when it cannot be written.
    fn save_store(&self) -> bool {
        match self.store.save(None) {
            Ok(()) => true,
            Err(e) => {
                did(lf!("GPU profiles were not saved: {0}", e), Sev::Warn);
                false
            }
        }
    }
    /// Fills the sliders with a profile, cut to what this card allows. True when a value had to be cut.
    fn load(&mut self, p: &GpuProfile) -> bool {
        self.core = self.core_range.snap(p.core_mhz as f64);
        self.mem = self.mem_range.snap(p.mem_mhz as f64);
        self.volt = if self.has_volt { GpuOcLogic::VOLT.snap(p.volt_pct as f64) } else { 0 };
        let amd = self.amd();
        self.power = if amd { self.power_range.snap(p.power_pct as f64) } else { 0 };
        self.volt_mv = if amd { self.volt_mv_range.snap(p.volt_mv as f64) } else { 0 };
        let cut = self.core != p.core_mhz || self.mem != p.mem_mhz;
        // Values this maker's card does not have count as cut: NVIDIA has no power limit here, AMD no voltage boost.
        if amd {
            cut || self.power != p.power_pct || self.volt_mv != p.volt_mv || p.volt_pct != 0
        } else {
            cut || (self.has_volt && self.volt != p.volt_pct) || p.power_pct != 0 || p.volt_mv != 0
        }
    }
}

/// Whose overclocking the page offers: NVIDIA when an NVIDIA card with its driver is there, else AMD.
fn vendor_of(f: Option<&Facts>) -> String {
    let Some(f) = f else { return String::new() };
    if f.gpus.iter().any(|g| g.vendor == "nvidia" && !g.basic) {
        return GpuOc::NVIDIA.into();
    }
    if f.gpus.iter().any(|g| g.vendor == "amd" && !g.basic) {
        return GpuOc::AMD.into();
    }
    String::new()
}

/// Shown(): the hardware, the profiles and the driver's values, read again.
fn gpu_read_now() -> Value {
    let facts = hardware::detect();
    let vendor = vendor_of(Some(&facts));
    let store = hub::gpu_profiles_load();
    let (mut clocks, mut volt, mut amd) = (None, None, None);
    if Flags::open("gpu-oc") && vendor == GpuOc::NVIDIA {
        clocks = NvOc::read();
        volt = NvOc::get_voltage();
    } else if Flags::open("gpu-oc") && vendor == GpuOc::AMD {
        amd = AmdOc::read();
        clocks = amd.as_ref().map(AmdOc::clocks);
    }
    let mut g = lock(&GPU);
    let p = g.get_or_insert_with(GpuPage::new);
    p.store = store;
    p.vendor = vendor;
    p.amd = amd;
    p.facts = Some(facts);
    p.clocks = clocks;
    p.core_range = GpuOcLogic::core_range(p.clock(0));
    p.mem_range = GpuOcLogic::mem_range(p.clock(4));
    // The sliders start at what the driver reports now (read back, not what was asked for).
    p.core = p.clock(0).map(|c| c.delta_khz).unwrap_or(0) / 1000;
    p.mem = p.clock(4).map(|c| c.delta_khz).unwrap_or(0) / 1000;
    p.volt = volt.unwrap_or(0);
    p.has_volt = volt.is_some();
    // AMD: the power limit and the voltage as offsets from the driver's stock values.
    p.power_range = GpuOcLogic::power_range(p.amd.as_ref().map(|a| &a.power));
    p.volt_mv_range = GpuOcLogic::volt_mv_range(p.amd.as_ref().map(|a| &a.volt));
    p.power = p.amd.as_ref().filter(|a| a.power.editable).map(|a| a.power.offset()).unwrap_or(0);
    p.volt_mv = p.amd.as_ref().filter(|a| a.volt.editable).map(|a| a.volt.offset().min(0)).unwrap_or(0);
    p.loaded = true;
    // SliderRow snaps the values to the ranges when it is built.
    p.core = p.core_range.snap(p.core as f64);
    p.mem = p.mem_range.snap(p.mem as f64);
    p.volt = GpuOcLogic::VOLT.snap(p.volt as f64);
    p.power = p.power_range.snap(p.power as f64);
    p.volt_mv = p.volt_mv_range.snap(p.volt_mv as f64);
    gpu_view(p)
}

fn tile(caption: &str, value: String, unit: Option<&str>, sub: String, temp: bool) -> Value {
    json!({"caption": caption, "value": value, "unit": unit, "sub": sub, "temp": temp})
}

/// A slider of SliderRow: the value text, the span, the scale ends and the zero mark.
fn slider(key: &str, label: &str, unit: &str, r: GpuRange, value: i32, hint: String) -> Value {
    let f = if r.max > r.min { ((0.0 - r.min as f64) / (r.max - r.min) as f64).clamp(0.0, 1.0) } else { 0.0 };
    let pct = unit == "%" || unit == "±%";
    json!({"slider": {
        "key": key, "label": label, "unit": if pct { "%" } else if unit == "mV" { "mV" } else { L::t("MHz") },
        "min": r.min, "max": r.max, "step": r.step, "value": value, "value_text": value_text(unit, value),
        "span": format!("{}…{}", signed(r.min), signed(r.max)), "hint": hint,
        "lo": signed(r.min), "hi": format!("{}{}", signed(r.max), if pct { " %" } else if unit == "mV" { " mV" } else { "" }),
        "zero": r.min < 0 && r.max > 0 && f > 0.08 && f < 0.92, "zero_at": f,
    }})
}

fn value_text(unit: &str, v: i32) -> String {
    // "%": 0–100 (NVIDIA voltage boost); "±%": a signed percent offset (AMD power limit); "mV": AMD voltage offset.
    if unit == "%" { v.to_string() } else { signed(v) }
}

fn gpu_view(p: &GpuPage) -> Value {
    let flag = Flags::open("gpu-oc");
    let amd = p.amd();
    let err = p.oc_error();
    let gpus: Vec<Value> = p
        .facts
        .as_ref()
        .map(|f| {
            f.gpus
                .iter()
                .map(|g| {
                    json!({"name": g.name, "driver": if g.basic { L::t("the real driver is missing").to_string() } else { format!("{} · {}", g.version, g.date) },
                        "nv_link": g.vendor == "nvidia", "maker_url": hardware::gpu_link(&g.vendor)})
                })
                .collect()
        })
        .unwrap_or_default();
    let no_gpu = p.facts.as_ref().map(|f| f.gpus.is_empty()).unwrap_or(false);
    // tiles
    let mut tiles = Vec::new();
    if let (Some(_), true) = (&p.clocks, flag) {
        if let Some(c) = p.clock(0) {
            tiles.push(tile(L::t("Core"), int(((c.base_khz + c.delta_khz) / 1000) as i64, "N0", &Culture::current()), Some(L::t("MHz")), lf!("stock {0} · offset {1}", c.base_khz / 1000, signed(c.delta_khz / 1000)), false));
        }
        if let Some(c) = p.clock(4) {
            tiles.push(tile(L::t("Memory"), int(((c.base_khz + c.delta_khz) / 1000) as i64, "N0", &Culture::current()), Some(L::t("MHz")), lf!("stock {0} · offset {1}", c.base_khz / 1000, signed(c.delta_khz / 1000)), false));
        }
        tiles.push(tile(L::t("Temperature"), "—".into(), None, L::t("read every second").into(), true));
        if amd {
            let pw = p.amd.as_ref().map(|a| a.power.editable).unwrap_or(false);
            let off = p.amd.as_ref().map(|a| a.power.offset()).unwrap_or(0);
            tiles.push(tile(L::t("Power limit"), if pw { signed(off) } else { "—".into() }, if pw { Some("%") } else { None }, if pw { L::t("offset from stock") } else { L::t("not offered") }.into(), false));
        } else {
            tiles.push(tile(L::t("Voltage boost"), if p.has_volt { p.volt.to_string() } else { "—".into() }, if p.has_volt { Some("%") } else { None }, if p.has_volt { L::t("of what the maker allows") } else { L::t("not offered") }.into(), false));
        }
    }
    // the overclocking panel
    let (oc, oc_text) = if !flag {
        (0, L::t("Closed. Open the feature flag \"GPU overclocking\" on the Settings page if you know what you are doing: wrong values make the card unstable, and extra voltage shortens its life.").to_string())
    } else if p.vendor.is_empty() {
        (1, L::t("Available for NVIDIA cards with the NVIDIA driver and for AMD Radeon cards with the AMD driver installed. Intel cards are not supported yet.").to_string())
    } else if p.clocks.is_none() {
        // Nothing is offered and nothing is written: the driver's own answer says why (no ADL, no overdrive, odd table…).
        (2, if !err.is_empty() { err.clone() } else { L::t("The driver did not report its clocks.").to_string() })
    } else {
        (3, String::new())
    };
    let mut items = Vec::new();
    let (mut skipped, mut amd_note, mut now_text) = (String::new(), String::new(), String::new());
    if oc == 3 {
        let c0 = p.clock(0).cloned();
        let c4 = p.clock(4).cloned();
        if !p.store.skipped.is_empty() {
            skipped = lf!("Profile \"{0}\" was NOT applied at start ({1}): Windows crashed after it was set (a blue screen or a sudden power-off). Lower the values, apply them by hand and mark the profile for the start again.", &p.store.skipped, fmt_g(p.store.skipped_utc));
        }
        if let (true, Some(a)) = (amd, &p.amd) {
            amd_note = lf!("AMD Radeon ({0}): the offsets count from the card's stock maximum clocks that the driver reports; on older cards only the top performance state is raised, the lower states and the voltages stay as they are.", a.kind());
        }
        let dc = c0.as_ref().map(|c| c.delta_khz).unwrap_or(0) / 1000;
        let dm = c4.as_ref().map(|c| c.delta_khz).unwrap_or(0) / 1000;
        now_text = if let (true, Some(a)) = (amd, &p.amd) {
            lf!("In force now (read from the driver): core {0} · memory {1} · power limit {2} · voltage offset {3}", mhz(dc), mhz(dm),
                if a.power.editable { format!("{} %", signed(a.power.offset())) } else { "—".into() },
                if a.volt.editable { format!("{} mV", signed(a.volt.offset())) } else { "—".into() })
        } else {
            lf!("In force now (read from the driver): core {0} · memory {1} · voltage boost {2}", mhz(dc), mhz(dm), if p.has_volt { format!("{} %", p.volt) } else { "—".into() })
        };
        match c0.as_ref().filter(|c| c.editable) {
            Some(c) => items.push(slider("core", L::t("Core clock offset"), "MHz", p.core_range, p.core, lf!("stock {0} MHz · allowed {1}…{2} MHz · step {3} MHz", c.base_khz / 1000, p.core_range.min, p.core_range.max, p.core_range.step))),
            None => items.push(json!({"note": L::t("The driver does not allow changing the core clock on this card.")})),
        }
        match c4.as_ref().filter(|c| c.editable) {
            Some(c) => items.push(slider("mem", L::t("Memory clock offset"), "MHz", p.mem_range, p.mem, lf!("stock {0} MHz · allowed {1}…{2} MHz · step {3} MHz", c.base_khz / 1000, p.mem_range.min, p.mem_range.max, p.mem_range.step))),
            None => items.push(json!({"note": L::t("The driver does not allow changing the memory clock on this card.")})),
        }
        if let (true, Some(a)) = (amd, &p.amd) {
            if a.power.editable {
                items.push(slider("power", L::t("Power limit"), "±%", p.power_range, p.power, lf!("offset from the stock power limit · allowed {0}…{1} % (reported by the driver)", signed(p.power_range.min), signed(p.power_range.max))));
            } else {
                items.push(json!({"note": L::t("This card or driver does not offer a power limit.")}));
            }
            if a.volt.editable && p.volt_mv_range.min < 0 {
                items.push(slider("volt_mv", L::t("Voltage offset"), "mV", p.volt_mv_range, p.volt_mv, lf!("only lowering (undervolt) · allowed {0}…0 mV · step {1} mV", p.volt_mv_range.min, p.volt_mv_range.step)));
            } else {
                items.push(json!({"note": L::t("This card or driver does not offer a voltage offset.")}));
            }
        } else if p.has_volt {
            items.push(slider("volt", L::t("Voltage boost"), "%", GpuOcLogic::VOLT, p.volt, L::t("0–100% of the extra voltage the card's maker allows").to_string()));
        } else {
            items.push(json!({"note": L::t("This card or driver does not offer a voltage boost.")}));
        }
    }
    // profiles
    let sel = p.store.find(&p.selected).cloned();
    let start = p.store.start_profile().map(|x| x.name.clone());
    let profiles: Vec<Value> = p
        .store
        .profiles
        .iter()
        .map(|x| {
            let is_amd = x.maker() == GpuOc::AMD;
            json!({
                "name": x.name, "core": signed(x.core_mhz), "mem": signed(x.mem_mhz),
                // The last column: the voltage boost of an NVIDIA profile, the power limit offset of an AMD one.
                "last": if is_amd { signed(x.power_pct) } else { x.volt_pct.to_string() },
                "at_start": start.as_deref() == Some(x.name.as_str()),
                "tip": if is_amd {
                    lf!("AMD · core {0} · memory {1} · power limit {2} % · voltage offset {3} mV", mhz(x.core_mhz), mhz(x.mem_mhz), signed(x.power_pct), signed(x.volt_mv))
                } else {
                    lf!("core {0} · memory {1} · voltage boost {2} %", mhz(x.core_mhz), mhz(x.mem_mhz), x.volt_pct)
                },
                "selected": sel.as_ref().map(|s| s.name == x.name).unwrap_or(false),
            })
        })
        .collect();
    let actions = sel.as_ref().map(|x| {
        let at_start = start.as_deref() == Some(x.name.as_str());
        let (note, warn) = if !p.vendor.is_empty() && x.maker() != p.vendor {
            (L::t("This profile was saved for a card of another maker: it is never applied at start on this card. Loading it into the sliders keeps only what this card allows.").to_string(), false)
        } else if let Some(c) = GpuOcLogic::crashed_before(Some(x), Some(&p.store.crashes)) {
            (lf!("Windows crashed on {0} with these values or lower (core {1:+0;-0;0} MHz, memory {2:+0;-0;0} MHz). Lower at least one value before you apply this profile at start.", fmt_g(c.utc), c.core_mhz, c.mem_mhz), true)
        } else {
            (String::new(), false)
        };
        json!({"at_start": at_start, "at_start_label": if at_start { L::t("Do not apply at start") } else { L::t("Apply at start") }, "note": note, "note_warn": warn})
    });
    json!({
        "loaded": p.loaded, "flag_open": flag, "vendor": p.vendor, "amd": amd, "error": err,
        "gpus": gpus, "no_gpu": no_gpu, "oc": oc, "oc_text": oc_text, "tiles": tiles,
        "skipped": skipped, "amd_note": amd_note, "now_text": now_text, "items": items,
        "limits": if amd { L::t("Fan control and memory timings are not offered here.") } else { L::t("Power limit and fan control are not offered here.") },
        "risk": format!("▲  {}", L::t("Extra voltage raises temperature and wear. Neither this program nor its authors answer for damage: use it at your own risk.")),
        "stay": if amd {
            L::t("The values stay in the driver when the program closes. Whether the AMD driver keeps them after a restart of Windows depends on the driver; after a crash it usually returns the card to stock by itself. \"Reset to stock\" always writes the driver's own stock values.")
        } else {
            L::t("The values stay in the driver when the program closes. A restart of Windows or of the driver returns the card to stock until they are applied again.")
        },
        "profiles": profiles, "last_header": if amd { "P" } else { "U" },
        "selected": sel.as_ref().map(|x| x.name.clone()).unwrap_or_default(),
        "actions": actions,
    })
}

fn gpu_get() -> Value {
    let mut g = lock(&GPU);
    gpu_view(g.get_or_insert_with(GpuPage::new))
}

fn gpu_temp() -> Value {
    let g = lock(&GPU);
    let Some(p) = g.as_ref() else { return json!({"value": 0}) };
    if !p.loaded || !Flags::open("gpu-oc") {
        return json!({"value": 0});
    }
    let amd = p.amd();
    drop(g);
    json!({"value": if amd { AmdOc::temperature() } else { NvOc::temperature() }})
}

fn gpu_set(a: &Value) -> Value {
    let mut g = lock(&GPU);
    let p = g.get_or_insert_with(GpuPage::new);
    let v = a.get("value").and_then(|v| v.as_f64()).unwrap_or(0.0);
    let key = s(a, "key");
    let (r, unit) = match key.as_str() {
        "core" => (p.core_range, "MHz"),
        "mem" => (p.mem_range, "MHz"),
        "power" => (p.power_range, "±%"),
        "volt_mv" => (p.volt_mv_range, "mV"),
        "volt" => (GpuOcLogic::VOLT, "%"),
        _ => return json!({"error": "unknown slider"}),
    };
    let x = r.snap(v);
    match key.as_str() {
        "core" => p.core = x,
        "mem" => p.mem = x,
        "power" => p.power = x,
        "volt_mv" => p.volt_mv = x,
        _ => p.volt = x,
    }
    json!({"value": x, "value_text": value_text(unit, x)})
}

/// Writes the values, reads them back, logs what is in force and reads the page again.
fn gpu_apply(profile: Option<String>) -> u64 {
    let (core, mem, volt, power, volt_mv, amd, vendor) = {
        let mut g = lock(&GPU);
        let p = g.get_or_insert_with(GpuPage::new);
        if let Some(name) = &profile {
            if let Some(pr) = p.store.find(name).cloned() {
                p.load(&pr);
            }
        }
        let amd = p.amd();
        (p.core, p.mem, p.volt, if amd { p.power } else { 0 }, if amd { p.volt_mv } else { 0 }, amd, p.vendor.clone())
    };
    task(move || {
        let log = hub::gpu_oc_apply(core * 1000, mem * 1000, if amd { 0 } else { volt }, true, power, volt_mv);
        if let Some(n) = &profile {
            did(lf!("GPU profile \"{0}\" applied", n), Sev::Info);
        }
        let err_now = GpuOc::error(&vendor);
        for item in log {
            did(&item, if !err_now.is_empty() && item.contains(&err_now) { Sev::Warn } else { Sev::Info });
        }
        gpu_read_now()
    })
}

fn gpu_reset() -> u64 {
    let (amd, vendor) = lock(&GPU).as_ref().map(|p| (p.amd(), p.vendor.clone())).unwrap_or((false, String::new()));
    task(move || {
        let _ = Settings::set("GpuOcAppliedUtc", "");
        let ok = if amd { AmdOc::reset(true) } else { NvOc::reset(true) };
        let err = GpuOc::error(&vendor);
        did(
            if ok {
                if amd { L::t("GPU overclock: clocks, power limit and voltage are back to the driver's stock values").to_string() } else { L::t("GPU overclock: clocks and voltage are back to stock").to_string() }
            } else {
                lf!("GPU overclock: {0}", err)
            },
            if ok { Sev::Info } else { Sev::Warn },
        );
        {
            let mut g = lock(&GPU);
            let p = g.get_or_insert_with(GpuPage::new);
            if !p.store.at_start.is_empty() {
                p.store.set_at_start(None);
                if p.save_store() {
                    did(L::t("GPU overclock: apply at start off"), Sev::Info);
                }
            }
        }
        gpu_read_now()
    })
}

fn gpu_do(name: &str, a: &Value) -> Option<Value> {
    let mut g = lock(&GPU);
    let p = g.get_or_insert_with(GpuPage::new);
    Some(match name {
        "gpu_set" => {
            drop(g);
            return Some(gpu_set(a));
        }
        "gpu_select" => {
            let n = s(a, "name");
            p.selected = if ci::eq(&p.selected, &n) && p.store.find(&n).is_some() { String::new() } else { n };
            json!({"ok": true})
        }
        "gpu_skipped_ok" => {
            p.store.skipped = String::new();
            p.save_store();
            json!({"ok": true})
        }
        "gpu_profile_save" => {
            let Some(n) = GpuProfiles::clean_name(Some(&s(a, "name"))) else {
                did(lf!("A profile name needs 1–{0} characters", GpuProfiles::MAX_NAME as i32), Sev::Warn);
                return Some(json!({"ok": false}));
            };
            let existed = p.store.find(&n).is_some();
            let amd = p.amd();
            let (c, m, v, pw, mv, vendor) = (p.core, p.mem, if amd { 0 } else { p.volt }, if amd { p.power } else { 0 }, if amd { p.volt_mv } else { 0 }, p.vendor.clone());
            p.store.put(&n, c, m, v, pw, mv, Some(&vendor));
            if p.save_store() {
                did(if existed { lf!("GPU profile \"{0}\" overwritten", &n) } else { lf!("GPU profile \"{0}\" saved", &n) }, Sev::Info);
            }
            p.selected = n;
            json!({"ok": true})
        }
        "gpu_profile_load" => {
            let Some(pr) = p.store.find(&p.selected).cloned() else { return Some(json!({"ok": false})) };
            let cut = p.load(&pr);
            did(
                if cut { lf!("GPU profile \"{0}\" loaded; some values were cut to what this card allows", &pr.name) } else { lf!("GPU profile \"{0}\" loaded into the sliders", &pr.name) },
                Sev::Info,
            );
            json!({"ok": true})
        }
        "gpu_profile_at_start" => {
            let Some(pr) = p.store.find(&p.selected).cloned() else { return Some(json!({"ok": false})) };
            let at_start = p.store.start_profile().map(|x| x.name == pr.name).unwrap_or(false);
            // The same values (or higher) already crashed Windows: ask first, "No" is the default (1.16.0).
            let crash = if at_start { None } else { GpuOcLogic::crashed_before(Some(&pr), Some(&p.store.crashes)).cloned() };
            if let Some(c) = crash {
                if b(a, "declined") {
                    did(lf!("GPU profile \"{0}\": not marked for the start (it crashed Windows before)", &pr.name), Sev::Info);
                    return Some(json!({"ok": false}));
                }
                if !b(a, "confirmed") {
                    let q = lf!("Windows crashed on {0} with these values or lower (core {1:+0;-0;0} MHz, memory {2:+0;-0;0} MHz). Applied at every start, they can crash it again.\n\nLower at least one value first. Apply \"{3}\" at start anyway?", fmt_g(c.utc), c.core_mhz, c.mem_mhz, &pr.name);
                    return Some(json!({"ask": q, "title": L::t("Domain OS Toolbox: GPU profile")}));
                }
                AppLog::add_sev(lf!("GPU profile \"{0}\": marked for the start although Windows crashed with these values on {1}", &pr.name, fmt_g(c.utc)), Sev::Warn);
            }
            p.store.set_at_start(if at_start { None } else { Some(pr.name.as_str()) });
            if p.save_store() {
                did(if at_start { L::t("GPU overclock: apply at start off").to_string() } else { lf!("GPU profile \"{0}\": apply at start on", &pr.name) }, Sev::Info);
            }
            json!({"ok": true})
        }
        "gpu_profile_rename" => {
            let old = p.selected.clone();
            let to = s(a, "to");
            if !p.store.rename(&old, &to) {
                did(L::t("That name is empty, too long or already used by another profile"), Sev::Warn);
                return Some(json!({"ok": false}));
            }
            let new = GpuProfiles::clean_name(Some(&to)).unwrap_or(to);
            if p.save_store() {
                did(lf!("GPU profile \"{0}\" renamed to \"{1}\"", &old, &new), Sev::Info);
            }
            p.selected = new;
            json!({"ok": true})
        }
        "gpu_profile_delete" => {
            let n = p.selected.clone();
            p.store.delete(&n);
            if p.save_store() {
                did(lf!("GPU profile \"{0}\" deleted", &n), Sev::Info);
            }
            p.selected = String::new();
            json!({"ok": true})
        }
        _ => return None,
    })
}

// ---- drivers (DriversPage.cs) ------------------------------------------------------------------------------------

fn guide_path() -> std::path::PathBuf {
    std::path::Path::new(&win::program_files_dir()).join("Domain OS").join("Guide").join("New-DriverGuide.ps1")
}

fn step(name: &str, state: String, bad: bool, why: &str, how: &[&str], links: &[(&str, &str)]) -> Value {
    json!({
        "name": name,
        "state": if state.is_empty() { L::t("not found").to_string() } else { state },
        "bad": bad, "why": why,
        "how": how.iter().enumerate().map(|(n, t)| format!("{}.  {}", n + 1, t)).collect::<Vec<_>>(),
        "links": links.iter().filter(|l| !l.1.is_empty()).map(|l| json!({"text": l.0, "url": l.1})).collect::<Vec<_>>(),
    })
}

fn drivers() -> u64 {
    task(|| {
        let f = hardware::detect();
        let mut steps = Vec::new();
        let item2 = L::t("Open the official page");
        let item3 = hardware::board_link(&f.board_vendor);
        steps.push(step(L::t("Network driver"), f.net.join(", "), f.net.is_empty(), L::t("Without it nothing else can be downloaded. The driver from the maker of the board supports every function of the adapter."), &[], &[(item2, item3)]));
        steps.push(step(L::t("Chipset driver"), f.cpu.clone(), false, L::t("Install it first and restart. It gives Windows the right power and core information for this processor; on Ryzen X3D with two chip blocks it contains the component that keeps games on the big-cache cores."), &[], &[(item2, hardware::chipset_link(&f.cpu_vendor))]));
        for gpu in &f.gpus {
            let state2 = if gpu.basic { format!("{} — {}", gpu.name, L::t("the real driver is missing")) } else { format!("{} · {} · {}", gpu.name, gpu.version, gpu.date) };
            if gpu.vendor == "nvidia" {
                steps.push(step(
                    L::t("Graphics driver"),
                    state2,
                    gpu.basic,
                    L::t("NVCleanstall installs the NVIDIA driver without the extras: you lose game recording and automatic driver updates from NVIDIA, nothing else."),
                    &[
                        L::t("Download NVCleanstall from its official page (TechPowerUp) and start it."),
                        L::t("Choose \"Install best driver for my hardware\"."),
                        L::t("Keep only Display Driver. Add HD Audio via HDMI only if sound goes through the monitor, PhysX only if a game asks for it."),
                        L::t("Leave NVIDIA app / GeForce Experience and telemetry unticked."),
                        L::t("On the tweaks page tick only \"Perform a clean installation\" and \"Disable installer telemetry and advertising\"."),
                        L::t("Install and restart."),
                    ],
                    &[(L::t("NVCleanstall (official page)"), hardware::NV_CLEANSTALL), (L::t("NVIDIA drivers"), hardware::gpu_link("nvidia"))],
                ));
            } else {
                let how: Vec<&str> = if gpu.vendor == "amd" {
                    vec![
                        L::t("In the AMD installer choose \"Driver Only\" (or \"Minimal Install\" if you want the settings panel)."),
                        L::t("Tick \"Factory Reset\" for a clean installation if it is offered, then restart."),
                    ]
                } else {
                    Vec::new()
                };
                steps.push(step(L::t("Graphics driver"), state2, gpu.basic, L::t("Without the maker's driver games run on a basic adapter with no acceleration."), &how, &[(item2, hardware::gpu_link(&gpu.vendor))]));
            }
        }
        steps.push(step(L::t("Audio driver"), f.audio.join(", "), f.audio.is_empty(), L::t("The generic driver works; the maker's one adds the outputs, microphone processing and the control panel of your board. Choose your exact model on the support page."), &[], &[(item2, item3)]));
        let probs = if f.problems.is_empty() { L::t("none").to_string() } else { f.problems.iter().take(6).cloned().collect::<Vec<_>>().join(", ") };
        steps.push(step(L::t("Devices without a driver"), probs, !f.problems.is_empty(), L::t("A device with a yellow mark in Device Manager does not work and may keep the system from sleeping or saving power correctly."), &[], &[]));
        if f.refresh > 0 {
            steps.push(step(L::t("Refresh rate of the monitor"), format!("{} Hz", f.refresh), f.refresh <= 60, L::t("Windows often starts at 60 Hz on a faster monitor. Settings → System → Display → Advanced display: choose the highest rate your monitor supports."), &[], &[]));
        }
        if f.ram_rated > 0 && f.ram_now > 0 && f.ram_now < f.ram_rated - 50 {
            steps.push(step(L::t("Memory profile (XMP / EXPO)"), lf!("now {0} MHz, rated {1} MHz", f.ram_now, f.ram_rated), true, L::t("The memory runs slower than it was sold as, which lowers the minimum FPS. Turn on the XMP or EXPO profile in the BIOS."), &[], &[]));
        }
        json!({"facts": true, "steps": steps})
    })
}

fn drivers_guide_open() -> Value {
    let guide = guide_path();
    // Windows PowerShell by its full path, no profile, only Windows' own modules (SysExe).
    let mut psi = SysExe::start(&SysExe::power_shell_path(), false);
    for a in ["-NoProfile", "-ExecutionPolicy", "Bypass", "-WindowStyle", "Hidden", "-File"] {
        psi.arg(a);
    }
    psi.arg(&guide.to_string_lossy());
    if let Err(e) = psi.command().spawn() {
        did(lf!("Guide: {0}", e.to_string()), Sev::Warn);
    }
    json!({})
}

// ---- power (PowerPage.cs) ----------------------------------------------------------------------------------------

fn explain(guid: &str) -> &'static str {
    match ci::lower(guid).as_str() {
        "381b4222-f694-41f0-9685-ff5bb260df2e" => L::t("Balanced. The plan AMD and Intel tune their current processors for: idle cores sleep, which leaves more power and temperature headroom for the cores that work. Recommended unless you measured otherwise."),
        "8c5e7fda-e8bf-4a96-9a85-a6e23a8c635c" => L::t("High performance. Cores stay awake and at high frequency all the time: more power and heat when idle. It can shave a little wake-up latency on older processors; on current ones the gain is usually within noise."),
        "a1841308-3541-4fab-bc81-f71556f20b4a" => L::t("Power saver. Lowers the processor's frequency: not for games."),
        "e9a42b02-d5df-448d-aa00-03f14749eb61" => L::t("Ultimate Performance. High performance with a few more idle savings removed; made for workstations. No advantage over High performance in games."),
        _ => L::t("A plan added by the maker of the PC, by a driver or by a tool. Check its name before using it."),
    }
}

/// The plans in `powercfg /list` output.
pub fn parse_plans(output: &str) -> Vec<(String, String, bool)> {
    let rx = Rx::new(r"([0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12})\s+\((.+?)\)\s*(\*)?", false);
    rx.find_all(output).into_iter().map(|m| (m.group(1).unwrap_or_default(), m.group(2).unwrap_or_default(), m.group(3).is_some())).collect()
}

fn power_list() -> u64 {
    task(|| {
        let (_, out) = hardware::run("powercfg.exe", "/list", 15000);
        let plans = parse_plans(&out);
        if plans.is_empty() {
            return json!({"error": lf!("The power plans could not be read: {0}", out), "plans": []});
        }
        json!({"error": Value::Null, "plans": plans.iter().map(|(g, n, a)| json!({"guid": g, "name": n, "active": a, "explain": explain(g)})).collect::<Vec<_>>()})
    })
}

fn power_set(a: &Value) -> u64 {
    let (guid, name) = (s(a, "guid"), s(a, "name"));
    task(move || {
        let (code, out) = hardware::run("powercfg.exe", &format!("/setactive {}", guid), 15000);
        if code == 0 {
            did(lf!("Power plan: {0}", &name), Sev::Info);
        } else {
            did(lf!("Power plan: {0}", out), Sev::Error);
        }
        json!({})
    })
}

// ---- dispatch ------------------------------------------------------------------------------------------------------

pub fn get(name: &str, _a: &Value) -> Option<Value> {
    Some(match name {
        "gpu" => gpu_get(),
        "gpu_temp" => gpu_temp(),
        "drivers_guide" => json!({"exists": guide_path().exists()}),
        _ => return None,
    })
}

pub fn do_(name: &str, a: &Value) -> Option<Value> {
    Some(match name {
        "devices_allow" => {
            let _ = Flags::set("devices-write", true);
            json!({})
        }
        "drivers_guide_open" => drivers_guide_open(),
        n if n.starts_with("gpu_") => return gpu_do(n, a),
        _ => return None,
    })
}

pub fn start(name: &str, a: &Value) -> Option<u64> {
    Some(match name {
        "devices_plan" => devices_plan(),
        "devices_apply_plan" => devices_apply_plan(),
        "devices_restore" => devices_restore(s(a, "from")),
        "devices_list" => devices_list(),
        "device_facts" => device_facts(i(a, "index") as usize),
        "device_write" => device_write(a),
        "gpu_read" => task(gpu_read_now),
        "gpu_apply" => gpu_apply(a.get("profile").and_then(|v| v.as_str()).map(str::to_string)),
        "gpu_reset" => gpu_reset(),
        "drivers" => drivers(),
        "power_list" => power_list(),
        "power_set" => power_set(a),
        _ => return None,
    })
}
