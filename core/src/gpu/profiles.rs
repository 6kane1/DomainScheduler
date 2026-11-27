//! Port of `GpuProfiles.cs`: the saved GPU profiles (gpu-profiles.json), the crash memory, slider ranges and the start
//! decision (GpuOcLogic).
//!
//! The file is read and written like System.Text.Json does it for these classes (case-sensitive property names in
//! declaration order, the read-only properties IsStock/Maker/StartProfile/ConfirmProfile written and ignored on read,
//! unknown properties skipped, a wrong token type or a null for a number fails the whole load, WriteIndented with the
//! default encoder), so the C# and the native program read each other's file.

use super::amd_oc::{AmdDomain, GpuOc};
use super::nv_oc::{NvClock, NvOc};
use super::clampi;
use crate::base::ci;
use crate::base::hooks::Hook;
use crate::base::time::DateTime;
use serde::de::{self, Deserializer, IgnoredAny, MapAccess, Visitor};
use serde::Deserialize;
use std::fmt;
use std::path::PathBuf;

/// One set of GPU overclock values the user saved under a name. Clocks are MHz OFFSETS from the card's stock values for
/// both makers (NVIDIA: the driver's offset itself; AMD: the distance from the stock maximum clock the driver reports,
/// read again at every apply), so a profile never holds an absolute clock. VoltPct is NVIDIA's voltage boost (0–100 %);
/// PowerPct (power limit, offset in percent) and VoltMv (voltage offset in mV, only ≤ 0 is ever written) are AMD's.
/// Vendor says for which maker's card the profile was saved ("" = before 1.15.0 Test 1.5, when only NVIDIA existed).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GpuProfile {
    pub name: String,
    pub core_mhz: i32,
    pub mem_mhz: i32,
    pub volt_pct: i32,
    pub power_pct: i32,
    pub volt_mv: i32,
    pub vendor: String,
}

impl GpuProfile {
    pub fn is_stock(&self) -> bool {
        self.core_mhz == 0 && self.mem_mhz == 0 && self.volt_pct == 0 && self.power_pct == 0 && self.volt_mv == 0
    }

    /// The maker the profile belongs to: "amd" or "nvidia" (old profiles are NVIDIA's).
    pub fn maker(&self) -> &'static str {
        if self.vendor == GpuOc::AMD {
            GpuOc::AMD
        } else {
            GpuOc::NVIDIA
        }
    }
}

/// Values that were set when Windows crashed (1.16.0): kept so the same values (or higher) are not marked for the start
/// again by one click. Clocks are offsets like a profile's. Name and Vendor are None when the file said null (C# keeps
/// the null and writes it back).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GpuCrash {
    pub name: Option<String>,
    pub core_mhz: i32,
    pub mem_mhz: i32,
    pub volt_pct: i32,
    pub power_pct: i32,
    pub vendor: Option<String>,
    pub utc: DateTime,
}

impl Default for GpuCrash {
    fn default() -> Self {
        GpuCrash { name: Some(String::new()), core_mhz: 0, mem_mhz: 0, volt_pct: 0, power_pct: 0, vendor: Some(String::new()), utc: DateTime::default() }
    }
}

/// A slider range: whole steps from Min to Max (both on the step grid, Min ≤ 0 ≤ Max when the driver allows it).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GpuRange {
    pub min: i32,
    pub max: i32,
    pub step: i32,
}

impl GpuRange {
    pub const fn new(min: i32, max: i32, step: i32) -> Self {
        GpuRange { min, max, step }
    }

    /// Clamps into the range and rounds to the nearest whole step (steps counted from zero).
    pub fn snap(&self, value: f64) -> i32 {
        if value.is_nan() {
            return 0;
        }
        let v = value.max(self.min as f64).min(self.max as f64);
        let s = if self.step < 1 { 1 } else { self.step };
        // Math.Round(v / s, MidpointRounding.AwayFromZero): f64::round rounds halves away from zero.
        let mut r = ((v / s as f64).round() as i32).wrapping_mul(s);
        while r > self.max {
            r -= s;
        }
        while r < self.min {
            r += s;
        }
        clampi(r, self.min, self.max)
    }
}

