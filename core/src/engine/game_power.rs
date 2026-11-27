//! Cleanup only (port of `GamePower.cs`). Test builds 1.9.1 and 1.10.0 changed power settings of the active plan
//! while a game ran (processor idle disable, core parking) and kept the values from before in game-power.json. Those
//! features are gone; Restore puts back every value the file still holds (left over from a crash or an old build) and
//! deletes the file. Nothing here changes a power setting on its own.

use crate::base::app_log::{AppLog, Sev};
use crate::base::hooks::lock;
use crate::base::l::L;
use crate::lf;
use indexmap::IndexMap;
use serde_json::Value;
use std::path::PathBuf;
use std::sync::Mutex;

/// A GUID as its 16 bytes in the order of the "D" text (data1 data2 data3 data4).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Guid(pub [u8; 16]);

impl Guid {
    pub const EMPTY: Guid = Guid([0; 16]);

    fn hex32(s: &str) -> Option<[u8; 16]> {
        if s.len() != 32 || !s.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        let mut o = [0u8; 16];
        for (i, b) in o.iter_mut().enumerate() {
            *b = u8::from_str_radix(&s[2 * i..2 * i + 2], 16).ok()?;
        }
        Some(o)
    }

    /// The "D" form only (`xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx`), what System.Text.Json reads.
    pub fn parse_d(s: &str) -> Option<Guid> {
        let b = s.as_bytes();
        if b.len() != 36 || b[8] != b'-' || b[13] != b'-' || b[18] != b'-' || b[23] != b'-' {
            return None;
        }
        Self::hex32(&s.replace('-', "")).map(Guid)
    }

    /// `Guid.TryParse`: the D, N, B and P forms with whitespace around (the X form is not taken; see the report).
    pub fn try_parse(s: &str) -> Option<Guid> {
        let s = s.trim();
        if s.len() == 32 {
            return Self::hex32(s).map(Guid);
        }
        if s.len() == 38 && ((s.starts_with('{') && s.ends_with('}')) || (s.starts_with('(') && s.ends_with(')'))) {
            return Self::parse_d(&s[1..37]);
        }
        Self::parse_d(s)
    }

    /// The "D" text in lower case (how .NET writes a Guid).
    pub fn to_d(&self) -> String {
        let h: String = self.0.iter().map(|b| format!("{:02x}", b)).collect();
        format!("{}-{}-{}-{}-{}", &h[0..8], &h[8..12], &h[12..16], &h[16..20], &h[20..32])
    }
}

const SUB_PROCESSOR: &str = "54533251-82be-4824-96c1-47b60b740d00";

/// The only settings those builds changed, with the range each one takes: processor idle disable (0/1) and the two
/// core parking minimums (percent). Anything else in the file is never written.
const MANAGED: [(&str, u32); 3] = [
    ("5d76a2ca-e8c0-402f-a133-2158492d58ad", 1),
    ("0cc5b647-c1df-4637-891a-dec35c318583", 100),
    ("0cc5b647-c1df-4637-891a-dec35c318584", 100),
];

static GATE: Mutex<()> = Mutex::new(());

/// The file's content (C# `Saved`): `{"Scheme": guid, "Values": {setting: value}}`.
#[derive(Clone, Debug, Default, PartialEq)]
struct Saved {
    scheme: Guid,
    values: IndexMap<String, u32>,
}

/// `JsonSerializer.Deserialize<Saved>` as strict as the C# one: Scheme a "D" Guid string, Values an object of uints.
fn deserialize(json: &str) -> Result<Option<Saved>, String> {
    let v: Value = serde_json::from_str(json.trim_start_matches('\u{feff}')).map_err(|e| e.to_string())?;
    let o = match v {
        Value::Null => return Ok(None),
        Value::Object(o) => o,
        _ => return Err("The JSON value could not be converted to GamePower+Saved.".into()),
    };
    let mut s = Saved::default();
    if let Some(sc) = o.get("Scheme") {
        let t = sc.as_str().ok_or("The JSON value could not be converted to System.Guid.")?;
        s.scheme = Guid::parse_d(t).ok_or("The JSON value is not in a supported Guid format.")?;
    }
    match o.get("Values") {
        None | Some(Value::Null) => {}
        Some(Value::Object(_)) => {
            // serde_json's map sorts its keys; the file order is read again from the text (C# keeps it)
            let ordered: OrderedRoot = serde_json::from_str(json.trim_start_matches('\u{feff}')).map_err(|e| e.to_string())?;
            for (k, x) in ordered.values.0 {
                let n = x.as_u64().filter(|n| *n <= u32::MAX as u64).ok_or("The JSON value could not be converted to System.UInt32.")?;
                s.values.insert(k, n as u32);
            }
        }
        Some(_) => return Err("The JSON value could not be converted to System.Collections.Generic.Dictionary.".into()),
    }
    Ok(Some(s))
}

