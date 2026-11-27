//! C ABI (header `threadmap.h` next to the crate). Rules as in PLAN.md "C ABI": `extern "C"` functions prefixed
//! `tm_`; strings are UTF-8 and NUL-terminated; strings returned here are owned by this library and freed with
//! `tm_free_str`; bigger results are JSON (serde_json); no panic crosses the boundary (every entry point catches it
//! and returns an error code / NULL). The handle is internally locked, so the ETW thread may feed while the UI thread
//! reads; feed in batches to keep the lock short.

use std::ffi::{c_char, CStr, CString};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::ptr;
use std::sync::Mutex;

use crate::events::*;
use crate::graph::Query;
use crate::layout::{Layout, Pos};
use crate::map::{Config, ThreadMap};

pub const TM_OK: i32 = 0;
pub const TM_ERR_NULL: i32 = -1;
pub const TM_ERR_ARG: i32 = -2;
pub const TM_ERR_INTERNAL: i32 = -3;

struct Inner {
    map: ThreadMap,
    layout: Layout,
    query: Query,
}

/// Opaque handle (`tm_map*` in C).
pub struct TmMap {
    inner: Mutex<Inner>,
}

fn with<R>(h: *mut TmMap, fail: R, f: impl FnOnce(&mut Inner) -> R) -> R {
    if h.is_null() {
        return fail;
    }
    // SAFETY: a non-null handle comes from tm_new and lives until tm_free (C side contract).
    let m = unsafe { &*h };
    match catch_unwind(AssertUnwindSafe(|| {
        let mut g = m.inner.lock().unwrap_or_else(|e| e.into_inner());
        f(&mut g)
    })) {
        Ok(r) => r,
        Err(_) => fail,
    }
}

fn opt_str(p: *const c_char) -> Option<String> {
    if p.is_null() {
        return None;
    }
    // SAFETY: the caller passes a NUL-terminated string (C ABI rule).
    Some(unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned())
}

fn out_str(s: String) -> *mut c_char {
    CString::new(s).map(|c| c.into_raw()).unwrap_or(ptr::null_mut())
}

fn slice<'a, T>(p: *const T, n: usize) -> Option<&'a [T]> {
    if p.is_null() {
        return if n == 0 { Some(&[]) } else { None };
    }
    // SAFETY: the caller passes `n` valid elements (C ABI contract).
    Some(unsafe { std::slice::from_raw_parts(p, n) })
}

/// Library version, static string (do not free).
#[no_mangle]
pub extern "C" fn tm_version() -> *const c_char {
    concat!(env!("CARGO_PKG_VERSION"), "\0").as_ptr() as *const c_char
}

/// New map for timestamps in QPC ticks of `qpc_freq` per second. NULL when qpc_freq is 0.
#[no_mangle]
pub extern "C" fn tm_new(qpc_freq: u64) -> *mut TmMap {
    if qpc_freq == 0 {
        return ptr::null_mut();
    }
    catch_unwind(|| {
        Box::into_raw(Box::new(TmMap {
            inner: Mutex::new(Inner { map: ThreadMap::new(Config::new(qpc_freq)), layout: Layout::new(), query: Query::default() }),
        }))
    })
    .unwrap_or(ptr::null_mut())
}

#[no_mangle]
pub extern "C" fn tm_free(h: *mut TmMap) {
    if !h.is_null() {
        // SAFETY: from tm_new, freed once.
        let _ = catch_unwind(AssertUnwindSafe(|| drop(unsafe { Box::from_raw(h) })));
    }
}

/// Frees a string returned by this library (NULL is fine).
#[no_mangle]
pub extern "C" fn tm_free_str(s: *mut c_char) {
    if !s.is_null() {
        // SAFETY: from CString::into_raw in out_str.
        let _ = catch_unwind(AssertUnwindSafe(|| drop(unsafe { CString::from_raw(s) })));
    }
}

/// Drops all events and history.
#[no_mangle]
pub extern "C" fn tm_clear(h: *mut TmMap) -> i32 {
    with(h, TM_ERR_NULL, |i| {
        i.map.clear();
        TM_OK
    })
}