/// `GpuProfiles.PathHook` (tests: where the file is).
pub static PATH_HOOK: Hook<dyn Fn() -> Option<PathBuf> + Send + Sync> = Hook::new();

/// The saved GPU profiles: gpu-profiles.json in the data folder.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GpuProfiles {
    pub profiles: Vec<GpuProfile>,
    /// The profile set again when the program starts (empty = none).
    pub at_start: String,
    /// The profile that was NOT re-applied at the last start because of the crash guard (empty = none).
    pub skipped: String,
    pub skipped_utc: DateTime,
    /// The values set before each crash the guard saw, newest last (at most MAX_CRASHES).
    pub crashes: Vec<GpuCrash>,
    /// The profile that was marked "apply at start" in a version before 1.15.0 Test 1.3 (empty = none). The old file lay
    /// in a folder any program of the user can change, so the mark is not taken over as it is: the program asks once,
    /// in its own (administrator) window, whether to keep applying this profile at start. Yes makes it AtStart, No
    /// clears it; the profile itself stays in the list either way.
    pub confirm: String,
}

impl GpuProfiles {
    pub const MAX_CRASHES: usize = 10;
    pub const MAX_NAME: usize = 40;
    pub const FILE_NAME: &'static str = "gpu-profiles.json";

    pub fn new() -> Self {
        Self::default()
    }

    /// `GpuProfiles.FilePath`: the hook's path, else the data folder's file. Err: the data folder is not safe to use.
    pub fn file_path() -> Result<PathBuf, String> {
        if let Some(h) = PATH_HOOK.get() {
            if let Some(p) = h() {
                return Ok(p);
            }
        }
        Ok(crate::base::state_dir::store_dir()?.join(Self::FILE_NAME))
    }

    /// Remembers the profile's values as crashed (called by the crash guard at start).
    pub fn record_crash(&mut self, p: Option<&GpuProfile>, utc: DateTime) {
        let Some(p) = p else { return };
        self.crashes.push(GpuCrash {
            name: Some(p.name.clone()),
            core_mhz: p.core_mhz,
            mem_mhz: p.mem_mhz,
            volt_pct: p.volt_pct,
            power_pct: p.power_pct,
            vendor: Some(p.maker().to_string()),
            utc,
        });
        while self.crashes.len() > Self::MAX_CRASHES {
            self.crashes.remove(0);
        }
    }

    fn find_index(&self, name: Option<&str>) -> Option<usize> {
        let name = name?;
        if name.trim().is_empty() {
            return None;
        }
        let t = name.trim();
        self.profiles.iter().position(|p| ci::eq(&p.name, t))
    }

    pub fn find(&self, name: &str) -> Option<&GpuProfile> {
        self.find_index(Some(name)).map(|i| &self.profiles[i])
    }

    pub fn find_mut(&mut self, name: &str) -> Option<&mut GpuProfile> {
        self.find_index(Some(name)).map(move |i| &mut self.profiles[i])
    }

    pub fn start_profile(&self) -> Option<&GpuProfile> {
        self.find(&self.at_start)
    }

    /// A usable name: trimmed, 1–40 characters, no control characters. None when it is not usable.
    pub fn clean_name(name: Option<&str>) -> Option<String> {
        let n = name.unwrap_or("").trim();
        let len = n.encode_utf16().count();
        if len == 0 || len > Self::MAX_NAME || n.encode_utf16().any(is_control_unit) {
            return None;
        }
        Some(n.to_string())
    }

    /// Saves the values under a name; a profile with the same name (any case) is overwritten. False for a bad name.
    #[allow(clippy::too_many_arguments)]
    pub fn put(&mut self, name: &str, core_mhz: i32, mem_mhz: i32, volt_pct: i32, power_pct: i32, volt_mv: i32, vendor: Option<&str>) -> bool {
        let Some(n) = Self::clean_name(Some(name)) else { return false };
        let i = match self.find_index(Some(&n)) {
            Some(i) => i,
            None => {
                self.profiles.push(GpuProfile::default());
                self.profiles.len() - 1
            }
        };
        let p = &mut self.profiles[i];
        p.name = n;
        p.core_mhz = core_mhz;
        p.mem_mhz = mem_mhz;
        p.volt_pct = clampi(volt_pct, 0, 100);
        p.power_pct = clampi(power_pct, -100, 100);
        p.volt_mv = clampi(volt_mv, -500, 0);
        p.vendor = match vendor {
            Some(v) if v == GpuOc::AMD => GpuOc::AMD.into(),
            Some(v) if v == GpuOc::NVIDIA => GpuOc::NVIDIA.into(),
            _ => String::new(),
        };
        true
    }

