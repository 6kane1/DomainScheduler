//! Port of `IrqAuto.cs`.
//!
//! The automatic interrupt layout on a real PC, following DEVICE-TWEAKER's AUTO mode (its behaviour re-implemented
//! from its open-source description; no code was copied; its kernel-driver features are not part of it):
//! the device list and filters, the roles of USB controllers from their HID devices, `IrqLayout` for the
//! cores, then the writes per kind (MSI where the device supports it, priority, affinity policy, the network card's
//! RSS, USB and network power saving off on desktops, the USB selective suspend of the power plan on mains power,
//! ReservedCpuSets removed).
//! Safety rules (1.12.0): storage controllers and the boot device are never touched; MSI is switched on only when the
//! device already offers it; on a PC with a battery or Modern Standby no power-saving value is changed; no battery (DC)
//! power value is ever written; `BootGuard` reverts everything when Windows does not start fine afterwards.
//! Every value is backed up before it is first changed; `revert_extras` puts all of it back.
//! It takes effect after a restart of Windows; no device is restarted.

#![cfg_attr(not(windows), allow(dead_code))]

use super::rx::Rx;
use super::stj;
use crate::base::ci;
use crate::base::hooks::Hook;
use crate::base::state_dir::store_dir;
use crate::topo::irq_layout::{IrqLayout, IrqRole, IrqSlot};
use crate::topo::model::Topo;
use indexmap::IndexMap;
use serde::Deserialize;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::OnceLock;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub enum AutoKind {
    #[default]
    Usb,
    Gpu,
    Audio,
    Net,
    Stor,
}

/// A device of the automatic layout and everything AUTO needs to know about it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AutoDevice {
    pub id: String,
    pub name: String,
    pub class: String,
    pub service: String,
    pub driver_key: String,
    pub dev_inst: u32,
    pub kind: AutoKind,
    pub igpu: bool,
    pub wifi: bool,
    pub display_audio: bool,
    pub net_cx: bool,
    pub stor_tag: String,
    /// USB: the most important input role found on the controller
    pub role: Option<IrqRole>,
    /// AUDIO: names of its sound endpoints
    pub endpoints: Vec<String>,
    /// filled by PlanNow for USB with a role, discrete GPU, wired NIC
    pub slot: Option<IrqSlot>,
    /// NDIS: IRQ, RSS or BOTH
    pub nic_mode: String,
}

/// A compiled pattern, built once.
macro_rules! rx {
    ($name:ident, $pat:expr, $ic:expr) => {
        fn $name() -> &'static Rx {
            static R: OnceLock<Rx> = OnceLock::new();
            R.get_or_init(|| Rx::new($pat, $ic))
        }
    };
}

// ---- classification (pure, tested) -------------------------------------------------------------------------------

rx!(excluded_id, r"^ACPI\\PNP0100|^ACPI\\AMDI00(10|30)\\|^ACPI\\PNP0103|^ACPI\\PNP0501|^PCI\\VEN_1022&DEV_14DB|^PCI\\VEN_1022&DEV_14DD|^ACPI\\INTC1055\\|^PCI\\VEN_8086&DEV_51E8|^PCI\\VEN_8086&DEV_A73E", true);
rx!(virtual_net, "virtual|loopback|hyper-v|vmware|wan miniport|tap|tun|isatap|teredo|npcap", true);
rx!(soft_audio, "vb-?audio|voicemeeter|virtual cable|virtual audio|broadcast|software device", true);
rx!(system_bridge, r"PCI Express Root Port|PCI-to-PCI Bridge|PCI standard .*bridge|SMBus|LPC Controller|ISA Bridge|I2C Controller|SPI Controller|GPIO Controller|PS/2 Controller", true);
rx!(serial, r"\b(COM\d+|Serial Port|Communications Port)", true);
rx!(rst_vmd, r"Intel\s*\(R\)\s*RST VMD Controller", true);
rx!(stor_name, r"NVM\s*Express|NVMe|\bSATA\b|AHCI|\bRAID\b|SCSI|Storage|\bVMD\b|\bIDE\b|Mass Storage|Disk", true);
rx!(stor_service, r"^(stornvme|storahci|storport|stornvm\w*|iaStor\w*|iaVROC|rcraid|rcbottom|amd_sata|amdsata|amd_xata|amdxata|nvme\w*|secnvme|samsungnvme\w*|msahci|pciide|intelide|atapi|vmd|megasas\w*|lsi_\w*|arcsas|percsas\w*|stexstor|mvs\w*|disk|uaspstor|usbstor)$", true);
rx!(usb_name, "xHCI|Host Controller", true);
rx!(audio_name, "High Definition Audio|HD Audio|Audio Controller", true);
rx!(discrete, r"\bGeForce\b|\bRTX\b|\bGTX\b|\bQuadro\b|\bTesla\b|\bRadeon\s+RX\b|\bRadeon\s+Pro\b|Laptop\s+GPU\b|Intel\s*\(R\)\s*Arc\s*\(TM\)\s*A\d{3,}", true);
rx!(integrated, r"Intel\s*\(R\)\s*(UHD|HD|Iris( Xe)?)( Graphics)?\b|Intel\s*Arc\s*\(TM\)\s*Graphics\b|(AMD\s+)?Radeon\s*\(TM\)\s*Graphics\b|(AMD\s+)?Radeon\s*\(TM\)\s*\d{3,4}M\b", true);
rx!(strong_gpu, r"GeForce|RTX|GTX|Quadro|Laptop GPU|Intel\s*\(R\)\s*Arc\s*\(TM\)\s*A\d{3,}", true);
rx!(amd_any, "VEN_1002|Radeon", true);
rx!(radeon_rx_pro, r"Radeon\s+(RX|Pro)\b", true);
rx!(wifi_service, r"^(netwtw\w*|rtwl\w*|rtwlane\w*|rtwlanu\w*|athw\w*|ath\w*|bcmwl\w*|qcwlan\w*|qwlan\w*|iwl\w*|mtk\w*)$", true);
rx!(wifi_name, r"Wi-?Fi|Wireless|802\.11|WLAN", true);
rx!(display_audio_id, r"\\VEN_10DE&DEV_(1A|22|26|28)[0-9A-F]{2}|\\VEN_1002&DEV_AA[0-9A-F]{2}|\\VEN_8086&DEV_28[0-9A-F]{2}", true);
rx!(display_audio_name, "Display Audio|HDMI Audio|NVIDIA High Definition Audio|AMD High Definition Audio", true);
rx!(display_endpoint, r"hdmi|display\s*audio|monitor|монитор|displayport|\bdp\b", true);
rx!(spdif, r"s/pdif|spdif|iec958|toslink|digital audio", true);
rx!(pad_name, "game controller|gamepad", true);
rx!(audio_word, "headset|headphone|audio|mic|speaker", false);
rx!(nvidia_usb, r"VEN_10DE[&\\]", true);
rx!(cppc_group, "<Data Name=['\"]Group['\"]>(\\d+)</Data>", false);
rx!(cppc_number, "<Data Name=['\"]Number['\"]>(\\d+)</Data>", false);
rx!(cppc_perf, "<Data Name=['\"]MaximumPerformancePercent['\"]>(\\d+)</Data>", false);
rx!(netadapter_name, "netadapter", true);
rx!(netadapter_image, r"netadaptercx|netadapter\.sys", true);
// A device instance ID: ENUMERATOR\DEVICE\INSTANCE, nothing that could reach outside the device's key.
rx!(instance_rx, r"^[A-Za-z0-9_]{1,32}\\[A-Za-z0-9&_.#{}+\- ]{1,200}\\[A-Za-z0-9&_.#{}+\- ]{1,200}$", false);
rx!(driver_key_rx, r"^\{[0-9A-Fa-f]{8}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{12}\}\\\d{4}$", false);
rx!(value_name_rx, r"^[A-Za-z0-9 _\-]{1,64}$", false);
rx!(wmi_instance_rx, r"^[A-Za-z0-9&_.#{}+\\\- ]{1,400}$", false);

fn cppc_event() -> &'static Rx {
    static R: OnceLock<Rx> = OnceLock::new();
    R.get_or_init(|| Rx::with("<Event[ >].*?</Event>", false, true))
}

const PAD_WORDS: [&str; 19] = [
    "gamepad", "game pad", "game controller", "wireless controller", "xbox", "xinput", "dualshock", "dualsense", "playstation", "ps3", "ps4", "ps5",
    "8bitdo", "joycon", "joy-con", "switch pro", "pro controller", "steam controller", "nintendo",
];

const ENUM_ROOT: &str = r"SYSTEM\CurrentControlSet\Enum\";
const CLASS_ROOT: &str = r"SYSTEM\CurrentControlSet\Control\Class\";
const IM_SUB: &str = r"\Device Parameters\Interrupt Management";
const KERNEL_KEY: &str = r"SYSTEM\CurrentControlSet\Control\Session Manager\Kernel";
const RSS_NAMES: [&str; 6] = ["*NumRssQueues", "*RssBaseProcNumber", "*RssBaseProcGroup", "*MaxRssProcessors", "*RSSMaxProcGroup", "*RssMaxProcNumber"];
const RAW_MOUSE_ID: &str = r"HKCU|Control Panel\Mouse|RawMouseThrottleDuration";
const I32_MIN: i64 = i32::MIN as i64;
const I32_MAX: i64 = i32::MAX as i64;

/// `IrqAuto.CppcHook` (tests): MaximumPerformancePercent per processor without Windows.
pub static CPPC_HOOK: Hook<dyn Fn() -> HashMap<i32, i32> + Send + Sync> = Hook::new();
/// `BootGuard.MarkPending()` (system area). Unset: the marker cannot be saved, so `apply` writes nothing.
pub static MARK_PENDING_HOOK: Hook<dyn Fn() -> Result<(), String> + Send + Sync> = Hook::new();

/// `IrqAuto` (static class).
pub struct IrqAuto;

/// The backup file: "HIVE|path|name" -> "" (absent) / d:int / b:hex / s:text; "|*" = a deleted key; the WMI power
/// switches (MSPower_DeviceEnable InstanceName -> Enable); the USB suspend pair (AC, DC of the active plan, before).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Backup {
    pub reg: IndexMap<String, String>,
    pub wmi: IndexMap<String, bool>,
    pub usb_suspend: Option<Vec<u32>>,
}

/// The file as System.Text.Json reads it (missing or null members allowed).
#[derive(Deserialize)]
struct BackupIn {
    #[serde(rename = "Reg", default)]
    reg: Option<Ordered<Option<String>>>,
    #[serde(rename = "Wmi", default)]
    wmi: Option<Ordered<bool>>,
    #[serde(rename = "UsbSuspend", default)]
    usb_suspend: Option<Vec<u32>>,
}

/// A JSON object read in file order (a C# Dictionary keeps it; a repeated key keeps its first place, the last
/// value wins).
struct Ordered<V>(IndexMap<String, V>);

impl<'de, V: Deserialize<'de>> Deserialize<'de> for Ordered<V> {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct Vis<V>(std::marker::PhantomData<V>);
        impl<'de, V: Deserialize<'de>> serde::de::Visitor<'de> for Vis<V> {
            type Value = Ordered<V>;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("an object")
            }
            fn visit_map<A: serde::de::MapAccess<'de>>(self, mut m: A) -> Result<Self::Value, A::Error> {
                let mut out = IndexMap::new();
                while let Some((k, v)) = m.next_entry::<String, V>()? {
                    out.insert(k, v);
                }
                Ok(Ordered(out))
            }
        }
        d.deserialize_map(Vis(std::marker::PhantomData))
    }
}

impl Backup {
    fn is_empty(&self) -> bool {
        self.reg.is_empty() && self.wmi.is_empty() && self.usb_suspend.is_none()
    }

    /// `JsonSerializer.Serialize(backup)`.
    pub fn to_json(&self) -> String {
        let reg: Vec<String> = self.reg.iter().map(|(k, v)| format!("{}:{}", stj::string(k), stj::string(v))).collect();
        let wmi: Vec<String> = self.wmi.iter().map(|(k, v)| format!("{}:{}", stj::string(k), v)).collect();
        let usb = match &self.usb_suspend {
            Some(u) => format!("[{}]", u.iter().map(|x| x.to_string()).collect::<Vec<_>>().join(",")),
            None => "null".into(),
        };
        format!("{{\"Reg\":{{{}}},\"Wmi\":{{{}}},\"UsbSuspend\":{}}}", reg.join(","), wmi.join(","), usb)
    }

    fn parse(json: &str) -> Result<Option<BackupIn>, String> {
        serde_json::from_str::<Option<BackupIn>>(json).map_err(|e| e.to_string())
    }
}

/// .NET `Uri.IsHexDigit`.
fn is_hex(c: char) -> bool {
    c.is_ascii_hexdigit()
}