/// Game pid (0 = none): the "game" class and the time strip.
#[no_mangle]
pub extern "C" fn tm_set_game(h: *mut TmMap, pid: u32) -> i32 {
    with(h, TM_ERR_NULL, |i| {
        i.map.set_game((pid != 0).then_some(pid));
        TM_OK
    })
}

#[no_mangle]
pub extern "C" fn tm_set_cpu_count(h: *mut TmMap, n: u32) -> i32 {
    with(h, TM_ERR_NULL, |i| {
        i.map.set_cpu_count(n);
        TM_OK
    })
}

#[no_mangle]
pub extern "C" fn tm_set_reorder_ms(h: *mut TmMap, ms: u32) -> i32 {
    with(h, TM_ERR_NULL, |i| {
        i.map.set_reorder_ms(ms.min(10_000));
        TM_OK
    })
}

// ---- feeding ---------------------------------------------------------------------------------------------------

#[no_mangle]
pub extern "C" fn tm_feed_cswitch(h: *mut TmMap, ev: *const CSwitch, n: usize) -> i32 {
    let Some(s) = slice(ev, n) else { return TM_ERR_ARG };
    with(h, TM_ERR_NULL, |i| {
        s.iter().for_each(|e| i.map.push_cswitch(e));
        TM_OK
    })
}

#[no_mangle]
pub extern "C" fn tm_feed_ready(h: *mut TmMap, ev: *const ReadyThread, n: usize) -> i32 {
    let Some(s) = slice(ev, n) else { return TM_ERR_ARG };
    with(h, TM_ERR_NULL, |i| {
        s.iter().for_each(|e| i.map.push_ready(e));
        TM_OK
    })
}

#[no_mangle]
pub extern "C" fn tm_feed_link(h: *mut TmMap, ev: *const LinkEvent, n: usize) -> i32 {
    let Some(s) = slice(ev, n) else { return TM_ERR_ARG };
    with(h, TM_ERR_NULL, |i| {
        s.iter().for_each(|e| i.map.push_link(e));
        TM_OK
    })
}

#[no_mangle]
pub extern "C" fn tm_process_start(h: *mut TmMap, ts: i64, pid: u32, image: *const c_char) -> i32 {
    let image = opt_str(image).unwrap_or_default();
    with(h, TM_ERR_NULL, |i| {
        i.map.process_start(&ProcessInfo { ts, pid, image });
        TM_OK
    })
}

#[no_mangle]
pub extern "C" fn tm_process_end(h: *mut TmMap, ts: i64, pid: u32) -> i32 {
    with(h, TM_ERR_NULL, |i| {
        i.map.process_end(ts, pid);
        TM_OK
    })
}

/// Thread start or rundown. `description` and `start_module` may be NULL.
#[no_mangle]
pub extern "C" fn tm_thread_start(
    h: *mut TmMap,
    ts: i64,
    tid: u32,
    pid: u32,
    description: *const c_char,
    start_module: *const c_char,
    start_offset: u64,
) -> i32 {
    let info = ThreadInfo { ts, tid, pid, description: opt_str(description), start_module: opt_str(start_module), start_offset };
    with(h, TM_ERR_NULL, |i| {
        i.map.thread_start(&info);
        TM_OK
    })
}

#[no_mangle]
pub extern "C" fn tm_thread_end(h: *mut TmMap, ts: i64, tid: u32) -> i32 {
    with(h, TM_ERR_NULL, |i| {
        i.map.thread_end(ts, tid);
        TM_OK
    })
}

/// SetThreadDescription name of a thread (empty clears it).
#[no_mangle]
pub extern "C" fn tm_thread_name(h: *mut TmMap, ts: i64, tid: u32, name: *const c_char) -> i32 {
    let name = opt_str(name).unwrap_or_default();
    with(h, TM_ERR_NULL, |i| {
        i.map.thread_name(ts, tid, &name);
        TM_OK
    })
}

/// Processes held events older than the reorder window (snapshot calls it too).
#[no_mangle]
pub extern "C" fn tm_pump(h: *mut TmMap) -> i32 {
    with(h, TM_ERR_NULL, |i| {
        i.map.pump();
        TM_OK
    })
}