    /// `Put(name, core, mem, volt)` with the C# defaults (power 0, voltage offset 0, no maker).
    pub fn put3(&mut self, name: &str, core_mhz: i32, mem_mhz: i32, volt_pct: i32) -> bool {
        self.put(name, core_mhz, mem_mhz, volt_pct, 0, 0, None)
    }

    /// Renames a profile (the "apply at start" mark follows it). False when it is missing, the name is bad or taken.
    pub fn rename(&mut self, from: &str, to: &str) -> bool {
        let p = self.find_index(Some(from));
        let n = Self::clean_name(Some(to));
        let (Some(p), Some(n)) = (p, n) else { return false };
        if let Some(other) = self.find_index(Some(&n)) {
            if other != p {
                return false;
            }
        }
        let start = self.find_index(Some(&self.at_start)) == Some(p);
        let old = self.profiles[p].name.clone();
        let skipped = ci::eq(&self.skipped, &old);
        let confirm = ci::eq(&self.confirm, &old);
        self.profiles[p].name = n.clone();
        if start {
            self.at_start = n.clone();
        }
        if skipped {
            self.skipped = n.clone();
        }
        if confirm {
            self.confirm = n;
        }
        true
    }

    /// Deletes a profile; its "apply at start" mark goes with it.
    pub fn delete(&mut self, name: &str) -> bool {
        let Some(p) = self.find_index(Some(name)) else { return false };
        if self.find_index(Some(&self.at_start)) == Some(p) {
            self.at_start = String::new();
        }
        let old = self.profiles[p].name.clone();
        if ci::eq(&self.skipped, &old) {
            self.skipped = String::new();
        }
        if ci::eq(&self.confirm, &old) {
            self.confirm = String::new();
        }
        self.profiles.remove(p);
        true
    }

    /// Marks one profile (or none, with None/empty) to be applied at start. Marking clears the crash-guard notice; any
    /// choice made here also answers the pending question about the old mark (Confirm).
    pub fn set_at_start(&mut self, name: Option<&str>) -> bool {
        let Some(name) = name.filter(|n| !n.trim().is_empty()) else {
            self.at_start = String::new();
            self.confirm = String::new();
            return true;
        };
        let Some(p) = self.find_index(Some(name)) else { return false };
        self.at_start = self.profiles[p].name.clone();
        self.skipped = String::new();
        self.confirm = String::new();
        true
    }

    /// The profile whose old "apply at start" mark still waits for the user's answer (None = none).
    pub fn confirm_profile(&self) -> Option<&GpuProfile> {
        if !self.at_start.is_empty() {
            None
        } else {
            self.find(&self.confirm)
        }
    }

    /// `GpuProfiles.Load(path)`: the store in the file (path None: `file_path()`); an empty store when the file is
    /// missing, unreadable or not a profile file. Never fails.
    pub fn load(path: Option<&std::path::Path>) -> GpuProfiles {
        let path = match path {
            Some(p) => p.to_path_buf(),
            None => match Self::file_path() {
                Ok(p) => p,
                Err(_) => return GpuProfiles::new(),
            },
        };
        if !path.is_file() {
            return GpuProfiles::new();
        }
        match std::fs::read(&path) {
            Ok(bytes) => Self::parse(&read_all_text(&bytes)),
            Err(_) => GpuProfiles::new(),
        }
    }