/// `long.TryParse(s, NumberStyles.AllowLeadingSign, InvariantCulture)`.
fn parse_long_signed(s: &str) -> Option<i64> {
    let digits = s.strip_prefix(['+', '-']).unwrap_or(s);
    if digits.is_empty() || !digits.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    s.parse::<i64>().ok()
}

fn utf16_len(s: &str) -> usize {
    s.encode_utf16().count()
}

fn dword(enc: &str, lo: i64, hi: i64) -> bool {
    enc.starts_with("d:") && parse_long_signed(&enc[2..]).is_some_and(|v| v >= lo && v <= hi)
}

fn bin(enc: &str, max_bytes: usize) -> bool {
    enc.starts_with("b:") && utf16_len(enc) - 2 <= max_bytes * 2 && (utf16_len(enc) - 2) % 2 == 0 && enc[2..].chars().all(is_hex)
}

fn digits(enc: &str, max_len: usize) -> bool {
    enc.starts_with("s:") && utf16_len(enc) > 2 && utf16_len(enc) - 2 <= max_len && enc[2..].chars().all(|c| c.is_ascii_digit())
}

/// `str[..^tail.Length]` / `str[n..]` for strings compared case-insensitively: cut by chars.
fn cut_end(s: &str, tail_chars: usize) -> String {
    let n = s.chars().count();
    s.chars().take(n.saturating_sub(tail_chars)).collect()
}

fn skip_chars(s: &str, n: usize) -> String {
    s.chars().skip(n).collect()
}

impl IrqAuto {
    pub const BACKUP_NAME: &'static str = "irq-auto-backup.json";

    /// `IrqAuto.InstanceRx` (also used by the manual device backup in Devices.cs).
    pub fn instance_rx_match(s: &str) -> bool {
        instance_rx().is_match(s)
    }

    /// The exclusions applied before a device is classified.
    pub fn excluded(id: &str, name: &str, cls: &str) -> bool {
        let c = |x: &str| ci::eq(cls, x);
        if excluded_id().is_match(id) {
            return true;
        }
        if c("System") && ci::contains(name, "ACPI") {
            return true;
        }
        if c("Ports") || serial().is_match(name) {
            return true;
        }
        if c("Bluetooth") || ci::contains(name, "Bluetooth") {
            return true;
        }
        if c("Net") && virtual_net().is_match(name) {
            return true;
        }
        if c("SoftwareDevice") || soft_audio().is_match(name) {
            return true;
        }
        if c("System") && system_bridge().is_match(name) {
            return true;
        }
        if rst_vmd().is_match(name) {
            return true;
        }
        if c("Display") && (ci::contains(name, "VirtualBox") || ci::contains(id, "VEN_80EE")) {
            return true;
        }
        false
    }

    /// A storage controller (NVMe, SATA/AHCI, RAID, VMD, SCSI) or a disk: never touched by the interrupt layout or a
    /// manual pin. The boot disk always hangs on one of these, so a wrong value there can stop Windows from starting.
    pub fn is_storage(cls: &str, name: &str, service: &str) -> bool {
        for c in ["SCSIAdapter", "HDC", "IDE", "Storage", "DiskDrive", "Volume", "StorageVolume"] {
            if ci::eq(cls, c) {
                return true;
            }
        }
        stor_name().is_match(name) || stor_service().is_match(service)
    }

    /// Whether MSISupported = 1 may be written (pure). Only when the device already offers MSI: it uses message
    /// interrupts now, or its PCI interrupt support (DEVPKEY_PciDevice_InterruptSupport: 2 = MSI, 4 = MSI-X) says so,
    /// or, when that property cannot be read, its driver put a MessageNumberLimit into the MSI key. Unsure: no.
    pub fn msi_allowed(interrupt_support: Option<i32>, uses_msi_now: bool, driver_limit: bool) -> bool {
        if uses_msi_now {
            return true;
        }
        if let Some(v) = interrupt_support {
            return (v & 6) != 0;
        }
        driver_limit
    }

    /// Power saving of devices and the USB selective suspend may be changed only on a desktop on mains power with no
    /// battery and no Modern Standby (pure). BatteryFlag 128 = "no system battery"; 255 (unknown) counts as a battery.
    pub fn power_saving_allowed(battery_flag: u8, batteries_present: bool, modern_standby: bool) -> bool {
        battery_flag == 128 && !batteries_present && !modern_standby
    }

    /// The kind, or None when the device is not part of the list.
    pub fn classify(cls: &str, name: &str) -> Option<AutoKind> {
        let c = |x: &str| ci::eq(cls, x);
        if c("USB") || usb_name().is_match(name) {
            return Some(AutoKind::Usb);
        }
        if c("Display") {
            return Some(AutoKind::Gpu);
        }
        if c("Net") {
            return Some(AutoKind::Net);
        }
        if c("SCSIAdapter") || c("HDC") || c("IDE") || c("Storage") {
            return Some(AutoKind::Stor);
        }
        if c("MEDIA") || c("AudioEndpoint") || audio_name().is_match(name) {
            return Some(AutoKind::Audio);
        }
        None
    }

    pub fn is_integrated_gpu(id: &str, name: &str) -> bool {
        let s = format!("{} {}", id, name);
        !discrete().is_match(&s) && integrated().is_match(&s)
    }

    /// With two or more GPUs and one of them clearly discrete, any other plain Radeon is the integrated one.
    pub fn mark_integrated(gpus: &mut [&mut AutoDevice]) {
        for g in gpus.iter_mut() {
            g.igpu = Self::is_integrated_gpu(&g.id, &g.name);
        }
        if gpus.len() < 2 {
            return;
        }
        if !gpus.iter().any(|g| strong_gpu().is_match(&g.name)) {
            return;
        }
        for g in gpus.iter_mut() {
            if !strong_gpu().is_match(&g.name) && amd_any().is_match(&format!("{} {}", g.id, g.name)) && !radeon_rx_pro().is_match(&g.name) {
                g.igpu = true;
            }
        }
    }

    pub fn is_wifi(id: &str, name: &str, service: &str) -> bool {
        wifi_name().is_match(name) || ci::contains(id, "VEN_14C3") || wifi_service().is_match(service)
    }

    pub fn is_display_audio_device(id: &str, name: &str) -> bool {
        display_audio_id().is_match(id) || display_audio_name().is_match(name)
    }

    pub fn is_display_endpoint(endpoint: &str) -> bool {
        display_endpoint().is_match(endpoint)
    }

    pub fn is_spdif(endpoint: &str) -> bool {
        spdif().is_match(endpoint) && !Self::is_display_endpoint(endpoint)
    }

    /// The role of one HID collection from its usage (page 1: 2 mouse, 6 keyboard, 4/5/8 game controller), its setup
    /// class and its product string (game controller words). None = no input role.
    pub fn hid_role(usage_page: i32, usage: i32, cls: &str, name: &str, product: &str) -> Option<IrqRole> {
        let u = if usage_page == 1 {
            match usage {
                2 => Some(IrqRole::Mouse),
                6 => Some(IrqRole::Keyboard),
                4 | 5 | 8 => Some(IrqRole::Controller),
                _ => None,
            }
        } else {
            None
        };
        let pnp = if ci::eq(cls, "Keyboard") {
            Some(IrqRole::Keyboard)
        } else if ci::eq(cls, "Mouse") {
            Some(IrqRole::Mouse)
        } else if pad_name().is_match(name) {
            Some(IrqRole::Controller)
        } else {
            None
        };
        let pr = ci::lower(product);
        let audio = audio_word().is_match(&pr);
        let prod = if !audio && PAD_WORDS.iter().any(|w| pr.contains(w)) { Some(IrqRole::Controller) } else { None };
        if prod.is_some() {
            return if u.is_none() || u == prod { prod } else { None };
        }
        if pnp == Some(IrqRole::Controller) {
            return pnp;
        }
        u.or(pnp)
    }

    // ---- planning ---------------------------------------------------------------------------------------------

    /// MaximumPerformancePercent per processor from the newest Kernel-Processor-Power event 55 of each.
    pub fn cppc() -> HashMap<i32, i32> {
        if let Some(h) = CPPC_HOOK.get() {
            return h();
        }
        win_cppc().unwrap_or_default()
    }

    /// Newest first: the first value seen for a group-0 processor wins. Err where C# `int.Parse` throws (a number
    /// too big for an int).
    pub fn parse_cppc(xml: &str) -> Result<HashMap<i32, i32>, String> {
        let mut res = HashMap::new();
        for ev in cppc_event().find_all(xml) {
            let v = ev.value();
            let grp = cppc_group().find(&v);
            let num = cppc_number().find(&v);
            let perf = cppc_perf().find(&v);
            let (Some(num), Some(perf)) = (num, perf) else { continue };
            if let Some(g) = &grp {
                if g.group(1).as_deref() != Some("0") {
                    continue;
                }
            }
            let lp: i32 = num.group(1).unwrap_or_default().parse().map_err(|e: std::num::ParseIntError| e.to_string())?;
            if !res.contains_key(&lp) {
                let p: i32 = perf.group(1).unwrap_or_default().parse().map_err(|e: std::num::ParseIntError| e.to_string())?;
                res.insert(lp, p);
            }
        }
        Ok(res)
    }

    /// Gives each device its slot (USB with a role, discrete GPU, wired NIC) and plans the cores.
    pub fn assign(devs: &mut [AutoDevice], topo: Option<&Topo>, perf: &HashMap<i32, i32>, ccx_pairs: bool) {
        for d in devs.iter_mut() {
            let r = match d.kind {
                AutoKind::Usb => d.role,
                AutoKind::Gpu if !d.igpu => Some(IrqRole::Gpu),
                AutoKind::Net if !d.wifi => Some(IrqRole::Net),
                _ => None,
            };
            d.slot = r.map(|r| IrqSlot::new(&d.id, &d.name, r));
        }
        let slots: Vec<IrqSlot> = devs.iter().filter_map(|d| d.slot.clone()).collect();
        let planned = IrqLayout::plan(topo, slots, Some(perf), ccx_pairs);
        // C# plans the slot objects in place: the results go back to their devices (instance ids are unique).
        let mut used = vec![false; planned.len()];
        for d in devs.iter_mut() {
            let Some(s) = d.slot.as_mut() else { continue };
            if let Some(i) = planned.iter().enumerate().position(|(i, p)| !used[i] && p.key == s.key && p.role == s.role) {
                used[i] = true;
                *s = planned[i].clone();
            }
        }
    }

    pub fn choose_nic_mode(capable: bool, enabled: bool, configured: bool, queues: i32, max_proc: i32) -> String {
        if capable && (enabled || configured) {
            return "BOTH".into();
        }
        if capable && (queues > 1 || max_proc > 1) {
            return "BOTH".into();
        }
        (if capable { "RSS" } else { "IRQ" }).into()
    }

    // ---- what a backup may hold: exactly what this program (or an older version of it) writes -------------------

    /// The interrupt values of a device's Affinity Policy / MSI key: the names this program writes and puts back, each
    /// in its registry type: a DWORD, or for the affinity mask a binary value of at most 64 bytes (pure; also used for
    /// the manual device backup). The number itself is not limited: a backup holds whatever value the device had
    /// before, and it has to come back as it was. "" = the value did not exist.
    pub fn valid_irq_value(name: &str, enc: &str) -> bool {
        if enc.is_empty() {
            return matches!(name, "DevicePriority" | "DevicePolicy" | "AssignmentSetOverride" | "MSISupported" | "MessageNumberLimit");
        }
        match name {
            "DevicePriority" | "DevicePolicy" | "MSISupported" | "MessageNumberLimit" => dword(enc, I32_MIN, I32_MAX),
            "AssignmentSetOverride" => bin(enc, 64),
            _ => false,
        }
    }

