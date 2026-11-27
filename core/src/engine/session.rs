//! The crash-safe session file (section 12.10, port of `Session.cs`): baselines written before the first change.
//!
//! JSON shape of `System.Text.Json` with the default options (PascalCase property names in declaration order,
//! DateTime as ISO 8601 with the fraction's trailing zeros trimmed), so the C# and the native exe read each other's
//! file. Reading is as strict as the C# deserializer: a value of the wrong type makes the whole file unreadable (None).

use crate::base::hooks::Hook;
use crate::base::time::DateTime;
use serde::de::{self, Deserializer};
use serde::ser::Serializer;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

fn null_vec<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u32>, D::Error> {
    // C# `uint[]` may be null; every use coalesces it to an empty array.
    Ok(Option::<Vec<u32>>::deserialize(d)?.unwrap_or_default())
}

fn null_string<'de, D: Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    Ok(Option::<String>::deserialize(d)?.unwrap_or_default())
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct SessionProc {
    pub pid: u32,
    #[serde(deserialize_with = "null_string")]
    pub name: String,
    pub created: i64,
    pub background: bool,
    pub prio: u32,
    pub prio_set: bool,
    pub aff: u64,
    pub aff_set: bool,
    #[serde(deserialize_with = "null_vec")]
    pub sets: Vec<u32>,
    pub sets_set: bool,
    pub io: u32,
    pub io_set: bool,
    pub mem: u32,
    pub mem_set: bool,
    pub eco_ctl: u32,
    pub eco_state: u32,
    pub eco_set: bool,
    pub boost_disabled: bool,
    pub boost_set: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct SessionThread {
    pub pid: u32,
    pub tid: u32,
    pub born: i64,
    pub prio: i32,
    pub prio_set: bool,
    pub boost_disabled: bool,
    pub boost_set: bool,
    #[serde(deserialize_with = "null_vec")]
    pub sets: Vec<u32>,
    pub sets_set: bool,
    /// The ideal processor before the first change (crash restore puts it back).
    pub ideal: u32,
    pub ideal_set: bool,
}

fn ser_time<S: Serializer>(t: &DateTime, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_str(&stj_time(*t))
}

fn de_time<'de, D: Deserializer<'de>>(d: D) -> Result<DateTime, D::Error> {
    let s = String::deserialize(d)?;
    DateTime::parse_iso(&s, false).ok_or_else(|| de::Error::custom("The JSON value is not in a supported DateTime format."))
}

/// `System.Text.Json`'s DateTime text: the round-trip format with the fraction's trailing zeros trimmed.
pub fn stj_time(t: DateTime) -> String {
    let o = t.to_o(); // yyyy-MM-ddTHH:mm:ss.fffffff[K]
    let (head, tail) = o.split_at(19);
    let frac_end = tail.char_indices().skip(1).find(|(_, c)| !c.is_ascii_digit()).map(|(i, _)| i).unwrap_or(tail.len());
    let frac = tail[1..frac_end].trim_end_matches('0');
    let rest = &tail[frac_end..];
    if frac.is_empty() { format!("{}{}", head, rest) } else { format!("{}.{}{}", head, frac, rest) }
}

/// The session (C# `SessionData`). `procs` / `threads` null in the file read as empty lists; a null entry inside
/// them is counted (`null_procs` / `null_threads`) and skipped by the crash restore with the same warning as in C#.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct SessionData {
    #[serde(serialize_with = "ser_time")]
    pub started_utc: DateTime,
    pub clean_shutdown: bool,
    /// The fine timer resolution was requested (section 22.5): released first thing after a crash.
    pub timer_requested: bool,
    pub procs: Vec<SessionProc>,
    pub threads: Vec<SessionThread>,
    #[serde(skip)]
    pub null_procs: usize,
    #[serde(skip)]
    pub null_threads: usize,
}

