//! MasksPage.cs, ProcessesPage.cs, SettingsPage.cs, UpdatePage.cs (+ UpdateCard), CheckPage.cs, BlackBoxPage.cs,
//! ChangesPage.cs and AboutPage.cs.

use super::{b, did, hex, i, mask, queued, s, task, worker};
use crate::auto::check_stats::{CheckResult, CheckStats};
use crate::base::app_log::{AppLog, Sev};
use crate::base::ci::{self, CiSetExt};
use crate::base::fmt::{num, Culture};
use crate::base::flags::Flags;
use crate::base::hooks::lock;
use crate::base::l::L;
use crate::base::settings::Settings;
use crate::base::time::DateTime;
use crate::engine::Tables;
use crate::etw::black_box::BbCause;
use crate::etw::CheckTrace;
use crate::ffi::hub::Hub;
use crate::ffi::update_service::{tester_json, UpdateService, SITE_EN, SITE_RU};
use crate::ffi::{app, hardware, win, wmi};
use crate::lf;
use crate::system::tweaks::{RegChange, TweakState};
use crate::system::updater::{AppVersion, TesterKey, Updater};
use crate::system::{Autostart, Store, Tweaks};
use crate::topo::game_mask::Edits;
use crate::topo::{GameMask, Masks, Reservation, TopoBuilder};
use serde_json::{json, Value};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

fn auto_on() -> bool {
    app::hub().map(|h| h.auto.on()).unwrap_or(false)
}

fn configure() {
    if let Some(h) = app::hub() {
        h.configure();
    }
}

// ---- masks (MasksPage.cs) ----------------------------------------------------------------------------------------

struct Draft {
    spec: String,
    irq: String,
    no_smt: bool,
    no_core0: bool,
    add: u64,
    drop: u64,
}

impl Draft {
    fn of(a: &Value) -> Draft {
        Draft { spec: s(a, "spec"), irq: s(a, "irq"), no_smt: b(a, "nosmt"), no_core0: b(a, "nocore0"), add: mask(a, "add"), drop: mask(a, "drop") }
    }
    fn spec_text(&self) -> String {
        format!("{}{}{}", self.spec, if self.no_smt { "+nosmt" } else { "" }, if self.no_core0 { "+nocore0" } else { "" })
    }
    fn info(&self, t: &crate::topo::Topo) -> crate::base::model::GameMaskInfo {
        GameMask::build(t, &self.spec_text(), Some(&Edits { add: self.add, drop: self.drop, sig: GameMask::cpu_sig(t) }), &self.irq)
    }
}

fn masks_stored() -> Value {
    let t = TopoBuilder::live();
    let e = Hub::mask_edits();
    let flag = t.as_ref().map(|t| e.sig != GameMask::cpu_sig(t)).unwrap_or(true);
    json!({
        "spec": Settings::get("MaskName"), "irq": Settings::get("IrqMode"),
        "nosmt": Settings::get_bool("MaskNoSmt"), "nocore0": Settings::get_bool("MaskNoCore0"),
        "add": hex(if flag { 0 } else { e.add }), "drop": hex(if flag { 0 } else { e.drop }),
    })
}

fn masks_preview(a: &Value) -> Value {
    let Some(t) = TopoBuilder::live() else { return Value::Null };
    let d = Draft::of(a);
    let split = t.split();
    let info = d.info(&t);
    let mut options = vec![
        json!({"key": "auto", "label": L::t("auto — chosen for this processor")}),
        json!({"key": "all", "label": L::t("all processors")}),
        json!({"key": "game", "label": L::t("game cores")}),
        json!({"key": "nongame", "label": L::t("non-game cores")}),
        json!({"key": "vcache", "label": L::t("big-cache cores")}),
        json!({"key": "pcores", "label": L::t("P-cores")}),
        json!({"key": "ecores", "label": L::t("E-cores")}),
    ];
    if t.llc_groups.len() > 1 {
        for (n, llc) in t.llc_groups.iter().enumerate() {
            let mut text = lf!("L3 domain {0}", n as i32) + " · " + &Masks::ranges(TopoBuilder::mask(t.sets.iter().filter(|s| s.llc == *llc)));
            if t.big_cache_llc == Some(*llc) {
                text += &format!(" · {}", L::t("big cache"));
            }
            options.push(json!({"key": format!("ccd{}", n), "label": text}));
        }
    }
    let mr = Masks::resolve(&t, &d.spec_text());
    let mut warn: Vec<String> = mr.notes.iter().map(|n| Masks::note_text(n)).collect();
    if info.edits_stale {
        warn.push(L::t("The manual edits were made on another processor and are dropped.").to_string());
    }
    let mut notes: Vec<String> = info
        .notes
        .iter()
        .map(|n| if n == "wide-irq: partial" { L::t("wide-irq: partial — a device pinned to several cores takes out only its own processors, not whole cores").to_string() } else { n.clone() })
        .collect();
    if info.irq_ignored {
        notes.push(L::t("IRQ refinement ignored: it would remove more than half of the mask.").to_string());
    }
    for (k, v) in &info.irq_devices {
        notes.push(lf!("{0}: processors {1}", k, Masks::ranges(*v)));
    }
    for (k, v) in &info.irq_wide {
        notes.push(lf!("{0}: processors {1} (several cores, not excluded)", k, Masks::ranges(*v)));
    }
    let x3d = if split.family == "x3d" {
        format!(" ({})", if split.upper_half { L::t("big cache, upper half of the numbers") } else { L::t("big cache, lower half of the numbers") })
    } else {
        String::new()
    };
    json!({
        "options": options,
        "irq_options": [
            {"key": "Off", "label": L::t("IRQ: do not refine")},
            {"key": "Thread", "label": L::t("IRQ: exclude the pinned processors")},
            {"key": "Core", "label": L::t("IRQ: exclude the whole pinned cores")},
        ],
        "split_text": if split.can_split() { format!("{} — {}", split.label(), lf!("game {0}, non-game {1}", Masks::ranges(split.game), Masks::ranges(split.non_game))) } else { split.why() },
        "mask_text": format!("{}  {}{}", hex(info.final_), Masks::ranges(info.final_), if info.adjusted() { format!("  · {}", L::t("adjusted")) } else { String::new() }),
        "background_text": Masks::ranges(GameMask::background(&t, &info)),
        "warn": warn, "notes": notes,
        "base": hex(info.base), "final": hex(info.final_), "irq_drop": hex(info.irq_drop), "game": hex(split.game),
        "can_split": split.can_split(),
        "legend_game": format!("{}{}", L::t("game cores"), x3d),
        "click_note": if split.can_split() { L::t("Click a processor to add it to the mask or take it out.").to_string() } else { split.why() },
    })
}

