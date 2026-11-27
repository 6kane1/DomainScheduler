//! The `extern "C"` functions of app/src/dos_core.h. Every one catches a panic (where unwinding is on) and returns a
//! neutral value; strings go out as `CString::into_raw` and come back to `dos_free_str`.

use super::{app, pages, state};
use crate::base::app_log::{AppLog, Sev};
use crate::base::l::L;
use crate::base::settings::Settings;
use serde_json::{json, Value};
use std::ffi::{c_char, c_int, c_void, CStr, CString};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::Arc;

/// A string for the app (owned by the core until dos_free_str).
pub fn out(s: impl Into<String>) -> *mut c_char {
    let s: String = s.into();
    let s = if s.contains('\0') { s.replace('\0', " ") } else { s };
    CString::new(s).map(CString::into_raw).unwrap_or(std::ptr::null_mut())
}

pub fn out_json(v: &Value) -> *mut c_char {
    out(serde_json::to_string(v).unwrap_or_default())
}

/// A string from the app (NULL = "").
///
/// # Safety
/// `p` is NULL or a NUL-terminated string that stays valid during the call.
unsafe fn arg(p: *const c_char) -> String {
    if p.is_null() {
        return String::new();
    }
    CStr::from_ptr(p).to_string_lossy().into_owned()
}

fn args_of(p: *const c_char) -> Value {
    // SAFETY: the ABI contract (a NUL-terminated string or NULL).
    let s = unsafe { arg(p) };
    if s.trim().is_empty() {
        return json!({});
    }
    serde_json::from_str(&s).unwrap_or_else(|_| json!({}))
}

pub(super) fn guard<R>(fallback: R, f: impl FnOnce() -> R) -> R {
    match catch_unwind(AssertUnwindSafe(f)) {
        Ok(r) => r,
        Err(p) => {
            AppLog::add_sev(crate::lf!("Unhandled error: {0}", super::optimizer::panic_text(&*p)), Sev::Error);
            app::emergency_revert();
            fallback
        }
    }
}

/// # Safety
/// `s` is NULL or a string this library returned, freed once.
#[no_mangle]
pub unsafe extern "C" fn dos_free_str(s: *mut c_char) {
    if !s.is_null() {
        drop(CString::from_raw(s));
    }
}

// ---- start / exit ----------------------------------------------------------------------------------------------

/// # Safety
/// `args_json` is NULL or a NUL-terminated string; `show` is NULL or writable.
#[no_mangle]
pub unsafe extern "C" fn dos_app_start(args_json: *const c_char, show: *mut c_int) -> c_int {
    let args: Vec<String> = match args_of(args_json) {
        Value::Array(a) => a.into_iter().filter_map(|v| v.as_str().map(str::to_string)).collect(),
        _ => Vec::new(),
    };
    state::set_ui_thread(crate::native::sys::current_thread_id());
    let mut sh = true;
    let rc = guard(1, || app::start(args, &mut sh));
    if !show.is_null() {
        *show = sh as c_int;
    }
    rc
}

#[no_mangle]
pub extern "C" fn dos_app_exit() -> c_int {
    guard(1, || app::exit_app() as c_int)
}

#[no_mangle]
pub extern "C" fn dos_show_event() -> *mut c_void {
    app::app().map(|a| a.show_event as *mut c_void).unwrap_or(std::ptr::null_mut())
}

#[no_mangle]
pub extern "C" fn dos_quit_requested() -> c_int {
    state::quit_requested() as c_int
}

// ---- change notification -----------------------------------------------------------------------------------------

#[no_mangle]
pub extern "C" fn dos_state_version() -> u64 {
    state::version()
}

#[no_mangle]
pub extern "C" fn dos_state_event() -> *mut c_void {
    state::event() as *mut c_void
}

// ---- language and formatting -------------------------------------------------------------------------------------

/// # Safety
/// `en` is NULL or a NUL-terminated string.
#[no_mangle]
pub unsafe extern "C" fn dos_tr(en: *const c_char) -> *mut c_char {
    let en = arg(en);
    guard(std::ptr::null_mut(), || out(L::t(&en).to_string()))
}

#[no_mangle]
pub extern "C" fn dos_lang() -> *mut c_char {
    out(L::lang())
}

/// # Safety
/// Both are NULL or NUL-terminated strings.
#[no_mangle]
pub unsafe extern "C" fn dos_fmt(en: *const c_char, args_json: *const c_char) -> *mut c_char {
    let en = arg(en);
    let a = args_of(args_json);
    guard(std::ptr::null_mut(), || out(pages::fmt_args(&en, &a)))
}

/// # Safety
/// `format` is NULL or a NUL-terminated string.
#[no_mangle]
pub unsafe extern "C" fn dos_num(v: f64, format: *const c_char) -> *mut c_char {
    let f = arg(format);
    guard(std::ptr::null_mut(), || out(crate::base::fmt::num(v, &f, &crate::base::fmt::Culture::current())))
}

