//! The catalogue of documented system changes (port of `Tweaks.cs`: `IRegStore`, `WinRegStore`, `MemoryRegStore`,
//! `RegChange`, `Tweak`, `TweakState`, `Tweaks`). Apply saves the previous values first; Revert puts exactly those
//! back, or the Windows defaults when the change was made by something else. Nothing here touches security (the
//! protection switches of older versions are only reverted, see `Tweaks::revert_retired`).

use super::util::{create_parent, move_overwrite, read_all_text, to_json_indented, write_all_text};
use crate::base::app_log::{AppLog, Sev};
use crate::base::ci::{self, CiMap, CiMapExt, CiSet, CiSetExt};
use crate::base::hooks::{lock, Hook};
use crate::base::l::L;
use crate::base::state_dir::store_dir;
use crate::lf;
use indexmap::IndexMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::{Arc, Mutex, OnceLock, RwLock};

/// A failed registry (or bcdedit) call: the C# exception. access_denied = `UnauthorizedAccessException`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RegError {
    pub access_denied: bool,
    pub message: String,
}

impl RegError {
    pub fn new(message: impl Into<String>) -> Self {
        RegError { access_denied: false, message: message.into() }
    }
}

/// Registry access of the system-changes page (the real registry in the product, a dictionary in tests).
pub trait IRegStore: Send + Sync {
    /// The value as text ("" for an empty string), None when it does not exist.
    fn get(&self, hive: &str, path: &str, name: &str) -> Result<Option<String>, RegError>;
    fn set(&self, hive: &str, path: &str, name: &str, kind: &str, data: Option<&str>) -> Result<(), RegError>;
    fn delete(&self, hive: &str, path: &str, name: &str) -> Result<(), RegError>;
    /// The key exists (services: the service is installed on this Windows).
    fn has_key(&self, hive: &str, path: &str) -> Result<bool, RegError>;
}

/// `^(\{[A-Za-z0-9\-]{1,40}\}|[A-Za-z0-9]{1,40})$` (.NET's `$` also matches before one final "\n").
fn bcd_part(s: &str) -> bool {
    let s = s.strip_suffix('\n').unwrap_or(s);
    if let Some(inner) = s.strip_prefix('{').and_then(|x| x.strip_suffix('}')) {
        let n = inner.chars().count();
        return (1..=40).contains(&n) && inner.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
    }
    let n = s.chars().count();
    (1..=40).contains(&n) && s.chars().all(|c| c.is_ascii_alphanumeric())
}

/// bcdedit with every part a separate, checked argument (no command line is put together from text): the entry is
/// "{current}" or another "{…}" identifier, the element a plain word, the value letters and digits only.
pub fn bcd(verb: &str, entry: &str, element: Option<&str>, value: Option<&str>) -> Result<String, RegError> {
    if !bcd_part(entry) || !entry.starts_with('{') || element.map(|e| !bcd_part(e)).unwrap_or(false) || value.map(|v| !bcd_part(v)).unwrap_or(false) {
        return Err(RegError::new("bcdedit: refused (not a boot entry, element or value this program uses)"));
    }
    let mut psi = crate::native::sys_exe::SysExe::start("bcdedit.exe", true);
    psi.arg(verb);
    psi.arg(entry);
    if let Some(e) = element {
        psi.arg(e);
    }
    if let Some(v) = value {
        psi.arg(v);
    }
    let r = super::util::run_capture(&psi, 10000).map_err(|e| RegError::new(e.to_string()))?;
    let output = format!("{}{}", r.stdout, r.stderr);
    let Some(code) = r.code else { return Err(RegError::new("Process must exit before requested information can be determined.")) };
    if code != 0 && verb != "/enum" && verb != "/deletevalue" {
        return Err(RegError::new(format!("bcdedit {} {} {}: {}", verb, entry, element.unwrap_or(""), output.trim())));
    }
    Ok(output)
}

/// Element names in "bcdedit /enum" are not translated; the value is the second word of the line.
#[cfg_attr(not(windows), allow(dead_code))]
fn bcd_get(entry: &str, name: &str) -> Result<Option<String>, RegError> {
    for line in bcd("/enum", entry, None, None)?.split('\n') {
        let l = line.trim();
        if let Some(i) = l.find(char::is_whitespace) {
            let (a, b) = (&l[..i], l[i..].trim());
            if !b.is_empty() && ci::eq(a, name) {
                return Ok(Some(b.to_string()));
            }
        }
    }
    Ok(None)
}

/// The real registry. The pseudo hive "BCD" is the boot configuration of the running Windows: path is the entry
/// ("{current}"), name the element ("hypervisorlaunchtype"); it is read and written with bcdedit.
#[derive(Default)]
pub struct WinRegStore;

#[cfg(windows)]
impl WinRegStore {
    fn root(hive: &str) -> crate::native::reg::Hive {
        use crate::native::reg::Hive;
        if hive == "HKLM" { Hive::LocalMachine } else { Hive::CurrentUser }
    }

    fn err(msg: String) -> RegError {
        RegError { access_denied: msg == crate::native::reg::error_text(5), message: msg }
    }
}

#[cfg(windows)]
impl IRegStore for WinRegStore {
    fn get(&self, hive: &str, path: &str, name: &str) -> Result<Option<String>, RegError> {
        use crate::native::reg::{Key, RegValue};
        if hive == "BCD" {
            return bcd_get(path, name);
        }
        let Some(k) = Key::open(Self::root(hive), path, false) else { return Ok(None) };
        Ok(k.get(name).map(|v| match v {
            RegValue::Binary(b) | RegValue::Other(_, b) => super::util::hex_upper(&b),
            v => v.to_csharp_string(),
        }))
    }

    fn set(&self, hive: &str, path: &str, name: &str, kind: &str, data: Option<&str>) -> Result<(), RegError> {
        use crate::native::reg::{Key, RegValue};
        if hive == "BCD" {
            return bcd("/set", path, Some(name), data).map(|_| ());
        }
        let k = Key::create(Self::root(hive), path).map_err(Self::err)?;
        let v = match kind {
            "dword" => {
                let d = data.unwrap_or("");
                let n = crate::base::settings::parse_int_integer_style(d)
                    .ok_or_else(|| RegError::new(format!("The input string '{}' was not in a correct format.", d)))?;
                RegValue::Dword(n as u32)
            }
            "binary" => RegValue::Binary(
                super::util::from_hex(data.unwrap_or("")).ok_or_else(|| RegError::new("The input is not a valid hex string as it contains a non-hex character."))?,
            ),
            _ => RegValue::Sz(data.unwrap_or("").to_string()),
        };
        k.set(name, &v).map_err(Self::err)
    }

