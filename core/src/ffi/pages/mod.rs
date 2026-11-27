//! What the pages show and do (the logic of every *Page.cs, HomeView.cs, SystemPanel.cs and Shell.cs that decides
//! something), behind three calls: `get` (a state as JSON), `do_` (a command C# ran in the click handler) and
//! `start` (a background job: Shell.Busy on the worker, Shell.BusyTask / Task.Run on a thread, Opt.Queue).
//! The JSON shapes are documented in app/src/dos_core.h.

mod home;
mod hw;
mod other;

pub use hw::parse_plans as hw_parse_plans;

use super::{app, state};
use crate::auto::controller::AutoState;
use crate::base::app_log::{AppLog, Sev};
use crate::base::fmt::Arg;
use crate::base::l::L;
use crate::base::time::DateTime;
use crate::lf;
use crate::system::updater::AppVersion;
use crate::topo::TopoBuilder;
use serde_json::{json, Value};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::Arc;


/// `Ui.Hex`: "0x" + upper-case hex.
pub fn hex(m: u64) -> String {
    format!("0x{:X}", m)
}

/// `S.Did(message, sev)`: AppLog.Add and a redraw.
pub fn did(msg: impl AsRef<str>, sev: Sev) {
    AppLog::add_sev(msg, sev);
    state::bump();
}

pub fn s(v: &Value, k: &str) -> String {
    match v.get(k) {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Number(n)) => n.to_string(),
        Some(Value::Bool(b)) => b.to_string(),
        _ => String::new(),
    }
}

pub fn b(v: &Value, k: &str) -> bool {
    match v.get(k) {
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_f64().unwrap_or(0.0) != 0.0,
        Some(Value::String(s)) => s.eq_ignore_ascii_case("true"),
        _ => false,
    }
}

pub fn i(v: &Value, k: &str) -> i64 {
    match v.get(k) {
        Some(Value::Number(n)) => n.as_i64().or_else(|| n.as_f64().map(|f| f as i64)).unwrap_or(0),
        Some(Value::String(s)) => s.trim().parse().unwrap_or(0),
        _ => 0,
    }
}

/// A mask argument: "0x1F", "31" or a number.
pub fn mask(v: &Value, k: &str) -> u64 {
    match v.get(k) {
        Some(Value::Number(n)) => n.as_u64().unwrap_or(0),
        Some(Value::String(s)) => {
            let t = s.trim();
            if let Some(h) = t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
                u64::from_str_radix(h, 16).unwrap_or(0)
            } else {
                t.parse().unwrap_or(0)
            }
        }
        _ => 0,
    }
}

/// dos_fmt: L.F(en, args) with the JSON arguments as C# objects.
pub fn fmt_args(en: &str, a: &Value) -> String {
    let args: Vec<Arg> = match a {
        Value::Array(v) => v
            .iter()
            .map(|x| match x {
                Value::Bool(b) => Arg::B(*b),
                Value::Number(n) => match n.as_i64() {
                    Some(i) => Arg::I(i),
                    None => Arg::F(n.as_f64().unwrap_or(0.0)),
                },
                Value::String(s) => Arg::S(s.clone()),
                Value::Object(o) => match o.get("ticks").and_then(|t| t.as_i64()) {
                    Some(t) => Arg::T(DateTime::utc(t).to_local()),
                    None => Arg::S(String::new()),
                },
                _ => Arg::S(String::new()),
            })
            .collect(),
        _ => Vec::new(),
    };
    L::f(en, &args)
}

// ---- jobs ------------------------------------------------------------------------------------------------------

fn finish(id: u64, r: std::thread::Result<Value>) {
    let v = match r {
        Ok(v) => v,
        Err(p) => {
            AppLog::add_sev(lf!("Error: {0}", super::optimizer::panic_text(&*p)), Sev::Error);
            Value::Null
        }
    };
    state::job_done(id, v);
}

/// `Shell.BusyTask` / `Task.Run`: a thread of its own; the result goes to dos_job_poll.
pub fn task(f: impl FnOnce() -> Value + Send + 'static) -> u64 {
    let id = state::job_new();
    let r = std::thread::Builder::new().name("task".into()).spawn(move || finish(id, catch_unwind(AssertUnwindSafe(f))));
    if r.is_err() {
        state::job_done(id, Value::Null);
    }
    id
}