    /// Whether one backed-up entry ("HIVE|path|name" -> encoded value) is something the automatic layout writes (pure):
    /// the key (by its path: a device's interrupt keys, its Device Parameters, its driver's class key, the Kernel key,
    /// the mouse key), the value name and the registry type. Everything else (another key, another value name,
    /// another hive, another type, a text or binary value longer than the program ever wrote) is refused: the backup
    /// is never trusted to say what to write with administrator rights.
    pub fn allowed_entry(id: &str, enc: &str) -> bool {
        let f: Vec<&str> = id.split('|').collect();
        if f.len() != 3 {
            return false;
        }
        let (hive, path, name) = (f[0], f[1], f[2]);
        if hive == "HKCU" {
            return path == r"Control Panel\Mouse" && name == "RawMouseThrottleDuration" && (enc.is_empty() || dword(enc, I32_MIN, I32_MAX) || digits(enc, 10));
        }
        if hive != "HKLM" {
            return false;
        }
        if ci::eq(path, KERNEL_KEY) {
            return name == "ReservedCpuSets" && (enc.is_empty() || bin(enc, 64));
        }
        if ci::starts_with(path, CLASS_ROOT) {
            if !driver_key_rx().is_match(&skip_chars(path, CLASS_ROOT.chars().count())) {
                return false;
            }
            if name == "PnPCapabilities" {
                return enc.is_empty() || dword(enc, I32_MIN, I32_MAX);
            }
            if RSS_NAMES.contains(&name) || name == "*NumaNodeId" {
                return enc.is_empty() || dword(enc, I32_MIN, I32_MAX) || digits(enc, 10);
            }
            return false;
        }
        if !ci::starts_with(path, ENUM_ROOT) {
            return false;
        }
        let rest = skip_chars(path, ENUM_ROOT.chars().count());
        let tails: [(String, fn(&str, &str) -> bool); 4] = [
            (format!(r"{}\Affinity Policy", IM_SUB), |n, e| matches!(n, "DevicePriority" | "DevicePolicy" | "AssignmentSetOverride") && IrqAuto::valid_irq_value(n, e)),
            (format!(r"{}\MessageSignaledInterruptProperties", IM_SUB), |n, e| matches!(n, "MSISupported" | "MessageNumberLimit") && IrqAuto::valid_irq_value(n, e)),
            // The legacy Priority subkey is deleted whole: "*" marks the key, its values are kept as they were (bounded).
            (format!(r"{}\Priority", IM_SUB), |n, e| {
                if n == "*" {
                    e == "s:key"
                } else {
                    value_name_rx().is_match(n)
                        && (e.is_empty() || dword(e, i32::MIN as i64, i32::MAX as i64) || bin(e, 64) || (e.starts_with("s:") && utf16_len(e) <= 66 && !e.chars().any(|c| c.is_control())))
                }
            }),
            (r"\Device Parameters".to_string(), |n, e| matches!(n, "SelectiveSuspendEnabled" | "EnhancedPowerManagementEnabled") && (e.is_empty() || dword(e, I32_MIN, I32_MAX) || bin(e, 4))),
        ];
        for (tail, check) in tails.iter() {
            if !ci::ends_with(&rest, tail) {
                continue;
            }
            let inst = cut_end(&rest, tail.chars().count());
            return instance_rx().is_match(&inst) && check(name, enc);
        }
        false
    }

    /// The backup without anything `allowed_entry` refuses (WMI switches: only device instance names; the USB suspend
    /// pair: only 0/1). The refused entries are named in dropped (None: not collected).
    fn clean(b: Option<BackupIn>, mut dropped: Option<&mut Vec<String>>) -> Backup {
        let mut c = Backup::default();
        let Some(b) = b else { return c };
        for (id, enc) in b.reg.map(|o| o.0).unwrap_or_default() {
            match enc {
                Some(enc) if Self::allowed_entry(&id, &enc) => {
                    c.reg.insert(id, enc);
                }
                // A null value is never something the program wrote (C# kept it and failed on it later).
                _ => {
                    if let Some(d) = dropped.as_deref_mut() {
                        d.push(id);
                    }
                }
            }
        }
        for (inst, on) in b.wmi.map(|o| o.0).unwrap_or_default() {
            if wmi_instance_rx().is_match(&inst) {
                c.wmi.insert(inst, on);
            } else if let Some(d) = dropped.as_deref_mut() {
                d.push(format!("WMI {}", inst));
            }
        }
        match b.usb_suspend {
            Some(u) if u.len() == 2 && u[0] <= 1 && u[1] <= 1 => c.usb_suspend = Some(u),
            Some(_) => {
                if let Some(d) = dropped.as_deref_mut() {
                    d.push("UsbSuspend".into());
                }
            }
            None => {}
        }
        c
    }

    /// Takes over a backup file of an older version (from the user's folder): only the allowed entries, as JSON for
    /// the state folder; None when nothing is left. The refused ones are named in dropped. Err: the JSON is broken
    /// (C# throws).
    pub fn import_backup(json: &str, existing: Option<&str>, dropped: Option<&mut Vec<String>>) -> Result<Option<String>, String> {
        let b = Self::clean(Backup::parse(json)?, dropped);
        // A backup already in the state folder wins: it holds the values from before the first change.
        let mut cur = match existing {
            None => Backup::default(),
            Some(e) => Self::clean(Backup::parse(e)?, None),
        };
        for (k, v) in b.reg {
            cur.reg.entry(k).or_insert(v);
        }
        for (k, v) in b.wmi {
            cur.wmi.entry(k).or_insert(v);
        }
        if cur.usb_suspend.is_none() {
            cur.usb_suspend = b.usb_suspend;
        }
        Ok(if cur.is_empty() { None } else { Some(cur.to_json()) })
    }

    /// 1.16.0 Test 1.9: is the layout already written? Each pair is (the affinity policy now, the processors the plan
    /// gives; 0 = no core of its own, Windows decides). True only when every device already matches, so the Devices
    /// page does not offer to write the same layout again.
    pub fn layout_in_place(devices: &[(Option<u64>, u64)]) -> bool {
        let mut any = false;
        for (now, want) in devices {
            any = true;
            if now.unwrap_or(0) != *want {
                return false;
            }
        }
        any
    }

    fn backup_path() -> Result<PathBuf, String> {
        Ok(store_dir()?.join(Self::BACKUP_NAME))
    }

    pub fn has_backup() -> bool {
        Self::backup_path().map(|p| p.is_file()).unwrap_or(false)
    }

    /// The device instance a backed-up value belongs to ("...\Enum\<id>\Device Parameters..."), or None.
    pub fn device_of_entry(id: &str) -> Option<String> {
        let f: Vec<&str> = id.split('|').collect();
        if f.len() < 2 || !ci::starts_with(f[1], ENUM_ROOT) {
            return None;
        }
        let rest = skip_chars(f[1], ENUM_ROOT.chars().count());
        let i = ci::find(&rest, r"\Device Parameters")?;
        if i > 0 { Some(rest[..i].to_string()) } else { None }
    }

    /// `Load()`: the backup file with only the allowed entries; an empty one when there is none or it is unreadable.
    fn load() -> Backup {
        let r = (|| -> Result<Backup, String> {
            let path = Self::backup_path()?;
            if !path.is_file() {
                return Ok(Backup::default());
            }
            let mut dropped = Vec::new();
            let text = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
            let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
            let b = Self::clean(Backup::parse(text)?, Some(&mut dropped));
            if !dropped.is_empty() {
                let first: Vec<String> = dropped.iter().take(5).cloned().collect();
                crate::base::app_log::AppLog::add_sev(
                    crate::lf!("Device backup: entries that are not values the program writes were ignored: {0}", first.join(", ")),
                    crate::base::app_log::Sev::Warn,
                );
            }
            Ok(b)
        })();
        r.unwrap_or_default()
    }

    fn save(bk: &Backup) -> Result<(), String> {
        let path = Self::backup_path()?;
        if let Some(d) = path.parent() {
            std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
        }
        let tmp = PathBuf::from(format!("{}.tmp", path.display()));
        std::fs::write(&tmp, bk.to_json()).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, &path).map_err(|e| e.to_string())
    }
}

#[cfg(windows)]
fn win_cppc() -> Option<HashMap<i32, i32>> {
    win::cppc()
}

#[cfg(not(windows))]
fn win_cppc() -> Option<HashMap<i32, i32>> {
    None
}

/// The CPU is AMD family 17h (Zen, Zen+, Zen 2): two L3 groups form one CCD.
#[allow(unused_unsafe)]
fn amd_family17() -> bool {
    #[cfg(target_arch = "x86_64")]
    {
        // SAFETY: CPUID exists on every x86-64 processor.
        let (l0, l1) = unsafe { (core::arch::x86_64::__cpuid(0), core::arch::x86_64::__cpuid(1)) };
        let amd = l0.ebx == 0x68747541 && l0.edx == 0x69746E65 && l0.ecx == 0x444D4163;
        let eax = l1.eax as i32;
        let bas = (eax >> 8) & 0xF;
        let ext = (eax >> 20) & 0xFF;
        amd && (if bas == 0xF { bas + ext } else { bas }) == 0x17
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        false
    }
}

// ---- the Windows parts: reading the PC, writing with the backup ---------------------------------------------------

/// Lines for the log: (text, ok).
pub type LogLines = Vec<(String, bool)>;