    fn delete(&self, hive: &str, path: &str, name: &str) -> Result<(), RegError> {
        use crate::native::reg::Key;
        if hive == "BCD" {
            return bcd("/deletevalue", path, Some(name), None).map(|_| ());
        }
        match Key::open_err(Self::root(hive), path, true) {
            Ok(k) => k.delete_value(name, false).map_err(Self::err),
            Err(5) => Err(Self::err(crate::native::reg::error_text(5))),
            Err(_) => Ok(()),
        }
    }

    fn has_key(&self, hive: &str, path: &str) -> Result<bool, RegError> {
        if hive == "BCD" {
            return Ok(true);
        }
        Ok(crate::native::reg::Key::open(Self::root(hive), path, false).is_some())
    }
}

/// The registry as a dictionary (tests; also the store on platforms without a registry).
#[derive(Default)]
pub struct MemoryRegStore {
    values: Mutex<CiMap<String>>,
    writes: AtomicI32,
    /// Keys reported as not installed (everything else exists).
    missing: Mutex<CiSet>,
}

impl MemoryRegStore {
    pub fn new() -> Self {
        Self::default()
    }

    fn key(hive: &str, path: &str, name: &str) -> String {
        format!("{}\\{}\\{}", hive, path, name)
    }

    pub fn writes(&self) -> i32 {
        self.writes.load(Ordering::SeqCst)
    }

    pub fn values(&self) -> CiMap<String> {
        lock(&self.values).clone()
    }

    pub fn add_missing(&self, hive_path: &str) {
        lock(&self.missing).ci_add(hive_path);
    }
}

impl IRegStore for MemoryRegStore {
    fn get(&self, hive: &str, path: &str, name: &str) -> Result<Option<String>, RegError> {
        Ok(lock(&self.values).ci_get(&Self::key(hive, path, name)).cloned())
    }
    fn set(&self, hive: &str, path: &str, name: &str, _kind: &str, data: Option<&str>) -> Result<(), RegError> {
        self.writes.fetch_add(1, Ordering::SeqCst);
        lock(&self.values).ci_insert(&Self::key(hive, path, name), data.unwrap_or("").to_string());
        Ok(())
    }
    fn delete(&self, hive: &str, path: &str, name: &str) -> Result<(), RegError> {
        self.writes.fetch_add(1, Ordering::SeqCst);
        lock(&self.values).ci_remove(&Self::key(hive, path, name));
        Ok(())
    }
    fn has_key(&self, hive: &str, path: &str) -> Result<bool, RegError> {
        Ok(!lock(&self.missing).ci_contains(&format!("{}\\{}", hive, path)))
    }
}

/// One registry value of a change: what it is when the change is on, and what Windows has by default (None = no
/// value).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RegChange {
    pub hive: String,
    pub path: String,
    pub name: String,
    pub kind: String,
    pub on: String,
    pub default: Option<String>,
    /// Skipped when its key does not exist (a service that this Windows does not have); never creates the key.
    pub if_key_exists: bool,
}

impl RegChange {
    pub fn new(hive: &str, path: &str, name: &str, kind: &str, on: &str, def: Option<&str>) -> Self {
        RegChange { hive: hive.into(), path: path.into(), name: name.into(), kind: kind.into(), on: on.into(), default: def.map(|s| s.into()), if_key_exists: false }
    }
}

/// A documented system change: what it does, why it is offered, what you give up.
#[derive(Clone)]
pub struct Tweak {
    pub id: String,
    pub group: String,
    pub title: String,
    pub what: String,
    pub why: String,
    pub cost: String,
    pub recommended: bool,
    /// Takes effect only after a restart of Windows.
    pub needs_restart: bool,
    /// Runs before Apply; a Some text stops it (for example "turn off Tamper Protection first").
    pub pre_check: Option<Arc<dyn Fn() -> Option<String> + Send + Sync>>,
    pub changes: Vec<RegChange>,
}