fn masks_apply(a: &Value) -> Value {
    let Some(t) = TopoBuilder::live() else { return json!({"error": "no topology"}) };
    let d = Draft::of(a);
    let _ = Settings::set("MaskName", &d.spec);
    let _ = Settings::set("IrqMode", &d.irq);
    let _ = Settings::set_bool("MaskNoSmt", d.no_smt);
    let _ = Settings::set_bool("MaskNoCore0", d.no_core0);
    let _ = Settings::set("MaskAdd", &format!("{:X}", d.add));
    let _ = Settings::set("MaskDrop", &format!("{:X}", d.drop));
    let _ = Settings::set("MaskSig", &GameMask::cpu_sig(&t));
    configure();
    let info = d.info(&t);
    let msg = lf!("Mask saved: {0} → {1} ({2})", d.spec_text(), hex(info.final_), Masks::ranges(info.final_));
    did(&msg, Sev::Info);
    json!({"msg": msg})
}

fn reservation() -> Value {
    let Some(t) = TopoBuilder::live() else { return Value::Null };
    let st = Reservation::read();
    let v = if st.kind == "error" {
        lf!("cannot be read ({0})", &st.error)
    } else if st.mask == 0 {
        L::t("not set").to_string()
    } else {
        lf!("processors {0}", Masks::ranges(st.mask)) + &if st.pending { format!(" · {}", L::t("waits for a reboot")) } else { String::new() }
    };
    let list = Reservation::refusals(Some(&t), t.split().non_game);
    let on = auto_on();
    json!({
        "now": v,
        "refusals": if list.is_empty() { String::new() } else { lf!("Cannot be reserved here: {0}", list.join("; ")) },
        "can_apply": list.is_empty() && !on,
        "can_revert": st.has_backup && !on,
    })
}

// ---- processes (ProcessesPage.cs) --------------------------------------------------------------------------------

fn class_text(cls: u32) -> &'static str {
    match cls {
        64 => L::t("Idle"),
        16384 => L::t("Below normal"),
        32 => L::t("Normal"),
        32768 => L::t("Above normal"),
        128 => L::t("High"),
        256 => L::t("Realtime"),
        _ => "",
    }
}

fn image_path(pid: u32) -> String {
    #[cfg(windows)]
    {
        crate::native::win::image_path(pid)
    }
    #[cfg(not(windows))]
    {
        let _ = pid;
        String::new()
    }
}

fn processes_list() -> u64 {
    let Some(h) = app::hub() else { return 0 };
    worker(move || {
        let watch = h.watch();
        let mut rows = Vec::new();
        for (pid, (exe, _)) in h.engine.processes_now() {
            if pid == 0 || pid == 4 {
                continue;
            }
            let info = h.engine.read_process(pid, &exe);
            let path = if info.denied { String::new() } else { image_path(pid) };
            rows.push((pid, exe.clone(), path, info, watch.ci_contains(&exe)));
        }
        rows.sort_by(|a, b| b.4.cmp(&a.4).then_with(|| ci::upper(&a.1).cmp(&ci::upper(&b.1))).then(a.0.cmp(&b.0)));
        let v: Vec<Value> = rows
            .iter()
            .map(|(pid, exe, path, info, w)| {
                json!({"pid": pid, "exe": exe, "path": path, "watched": w, "denied": info.denied, "ok": info.ok, "changed": info.changed,
                    "class_text": if info.denied { L::t("denylist") } else if info.ok { class_text(info.class) } else { "" },
                    "affinity": hex(info.affinity)})
            })
            .collect();
        Ok(json!({"rows": v}))
    })
}