    /// The Load rules on a text (what `Load` does after `File.ReadAllText`): names cleaned, values clamped, duplicates
    /// dropped, marks that point at no profile dropped. An empty store for anything that is not a profile file.
    pub fn parse(text: &str) -> GpuProfiles {
        let raw: Option<RawRoot> = match serde_json::from_str::<Option<RawRoot>>(text) {
            Ok(r) => r,
            Err(_) => return GpuProfiles::new(),
        };
        let raw = raw.unwrap_or_default();
        let mut g = GpuProfiles::new();
        // Profiles: non-null with a usable name, the first of each name (any case), in the order of first appearance.
        for p in raw.profiles.unwrap_or_default().into_iter().flatten() {
            if Self::clean_name(p.name.as_deref()).is_none() {
                continue;
            }
            let name = p.name.unwrap_or_default();
            let key = name.trim();
            if g.profiles.iter().any(|q| ci::eq(&q.name, key)) {
                continue;
            }
            g.profiles.push(GpuProfile {
                name: key.to_string(),
                core_mhz: p.core_mhz,
                mem_mhz: p.mem_mhz,
                volt_pct: clampi(p.volt_pct, 0, 100),
                power_pct: clampi(p.power_pct, -100, 100),
                volt_mv: clampi(p.volt_mv, -500, 0),
                vendor: match p.vendor.as_deref() {
                    Some(v) if v == GpuOc::AMD || v == GpuOc::NVIDIA => v.to_string(),
                    _ => String::new(),
                },
            });
        }
        g.at_start = g.find_index(raw.at_start.as_deref()).map(|i| g.profiles[i].name.clone()).unwrap_or_default();
        g.skipped = raw.skipped.unwrap_or_default();
        g.skipped_utc = raw.skipped_utc;
        let crashes: Vec<GpuCrash> = raw.crashes.unwrap_or_default().into_iter().flatten().collect();
        let skip = crashes.len().saturating_sub(Self::MAX_CRASHES);
        g.crashes = crashes.into_iter().skip(skip).collect();
        g.confirm = if !g.at_start.is_empty() {
            String::new()
        } else {
            g.find_index(raw.confirm.as_deref()).map(|i| g.profiles[i].name.clone()).unwrap_or_default()
        };
        g
    }

    /// Atomic write (temporary file, then a move). Err: the data folder or the file cannot be written (C# throws).
    pub fn save(&self, path: Option<&std::path::Path>) -> Result<(), String> {
        let path = match path {
            Some(p) => p.to_path_buf(),
            None => Self::file_path()?,
        };
        if let Some(dir) = path.parent() {
            if !dir.as_os_str().is_empty() {
                std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
            }
        }
        let mut tmp = path.clone().into_os_string();
        tmp.push(".tmp");
        let tmp = PathBuf::from(tmp);
        std::fs::write(&tmp, self.to_json()).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, &path).map_err(|e| e.to_string())
    }

    /// `JsonSerializer.Serialize(this, new JsonSerializerOptions { WriteIndented = true })`.
    pub fn to_json(&self) -> String {
        let mut w = JsonWriter::new();
        w.begin_object();
        w.key("Profiles");
        w.begin_array();
        for p in &self.profiles {
            w.item();
            write_profile(&mut w, Some(p));
        }
        w.end_array();
        w.key("AtStart");
        w.string(Some(&self.at_start));
        w.key("Skipped");
        w.string(Some(&self.skipped));
        w.key("SkippedUtc");
        w.raw(&format!("\"{}\"", stj_date(self.skipped_utc)));
        w.key("Crashes");
        w.begin_array();
        for c in &self.crashes {
            w.item();
            w.begin_object();
            w.key("Name");
            w.string(c.name.as_deref());
            w.key("CoreMHz");
            w.raw(&c.core_mhz.to_string());
            w.key("MemMHz");
            w.raw(&c.mem_mhz.to_string());
            w.key("VoltPct");
            w.raw(&c.volt_pct.to_string());
            w.key("PowerPct");
            w.raw(&c.power_pct.to_string());
            w.key("Vendor");
            w.string(c.vendor.as_deref());
            w.key("Utc");
            w.raw(&format!("\"{}\"", stj_date(c.utc)));
            w.end_object();
        }
        w.end_array();
        w.key("Confirm");
        w.string(Some(&self.confirm));
        w.key("StartProfile");
        write_profile(&mut w, self.start_profile());
        w.key("ConfirmProfile");
        write_profile(&mut w, self.confirm_profile());
        w.end_object();
        w.out
    }
}

/// `char.IsControl` on one UTF-16 code unit (Unicode category Cc).
fn is_control_unit(c: u16) -> bool {
    c < 0x20 || (0x7F..=0x9F).contains(&c)
}