impl Default for Tweak {
    fn default() -> Self {
        Tweak {
            id: String::new(),
            group: String::new(),
            title: String::new(),
            what: String::new(),
            why: String::new(),
            cost: String::new(),
            recommended: true,
            needs_restart: false,
            pre_check: None,
            changes: Vec::new(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TweakState {
    Off,
    On,
    Mixed,
}

static REG: RwLock<Option<Arc<dyn IRegStore>>> = RwLock::new(None);
pub static BACKUP_PATH_HOOK: Hook<dyn Fn() -> PathBuf + Send + Sync> = Hook::new();

const CDM: &str = r"Software\Microsoft\Windows\CurrentVersion\ContentDeliveryManager";
const WU: &str = r"SOFTWARE\Policies\Microsoft\Windows\WindowsUpdate";
const CURRENT_VERSION: &str = r"SOFTWARE\Microsoft\Windows NT\CurrentVersion";
const DG: &str = r"SYSTEM\CurrentControlSet\Control\DeviceGuard";
const WD: &str = r"SOFTWARE\Policies\Microsoft\Windows Defender";

fn t(id: &str, group: &str, title: &str, what: &str, why: &str, cost: &str, changes: Vec<RegChange>) -> Tweak {
    Tweak { id: id.into(), group: group.into(), title: title.into(), what: what.into(), why: why.into(), cost: cost.into(), changes, ..Default::default() }
}

fn d(hive: &str, path: &str, name: &str, on: i32, def: Option<i32>) -> RegChange {
    let def = def.map(|x| x.to_string());
    RegChange::new(hive, path, name, "dword", &on.to_string(), def.as_deref())
}

fn s(hive: &str, path: &str, name: &str, on: &str, def: Option<&str>) -> RegChange {
    RegChange::new(hive, path, name, "sz", on, def)
}

fn b(hive: &str, path: &str, name: &str, on: &str, def: Option<&str>) -> RegChange {
    RegChange::new(hive, path, name, "binary", on, def)
}

/// 1.16.2 (owner's decision): these Windows changes are no longer offered in Toolbox. The playbook still sets them
/// (and Undo Domain OS puts them back); here they are only hidden. Nothing is reverted at start: a value a user chose
/// stays as it is. Their backups are still read (`import_backup`) so nothing saved is thrown away.
const WITHDRAWN_IDS: [&str; 10] = ["sticky-keys", "telemetry", "ad-id", "activity", "recall", "suggestions", "bing-search", "widgets", "wu-reboot", "file-ext"];

fn build() -> Vec<Tweak> {
    let n = L::n;
    vec![
        t(
            "game-dvr",
            n("Games"),
            n("Background recording off (Game DVR)"),
            n("Windows stops recording the last minutes of every game in the background."),
            n("Background recording takes GPU encoder and disk time in every game, whether you ever save a clip or not."),
            n("\"Record what happened\" (Win+Alt+G) no longer works. Recording by hand (Win+Alt+R) still does."),
            vec![
                d("HKCU", r"System\GameConfigStore", "GameDVR_Enabled", 0, Some(1)),
                d("HKCU", r"Software\Microsoft\Windows\CurrentVersion\GameDVR", "AppCaptureEnabled", 0, Some(1)),
                d("HKCU", r"Software\Microsoft\Windows\CurrentVersion\GameDVR", "HistoricalCaptureEnabled", 0, None),
            ],
        ),
        t(
            "mouse-accel",
            n("Games"),
            n("Pointer acceleration off"),
            n("\"Enhance pointer precision\" is switched off."),
            n("With acceleration the cursor travels further when the hand moves faster; without it the same hand movement always gives the same distance, which aiming relies on."),
            n("The pointer feels slower on the desktop at first. Games that read raw input are not affected either way."),
            vec![
                s("HKCU", r"Control Panel\Mouse", "MouseSpeed", "0", Some("1")),
                s("HKCU", r"Control Panel\Mouse", "MouseThreshold1", "0", Some("6")),
                s("HKCU", r"Control Panel\Mouse", "MouseThreshold2", "0", Some("10")),
            ],
        ),
        t(
            "sticky-keys",
            n("Games"),
            n("Sticky Keys shortcuts off"),
            n("Five presses of Shift, holding Shift for 8 seconds and holding Num Lock no longer open the accessibility prompts."),
            n("These prompts throw a window over a full-screen game at the worst moment."),
            n("The accessibility features themselves stay; only their keyboard shortcuts are off."),
            vec![
                s("HKCU", r"Control Panel\Accessibility\StickyKeys", "Flags", "506", Some("510")),
                s("HKCU", r"Control Panel\Accessibility\Keyboard Response", "Flags", "122", Some("126")),
                s("HKCU", r"Control Panel\Accessibility\ToggleKeys", "Flags", "58", Some("62")),
            ],
        ),
        t(
            "telemetry",
            n("Privacy"),
            n("Diagnostic data: required only"),
            n("Windows sends only the required diagnostic data."),
            n("Optional diagnostic data includes browsing and app-usage details that are of no use to the owner of the PC."),
            n("Nothing. The value 0 is not used: Home and Pro ignore it."),
            vec![d("HKLM", r"SOFTWARE\Policies\Microsoft\Windows\DataCollection", "AllowTelemetry", 1, None)],
        ),
        t(
            "ad-id",
            n("Privacy"),
            n("Advertising ID and tailored experiences off"),
            n("Apps do not get an advertising identifier, and Windows does not build tips and ads from your diagnostic data."),
            n("Less profiling; no effect on how Windows or games work."),
            n("Ads inside Store apps become generic instead of personal."),
            vec![
                d("HKCU", r"Software\Microsoft\Windows\CurrentVersion\AdvertisingInfo", "Enabled", 0, Some(1)),
                d("HKCU", r"Software\Microsoft\Windows\CurrentVersion\Privacy", "TailoredExperiencesWithDiagnosticDataEnabled", 0, Some(1)),
            ],
        ),
        t(
            "activity",
            n("Privacy"),
            n("Activity history off"),
            n("Windows does not collect or upload the list of apps and files you used."),
            n("The history is only used for cross-device suggestions."),
            n("\"Pick up where you left off\" across devices stops working."),
            vec![
                d("HKLM", r"SOFTWARE\Policies\Microsoft\Windows\System", "PublishUserActivities", 0, None),
                d("HKLM", r"SOFTWARE\Policies\Microsoft\Windows\System", "UploadUserActivities", 0, None),
            ],
        ),
        t(
            "recall",
            n("Privacy"),
            n("Recall off"),
            n("Windows does not save snapshots of the screen for Recall."),
            n("Snapshots take disk and processing time and contain everything that was on the screen."),
            n("The Recall search over past screens is not available."),
            vec![d("HKLM", r"SOFTWARE\Policies\Microsoft\Windows\WindowsAI", "DisableAIDataAnalysis", 1, None)],
        ),
        t(
            "suggestions",
            n("Advertising"),
            n("Suggestions and advertising off"),
            n("No suggested apps in Start, no tips and offers in Settings, on the lock screen and in notifications, no silently installed \"suggested\" apps."),
            n("These are advertising slots; some of them download and install apps on their own."),
            n("Nothing you would miss."),
            vec![
                d("HKCU", CDM, "SilentInstalledAppsEnabled", 0, Some(1)),
                d("HKCU", CDM, "SystemPaneSuggestionsEnabled", 0, Some(1)),
                d("HKCU", CDM, "SoftLandingEnabled", 0, Some(1)),
                d("HKCU", CDM, "RotatingLockScreenOverlayEnabled", 0, Some(1)),
                d("HKCU", CDM, "SubscribedContent-338388Enabled", 0, Some(1)),
                d("HKCU", CDM, "SubscribedContent-338389Enabled", 0, Some(1)),
                d("HKCU", CDM, "SubscribedContent-338393Enabled", 0, Some(1)),
                d("HKCU", CDM, "SubscribedContent-353694Enabled", 0, Some(1)),
                d("HKCU", CDM, "SubscribedContent-353696Enabled", 0, Some(1)),
                d("HKCU", r"Software\Microsoft\Windows\CurrentVersion\Explorer\Advanced", "Start_IrisRecommendations", 0, Some(1)),
            ],
        ),
        t(
            "bing-search",
            n("Advertising"),
            n("Start menu search without Bing"),
            n("Typing in the Start menu searches only this PC."),
            n("By default every keystroke in Start is sent to Bing and web results are mixed into the list."),
            n("No web results in the Start menu; the browser still searches the web."),
            vec![d("HKCU", r"Software\Policies\Microsoft\Windows\Explorer", "DisableSearchBoxSuggestions", 1, None)],
        ),
        t(
            "widgets",
            n("Advertising"),
            n("Widgets board off"),
            n("The news and weather board and its taskbar button are off."),
            n("The board keeps a web-view process running to load a news feed."),
            n("No weather on the taskbar."),
            vec![d("HKLM", r"SOFTWARE\Policies\Microsoft\Dsh", "AllowNewsAndInterests", 0, None)],
        ),
        t(
            "wu-feature",
            n("Windows Update"),
            n("Stay on this Windows release"),
            n("Windows Update offers only updates for the Windows 11 release this PC runs now; the next feature release is not installed by itself."),
            n("A feature update reinstalls Windows components and can undo settings and reinstall removed apps. You choose when to move on."),
            n("You get new Windows features only after switching this off. Security updates keep coming."),
            vec![
                d("HKLM", WU, "TargetReleaseVersion", 1, None),
                s("HKLM", WU, "ProductVersion", "Windows 11", None),
                s("HKLM", WU, "TargetReleaseVersionInfo", &Tweaks::pin_release(win_build(), win_display_version().as_deref()), None),
            ],
        ),
        t(
            "wu-drivers",
            n("Windows Update"),
            n("No drivers from Windows Update"),
            n("Windows Update does not install or replace device drivers."),
            n("Windows Update can replace a graphics or chipset driver you chose with an older one from its catalogue."),
            n("You install drivers yourself (see the Drivers page). A new device may need its driver from the maker's site."),
            vec![
                d("HKLM", WU, "ExcludeWUDriversInQualityUpdate", 1, None),
                d("HKLM", r"SOFTWARE\Microsoft\Windows\CurrentVersion\DriverSearching", "SearchOrderConfig", 0, Some(1)),
            ],
        ),
        t(
            "wu-reboot",
            n("Windows Update"),
            n("No automatic restart while signed in"),
            n("Windows does not restart by itself to finish an update while somebody is signed in, and does not wake the PC to install updates."),
            n("A forced restart can end a match or lose unsaved work."),
            n("Updates wait until you restart; do it yourself regularly."),
            vec![
                d("HKLM", &format!(r"{}\AU", WU), "NoAutoRebootWithLoggedOnUsers", 1, None),
                d("HKLM", &format!(r"{}\AU", WU), "AUPowerManagement", 0, None),
            ],
        ),
        t(
            "file-ext",
            n("Interface"),
            n("Show file name extensions"),
            n("Explorer shows \".exe\", \".jpg\" and the other extensions."),
            n("You can tell a program from a document: \"photo.jpg.exe\" is no longer shown as \"photo.jpg\"."),
            n("Nothing."),
            vec![d("HKCU", r"Software\Microsoft\Windows\CurrentVersion\Explorer\Advanced", "HideFileExt", 0, Some(1))],
        ),
    ]
}

// ---- the release "Stay on this Windows release" pins ---------------------------------------------------------

fn win_build() -> i32 {
    #[cfg(windows)]
    {
        if let Some(k) = crate::native::reg::Key::open(crate::native::reg::Hive::LocalMachine, CURRENT_VERSION, false) {
            if let Some(b) = k.get_string("CurrentBuildNumber").and_then(|s| crate::base::settings::parse_int_integer_style(&s)) {
                return b;
            }
        }
        // Environment.OSVersion.Version.Build (RtlGetVersion: the real version, whatever the manifest says).
        #[repr(C)]
        struct OsVersionInfoW {
            size: u32,
            major: u32,
            minor: u32,
            build: u32,
            platform: u32,
            csd: [u16; 128],
        }
        #[link(name = "ntdll")]
        extern "system" {
            fn RtlGetVersion(info: *mut OsVersionInfoW) -> i32;
        }
        let mut v = OsVersionInfoW { size: std::mem::size_of::<OsVersionInfoW>() as u32, major: 0, minor: 0, build: 0, platform: 0, csd: [0; 128] };
        if unsafe { RtlGetVersion(&mut v) } == 0 {
            return v.build as i32;
        }
        0
    }
    #[cfg(not(windows))]
    {
        let _ = CURRENT_VERSION;
        0
    }
}

fn win_display_version() -> Option<String> {
    #[cfg(windows)]
    {
        crate::native::reg::Key::open(crate::native::reg::Hive::LocalMachine, CURRENT_VERSION, false)?.get_string("DisplayVersion")
    }
    #[cfg(not(windows))]
    {
        None
    }
}

/// `^\d{2}H[12]$` (also before one final "\n", as .NET's `$`).
fn release_text(v: &str) -> bool {
    let v = v.strip_suffix('\n').unwrap_or(v);
    let b = v.as_bytes();
    b.len() == 4 && b[0].is_ascii_digit() && b[1].is_ascii_digit() && b[2] == b'H' && (b[3] == b'1' || b[3] == b'2')
}

/// The backup: change id -> (value id -> saved value; "\0" = there was no value). A C# dictionary keeps its order.
type Backup = IndexMap<String, Option<IndexMap<String, Option<String>>>>;

/// Serializable view of the inner maps (insertion order).
struct OrderedInner<'a>(&'a IndexMap<String, Option<String>>);

impl serde::Serialize for OrderedInner<'_> {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        let mut m = s.serialize_map(Some(self.0.len()))?;
        for (k, v) in self.0 {
            m.serialize_entry(k, v)?;
        }
        m.end()
    }
}

struct OrderedBackup<'a>(&'a Backup);