#[cfg(windows)]
mod win {
    use super::*;
    use crate::base::app_log::{AppLog, Sev};
    use crate::base::flags::Flags;
    use crate::base::l::L;
    use crate::lf;
    use crate::native::reg::{error_text, Hive, Key, RegValue};
    use crate::native::sys_exe::SysExe;
    use crate::native::win::wide;
    use crate::topo::builder::TopoBuilder;
    use crate::topo::masks::Masks;
    use std::io::Read;
    use windows_sys::core::GUID;
    use windows_sys::Win32::Devices::DeviceAndDriverInstallation::*;
    use windows_sys::Win32::Devices::Properties::DEVPROPKEY;
    use windows_sys::Win32::Foundation::{CloseHandle, LocalFree, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Storage::FileSystem::CreateFileW;
    use windows_sys::Win32::System::Power::*;

    #[link(name = "hid")]
    extern "system" {
        fn HidD_GetPreparsedData(h: *mut core::ffi::c_void, data: *mut isize) -> u8;
        fn HidD_FreePreparsedData(data: isize) -> u8;
        fn HidP_GetCaps(data: isize, caps: *mut u8) -> i32;
        fn HidD_GetProductString(h: *mut core::ffi::c_void, buf: *mut u8, len: u32) -> u8;
    }

    const DIGCF_PRESENT_: u32 = 0x2;
    const DIGCF_ALLCLASSES_: u32 = 0x4;
    const DIGCF_DEVICEINTERFACE_: u32 = 0x10;
    const SPDRP_DEVICEDESC_: u32 = 0;
    const SPDRP_SERVICE_: u32 = 4;
    const SPDRP_CLASS_: u32 = 7;
    const SPDRP_DRIVER_: u32 = 9;
    const SPDRP_FRIENDLYNAME_: u32 = 12;
    const HID_GUID: u128 = 0x4d1e55b2_f16f_11cf_88cb_001111000030;

    // ---- the registry the way Microsoft.Win32.Registry behaves -------------------------------------------------

    /// What a registry call threw in C#: UnauthorizedAccessException (Apply reports "Access denied") or another one.
    pub enum RegErr {
        Unauthorized(String),
        Other(String),
    }

    impl RegErr {
        pub fn message(&self) -> String {
            match self {
                RegErr::Unauthorized(m) | RegErr::Other(m) => m.clone(),
            }
        }
    }

    fn hive_of(h: &str) -> Result<(Hive, &'static str), RegErr> {
        match h {
            "HKLM" => Ok((Hive::LocalMachine, "HKEY_LOCAL_MACHINE")),
            "HKCU" => Ok((Hive::CurrentUser, "HKEY_CURRENT_USER")),
            _ => Err(RegErr::Other(format!("hive {}", h))),
        }
    }

    fn fail(hive_name: &str, path: &str, e: String) -> RegErr {
        if e == error_text(5) {
            RegErr::Unauthorized(format!("Access to the registry key '{}\\{}' is denied.", hive_name, path))
        } else {
            RegErr::Other(e)
        }
    }

    /// `OpenSubKey(path, writable)`: None when absent; a denied key throws (SecurityException in C#).
    fn open(hive: &str, path: &str, writable: bool) -> Result<Option<Key>, RegErr> {
        let (h, _) = hive_of(hive)?;
        match Key::open_err(h, path, writable) {
            Ok(k) => Ok(Some(k)),
            Err(2) | Err(3) => Ok(None),
            Err(5) => Err(RegErr::Other("Requested registry access is not allowed.".into())),
            Err(c) => Err(RegErr::Other(error_text(c))),
        }
    }

    fn create(hive: &str, path: &str) -> Result<Key, RegErr> {
        let (h, n) = hive_of(hive)?;
        Key::create(h, path).map_err(|e| fail(n, path, e))
    }

    /// Lenient read (`Registry.LocalMachine.OpenSubKey(path)` where the C# code does not care why it is missing).
    fn lm(path: &str) -> Option<Key> {
        Key::open(Hive::LocalMachine, path, false)
    }

    /// `GetValue(name) as int`.
    fn get_int(k: &Key, name: &str) -> Option<i32> {
        match k.get(name)? {
            RegValue::Dword(v) => Some(v as i32),
            _ => None,
        }
    }

    fn get_multi(k: &Key, name: &str) -> Vec<String> {
        match k.get(name) {
            Some(RegValue::MultiSz(v)) => v,
            _ => Vec::new(),
        }
    }

    /// `Enc(value)`: "" absent, d:int, b:HEX, s:text; anything else (QWORD, MULTI_SZ) "".
    fn enc(v: Option<RegValue>) -> String {
        match v {
            None => String::new(),
            Some(RegValue::Dword(d)) => format!("d:{}", d as i32),
            Some(RegValue::Binary(b)) => format!("b:{}", b.iter().map(|x| format!("{:02X}", x)).collect::<String>()),
            Some(RegValue::Sz(s)) | Some(RegValue::ExpandSz(s)) => format!("s:{}", s),
            Some(_) => String::new(),
        }
    }

    fn remember(bk: &mut Backup, hive: &str, path: &str, name: &str) -> Result<(), RegErr> {
        let id = format!("{}|{}|{}", hive, path, name);
        if bk.reg.contains_key(&id) {
            return Ok(());
        }
        let k = open(hive, path, false)?;
        bk.reg.insert(id, enc(k.and_then(|k| k.get(name))));
        Ok(())
    }

    fn set_d(bk: &mut Backup, hive: &str, path: &str, name: &str, v: i32) -> Result<(), RegErr> {
        remember(bk, hive, path, name)?;
        let k = create(hive, path)?;
        let n = hive_of(hive)?.1;
        k.set(name, &RegValue::Dword(v as u32)).map_err(|e| fail(n, path, e))
    }

    fn set_b(bk: &mut Backup, hive: &str, path: &str, name: &str, v: &[u8]) -> Result<(), RegErr> {
        remember(bk, hive, path, name)?;
        let k = create(hive, path)?;
        let n = hive_of(hive)?.1;
        k.set(name, &RegValue::Binary(v.to_vec())).map_err(|e| fail(n, path, e))
    }

    fn del(bk: &mut Backup, hive: &str, path: &str, name: &str) -> Result<(), RegErr> {
        let Some(k) = open(hive, path, true)? else { return Ok(()) };
        if !k.value_names().iter().any(|x| ci::eq(x, name)) {
            return Ok(());
        }
        remember(bk, hive, path, name)?;
        let n = hive_of(hive)?.1;
        k.delete_value(name, false).map_err(|e| fail(n, path, e))
    }

    /// Deletes a subkey; its values are remembered so the restore can recreate it.
    fn del_key(bk: &mut Backup, path: &str) -> Result<(), RegErr> {
        {
            let Some(k) = open("HKLM", path, false)? else { return Ok(()) };
            bk.reg.entry(format!("HKLM|{}|*", path)).or_insert_with(|| "s:key".into());
            for n in k.value_names() {
                remember(bk, "HKLM", path, &n)?;
            }
        }
        let root = Key::open_err(Hive::LocalMachine, "", true).map_err(|c| fail("HKEY_LOCAL_MACHINE", path, error_text(c)))?;
        root.delete_subkey_tree(path).map_err(|e| fail("HKEY_LOCAL_MACHINE", path, e))
    }

    // ---- reading the PC -----------------------------------------------------------------------------------

    /// DEVPKEY_PciDevice_InterruptSupport (bit 1 line, 2 MSI, 4 MSI-X), or None when it cannot be read.
    fn interrupt_support(dev_inst: u32) -> Option<i32> {
        let key = DEVPROPKEY { fmtid: GUID::from_u128(0x3ab22e31_8264_4b4e_9af5_a8d2d8e33e62), pid: 14 };
        let mut buf = [0u8; 8];
        let mut size = buf.len() as u32;
        let mut ty: u32 = 0;
        // SAFETY: buf has `size` bytes.
        let rc = unsafe { CM_Get_DevNode_PropertyW(dev_inst, &key, &mut ty, buf.as_mut_ptr(), &mut size, 0) };
        if rc != 0 || size < 4 || ty != 7 {
            return None; // DEVPROP_TYPE_UINT32
        }
        Some(i32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]))
    }

    /// Any allocated interrupt of the device is message-signalled (`Devices.Resources(devInst).Any(r => r.Message)`:
    /// IRQ_DES_64 records of ALLOC_LOG_CONF, the top bit of Alloc_Num). A private copy of the system area's reader.
    fn uses_msi_now(dev_inst: u32) -> bool {
        let mut conf: usize = 0;
        // SAFETY: plain out parameters; every handle is freed below.
        unsafe {
            if CM_Get_First_Log_Conf(&mut conf, dev_inst, 2 /* ALLOC_LOG_CONF */) != 0 {
                return false;
            }
            let mut any = false;
            let mut cur = conf;
            loop {
                let mut rd: usize = 0;
                let mut rid: u32 = 0;
                if CM_Get_Next_Res_Des(&mut rd, cur, 4 /* ResType_IRQ */, &mut rid, 0) != 0 {
                    break;
                }
                let mut size: u32 = 0;
                if CM_Get_Res_Des_Data_Size(&mut size, rd, 0) == 0 && size >= 16 {
                    let mut data = vec![0u8; size as usize];
                    if CM_Get_Res_Des_Data(rd, data.as_mut_ptr() as *mut core::ffi::c_void, size, 0) == 0 {
                        let mut off = 0;
                        while off + 16 <= data.len() {
                            let alloc = u32::from_le_bytes([data[off + 12], data[off + 13], data[off + 14], data[off + 15]]);
                            if alloc & 0x80000000 != 0 {
                                any = true;
                            }
                            off += 24;
                        }
                    }
                }
                if cur != conf {
                    CM_Free_Res_Des_Handle(cur);
                }
                cur = rd;
            }
            if cur != conf {
                CM_Free_Res_Des_Handle(cur);
            }
            CM_Free_Log_Conf_Handle(conf);
            any
        }
    }

    /// The device offers MSI (see `msi_allowed`).
    fn msi_capable(d: &AutoDevice) -> bool {
        let uses = d.dev_inst != 0 && uses_msi_now(d.dev_inst);
        let limit = lm(&format!("{}{}{}\\MessageSignaledInterruptProperties", ENUM_ROOT, d.id, IM_SUB)).is_some_and(|k| k.get("MessageNumberLimit").is_some());
        IrqAuto::msi_allowed(if d.dev_inst != 0 { interrupt_support(d.dev_inst) } else { None }, uses, limit)
    }

    /// A battery (laptop, tablet, UPS reported as a battery) or Modern Standby: power saving is left alone.
    pub fn battery_or_modern_standby() -> bool {
        // SAFETY: plain out parameters; caps is a 256-byte buffer (larger than SYSTEM_POWER_CAPABILITIES).
        unsafe {
            let mut st: SYSTEM_POWER_STATUS = std::mem::zeroed();
            let flag = if GetSystemPowerStatus(&mut st) != 0 { st.BatteryFlag } else { 255 };
            let mut caps = [0u64; 32];
            let (mut batteries, mut aoac) = (false, false);
            if GetPwrCapabilities(caps.as_mut_ptr() as *mut SYSTEM_POWER_CAPABILITIES) != 0 {
                let b = std::slice::from_raw_parts(caps.as_ptr() as *const u8, 256);
                aoac = b[20] != 0; // SYSTEM_POWER_CAPABILITIES.AoAc
                batteries = b[30] != 0; // SystemBatteriesPresent
            }
            !IrqAuto::power_saving_allowed(flag, batteries, aoac)
        }
    }

    fn prop(set: HDEVINFO, d: &SP_DEVINFO_DATA, p: u32) -> String {
        let mut buf = vec![0u8; 2048];
        let mut need: u32 = 0;
        let mut ty: u32 = 0;
        // SAFETY: buf has 2048 bytes.
        if unsafe { SetupDiGetDeviceRegistryPropertyW(set, d, p, &mut ty, buf.as_mut_ptr(), buf.len() as u32, &mut need) } == 0 {
            return String::new();
        }
        let n = (need as usize).min(buf.len()) / 2;
        let w: Vec<u16> = (0..n).map(|i| u16::from_le_bytes([buf[2 * i], buf[2 * i + 1]])).collect();
        let s = String::from_utf16_lossy(&w);
        s.trim_end_matches('\0').split('\0').next().unwrap_or("").to_string()
    }

    fn id_of(inst: u32) -> String {
        let mut buf = vec![0u16; 512];
        // SAFETY: buf has 512 chars.
        if unsafe { CM_Get_Device_IDW(inst, buf.as_mut_ptr(), buf.len() as u32, 0) } == 0 {
            let n = buf.iter().position(|c| *c == 0).unwrap_or(buf.len());
            ci::upper(&String::from_utf16_lossy(&buf[..n]))
        } else {
            String::new()
        }
    }

    /// The PCI device a device hangs on (walking up the device tree), or None.
    fn pci_parent(mut node: u32) -> Option<(String, u32)> {
        for _ in 0..16 {
            let mut parent: u32 = 0;
            // SAFETY: plain out parameter.
            if unsafe { CM_Get_Parent(&mut parent, node, 0) } != 0 {
                return None;
            }
            let id = id_of(parent);
            if id.starts_with("PCI\\") {
                return Some((id, parent));
            }
            node = parent;
        }
        None
    }

    #[derive(Clone)]
    struct Present {
        id: String,
        name: String,
        class: String,
        service: String,
        driver: String,
        inst: u32,
    }

    fn info_data() -> SP_DEVINFO_DATA {
        // SAFETY: plain data.
        let mut d: SP_DEVINFO_DATA = unsafe { std::mem::zeroed() };
        d.cbSize = std::mem::size_of::<SP_DEVINFO_DATA>() as u32;
        d
    }

    fn all_present() -> Vec<Present> {
        let mut list = Vec::new();
        // SAFETY: the set is destroyed below.
        let set = unsafe { SetupDiGetClassDevsW(std::ptr::null(), std::ptr::null(), std::ptr::null_mut(), DIGCF_PRESENT_ | DIGCF_ALLCLASSES_) };
        if set == 0 || set == -1 {
            return list;
        }
        let mut d = info_data();
        let mut i = 0u32;
        // SAFETY: d is a valid SP_DEVINFO_DATA.
        while unsafe { SetupDiEnumDeviceInfo(set, i, &mut d) } != 0 {
            let (mut status, mut problem) = (0u32, 0u32);
            // SAFETY: plain out parameters.
            if unsafe { CM_Get_DevNode_Status(&mut status, &mut problem, d.DevInst, 0) } == 0 && problem == 0 {
                let mut name = prop(set, &d, SPDRP_FRIENDLYNAME_);
                if name.is_empty() {
                    name = prop(set, &d, SPDRP_DEVICEDESC_);
                }
                list.push(Present {
                    id: id_of(d.DevInst),
                    name,
                    class: prop(set, &d, SPDRP_CLASS_),
                    service: prop(set, &d, SPDRP_SERVICE_),
                    driver: prop(set, &d, SPDRP_DRIVER_),
                    inst: d.DevInst,
                });
            }
            d.cbSize = std::mem::size_of::<SP_DEVINFO_DATA>() as u32;
            i += 1;
        }
        // SAFETY: from SetupDiGetClassDevsW.
        unsafe { SetupDiDestroyDeviceInfoList(set) };
        list
    }

    /// Every HID collection with an input role: (its PCI controller, role).
    fn hid_collections() -> Vec<(String, IrqRole)> {
        let mut res = Vec::new();
        let g = GUID::from_u128(HID_GUID);
        // SAFETY: the set is destroyed below.
        let set = unsafe { SetupDiGetClassDevsW(&g, std::ptr::null(), std::ptr::null_mut(), DIGCF_PRESENT_ | DIGCF_DEVICEINTERFACE_) };
        if set == 0 || set == -1 {
            return res;
        }
        // SAFETY: plain data.
        let mut ifd: SP_DEVICE_INTERFACE_DATA = unsafe { std::mem::zeroed() };
        ifd.cbSize = std::mem::size_of::<SP_DEVICE_INTERFACE_DATA>() as u32;
        let mut i = 0u32;
        // SAFETY: every buffer below has the size passed with it; handles are closed.
        unsafe {
            while SetupDiEnumDeviceInterfaces(set, std::ptr::null(), &g, i, &mut ifd) != 0 {
                i += 1;
                let mut info = info_data();
                let mut need: u32 = 0;
                SetupDiGetDeviceInterfaceDetailW(set, &ifd, std::ptr::null_mut(), 0, &mut need, &mut info);
                let mut detail = vec![0u64; (need.max(16) as usize).div_ceil(8)];
                let dp = detail.as_mut_ptr() as *mut u8;
                (dp as *mut u32).write_unaligned(8); // cbSize of SP_DEVICE_INTERFACE_DETAIL_DATA_W on x64
                if SetupDiGetDeviceInterfaceDetailW(set, &ifd, dp as *mut SP_DEVICE_INTERFACE_DETAIL_DATA_W, need, std::ptr::null_mut(), &mut info) == 0 {
                    ifd.cbSize = std::mem::size_of::<SP_DEVICE_INTERFACE_DATA>() as u32;
                    continue;
                }
                let pw = dp.add(4) as *const u16;
                let mut n = 0usize;
                let max = (detail.len() * 8 - 4) / 2;
                while n < max && *pw.add(n) != 0 {
                    n += 1;
                }
                let path = String::from_utf16_lossy(std::slice::from_raw_parts(pw, n));
                let cls = prop(set, &info, SPDRP_CLASS_);
                let mut name = prop(set, &info, SPDRP_FRIENDLYNAME_);
                if name.is_empty() {
                    name = prop(set, &info, SPDRP_DEVICEDESC_);
                }
                let (mut page, mut usage) = (0i32, 0i32);
                let mut product = String::new();
                let wp = wide(&path);
                let h = CreateFileW(wp.as_ptr(), 0, 3, std::ptr::null(), 3, 0, std::ptr::null_mut());
                if h != INVALID_HANDLE_VALUE {
                    let mut pp: isize = 0;
                    if HidD_GetPreparsedData(h, &mut pp) != 0 {
                        let mut caps = [0u8; 64];
                        if HidP_GetCaps(pp, caps.as_mut_ptr()) == 0x00110000 {
                            usage = u16::from_le_bytes([caps[0], caps[1]]) as i32;
                            page = u16::from_le_bytes([caps[2], caps[3]]) as i32;
                        }
                        HidD_FreePreparsedData(pp);
                    }
                    let mut pb = [0u8; 512];
                    if HidD_GetProductString(h, pb.as_mut_ptr(), pb.len() as u32) != 0 {
                        let w: Vec<u16> = (0..256).map(|i| u16::from_le_bytes([pb[2 * i], pb[2 * i + 1]])).collect();
                        product = String::from_utf16_lossy(&w).trim_end_matches('\0').to_string();
                    }
                    CloseHandle(h);
                }
                ifd.cbSize = std::mem::size_of::<SP_DEVICE_INTERFACE_DATA>() as u32;
                let Some(role) = IrqAuto::hid_role(page, usage, &cls, &name, &product) else { continue };
                if let Some((ctrl, _)) = pci_parent(info.DevInst) {
                    res.push((ctrl, role));
                }
            }
            SetupDiDestroyDeviceInfoList(set);
        }
        res
    }

    fn non_default_affinity(id: &str) -> bool {
        let Some(k) = lm(&format!("{}{}{}\\Affinity Policy", ENUM_ROOT, id, IM_SUB)) else { return false };
        if get_int(&k, "DevicePolicy").is_some_and(|p| p != 0) {
            return true;
        }
        if get_int(&k, "DevicePriority").is_some_and(|p| p != 2) {
            return true;
        }
        k.get_binary("AssignmentSetOverride").is_some_and(|b| b.iter().any(|x| *x != 0))
    }

    fn is_net_cx(p: &Present) -> bool {
        if !p.service.is_empty() {
            if let Some(s) = lm(&format!(r"SYSTEM\CurrentControlSet\Services\{}", p.service)) {
                if netadapter_name().is_match(&s.get_string("DisplayName").unwrap_or_default()) {
                    return true;
                }
                if netadapter_image().is_match(&s.get_string("ImagePath").unwrap_or_default()) {
                    return true;
                }
                if get_multi(&s, "DependOnService").iter().any(|x| ci::contains(x, "netadapter")) {
                    return true;
                }
            }
        }
        let e = lm(&format!("{}{}", ENUM_ROOT, p.id));
        for f in ["UpperFilters", "LowerFilters"] {
            if e.as_ref().map(|k| get_multi(k, f)).unwrap_or_default().iter().any(|x| ci::contains(x, "netadapter")) {
                return true;
            }
        }
        false
    }

    /// The device list in DEVICE-TWEAKER's order (USB, GPU, audio, network, storage), with roles and kinds.
    pub fn discover() -> Vec<AutoDevice> {
        let all = all_present();
        let mut list: Vec<AutoDevice> = Vec::new();
        for p in &all {
            if IrqAuto::excluded(&p.id, &p.name, &p.class) {
                continue;
            }
            if IrqAuto::is_storage(&p.class, &p.name, &p.service) {
                continue; // never: the boot disk hangs on one of these
            }
            let Some(kind) = IrqAuto::classify(&p.class, &p.name) else { continue };
            if ci::eq(&p.class, "AudioEndpoint") {
                continue;
            }
            if kind == AutoKind::Usb && nvidia_usb().is_match(&p.id) {
                continue; // NVIDIA USB-C
            }
            if lm(&format!("{}{}{}", ENUM_ROOT, p.id, IM_SUB)).is_none() {
                continue;
            }
            list.push(AutoDevice {
                id: p.id.clone(),
                name: p.name.clone(),
                class: p.class.clone(),
                service: p.service.clone(),
                driver_key: p.driver.clone(),
                dev_inst: p.inst,
                kind,
                ..Default::default()
            });
        }
        // USB roles from the HID collections: the most important one wins (mouse, game controller, keyboard).
        for (ctrl, role) in hid_collections() {
            if let Some(dev) = list.iter_mut().find(|d| d.kind == AutoKind::Usb && d.id == ctrl) {
                if dev.role.is_none() || Some(role) < dev.role {
                    dev.role = Some(role);
                }
            }
        }
        // Every USB host controller is listed (a PC usually has two or more, e.g. USB 3.10 and USB 3.0): the ones with a
        // mouse, keyboard or game controller get a slot, the others stay with Windows' own placement.
        {
            let mut gpus: Vec<&mut AutoDevice> = list.iter_mut().filter(|d| d.kind == AutoKind::Gpu).collect();
            IrqAuto::mark_integrated(&mut gpus);
        }
        for n in list.iter_mut().filter(|d| d.kind == AutoKind::Net) {
            n.wifi = IrqAuto::is_wifi(&n.id, &n.name, &n.service);
            if let Some(p) = all.iter().find(|x| x.id == n.id) {
                n.net_cx = is_net_cx(p);
            }
        }
        // Audio: endpoints mapped to their controller; no endpoints or only S/PDIF: not listed.
        for ep in all.iter().filter(|x| ci::eq(&x.class, "AudioEndpoint")) {
            if let Some((ctrl, _)) = pci_parent(ep.inst) {
                if let Some(dev) = list.iter_mut().find(|d| d.kind == AutoKind::Audio && d.id == ctrl) {
                    dev.endpoints.push(ep.name.clone());
                }
            }
        }
        for a in list.iter_mut().filter(|d| d.kind == AutoKind::Audio) {
            a.display_audio = IrqAuto::is_display_audio_device(&a.id, &a.name) || (!a.endpoints.is_empty() && a.endpoints.iter().all(|e| IrqAuto::is_display_endpoint(e)));
        }
        list.retain(|d| !(d.kind == AutoKind::Audio && (d.endpoints.is_empty() || d.endpoints.iter().all(|e| IrqAuto::is_spdif(e)))));
        list.retain(|d| d.kind != AutoKind::Stor);
        list.sort_by_key(|d| d.kind); // stable: equal kinds keep their order
        list
    }

    pub fn cppc() -> Option<HashMap<i32, i32>> {
        let n = (4 * std::thread::available_parallelism().map(|x| x.get()).unwrap_or(1)).max(16);
        let mut psi = SysExe::start("wevtutil.exe", false);
        psi.redirect_standard_output = true;
        for a in [
            "qe".to_string(),
            "System".to_string(),
            "/q:*[System[Provider[@Name='Microsoft-Windows-Kernel-Processor-Power'] and (EventID=55)]]".to_string(),
            format!("/c:{}", n),
            "/rd:true".to_string(),
            "/f:xml".to_string(),
        ] {
            psi.arg(&a);
        }
        let mut child = psi.command().spawn().ok()?;
        let mut xml = String::new();
        if let Some(mut out) = child.stdout.take() {
            let mut b = Vec::new();
            let _ = out.read_to_end(&mut b);
            xml = String::from_utf8_lossy(&b).to_string();
        }
        wait_for(&mut child, 10000);
        IrqAuto::parse_cppc(&xml).ok()
    }

    /// `Process.WaitForExit(ms)`: true when it ended in time (the process is left running otherwise, as in C#).
    fn wait_for(child: &mut std::process::Child, ms: u64) -> bool {
        let until = std::time::Instant::now() + std::time::Duration::from_millis(ms);
        loop {
            match child.try_wait() {
                Ok(Some(_)) => return true,
                Ok(None) => {}
                Err(_) => return false,
            }
            if std::time::Instant::now() >= until {
                return false;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }

    /// Discovers the devices and plans their cores (nothing is written).
    pub fn plan_now() -> Vec<AutoDevice> {
        let mut devs = discover();
        let topo = TopoBuilder::live();
        IrqAuto::assign(&mut devs, topo.as_deref(), &IrqAuto::cppc(), amd_family17());
        for n in devs.iter_mut().filter(|d| d.kind == AutoKind::Net && !d.wifi && !d.net_cx) {
            n.nic_mode = nic_mode(n);
        }
        devs
    }

    /// NDIS network card: RSS, IRQ or BOTH, as DEVICE-TWEAKER decides it (one queue planned).
    fn nic_mode(n: &AutoDevice) -> String {
        let (mut cap, mut configured, mut enabled) = (false, false, false);
        let (mut queues, mut max_proc) = (0i32, 0i32);
        if let Some(k) = lm(&format!("{}{}", CLASS_ROOT, n.driver_key)) {
            configured = k.get("*RssBaseProcNumber").is_some() || k.get("*NumRssQueues").is_some();
            let ndi = k.open_sub(r"Ndi\Params\*RssBaseProcNumber", false);
            cap = configured || ndi.is_some();
        }
        if lm(&format!("{}{}", ENUM_ROOT, n.id)).is_some_and(|e| e.get("*RssBaseProcNumber").is_some()) {
            cap = true;
        }
        let script = "Get-NetAdapter -IncludeHidden -EA 0 | ForEach-Object { $r = Get-NetAdapterRss -Name $_.Name -EA 0; if ($r) { $_.PnPDeviceID + '|' + [int]$r.Enabled + '|' + $r.NumberOfReceiveQueues + '|' + $r.MaxProcessors } }";
        // Windows' own NetAdapter module only (SysExe.PowerShell: no profile, system module path).
        let psi = SysExe::power_shell(script, &[]);
        if let Ok(mut child) = psi.command().spawn() {
            let out = child.stdout.take();
            let err = child.stderr.take();
            let reader = std::thread::spawn(move || {
                let mut b = Vec::new();
                if let Some(mut o) = out {
                    let _ = o.read_to_end(&mut b);
                }
                String::from_utf8_lossy(&b).to_string()
            });
            let _drain = std::thread::spawn(move || {
                if let Some(mut e) = err {
                    let mut b = Vec::new();
                    let _ = e.read_to_end(&mut b);
                }
            });
            if wait_for(&mut child, 10000) {
                let text = reader.join().unwrap_or_default();
                for line in text.split('\n') {
                    let f: Vec<&str> = line.trim().split('|').collect();
                    if f.len() == 4 && ci::eq(f[0], &n.id) {
                        cap = true;
                        enabled = f[1] == "1";
                        queues = f[2].trim().parse().unwrap_or(0);
                        max_proc = f[3].trim().parse().unwrap_or(0);
                    }
                }
            }
        }
        IrqAuto::choose_nic_mode(cap, enabled, configured, queues, max_proc)
    }

    // ---- writing, with the backup -------------------------------------------------------------------------

    /// The interrupt values of one device. priority None = deleted; policy None = untouched; 0 = written 0 and the mask
    /// deleted. MSI is switched on only when the device offers it; the driver's MessageNumberLimit is never touched.
    fn irq(bk: &mut Backup, d: &AutoDevice, priority: Option<i32>, policy: Option<i32>, mask: u64, log: &mut LogLines, delete_policy: bool) -> Result<(), RegErr> {
        if IrqAuto::is_storage(&d.class, &d.name, &d.service) {
            return Ok(());
        }
        let im = format!("{}{}{}", ENUM_ROOT, d.id, IM_SUB);
        let ap = format!("{}\\Affinity Policy", im);
        msi(bk, d, log)?;
        match priority {
            Some(p) => set_d(bk, "HKLM", &ap, "DevicePriority", p)?,
            None => del(bk, "HKLM", &ap, "DevicePriority")?,
        }
        del_key(bk, &format!("{}\\Priority", im))?;
        if delete_policy {
            del(bk, "HKLM", &ap, "DevicePolicy")?;
            del(bk, "HKLM", &ap, "AssignmentSetOverride")?;
        } else if policy == Some(4) && mask != 0 {
            set_d(bk, "HKLM", &ap, "DevicePolicy", 4)?;
            set_b(bk, "HKLM", &ap, "AssignmentSetOverride", &mask.to_le_bytes())?;
        } else if policy.is_some() {
            set_d(bk, "HKLM", &ap, "DevicePolicy", 0)?;
            del(bk, "HKLM", &ap, "AssignmentSetOverride")?;
        }
        Ok(())
    }

    fn msi(bk: &mut Backup, d: &AutoDevice, log: &mut LogLines) -> Result<(), RegErr> {
        if msi_capable(d) {
            set_d(bk, "HKLM", &format!("{}{}{}\\MessageSignaledInterruptProperties", ENUM_ROOT, d.id, IM_SUB), "MSISupported", 1)
        } else {
            log.push((lf!("{0}: MSI left as it is (the device does not offer it)", d.name.as_str()), true));
            Ok(())
        }
    }

    fn rss(bk: &mut Backup, d: &AutoDevice, lp: i32) -> Result<(), RegErr> {
        let c = format!("{}{}", CLASS_ROOT, d.driver_key);
        let values = [1, lp, 0, 1, 0, lp];
        for (i, n) in RSS_NAMES.iter().enumerate() {
            set_d(bk, "HKLM", &c, n, values[i])?;
        }
        del(bk, "HKLM", &c, "*NumaNodeId")
    }

    fn rss_clear(bk: &mut Backup, d: &AutoDevice) -> Result<(), RegErr> {
        let c = format!("{}{}", CLASS_ROOT, d.driver_key);
        for n in RSS_NAMES.iter().copied().chain(std::iter::once("*NumaNodeId")) {
            del(bk, "HKLM", &c, n)?;
        }
        Ok(())
    }

    /// Power saving off for one device: class-key PnPCapabilities bit 0x08 and WMI MSPower_DeviceEnable.
    fn power_off(bk: &mut Backup, id: &str, driver_key: &str, log: &mut LogLines) -> Result<(), RegErr> {
        if !driver_key.is_empty() {
            let c = format!("{}{}", CLASS_ROOT, driver_key);
            let cur = Key::open(Hive::LocalMachine, &c, false).and_then(|k| get_int(&k, "PnPCapabilities")).unwrap_or(0);
            set_d(bk, "HKLM", &c, "PnPCapabilities", cur | 0x08)?;
        }
        let wmi = &mut bk.wmi;
        let r = super::super::wmi::device_enable(|key, enable| {
            if !ci::starts_with(&key.replace('#', "\\"), id) {
                return None;
            }
            wmi.entry(key.to_string()).or_insert(enable == Some(true));
            Some(false)
        });
        if let Err(e) = r {
            log.push((lf!("{0}: the power-saving switch was not changed ({1})", id, e.as_str()), false));
        }
        Ok(())
    }

    fn root_hubs(controller: u32) -> Vec<(String, String)> {
        let mut res = Vec::new();
        let mut child: u32 = 0;
        // SAFETY: plain out parameters.
        if unsafe { CM_Get_Child(&mut child, controller, 0) } != 0 {
            return res;
        }
        for _ in 0..64 {
            let id = id_of(child);
            if ci::contains(&id, "ROOT_HUB") {
                let drv = lm(&format!("{}{}", ENUM_ROOT, id)).and_then(|k| k.get_string("Driver")).unwrap_or_default();
                res.push((id, drv));
            }
            let mut next: u32 = 0;
            // SAFETY: plain out parameter.
            if unsafe { CM_Get_Sibling(&mut next, child, 0) } != 0 {
                break;
            }
            child = next;
        }
        res
    }

    // Power plan: USB selective suspend of the active plan.
    const USB_SUB: u128 = 0x2a737441_1930_4402_8d77_b2bebba308a3;
    const USB_SUSPEND: u128 = 0x48e6b7a6_50f5_4782_a5d4_53bb8f07e226;

    /// The USB selective suspend of the active plan, mains power (AC) only. The battery (DC) value is never changed;
    /// restore_dc only puts back a DC value that an older version (before 1.12.0) changed.
    fn usb_suspend_plan(bk: &mut Backup, ac: u32, restore_dc: Option<u32>, remember: bool) {
        // SAFETY: plain power API calls; the scheme pointer is freed with LocalFree.
        unsafe {
            let mut p: *mut GUID = std::ptr::null_mut();
            if PowerGetActiveScheme(std::ptr::null_mut(), &mut p) != 0 {
                return;
            }
            let scheme = *p;
            LocalFree(p as *mut core::ffi::c_void);
            let (sub, set) = (GUID::from_u128(USB_SUB), GUID::from_u128(USB_SUSPEND));
            if remember && bk.usb_suspend.is_none() {
                let (mut a, mut d) = (0u32, 0u32);
                PowerReadACValueIndex(std::ptr::null_mut(), &scheme, &sub, &set, &mut a);
                PowerReadDCValueIndex(std::ptr::null_mut(), &scheme, &sub, &set, &mut d);
                bk.usb_suspend = Some(vec![a, d]);
            }
            PowerWriteACValueIndex(std::ptr::null_mut(), &scheme, &sub, &set, ac);
            if let Some(dc) = restore_dc {
                let mut now = 0u32;
                if PowerReadDCValueIndex(std::ptr::null_mut(), &scheme, &sub, &set, &mut now) == 0 && now != dc {
                    PowerWriteDCValueIndex(std::ptr::null_mut(), &scheme, &sub, &set, dc);
                }
            }
            PowerSetActiveScheme(std::ptr::null_mut(), &scheme);
        }
    }

    fn mark_pending() -> Result<(), String> {
        match MARK_PENDING_HOOK.get() {
            Some(h) => h(),
            None => Err("the boot guard is not connected".into()),
        }
    }

    /// Writes the plan in DEVICE-TWEAKER's order: ReservedCpuSets removed, every device (Wi-Fi and storage skipped),
    /// the power plan's USB selective suspend (mains power, desktops only). The boot guard's marker is saved first;
    /// without it nothing is written. Returns lines for the log; Err where C# lets an exception out.
    pub fn apply(devs: &[AutoDevice]) -> Result<LogLines, String> {
        let mut lines: LogLines = Vec::new();
        if Flags::refuse(Flags::DEVICES_WRITE, L::t("Device policy write")) {
            lines.push((L::t("The feature flag devices-write is closed").to_string(), false));
            return Ok(lines);
        }
        if let Err(e) = mark_pending() {
            lines.push((lf!("Nothing was written: the safety marker could not be saved ({0})", e.as_str()), false));
            return Ok(lines);
        }
        let mut bk = IrqAuto::load();
        let mut log: LogLines = Vec::new();
        let power = !battery_or_modern_standby();
        if !power {
            lines.push((L::t("Battery or Modern Standby found: USB and network power saving stay as Windows set them").to_string(), true));
        }
        let body = (|| -> Result<(), RegErr> {
            del(&mut bk, "HKLM", KERNEL_KEY, "ReservedCpuSets")?;
            for d in devs {
                let r = (|| -> Result<bool, RegErr> {
                    let s = d.slot.as_ref();
                    let placed = s.is_some_and(|s| s.mask != 0);
                    let mask = s.map(|s| s.mask).unwrap_or(0);
                    let what: String = match d.kind {
                        AutoKind::Usb if placed => {
                            irq(&mut bk, d, Some(3), Some(4), mask, &mut log, false)?;
                            lf!("processor {0}", Masks::ranges(mask))
                        }
                        AutoKind::Usb if s.is_some() => {
                            irq(&mut bk, d, Some(3), Some(0), 0, &mut log, false)?;
                            L::t("high priority, no core of its own").to_string()
                        }
                        AutoKind::Usb => {
                            irq(&mut bk, d, None, Some(0), 0, &mut log, false)?;
                            L::t("Windows decides").to_string()
                        }
                        AutoKind::Gpu if d.igpu => {
                            irq(&mut bk, d, None, Some(0), 0, &mut log, false)?;
                            L::t("integrated graphics: Windows decides").to_string()
                        }
                        AutoKind::Gpu if placed => {
                            irq(&mut bk, d, Some(3), Some(4), mask, &mut log, false)?;
                            lf!("processor {0}", Masks::ranges(mask))
                        }
                        AutoKind::Gpu => {
                            irq(&mut bk, d, Some(3), Some(0), 0, &mut log, false)?;
                            L::t("high priority, no core of its own").to_string()
                        }
                        AutoKind::Audio if d.display_audio => {
                            msi(&mut bk, d, &mut log)?;
                            L::t("display audio: MSI only").to_string()
                        }
                        AutoKind::Audio => {
                            irq(&mut bk, d, None, Some(0), 0, &mut log, false)?;
                            L::t("Windows decides").to_string()
                        }
                        AutoKind::Net if d.wifi => {
                            lines.push((lf!("{0}: Wi-Fi, not touched", d.name.as_str()), true));
                            return Ok(false);
                        }
                        AutoKind::Net if d.net_cx && placed => {
                            irq(&mut bk, d, Some(3), Some(4), mask, &mut log, false)?;
                            lf!("processor {0}", Masks::ranges(mask))
                        }
                        AutoKind::Net if !placed => {
                            irq(&mut bk, d, Some(3), Some(0), 0, &mut log, false)?;
                            L::t("high priority, no core of its own").to_string()
                        }
                        AutoKind::Net => {
                            let lp = mask.trailing_zeros() as i32;
                            if d.nic_mode == "RSS" {
                                irq(&mut bk, d, Some(3), None, 0, &mut log, true)?;
                            } else {
                                irq(&mut bk, d, Some(3), Some(4), mask, &mut log, false)?;
                            }
                            if d.nic_mode == "IRQ" {
                                rss_clear(&mut bk, d)?;
                            } else {
                                rss(&mut bk, d, lp)?;
                            }
                            lf!("processor {0}, mode {1}", lp, d.nic_mode.as_str())
                        }
                        AutoKind::Stor => {
                            lines.push((lf!("{0}: storage, never touched", d.name.as_str()), true));
                            return Ok(false);
                        }
                    };
                    if d.kind == AutoKind::Usb && power {
                        let mut hubs = vec![(d.id.clone(), d.driver_key.clone())];
                        hubs.extend(root_hubs(d.dev_inst));
                        for (id, drv) in hubs {
                            let dp = format!("{}{}\\Device Parameters", ENUM_ROOT, id);
                            set_d(&mut bk, "HKLM", &dp, "SelectiveSuspendEnabled", 0)?;
                            set_d(&mut bk, "HKLM", &dp, "EnhancedPowerManagementEnabled", 0)?;
                            power_off(&mut bk, &id, &drv, &mut log)?;
                        }
                    }
                    if d.kind == AutoKind::Net && power {
                        power_off(&mut bk, &d.id, &d.driver_key, &mut log)?;
                    }
                    lines.push((lf!("{0}: {1}", d.name.as_str(), what.as_str()), true));
                    Ok(true)
                })();
                if let Err(e) = r {
                    lines.push((lf!("{0}: {1}", d.name.as_str(), e.message().as_str()), false));
                }
            }
            if power && devs.iter().any(|d| d.kind == AutoKind::Usb) {
                usb_suspend_plan(&mut bk, 0, None, true);
            }
            Ok(())
        })();
        let escaped = match body {
            Ok(()) => None,
            Err(RegErr::Unauthorized(_)) => {
                lines.push((L::t("Access denied: run as administrator").to_string(), false));
                None
            }
            Err(RegErr::Other(m)) => Some(m),
        };
        IrqAuto::save(&bk)?; // finally
        if let Some(m) = escaped {
            return Err(m);
        }
        lines.extend(log);
        Ok(lines)
    }

    fn restore_entry(id: &str, enc: &str) -> Result<(), RegErr> {
        if !IrqAuto::allowed_entry(id, enc) {
            AppLog::add_sev(lf!("Device backup: {0} is not a value the program writes; it was not restored", id), Sev::Warn);
            return Ok(());
        }
        let f: Vec<&str> = id.split('|').collect();
        if enc.is_empty() {
            if let Some(k) = open(f[0], f[1], true)? {
                let n = hive_of(f[0])?.1;
                k.delete_value(f[2], false).map_err(|e| fail(n, f[1], e))?;
            }
            return Ok(());
        }
        let w = create(f[0], f[1])?;
        let n = hive_of(f[0])?.1;
        let v = if let Some(x) = enc.strip_prefix("d:") {
            RegValue::Dword(x.parse::<i32>().map_err(|e| RegErr::Other(e.to_string()))? as u32)
        } else if let Some(x) = enc.strip_prefix("b:") {
            let b: Result<Vec<u8>, _> = (0..x.len() / 2).map(|i| u8::from_str_radix(&x[2 * i..2 * i + 2], 16)).collect();
            RegValue::Binary(b.map_err(|e| RegErr::Other(e.to_string()))?)
        } else if let Some(x) = enc.strip_prefix("s:") {
            RegValue::Sz(x.to_string())
        } else {
            return Ok(());
        };
        w.set(f[2], &v).map_err(|e| fail(n, f[1], e))
    }

    fn create_key_hklm(path: &str) -> Result<(), String> {
        Key::create(Hive::LocalMachine, path).map(|_| ())
    }

    /// Puts back everything the automatic layout changed (part of "Restore all devices"). Err where C# lets an
    /// exception out (a deleted key that cannot be made again, the file that cannot be deleted).
    pub fn revert_extras() -> Result<(), String> {
        if !IrqAuto::has_backup() {
            return Ok(());
        }
        let mut bk = IrqAuto::load(); // only allowed entries (Clean)
        for id in bk.reg.keys().filter(|k| k.ends_with("|*") && k.starts_with("HKLM|")) {
            create_key_hklm(id.split('|').nth(1).unwrap_or(""))?;
        }
        for (id, enc) in bk.reg.iter().filter(|(k, _)| !k.ends_with("|*")) {
            let _ = restore_entry(id, enc);
        }
        if !bk.wmi.is_empty() {
            let want = bk.wmi.clone();
            let _ = super::super::wmi::device_enable(|key, _| want.get(key).copied());
        }
        if let Some(u) = bk.usb_suspend.clone() {
            if u.len() == 2 {
                usb_suspend_plan(&mut bk, u[0], Some(u[1]), false);
            }
        }
        let p = IrqAuto::backup_path()?;
        match std::fs::remove_file(&p) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.to_string()),
        }
    }

    /// First start of 1.12.0: an automatic layout written by an older version (it could touch storage controllers,
    /// force MSI, change laptop power saving and RawMouseThrottleDuration) is reverted from its backup, then the safe
    /// layout of this version is planned and written again, with the boot guard. Takes effect after a restart.
    pub fn redo_old_layout() -> Result<LogLines, String> {
        let mut log: LogLines = Vec::new();
        if !IrqAuto::has_backup() {
            return Ok(log);
        }
        revert_extras()?;
        log.push((L::t("The automatic device layout of an older version was reverted: it could touch storage, force MSI and change power saving on laptops").to_string(), true));
        if !Flags::open(Flags::DEVICES_WRITE) {
            log.push((L::t("Restart Windows: the device values are back as they were before the layout").to_string(), true));
            return Ok(log);
        }
        log.push((L::t("The new safe device layout is written again:").to_string(), true));
        log.extend(apply(&plan_now())?);
        log.push((L::t("Restart Windows: the new safe device layout starts working after the restart").to_string(), true));
        Ok(log)
    }

    fn storage_instance(id: &str) -> bool {
        let Some(k) = lm(&format!("{}{}", ENUM_ROOT, id)) else { return false };
        let mut desc = k.get_string("FriendlyName").or_else(|| k.get_string("DeviceDesc")).unwrap_or_default();
        if let Some(semi) = desc.rfind(';') {
            desc = desc[semi + 1..].to_string();
        }
        IrqAuto::is_storage(&k.get_string("Class").unwrap_or_default(), &desc, &k.get_string("Service").unwrap_or_default())
    }

    /// At every start: what older versions wrote and 1.12.0 no longer does is put back from the backup:
    /// RawMouseThrottleDuration (it only affects windows in the background) and every value of a storage controller.
    /// guess_raw_mouse: with no backup at all, a RawMouseThrottleDuration of exactly 20 (the value older versions wrote)
    /// is removed (the caller does that only once). Returns lines for the log.
    pub fn cleanup_legacy(guess_raw_mouse: bool) -> Result<Vec<String>, String> {
        let mut log = Vec::new();
        if IrqAuto::has_backup() {
            let mut bk = IrqAuto::load();
            let mut changed = false;
            if let Some(enc) = bk.reg.get(RAW_MOUSE_ID).cloned() {
                match restore_entry(RAW_MOUSE_ID, &enc) {
                    Ok(()) => {
                        bk.reg.shift_remove(RAW_MOUSE_ID);
                        changed = true;
                        log.push(L::t("RawMouseThrottleDuration is back to your previous value (it only affected windows in the background)").to_string());
                    }
                    Err(e) => log.push(lf!("RawMouseThrottleDuration: {0}", e.message().as_str())),
                }
            }
            let mut by_device: IndexMap<String, Vec<String>> = IndexMap::new();
            for k in bk.reg.keys() {
                if let Some(d) = IrqAuto::device_of_entry(k) {
                    by_device.entry(d).or_default().push(k.clone());
                }
            }
            for (dev, keys) in by_device {
                if !storage_instance(&dev) {
                    continue;
                }
                let r = (|| -> Result<(), RegErr> {
                    for id in keys.iter().filter(|k| k.ends_with("|*") && k.starts_with("HKLM|")) {
                        create_key_hklm(id.split('|').nth(1).unwrap_or("")).map_err(RegErr::Other)?;
                    }
                    for id in keys.iter().filter(|k| !k.ends_with("|*")) {
                        let enc = bk.reg.get(id).cloned().unwrap_or_default();
                        restore_entry(id, &enc)?;
                    }
                    for id in &keys {
                        bk.reg.shift_remove(id);
                    }
                    Ok(())
                })();
                match r {
                    Ok(()) => {
                        changed = true;
                        log.push(lf!("Storage controller {0}: its interrupt values are back as they were (storage is no longer touched); restart Windows", dev.as_str()));
                    }
                    Err(e) => log.push(lf!("Storage controller {0}: {1}", dev.as_str(), e.message().as_str())),
                }
            }
            if changed {
                if bk.is_empty() {
                    std::fs::remove_file(IrqAuto::backup_path()?).or_else(|e| if e.kind() == std::io::ErrorKind::NotFound { Ok(()) } else { Err(e.to_string()) })?;
                } else {
                    IrqAuto::save(&bk)?;
                }
            }
        } else if guess_raw_mouse {
            if let Some(k) = Key::open(Hive::CurrentUser, r"Control Panel\Mouse", true) {
                if get_int(&k, "RawMouseThrottleDuration") == Some(20) {
                    let _ = k.delete_value("RawMouseThrottleDuration", false);
                    log.push(L::t("RawMouseThrottleDuration 20 written by an older version was removed (it only affected windows in the background)").to_string());
                }
            }
        }
        Ok(log)
    }

    #[allow(dead_code)]
    pub fn non_default_affinity_of(id: &str) -> bool {
        non_default_affinity(id)
    }
}

