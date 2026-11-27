//! The program log (section 25, port of `AppLog.cs`): a ring buffer of about 2000 entries in memory (the Report reads
//! from here) and the file log.txt in the state folder. A warning or an error is written to the file at once (it is
//! there after a blue screen too); other entries go with it, or at the latest a minute later, and at exit. Each entry
//! is written once: a flush appends only what came since the last one.

use super::hooks::{lock, Hook};
use super::time::{DateTime, TimeSpan};
use std::fmt;
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub enum Sev {
    #[default]
    Info,
    Warn,
    Error,
}

impl fmt::Display for Sev {
    /// The enum name, as C# writes it into the file ("Info", "Warn", "Error").
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Sev::Info => "Info",
            Sev::Warn => "Warn",
            Sev::Error => "Error",
        })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub utc: DateTime,
    pub sev: Sev,
    pub msg: String,
}

impl fmt::Display for Entry {
    /// "HH:mm:ss  WARN   message" in local time.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}  {}  {}",
            self.utc.to_local().format("HH:mm:ss", &super::fmt::Culture::invariant()),
            match self.sev {
                Sev::Warn => "WARN ",
                Sev::Error => "ERROR",
                Sev::Info => "INFO ",
            },
            self.msg
        )
    }
}

struct Ring {
    ring: Vec<Entry>,
    next: usize,
    count: usize,
    /// Entries added since the last flush (not yet in the file).
    unwritten: usize,
    last_flush: DateTime,
}

static GATE: Mutex<Ring> = Mutex::new(Ring { ring: Vec::new(), next: 0, count: 0, unwritten: 0, last_flush: DateTime::MIN });
static FILE_GATE: Mutex<bool> = Mutex::new(false); // header done
static VERSION: AtomicI64 = AtomicI64::new(0);

type NowFn = dyn Fn() -> DateTime + Send + Sync;
type PathFn = dyn Fn() -> PathBuf + Send + Sync;
type ErrorFn = dyn Fn(&Entry) + Send + Sync;

/// `AppLog.Now` (tests replace the clock; default `DateTime.UtcNow`).
pub static NOW_HOOK: Hook<NowFn> = Hook::new();
/// `AppLog.PathHook`: where the file goes (tests).
pub static PATH_HOOK: Hook<PathFn> = Hook::new();
static ERROR_ADDED: Mutex<Vec<Arc<ErrorFn>>> = Mutex::new(Vec::new());

/// `AppLog` (static class).
pub struct AppLog;

impl AppLog {
    pub const CAPACITY: usize = 2000;
    pub const FLUSH_EVERY: TimeSpan = TimeSpan::from_secs_i(60);

    fn now() -> DateTime {
        match NOW_HOOK.get() {
            Some(f) => f(),
            None => DateTime::utc_now(),
        }
    }

    /// The file (log.txt in the state folder); Err when the state folder cannot be used.
    pub fn file_path() -> Result<PathBuf, String> {
        if let Some(p) = PATH_HOOK.get() {
            return Ok(p());
        }
        Ok(super::state_dir::store_dir()?.join("log.txt"))
    }

    /// Where the file is, for display (never fails).
    pub fn shown_path() -> PathBuf {
        if let Some(p) = PATH_HOOK.get() {
            return p();
        }
        super::state_dir::store_dir_shown().join("log.txt")
    }

    /// The file is written on Windows, and in tests only when the path hook points somewhere (a test build on Windows
    /// or Wine never touches the real state folder).
    fn file_on() -> bool {
        PATH_HOOK.is_set() || (cfg!(windows) && !cfg!(test))
    }

    /// Grows with every entry (the log panel redraws only when it changed).
    pub fn version() -> i64 {
        VERSION.load(Ordering::SeqCst)
    }

    /// `AppLog.ErrorAdded`: raised for every error entry (outside the lock): the app shows it in a window.
    pub fn on_error_added(f: Arc<ErrorFn>) {
        lock(&ERROR_ADDED).push(f);
    }

    /// `Add(msg)` (Info).
    pub fn add(msg: impl AsRef<str>) {
        Self::add_sev(msg, Sev::Info);
    }
    pub fn warn(msg: impl AsRef<str>) {
        Self::add_sev(msg, Sev::Warn);
    }
    pub fn error(msg: impl AsRef<str>) {
        Self::add_sev(msg, Sev::Error);
    }