fn process_apply(a: &Value) -> u64 {
    let Some(h) = app::hub() else { return 0 };
    let exe = s(a, "exe");
    let m = mask(a, "mask");
    let _ = Settings::save_aff_rule(&exe, &hex(m));
    h.configure();
    h.set_paused(false);
    let h2 = h.clone();
    worker(move || {
        h2.sys.pass(h2.auto.game_pid());
        did(lf!("{0}: mask {1} ({2}) saved and applied", &exe, hex(m), Masks::ranges(m)), Sev::Info);
        Ok(json!({}))
    })
}

fn process_reset(a: &Value) -> u64 {
    let Some(h) = app::hub() else { return 0 };
    let exe = s(a, "exe");
    let pid = i(a, "pid") as u32;
    let _ = Settings::save_aff_rule(&exe, "");
    h.configure();
    let h2 = h.clone();
    worker(move || {
        let n = h2.engine.revert(pid);
        did(lf!("{0}: rule removed, {1} values put back", &exe, n), Sev::Info);
        Ok(json!({}))
    })
}

// ---- settings (SettingsPage.cs) ----------------------------------------------------------------------------------

/// The Toggle rows: key, label, hint, fallback (the Functions card order), plus UpdatePage's "CheckUpdates".
fn toggles() -> Vec<(&'static str, &'static str, &'static str, bool)> {
    vec![
        ("BgOn", L::t("Lower background programs while a watched game runs"), L::t("Lower priority and power-saving mode for background programs. Audio, voice, streaming, VR, launchers and Explorer are never touched. Off puts everything back."), false),
        ("TimerGuard", L::t("Ignore timer requests of background programs while a game runs"), L::t("Background programs (RGB, monitoring, launchers) ask for a fast timer that wakes every core up to 2000 times a second. While the game runs, their requests are ignored; the game keeps its own. Off puts everything back."), true),
        // 1.16.0: the burst guard (on, soft mode by default).
        ("BurstGuard", L::t("Burst guard: background programs that load the processor give way to the game"), L::t("Only a program that keeps using a third of a processor or more; it is limited to half a processor until the game ends or you switch to it. When free memory is low, the game keeps its memory. On a clean PC it does nothing."), true),
        ("BurstGuardSoft", L::t("Burst guard: soft mode"), L::t("On: a busy background program only gives way to the game and is never stopped. Off: a hard limit of half a processor, which can pause that program for a moment."), true),
        ("WaitProbe", L::t("Rank the game's threads by how long they wait (wait probe)"), L::t("While Auto handles a game, Windows' trace is switched on for a quarter of a second every 2 seconds: Top-5 then helps the threads that wait for a core, keeps off processors busy with drivers, and the game's memory is held only when it really reads pages back from disk. Only reads."), true),
        // 1.17.1: Dynamic Top-5's other threads.
        ("AllThreadsIdeal", L::t("Ideal cores for all game threads"), L::t("With Auto, every other thread of the game (DLL threads too) gets a preferred core away from core 0, the Top-5 cores and the cores drivers keep busy, and is moved only for a clear reason. Only a hint: the threads still migrate. Off puts the values back."), true),
        ("FaceitCs2", L::t("CS2 under FACEIT: tune the game too"), L::t("FACEIT confirmed in writing (25.11.2026) that their anti-cheat allows it: priorities and cores of CS2's threads. Only CS2 and only FACEIT; other anti-cheats stay untouched. Off: under FACEIT only the background is quieted."), true),
        ("BlackBox", L::t("Black box: explain every stutter of the game"), L::t("While Auto handles a game, the last 30 seconds of the system trace stay in memory; a stutter is explained on the Black box page. Only reads, changes nothing."), false),
        ("LiveIrq", L::t("Watch driver latency while a game runs (live DPC/ISR)"), L::t("While the game runs, Windows measures how long each driver works. Drivers that took 1 ms or more go to the log. Nothing is changed."), true),
        ("LiveIrqMask", L::t("Keep the game off cores busy with interrupts"), L::t("Needs the line above. A core busy with interrupts more than 4% of the time (for 20 s) is taken away from the game until it calms down under 2%."), true),
    ]
}

fn on_off(label: &str, v: bool) -> String {
    if v { lf!("{0}: on", label) } else { lf!("{0}: off", label) }
}

fn settings() -> Value {
    let reapply = Settings::get("ReapplyMs");
    let keys = ["1000", "2000", "5000", "10000", "30000"];
    let watch = app::hub().map(|h| h.watch()).unwrap_or_default();
    let mut w: Vec<String> = watch.iter().map(|x| x.0.clone()).collect();
    w.sort_by(|a, b| ci::upper(a).cmp(&ci::upper(b)));
    let mut known: Vec<(String, String)> = Tables::known_games().iter().filter(|g| !watch.ci_contains(&g.0 .0)).map(|g| (g.0 .0.clone(), g.1.clone())).collect();
    known.sort_by(|a, b| ci::upper(&a.1).cmp(&ci::upper(&b.1)));
    json!({
        "sys_presets": Settings::get_bool("SysPresets"),
        "home_charts": Settings::get_bool_or("HomeCharts", true),
        "reapply": if keys.contains(&reapply.as_str()) { reapply } else { "10000".into() },
        "reapply_options": keys.iter().map(|k| json!({"key": k, "label": lf!("{0} s", k.parse::<i32>().unwrap_or(0) / 1000)})).collect::<Vec<_>>(),
        "lang": L::lang(),
        "languages": L::available().iter().map(|c| json!({"key": c, "label": L::name_of(c)})).collect::<Vec<_>>(),
        "functions": toggles().iter().map(|(k, l, h, f)| json!({"key": k, "label": l, "hint": h, "value": Settings::get_bool_or(k, *f)})).collect::<Vec<_>>(),
        "flags": [
            {"key": "devices-write", "label": L::t("Device interrupt policy write (Devices page)"), "open": Flags::open("devices-write")},
            {"key": "gpu-oc", "label": L::t("GPU overclocking (NVIDIA: clock offsets and voltage boost; AMD Radeon: clocks, power limit and undervolt)"), "open": Flags::open("gpu-oc")},
        ],
        "watch": w,
        "known": known.iter().map(|(k, v)| json!({"key": k, "label": format!("{} — {}", v, k)})).collect::<Vec<_>>(),
        "check_updates": UpdateService::enabled(),
    })
}