impl serde::Serialize for OrderedBackup<'_> {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        let mut m = s.serialize_map(Some(self.0.len()))?;
        for (k, v) in self.0 {
            m.serialize_entry(k, &v.as_ref().map(OrderedInner))?;
        }
        m.end()
    }
}

/// Parses the backup JSON keeping the order of the file (serde_json's own maps are sorted).
fn parse_backup(json: &str) -> Result<Option<Backup>, String> {
    use serde::de::{Deserializer, MapAccess, Visitor};
    struct Inner(IndexMap<String, Option<String>>);
    impl<'de> serde::Deserialize<'de> for Inner {
        fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
            struct V;
            impl<'de> Visitor<'de> for V {
                type Value = Inner;
                fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                    f.write_str("an object of strings")
                }
                fn visit_map<A: MapAccess<'de>>(self, mut a: A) -> Result<Inner, A::Error> {
                    let mut m = IndexMap::new();
                    while let Some((k, v)) = a.next_entry::<String, Option<String>>()? {
                        m.insert(k, v);
                    }
                    Ok(Inner(m))
                }
            }
            d.deserialize_map(V)
        }
    }
    struct Outer(Backup);
    impl<'de> serde::Deserialize<'de> for Outer {
        fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
            struct V;
            impl<'de> Visitor<'de> for V {
                type Value = Outer;
                fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                    f.write_str("an object of objects")
                }
                fn visit_map<A: MapAccess<'de>>(self, mut a: A) -> Result<Outer, A::Error> {
                    let mut m = IndexMap::new();
                    while let Some((k, v)) = a.next_entry::<String, Option<Inner>>()? {
                        m.insert(k, v.map(|x| x.0));
                    }
                    Ok(Outer(m))
                }
            }
            d.deserialize_map(V)
        }
    }
    serde_json::from_str::<Option<Outer>>(json).map(|o| o.map(|x| x.0)).map_err(|e| e.to_string())
}

/// `Tweaks` (static class).
pub struct Tweaks;

impl Tweaks {
    pub const BACKUP_NAME: &'static str = "system-changes-backup.json";

    /// `Tweaks.Reg`: the registry the changes go through.
    pub fn reg() -> Arc<dyn IRegStore> {
        if let Some(r) = REG.read().unwrap_or_else(|e| e.into_inner()).as_ref() {
            return r.clone();
        }
        #[cfg(windows)]
        {
            Arc::new(WinRegStore)
        }
        #[cfg(not(windows))]
        {
            static EMPTY: OnceLock<Arc<MemoryRegStore>> = OnceLock::new();
            EMPTY.get_or_init(|| Arc::new(MemoryRegStore::new())).clone()
        }
    }