#[cfg(windows)]
impl IrqAuto {
    pub fn discover() -> Vec<AutoDevice> {
        win::discover()
    }
    pub fn plan_now() -> Vec<AutoDevice> {
        win::plan_now()
    }
    pub fn apply(devs: &[AutoDevice]) -> Result<LogLines, String> {
        win::apply(devs)
    }
    pub fn revert_extras() -> Result<(), String> {
        win::revert_extras()
    }
    pub fn redo_old_layout() -> Result<LogLines, String> {
        win::redo_old_layout()
    }
    pub fn cleanup_legacy(guess_raw_mouse: bool) -> Result<Vec<String>, String> {
        win::cleanup_legacy(guess_raw_mouse)
    }
    pub fn battery_or_modern_standby() -> bool {
        win::battery_or_modern_standby()
    }
}

/// Other platforms (the Linux test host): no devices, nothing is written.
#[cfg(not(windows))]
impl IrqAuto {
    pub fn discover() -> Vec<AutoDevice> {
        Vec::new()
    }
    pub fn plan_now() -> Vec<AutoDevice> {
        Vec::new()
    }
    pub fn apply(_devs: &[AutoDevice]) -> Result<LogLines, String> {
        Err("not supported on this platform".into())
    }
    pub fn revert_extras() -> Result<(), String> {
        Ok(())
    }
    pub fn redo_old_layout() -> Result<LogLines, String> {
        Ok(Vec::new())
    }
    pub fn cleanup_legacy(_guess_raw_mouse: bool) -> Result<Vec<String>, String> {
        Ok(Vec::new())
    }
    pub fn battery_or_modern_standby() -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::base::testrand::{iterations, Rand};