impl Default for SessionData {
    fn default() -> Self {
        SessionData {
            started_utc: DateTime::utc_now(),
            clean_shutdown: false,
            timer_requested: false,
            procs: Vec::new(),
            threads: Vec::new(),
            null_procs: 0,
            null_threads: 0,
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct RawData {
    #[serde(default, deserialize_with = "de_time_opt")]
    started_utc: Option<DateTime>,
    #[serde(default)]
    clean_shutdown: bool,
    #[serde(default)]
    timer_requested: bool,
    #[serde(default)]
    procs: Option<Vec<Option<SessionProc>>>,
    #[serde(default)]
    threads: Option<Vec<Option<SessionThread>>>,
}

fn de_time_opt<'de, D: Deserializer<'de>>(d: D) -> Result<Option<DateTime>, D::Error> {
    de_time(d).map(Some)
}

impl<'de> Deserialize<'de> for SessionData {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let r = RawData::deserialize(d)?;
        let procs = r.procs.unwrap_or_default();
        let threads = r.threads.unwrap_or_default();
        Ok(SessionData {
            started_utc: r.started_utc.unwrap_or_else(DateTime::utc_now),
            clean_shutdown: r.clean_shutdown,
            timer_requested: r.timer_requested,
            null_procs: procs.iter().filter(|p| p.is_none()).count(),
            null_threads: threads.iter().filter(|t| t.is_none()).count(),
            procs: procs.into_iter().flatten().collect(),
            threads: threads.into_iter().flatten().collect(),
        })
    }
}

/// `SessionFile.PathHook`.
pub static PATH_HOOK: Hook<dyn Fn() -> PathBuf + Send + Sync> = Hook::new();

/// `SessionFile` (static class).
pub struct SessionFile;

impl SessionFile {
    pub const FILE_NAME: &'static str = "session.json";

    /// `SessionFile.FilePath`: session.json in the data folder (Err when the folder cannot be made safe).
    pub fn file_path() -> Result<PathBuf, String> {
        if let Some(h) = PATH_HOOK.get() {
            return Ok(h());
        }
        Ok(crate::base::state_dir::store_dir()?.join(Self::FILE_NAME))
    }

    pub fn to_json(s: &SessionData) -> String {
        serde_json::to_string(s).unwrap_or_default()
    }

    pub fn from_json(text: &str) -> Option<SessionData> {
        serde_json::from_str(text.trim_start_matches('\u{feff}')).ok()
    }

    /// Writes the file atomically (tmp + move). Err = the C# exception (the caller decides).
    pub fn write(s: &SessionData) -> Result<(), String> {
        let p = Self::file_path()?;
        if let Some(dir) = p.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        let mut tmp = p.clone().into_os_string();
        tmp.push(".tmp");
        let tmp = PathBuf::from(tmp);
        std::fs::write(&tmp, Self::to_json(s)).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, &p).map_err(|e| e.to_string())
    }

    pub fn read() -> Option<SessionData> {
        let p = Self::file_path().ok()?;
        if !p.is_file() {
            return None;
        }
        let text = std::fs::read_to_string(&p).ok()?;
        Self::from_json(&text)
    }

    pub fn delete() {
        if let Ok(p) = Self::file_path() {
            let _ = std::fs::remove_file(p);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::base::hooks::static_hooks;
    use crate::base::time::Kind;
    use std::sync::Arc;

    #[test]
    fn json_shape_is_the_csharp_one_and_reads_back() {
        let s = SessionData {
            started_utc: DateTime::from_parts(2026, 11, 27, 10, 0, 0, Kind::Utc).unwrap(),
            timer_requested: true,
            procs: vec![SessionProc { pid: 500, name: "cs2.exe".into(), created: 77, prio: 32, prio_set: true, sets: vec![256, 258], ..Default::default() }],
            threads: vec![SessionThread { pid: 500, tid: 100, born: 1000, prio: 0, prio_set: true, ..Default::default() }],
            ..Default::default()
        };
        let j = SessionFile::to_json(&s);
        assert!(j.starts_with("{\"StartedUtc\":\"2026-11-27T10:00:00Z\",\"CleanShutdown\":false,\"TimerRequested\":true,\"Procs\":[{\"Pid\":500,\"Name\":\"cs2.exe\",\"Created\":77,"), "{}", j);
        assert!(j.contains("\"Sets\":[256,258],\"SetsSet\":false"));
        let back = SessionFile::from_json(&j).unwrap();
        assert_eq!(s, back);
        assert_eq!("2026-11-27T10:00:00.5Z", stj_time(DateTime::from_parts(2026, 11, 27, 10, 0, 0, Kind::Utc).unwrap().add_ms(500.0)));
        // the C# reader's strictness: wrong types make the file unreadable; nulls are taken
        assert!(SessionFile::from_json("{\"Procs\":[{\"Pid\":-1}]}").is_none());
        assert!(SessionFile::from_json("{\"Procs\":[{\"Pid\":\"5\"}]}").is_none());
        let n = SessionFile::from_json("{\"Procs\":null,\"Threads\":[null,{\"Tid\":3,\"Sets\":null}]}").unwrap();
        assert!(n.procs.is_empty());
        assert_eq!((1, 1), (n.null_threads, n.threads.len()));
        assert!(SessionFile::from_json("{ broken").is_none());
    }

    #[test]
    fn file_round_trip() {
        let _g = static_hooks();
        let dir = std::env::temp_dir().join(format!("ds-sess-{}", std::process::id()));
        let f = dir.join("session.json");
        let f2 = f.clone();
        PATH_HOOK.set(Some(Arc::new(move || f2.clone())));
        let s = SessionData { clean_shutdown: true, ..Default::default() };
        SessionFile::write(&s).unwrap();
        assert!(SessionFile::read().unwrap().clean_shutdown);
        SessionFile::delete();
        assert!(SessionFile::read().is_none());
        PATH_HOOK.set(None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