    /// `Tweaks.Reg = r` (None: the Windows registry again).
    pub fn set_reg(r: Option<Arc<dyn IRegStore>>) {
        *REG.write().unwrap_or_else(|e| e.into_inner()) = r;
    }

    pub fn backup_path() -> Result<PathBuf, String> {
        if let Some(h) = BACKUP_PATH_HOOK.get() {
            return Ok(h());
        }
        Ok(store_dir()?.join(Self::BACKUP_NAME))
    }

    /// The changes Toolbox offers.
    pub fn catalog() -> &'static [Tweak] {
        static C: OnceLock<Vec<Tweak>> = OnceLock::new();
        C.get_or_init(|| build().into_iter().filter(|t| !WITHDRAWN_IDS.contains(&t.id.as_str())).collect())
    }

    /// The changes no longer offered (1.16.2); only their backups are still read.
    pub fn withdrawn() -> &'static [Tweak] {
        static W: OnceLock<Vec<Tweak>> = OnceLock::new();
        W.get_or_init(|| build().into_iter().filter(|t| WITHDRAWN_IDS.contains(&t.id.as_str())).collect())
    }

    /// The release written to TargetReleaseVersionInfo. Builds 26200 (25H2) and 26300 (26H2) are held on 26H2, the
    /// same value the Domain OS playbook writes (25H2 -> 26H2 is a small enablement update; 25H2 loses security updates
    /// a year earlier). Any other build keeps the release it runs now (DisplayVersion, for example "24H2").
    pub fn pin_release(build: i32, display_version: Option<&str>) -> String {
        if build == 26200 || build == 26300 {
            return "26H2".into();
        }
        let v = ci::upper(display_version.unwrap_or("").trim());
        if release_text(&v) { v } else { "26H2".into() }
    }

    /// The security switches of 1.6-1.11 (memory integrity / VBS / hypervisor, Spectre and Meltdown workarounds,
    /// exploit-protection defaults, Defender and SmartScreen). They are no longer offered anywhere. Only their values
    /// are kept, so that a PC where one of them was switched on gets its saved previous values back (`revert_retired`,
    /// at every start and in the factory reset). Ids must stay as they were: the backup is keyed by them.
    pub fn retired() -> &'static [Tweak] {
        static R: OnceLock<Vec<Tweak>> = OnceLock::new();
        R.get_or_init(|| {
            let mm = r"SYSTEM\CurrentControlSet\Control\Session Manager\Memory Management";
            let kernel = r"SYSTEM\CurrentControlSet\Control\Session Manager\kernel";
            let rtp = format!(r"{}\Real-Time Protection", WD);
            vec![
                Tweak {
                    id: "sec-vbs".into(),
                    changes: vec![
                        d("HKLM", &format!(r"{}\Scenarios\HypervisorEnforcedCodeIntegrity", DG), "Enabled", 0, Some(1)),
                        d("HKLM", DG, "EnableVirtualizationBasedSecurity", 0, None),
                        d("HKLM", &format!(r"{}\Scenarios\KernelShadowStacks", DG), "Enabled", 0, None),
                        d("HKLM", &format!(r"{}\Scenarios\CredentialGuard", DG), "Enabled", 0, None),
                        RegChange::new("BCD", "{current}", "hypervisorlaunchtype", "sz", "Off", None),
                    ],
                    ..Default::default()
                },
                Tweak {
                    id: "sec-cpu".into(),
                    changes: vec![d("HKLM", mm, "FeatureSettingsOverride", 3, None), d("HKLM", mm, "FeatureSettingsOverrideMask", 3, None)],
                    ..Default::default()
                },
                Tweak {
                    id: "sec-exploit".into(),
                    changes: vec![b("HKLM", kernel, "MitigationOptions", &"2".repeat(48), None), b("HKLM", kernel, "MitigationAuditOptions", &"2".repeat(48), None)],
                    ..Default::default()
                },
                Tweak {
                    id: "sec-defender".into(),
                    changes: vec![
                        d("HKLM", WD, "DisableAntiSpyware", 1, None),
                        d("HKLM", &rtp, "DisableRealtimeMonitoring", 1, None),
                        d("HKLM", &rtp, "DisableBehaviorMonitoring", 1, None),
                        d("HKLM", &rtp, "DisableScanOnRealtimeEnable", 1, None),
                        d("HKLM", &rtp, "DisableOnAccessProtection", 1, None),
                        d("HKLM", r"SOFTWARE\Policies\Microsoft\Windows\System", "EnableSmartScreen", 0, None),
                        s("HKLM", r"SOFTWARE\Microsoft\Windows\CurrentVersion\Explorer", "SmartScreenEnabled", "Off", Some("Warn")),
                    ],
                    ..Default::default()
                },
            ]
        })
    }

    fn is_retired(t: &Tweak) -> bool {
        Self::retired().iter().any(|r| std::ptr::eq(r, t))
    }

    /// Puts back the previous values of every retired security switch that this program switched on earlier (it has a
    /// backup). Switches without a backup were not changed by this program and are left alone. A failed revert keeps its
    /// backup and is tried again at the next start. Returns lines for the log.
    pub fn revert_retired() -> Vec<String> {
        let mut log = Vec::new();
        let backup = Self::load_backup();
        for t in Self::retired().iter().filter(|t| backup.contains_key(&t.id)) {
            log.push(match Self::revert(t) {
                None => lf!("A removed protection switch ({0}) was put back to your previous values; restart Windows", t.id.as_str()),
                Some(err) => lf!("A removed protection switch ({0}) could not be put back yet: {1}", t.id.as_str(), err),
            });
        }
        log
    }

    fn present(t: &Tweak) -> Result<Vec<&RegChange>, RegError> {
        let reg = Self::reg();
        let mut v = Vec::new();
        for c in &t.changes {
            if !c.if_key_exists || reg.has_key(&c.hive, &c.path)? {
                v.push(c);
            }
        }
        Ok(v)
    }

    /// Err: the registry could not be read (C# throws to the caller).
    pub fn state(t: &Tweak) -> Result<TweakState, RegError> {
        let reg = Self::reg();
        let list = Self::present(t)?;
        let mut on = 0;
        for c in &list {
            if reg.get(&c.hive, &c.path, &c.name)?.map(|v| ci::eq(&v, &c.on)).unwrap_or(false) {
                on += 1;
            }
        }
        Ok(if !list.is_empty() && on == list.len() {
            TweakState::On
        } else if on == 0 {
            TweakState::Off
        } else {
            TweakState::Mixed
        })
    }

    // ---- backup: the values a change replaced, kept until the change is reverted ----------------------------

    fn load_backup() -> Backup {
        let r = (|| -> Option<Backup> {
            let p = Self::backup_path().ok()?;
            if !p.is_file() {
                return None;
            }
            parse_backup(&read_all_text(&p).ok()?).ok()?
        })();
        r.unwrap_or_default()
    }

    /// Whether a saved value may be written back for this change (pure): the change's own value name (the caller only
    /// looks up its own changes), in the change's type (a 32-bit number for a DWORD, hex bytes for a binary value, a
    /// short printable text for a string) and for the boot entry only "Auto". The retired security switches also
    /// refuse a value that would keep a protection off: their "Disable…" values and the "Enabled" switches accept only
    /// 0 / 1 (the value that turns them off is replaced by the Windows default anyway), SmartScreen only its own modes.
    /// "\0" (there was no value) is always fine.
    pub fn acceptable(c: &RegChange, saved: Option<&str>, retired: bool) -> bool {
        let Some(saved) = saved else { return true };
        if saved == "\0" {
            return true;
        }
        if c.hive == "BCD" {
            return ci::eq(saved, "Auto");
        }
        let n16 = saved.encode_utf16().count();
        let type_ok = match c.kind.as_str() {
            "dword" => crate::base::settings::parse_int_leading_sign(saved).is_some() || (!saved.is_empty() && saved.bytes().all(|x| x.is_ascii_digit()) && saved.parse::<u32>().is_ok()),
            "binary" => n16 <= 256 && n16 % 2 == 0 && saved.chars().all(|x| x.is_ascii_hexdigit()),
            _ => n16 <= 64 && !saved.encode_utf16().any(|u| u < 0x20 || (0x7F..=0x9F).contains(&u)),
        };
        if !type_ok {
            return false;
        }
        if c.name == "TargetReleaseVersionInfo" {
            return release_text(saved);
        }
        if c.name == "ProductVersion" {
            return saved == "Windows 10" || saved == "Windows 11";
        }
        if !retired {
            return true;
        }
        if c.name == "SmartScreenEnabled" {
            return matches!(saved, "Warn" | "RequireAdmin" | "Prompt" | "Off");
        }
        if c.name.starts_with("Disable") || c.name == "Enabled" || c.name == "EnableVirtualizationBasedSecurity" {
            return saved == "0" || saved == "1";
        }
        true
    }

    /// Takes over the backup of an older version (from the user's folder): only changes of the catalogue or the retired
    /// list, only their own values, only values `acceptable` lets through. None when nothing is left. Err: not JSON.
    pub fn import_backup(json: &str, existing: Option<&str>, mut dropped: Option<&mut Vec<String>>) -> Result<Option<String>, String> {
        let src = parse_backup(json)?.unwrap_or_default();
        let mut res: Backup = match existing {
            None => Backup::new(),
            Some(e) => parse_backup(&Self::import_backup(e, None, None)?.unwrap_or_else(|| "{}".into()))?.unwrap_or_default(),
        };
        let mut fresh = Backup::new();
        for (id, vals) in &src {
            let t = Self::catalog().iter().chain(Self::retired()).chain(Self::withdrawn()).find(|x| x.id == *id);
            let (Some(t), Some(vals)) = (t, vals) else {
                if let Some(d) = dropped.as_deref_mut() {
                    d.push(id.clone());
                }
                continue;
            };
            let retired = Self::is_retired(t);
            let mut keep = IndexMap::new();
            for (key, v) in vals {
                match t.changes.iter().find(|x| ci::eq(&Self::id(x), key)) {
                    Some(c) if Self::acceptable(c, v.as_deref(), retired) => {
                        keep.insert(Self::id(c), v.clone());
                    }
                    _ => {
                        if let Some(d) = dropped.as_deref_mut() {
                            d.push(format!("{}: {}", id, key));
                        }
                    }
                }
            }
            fresh.insert(id.clone(), Some(keep)); // an entry without values still says "this program switched it on"
        }
        for (k, v) in fresh {
            res.entry(k).or_insert(v); // what the state folder already has wins
        }
        Ok(if res.is_empty() { None } else { Some(to_json_indented(&OrderedBackup(&res))) })
    }

    fn save_backup(b: &Backup) -> Result<(), RegError> {
        // An UnauthorizedAccessException of the file calls is "access denied" too.
        let io = |e: std::io::Error| RegError { access_denied: e.kind() == std::io::ErrorKind::PermissionDenied, message: e.to_string() };
        let p = Self::backup_path().map_err(RegError::new)?;
        create_parent(&p).map_err(io)?;
        let tmp = PathBuf::from(format!("{}.tmp", p.display()));
        write_all_text(&tmp, &to_json_indented(&OrderedBackup(b))).map_err(io)?;
        move_overwrite(&tmp, &p).map_err(io)
    }

    fn id(c: &RegChange) -> String {
        format!("{}\\{}\\{}", c.hive, c.path, c.name)
    }

    pub fn has_backup(t: &Tweak) -> bool {
        Self::load_backup().contains_key(&t.id)
    }

    fn denied_or(e: RegError) -> String {
        if e.access_denied { L::t("Access denied: run as administrator").to_string() } else { e.message }
    }

    /// Switches a change on: the previous values are saved first, each value is read back. Returns an error text or
    /// None.
    pub fn apply(t: &Tweak) -> Option<String> {
        let r = (|| -> Result<Option<String>, RegError> {
            if let Some(pre) = t.pre_check.as_ref().and_then(|f| f()) {
                return Ok(Some(pre));
            }
            let reg = Self::reg();
            let mut backup = Self::load_backup();
            if !backup.contains_key(&t.id) {
                // "\0" marks "there was no value" (an empty string is a real value).
                let mut vals = IndexMap::new();
                for c in Self::present(t)? {
                    // ToDictionary: a value id listed twice would throw; the catalogue has none.
                    vals.insert(Self::id(c), Some(reg.get(&c.hive, &c.path, &c.name)?.unwrap_or_else(|| "\0".into())));
                }
                backup.insert(t.id.clone(), Some(vals));
                Self::save_backup(&backup)?;
            }
            // Every value is tried; the ones Windows refused are named at the end (the change then shows "Partly").
            let mut failed = Vec::new();
            for c in Self::present(t)? {
                if reg.get(&c.hive, &c.path, &c.name)?.map(|v| ci::eq(&v, &c.on)).unwrap_or(false) {
                    continue;
                }
                if let Err(e) = reg.set(&c.hive, &c.path, &c.name, &c.kind, Some(&c.on)) {
                    if t.changes.len() <= 1 {
                        return Err(e);
                    }
                }
                if !reg.get(&c.hive, &c.path, &c.name)?.map(|v| ci::eq(&v, &c.on)).unwrap_or(false) {
                    failed.push(if c.if_key_exists { c.path.rsplit('\\').next().unwrap_or("").to_string() } else { c.name.clone() });
                }
            }
            Ok(if failed.is_empty() { None } else { Some(lf!("Windows refused: {0}", failed.join(", "))) })
        })();
        r.unwrap_or_else(|e| Some(Self::denied_or(e)))
    }

    /// Switches a change off: the saved values come back; without a backup the Windows defaults are written.
    pub fn revert(t: &Tweak) -> Option<String> {
        let r = (|| -> Result<(), RegError> {
            let reg = Self::reg();
            let mut backup = Self::load_backup();
            let saved: Option<IndexMap<String, Option<String>>> = backup.get(&t.id).cloned().flatten();
            let retired = Self::is_retired(t);
            for c in Self::present(t)? {
                let raw: Option<&Option<String>> = saved.as_ref().and_then(|m| m.get(&Self::id(c)));
                let mut want: Option<String> = match raw {
                    Some(s) => match s.as_deref() {
                        Some("\0") => None,
                        other => other.map(|x| x.to_string()),
                    },
                    None => c.default.clone(),
                };
                // Only a value this change itself uses is written back; anything else falls back to the Windows default.
                if let Some(rv) = raw {
                    if !Self::acceptable(c, rv.as_deref(), retired) {
                        AppLog::add_sev(
                            lf!("System changes: the saved value of {0} is not one this change uses; the Windows default is written instead", c.name.as_str()),
                            Sev::Warn,
                        );
                        want = c.default.clone();
                    }
                }
                // A backup taken while the change was already on would "restore" the change itself: the default is used then.
                if let Some(w) = &want {
                    if ci::eq(w, &c.on) && c.default.as_deref() != Some(c.on.as_str()) {
                        want = c.default.clone();
                    }
                }
                let now = reg.get(&c.hive, &c.path, &c.name)?;
                match &want {
                    None => {
                        if now.is_some() {
                            reg.delete(&c.hive, &c.path, &c.name)?;
                        }
                    }
                    Some(w) => {
                        if !now.map(|n| ci::eq(&n, w)).unwrap_or(false) {
                            reg.set(&c.hive, &c.path, &c.name, &c.kind, Some(w))?;
                        }
                    }
                }
            }
            if backup.shift_remove(&t.id).is_some() {
                Self::save_backup(&backup)?;
            }
            Ok(())
        })();
        r.err().map(Self::denied_or)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::base::hooks::static_hooks;
    use crate::base::settings::{MemorySettingsBackend, Settings};

    struct Env {
        reg: Arc<MemoryRegStore>,
        dir: PathBuf,
        _g: std::sync::MutexGuard<'static, ()>,
        _b: crate::base::settings::BackendGuard,
    }

    impl Env {
        fn new(file: &'static str) -> Env {
            let g = static_hooks();
            let b = Settings::scoped_backend(Arc::new(MemorySettingsBackend::new()));
            let reg = Arc::new(MemoryRegStore::new());
            Tweaks::set_reg(Some(reg.clone()));
            let dir = std::env::temp_dir().join(format!("ds-tweaks-{}", super::super::util::guid_n()));
            let d2 = dir.clone();
            BACKUP_PATH_HOOK.set(Some(Arc::new(move || d2.join(file))));
            AppLog::clear();
            Env { reg, dir, _g: g, _b: b }
        }
        fn get(&self, h: &str, p: &str, n: &str) -> Option<String> {
            self.reg.get(h, p, n).unwrap()
        }
        fn set(&self, h: &str, p: &str, n: &str, k: &str, v: &str) {
            self.reg.set(h, p, n, k, Some(v)).unwrap()
        }
    }

    impl Drop for Env {
        fn drop(&mut self) {
            Tweaks::set_reg(None);
            BACKUP_PATH_HOOK.set(None);
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    fn find<'a>(list: &'a [Tweak], id: &str) -> &'a Tweak {
        list.iter().find(|t| t.id == id).unwrap()
    }

    /// SystemToolsTests.EveryChangeIsExplainedAndTouchesNoSecuritySetting
    #[test]
    fn every_change_is_explained_and_touches_no_security_setting() {
        assert_eq!(Tweaks::catalog().len(), 4); // 1.16.2: game DVR, pointer acceleration, two Windows Update changes
        let mut ids: Vec<&str> = Tweaks::catalog().iter().map(|t| t.id.as_str()).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), Tweaks::catalog().len());
        let forbidden = [
            "Defender", "SmartScreen", "DeviceGuard", "HypervisorEnforcedCodeIntegrity", "Mitigation", "SecureBoot", "EnableLUA", "Lsa", "Spectre",
            "FeatureSettingsOverride", "DisableAntiSpyware",
        ];
        for t in Tweaks::catalog().iter().chain(Tweaks::withdrawn()) {
            assert!(t.what.chars().count() > 20 && t.why.chars().count() > 20 && t.cost.chars().count() > 5, "{}", t.id);
            assert!(!t.changes.is_empty());
            for c in &t.changes {
                assert!(c.hive == "HKLM" || c.hive == "HKCU");
                assert!(!forbidden.iter().any(|f| ci::contains(&format!("{}\\{}", c.path, c.name), f)));
                assert_ne!(Some(c.on.as_str()), c.default.as_deref());
            }
        }
    }

    /// SystemToolsTests.SecuritySwitchesAreNoLongerOffered (the reflection check has no Rust counterpart)
    #[test]
    fn security_switches_are_no_longer_offered() {
        assert!(!Tweaks::catalog().iter().any(|t| t.id.starts_with("sec-")));
        let ids: Vec<&str> = Tweaks::retired().iter().map(|t| t.id.as_str()).collect();
        assert_eq!(ids, ["sec-vbs", "sec-cpu", "sec-exploit", "sec-defender"]);
    }

    /// SystemToolsTests.RetiredSecuritySwitchesAreRevertedFromTheirBackupOnly
    #[test]
    fn retired_security_switches_are_reverted_from_their_backup_only() {
        let e = Env::new("backup.json");
        let cpu = find(Tweaks::retired(), "sec-cpu");
        let vbs = find(Tweaks::retired(), "sec-vbs");
        let def = find(Tweaks::retired(), "sec-defender");
        let mm = r"SYSTEM\CurrentControlSet\Control\Session Manager\Memory Management";
        // An older version switched these on: the previous values went to the backup first.
        e.set("HKLM", mm, "FeatureSettingsOverride", "dword", "72"); // the user's own value before
        assert!(Tweaks::apply(cpu).is_none());
        assert!(Tweaks::apply(vbs).is_none());
        assert_eq!(e.get("HKLM", mm, "FeatureSettingsOverride").as_deref(), Some("3"));
        assert_eq!(e.get("BCD", "{current}", "hypervisorlaunchtype").as_deref(), Some("Off"));
        // Defender policies set by something else (no backup of this program): left alone.
        for c in &def.changes {
            e.set(&c.hive, &c.path, &c.name, &c.kind, &c.on);
        }
        let log = Tweaks::revert_retired();
        assert_eq!(log.len(), 2);
        assert_eq!(e.get("HKLM", mm, "FeatureSettingsOverride").as_deref(), Some("72")); // exactly the previous value
        assert!(e.get("HKLM", mm, "FeatureSettingsOverrideMask").is_none()); // there was none: there is none again
        assert!(e.get("BCD", "{current}", "hypervisorlaunchtype").is_none());
        assert!(e.get("HKLM", r"SYSTEM\CurrentControlSet\Control\DeviceGuard\Scenarios\HypervisorEnforcedCodeIntegrity", "Enabled").is_none());
        assert!(!Tweaks::has_backup(cpu));
        assert!(!Tweaks::has_backup(vbs));
        assert_eq!(Tweaks::state(def).unwrap(), TweakState::On);
        let writes = e.reg.writes();
        assert!(Tweaks::revert_retired().is_empty()); // nothing left to do
        assert_eq!(writes, e.reg.writes());
    }

    /// SystemToolsTests.StayOnThisReleaseUsesTheRunningRelease
    #[test]
    fn stay_on_this_release_uses_the_running_release() {
        assert_eq!(Tweaks::pin_release(26200, Some("25H2")), "26H2"); // held on 26H2, as the playbook does
        assert_eq!(Tweaks::pin_release(26300, Some("26H2")), "26H2");
        assert_eq!(Tweaks::pin_release(26100, Some("24H2")), "24H2");
        assert_eq!(Tweaks::pin_release(0, None), "26H2");
        let wu = find(Tweaks::catalog(), "wu-feature");
        assert!(!wu.changes.iter().any(|c| c.on == "25H2"));
    }

    /// SystemToolsTests.ApplySavesWhatWasThereAndRevertPutsExactlyThatBack
    #[test]
    fn apply_saves_what_was_there_and_revert_puts_exactly_that_back() {
        let e = Env::new("backup.json");
        let dvr = find(Tweaks::catalog(), "game-dvr");
        let first = &dvr.changes[0];
        e.set(&first.hive, &first.path, &first.name, "dword", "7"); // an unusual value the user had
        assert_eq!(Tweaks::state(dvr).unwrap(), TweakState::Off);
        assert!(Tweaks::apply(dvr).is_none());
        assert_eq!(Tweaks::state(dvr).unwrap(), TweakState::On);
        assert!(Tweaks::has_backup(dvr));
        let writes = e.reg.writes();
        assert!(Tweaks::apply(dvr).is_none()); // already on: nothing is written
        assert_eq!(writes, e.reg.writes());

        assert!(Tweaks::revert(dvr).is_none());
        assert_eq!(e.get(&first.hive, &first.path, &first.name).as_deref(), Some("7")); // the user's own value, not a guess
        let c2 = &dvr.changes[2];
        assert!(e.get(&c2.hive, &c2.path, &c2.name).is_none()); // there was no value: there is none again
        assert!(!Tweaks::has_backup(dvr));
        assert_eq!(Tweaks::state(dvr).unwrap(), TweakState::Off);

        // Switched on by something else (the playbook): no backup, Revert writes the Windows defaults.
        let mouse = find(Tweaks::catalog(), "mouse-accel");
        for c in &mouse.changes {
            e.set(&c.hive, &c.path, &c.name, &c.kind, &c.on);
        }
        assert_eq!(Tweaks::state(mouse).unwrap(), TweakState::On);
        assert!(Tweaks::revert(mouse).is_none());
        assert_eq!(e.get("HKCU", r"Control Panel\Mouse", "MouseSpeed").as_deref(), Some("1"));
        assert_eq!(e.get("HKCU", r"Control Panel\Mouse", "MouseThreshold1").as_deref(), Some("6"));

        // One value of three set: the state is "partly".
        let m0 = &mouse.changes[0];
        e.set(&m0.hive, &m0.path, &m0.name, "sz", "0");
        assert_eq!(Tweaks::state(mouse).unwrap(), TweakState::Mixed);
    }

    /// SecurityTests.PlantedSystemChangeBackupIsNotWrittenBack
    #[test]
    fn planted_system_change_backup_is_not_written_back() {
        let e = Env::new(Tweaks::BACKUP_NAME);
        let vbs = find(Tweaks::retired(), "sec-vbs");
        for c in &vbs.changes {
            e.set(&c.hive, &c.path, &c.name, &c.kind, &c.on);
        }
        let planted = r#"{"sec-vbs":{"BCD\\{current}\\hypervisorlaunchtype":"Off /set {bootmgr} path \\evil.efi","HKLM\\SYSTEM\\CurrentControlSet\\Control\\DeviceGuard\\Scenarios\\HypervisorEnforcedCodeIntegrity\\Enabled":"not a number"},"not-a-tweak":{"HKLM\\SYSTEM\\CurrentControlSet\\Services\\evil\\ImagePath":"evil.exe"}}"#;
        let p = Tweaks::backup_path().unwrap();
        create_parent(&p).unwrap();
        std::fs::write(&p, planted).unwrap();
        Tweaks::revert_retired();
        assert!(e.get("BCD", "{current}", "hypervisorlaunchtype").is_none()); // the Windows default, not the planted text
        assert_eq!(e.get("HKLM", r"SYSTEM\CurrentControlSet\Control\DeviceGuard\Scenarios\HypervisorEnforcedCodeIntegrity", "Enabled").as_deref(), Some("1"));
        assert!(e.get("HKLM", r"SYSTEM\CurrentControlSet\Services\evil", "ImagePath").is_none());
        assert!(AppLog::tail(500).iter().any(|x| x.sev == Sev::Warn && x.msg.contains("hypervisorlaunchtype")));

        // The take-over drops unknown changes and values the change does not use.
        let mut dropped = Vec::new();
        let json = Tweaks::import_backup(planted, None, Some(&mut dropped)).unwrap().unwrap();
        assert!(!json.contains("evil"));
        assert_eq!(dropped.len(), 3);

        // The boot entry never reaches bcdedit as a command line put together from text.
        let w = WinRegStore;
        let _ = &w;
        assert!(bcd("/set", "{current} /set {bootmgr} path x", Some("hypervisorlaunchtype"), Some("Auto")).is_err());
        assert!(bcd("/set", "{current}", Some("hypervisorlaunchtype"), Some("Auto & calc")).is_err());
    }

    #[test]
    fn acceptable_values() {
        let c = RegChange::new("HKLM", "x", "DisableAntiSpyware", "dword", "1", None);
        assert!(Tweaks::acceptable(&c, Some("0"), true));
        assert!(!Tweaks::acceptable(&c, Some("2"), true));
        assert!(Tweaks::acceptable(&c, Some("4294967295"), false));
        assert!(!Tweaks::acceptable(&c, Some("4294967296"), false));
        assert!(Tweaks::acceptable(&c, Some("\0"), true));
        let r = RegChange::new("HKLM", "x", "TargetReleaseVersionInfo", "sz", "26H2", None);
        assert!(Tweaks::acceptable(&r, Some("24H2"), false));
        assert!(!Tweaks::acceptable(&r, Some("24H3"), false));
    }
}