    /// IrqLayoutTests.Device_recognition_follows_the_rules
    #[test]
    fn device_recognition_follows_the_rules() {
        assert!(IrqAuto::excluded(r"PCI\VEN_1022&DEV_14DB&SUBSYS_0", "PCI bridge", "System"));
        assert!(IrqAuto::excluded(r"BTH\X", "Intel(R) Wireless Bluetooth(R)", "Bluetooth"));
        assert!(IrqAuto::excluded(r"ROOT\NET\0000", "Hyper-V Virtual Ethernet Adapter", "Net"));
        assert!(!IrqAuto::excluded(r"PCI\VEN_10DE&DEV_2D05", "NVIDIA GeForce RTX 5060", "Display"));
        assert_eq!(Some(AutoKind::Usb), IrqAuto::classify("USB", "AMD USB 3.10 eXtensible Host Controller - 1.10 (Microsoft)"));
        assert_eq!(Some(AutoKind::Stor), IrqAuto::classify("SCSIAdapter", "Standard NVM Express Controller"));
        assert_eq!(Some(AutoKind::Audio), IrqAuto::classify("MEDIA", "Realtek(R) Audio"));
        assert_eq!(None, IrqAuto::classify("System", "AMD GPIO Controller"));
        assert!(IrqAuto::is_integrated_gpu("", "AMD Radeon(TM) Graphics"));
        assert!(!IrqAuto::is_integrated_gpu("", "NVIDIA GeForce RTX 5060"));
        assert!(IrqAuto::is_wifi("", "Intel(R) Wi-Fi 6E AX211 160MHz", ""));
        assert!(!IrqAuto::is_wifi("", "Realtek PCIe 2.5GbE Family Controller", "rt25cx21x64"));
        assert!(IrqAuto::is_display_audio_device(r"HDAUDIO\FUNC_01&VEN_10DE&DEV_0099", "NVIDIA High Definition Audio"));
        assert!(IrqAuto::is_spdif("Realtek Digital Output (S/PDIF)"));
        assert!(!IrqAuto::is_spdif("LG ULTRAGEAR (NVIDIA High Definition Audio) HDMI"));
    }