/// `Shell.Busy` / `Opt.Run`: on the worker, in order with every other change. Err = "Error: {0}" in the log.
pub fn worker(f: impl FnOnce() -> Result<Value, String> + Send + 'static) -> u64 {
    let Some(a) = app::app() else { return 0 };
    let id = state::job_new();
    a.opt.run(f, move |r: Option<Value>| state::job_done(id, r.unwrap_or(Value::Null)));
    id
}

/// `Opt.Queue(job, skipIfBusy: false, reports: true)`.
pub fn queued(f: impl FnOnce() -> Value + Send + 'static) -> u64 {
    let Some(a) = app::app() else { return 0 };
    let id = state::job_new();
    a.opt.queue(Box::new(move || finish(id, catch_unwind(AssertUnwindSafe(f)))), false, true);
    id
}

// ---- shell -----------------------------------------------------------------------------------------------------

pub fn shell_state() -> Value {
    let Some(h) = app::hub() else {
        return json!({"state": 0, "on": false, "button_enabled": false, "applied_age_s": -1, "status": "", "game_exe": "",
            "blocked": "", "threads": 0, "mask_now": "0x0", "baseline_p99": 0.0, "background_count": 0, "changed_count": 0,
            "game_running": false, "paused": false});
    };
    let a = &h.auto;
    let st = a.state();
    let applied = a.applied_at();
    let now = DateTime::utc_now();
    let age = if applied == DateTime::MIN || applied > now { -1.0 } else { (now - applied).total_seconds() };
    let threads = if a.top5_mode() { a.dynamic().view().len() } else { a.plan_view().iter().filter(|p| p.4).count() };
    json!({
        "state": st as i32,
        "on": a.on(),
        "button_enabled": a.button_enabled(),
        "applied_age_s": age,
        "status": a.status(),
        "game_exe": a.game_exe(),
        "blocked": a.blocked(),
        "threads": threads,
        "mask_now": hex(a.mask_now()),
        "baseline_p99": a.baseline_p99(),
        "background_count": h.engine.background_count(),
        "changed_count": h.engine.changed_pids().len(),
        "game_running": st == AutoState::Active || a.last_game_running(),
        "paused": h.paused(),
    })
}

pub fn facts() -> Value {
    let t = TopoBuilder::live();
    let f = app::app().and_then(|a| a.facts.read().unwrap_or_else(|e| e.into_inner()).clone());
    let gpu = f.as_ref().and_then(|f| f.main_gpu().map(|g| g.name.clone())).unwrap_or_default();
    json!({
        "version": AppVersion::display(),
        "cpu": crate::native::sys::cpu_name(),
        "physical": t.as_ref().map(|t| t.physical_count).unwrap_or(0),
        "logical": t.as_ref().map(|t| t.logical_count).unwrap_or(0),
        "gpu": gpu,
        "hardware": f.map(|f| serde_json::to_value(f).unwrap_or(Value::Null)),
    })
}

/// The status column's Turn on / Turn off.
pub fn auto_click() {
    let Some(a) = app::app() else { return };
    a.hub.set_paused(false);
    let auto = a.hub.auto.clone();
    a.opt.queue(Box::new(move || auto.click()), false, true);
}

/// Shell.UndoEverything.
pub fn undo_everything() {
    let Some(a) = app::app() else { return };
    a.hub.set_paused(true);
    let auto = a.hub.auto.clone();
    a.opt.run(
        move || Ok(auto.revert_everything(L::t("Button"))),
        |_| did(L::t("Everything was put back"), Sev::Info),
    );
}

// ---- dispatch ----------------------------------------------------------------------------------------------------

fn unknown(name: &str) -> Value {
    json!({"error": format!("unknown: {}", name)})
}

pub fn get(name: &str, a: &Value) -> Value {
    home::get(name, a).or_else(|| hw::get(name, a)).or_else(|| other::get(name, a)).unwrap_or_else(|| unknown(name))
}

pub fn do_(name: &str, a: &Value) -> Value {
    home::do_(name, a).or_else(|| hw::do_(name, a)).or_else(|| other::do_(name, a)).unwrap_or_else(|| unknown(name))
}

pub fn start(name: &str, a: Arc<Value>) -> u64 {
    home::start(name, &a).or_else(|| hw::start(name, &a)).or_else(|| other::start(name, &a)).unwrap_or(0)
}
