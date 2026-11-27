//! The bookkeeping between the core and the app: the state version (WPF: "the UI thread redraws"), the tray notices,
//! the error entries for ErrorPopup, the background jobs (Shell.Busy / BusyTask) and the request to quit.

use super::win;
use crate::base::hooks::lock;
use serde_json::Value;
use std::collections::BTreeMap as HashMap;
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU32, AtomicU64, Ordering};
use std::sync::Mutex;

static VERSION: AtomicU64 = AtomicU64::new(1);
static EVENT: AtomicIsize = AtomicIsize::new(0);
static UI_THREAD: AtomicU32 = AtomicU32::new(0);
static QUIT: AtomicBool = AtomicBool::new(false);

/// The state changed: the app redraws (C#: Opt.Evaluate / Dispatcher.BeginInvoke(Refresh)).
pub fn bump() {
    VERSION.fetch_add(1, Ordering::SeqCst);
    let e = EVENT.load(Ordering::SeqCst);
    if e != 0 {
        win::set_event(e);
    }
}

pub fn version() -> u64 {
    VERSION.load(Ordering::SeqCst)
}

/// The auto-reset event set with every bump (created once).
pub fn event() -> isize {
    let e = EVENT.load(Ordering::SeqCst);
    if e != 0 {
        return e;
    }
    let n = win::create_event(false, false, None);
    match EVENT.compare_exchange(0, n, Ordering::SeqCst, Ordering::SeqCst) {
        Ok(_) => n,
        Err(old) => {
            win::close(n);
            old
        }
    }
}

/// The thread of dos_app_start (the app's UI thread).
pub fn set_ui_thread(id: u32) {
    UI_THREAD.store(id, Ordering::SeqCst);
}

pub fn ui_thread() -> u32 {
    UI_THREAD.load(Ordering::SeqCst)
}

/// `Application.Shutdown()`: the app's loop gets WM_QUIT.
pub fn request_quit() {
    QUIT.store(true, Ordering::SeqCst);
    win::post_quit(ui_thread());
    bump();
}

pub fn quit_requested() -> bool {
    QUIT.load(Ordering::SeqCst)
}

// ---- tray notices ----------------------------------------------------------------------------------------------

static TRAY: Mutex<Vec<(String, String)>> = Mutex::new(Vec::new());

/// `_tray?.Info(text)` / `_tray?.Notice(text)`: queued for the app's tray ("info" or "error").
pub fn tray(kind: &str, text: &str) {
    lock(&TRAY).push((kind.to_string(), text.to_string()));
    bump();
}

pub fn take_tray() -> Vec<(String, String)> {
    std::mem::take(&mut *lock(&TRAY))
}

// ---- error entries (AppLog.ErrorAdded -> ErrorPopup) -----------------------------------------------------------

static ERRORS: Mutex<Vec<(String, String)>> = Mutex::new(Vec::new());

pub fn add_error(t: String, msg: String) {
    lock(&ERRORS).push((t, msg));
    bump();
}

pub fn take_errors() -> Vec<(String, String)> {
    std::mem::take(&mut *lock(&ERRORS))
}

// ---- jobs ----------------------------------------------------------------------------------------------------

struct Jobs {
    next: u64,
    /// None = running; Some(result) = done (the result may be JSON null).
    all: HashMap<u64, Option<Value>>,
}

static JOBS: Mutex<Jobs> = Mutex::new(Jobs { next: 1, all: HashMap::new() });

/// A new running job.
pub fn job_new() -> u64 {
    let mut j = lock(&JOBS);
    let id = j.next;
    j.next += 1;
    j.all.insert(id, None);
    id
}

/// The job is done (C#: the `done` callback runs on the UI thread, then Shell.Refresh).
pub fn job_done(id: u64, result: Value) {
    lock(&JOBS).all.insert(id, Some(result));
    bump();
}

/// None = still running; Some(None) = unknown or done without result.
pub fn job_take(id: u64) -> Option<Option<Value>> {
    let mut j = lock(&JOBS);
    match j.all.get(&id) {
        Some(None) => None,
        Some(Some(_)) => j.all.remove(&id).flatten().map(Some),
        None => Some(None),
    }
}