    /// IrqLayoutTests.Two_gpus_with_a_geforce_make_the_plain_radeon_integrated
    #[test]
    fn two_gpus_with_a_geforce_make_the_plain_radeon_integrated() {
        let mut g = vec![
            AutoDevice { id: r"PCI\VEN_10DE&DEV_2D05".into(), name: "NVIDIA GeForce RTX 5060".into(), kind: AutoKind::Gpu, ..Default::default() },
            AutoDevice { id: r"PCI\VEN_1002&DEV_164E".into(), name: "AMD Radeon(TM) Graphics".into(), kind: AutoKind::Gpu, ..Default::default() },
        ];
        {
            let mut refs: Vec<&mut AutoDevice> = g.iter_mut().collect();
            IrqAuto::mark_integrated(&mut refs);
        }
        assert!(!g[0].igpu);
        assert!(g[1].igpu);
    }

    /// IrqLayoutTests.Hid_roles_from_usage_class_and_product
    #[test]
    fn hid_roles_from_usage_class_and_product() {
        assert_eq!(Some(IrqRole::Mouse), IrqAuto::hid_role(1, 2, "HIDClass", "HID-compliant mouse", "G Pro Wireless"));
        assert_eq!(Some(IrqRole::Keyboard), IrqAuto::hid_role(1, 6, "Keyboard", "HID Keyboard Device", ""));
        assert_eq!(Some(IrqRole::Controller), IrqAuto::hid_role(1, 5, "HIDClass", "HID-compliant game controller", "Xbox Wireless Controller"));
        assert_eq!(None, IrqAuto::hid_role(0xC, 1, "HIDClass", "HID-compliant consumer control device", ""));
        assert_eq!(None, IrqAuto::hid_role(1, 2, "HIDClass", "x", "Xbox Wireless Controller")); // product says pad, usage says mouse: no role
    }

    /// IrqLayoutTests.Nic_mode_and_cppc_parsing
    #[test]
    fn nic_mode_and_cppc_parsing() {
        assert_eq!("BOTH", IrqAuto::choose_nic_mode(true, true, false, 4, 4));
        assert_eq!("BOTH", IrqAuto::choose_nic_mode(true, false, false, 2, 1));
        assert_eq!("RSS", IrqAuto::choose_nic_mode(true, false, false, 1, 1));
        assert_eq!("IRQ", IrqAuto::choose_nic_mode(false, false, false, 0, 0));
        let xml = "<Events><Event><Data Name='Group'>0</Data><Data Name='Number'>3</Data><Data Name='MaximumPerformancePercent'>120</Data></Event>".to_string()
            + "<Event><Data Name='Group'>0</Data><Data Name='Number'>3</Data><Data Name='MaximumPerformancePercent'>99</Data></Event></Events>";
        assert_eq!(120, IrqAuto::parse_cppc(&xml).unwrap()[&3]); // newest first wins
    }

    #[test]
    fn cppc_skips_other_groups_and_reads_attributes() {
        let xml = "<Event xmlns='x'><Data Name=\"Group\">1</Data><Data Name=\"Number\">0</Data><Data Name=\"MaximumPerformancePercent\">150</Data></Event>\n\
                   <Event><Data Name=\"Number\">2</Data><Data Name=\"MaximumPerformancePercent\">140</Data></Event>";
        let m = IrqAuto::parse_cppc(xml).unwrap();
        assert!(!m.contains_key(&0));
        assert_eq!(m[&2], 140);
        assert!(IrqAuto::parse_cppc("<Event><Data Name='Number'>99999999999</Data><Data Name='MaximumPerformancePercent'>1</Data></Event>").is_err());
    }