/// The "Values" object with its keys in file order.
#[derive(Default)]
struct OrderedMap(Vec<(String, Value)>);

impl<'de> serde::Deserialize<'de> for OrderedMap {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> serde::de::Visitor<'de> for V {
            type Value = OrderedMap;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("an object")
            }
            fn visit_map<A: serde::de::MapAccess<'de>>(self, mut m: A) -> Result<OrderedMap, A::Error> {
                let mut v = Vec::new();
                while let Some((k, x)) = m.next_entry::<String, Value>()? {
                    v.push((k, x));
                }
                Ok(OrderedMap(v))
            }
        }
        d.deserialize_map(V)
    }
}

#[derive(serde::Deserialize)]
struct OrderedRoot {
    #[serde(rename = "Values", default)]
    values: OrderedMap,
}

fn serialize(s: &Saved) -> String {
    let mut parts = Vec::new();
    for (k, v) in &s.values {
        parts.push(format!("{}:{}", serde_json::to_string(k).unwrap_or_default(), v));
    }
    // insertion order kept (serde_json's map would sort the keys)
    format!("{{\"Scheme\":\"{}\",\"Values\":{{{}}}}}", s.scheme.to_d(), parts.join(","))
}

fn clean(s: Option<&Saved>, mut dropped: Option<&mut Vec<String>>) -> Saved {
    let mut c = Saved { scheme: s.map(|x| x.scheme).unwrap_or(Guid::EMPTY), values: IndexMap::new() };
    if let Some(s) = s {
        for (k, v) in &s.values {
            if GamePower::valid_value(k, *v) {
                c.values.insert(k.clone(), *v);
            } else if let Some(d) = dropped.as_deref_mut() {
                d.push(k.clone());
            }
        }
    }
    c
}

/// `GamePower` (static class).
pub struct GamePower;

impl GamePower {
    pub const FILE_NAME: &'static str = "game-power.json";

    fn file() -> Result<PathBuf, String> {
        Ok(crate::base::state_dir::store_dir()?.join(Self::FILE_NAME))
    }

    /// One saved value may be put back (pure): a managed setting and a value in its range.
    pub fn valid_value(setting: &str, value: u32) -> bool {
        let Some(g) = Guid::try_parse(setting) else { return false };
        MANAGED.iter().any(|(k, max)| Guid::parse_d(k) == Some(g) && value <= *max)
    }

    /// Takes over the file of an older version: only managed settings; None when nothing is left. Err = the C#
    /// exception of a file that is not JSON of the right shape.
    pub fn import_file(json: &str, dropped: Option<&mut Vec<String>>) -> Result<Option<String>, String> {
        let s = deserialize(json)?;
        let c = clean(s.as_ref(), dropped);
        Ok(if c.scheme == Guid::EMPTY || c.values.is_empty() { None } else { Some(serialize(&c)) })
    }

    pub fn applied() -> bool {
        Self::file().map(|f| f.is_file()).unwrap_or(false)
    }

    /// Puts the saved values back (left over from a test build or a crash); "" when there is nothing to do.
    pub fn restore() -> String {
        let _g = lock(&GATE);
        if !Self::applied() {
            return String::new();
        }
        let Ok(file) = Self::file() else { return String::new() };
        let mut dropped = Vec::new();
        let saved = match std::fs::read_to_string(&file).map_err(|e| e.to_string()).and_then(|t| deserialize(&t)) {
            Ok(s) => clean(s.as_ref(), Some(&mut dropped)),
            Err(_) => {
                let _ = std::fs::remove_file(&file);
                return String::new();
            }
        };
        if !dropped.is_empty() {
            let first: Vec<String> = dropped.iter().take(5).cloned().collect();
            AppLog::add_sev(lf!("Power for the game: saved settings the program does not manage were ignored: {0}", first.join(", ")), Sev::Warn);
        }
        let values: Vec<(Guid, u32)> = saved.values.iter().filter_map(|(k, v)| Guid::try_parse(k).map(|g| (g, *v))).collect();
        power::restore(saved.scheme, Guid::parse_d(SUB_PROCESSOR).unwrap_or_default(), &values);
        let _ = std::fs::remove_file(&file);
        L::t("Power for the game: the previous values are back").to_string()
    }
}

