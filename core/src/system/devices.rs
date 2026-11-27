//! PCI devices and their interrupt affinity (section 7), port of `Devices.cs`. Reading is used by the game mask
//! (pinned interrupts are kept off the game's cores); writing is a separate, manual operation: backup -> registry ->
//! device restart -> read-back -> rollback on mismatch. The Auto path never writes.

use super::boot_guard::BootGuard;
use super::deps;
#[cfg_attr(not(windows), allow(unused_imports))]
use super::util::{create_parent, from_hex, hex_upper, move_overwrite, parse_stj_date, read_all_text, stj_date, write_all_text};
use crate::base::app_log::{AppLog, Sev};
use crate::base::ci::{self, CiMap, CiMapExt};
use crate::base::flags::Flags;
use crate::base::hooks::Hook;
use crate::base::l::L;
use crate::base::model::{IrqConfig, PciDevice};
use crate::base::state_dir::store_dir;
use crate::base::time::DateTime;
use crate::lf;
use crate::topo::reservation::Reservation;
use indexmap::IndexMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicI32, Ordering};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IrqKind {
    Dpc,
    Isr,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IrqResource {
    pub vector: i32,
    pub affinity: u64,
    pub group: u16,
    /// true = MSI
    pub message: bool,
    pub kind: IrqKind,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DeviceFacts {
    pub driver_file: String,
    /// From the Devices state file, not the registry.
    pub restarted: DateTime,
    /// `Reservation::boot_utc()`
    pub boot: DateTime,
    pub resources: Vec<IrqResource>,
    /// Pins of the other devices.
    pub others: Vec<(String, u64)>,
}

impl Default for DeviceFacts {
    fn default() -> Self {
        DeviceFacts { driver_file: String::new(), restarted: DateTime::MIN, boot: DateTime::MIN, resources: Vec::new(), others: Vec::new() }
    }
}

// Test hooks.
pub static LIST_HOOK: Hook<dyn Fn() -> Vec<PciDevice> + Send + Sync> = Hook::new();
/// devInst -> (message interrupts, line interrupts, numbers)
pub static ALLOC_HOOK: Hook<dyn Fn(u32) -> (i32, i32, Vec<i32>) + Send + Sync> = Hook::new();
/// instance id -> raw IRQ resource descriptor data
pub static RESOURCE_HOOK: Hook<dyn Fn(&str) -> Option<Vec<u8>> + Send + Sync> = Hook::new();
/// instance id -> registry values
pub static READ_HOOK: Hook<dyn Fn(&str) -> IrqConfig + Send + Sync> = Hook::new();
pub static STATE_PATH_HOOK: Hook<dyn Fn() -> PathBuf + Send + Sync> = Hook::new();

static WRITE_CALLS: AtomicI32 = AtomicI32::new(0);

#[cfg_attr(not(windows), allow(dead_code))]
const ENUM_ROOT: &str = r"SYSTEM\CurrentControlSet\Enum\";
#[cfg_attr(not(windows), allow(dead_code))]
const POLICY_SUB: &str = r"\Device Parameters\Interrupt Management\Affinity Policy";
#[cfg_attr(not(windows), allow(dead_code))]
const MSI_SUB: &str = r"\Device Parameters\Interrupt Management\MessageSignaledInterruptProperties";
const BACKED_UP_NAMES: [&str; 5] = ["AssignmentSetOverride", "DevicePolicy", "DevicePriority", "MSISupported", "MessageNumberLimit"];

// ---- private copies of IrqAuto's pure checks (IrqAuto.cs; the etw area owns the original) ------------------

/// `\w` of .NET regex (letters, digits, connector punctuation, marks): enough for `\b`.
fn word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// `\bWORD\b` (ignore case) somewhere in s (s and w already lower case).
fn has_word(s: &[char], w: &str) -> bool {
    let w: Vec<char> = w.chars().collect();
    if w.len() > s.len() {
        return false;
    }
    (0..=s.len() - w.len()).any(|i| s[i..i + w.len()] == w[..] && (i == 0 || !word(s[i - 1])) && (i + w.len() == s.len() || !word(s[i + w.len()])))
}

/// `NVM\s*Express|NVMe|\bSATA\b|AHCI|\bRAID\b|SCSI|Storage|\bVMD\b|\bIDE\b|Mass Storage|Disk` (ignore case).
fn stor_name(name: &str) -> bool {
    let l = ci::lower(name);
    for part in ["nvme", "ahci", "scsi", "storage", "mass storage", "disk"] {
        if l.contains(part) {
            return true;
        }
    }
    let c: Vec<char> = l.chars().collect();
    for i in 0..c.len() {
        if c[i..].starts_with(&['n', 'v', 'm']) {
            let mut j = i + 3;
            while j < c.len() && c[j].is_whitespace() {
                j += 1;
            }
            if c[j..].starts_with(&['e', 'x', 'p', 'r', 'e', 's', 's']) {
                return true;
            }
        }
    }
    ["sata", "raid", "vmd", "ide"].iter().any(|w| has_word(&c, w))
}

/// `^(stornvme|storahci|...|usbstor)$` (ignore case; `$` also before a final "\n").
fn stor_service(service: &str) -> bool {
    let s = service.strip_suffix('\n').unwrap_or(service);
    let l = ci::lower(s);
    const EXACT: [&str; 21] = [
        "stornvme", "storahci", "storport", "iavroc", "rcraid", "rcbottom", "amd_sata", "amdsata", "amd_xata", "amdxata", "secnvme", "msahci", "pciide", "intelide",
        "atapi", "vmd", "arcsas", "stexstor", "disk", "uaspstor", "usbstor",
    ];
    const PREFIX: [&str; 8] = ["stornvm", "iastor", "nvme", "samsungnvme", "megasas", "lsi_", "percsas", "mvs"];
    EXACT.contains(&l.as_str()) || PREFIX.iter().any(|p| l.starts_with(p) && l[p.len()..].chars().all(word))
}

/// `IrqAuto.IsStorage`: a storage controller (NVMe, SATA/AHCI, RAID, VMD, SCSI) or a disk: never touched by the
/// interrupt layout or a manual pin. The boot disk always hangs on one of these, so a wrong value there can stop
/// Windows from starting.
fn irq_is_storage(cls: &str, name: &str, service: &str) -> bool {
    for c in ["SCSIAdapter", "HDC", "IDE", "Storage", "DiskDrive", "Volume", "StorageVolume"] {
        if ci::eq(cls, c) {
            return true;
        }
    }
    stor_name(name) || stor_service(service)
}

/// `IrqAuto.InstanceRx`: ENUMERATOR\DEVICE\INSTANCE, nothing that could reach outside the device's key.
fn instance_rx(s: &str) -> bool {
    let s = s.strip_suffix('\n').unwrap_or(s);
    let p: Vec<&str> = s.split('\\').collect();
    if p.len() != 3 {
        return false;
    }
    let rest = |x: &str| (1..=200).contains(&x.chars().count()) && x.chars().all(|c| c.is_ascii_alphanumeric() || "&_.#{}+- ".contains(c));
    (1..=32).contains(&p[0].chars().count()) && p[0].chars().all(|c| c.is_ascii_alphanumeric() || c == '_') && rest(p[1]) && rest(p[2])
}

fn dword_enc(enc: &str, lo: i64, hi: i64) -> bool {
    enc.starts_with("d:") && crate::base::settings::parse_int_leading_sign(&enc[2..]).map(|v| v >= lo && v <= hi).unwrap_or(false)
}

fn bin_enc(enc: &str, max_bytes: usize) -> bool {
    enc.starts_with("b:") && enc.len() - 2 <= max_bytes * 2 && (enc.len() - 2) % 2 == 0 && enc[2..].chars().all(|c| c.is_ascii_hexdigit())
}

/// `IrqAuto.ValidIrqValue`: the interrupt values of a device's Affinity Policy / MSI key, each in its registry type.
fn valid_irq_value(name: &str, enc: &str) -> bool {
    if enc.is_empty() {
        return BACKED_UP_NAMES.contains(&name);
    }
    match name {
        "DevicePriority" | "DevicePolicy" | "MSISupported" | "MessageNumberLimit" => dword_enc(enc, i32::MIN as i64, i32::MAX as i64),
        "AssignmentSetOverride" => bin_enc(enc, 64),
        _ => false,
    }
}

/// `IrqAuto.HasBackup` (irq-auto-backup.json exists; false when the state folder cannot be used).
pub(crate) fn irq_auto_has_backup() -> bool {
    store_dir().map(|d| d.join(deps::names::IRQ_AUTO_BACKUP).is_file()).unwrap_or(false)
}

/// The manual device backup: PCI instance -> value name -> "" (absent) / "d:int" / "b:hex". C# Dictionary order.
#[derive(Default, Clone)]
struct Backup {
    devices: IndexMap<String, Option<IndexMap<String, Option<String>>>>,
}

fn parse_backup(json: &str) -> Result<Option<Backup>, String> {
    let v: serde_json::Value = serde_json::from_str(json).map_err(|e| e.to_string())?;
    let o = match v {
        serde_json::Value::Null => return Ok(None),
        serde_json::Value::Object(o) => o,
        _ => return Err("The JSON value could not be converted to Backup.".into()),
    };
    let mut b = Backup::default();
    match o.get("Devices") {
        None | Some(serde_json::Value::Null) => {}
        Some(serde_json::Value::Object(d)) => {
            for (pdo, vals) in d {
                let inner = match vals {
                    serde_json::Value::Null => None,
                    serde_json::Value::Object(m) => {
                        let mut im = IndexMap::new();
                        for (k, x) in m {
                            im.insert(
                                k.clone(),
                                match x {
                                    serde_json::Value::Null => None,
                                    serde_json::Value::String(s) => Some(s.clone()),
                                    _ => return Err("The JSON value could not be converted to System.String.".into()),
                                },
                            );
                        }
                        Some(im)
                    }
                    _ => return Err("The JSON value could not be converted to Dictionary.".into()),
                };
                b.devices.insert(pdo.clone(), inner);
            }
        }
        Some(_) => return Err("The JSON value could not be converted to Dictionary.".into()),
    }
    Ok(Some(b))
}

fn backup_json(b: &Backup) -> String {
    let mut devs = serde_json::Map::new();
    for (pdo, vals) in &b.devices {
        let v = match vals {
            None => serde_json::Value::Null,
            Some(m) => {
                let mut o = serde_json::Map::new();
                for (k, x) in m {
                    o.insert(k.clone(), x.clone().map(serde_json::Value::String).unwrap_or(serde_json::Value::Null));
                }
                serde_json::Value::Object(o)
            }
        };
        devs.insert(pdo.clone(), v);
    }
    // serde_json's map is sorted; the C# file keeps insertion order. Both read the same.
    let mut root = serde_json::Map::new();
    root.insert("Devices".into(), serde_json::Value::Object(devs));
    serde_json::Value::Object(root).to_string()
}

/// `Devices` (static class).
pub struct Devices;

impl Devices {
    pub const BACKUP_NAME: &'static str = "irq-backup.json";

    pub fn backup_path() -> Result<PathBuf, String> {
        Ok(store_dir()?.join(Self::BACKUP_NAME))
    }

    /// A backup file exists (false when the state folder cannot be used).
    pub fn has_backup() -> bool {
        Self::backup_path().map(|p| p.is_file()).unwrap_or(false)
    }

    /// The present PCI devices (no name or no instance: skipped).
    pub fn list() -> Vec<PciDevice> {
        if let Some(h) = LIST_HOOK.get() {
            return h();
        }
        #[cfg(windows)]
        {
            win::list()
        }
        #[cfg(not(windows))]
        {
            Vec::new()
        }
    }

    pub fn list_pci_names() -> Vec<(String, String)> {
        Self::list().into_iter().map(|d| (d.name, d.pdo)).collect()
    }

    /// The device's interrupt settings in the registry.
    pub fn read_irq(instance: &str) -> IrqConfig {
        if let Some(h) = READ_HOOK.get() {
            return h(instance);
        }
        #[cfg(windows)]
        {
            win::read_irq(instance)
        }
        #[cfg(not(windows))]
        {
            IrqConfig::default()
        }
    }

    /// Devices whose interrupts are pinned to given processors (AssignmentSetOverride != 0).
    pub fn pinned_irqs() -> Vec<(String, u64)> {
        let mut res = Vec::new();
        for (name, inst) in Self::list_pci_names() {
            let c = Self::read_irq(&inst);
            if let Some(a) = c.assign.filter(|a| *a != 0) {
                res.push((name, a));
            }
        }
        res
    }

    /// The interrupts Windows allocated to a device: (message, line, numbers).
    pub fn allocated(dev_inst: u32) -> (i32, i32, Vec<i32>) {
        if let Some(h) = ALLOC_HOOK.get() {
            return h(dev_inst);
        }
        let res = Self::resources(dev_inst);
        (res.iter().filter(|r| r.message).count() as i32, res.iter().filter(|r| !r.message).count() as i32, res.iter().map(|r| r.vector).collect())
    }

    /// Allocated interrupt resources of a device (CfgMgr32, ALLOC_LOG_CONF, ResType_IRQ).
    pub fn resources(dev_inst: u32) -> Vec<IrqResource> {
        #[cfg(windows)]
        {
            win::resources(dev_inst)
        }
        #[cfg(not(windows))]
        {
            let _ = dev_inst;
            Vec::new()
        }
    }

    /// IRQ_DES_64 records (24 bytes each): Count, Type, Flags, Alloc_Num, Affinity. Message signalled interrupts carry
    /// the top bit of Alloc_Num. CfgMgr32 lists the interrupt (ISR) resources; DPC routines are not resources and are
    /// not visible here.
    pub fn parse_irq(data: Option<&[u8]>) -> Vec<IrqResource> {
        let mut list = Vec::new();
        let Some(data) = data else { return list };
        let mut off = 0usize;
        while off + 16 <= data.len() {
            let alloc = u32::from_le_bytes(data[off + 12..off + 16].try_into().unwrap_or([0; 4]));
            let aff = if off + 24 <= data.len() { u64::from_le_bytes(data[off + 16..off + 24].try_into().unwrap_or([0; 8])) } else { 0 };
            list.push(IrqResource { vector: alloc as i32, affinity: aff, group: 0, message: alloc & 0x8000_0000 != 0, kind: IrqKind::Isr });
            off += 24;
        }
        list
    }

    /// Raw IRQ descriptor data of a device (tests).
    pub fn resource(instance: &str) -> Option<Vec<u8>> {
        RESOURCE_HOOK.get().and_then(|h| h(instance))
    }

    // ---- facts (section 20): read-only diagnostics, used by the Report only ------------------------------------

    pub fn state_path() -> Result<PathBuf, String> {
        if let Some(h) = STATE_PATH_HOOK.get() {
            return Ok(h());
        }
        Ok(store_dir()?.join("devices-state.json"))
    }

    fn read_state() -> IndexMap<String, DateTime> {
        let r = (|| -> Option<IndexMap<String, DateTime>> {
            let p = Self::state_path().ok()?;
            if !p.is_file() {
                return None;
            }
            let v: serde_json::Value = serde_json::from_str(&read_all_text(&p).ok()?).ok()?;
            let mut m = IndexMap::new();
            match v {
                serde_json::Value::Object(o) => {
                    for (k, x) in o {
                        m.insert(k, parse_stj_date(x.as_str()?)?);
                    }
                }
                _ => return None,
            }
            Some(m)
        })();
        r.unwrap_or_default()
    }

    #[cfg_attr(not(windows), allow(dead_code))]
    fn mark_restarted(pdo: &str) {
        let _ = (|| -> Result<(), String> {
            let mut st = Self::read_state();
            st.insert(pdo.to_string(), DateTime::utc_now());
            let p = Self::state_path()?;
            create_parent(&p).map_err(|e| e.to_string())?;
            let mut o = serde_json::Map::new();
            for (k, v) in &st {
                o.insert(k.clone(), serde_json::Value::String(stj_date(*v)));
            }
            write_all_text(&p, &serde_json::Value::Object(o).to_string()).map_err(|e| e.to_string())
        })();
    }

    fn driver_file_of(service: &str) -> String {
        if service.is_empty() {
            return String::new();
        }
        #[cfg(windows)]
        {
            use crate::native::reg::{Hive, Key, RegValue};
            let Some(k) = Key::open(Hive::LocalMachine, &format!(r"SYSTEM\CurrentControlSet\Services\{}", service), false) else { return String::new() };
            match k.get("ImagePath") {
                Some(RegValue::Sz(p)) | Some(RegValue::ExpandSz(p)) => {
                    let p = p.trim_matches('"');
                    let name = p.rsplit(['\\', '/']).next().unwrap_or("");
                    // "C:file" (no separator): the name after the drive.
                    let b = name.as_bytes();
                    if p == name && b.len() >= 2 && b[1] == b':' && b[0].is_ascii_alphabetic() { name[2..].to_string() } else { name.to_string() }
                }
                _ => String::new(),
            }
        }
        #[cfg(not(windows))]
        {
            String::new()
        }
    }

    pub fn facts(dev: &PciDevice, all_pins: Option<&[(String, u64)]>) -> DeviceFacts {
        let mut f = DeviceFacts { driver_file: Self::driver_file_of(&dev.service), ..Default::default() };
        f.boot = Reservation::boot_utc();
        if let Some(t) = Self::read_state().get(&dev.pdo) {
            f.restarted = *t;
        }
        f.resources = match Self::resource(&dev.pdo) {
            Some(raw) => Self::parse_irq(Some(&raw)),
            None => Self::resources(dev.dev_inst),
        };
        f.others = all_pins.unwrap_or(&[]).iter().filter(|p| p.0 != dev.name).cloned().collect();
        f
    }

    // ---- writing (manual only) -----------------------------------------------------------------------------

    /// One device of the backup may be restored (pure): a PCI instance ID and, for every value, a name and range this
    /// program writes (`IrqAuto.ValidIrqValue`).
    pub fn valid_backup_device(pdo: &str, vals: Option<&IndexMap<String, Option<String>>>) -> bool {
        let Some(vals) = vals else { return false };
        ci::starts_with(pdo, "PCI\\")
            && instance_rx(pdo)
            && vals.iter().all(|(k, v)| BACKED_UP_NAMES.contains(&k.as_str()) && valid_irq_value(k, v.as_deref().unwrap_or("")))
    }

    fn clean(b: Option<Backup>, mut dropped: Option<&mut Vec<String>>) -> Backup {
        let mut c = Backup::default();
        for (pdo, vals) in b.map(|x| x.devices).unwrap_or_default() {
            if Self::valid_backup_device(&pdo, vals.as_ref()) {
                c.devices.insert(pdo, vals);
            } else if let Some(d) = dropped.as_deref_mut() {
                d.push(pdo);
            }
        }
        c
    }

    fn load_backup() -> Result<Backup, String> {
        let p = Self::backup_path()?;
        if !p.is_file() {
            return Ok(Backup::default());
        }
        let mut dropped = Vec::new();
        let b = Self::clean(parse_backup(&read_all_text(&p).map_err(|e| e.to_string())?)?, Some(&mut dropped));
        if !dropped.is_empty() {
            let first: Vec<&str> = dropped.iter().take(5).map(|s| s.as_str()).collect();
            AppLog::add_sev(lf!("Device backup: entries that are not values the program writes were ignored: {0}", first.join(", ")), Sev::Warn);
        }
        Ok(b)
    }

    /// Takes over the manual device backup of an older version: only valid devices; None when none is left.
    pub fn import_backup(json: &str, existing: Option<&str>, dropped: Option<&mut Vec<String>>) -> Result<Option<String>, String> {
        let b = Self::clean(parse_backup(json)?, dropped);
        let mut cur = match existing {
            None => Backup::default(),
            Some(e) => Self::clean(parse_backup(e)?, None),
        };
        for (k, v) in b.devices {
            cur.devices.entry(k).or_insert(v);
        }
        Ok(if cur.devices.is_empty() { None } else { Some(backup_json(&cur)) })
    }

    /// `Devices.WriteCalls`.
    pub fn write_calls() -> i32 {
        WRITE_CALLS.load(Ordering::SeqCst)
    }

    /// Writes a device's interrupt policy: backup -> values -> device restart -> read-back; on a mismatch the backup is
    /// put back. Returns an error text, None on success.
    pub fn write(dev: Option<&PciDevice>, want: &IrqConfig) -> Option<String> {
        Self::write_with(dev, want, true)
    }

    /// restart = false: only the registry is written (and read back); the policy starts at the next start of Windows.
    pub fn write_with(dev: Option<&PciDevice>, want: &IrqConfig, restart: bool) -> Option<String> {
        WRITE_CALLS.fetch_add(1, Ordering::SeqCst);
        if Flags::refuse(Flags::DEVICES_WRITE, L::t("Device policy write")) {
            return Some(L::t("The feature flag devices-write is closed").to_string());
        }
        let Some(dev) = dev.filter(|d| !d.pdo.is_empty()) else { return Some(L::t("No device").to_string()) };
        if Self::is_storage(Some(dev)) {
            return Some(L::t("Storage controllers are never changed: a wrong value there can stop Windows from starting").to_string());
        }
        // The boot guard's marker first: Windows that does not start fine afterwards gets the backup back.
        if let Err(e) = BootGuard::mark_pending() {
            return Some(lf!("Nothing was written: the safety marker could not be saved ({0})", e));
        }
        #[cfg(windows)]
        {
            match win::write(dev, want, restart) {
                Ok(r) => r,
                Err(e) => Some(e),
            }
        }
        #[cfg(not(windows))]
        {
            let _ = (want, restart);
            Some("Device writes need Windows".into())
        }
    }

    /// A storage controller or disk (never written; see `IrqAuto.IsStorage`).
    pub fn is_storage(dev: Option<&PciDevice>) -> bool {
        dev.map(|d| irq_is_storage(&d.class, &d.name, &d.service)).unwrap_or(false)
    }

    /// Puts every backed-up device back and removes the backup (and the boot guard's marker).
    pub fn revert_all() -> Option<String> {
        if let Some(h) = deps::IRQ_AUTO_REVERT_EXTRAS.get() {
            let _ = h();
        }
        if !Self::has_backup() {
            BootGuard::clear();
            return None;
        }
        let r = (|| -> Result<(), String> {
            let bk = Self::load_backup()?;
            let mut devs: CiMap<PciDevice> = CiMap::new();
            for d in Self::list() {
                // ToDictionary: a duplicate instance id would throw; SetupAPI lists each device once.
                if !devs.ci_contains(&d.pdo) {
                    let k = d.pdo.clone();
                    devs.ci_insert(&k, d);
                }
            }
            for (pdo, vals) in &bk.devices {
                Self::restore_values(pdo, vals.as_ref())?;
                if let Some(d) = devs.ci_get(pdo) {
                    Self::restart(d);
                }
            }
            std::fs::remove_file(Self::backup_path()?).map_err(|e| e.to_string())?;
            BootGuard::clear();
            Ok(())
        })();
        r.err()
    }

    fn restore_values(pdo: &str, vals: Option<&IndexMap<String, Option<String>>>) -> Result<(), String> {
        if !Self::valid_backup_device(pdo, vals) {
            AppLog::add_sev(lf!("Device backup: {0} holds values the program does not write; it was not restored", pdo), Sev::Warn);
            return Ok(());
        }
        #[cfg(windows)]
        {
            win::restore_values(pdo, vals.unwrap_or(&IndexMap::new()))
        }
        #[cfg(not(windows))]
        {
            Ok(())
        }
    }

    /// Restarts a device so the interrupt policy takes effect (DIF_PROPERTYCHANGE / DICS_PROPCHANGE).
    fn restart(dev: &PciDevice) -> Option<String> {
        #[cfg(windows)]
        {
            win::restart(dev)
        }
        #[cfg(not(windows))]
        {
            let _ = dev;
            None
        }
    }
}

/// `Enc(object)`: "b:HEX" for a binary value, "d:int" for a DWORD, "" for none or anything else.
#[cfg_attr(not(windows), allow(dead_code))]
fn enc(v: Option<crate_reg::Value>) -> String {
    match v {
        Some(crate_reg::Value::Binary(b)) => format!("b:{}", hex_upper(&b)),
        Some(crate_reg::Value::Dword(d)) => format!("d:{}", d as i32),
        _ => String::new(),
    }
}

/// The registry value kinds `enc` tells apart (a platform-neutral mirror of `native::reg::RegValue`).
#[allow(dead_code)]
mod crate_reg {
    pub enum Value {
        Binary(Vec<u8>),
        Dword(u32),
        Other,
    }
}

#[cfg(windows)]
mod win {
    use super::*;
    use crate::native::reg::{error_text, Hive, Key, RegValue};
    use crate::native::win::wide;
    use windows_sys::Win32::Devices::DeviceAndDriverInstallation::*;

    const INVALID: isize = -1;
    const SPDRP_DEVICEDESC: u32 = 0x0;
    const SPDRP_SERVICE: u32 = 0x4;
    const SPDRP_CLASS: u32 = 0x7;
    const SPDRP_DRIVER: u32 = 0x9;
    const SPDRP_FRIENDLYNAME: u32 = 0xC;
    const SPDRP_LOCATION_INFORMATION: u32 = 0xD;
    const CR_SUCCESS: u32 = 0;

    fn to_value(v: Option<RegValue>) -> Option<crate_reg::Value> {
        v.map(|v| match v {
            RegValue::Binary(b) => crate_reg::Value::Binary(b),
            RegValue::Dword(d) => crate_reg::Value::Dword(d),
            _ => crate_reg::Value::Other,
        })
    }

    fn err(msg: String) -> String {
        if msg == error_text(5) { L::t("Access denied: run as administrator").to_string() } else { msg }
    }

    fn new_data() -> SP_DEVINFO_DATA {
        let mut d: SP_DEVINFO_DATA = unsafe { std::mem::zeroed() };
        d.cbSize = std::mem::size_of::<SP_DEVINFO_DATA>() as u32;
        d
    }

    fn prop(set: HDEVINFO, d: &SP_DEVINFO_DATA, p: u32) -> String {
        let mut buf = vec![0u8; 1024];
        let mut need: u32 = 0;
        if unsafe { SetupDiGetDeviceRegistryPropertyW(set, d, p, std::ptr::null_mut(), buf.as_mut_ptr(), buf.len() as u32, &mut need) } == 0 {
            return String::new();
        }
        let n = (need as usize).min(buf.len());
        let w: Vec<u16> = buf[..n].chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
        let s = String::from_utf16_lossy(&w);
        s.trim_end_matches('\0').split('\0').next().unwrap_or("").to_string()
    }

    fn class_devs() -> HDEVINFO {
        let pci = wide("PCI");
        unsafe { SetupDiGetClassDevsW(std::ptr::null(), pci.as_ptr(), std::ptr::null_mut(), DIGCF_PRESENT | DIGCF_ALLCLASSES) }
    }

    pub fn list() -> Vec<PciDevice> {
        let mut list = Vec::new();
        let set = class_devs();
        if set == INVALID || set == 0 {
            return list;
        }
        let mut d = new_data();
        let mut i = 0u32;
        while unsafe { SetupDiEnumDeviceInfo(set, i, &mut d) } != 0 {
            i += 1;
            let mut id = vec![0u16; 512];
            if unsafe { SetupDiGetDeviceInstanceIdW(set, &d, id.as_mut_ptr(), id.len() as u32, std::ptr::null_mut()) } == 0 {
                continue;
            }
            let id = crate::native::win::from_wide(&id);
            let mut name = prop(set, &d, SPDRP_FRIENDLYNAME);
            if name.is_empty() {
                name = prop(set, &d, SPDRP_DEVICEDESC);
            }
            if name.is_empty() || id.is_empty() {
                continue;
            }
            list.push(PciDevice {
                name,
                pdo: id,
                service: prop(set, &d, SPDRP_SERVICE),
                driver_key: prop(set, &d, SPDRP_DRIVER),
                dev_inst: d.DevInst,
                location: prop(set, &d, SPDRP_LOCATION_INFORMATION),
                class: prop(set, &d, SPDRP_CLASS),
            });
            d.cbSize = std::mem::size_of::<SP_DEVINFO_DATA>() as u32;
        }
        unsafe { SetupDiDestroyDeviceInfoList(set) };
        list
    }

    pub fn read_irq(instance: &str) -> IrqConfig {
        let mut c = IrqConfig::default();
        if let Some(k) = Key::open(Hive::LocalMachine, &format!("{}{}{}", ENUM_ROOT, instance, POLICY_SUB), false) {
            if let Some(RegValue::Binary(b)) = k.get("AssignmentSetOverride") {
                if !b.is_empty() {
                    let mut full = [0u8; 8];
                    let n = b.len().min(8);
                    full[..n].copy_from_slice(&b[..n]);
                    c.assign = Some(u64::from_le_bytes(full));
                }
            }
            if let Some(RegValue::Dword(p)) = k.get("DevicePriority") {
                c.priority = Some(p as i32);
            }
        }
        if let Some(k) = Key::open(Hive::LocalMachine, &format!("{}{}{}", ENUM_ROOT, instance, MSI_SUB), false) {
            if let Some(RegValue::Dword(m)) = k.get("MSISupported") {
                c.msi = m != 0;
            }
            if let Some(RegValue::Dword(n)) = k.get("MessageNumberLimit") {
                c.msg_limit = n as i32;
            }
        }
        c
    }

    pub fn resources(dev_inst: u32) -> Vec<IrqResource> {
        let mut list = Vec::new();
        let mut conf: usize = 0;
        if unsafe { CM_Get_First_Log_Conf(&mut conf, dev_inst, ALLOC_LOG_CONF) } != CR_SUCCESS {
            return list;
        }
        let mut cur = conf;
        loop {
            let mut rd: usize = 0;
            if unsafe { CM_Get_Next_Res_Des(&mut rd, cur, ResType_IRQ, std::ptr::null_mut(), 0) } != CR_SUCCESS {
                break;
            }
            let mut size: u32 = 0;
            if unsafe { CM_Get_Res_Des_Data_Size(&mut size, rd, 0) } == CR_SUCCESS && size >= 16 {
                let mut data = vec![0u8; size as usize];
                if unsafe { CM_Get_Res_Des_Data(rd, data.as_mut_ptr() as *mut core::ffi::c_void, size, 0) } == CR_SUCCESS {
                    list.extend(Devices::parse_irq(Some(&data)));
                }
            }
            if cur != conf {
                unsafe { CM_Free_Res_Des_Handle(cur) };
            }
            cur = rd;
        }
        if cur != conf {
            unsafe { CM_Free_Res_Des_Handle(cur) };
        }
        unsafe { CM_Free_Log_Conf_Handle(conf) };
        list
    }

    fn set_or_delete(k: &Key, name: &str, e: &str) -> Result<(), String> {
        if let Some(h) = e.strip_prefix("b:") {
            let b = from_hex(h).ok_or_else(|| "The input is not a valid hex string as it contains a non-hex character.".to_string())?;
            k.set(name, &RegValue::Binary(b)).map_err(err)
        } else if let Some(n) = e.strip_prefix("d:") {
            let v = crate::base::settings::parse_int_integer_style(n).ok_or_else(|| format!("The input string '{}' was not in a correct format.", n))?;
            k.set(name, &RegValue::Dword(v as u32)).map_err(err)
        } else {
            k.delete_value(name, false).map_err(err)
        }
    }

    pub fn write(dev: &PciDevice, want: &IrqConfig, restart: bool) -> Result<Option<String>, String> {
        let pol_path = format!("{}{}{}", ENUM_ROOT, dev.pdo, POLICY_SUB);
        let msi_path = format!("{}{}{}", ENUM_ROOT, dev.pdo, MSI_SUB);
        // Backup of the original values (kept from the first change).
        let mut bk = Devices::load_backup()?;
        if !bk.devices.contains_key(&dev.pdo) {
            let pk = Key::open(Hive::LocalMachine, &pol_path, false);
            let mk = Key::open(Hive::LocalMachine, &msi_path, false);
            let g = |k: &Option<Key>, n: &str| enc(to_value(k.as_ref().and_then(|k| k.get(n))));
            let mut vals = IndexMap::new();
            vals.insert("AssignmentSetOverride".to_string(), Some(g(&pk, "AssignmentSetOverride")));
            vals.insert("DevicePolicy".to_string(), Some(g(&pk, "DevicePolicy")));
            vals.insert("DevicePriority".to_string(), Some(g(&pk, "DevicePriority")));
            vals.insert("MSISupported".to_string(), Some(g(&mk, "MSISupported")));
            vals.insert("MessageNumberLimit".to_string(), Some(g(&mk, "MessageNumberLimit")));
            bk.devices.insert(dev.pdo.clone(), Some(vals));
            // An UnauthorizedAccessException of the file calls is "access denied" too.
            let io = |e: std::io::Error| err(if e.kind() == std::io::ErrorKind::PermissionDenied { error_text(5) } else { e.to_string() });
            std::fs::create_dir_all(store_dir()?).map_err(io)?;
            let p = Devices::backup_path()?;
            let tmp = PathBuf::from(format!("{}.tmp", p.display()));
            write_all_text(&tmp, &backup_json(&bk)).map_err(io)?;
            move_overwrite(&tmp, &p).map_err(io)?;
        }
        {
            let pk = Key::create(Hive::LocalMachine, &pol_path).map_err(err)?;
            match want.assign.filter(|a| *a != 0) {
                Some(a) => {
                    pk.set("AssignmentSetOverride", &RegValue::Binary(a.to_le_bytes().to_vec())).map_err(err)?;
                    pk.set("DevicePolicy", &RegValue::Dword(4)).map_err(err)?; // IrqPolicySpecifiedProcessors
                }
                None => {
                    pk.delete_value("AssignmentSetOverride", false).map_err(err)?;
                    pk.delete_value("DevicePolicy", false).map_err(err)?;
                }
            }
            match want.priority {
                Some(p) => pk.set("DevicePriority", &RegValue::Dword(p as u32)).map_err(err)?,
                None => pk.delete_value("DevicePriority", false).map_err(err)?,
            }
        }
        {
            let mk = Key::create(Hive::LocalMachine, &msi_path).map_err(err)?;
            mk.set("MSISupported", &RegValue::Dword(if want.msi { 1 } else { 0 })).map_err(err)?;
            if want.msg_limit > 0 {
                mk.set("MessageNumberLimit", &RegValue::Dword(want.msg_limit as u32)).map_err(err)?;
            } else {
                mk.delete_value("MessageNumberLimit", false).map_err(err)?;
            }
        }
        let restarted = if restart { super::win::restart(dev) } else { None };
        let got = read_irq(&dev.pdo);
        let same = got.assign.unwrap_or(0) == want.assign.unwrap_or(0) && got.priority == want.priority && got.msi == want.msi && got.msg_limit == want.msg_limit;
        if !same {
            let vals = bk.devices.get(&dev.pdo).cloned().flatten();
            Devices::restore_values(&dev.pdo, vals.as_ref())?;
            if restart {
                super::win::restart(dev);
            }
            return Ok(Some(L::t("The values did not stick: the backup was put back").to_string()));
        }
        Ok(restarted)
    }

    pub fn restore_values(pdo: &str, vals: &IndexMap<String, Option<String>>) -> Result<(), String> {
        let get = |n: &str| vals.get(n).cloned().flatten().unwrap_or_default();
        let pk = Key::create(Hive::LocalMachine, &format!("{}{}{}", ENUM_ROOT, pdo, POLICY_SUB)).map_err(err)?;
        for n in ["AssignmentSetOverride", "DevicePolicy", "DevicePriority"] {
            set_or_delete(&pk, n, &get(n))?;
        }
        let mk = Key::create(Hive::LocalMachine, &format!("{}{}{}", ENUM_ROOT, pdo, MSI_SUB)).map_err(err)?;
        for n in ["MSISupported", "MessageNumberLimit"] {
            set_or_delete(&mk, n, &get(n))?;
        }
        Ok(())
    }

    pub fn restart(dev: &PciDevice) -> Option<String> {
        let set = class_devs();
        if set == INVALID || set == 0 {
            return Some(L::t("Device list unavailable").to_string());
        }
        let mut d = new_data();
        let mut i = 0u32;
        let mut result = Some(L::t("The device was not found for a restart").to_string());
        while unsafe { SetupDiEnumDeviceInfo(set, i, &mut d) } != 0 {
            i += 1;
            if d.DevInst != dev.dev_inst {
                continue;
            }
            let p = SP_PROPCHANGE_PARAMS {
                ClassInstallHeader: SP_CLASSINSTALL_HEADER { cbSize: std::mem::size_of::<SP_CLASSINSTALL_HEADER>() as u32, InstallFunction: DIF_PROPERTYCHANGE },
                StateChange: DICS_PROPCHANGE,
                Scope: DICS_FLAG_CONFIGSPECIFIC,
                HwProfile: 0,
            };
            let ok = unsafe { SetupDiSetClassInstallParamsW(set, &d, &p.ClassInstallHeader, std::mem::size_of::<SP_PROPCHANGE_PARAMS>() as u32) } != 0
                && unsafe { SetupDiCallClassInstaller(DIF_PROPERTYCHANGE, set, &d) } != 0;
            result = if !ok {
                Some(lf!("The device did not restart (error {0}): the change applies after a reboot", crate::native::win::last_error()))
            } else {
                Devices::mark_restarted(&dev.pdo);
                None
            };
            break;
        }
        unsafe { SetupDiDestroyDeviceInfoList(set) };
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::base::hooks::static_hooks;
    use crate::base::settings::{MemorySettingsBackend, Settings};
    use crate::base::time::Kind;
    use std::sync::Arc;

    /// DevicesTests.PinnedIrqsTakeNonZeroOverrides
    #[test]
    fn pinned_irqs_take_non_zero_overrides() {
        let _g = static_hooks();
        LIST_HOOK.set(Some(Arc::new(|| {
            ["GPU", "NIC", "USB"].iter().map(|n| PciDevice { name: n.to_string(), pdo: format!("PCI\\{}", n), ..Default::default() }).collect()
        })));
        READ_HOOK.set(Some(Arc::new(|id: &str| match id {
            "PCI\\GPU" => IrqConfig { assign: Some(0x10), ..Default::default() },
            "PCI\\NIC" => IrqConfig { assign: Some(0), ..Default::default() },
            _ => IrqConfig::default(),
        })));
        let p = Devices::pinned_irqs();
        LIST_HOOK.set(None);
        READ_HOOK.set(None);
        assert_eq!(p.len(), 1);
        assert_eq!(p[0].0, "GPU");
        assert_eq!(p[0].1, 0x10);
    }

    /// DevicesTests.ParseIrqDescriptor
    #[test]
    fn parse_irq_descriptor() {
        let mut d = vec![0u8; 48];
        d[12..16].copy_from_slice(&0xFFFF_FFF0u32.to_le_bytes());
        d[16..24].copy_from_slice(&0x30u64.to_le_bytes());
        d[36..40].copy_from_slice(&16u32.to_le_bytes());
        let r = Devices::parse_irq(Some(&d));
        assert_eq!(r.len(), 2);
        assert!(r[0].message);
        assert_eq!(r[0].affinity, 0x30);
        assert!(!r[1].message);
        assert_eq!(r[1].vector, 16);
        assert!(r.iter().all(|x| x.kind == IrqKind::Isr));
    }

    /// DevicesTests.FactsReadRestartFromTheStateFile
    #[test]
    fn facts_read_restart_from_the_state_file() {
        let _g = static_hooks();
        let dir = std::env::temp_dir().join(format!("ds-dev-{}", super::super::util::guid_n()));
        std::fs::create_dir_all(&dir).unwrap();
        let d2 = dir.clone();
        STATE_PATH_HOOK.set(Some(Arc::new(move || d2.join("state.json"))));
        RESOURCE_HOOK.set(Some(Arc::new(|_: &str| {
            let mut d = vec![0u8; 24];
            d[12..16].copy_from_slice(&0x8000_0010u32.to_le_bytes());
            Some(d)
        })));
        let at = DateTime::from_parts(2026, 5, 1, 10, 0, 0, Kind::Utc).unwrap();
        std::fs::write(Devices::state_path().unwrap(), r#"{"PCI\\GPU":"2026-05-01T10:00:00Z"}"#).unwrap();
        let f = Devices::facts(&PciDevice { name: "GPU".into(), pdo: "PCI\\GPU".into(), ..Default::default() }, Some(&[("GPU".into(), 0x10), ("NIC".into(), 0x20)]));
        STATE_PATH_HOOK.set(None);
        RESOURCE_HOOK.set(None);
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(f.restarted, at);
        assert!(f.resources[0].message);
        assert_eq!(f.others.len(), 1);
        assert_eq!(f.others[0].0, "NIC");
    }

    /// FuzzTests.InterruptAndCpuSetBuffersNeverThrow (the Devices part)
    #[test]
    fn interrupt_buffers_never_throw() {
        let mut r = crate::base::testrand::Rand::new(1603);
        for _ in 0..crate::base::testrand::iterations() {
            Devices::parse_irq(Some(&r.bytes(600)));
        }
        assert!(Devices::parse_irq(None).is_empty());
    }

    /// SafetyTests.StorageControllersAreNeverTouched (IrqAuto.IsStorage cases and the Devices part)
    #[test]
    fn storage_controllers_are_never_touched() {
        assert!(irq_is_storage("SCSIAdapter", "Standard NVM Express Controller", "stornvme"));
        assert!(irq_is_storage("HDC", "Standard SATA AHCI Controller", "storahci"));
        assert!(irq_is_storage("System", "Intel(R) RST VMD Controller", "iaStorVD"));
        assert!(irq_is_storage("", "AMD-RAID Bottom Device", "rcbottom"));
        assert!(irq_is_storage("", "Samsung NVMe Controller", ""));
        assert!(irq_is_storage("", "x", "secnvme"));
        assert!(!irq_is_storage("Display", "NVIDIA GeForce RTX 5060", "nvlddmkm"));
        assert!(!irq_is_storage("USB", "AMD USB 3.10 eXtensible Host Controller - 1.10 (Microsoft)", "USBXHCI"));
        assert!(!irq_is_storage("Net", "Realtek PCIe 2.5GbE Family Controller", "rt25cx21x64"));
        assert!(!irq_is_storage("MEDIA", "Realtek(R) Audio", "IntcAzAudAddService"));
        assert!(Devices::is_storage(Some(&PciDevice { class: "SCSIAdapter".into(), name: "Standard NVM Express Controller".into(), ..Default::default() })));
        assert!(stor_name("NVM   express x") && !stor_name("Ideal") && stor_name("IDE Channel"));
    }

    fn vals(list: &[(&str, &str)]) -> IndexMap<String, Option<String>> {
        list.iter().map(|(k, v)| (k.to_string(), Some(v.to_string()))).collect()
    }

    /// SecurityTests.DeviceLayoutBackupAcceptsOnlyWhatTheLayoutWrites (the manual device backup part; the IrqAuto
    /// entries belong to the etw area)
    #[test]
    fn device_backup_accepts_only_what_the_program_writes() {
        let pdo = r"PCI\VEN_10DE&DEV_2D04&SUBSYS_00000000&REV_A1\4&2f9a8b1&0&0008";
        assert!(Devices::valid_backup_device(pdo, Some(&vals(&[("DevicePolicy", "d:4"), ("AssignmentSetOverride", "b:04"), ("MSISupported", "")]))));
        assert!(!Devices::valid_backup_device(pdo, Some(&vals(&[("ImagePath", "s:x")]))));
        assert!(!Devices::valid_backup_device(pdo, Some(&vals(&[("DevicePriority", "s:7")]))));
        assert!(!Devices::valid_backup_device(r"ROOT\LEGACY_EVIL\0000", Some(&vals(&[("DevicePolicy", "d:4")]))));
        let mut dropped = Vec::new();
        let j = Devices::import_backup(
            &format!(r#"{{"Devices":{{"{}":{{"DevicePolicy":"d:4"}},"PCI\\A|B":{{"DevicePolicy":"d:4"}}}}}}"#, pdo.replace('\\', "\\\\")),
            None,
            Some(&mut dropped),
        )
        .unwrap()
        .unwrap();
        assert!(j.contains("DevicePolicy"));
        assert_eq!(dropped, vec!["PCI\\A|B".to_string()]);
    }

    /// RoadmapTests.ClosedFlagsRefuseTheirActionsAndOnlyCoreDevicesAndGpuAreOpenByDefault (the Devices part)
    #[test]
    fn closed_flag_refuses_the_device_write() {
        let _g = static_hooks();
        let _b = Settings::scoped_backend(Arc::new(MemorySettingsBackend::new()));
        AppLog::clear();
        Flags::set(Flags::DEVICES_WRITE, false).unwrap();
        let calls = Devices::write_calls();
        let err = Devices::write(Some(&PciDevice { name: "NIC".into(), pdo: r"PCI\VEN_1".into(), ..Default::default() }), &IrqConfig { assign: Some(4), ..Default::default() });
        assert!(err.unwrap().contains("devices-write"));
        assert_eq!(calls + 1, Devices::write_calls());
        assert!(AppLog::tail(500).iter().any(|e| e.sev == Sev::Warn && e.msg.contains("devices-write")));
    }

    #[test]
    fn state_file_dates_round_trip() {
        assert!(instance_rx(r"PCI\VEN_8086&DEV_7AE0\3&11583659&0&A0") && !instance_rx(r"PCI\A\B\C") && !instance_rx(r"PCI\A|B\C"));
        assert!(stor_service("iaStorVD") && stor_service("nvme_x") && !stor_service("nvme-x") && stor_service("disk\n"));
    }
}