    /// FuzzTests.TextParsersNeverThrow (the IrqAuto part)
    #[test]
    fn text_parsers_never_throw() {
        let alphabet: Vec<char> = "{}[]\":,0123456789abcdefABCDEF-+.eE \n\t\\/xyzDOSTАБВ\u{0}\u{ffff}😀".chars().collect();
        let mut r = Rand::new(1604);
        for _ in 0..iterations() {
            let len = r.next(400);
            let s: String = (0..len).map(|_| alphabet[r.next(alphabet.len() as i32) as usize]).collect();
            let _ = IrqAuto::parse_cppc(&s);
            let _ = IrqAuto::parse_cppc(&format!("<cppc>{}</cppc>", s));
        }
    }

    /// LayoutInPlaceTests.OnlyAFullMatchCountsAsInPlace
    #[test]
    fn only_a_full_match_counts_as_in_place() {
        assert!(IrqAuto::layout_in_place(&[(Some(0b100), 0b100), (None, 0), (Some(0), 0)]));
        assert!(!IrqAuto::layout_in_place(&[(Some(0b100), 0b1000)])); // another core
        assert!(!IrqAuto::layout_in_place(&[(None, 0b100)])); // not written yet
        assert!(!IrqAuto::layout_in_place(&[(Some(0b100), 0)])); // pinned, plan says Windows
        assert!(!IrqAuto::layout_in_place(&[])); // nothing to compare
    }

    /// SafetyTests.StorageControllersAreRecognisedAndNeverListed (the IrqAuto part)
    #[test]
    fn storage_controllers_are_recognised_and_never_listed() {
        assert!(IrqAuto::is_storage("SCSIAdapter", "Standard NVM Express Controller", "stornvme"));
        assert!(IrqAuto::is_storage("HDC", "Standard SATA AHCI Controller", "storahci"));
        assert!(IrqAuto::is_storage("System", "Intel(R) RST VMD Controller", "iaStorVD"));
        assert!(IrqAuto::is_storage("", "AMD-RAID Bottom Device", "rcbottom"));
        assert!(IrqAuto::is_storage("", "Samsung NVMe Controller", ""));
        assert!(IrqAuto::is_storage("", "x", "secnvme"));
        assert!(!IrqAuto::is_storage("Display", "NVIDIA GeForce RTX 5060", "nvlddmkm"));
        assert!(!IrqAuto::is_storage("USB", "AMD USB 3.10 eXtensible Host Controller - 1.10 (Microsoft)", "USBXHCI"));
        assert!(!IrqAuto::is_storage("Net", "Realtek PCIe 2.5GbE Family Controller", "rt25cx21x64"));
        assert!(!IrqAuto::is_storage("MEDIA", "Realtek(R) Audio", "IntcAzAudAddService"));
        assert_eq!(
            Some(r"PCI\VEN_144D&DEV_A80A\4&1".to_string()),
            IrqAuto::device_of_entry(r"HKLM|SYSTEM\CurrentControlSet\Enum\PCI\VEN_144D&DEV_A80A\4&1\Device Parameters\Interrupt Management\Affinity Policy|DevicePriority")
        );
        assert_eq!(None, IrqAuto::device_of_entry(r"HKCU|Control Panel\Mouse|RawMouseThrottleDuration"));
    }

    /// SafetyTests.MsiIsSwitchedOnOnlyWhenTheDeviceOffersIt
    #[test]
    fn msi_is_switched_on_only_when_the_device_offers_it() {
        assert!(IrqAuto::msi_allowed(None, true, false)); // already message-signalled
        assert!(IrqAuto::msi_allowed(Some(2), false, false)); // PCI: MSI
        assert!(IrqAuto::msi_allowed(Some(5), false, false)); // PCI: line + MSI-X
        assert!(!IrqAuto::msi_allowed(Some(1), false, true)); // PCI: line only, the property wins
        assert!(IrqAuto::msi_allowed(None, false, true)); // unknown, the driver set a limit
        assert!(!IrqAuto::msi_allowed(None, false, false)); // unsure: no
    }

    /// SafetyTests.PowerSavingIsLeftAloneWithABatteryOrModernStandby
    #[test]
    fn power_saving_is_left_alone_with_a_battery_or_modern_standby() {
        assert!(IrqAuto::power_saving_allowed(128, false, false)); // desktop: no system battery
        assert!(!IrqAuto::power_saving_allowed(1, true, false)); // laptop, battery high
        assert!(!IrqAuto::power_saving_allowed(8, true, false)); // charging
        assert!(!IrqAuto::power_saving_allowed(128, false, true)); // Modern Standby
        assert!(!IrqAuto::power_saving_allowed(255, false, false)); // unknown: counts as a battery
    }

    const GPU: &str = r"SYSTEM\CurrentControlSet\Enum\PCI\VEN_10DE&DEV_2D04&SUBSYS_00000000&REV_A1\4&2f9a8b1&0&0008";
    fn ap() -> String {
        format!(r"{}\Device Parameters\Interrupt Management\Affinity Policy", GPU)
    }
    const NIC: &str = r"SYSTEM\CurrentControlSet\Control\Class\{4d36e972-e325-11ce-bfc1-08002be10318}\0001";

    /// SecurityTests.DeviceLayoutBackupAcceptsOnlyWhatTheLayoutWrites (the IrqAuto part)
    #[test]
    fn device_layout_backup_accepts_only_what_the_layout_writes() {
        let ap = ap();
        assert!(IrqAuto::allowed_entry(&format!("HKLM|{}|DevicePolicy", ap), ""));
        assert!(IrqAuto::allowed_entry(&format!("HKLM|{}|DevicePolicy", ap), "d:4"));
        assert!(IrqAuto::allowed_entry(&format!("HKLM|{}|AssignmentSetOverride", ap), "b:0400000000000000"));
        assert!(IrqAuto::allowed_entry(&format!("HKLM|{}|*RssBaseProcNumber", NIC), "s:2"));
        assert!(IrqAuto::allowed_entry(&format!("HKLM|{}|PnPCapabilities", NIC), "d:24"));
        assert!(IrqAuto::allowed_entry(r"HKLM|SYSTEM\CurrentControlSet\Control\Session Manager\Kernel|ReservedCpuSets", "b:FEFF000000000000"));
        assert!(IrqAuto::allowed_entry(r"HKCU|Control Panel\Mouse|RawMouseThrottleDuration", "d:20"));
        assert!(IrqAuto::allowed_entry(&format!(r"HKLM|{}\Device Parameters\Interrupt Management\Priority|*", GPU), "s:key"));

        // Planted: another key, another value name, a type or range the layout never writes, another hive.
        assert!(!IrqAuto::allowed_entry(r"HKLM|SYSTEM\CurrentControlSet\Services\evil|ImagePath", r"s:C:\Users\x\evil.exe"));
        assert!(!IrqAuto::allowed_entry(&format!("HKLM|{}|ImagePath", ap), "s:x"));
        assert!(!IrqAuto::allowed_entry(&format!("HKLM|{}|DevicePolicy", ap), "b:04"));
        assert!(!IrqAuto::allowed_entry(&format!("HKLM|{}|DevicePolicy", ap), "s:4"));
        assert!(!IrqAuto::allowed_entry(&format!("HKLM|{}|DevicePolicy", ap), "d:99999999999"));
        assert!(!IrqAuto::allowed_entry(&format!("HKU|{}|DevicePolicy", ap), "d:4"));
        assert!(!IrqAuto::allowed_entry(r"HKLM|SYSTEM\CurrentControlSet\Control\Class\..\..\Services\evil|PnPCapabilities", "d:1"));
        assert!(!IrqAuto::allowed_entry(r"HKCU|Software\Microsoft\Windows\CurrentVersion\Run|evil", "s:evil.exe"));
        assert!(!IrqAuto::allowed_entry(r"HKLM|SYSTEM\CurrentControlSet\Enum\PCI\A|B\Device Parameters\Interrupt Management\Affinity Policy|DevicePolicy", "d:4"));
        assert!(!IrqAuto::allowed_entry(r"HKLM|SYSTEM\CurrentControlSet\Services\evil\Device Parameters|SelectiveSuspendEnabled", "d:0"));
        assert!(!IrqAuto::allowed_entry(&format!("HKLM|{}|AssignmentSetOverride", ap), &format!("b:{}", "F".repeat(200))));
        // the manual device backup's value check (Devices.ValidBackupDevice uses it)
        assert!(IrqAuto::valid_irq_value("AssignmentSetOverride", "b:04"));
        assert!(!IrqAuto::valid_irq_value("DevicePriority", "s:7"));
        assert!(IrqAuto::instance_rx_match(r"PCI\VEN_10DE&DEV_2D04&SUBSYS_00000000&REV_A1\4&2f9a8b1&0&0008"));
    }

    /// SecurityTests.PlantedDeviceBackupEntriesAreDroppedWhenTakenOver
    #[test]
    fn planted_device_backup_entries_are_dropped_when_taken_over() {
        let ap = ap();
        let planted = serde_json::json!({
            "Reg": {
                format!("HKLM|{}|DevicePolicy", ap): "",
                r"HKLM|SYSTEM\CurrentControlSet\Services\evil|ImagePath": r"s:C:\evil.exe",
                format!("HKLM|{}|DevicePriority", ap): "s:250",
            },
            "Wmi": { r"PCI\VEN_8086&DEV_7AE0\3&11583659&0&A0_0": true, "evil|x": false },
            "UsbSuspend": [1, 7],
        })
        .to_string();
        let mut dropped = Vec::new();
        let json = IrqAuto::import_backup(&planted, None, Some(&mut dropped)).unwrap().expect("json");
        assert!(json.contains("DevicePolicy"));
        assert!(!json.contains("evil"));
        assert!(!json.contains("DevicePriority"));
        assert!(!json.contains("UsbSuspend\":["));
        assert_eq!(4, dropped.len()); // ImagePath, the priority as text, the WMI name with "|", the suspend pair

        // A backup already in the state folder wins over the old one.
        let first = serde_json::json!({ "Reg": { format!("HKLM|{}|DevicePolicy", ap): "d:0" } }).to_string();
        let existing = IrqAuto::import_backup(&first, None, None).unwrap().expect("json");
        let mut d2 = Vec::new();
        let merged = IrqAuto::import_backup(&planted, Some(&existing), Some(&mut d2)).unwrap().expect("json");
        assert!(merged.contains("\"d:0\""));
        assert!(IrqAuto::import_backup("{ not json", None, None).is_err());
        assert_eq!(IrqAuto::import_backup("{}", None, None).unwrap(), None);
    }

    #[test]
    fn backup_json_has_the_system_text_json_shape() {
        let mut b = Backup::default();
        b.reg.insert(r"HKLM|A\B&C|*".into(), "s:key".into());
        b.wmi.insert(r"PCI\X".into(), true);
        assert_eq!(b.to_json(), "{\"Reg\":{\"HKLM|A\\\\B\\u0026C|*\":\"s:key\"},\"Wmi\":{\"PCI\\\\X\":true},\"UsbSuspend\":null}");
        b.usb_suspend = Some(vec![1, 0]);
        assert!(b.to_json().ends_with("\"UsbSuspend\":[1,0]}"));
    }

    #[test]
    fn assign_gives_slots_to_usb_with_a_role_discrete_gpus_and_wired_nics() {
        let x = crate::topo::synth::x7800x3d();
        let mut devs = vec![
            AutoDevice { id: r"PCI\USB1".into(), name: "USB".into(), kind: AutoKind::Usb, role: Some(IrqRole::Mouse), ..Default::default() },
            AutoDevice { id: r"PCI\USB2".into(), name: "USB2".into(), kind: AutoKind::Usb, ..Default::default() },
            AutoDevice { id: r"PCI\GPU".into(), name: "NVIDIA GeForce RTX 5060".into(), kind: AutoKind::Gpu, ..Default::default() },
            AutoDevice { id: r"PCI\WIFI".into(), name: "Wi-Fi".into(), kind: AutoKind::Net, wifi: true, ..Default::default() },
        ];
        IrqAuto::assign(&mut devs, Some(&x), &HashMap::new(), false);
        assert_eq!(devs[0].slot.as_ref().map(|s| s.role), Some(IrqRole::Mouse));
        assert!(devs[1].slot.is_none());
        assert_eq!(devs[2].slot.as_ref().map(|s| s.role), Some(IrqRole::Gpu));
        assert!(devs[3].slot.is_none());
        let expect = IrqLayout::plan(Some(&x), vec![IrqSlot::new(r"PCI\USB1", "USB", IrqRole::Mouse), IrqSlot::new(r"PCI\GPU", "NVIDIA GeForce RTX 5060", IrqRole::Gpu)], Some(&HashMap::new()), false);
        assert_eq!(devs[0].slot.as_ref().unwrap().mask, expect[0].mask);
        assert_eq!(devs[2].slot.as_ref().unwrap().mask, expect[1].mask);
    }
}