/// `File.ReadAllText`: UTF-8 (a BOM is dropped) or UTF-16/32 by their BOM; invalid bytes become U+FFFD.
fn read_all_text(b: &[u8]) -> String {
    if b.starts_with(&[0xEF, 0xBB, 0xBF]) {
        return String::from_utf8_lossy(&b[3..]).into_owned();
    }
    if b.starts_with(&[0xFF, 0xFE]) && !b.starts_with(&[0xFF, 0xFE, 0, 0]) {
        let u: Vec<u16> = b[2..].chunks(2).map(|c| u16::from_le_bytes([c[0], *c.get(1).unwrap_or(&0)])).collect();
        return String::from_utf16_lossy(&u);
    }
    if b.starts_with(&[0xFE, 0xFF]) {
        let u: Vec<u16> = b[2..].chunks(2).map(|c| u16::from_be_bytes([c[0], *c.get(1).unwrap_or(&0)])).collect();
        return String::from_utf16_lossy(&u);
    }
    String::from_utf8_lossy(b).into_owned()
}

/// How System.Text.Json writes a DateTime: ISO 8601 with the fraction only when it is not zero (trailing zeros cut),
/// "Z" for Utc, the offset for Local, nothing for Unspecified.
fn stj_date(d: DateTime) -> String {
    let o = d.to_o(); // yyyy-MM-ddTHH:mm:ss.fffffffK
    let (head, rest) = o.split_at(19);
    let frac = rest[1..8].trim_end_matches('0');
    let k = &rest[8..];
    if frac.is_empty() {
        format!("{}{}", head, k)
    } else {
        format!("{}.{}{}", head, frac, k)
    }
}

fn write_profile(w: &mut JsonWriter, p: Option<&GpuProfile>) {
    let Some(p) = p else {
        w.raw("null");
        return;
    };
    w.begin_object();
    w.key("Name");
    w.string(Some(&p.name));
    w.key("CoreMHz");
    w.raw(&p.core_mhz.to_string());
    w.key("MemMHz");
    w.raw(&p.mem_mhz.to_string());
    w.key("VoltPct");
    w.raw(&p.volt_pct.to_string());
    w.key("PowerPct");
    w.raw(&p.power_pct.to_string());
    w.key("VoltMv");
    w.raw(&p.volt_mv.to_string());
    w.key("Vendor");
    w.string(Some(&p.vendor));
    w.key("IsStock");
    w.raw(if p.is_stock() { "true" } else { "false" });
    w.key("Maker");
    w.string(Some(p.maker()));
    w.end_object();
}

/// The indented Utf8JsonWriter: two spaces per level, `"key": value`, Environment.NewLine between lines, an empty
/// array as `[]`, strings through JavaScriptEncoder.Default.
struct JsonWriter {
    out: String,
    depth: usize,
    /// Per open container: has it got an element yet.
    first: Vec<bool>,
}

#[cfg(windows)]
const NL: &str = "\r\n";
#[cfg(not(windows))]
const NL: &str = "\n";

impl JsonWriter {
    fn new() -> Self {
        JsonWriter { out: String::new(), depth: 0, first: Vec::new() }
    }

    fn newline(&mut self) {
        self.out.push_str(NL);
        for _ in 0..self.depth {
            self.out.push_str("  ");
        }
    }

    /// Before a member or an array element: the comma and the new line.
    fn sep(&mut self) {
        if let Some(f) = self.first.last_mut() {
            if *f {
                *f = false;
            } else {
                self.out.push(',');
            }
            self.newline();
        }
    }

    fn begin_object(&mut self) {
        self.out.push('{');
        self.depth += 1;
        self.first.push(true);
    }

    fn end_object(&mut self) {
        self.depth -= 1;
        let empty = self.first.pop().unwrap_or(true);
        if !empty {
            self.newline();
        }
        self.out.push('}');
    }

    fn begin_array(&mut self) {
        self.out.push('[');
        self.depth += 1;
        self.first.push(true);
    }

    fn end_array(&mut self) {
        self.depth -= 1;
        let empty = self.first.pop().unwrap_or(true);
        if !empty {
            self.newline();
        }
        self.out.push(']');
    }

    fn key(&mut self, k: &str) {
        self.sep();
        self.out.push_str(&encode(k));
        self.out.push_str(": ");
    }