/// Processes every held event (end of a recording).
#[no_mangle]
pub extern "C" fn tm_flush(h: *mut TmMap) -> i32 {
    with(h, TM_ERR_NULL, |i| {
        i.map.flush();
        TM_OK
    })
}

// ---- reading ---------------------------------------------------------------------------------------------------

/// Builds the graph snapshot as JSON and hands its nodes and links to the layout. `query_json` is a Query object
/// (missing fields take defaults); NULL repeats the last query. Free the result with tm_free_str; NULL on a bad
/// query or an error.
#[no_mangle]
pub extern "C" fn tm_snapshot_json(h: *mut TmMap, query_json: *const c_char) -> *mut c_char {
    let q = opt_str(query_json);
    with(h, ptr::null_mut(), |i| {
        if let Some(q) = q {
            match serde_json::from_str::<Query>(&q) {
                Ok(q) => i.query = q,
                Err(_) => return ptr::null_mut(),
            }
        }
        i.map.pump();
        let s = i.map.snapshot(&i.query);
        i.layout.sync(&s);
        serde_json::to_string(&s).map(out_str).unwrap_or(ptr::null_mut())
    })
}

/// Time strip (300 buckets, oldest first) as JSON. Free with tm_free_str.
#[no_mangle]
pub extern "C" fn tm_series_json(h: *mut TmMap) -> *mut c_char {
    with(h, ptr::null_mut(), |i| serde_json::to_string(&i.map.series()).map(out_str).unwrap_or(ptr::null_mut()))
}

/// Counters (events, late drops, threads known, history bytes) as JSON. Free with tm_free_str.
#[no_mangle]
pub extern "C" fn tm_stats_json(h: *mut TmMap) -> *mut c_char {
    with(h, ptr::null_mut(), |i| serde_json::to_string(&i.map.stats()).map(out_str).unwrap_or(ptr::null_mut()))
}

// ---- layout ----------------------------------------------------------------------------------------------------

/// One simulation step for a frame of `dt` seconds; returns the kinetic energy left (< 0 on error).
#[no_mangle]
pub extern "C" fn tm_layout_step(h: *mut TmMap, dt: f32) -> f32 {
    with(h, -1.0, |i| i.layout.step(if dt.is_finite() { dt } else { 0.0 }))
}

/// `steps` steps at once (first placement). Returns the energy left.
#[no_mangle]
pub extern "C" fn tm_layout_warm(h: *mut TmMap, steps: u32) -> f32 {
    with(h, -1.0, |i| i.layout.warm(steps))
}

#[no_mangle]
pub extern "C" fn tm_layout_count(h: *mut TmMap) -> usize {
    with(h, 0, |i| i.layout.len())
}

/// Copies up to `cap` positions into `out` (same order as the snapshot's nodes); returns how many were written.
#[no_mangle]
pub extern "C" fn tm_layout_positions(h: *mut TmMap, out: *mut Pos, cap: usize) -> usize {
    if out.is_null() {
        return 0;
    }
    with(h, 0, |i| {
        let p = i.layout.positions();
        let n = p.len().min(cap);
        // SAFETY: the caller provides room for `cap` elements.
        unsafe { ptr::copy_nonoverlapping(p.as_ptr(), out, n) };
        n
    })
}

/// Moves node `id` to (x, y) in layout space and pins it.
#[no_mangle]
pub extern "C" fn tm_layout_drag(h: *mut TmMap, id: u64, x: f32, y: f32) -> i32 {
    with(h, TM_ERR_NULL, |i| if i.layout.drag(id, x, y) { TM_OK } else { TM_ERR_ARG })
}

#[no_mangle]
pub extern "C" fn tm_layout_pin(h: *mut TmMap, id: u64, pinned: bool) -> i32 {
    with(h, TM_ERR_NULL, |i| if i.layout.set_pinned(id, pinned) { TM_OK } else { TM_ERR_ARG })
}

