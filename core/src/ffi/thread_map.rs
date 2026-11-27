//! The Thread map page's calls (native-only; app/src/dos_core.h "thread map"). The page reads the map itself through
//! the threadmap crate's C ABI (tm_snapshot_json, tm_series_json, tm_layout_*; header threadmap/threadmap.h, linked
//! into this library); these calls only start and stop its trace and report its state.

use super::abi::{out_json, guard};
use super::app;
use crate::base::l::L;
use crate::etw::thread_map::ThreadMapTrace;
use serde_json::json;
use std::ffi::{c_char, c_int, c_void};

/// The game Auto handles now (0 = none) and its exe.
fn game() -> (u32, String) {
    match app::hub() {
        Some(h) => {
            let pid = h.auto.game_pid();
            (pid, if pid != 0 { h.auto.game_exe() } else { String::new() })
        }
        None => (0, String::new()),
    }
}

/// Starts the trace (demo != 0: the synthetic feed) unless it runs, and returns the tm_map* (owned by the core,
/// valid until the process ends; NULL only when it could not be allocated). Call it while the page is on screen.
#[no_mangle]
pub extern "C" fn dos_tm_open(demo: c_int) -> *mut c_void {
    guard(std::ptr::null_mut(), || {
        let t = ThreadMapTrace::global();
        t.start(demo != 0);
        t.set_game(game().0);
        t.handle() as *mut c_void
    })
}

/// Stops the trace (the page was left or the window hid). The map keeps what it has.
#[no_mangle]
pub extern "C" fn dos_tm_close() {
    guard((), || {
        ThreadMapTrace::global().stop();
    })
}

/// The page's 10 Hz poll: hands the batched events to the map, sets the game pid from Auto, and returns
/// {"running","demo","error" (translated, "" = none),"game_pid","game_exe","lost","reorder_ms"}.
#[no_mangle]
pub extern "C" fn dos_tm_status() -> *mut c_char {
    guard(std::ptr::null_mut(), || {
        let t = ThreadMapTrace::global();
        let (pid, exe) = game();
        t.set_game(pid);
        t.flush_pending();
        let err = t.error();
        out_json(&json!({
            "running": t.running(),
            "demo": t.demo(),
            "error": if err.is_empty() { String::new() } else { crate::lf!("Kernel trace is not available: {0}", err) },
            "game_pid": t.game(),
            "game_exe": if t.demo() { "cs2.exe".to_string() } else { exe },
            "lost": t.lost(),
            "reorder_ms": t.reorder_ms(),
            "lang": L::lang(),
        }))
    })
}