#[cfg(windows)]
mod power {
    //! powrprof: the three calls of the C# DllImports.
    use super::Guid;
    use windows_sys::core::GUID;
    use windows_sys::Win32::Foundation::{LocalFree, HLOCAL};
    use windows_sys::Win32::System::Power::{PowerGetActiveScheme, PowerSetActiveScheme, PowerWriteACValueIndex};

    fn g(x: Guid) -> GUID {
        let b = x.0;
        GUID {
            data1: u32::from_be_bytes([b[0], b[1], b[2], b[3]]),
            data2: u16::from_be_bytes([b[4], b[5]]),
            data3: u16::from_be_bytes([b[6], b[7]]),
            data4: [b[8], b[9], b[10], b[11], b[12], b[13], b[14], b[15]],
        }
    }

    pub fn restore(scheme: Guid, sub: Guid, values: &[(Guid, u32)]) {
        let sc = g(scheme);
        let sb = g(sub);
        for (k, v) in values {
            let st = g(*k);
            // SAFETY: pointers to live GUIDs on the stack; a null root key is the documented default.
            unsafe { PowerWriteACValueIndex(std::ptr::null_mut(), &sc, &sb, &st, *v) };
        }
        let mut ps: *mut GUID = std::ptr::null_mut();
        // SAFETY: Windows allocates the GUID; it is read once and freed with LocalFree.
        unsafe {
            if PowerGetActiveScheme(std::ptr::null_mut(), &mut ps) == 0 && !ps.is_null() {
                let active = *ps;
                LocalFree(ps as HLOCAL);
                if active.data1 == sc.data1 && active.data2 == sc.data2 && active.data3 == sc.data3 && active.data4 == sc.data4 {
                    PowerSetActiveScheme(std::ptr::null_mut(), &sc);
                }
            }
        }
    }
}

#[cfg(not(windows))]
mod power {
    use super::Guid;
    pub fn restore(_scheme: Guid, _sub: Guid, _values: &[(Guid, u32)]) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    /// SecurityTests.ReservationAndPowerRestoreOnlyTheirOwnValues (the power part; the Reservation part is topo's)
    #[test]
    fn power_restores_only_its_own_values() {
        assert!(GamePower::valid_value("5d76a2ca-e8c0-402f-a133-2158492d58ad", 1));
        assert!(!GamePower::valid_value("5d76a2ca-e8c0-402f-a133-2158492d58ad", 2));
        assert!(GamePower::valid_value("0cc5b647-c1df-4637-891a-dec35c318583", 100));
        assert!(!GamePower::valid_value("3b04d4fd-1cc7-4f23-ab1c-d1337819c4bb", 0));
        let mut dropped = Vec::new();
        assert_eq!(
            None,
            GamePower::import_file("{\"Scheme\":\"381b4222-f694-41f0-9685-ff5bb260df2e\",\"Values\":{\"3b04d4fd-1cc7-4f23-ab1c-d1337819c4bb\":0}}", Some(&mut dropped)).unwrap()
        );
        assert_eq!(1, dropped.len());
    }

    #[test]
    fn import_keeps_managed_values_in_the_csharp_shape() {
        let j = GamePower::import_file(
            "{\"Scheme\":\"381B4222-F694-41F0-9685-FF5BB260DF2E\",\"Values\":{\"{5D76A2CA-E8C0-402F-A133-2158492D58AD}\":1,\"0cc5b647c1df4637891adec35c318583\":50}}",
            None,
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            "{\"Scheme\":\"381b4222-f694-41f0-9685-ff5bb260df2e\",\"Values\":{\"{5D76A2CA-E8C0-402F-A133-2158492D58AD}\":1,\"0cc5b647c1df4637891adec35c318583\":50}}",
            j
        );
        assert!(GamePower::import_file("{\"Scheme\":\"x\"}", None).is_err());
        assert!(GamePower::import_file("{\"Values\":{\"a\":-1}}", None).is_err());
    }
}