    fn item(&mut self) {
        self.sep();
    }

    fn raw(&mut self, v: &str) {
        self.out.push_str(v);
    }

    fn string(&mut self, s: Option<&str>) {
        match s {
            Some(s) => {
                let e = encode(s);
                self.out.push_str(&e);
            }
            None => self.out.push_str("null"),
        }
    }
}

/// A JSON string as JavaScriptEncoder.Default writes it: printable ASCII as it is except the HTML-sensitive
/// characters, \b \t \n \f \r \\ as short escapes, everything else as \uXXXX (upper-case hex, UTF-16 units).
fn encode(s: &str) -> String {
    let mut o = String::with_capacity(s.len() + 2);
    o.push('"');
    for u in s.encode_utf16() {
        match u {
            0x08 => o.push_str("\\b"),
            0x09 => o.push_str("\\t"),
            0x0A => o.push_str("\\n"),
            0x0C => o.push_str("\\f"),
            0x0D => o.push_str("\\r"),
            0x5C => o.push_str("\\\\"),
            0x22 | 0x26 | 0x27 | 0x2B | 0x3C | 0x3E | 0x60 => o.push_str(&format!("\\u{:04X}", u)),
            0x20..=0x7E => o.push(u as u8 as char),
            _ => o.push_str(&format!("\\u{:04X}", u)),
        }
    }
    o.push('"');
    o
}

// ---- reading (System.Text.Json rules) ----------------------------------------------------------------

#[derive(Default)]
struct RawRoot {
    profiles: Option<Vec<Option<RawProfile>>>,
    at_start: Option<String>,
    skipped: Option<String>,
    skipped_utc: DateTime,
    crashes: Option<Vec<Option<GpuCrash>>>,
    confirm: Option<String>,
}

struct RawProfile {
    name: Option<String>,
    core_mhz: i32,
    mem_mhz: i32,
    volt_pct: i32,
    power_pct: i32,
    volt_mv: i32,
    vendor: Option<String>,
}

/// A DateTime property: a JSON string in ISO 8601 (anything else fails the load).
struct JsonDate(DateTime);

impl<'de> Deserialize<'de> for JsonDate {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = JsonDate;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("an ISO 8601 date")
            }
            fn visit_str<E: de::Error>(self, s: &str) -> Result<JsonDate, E> {
                DateTime::parse_iso(s, false).map(JsonDate).ok_or_else(|| E::custom("not a date"))
            }
        }
        d.deserialize_str(V)
    }
}

/// Reads an object member by member, in order (a later duplicate overwrites an earlier one, like System.Text.Json).
macro_rules! object_visitor {
    ($ty:ty, $expect:expr, |$map:ident, $key:ident, $v:ident| $body:block) => {
        impl<'de> Deserialize<'de> for $ty {
            fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                struct V;
                impl<'de> Visitor<'de> for V {
                    type Value = $ty;
                    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                        f.write_str($expect)
                    }
                    fn visit_map<A: MapAccess<'de>>(self, mut $map: A) -> Result<$ty, A::Error> {
                        let mut $v = <$ty>::fresh();
                        while let Some($key) = $map.next_key::<String>()? {
                            $body
                        }
                        Ok($v)
                    }
                }
                d.deserialize_map(V)
            }
        }
    };
}

impl RawRoot {
    fn fresh() -> Self {
        RawRoot { profiles: Some(Vec::new()), at_start: Some(String::new()), skipped: Some(String::new()), skipped_utc: DateTime::default(), crashes: Some(Vec::new()), confirm: Some(String::new()) }
    }
}

impl RawProfile {
    fn fresh() -> Self {
        RawProfile { name: Some(String::new()), core_mhz: 0, mem_mhz: 0, volt_pct: 0, power_pct: 0, volt_mv: 0, vendor: Some(String::new()) }
    }
}

impl GpuCrash {
    fn fresh() -> Self {
        GpuCrash::default()
    }
}