fn setting_toggle(a: &Value) -> Value {
    let key = s(a, "key");
    let v = b(a, "value");
    let label = if key == "CheckUpdates" {
        L::t("Check for updates")
    } else {
        match toggles().into_iter().find(|t| t.0 == key) {
            Some(t) => t.1,
            None => return json!({"error": "unknown toggle"}),
        }
    };
    let _ = Settings::set_bool(&key, v);
    configure();
    did(on_off(label, v), Sev::Info);
    if !v {
        if let (Some(ap), true) = (app::app(), key == "BgOn" || key == "TimerGuard") {
            let h = ap.hub.clone();
            if key == "BgOn" {
                ap.opt.queue(Box::new(move || {
                    h.auto.standalone_background(false, 0);
                }), false, true);
            } else {
                ap.opt.queue(Box::new(move || {
                    h.engine.revert_background();
                }), false, true);
            }
        }
    }
    json!({})
}

fn autostart_set(v: bool) -> u64 {
    task(move || {
        let r = if v {
            Autostart::task_create(&std::env::current_exe().unwrap_or_default().to_string_lossy())
        } else {
            Autostart::task_delete();
            Ok(())
        };
        let exists = match r {
            Ok(()) => {
                let _ = Settings::set_bool("Autostart", v);
                Autostart::task_exists()
            }
            Err(e) => {
                AppLog::add_sev(lf!("Error: {0}", e), Sev::Error);
                false
            }
        };
        did(if exists { L::t("Autostart task created") } else { L::t("Autostart task removed") }, if exists != v { Sev::Error } else { Sev::Info });
        json!({"exists": exists})
    })
}

fn resync() -> u64 {
    let Some(h) = app::hub() else { return 0 };
    worker(move || {
        TopoBuilder::invalidate();
        GameMask::invalidate_irq();
        Tables::reload();
        h.load_presets()?;
        h.configure();
        let n = h.presets().len();
        let w: Vec<String> = h.watch().iter().map(|x| x.0.clone()).collect();
        did(lf!("Resynced: {0} presets, watchlist {1}", n as i32, w.join(", ")), Sev::Info);
        Ok(json!({"presets": n}))
    })
}

// ---- updates (UpdatePage.cs) ---------------------------------------------------------------------------------------

fn update() -> Value {
    let mut v = UpdateService::instance().map(|u| u.card(app::prev_exists())).unwrap_or_else(|| json!({}));
    if let Some(o) = v.as_object_mut() {
        o.insert("tester".into(), tester_json());
        o.insert("head".into(), json!(L::t("Updates")));
    }
    v
}

fn check_now() {
    if let Some(u) = UpdateService::instance() {
        u.check_now();
    }
}

fn tester_key_save(typed: String) -> u64 {
    task(move || {
        let Some(key) = TesterKey::parse(Some(&typed)) else {
            return json!({"ok": false, "state": L::t("This is not a tester key. Copy it whole, starting with DOST-.")});
        };
        if let Some(err) = Updater::try_key(&key) {
            return json!({"ok": false, "state": L::t(&err)});
        }
        let _ = Settings::set("TesterKey", &TesterKey::format(&key));
        did(L::t("Tester key saved: test channel on"), Sev::Info);
        check_now();
        json!({"ok": true, "state": tester_json()["state"].clone()})
    })
}

// ---- check (CheckPage.cs) ------------------------------------------------------------------------------------------

pub const RUN_SECONDS: i32 = 30;
pub const WARMUP: i32 = 6;
pub const COUNTDOWN: i32 = 10;

struct CheckSt {
    running: bool,
    cancel: Arc<AtomicBool>,
    done: Vec<CheckResult>,
    game: String,
    text: String,
    brush: &'static str,
}

static CHECK: Mutex<Option<CheckSt>> = Mutex::new(None);

fn check_st<R>(f: impl FnOnce(&mut CheckSt) -> R) -> R {
    let mut g = lock(&CHECK);
    let st = g.get_or_insert_with(|| CheckSt { running: false, cancel: Arc::new(AtomicBool::new(false)), done: Vec::new(), game: String::new(), text: L::t("Ready.").to_string(), brush: "Text" });
    f(st)
}