/// "Lay out again": unpin all, new spots.
#[no_mangle]
pub extern "C" fn tm_layout_relayout(h: *mut TmMap) -> i32 {
    with(h, TM_ERR_NULL, |i| {
        i.layout.relayout();
        TM_OK
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CString;

    #[test]
    fn layouts_match_the_header() {
        use std::mem::{align_of, offset_of, size_of};
        assert_eq!(size_of::<CSwitch>(), 40);
        assert_eq!(offset_of!(CSwitch, old_wait_reason), 22);
        assert_eq!(offset_of!(CSwitch, new_wait_time), 28);
        assert_eq!(size_of::<ReadyThread>(), 24);
        assert_eq!(offset_of!(ReadyThread, flags), 22);
        assert_eq!(size_of::<LinkEvent>(), 24);
        assert_eq!(offset_of!(LinkEvent, count), 20);
        assert_eq!(size_of::<Pos>(), 24);
        assert_eq!(align_of::<Pos>(), 8);
    }

    #[test]
    fn roundtrip_through_c_abi() {
        assert!(tm_new(0).is_null());
        let h = tm_new(10_000_000);
        assert!(!h.is_null());
        assert_eq!(tm_set_game(h, 100), TM_OK);
        let img = CString::new("game.exe").unwrap();
        assert_eq!(tm_process_start(h, 0, 100, img.as_ptr()), TM_OK);
        let main = CString::new("MainThread").unwrap();
        assert_eq!(tm_thread_start(h, 0, 1000, 100, main.as_ptr(), ptr::null(), 0), TM_OK);
        assert_eq!(tm_thread_start(h, 0, 1004, 100, ptr::null(), ptr::null(), 0), TM_OK);
        let mut cs = Vec::new();
        let mut rd = Vec::new();
        for i in 0..200i64 {
            let t = i * 10_000; // 1 ms
            rd.push(ReadyThread { ts: t, cpu: 0, readier_tid: 1000, target_tid: 1004, ..Default::default() });
            cs.push(CSwitch { ts: t + 100, cpu: 1, new_tid: 1004, old_tid: 0, old_state: 2, ..Default::default() });
            cs.push(CSwitch { ts: t + 5000, cpu: 1, new_tid: 0, old_tid: 1004, old_state: 5, old_wait_reason: 6, ..Default::default() });
        }
        assert_eq!(tm_feed_ready(h, rd.as_ptr(), rd.len()), TM_OK);
        assert_eq!(tm_feed_cswitch(h, cs.as_ptr(), cs.len()), TM_OK);
        assert_eq!(tm_feed_cswitch(h, ptr::null(), 3), TM_ERR_ARG);
        assert_eq!(tm_flush(h), TM_OK);
        let q = CString::new(r#"{"focus_pid":100,"expand":[100],"span":5}"#).unwrap();
        let s = tm_snapshot_json(h, q.as_ptr());
        assert!(!s.is_null());
        let js = unsafe { CStr::from_ptr(s) }.to_str().unwrap().to_string();
        tm_free_str(s);
        let v: serde_json::Value = serde_json::from_str(&js).unwrap();
        assert!(v["links"].as_array().unwrap().iter().any(|l| l["kind"] == "wake"));
        let bad = CString::new("{not json").unwrap();
        assert!(tm_snapshot_json(h, bad.as_ptr()).is_null());
        assert!(tm_layout_warm(h, 50) >= 0.0);
        let n = tm_layout_count(h);
        assert!(n >= 2);
        let mut pos = vec![Pos::default(); n];
        assert_eq!(tm_layout_positions(h, pos.as_mut_ptr(), n), n);
        assert_eq!(tm_layout_drag(h, pos[0].id, 5.0, 6.0), TM_OK);
        assert_eq!(tm_layout_drag(h, 12345, 5.0, 6.0), TM_ERR_ARG);
        let ser = tm_series_json(h);
        assert!(!ser.is_null());
        tm_free_str(ser);
        let st = tm_stats_json(h);
        tm_free_str(st);
        assert_eq!(tm_pump(ptr::null_mut()), TM_ERR_NULL);
        assert!(tm_snapshot_json(ptr::null_mut(), ptr::null()).is_null());
        tm_free(h);
        assert!(unsafe { CStr::from_ptr(tm_version()) }.to_str().unwrap().starts_with("0."));
    }
}