// ---- settings and log ----------------------------------------------------------------------------------------------

/// # Safety
/// `key` is NULL or a NUL-terminated string.
#[no_mangle]
pub unsafe extern "C" fn dos_setting_get(key: *const c_char) -> *mut c_char {
    let k = arg(key);
    guard(std::ptr::null_mut(), || out(Settings::get(&k)))
}

/// # Safety
/// Both are NULL or NUL-terminated strings.
#[no_mangle]
pub unsafe extern "C" fn dos_setting_set(key: *const c_char, value: *const c_char) {
    let (k, v) = (arg(key), arg(value));
    guard((), || {
        let _ = Settings::set(&k, &v);
    })
}

/// # Safety
/// `msg` is NULL or a NUL-terminated string.
#[no_mangle]
pub unsafe extern "C" fn dos_log_add(msg: *const c_char, sev: c_int) {
    let m = arg(msg);
    let s = match sev {
        2 => Sev::Error,
        1 => Sev::Warn,
        _ => Sev::Info,
    };
    guard((), || {
        AppLog::add_sev(m, s);
        state::bump();
    })
}

#[no_mangle]
pub extern "C" fn dos_log_take_errors() -> *mut c_char {
    let e = state::take_errors();
    if e.is_empty() {
        return std::ptr::null_mut();
    }
    out_json(&Value::Array(e.into_iter().map(|(t, m)| json!({"t": t, "msg": m})).collect()))
}

#[no_mangle]
pub extern "C" fn dos_log_shown_path() -> *mut c_char {
    out(AppLog::shown_path().to_string_lossy().into_owned())
}

// ---- shell -------------------------------------------------------------------------------------------------------

#[no_mangle]
pub extern "C" fn dos_shell_state() -> *mut c_char {
    guard(std::ptr::null_mut(), || out_json(&pages::shell_state()))
}

#[no_mangle]
pub extern "C" fn dos_facts() -> *mut c_char {
    guard(std::ptr::null_mut(), || out_json(&pages::facts()))
}

#[no_mangle]
pub extern "C" fn dos_auto_click() {
    guard((), pages::auto_click)
}

#[no_mangle]
pub extern "C" fn dos_undo_everything() {
    guard((), pages::undo_everything)
}

#[no_mangle]
pub extern "C" fn dos_ui_visible(on: c_int) {
    if let Some(h) = app::hub() {
        h.set_ui_visible(on != 0);
    }
}

#[no_mangle]
pub extern "C" fn dos_note_ui(ms: f64) {
    if let Some(h) = app::hub() {
        h.note_ui(ms);
    }
}

#[no_mangle]
pub extern "C" fn dos_note_ui_skipped() {
    if let Some(h) = app::hub() {
        h.note_ui_skipped();
    }
}

#[no_mangle]
pub extern "C" fn dos_tray_take() -> *mut c_char {
    let t = state::take_tray();
    if t.is_empty() {
        return std::ptr::null_mut();
    }
    out_json(&Value::Array(t.into_iter().map(|(k, x)| json!({"kind": k, "text": x})).collect()))
}

// ---- pages ---------------------------------------------------------------------------------------------------------

/// # Safety
/// Both are NULL or NUL-terminated strings.
#[no_mangle]
pub unsafe extern "C" fn dos_get(name: *const c_char, args_json: *const c_char) -> *mut c_char {
    let n = arg(name);
    let a = args_of(args_json);
    guard(std::ptr::null_mut(), || out_json(&pages::get(&n, &a)))
}

/// # Safety
/// Both are NULL or NUL-terminated strings.
#[no_mangle]
pub unsafe extern "C" fn dos_do(name: *const c_char, args_json: *const c_char) -> *mut c_char {
    let n = arg(name);
    let a = args_of(args_json);
    guard(std::ptr::null_mut(), || {
        let r = pages::do_(&n, &a);
        state::bump();
        out_json(&r)
    })
}

/// # Safety
/// Both are NULL or NUL-terminated strings.
#[no_mangle]
pub unsafe extern "C" fn dos_start(name: *const c_char, args_json: *const c_char) -> u64 {
    let n = arg(name);
    let a = Arc::new(args_of(args_json));
    guard(0, || pages::start(&n, a))
}

/// # Safety
/// `result` is NULL or writable.
#[no_mangle]
pub unsafe extern "C" fn dos_job_poll(id: u64, result: *mut *mut c_char) -> c_int {
    if !result.is_null() {
        *result = std::ptr::null_mut();
    }
    match state::job_take(id) {
        None => 0,
        Some(v) => {
            if let (Some(v), false) = (v, result.is_null()) {
                *result = out_json(&v);
            }
            1
        }
    }
}