/// The game to measure: the one Auto handles, else the only running program from the watchlist.
fn find_game() -> (u32, String) {
    let Some(h) = app::hub() else { return (0, String::new()) };
    let pid = h.auto.game_pid();
    if pid != 0 {
        return (pid, h.auto.game_exe());
    }
    let mut watch = crate::base::ci::CiSet::new();
    for w in Settings::watch() {
        watch.ci_add(&w);
    }
    let running: Vec<(u32, String)> = h.engine.processes_now().into_iter().filter(|p| watch.ci_contains(&p.1 .0)).map(|p| (p.0, p.1 .0)).collect();
    if running.len() == 1 { running[0].clone() } else { (0, String::new()) }
}

fn say(text: String) {
    check_st(|c| {
        c.text = text;
        c.brush = "Accent";
    });
    crate::ffi::state::bump();
}

struct Canceled;

fn sleep(seconds: i32, ct: &AtomicBool) -> Result<(), Canceled> {
    for _ in 0..seconds * 20 {
        if ct.load(Ordering::SeqCst) {
            return Err(Canceled);
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    if ct.load(Ordering::SeqCst) { Err(Canceled) } else { Ok(()) }
}

enum CheckErr {
    Canceled,
    Failed(String),
}

impl From<Canceled> for CheckErr {
    fn from(_: Canceled) -> Self {
        CheckErr::Canceled
    }
}

fn measure(pid: u32, group: &str, run: i32, ct: &AtomicBool) -> Result<CheckResult, CheckErr> {
    let trace = CheckTrace::new(pid);
    let r = (|| -> Result<CheckResult, CheckErr> {
        if !trace.start() {
            return Err(CheckErr::Failed(trace.error()));
        }
        say(lf!("{0}, run {1}: the trace starts…", group, run));
        sleep(WARMUP, ct)?;
        win::beep(880, 150);
        trace.begin();
        for s in 1..=RUN_SECONDS {
            sleep(1, ct)?;
            say(lf!("{0}, run {1}: {2} / {3} s", group, run, s, RUN_SECONDS));
        }
        trace.end();
        win::beep(1175, 150);
        let (waits, presents, first, last) = trace.snapshot();
        let r = CheckStats::from(group, run, &waits, &presents, first, last, CheckTrace::ticks_per_second(), trace.lost());
        AppLog::add(lf!("Check {0} #{1}: {2:0} FPS, 1% low {3:0}, longest frame {4:0.00} ms, waits over 500 µs {5}", group, run, r.fps, r.low1, r.max_ms, r.over500));
        Ok(r)
    })();
    trace.dispose();
    r
}

fn save_report() -> String {
    let (done, game) = check_st(|c| (c.done.clone(), c.game.clone()));
    if done.is_empty() {
        return L::t("Nothing was measured.").to_string();
    }
    let inv = Culture::invariant();
    let header = lf!("Domain OS Toolbox {0} — check, {1}", AppVersion::full(), DateTime::now().format("yyyy-MM-dd HH:mm", &inv));
    let text = CheckStats::report(&done, &game, &header);
    let name = format!("DomainOS-Check-{}.txt", DateTime::now().format("yyyyMMdd-HHmmss", &inv));
    let file = std::path::Path::new(&win::desktop_dir()).join(&name);
    match std::fs::write(&file, format!("\u{feff}{}", text)) {
        Ok(()) => {
            AppLog::add(lf!("Check saved: {0}", file.to_string_lossy().into_owned()));
            lf!("Done. The report is on the desktop: {0}", name)
        }
        Err(e) => lf!("Done, but the report could not be saved: {0}", e.to_string()),
    }
}

fn check_start() -> Value {
    let (pid, exe) = find_game();
    if pid == 0 {
        check_st(|c| {
            c.text = L::t("No game to measure.").to_string();
            c.brush = "Warn";
        });
        return json!({"started": false});
    }
    let ct = Arc::new(AtomicBool::new(false));
    let started = check_st(|c| {
        if c.running {
            return false;
        }
        c.running = true;
        c.cancel = ct.clone();
        c.done.clear();
        c.game = exe.clone();
        true
    });
    if !started {
        return json!({"started": false});
    }
    let _ = std::thread::Builder::new().name("check".into()).spawn(move || {
        let single = || -> Result<String, CheckErr> {
            for s in (1..=COUNTDOWN).rev() {
                say(lf!("Switch to the game: the check starts in {0} s", s));
                sleep(1, &ct)?;
            }
            let group = if app::hub().map(|h| h.auto.game_pid() == pid).unwrap_or(false) { L::t("with Auto") } else { L::t("without Auto") };
            let r = measure(pid, group, 1, &ct)?;
            check_st(|c| c.done.push(r));
            Ok(save_report())
        };
        let end = match single() {
            Ok(t) => t,
            Err(CheckErr::Canceled) => L::t("Stopped.").to_string(),
            Err(CheckErr::Failed(e)) => {
                let t = lf!("The check stopped: {0}", e);
                AppLog::add_sev(&t, Sev::Warn);
                t
            }
        };
        check_st(|c| {
            c.running = false;
            c.text = end;
            c.brush = "Text";
        });
        crate::ffi::state::bump();
    });
    json!({"started": true})
}

fn check() -> Value {
    let (pid, exe) = find_game();
    check_st(|c| {
        let results: Vec<Value> = c
            .done
            .iter()
            .map(|r| {
                json!({
                    "head": lf!("{0}, run {1}", &r.group, r.run),
                    "frames": if r.frames > 0 {
                        lf!("{0:0} FPS · 1% low {1:0} · 0.1% low {2:0} · longest frame {3:0.00} ms · hitches {4}", r.fps, r.low1, r.low01, r.max_ms, r.hitches)
                    } else {
                        L::t("No frames seen (the game does not use DirectX 10/11/12, or it was not in front).").to_string()
                    },
                    "waits": lf!("Game threads waited for a processor: p99 {0:0.0} µs · p99.9 {1:0.0} µs · longest {2:0} µs · over 500 µs: {3}", r.wait_p99_us, r.wait_p999_us, r.wait_max_us, r.over500),
                })
            })
            .collect();
        json!({
            "running": c.running,
            "game_found": pid != 0,
            "game": if pid != 0 { lf!("Game: {0} (PID {1})", &exe, pid) } else { L::t("Game: none found. Start a game from the watchlist (or let Auto pick it up), then press a button.").to_string() },
            "state": c.text, "state_brush": c.brush,
            "results": if c.running { Vec::new() } else { results },
        })
    })
}

// ---- black box (BlackBoxPage.cs) ---------------------------------------------------------------------------------

fn cause_badge(c: BbCause) -> &'static str {
    match c {
        BbCause::Game => L::t("game or graphics card"),
        BbCause::Driver => L::t("driver"),
        BbCause::Other => L::t("another program"),
        BbCause::Loading => L::t("loading"),
        BbCause::Cores => L::t("not enough free cores"),
    }
}

fn blackbox() -> Value {
    let Some(h) = app::hub() else { return Value::Null };
    let state = if !h.black_box_on() {
        L::t("Off (Settings, Black box).").to_string()
    } else if h.bbox.running() {
        lf!("Recording {0}. Play as usual.", h.auto.game_exe())
    } else {
        L::t("Waiting for a game: turn Auto on and start the game.").to_string()
    };
    let reports = h.bbox.reports();
    if reports.is_empty() {
        return json!({"state": state, "count": 0, "summary": L::t("No stutters recorded yet."),
            "empty_note": L::t("When a frame takes more than three times the usual and longer than two refreshes of the monitor, it appears here."), "reports": []});
    }
    let ru = L::lang() == "ru";
    let n = reports.len();
    let inv = Culture::invariant();
    let part = |c: BbCause, name: &str| -> Option<String> {
        let k = reports.iter().filter(|r| r.cause == c).count();
        (k > 0).then(|| format!("{} {} ({}%)", name, k, num(k as f64 * 100.0 / n as f64, "0", &inv)))
    };
    let parts: Vec<String> = [
        part(BbCause::Game, L::t("game or graphics card")),
        part(BbCause::Driver, L::t("driver")),
        part(BbCause::Other, L::t("another program")),
        part(BbCause::Cores, L::t("not enough free cores")),
        part(BbCause::Loading, L::t("loading")),
    ]
    .into_iter()
    .flatten()
    .collect();
    let list: Vec<Value> = reports
        .iter()
        .take(30)
        .map(|r| {
            json!({
                "when": format!("{}   {} ms", r.when.format("HH:mm:ss", &inv), num(r.frame_ms, "0.0", &inv)),
                "badge": cause_badge(r.cause),
                "brush": match r.cause { BbCause::Game | BbCause::Loading => "Dim", BbCause::Cores => "Warn", _ => "Accent" },
                "text": if ru { &r.text_ru } else { &r.text_en },
                "advice": format!("{}{}", if ru { "Что сделать: " } else { "What to do: " }, if ru { &r.advice_ru } else { &r.advice_en }),
            })
        })
        .collect();
    json!({"state": state, "count": n, "summary": lf!("{0} stutters: ", n as i32) + &parts.join(", ") + ".", "empty_note": "", "reports": list})
}

fn blackbox_open() -> Value {
    if let Ok(d) = Store::dir() {
        let f = d.join("blackbox");
        let _ = std::fs::create_dir_all(&f);
        let _ = win::shell_open("explorer.exe", Some(&format!("\"{}\"", f.to_string_lossy())));
    }
    json!({})
}

// ---- changes (ChangesPage.cs) ------------------------------------------------------------------------------------

fn describe(c: &RegChange) -> String {
    if !c.if_key_exists {
        if c.hive != "BCD" {
            let u: Vec<u16> = c.on.encode_utf16().collect();
            let on = if u.len() > 12 { String::from_utf16_lossy(&u[..12]) + "…" } else { c.on.clone() };
            return format!("{}\\…\\{} = {}", c.hive, c.name, on);
        }
        return format!("bcdedit {} {}", c.name, c.on);
    }
    format!("{} → {}", c.path.split('\\').next_back().unwrap_or(""), L::t("disabled"))
}

fn changes() -> Value {
    let mut groups: Vec<(String, Vec<Value>)> = Vec::new();
    for t in Tweaks::catalog() {
        let st = Tweaks::state(t).unwrap_or(TweakState::Off);
        let item = json!({
            "id": t.id, "title": L::t(&t.title),
            "state": match st { TweakState::Off => 0, TweakState::On => 1, TweakState::Mixed => 2 },
            "pill": match st { TweakState::On => L::t("On"), TweakState::Mixed => L::t("Partly"), _ => L::t("Off") },
            "button": if st == TweakState::On { L::t("Switch off") } else { L::t("Switch on") },
            "primary": st != TweakState::On,
            "what": L::t(&t.what), "why": L::t(&t.why), "cost": L::t(&t.cost),
            "changes": t.changes.iter().map(describe).collect::<Vec<_>>().join("   "),
        });
        match groups.iter_mut().find(|g| g.0 == t.group) {
            Some(g) => g.1.push(item),
            None => groups.push((t.group.clone(), vec![item])),
        }
    }
    json!({"groups": groups.into_iter().map(|(g, items)| json!({"title": L::t(&g), "items": items})).collect::<Vec<_>>()})
}

fn tweak_toggle(id: &str) -> Value {
    let Some(t) = Tweaks::catalog().iter().find(|t| t.id == id) else { return json!({"error": "unknown tweak"}) };
    let on = Tweaks::state(t).map(|s| s == TweakState::On).unwrap_or(false);
    let text = if on { Tweaks::revert(t) } else { Tweaks::apply(t) };
    let title = L::t(&t.title);
    match &text {
        None => did(if on { lf!("{0}: off, the previous values are back", title) } else { lf!("{0}: on", title) }, Sev::Info),
        Some(e) => did(format!("{}: {}", title, e), Sev::Error),
    }
    if text.is_none() && t.needs_restart {
        did(L::t("Restart Windows for this to take effect."), Sev::Info);
    }
    json!({})
}

fn tweaks_recommended() -> Value {
    let mut n = 0;
    for t in Tweaks::catalog().iter().filter(|t| t.recommended && Tweaks::state(t).map(|s| s != TweakState::On).unwrap_or(true)) {
        match Tweaks::apply(t) {
            None => n += 1,
            Some(e) => AppLog::add_sev(format!("{}: {}", L::t(&t.title), e), Sev::Error),
        }
    }
    did(lf!("System changes switched on: {0}", n), Sev::Info);
    json!({})
}

// ---- about (AboutPage.cs) ----------------------------------------------------------------------------------------

fn size_text(bytes: i64) -> String {
    let c = Culture::current();
    if bytes < 1073741824 {
        if bytes < 1048576 {
            return format!("{} KB", num(bytes as f64 / 1024.0, "0", &c));
        }
        return format!("{} MB", num(bytes as f64 / 1048576.0, "0.0", &c));
    }
    format!("{} GB", num(bytes as f64 / 1073741824.0, "0.0", &c))
}

fn file_time(p: &std::path::Path) -> Option<DateTime> {
    let m = std::fs::metadata(p).ok()?.modified().ok()?;
    let d = m.duration_since(std::time::UNIX_EPOCH).ok()?;
    Some(DateTime::from_file_time_utc(d.as_nanos() as i64 / 100 + 116_444_736_000_000_000))
}

fn about() -> Value {
    let topo = TopoBuilder::live();
    let cpu = crate::native::sys::cpu_name();
    let exe = std::env::current_exe().unwrap_or_default();
    let h = app::hub();
    let st = h.as_ref().map(|h| h.self_test()).unwrap_or_default();
    json!({
        "cpu": match &topo { Some(t) => format!("{} · {}", cpu, t.split().label()), None => cpu },
        "topology": topo.as_ref().map(|t| lf!("{0} logical, {1} physical, SMT {2}, groups {3}, L3 domains {4}", t.logical_count, t.physical_count, if t.smt { L::t("on") } else { L::t("off") }, t.groups, t.llc_groups.len() as i32)).unwrap_or_default(),
        "version": AppVersion::display() + &if AppVersion::is_test() { format!(" · {}", L::t("test channel")) } else { String::new() },
        "build": file_time(&exe).map(|t| t.to_local().format("g", &Culture::current())).unwrap_or_default(),
        "program": exe.to_string_lossy(),
        "data": Store::dir_shown().to_string_lossy(),
        "log": AppLog::shown_path().to_string_lossy(),
        "settings": "HKCU\\Software\\DomainScheduler · HKLM\\SOFTWARE\\Domain OS\\Toolbox",
        "own_load": h.as_ref().map(|h| h.self_line()).unwrap_or_default(),
        "self_test": if st.is_empty() { L::t("OK").to_string() } else { st.join("; ") },
    })
}

fn wmi_list(cls: &str, prop: &str) -> Vec<String> {
    wmi::query(cls, &[prop]).iter().map(|r| r[0].str()).filter(|t| !t.is_empty()).collect()
}

fn about_wmi() -> u64 {
    task(|| {
        let g = wmi_list("Win32_VideoController", "Name").join(", ");
        let bytes: i64 = wmi_list("Win32_PhysicalMemory", "Capacity").iter().map(|x| x.parse::<i64>().unwrap_or(0)).sum();
        let mut board = wmi_list("Win32_BaseBoard", "Manufacturer");
        board.extend(wmi_list("Win32_BaseBoard", "Product"));
        json!({"gpu": g, "ram": if bytes > 0 { size_text(bytes) } else { String::new() }, "board": board.join(" ")})
    })
}

// ---- dispatch ------------------------------------------------------------------------------------------------------

pub fn get(name: &str, a: &Value) -> Option<Value> {
    Some(match name {
        "masks_stored" => masks_stored(),
        "masks_preview" => masks_preview(a),
        "reservation" => reservation(),
        "settings" => settings(),
        "update" => update(),
        "check" => check(),
        "blackbox" => blackbox(),
        "changes" => changes(),
        "about" => about(),
        _ => return None,
    })
}

pub fn do_(name: &str, a: &Value) -> Option<Value> {
    Some(match name {
        "masks_apply" => masks_apply(a),
        "process_watch" => {
            if let Some(h) = app::hub() {
                h.set_watched(&s(a, "exe"), b(a, "on"));
            }
            json!({})
        }
        "setting_toggle" => setting_toggle(a),
        "check_updates" => setting_toggle(&json!({"key": "CheckUpdates", "value": b(a, "value")})),
        "sys_presets" => {
            let v = b(a, "value");
            let _ = Settings::set_bool("SysPresets", v);
            configure();
            did(if v { L::t("Built-in presets on") } else { L::t("Built-in presets off: what they changed goes back on Revert or exit") }, Sev::Info);
            json!({})
        }
        "reapply_set" => {
            let _ = Settings::set("ReapplyMs", &s(a, "ms"));
            configure();
            did(lf!("Presets are re-applied every {0} s", app::hub().map(|h| h.reapply_ticks()).unwrap_or(10)), Sev::Info);
            json!({})
        }
        "language_set" => {
            let k = s(a, "code");
            if k != L::lang() {
                let _ = Settings::set("Language", &k);
                L::load(&k);
                did(lf!("Language: {0}", L::name_of(&k)), Sev::Info);
            }
            json!({})
        }
        "home_charts" => {
            let v = b(a, "value");
            let _ = Settings::set_bool("HomeCharts", v);
            did(on_off(L::t("Live charts on the home screen"), v), Sev::Info);
            json!({})
        }
        "watch_set" => {
            if let Some(h) = app::hub() {
                h.set_watched(&s(a, "exe"), b(a, "on"));
            }
            json!({})
        }
        "flag_set" => {
            let _ = Flags::set(&s(a, "key"), b(a, "open"));
            configure();
            json!({})
        }
        "factory_reset" => {
            app::factory_reset();
            json!({})
        }
        "update_check_now" => {
            check_now();
            json!({})
        }
        "update_install" => {
            if let Some(u) = UpdateService::instance() {
                u.install_toolbox();
            }
            json!({})
        }
        "playbook_download" => {
            if let Some(u) = UpdateService::instance() {
                u.download_playbook();
            }
            json!({})
        }
        "playbook_show" => {
            if let Some(f) = UpdateService::instance().and_then(|u| u.playbook_file()) {
                UpdateService::show_file(&f);
            }
            json!({})
        }
        "playbook_site" => {
            hardware::open_link(if L::lang() == "ru" { SITE_RU } else { SITE_EN });
            json!({})
        }
        "update_rollback" => {
            app::roll_back();
            json!({})
        }
        "tester_channel" => {
            let on = AppVersion::channel() == "test";
            let _ = Settings::set("TestBuilds", if on { "false" } else { "true" });
            did(if on { L::t("Update channel: stable releases only") } else { L::t("Update channel: test builds and stable releases") }, Sev::Info);
            check_now();
            json!({})
        }
        "tester_key_remove" => {
            let _ = Settings::reset("TesterKey");
            did(L::t("Tester key removed: stable channel"), Sev::Info);
            check_now();
            json!({})
        }
        "check_start" => check_start(),
        "check_stop" => {
            check_st(|c| c.cancel.store(true, Ordering::SeqCst));
            json!({})
        }
        "blackbox_open" => blackbox_open(),
        "tweak_toggle" => tweak_toggle(&s(a, "id")),
        "tweaks_recommended" => tweaks_recommended(),
        _ => return None,
    })
}

pub fn start(name: &str, a: &Value) -> Option<u64> {
    Some(match name {
        "reservation_apply" => task(|| {
            let err = Reservation::apply_non_game(TopoBuilder::live().as_deref());
            match err {
                None => did(L::t("Reservation written and read back: it works after a reboot"), Sev::Info),
                Some(e) => did(lf!("Reservation: {0}", e), Sev::Error),
            }
            json!({})
        }),
        "reservation_revert" => task(|| {
            match Reservation::revert() {
                None => did(L::t("Reservation removed: the original value is back"), Sev::Info),
                Some(e) => did(lf!("Reservation: {0}", e), Sev::Warn),
            }
            json!({})
        }),
        "processes_list" => processes_list(),
        "process_apply" => process_apply(a),
        "process_reset" => process_reset(a),
        "autostart_read" => task(|| json!({"exists": Autostart::task_exists()})),
        "autostart_set" => autostart_set(b(a, "value")),
        "tuner_start" => {
            let Some(h) = app::hub() else { return Some(0) };
            queued(move || {
                let msg = h.tuner_start();
                did(&msg, Sev::Info);
                json!({"msg": msg})
            })
        }
        "tuner_stop" => {
            let Some(h) = app::hub() else { return Some(0) };
            queued(move || {
                h.tuner_stop(L::t("stopped by the user"));
                json!({})
            })
        }
        "resync" => resync(),
        "tester_key_save" => tester_key_save(s(a, "key")),
        "about_wmi" => about_wmi(),
        _ => return None,
    })
}