object_visitor!(RawRoot, "a GPU profile file", |map, key, v| {
    match key.as_str() {
        "Profiles" => v.profiles = map.next_value::<Option<Vec<Option<RawProfile>>>>()?,
        "AtStart" => v.at_start = map.next_value()?,
        "Skipped" => v.skipped = map.next_value()?,
        "SkippedUtc" => v.skipped_utc = map.next_value::<JsonDate>()?.0,
        "Crashes" => v.crashes = map.next_value::<Option<Vec<Option<GpuCrash>>>>()?,
        "Confirm" => v.confirm = map.next_value()?,
        _ => {
            map.next_value::<IgnoredAny>()?;
        }
    }
});

object_visitor!(RawProfile, "a GPU profile", |map, key, v| {
    match key.as_str() {
        "Name" => v.name = map.next_value()?,
        "CoreMHz" => v.core_mhz = map.next_value()?,
        "MemMHz" => v.mem_mhz = map.next_value()?,
        "VoltPct" => v.volt_pct = map.next_value()?,
        "PowerPct" => v.power_pct = map.next_value()?,
        "VoltMv" => v.volt_mv = map.next_value()?,
        "Vendor" => v.vendor = map.next_value()?,
        _ => {
            map.next_value::<IgnoredAny>()?;
        }
    }
});

object_visitor!(GpuCrash, "a GPU crash", |map, key, v| {
    match key.as_str() {
        "Name" => v.name = map.next_value()?,
        "CoreMHz" => v.core_mhz = map.next_value()?,
        "MemMHz" => v.mem_mhz = map.next_value()?,
        "VoltPct" => v.volt_pct = map.next_value()?,
        "PowerPct" => v.power_pct = map.next_value()?,
        "Vendor" => v.vendor = map.next_value()?,
        "Utc" => v.utc = map.next_value::<JsonDate>()?.0,
        _ => {
            map.next_value::<IgnoredAny>()?;
        }
    }
});

// ---- GpuOcLogic ----------------------------------------------------------------------------------------

/// What happens at the start of the program (`GpuOcLogic.Start`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Start {
    Nothing,
    Apply,
    SkipCrash,
}

/// The pure decisions of the GPU overclock: slider ranges and what happens at the start of the program.
pub struct GpuOcLogic;

impl GpuOcLogic {
    pub const CORE_STEP: i32 = 5;
    pub const MEM_STEP: i32 = 25;
    pub const VOLT_STEP: i32 = 1;
    /// Used when the driver did not report a range: conservative fixed ranges (still cut by NvOc's caps).
    pub const CORE_FALLBACK: GpuRange = GpuRange::new(-200, 300, Self::CORE_STEP);
    pub const MEM_FALLBACK: GpuRange = GpuRange::new(-500, 2000, Self::MEM_STEP);
    pub const VOLT: GpuRange = GpuRange::new(0, 100, Self::VOLT_STEP);
    pub const POWER_STEP: i32 = 1;
    pub const VOLT_MV_STEP: i32 = 5;

    /// The slider range of a clock: the driver's reported offset range cut by NvOc's own caps, on the step grid.
    pub fn range(c: Option<&NvClock>, domain: i32, step: i32, fallback: GpuRange) -> GpuRange {
        // No reported range: the fixed one, still cut by the same caps the driver write uses.
        let fb;
        let c = match c {
            Some(c) => c,
            None => {
                fb = NvClock { domain, min_delta_khz: fallback.min * 1000, max_delta_khz: fallback.max * 1000, ..Default::default() };
                &fb
            }
        };
        let lo = NvOc::clamp(c, i32::MIN + 1) / 1000;
        let hi = NvOc::clamp(c, i32::MAX) / 1000;
        let mut min = (lo as f64 / step as f64).ceil() as i32 * step; // inward onto the step grid
        let mut max = (hi as f64 / step as f64).floor() as i32 * step;
        if min > max {
            min = 0;
            max = 0;
        }
        GpuRange::new(min, max, step)
    }

    pub fn core_range(c: Option<&NvClock>) -> GpuRange {
        Self::range(c, NvOc::DOMAIN_CORE, Self::CORE_STEP, Self::CORE_FALLBACK)
    }
    pub fn mem_range(c: Option<&NvClock>) -> GpuRange {
        Self::range(c, NvOc::DOMAIN_MEMORY, Self::MEM_STEP, Self::MEM_FALLBACK)
    }