    /// `Add(msg, sev)`.
    pub fn add_sev(msg: impl AsRef<str>, sev: Sev) {
        let msg = msg.as_ref();
        if msg.is_empty() {
            return;
        }
        VERSION.fetch_add(1, Ordering::SeqCst);
        let e = Entry { utc: Self::now(), sev, msg: msg.to_string() };
        let flush;
        {
            let mut g = lock(&GATE);
            if g.ring.len() < Self::CAPACITY {
                g.ring.resize(Self::CAPACITY, Entry { utc: DateTime::MIN, sev: Sev::Info, msg: String::new() });
            }
            let n = g.next;
            g.ring[n] = e.clone();
            g.next = (n + 1) % Self::CAPACITY;
            if g.count < Self::CAPACITY {
                g.count += 1;
            }
            if g.unwritten < Self::CAPACITY {
                g.unwritten += 1;
            }
            flush = sev != Sev::Info || e.utc - g.last_flush >= Self::FLUSH_EVERY;
        }
        if flush {
            Self::flush();
        }
        if sev == Sev::Error {
            let subs: Vec<Arc<ErrorFn>> = lock(&ERROR_ADDED).clone();
            for s in subs {
                // C# catches anything the handler throws; a panicking handler is a bug of the handler.
                let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| s(&e)));
            }
        }
    }

    /// The last n entries, oldest first.
    pub fn tail(n: usize) -> Vec<Entry> {
        let g = lock(&GATE);
        let n = n.min(g.count);
        let mut list = Vec::with_capacity(n);
        for i in (1..=n).rev() {
            list.push(g.ring[(g.next + Self::CAPACITY - i) % Self::CAPACITY].clone());
        }
        list
    }

    pub fn clear() {
        let mut g = lock(&GATE);
        g.next = 0;
        g.count = 0;
        g.unwritten = 0;
    }

    /// Appends the entries that are not in the file yet (each one once). Never fails.
    pub fn flush() {
        if !Self::file_on() {
            let mut g = lock(&GATE);
            g.unwritten = 0;
            g.last_flush = Self::now();
            return;
        }
        let mut header_done = lock(&FILE_GATE);
        let fresh: Vec<Entry> = {
            let mut g = lock(&GATE);
            let n = g.unwritten.min(g.count);
            let mut fresh = Vec::with_capacity(n);
            for i in (1..=n).rev() {
                fresh.push(g.ring[(g.next + Self::CAPACITY - i) % Self::CAPACITY].clone());
            }
            g.unwritten = 0;
            g.last_flush = Self::now();
            fresh
        };
        if fresh.is_empty() && *header_done {
            return;
        }
        let r = (|| -> Result<(), String> {
            let path = Self::file_path()?;
            if let Some(d) = path.parent() {
                std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
            }
            let nl = if cfg!(windows) { "\r\n" } else { "\n" };
            let mut sb = String::new();
            if !*header_done {
                sb.push_str("---- ");
                sb.push_str(&DateTime::utc_now().to_o());
                sb.push_str(nl);
            }
            for e in &fresh {
                sb.push_str(&e.utc.to_o());
                sb.push(' ');
                sb.push_str(&e.sev.to_string());
                sb.push(' ');
                sb.push_str(&e.msg);
                sb.push_str(nl);
            }
            let mut f = std::fs::OpenOptions::new().create(true).append(true).open(&path).map_err(|e| e.to_string())?;
            f.write_all(sb.as_bytes()).map_err(|e| e.to_string())?;
            drop(f);
            *header_done = true;
            let len = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
            if len > 4 << 20 {
                // keep the file small: the last 2 MB
                let all = std::fs::read(&path).map_err(|e| e.to_string())?;
                std::fs::write(&path, &all[all.len() - (2 << 20)..]).map_err(|e| e.to_string())?;
            }
            Ok(())
        })();
        let _ = r;
    }

    /// `AppLog.Describe(Exception)`: the error and its sources (up to 3), joined with " ← ", so an error with an empty
    /// message still says what failed. Rust errors have no type names or stack frames; the Display texts are used.
    pub fn describe(e: &(dyn std::error::Error + 'static)) -> String {
        let mut chain = Vec::new();
        let mut cur: Option<&(dyn std::error::Error + 'static)> = Some(e);
        let mut d = 0;
        while let Some(x) = cur {
            if d >= 3 {
                break;
            }
            let m = x.to_string();
            chain.push(if m.trim().is_empty() { "Error".to_string() } else { m });
            cur = x.source();
            d += 1;
        }
        chain.join(" ← ")
    }

    /// The text of a caught panic payload (the C# catch of an unexpected exception).
    pub fn describe_panic(p: &(dyn std::any::Any + Send)) -> String {
        if let Some(s) = p.downcast_ref::<&str>() {
            (*s).to_string()
        } else if let Some(s) = p.downcast_ref::<String>() {
            s.clone()
        } else {
            "panic".to_string()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::base::hooks::static_hooks;

    /// SecurityTests.WarningsReachTheFileAtOnceAndNothingIsWrittenTwice
    #[test]
    fn warnings_reach_the_file_at_once_and_nothing_is_written_twice() {
        let _g = static_hooks();
        let dir = std::env::temp_dir().join(format!("ds-state-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("log.txt");
        let f2 = file.clone();
        PATH_HOOK.set(Some(Arc::new(move || f2.clone())));
        AppLog::clear();
        AppLog::flush();
        AppLog::add("first info");
        AppLog::warn("a warning");
        let text = std::fs::read_to_string(&file).unwrap();
        assert!(text.contains("a warning"));
        assert!(text.contains("first info"));
        AppLog::flush();
        AppLog::flush();
        AppLog::error("an error");
        let text = std::fs::read_to_string(&file).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.iter().filter(|l| l.contains("a warning")).count(), 1);
        assert_eq!(lines.iter().filter(|l| l.contains("first info")).count(), 1);
        assert_eq!(lines.iter().filter(|l| l.contains("an error")).count(), 1);
        PATH_HOOK.set(None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// AddendumTests.AppLogLevels (the ring part; the engine part is ported with the engine).
    #[test]
    fn app_log_ring_keeps_the_capacity() {
        let _g = static_hooks();
        AppLog::clear();
        for i in 0..AppLog::CAPACITY + 10 {
            AppLog::add(format!("x{}", i));
        }
        let all = AppLog::tail(usize::MAX);
        assert_eq!(AppLog::CAPACITY, all.len());
        assert_eq!(all.last().unwrap().msg, format!("x{}", AppLog::CAPACITY + 9));
        assert_eq!(all[0].msg, "x10");
    }
}