    /// An AMD offset range (power limit, voltage) from the driver's absolute range around its stock value.
    pub fn offset_range(d: Option<&AmdDomain>, step: i32) -> GpuRange {
        let Some(d) = d.filter(|d| d.editable) else { return GpuRange::new(0, 0, step) };
        let lo = 0i32.min(d.min.wrapping_sub(d.default));
        let hi = 0i32.max(d.max.wrapping_sub(d.default));
        let mut min = (lo as f64 / step as f64).ceil() as i32 * step;
        let mut max = (hi as f64 / step as f64).floor() as i32 * step;
        if min > max {
            min = 0;
            max = 0;
        }
        GpuRange::new(min, max, step)
    }

    pub fn power_range(d: Option<&AmdDomain>) -> GpuRange {
        Self::offset_range(d, Self::POWER_STEP)
    }

    /// The AMD voltage offset only goes down: the range ends at zero.
    pub fn volt_mv_range(d: Option<&AmdDomain>) -> GpuRange {
        let r = Self::offset_range(d, Self::VOLT_MV_STEP);
        GpuRange::new(r.min, 0, r.step)
    }

    /// Whether a profile may be applied at start on this maker's card: only on the maker it was saved for (old profiles
    /// are NVIDIA's). A profile of the other maker is never applied by itself; loading it by hand stays possible.
    pub fn fits(p: Option<&GpuProfile>, vendor: &str) -> bool {
        match p {
            Some(p) => (vendor == GpuOc::NVIDIA || vendor == GpuOc::AMD) && p.maker() == vendor,
            None => false,
        }
    }

    /// The newest crash whose values this profile reaches or exceeds on the same maker's card: every clock offset and the
    /// voltage boost / power limit at least as high. None when the profile is lower in at least one of them (or is stock).
    /// Marking such a profile for the start asks first (1.16.0): 19.11.2026 the same values were marked again 9 minutes
    /// after a crash.
    pub fn crashed_before<'a>(p: Option<&GpuProfile>, crashes: Option<&'a [GpuCrash]>) -> Option<&'a GpuCrash> {
        let p = p?;
        let crashes = crashes?;
        if p.is_stock() {
            return None;
        }
        let mut best: Option<&GpuCrash> = None;
        for c in crashes {
            // C#: c.Vendor.Length on a null Vendor (a file that said null) throws; here a null counts as "" (NVIDIA).
            let v = c.vendor.as_deref().unwrap_or("");
            let maker = if v.is_empty() { GpuOc::NVIDIA } else { v };
            if maker == p.maker() && p.core_mhz >= c.core_mhz && p.mem_mhz >= c.mem_mhz && p.volt_pct >= c.volt_pct && p.power_pct >= c.power_pct {
                // OrderByDescending(Utc).FirstOrDefault(): the newest; of equal times the first in the list.
                if best.map_or(true, |b| c.utc > b.utc) {
                    best = Some(c);
                }
            }
        }
        best
    }

    /// At the start of the program: apply the marked profile, unless the flag is closed, there is none, it is all zero,
    /// or Windows crashed after the last apply (pending = a blue screen or sudden power-off was logged since then).
    pub fn decide_start(flag_open: bool, store: Option<&GpuProfiles>, pending: bool) -> (Start, Option<GpuProfile>) {
        let p = store.and_then(|s| s.start_profile()).cloned();
        match &p {
            Some(pp) if flag_open && !pp.is_stock() => (if pending { Start::SkipCrash } else { Start::Apply }, p),
            _ => (Start::Nothing, p),
        }
    }

    /// Brings over the "apply at start" values of 1.12 and older (stored in the settings) as a profile marked for the start.
    /// Only when there is no profile file yet; returns true when the store was changed.
    #[allow(clippy::too_many_arguments)]
    pub fn migrate(store: Option<&mut GpuProfiles>, file_exists: bool, at_start: bool, core_khz: i32, mem_khz: i32, volt_pct: i32, name: &str) -> bool {
        let Some(store) = store else { return false };
        if file_exists || !at_start || (core_khz == 0 && mem_khz == 0 && volt_pct == 0) {
            return false;
        }
        if !store.put3(name, core_khz / 1000, mem_khz / 1000, volt_pct) {
            return false;
        }
        store.set_at_start(Some(name));
        true
    }
}
