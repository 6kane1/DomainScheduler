//! HomeView.cs, QuickPage.cs, GameThreadsPage.cs, SystemPanel.cs and the shared getters (topology, ranges).

use super::{did, hex, mask, queued, s, task};
use crate::auto::controller::AutoState;
use crate::auto::hud::{CoreKind, FrameStats, HudMath};
use crate::auto::tuner::AbTuner;
use crate::base::app_log::Sev;
use crate::base::fmt::{num, Culture};
use crate::base::flags::Flags;
use crate::base::hooks::lock;
use crate::base::l::L;
use crate::base::settings::Settings;
use crate::base::time::{DateTime, TimeSpan};
use crate::engine::{AutoLogic, ThreadRole};
use crate::etw::{IFrameSource, IrqAuto};
use crate::ffi::{app, hardware, state};
use crate::lf;
use crate::system::boot_guard::HomeDevices;
use crate::system::{BootGuard, Devices};
use crate::topo::game_mask::BUSY_HOOK;
use crate::topo::{GameMask, Masks, Reservation, Topo, TopoBuilder};
use indexmap::IndexMap;
use serde_json::{json, Value};
use std::sync::Mutex;

const BUCKETS: usize = 240;
const BUCKET_MS: i64 = 250;
/// ETW hands frames over in batches about every half second: the chart ends a second back so its right edge is complete.
const LAG_MS: i64 = 1000;

/// HomeView's own fields (the 3 s device cache, the processor-map masks read off the UI thread, the last stats).
pub struct Home {
    dev_done: bool,
    dev_busy: bool,
    dev_applied_utc: DateTime,
    dev_at: DateTime,
    stats: FrameStats,
    map_game: u64,
    map_irq: u64,
    map_at: DateTime,
    map_busy: bool,
    mask_key: String,
    times: Vec<DateTime>,
}

static HOME: Mutex<Home> = Mutex::new(Home {
    dev_done: false,
    dev_busy: false,
    dev_applied_utc: DateTime::MIN,
    dev_at: DateTime::MIN,
    stats: FrameStats { frames: 0, span_sec: 0.0, avg_fps: 0.0, low1_fps: 0.0, low01_fps: 0.0, frame_ms: 0.0, median_ms: 0.0, p99_ms: 0.0, hitches: 0, hitches_per_min: 0.0 },
    map_game: 0,
    map_irq: 0,
    map_at: DateTime::MIN,
    map_busy: false,
    mask_key: String::new(),
    times: Vec::new(),
});

impl Home {
    /// `_devAt = default; _mapAt = default;` after a device change.
    fn reset_dev() {
        let mut h = lock(&HOME);
        h.dev_at = DateTime::MIN;
        h.map_at = DateTime::MIN;
        h.dev_busy = false;
    }
}

fn cur() -> Culture {
    Culture::current()
}

pub fn topo_json(t: Option<&Topo>) -> Value {
    let Some(t) = t else { return Value::Null };
    let sp = t.split();
    json!({
        "logical": t.logical_count,
        "physical": t.physical_count,
        "groups": t.groups,
        "smt": t.smt,
        "name": t.name,
        "full_mask": hex(t.full_mask()),
        "first_core": t.first_core(),
        "big_cache_llc": t.big_cache_llc.map(|x| x as i32).unwrap_or(-1),
        "llc_groups": t.llc_groups,
        "l3_by_llc": t.l3_by_llc.iter().map(|(k, v)| json!({"llc": k, "bytes": v})).collect::<Vec<_>>(),
        "sets": t.sets.iter().map(|c| json!({"id": c.id, "group": c.group, "lp": c.lp, "core": c.core, "llc": c.llc, "eff": c.eff})).collect::<Vec<_>>(),
        "split": {
            "can_split": sp.can_split(),
            "label": sp.label(),
            "why": sp.why(),
            "family": sp.family,
            "upper_half": sp.upper_half,
            "game": hex(sp.game),
            "non_game": hex(sp.non_game),
        },
    })
}

// ---- home ----------------------------------------------------------------------------------------------------

fn stats_json(st: &FrameStats) -> Value {
    json!({
        "valid": st.valid(),
        "low1_fps": st.low1_fps,
        "avg_fps": st.avg_fps,
        "low01_fps": st.low01_fps,
        "frame_ms": st.frame_ms,
        "hitches_per_min": st.hitches_per_min,
        "median_ms": st.median_ms,
        "p99_ms": st.p99_ms,
    })
}

fn home() -> Value {
    let Some(h) = app::hub() else { return Value::Null };
    let auto = &h.auto;
    let st = auto.state();
    let active = st == AutoState::Active;
    let frames_ok = h.frames.as_ref().map(|f| f.available()).unwrap_or(false);
    let live_charts = Settings::get_bool_or("HomeCharts", true);
    let on = auto.on();
    let blocked = auto.blocked();
    let game_exe = auto.game_exe();

    // Auto card
    let mut sub = match st {
        AutoState::Armed => L::t("Start the game and keep it in front: the program finds it by itself.").to_string(),
        AutoState::Active => lf!("{0}: the game has the best cores, background programs are quieter. Everything comes back when the game closes.", &game_exe),
        AutoState::Faulted => L::t("Something went wrong: everything was put back. Open Advanced mode to see the activity log.").to_string(),
        AutoState::Reverting => L::t("Putting everything back as it was…").to_string(),
        _ => L::t("Turn on Auto: the program finds the game by itself, gives it the best cores and quiets background programs. Everything comes back when the game closes.").to_string(),
    };
    if !blocked.is_empty() && on {
        sub = format!("{}\n{}", sub, blocked);
    }

    // Auto-tune card
    let tuner = {
        let t = lock(&h.tuner);
        let running = t.tuner.running();
        let (idx, cnt) = (t.tuner.step_index(), t.tuner.step_count());
        let pending = t.pending.as_ref().map(|p| p.name.clone()).unwrap_or_default();
        let no_gain = t.no_gain && !running;
        let (banner, banner_text) = if t.pending.is_some() && !running {
            (1, format!("{}\n{}", t.offer, L::t("Apply the new values? The current ones stay until you press Apply.")))
        } else if no_gain {
            (2, L::t("Auto-tune finished: the current settings were the best, nothing needs to change.").to_string())
        } else {
            (0, String::new())
        };
        if running {
            let left = (((cnt - idx) * (AbTuner::PHASE_SECONDS + AbTuner::LAG_SECONDS)) as f64 / 60.0).ceil() as i32;
            json!({"running": true, "step_index": idx, "step_count": cnt, "left_min": left,
                "tag": format!("{}/{}", idx + 1, cnt),
                "sub": lf!("Step {0} of {1} · about {2} min left · keep playing", idx + 1, cnt, left),
                "enabled": true, "banner": banner, "banner_text": banner_text, "pending": pending})
        } else {
            let sub = if !frames_ok {
                L::t("frame time cannot be measured here")
            } else if active {
                L::t("about 8 minutes while you play")
            } else {
                L::t("turn on Auto and start the game")
            };
            json!({"running": false, "step_index": idx, "step_count": cnt, "left_min": 0, "tag": "", "sub": sub,
                "enabled": active && frames_ok, "banner": banner, "banner_text": banner_text, "pending": pending})
        }
    };

    // Device optimization card
    let game = auto.game_pid() != 0;
    let dev = {
        let now = DateTime::utc_now();
        let (done, busy, applied) = {
            let mut hm = lock(&HOME);
            if (now - hm.dev_at).total_seconds() >= 3.0 {
                hm.dev_at = now;
                hm.dev_done = IrqAuto::has_backup();
                hm.dev_applied_utc = if hm.dev_done { BootGuard::pending_applied_utc() } else { DateTime::MIN };
            }
            (hm.dev_done, hm.dev_busy, hm.dev_applied_utc)
        };
        let hs = BootGuard::home_state(done, game, applied, Reservation::boot_utc());
        let note = match hs {
            HomeDevices::Offer => L::t("Press once, then restart your PC — the changes apply after a restart."),
            HomeDevices::BlockedByGame => L::t("Not while a game runs: close the game first."),
            HomeDevices::AppliedRestartNeeded => L::t("Restart your PC so the changes apply."),
            _ => "",
        };
        let title = if done {
            if hs == HomeDevices::AppliedRestartNeeded { L::t("Devices optimized · restart needed") } else { L::t("Devices optimized") }
        } else {
            L::t("Optimize devices")
        };
        json!({
            "done": done, "busy": busy, "game": game, "home": hs as i32, "title": title,
            "tag": if !done && hs == HomeDevices::Offer { L::t("once") } else { "" },
            "note": note, "note_warn": hs != HomeDevices::Offer,
            "enabled": if done { true } else { !game && !busy },
            "back": done && !game,
        })
    };

    // Frame time
    let measure = active && frames_ok;
    let mut stats = FrameStats::default();
    if measure {
        if let Some(f) = &h.frames {
            let now = DateTime::utc_now();
            stats = HudMath::stats(Some(&f.intervals(now.add_seconds(-60.0), now)));
        }
    }
    lock(&HOME).stats = stats;
    let empty = if !frames_ok {
        L::t("frame time cannot be measured here")
    } else if !active {
        L::t("Start a game — its frame time appears here")
    } else if !live_charts {
        L::t("Live charts are off in the settings")
    } else {
        ""
    };

    json!({
        "state": st as i32, "on": on, "button_enabled": auto.button_enabled(), "blocked": blocked, "game_exe": game_exe,
        "active": active, "frames_ok": frames_ok, "live_charts": live_charts,
        "chart_wanted": live_charts && active && frames_ok,
        "auto_sub": sub, "auto_faulted": st == AutoState::Faulted,
        "tuner": tuner, "dev": dev, "stats": stats_json(&stats), "empty": empty,
        "map": map(&h),
        "background_count": h.engine.background_count(),
    })
}

/// HomeView.Map: the masks are read off the UI thread (the interrupt pins come from the registry): when Auto's state
/// or mask changed, otherwise every 30 s (every 5 s while a game runs: the live interrupt cores move).
fn map(h: &std::sync::Arc<crate::ffi::hub::Hub>) -> Value {
    let Some(topo) = TopoBuilder::live() else { return Value::Null };
    let mut lps: Vec<i32> = topo.sets.iter().filter(|s| s.group == 0 && s.lp < 64).map(|s| s.lp as i32).collect();
    lps.sort();
    let auto = &h.auto;
    let now_mask = auto.mask_now();
    let st = auto.state();
    let mask_key = format!("{:?}|{}", st, now_mask);
    let (game, irq) = {
        let mut hm = lock(&HOME);
        let every = if st == AutoState::Active { 5.0 } else { 30.0 };
        if !hm.map_busy && (mask_key != hm.mask_key || (DateTime::utc_now() - hm.map_at).total_seconds() >= every) {
            hm.map_busy = true;
            hm.map_at = DateTime::utc_now();
            hm.mask_key = mask_key;
            let fallback = h.game_mask_value();
            let t2 = topo.clone();
            let r = std::thread::Builder::new().name("home-map".into()).spawn(move || {
                let mut game = now_mask;
                let info = crate::ffi::hub::Hub::mask_info(&t2);
                if game == 0 {
                    game = if info.final_ != 0 { info.final_ } else { fallback };
                }
                let mut irq = info.irq_drop;
                if let Some(b) = BUSY_HOOK.get() {
                    irq |= b(&t2).unwrap_or(0);
                }
                let mut hm = lock(&HOME);
                hm.map_game = game;
                hm.map_irq = irq;
                hm.map_busy = false;
                drop(hm);
                state::bump();
            });
            if r.is_err() {
                hm.map_busy = false;
            }
        }
        (hm.map_game, hm.map_irq)
    };
    let cpu = h.cpu_load();
    let load: Vec<f64> = lps.iter().map(|&lp| if (lp as usize) < cpu.len() { cpu[lp as usize] } else { 0.0 }).collect();
    let kinds: Vec<i32> = lps
        .iter()
        .map(|&lp| match HudMath::classify(lp, game, irq) {
            CoreKind::Background => 0,
            CoreKind::Game => 1,
            CoreKind::Interrupt => 2,
        })
        .collect();
    json!({"physical": topo.physical_count, "logical": topo.logical_count, "lps": lps, "load": load, "kinds": kinds})
}

/// HomeView.ChartTick.
fn home_chart() -> Value {
    let Some(h) = app::hub() else { return Value::Null };
    let Some(f) = h.frames.clone() else { return Value::Null };
    let (v, ms) = crate::ffi::hub::Hub::time_ms(|| {
        let bucket = TimeSpan::from_ms_i(BUCKET_MS);
        let end = HudMath::align_end(DateTime::utc_now(), TimeSpan::from_ms_i(LAG_MS), bucket);
        let mut hm = lock(&HOME);
        let mut times = std::mem::take(&mut hm.times);
        f.times(end - TimeSpan::from_ticks(bucket.ticks() * BUCKETS as i64) - TimeSpan::from_secs(2.0), &mut times);
        let mut bmax = vec![0.0; BUCKETS];
        let mut bend = vec![0.0; BUCKETS];
        HudMath::buckets(Some(&times), end, bucket, &mut bmax, Some(&mut bend));
        hm.times = times;
        let st = hm.stats;
        json!({"max": bmax, "end": bend, "median_ms": if st.valid() { st.median_ms } else { 0.0 }, "p99_ms": if st.valid() { st.p99_ms } else { 0.0 }})
    });
    h.note_chart(ms);
    v
}

fn tuner_toggle() -> u64 {
    let Some(h) = app::hub() else { return 0 };
    let running = lock(&h.tuner).tuner.running();
    queued(move || {
        let msg = if running { None } else { Some(h.tuner_start()) };
        if running {
            h.tuner_stop(L::t("stopped by the user"));
        }
        if let Some(m) = &msg {
            did(m, Sev::Info);
        }
        json!({"msg": msg})
    })
}

fn devices_optimize() -> u64 {
    let Some(h) = app::hub() else { return 0 };
    {
        let mut hm = lock(&HOME);
        if hm.dev_done || hm.dev_busy || h.auto.game_pid() != 0 {
            return 0;
        }
        hm.dev_busy = true;
    }
    // Pressing the button is the consent the Devices page asks for with "Allow device policy changes".
    let _ = Flags::set(Flags::DEVICES_WRITE, true);
    task(|| {
        let lines = super::hw::apply_lines(IrqAuto::apply(&IrqAuto::plan_now()));
        GameMask::invalidate_irq();
        Home::reset_dev();
        for (message, ok) in &lines {
            did(message, if *ok { Sev::Info } else { Sev::Error });
        }
        did(L::t("Restart Windows: the new interrupt cores start working after the restart"), Sev::Info);
        json!({})
    })
}

fn devices_put_back() -> u64 {
    let Some(h) = app::hub() else { return 0 };
    {
        let mut hm = lock(&HOME);
        if hm.dev_busy || h.auto.game_pid() != 0 {
            return 0;
        }
        hm.dev_busy = true;
    }
    task(|| {
        let err = Devices::revert_all();
        GameMask::invalidate_irq();
        Home::reset_dev();
        match err {
            None => did(L::t("Device policies restored from the backup; restart Windows"), Sev::Info),
            Some(e) => did(lf!("Devices: {0}", e), Sev::Error),
        }
        json!({})
    })
}

// ---- quick (QuickPage.cs) ------------------------------------------------------------------------------------------

fn role_text(r: ThreadRole) -> &'static str {
    match r {
        ThreadRole::Main => L::t("Main"),
        ThreadRole::Render => L::t("Render"),
        ThreadRole::Audio => L::t("Audio"),
        _ => L::t("Worker"),
    }
}

fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let n = sorted.len() as i32;
    sorted[(n - 1).min(0.max((n as f64 * p).ceil() as i32 - 1)) as usize]
}

fn avg(v: impl Iterator<Item = f64>) -> f64 {
    let (mut s, mut n) = (0.0, 0);
    for x in v {
        s += x;
        n += 1;
    }
    if n == 0 { 0.0 } else { s / n as f64 }
}

fn quick() -> Value {
    let Some(h) = app::hub() else { return Value::Null };
    let auto = &h.auto;
    let flag = auto.state() == AutoState::Active;
    let m = auto.mask_now();
    let mut roles: IndexMap<u32, ThreadRole> = IndexMap::new();
    if flag {
        if auto.top5_mode() {
            for s in auto.dynamic().view() {
                roles.insert(s.lp, s.role);
            }
        } else {
            for p in auto.plan_view().into_iter().filter(|p| p.4) {
                roles.insert(p.3, p.0);
            }
        }
    }
    let load = h.cpu_load();
    let topo = TopoBuilder::live();
    let mut domains = Vec::new();
    let mut outer_cols = 0;
    if let Some(t) = &topo {
        let b0 = t.first_core();
        let mut llcs: Vec<u8> = t.sets.iter().filter(|s| s.group == 0).map(|s| s.llc).collect();
        llcs.sort();
        llcs.dedup();
        outer_cols = 2.min(1.max(llcs.len()));
        for llc in &llcs {
            let group: Vec<_> = t.sets.iter().filter(|s| s.group == 0 && s.llc == *llc).collect();
            let num3 = TopoBuilder::mask(group.iter().copied());
            let flag2 = flag && m != 0 && (num3 & m) != 0;
            let flag3 = m == 0 || (m & t.full_mask()) == t.full_mask();
            let num4 = t.l3_by_llc.get(llc).copied().unwrap_or(0);
            let caption = lf!("Cache domain {0}", *llc as i32)
                + &if !flag || flag3 { String::new() } else if flag2 { format!(" — {}", L::t("game")) } else { format!(" — {}", L::t("background")) };
            let l3_text = (if num4 != 0 { format!("{} MB L3", num4 >> 20) } else { String::new() })
                + &if t.big_cache_llc == Some(*llc) { format!(" ({})", L::t("big cache")) } else { String::new() };
            let mut cores_k: Vec<u8> = group.iter().map(|s| s.core).collect();
            cores_k.sort();
            cores_k.dedup();
            let columns = if llcs.len() == 1 { 8.min(4.max(cores_k.len())) } else { 4 };
            let mut cores = Vec::new();
            for core in cores_k {
                let mut l3: Vec<_> = group.iter().filter(|s| s.core == core).copied().collect();
                l3.sort_by_key(|s| s.lp);
                let l4: Vec<ThreadRole> = l3.iter().filter_map(|s| roles.get(&(s.lp as u32)).copied()).collect();
                let flag4 = l3.iter().any(|s| s.lp < 64 && (m >> s.lp) & 1 != 0);
                let num5 = avg(l3.iter().filter(|s| (s.lp as usize) < load.len()).map(|s| load[s.lp as usize]));
                let text2 = if l3.len() <= 1 { l3[0].lp.to_string() } else { format!("{}–{}", l3[0].lp, l3[l3.len() - 1].lp) };
                let (pill, pill_accent, text) = if !l4.is_empty() {
                    let r = *l4.iter().min_by_key(|r| **r as i32).unwrap_or(&ThreadRole::Worker);
                    (role_text(r).to_string(), (r as u32) <= 2, String::new())
                } else {
                    let t = if !flag {
                        ""
                    } else if core == b0 && (flag4 || flag3) {
                        L::t("Left to Windows")
                    } else if flag3 || flag4 {
                        L::t("Free")
                    } else {
                        L::t("Background")
                    };
                    (String::new(), false, t.to_string())
                };
                cores.push(json!({
                    "caption": lf!("Core {0} · {1}", core as i32, text2),
                    "lps": l3.iter().map(|s| s.lp).collect::<Vec<_>>(),
                    "load": num5, "pill": pill, "pill_accent": pill_accent, "text": text, "accent_line": !l4.is_empty(),
                }));
            }
            domains.push(json!({"caption": caption, "l3_text": l3_text, "accent": flag2 && !flag3, "columns": columns, "cores": cores}));
        }
    }
    // frames
    let dash = "—";
    let frames = match (&h.frames, flag) {
        (Some(f), true) if f.available() => {
            let now = DateTime::utc_now();
            let list = f.intervals(now.add_seconds(-60.0), now);
            let mut list2 = f.intervals(now.add_seconds(-10.0), now);
            list2.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            let mut list3 = list.clone();
            list3.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            let base = auto.baseline_p99();
            let applied = auto.applied_at();
            let ago = if applied == DateTime::MIN { -1.0 } else { (now - applied).total_seconds() };
            let c = cur();
            json!({
                "shown": true,
                "p99": if list2.len() >= 50 { num(percentile(&list2, 0.99), "0.0", &c) } else { dash.into() },
                "before": if base > 0.0 { num(base, "0.0", &c) } else { dash.into() },
                "low1": if list3.len() >= 200 { num(1000.0 / percentile(&list3, 0.99), "0", &c) } else { dash.into() },
                "low01": if list3.len() >= 2000 { num(1000.0 / percentile(&list3, 0.999), "0", &c) } else { dash.into() },
                "note": if list.is_empty() { L::t("The game sends no frames to measure (it is not in front, or it does not use DXGI)") } else { L::t("Last 60 seconds · lower and flatter is better") },
                "intervals": list, "baseline": base, "applied_ago": ago,
                "mark_label": L::t("Turned on here"), "before_label": L::t("worst frames before (p99)"),
            })
        }
        _ => {
            let note = match &h.frames {
                Some(f) if !f.available() => lf!("Not measured: {0}", f.error()),
                _ => L::t("Shown while a game is active").to_string(),
            };
            json!({"shown": false, "p99": dash, "before": dash, "low1": dash, "low01": dash, "note": note,
                "intervals": [], "baseline": 0.0, "applied_ago": -1.0, "mark_label": "", "before_label": ""})
        }
    };
    let bgc = h.engine.background_count();
    json!({
        "topo_ok": topo.is_some(),
        "active": flag,
        "domains_columns": outer_cols,
        "domains": domains,
        "game_text": if flag {
            format!("{}\n{}", L::t("Priority class: Above normal"), if m != 0 { lf!("Cores: processors {0}", Masks::ranges(m)) } else { L::t("Cores: not limited").to_string() })
        } else {
            L::t("No game is handled now").to_string()
        },
        "bg_text": if bgc > 0 { lf!("{0} programs: lower priority, I/O and memory priority, EcoQoS", bgc) } else { L::t("Nothing is lowered now").to_string() },
        "undo_text": L::t("Everything returns when the game closes, on Turn off, on Undo everything, or on exit"),
        "layer_text": if h.paused() {
            L::t("Presets are paused after Revert: press Reapply to resume.").to_string()
        } else {
            lf!("Presets: {0} processes changed, re-applied every {1} s.", h.engine.changed_pids().len() as i32, if auto.on() { 5 } else { h.reapply_ticks() })
        },
        "frames": frames,
    })
}

// ---- game threads (GameThreadsPage.cs) --------------------------------------------------------------------------

fn gt_role_text(r: ThreadRole) -> &'static str {
    match r {
        ThreadRole::Main => L::t("Main thread"),
        ThreadRole::Render => L::t("Render thread"),
        ThreadRole::Audio => L::t("Audio thread"),
        ThreadRole::Worker => L::t("Worker"),
        _ => L::t("Other thread"),
    }
}

fn gt_prio_text(p: Option<i32>) -> String {
    match p {
        Some(2) => L::t("Highest").to_string(),
        Some(1) => L::t("Above normal").to_string(),
        Some(0) => L::t("Normal").to_string(),
        None => L::t("Unchanged").to_string(),
        Some(v) => crate::base::fmt::int(v as i64, "+0;-0", &Culture::invariant()),
    }
}

fn mask_on() -> bool {
    !AutoLogic::keeps_all_cores(TopoBuilder::live().as_deref(), &app::mask_setting())
}

fn game_threads() -> Value {
    let Some(h) = app::hub() else { return Value::Null };
    let auto = &h.auto;
    let top5 = auto.top5_mode();
    let st = auto.state();
    let blocked = auto.blocked();
    let exe = auto.game_exe();
    let on = mask_on();
    let dynv = auto.dynamic();
    let mut key = format!("{}|{:?}|{}|{}|{}|{}|", L::lang(), st, top5, exe, blocked, num(auto.confidence(), "0.00", &Culture::invariant()));
    if st == AutoState::Active {
        if top5 {
            let v: Vec<String> = dynv.view().iter().map(|s| format!("{},{},{},{:?},{:.0}", s.index, s.tid, s.lp, s.prio, s.share * 100.0)).collect();
            key += &format!("{}|{}|{}", dynv.moves(), dynv.threads(), v.join(";"));
        } else {
            let v: Vec<String> = auto.plan_view().iter().map(|p| format!("{},{},{}", p.1, p.3, p.4)).collect();
            key += &v.join(";");
        }
    }
    let how = if top5 {
        L::t("The five busiest threads are re-checked every second and moved between cores and priorities when the load changes.")
    } else {
        L::t("The threads are recognized by their role (main, render, audio, workers) and placed once.")
    };
    let mut out = json!({
        "key": key, "top5_mode": top5, "how": how,
        "mask_on": on,
        "mask_button": if on { L::t("Core mask: on") } else { L::t("Core mask: off (every core to the game)") },
        "mask_tip": if on { L::t("Click to give the game every core") } else { L::t("Click to keep a few cores for devices and the background (only for processors with more than 8 cores: Intel with E-cores or Ryzen with two CCDs)") },
        "slot_header": if top5 { L::t("Slot") } else { L::t("No.") },
        "share_column": top5,
    });
    let o = out.as_object_mut().expect("object");
    if st != AutoState::Active {
        o.insert("sub".into(), json!(L::t("Fully automatic. Nothing on this page needs to be set.")));
        let empty = if st != AutoState::Armed {
            L::t("Press Turn on (left) and start the game. The threads will appear here.").to_string()
        } else if !blocked.is_empty() {
            blocked.clone()
        } else {
            L::t("Waiting for the game: start it and keep it in front for a few seconds.").to_string()
        };
        o.insert("empty".into(), json!(empty));
        o.insert("empty_warn".into(), json!(st == AutoState::Armed && !blocked.is_empty()));
        o.insert("rows".into(), json!([]));
        o.insert("moves".into(), json!(L::t("No game is handled now.")));
        o.insert("log".into(), json!([]));
        o.insert("log_empty".into(), json!(L::t("No moves yet.")));
        return out;
    }
    o.insert("sub".into(), json!(lf!("Fully automatic · {0}", &exe)));
    o.insert("empty_warn".into(), json!(false));
    if top5 {
        let list = dynv.view();
        let rows: Vec<Value> = list
            .iter()
            .map(|s| {
                json!({"n": s.index + 1, "lead": s.index < 2, "name": gt_role_text(s.role), "detail": format!("{} · {}", s.name, s.tid),
                    "share": s.share, "core": format!("CPU {}", s.lp), "prio": gt_prio_text(s.prio), "prio_dim": s.prio.is_none()})
            })
            .collect();
        o.insert("empty".into(), json!(if rows.is_empty() { L::t("Measuring the threads…") } else { "" }));
        o.insert("rows".into(), Value::Array(rows));
        o.insert("moves".into(), json!(lf!("{0} threads watched, {1} moves since the game started. No thread moves twice within 2 seconds.", dynv.threads(), dynv.moves())));
        let log = dynv.log();
        o.insert("log_empty".into(), json!(if log.is_empty() { L::t("No moves yet.") } else { "" }));
        o.insert("log".into(), json!(log));
        return out;
    }
    let list2 = auto.plan_view();
    let rows: Vec<Value> = list2
        .iter()
        .enumerate()
        .map(|(n, p)| {
            let lead = (p.0 as u32) <= 2;
            json!({"n": n + 1, "lead": lead, "name": gt_role_text(p.0), "detail": format!("{} · {}", p.2, p.1), "share": Value::Null,
                "core": if p.4 { format!("CPU {}", p.3) } else { "—".to_string() },
                "prio": if lead { L::t("Above normal") } else { L::t("Unchanged") }, "prio_dim": !lead})
        })
        .collect();
    o.insert("empty".into(), json!(if rows.is_empty() { L::t("Measuring the threads…") } else { "" }));
    o.insert(
        "moves".into(),
        json!(if list2.is_empty() {
            String::new()
        } else if list2[0].4 {
            lf!("{0} threads placed, confidence {1:0.00}.", list2.len() as i32, auto.confidence())
        } else {
            lf!("The threads are not recognized well enough (confidence {0:0.00}): only priorities and the mask are set. The dynamic mode does not need thread names.", auto.confidence())
        }),
    );
    o.insert("rows".into(), Value::Array(rows));
    o.insert("log".into(), json!([L::t("The role plan places the threads once and keeps them.")]));
    o.insert("log_empty".into(), json!(""));
    out
}

/// The mask button: it flips the mask whatever the rule says: back to the rule ("auto") or against it.
fn mask_toggle() -> Value {
    let t = TopoBuilder::live();
    let rule_mask = !AutoLogic::keeps_all_cores(t.as_deref(), "auto");
    let on = mask_on();
    let v = if on { if rule_mask { "off" } else { "auto" } } else if rule_mask { "auto" } else { "on" };
    let _ = Settings::set("AutoMask", v);
    let _ = Settings::set_bool("AutoNoMask", false);
    let msg = if on { L::t("Core mask: off, the game gets every core") } else { L::t("Core mask: on, a few cores are kept for devices and the background") };
    did(msg, Sev::Info);
    json!({"msg": msg})
}

// ---- system panel (SystemPanel.cs) ------------------------------------------------------------------------------

/// "Windows 11 Pro 26H2 · 26300.9974" from the registry (ProductName still says Windows 10 on 11).
pub fn windows_text() -> String {
    #[cfg(windows)]
    {
        use crate::native::reg::{Hive, Key};
        let Some(k) = Key::open(Hive::LocalMachine, "SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion", false) else { return String::new() };
        let str_of = |n: &str| k.get_string(n).unwrap_or_default();
        let ed = str_of("EditionID");
        let edition = match ed.as_str() {
            "Professional" => "Pro".to_string(),
            "ProfessionalN" => "Pro N".to_string(),
            "ProfessionalWorkstation" => "Pro for Workstations".to_string(),
            "Core" | "CoreSingleLanguage" => "Home".to_string(),
            e => e.to_string(),
        };
        let ver = str_of("DisplayVersion");
        let build = str_of("CurrentBuild");
        let ubr = k.get("UBR").map(|v| v.to_csharp_string());
        let bn: i32 = build.trim().parse().unwrap_or(0);
        let name = if bn >= 22000 { "Windows 11" } else { "Windows 10" };
        let mut out = format!("{} {} {}", name, edition, ver).trim().to_string();
        if !build.is_empty() {
            out += &format!(" · {}{}", build, ubr.map(|u| format!(".{}", u)).unwrap_or_default());
        }
        out
    }
    #[cfg(not(windows))]
    String::new()
}

pub fn game_mode() -> bool {
    #[cfg(windows)]
    {
        use crate::native::reg::{Hive, Key};
        let Some(k) = Key::open(Hive::CurrentUser, "Software\\Microsoft\\GameBar", false) else { return true };
        match k.get_dword("AutoGameModeEnabled") {
            Some(v) => v != 0, // not set = Windows' default, which is on
            None => k.get("AutoGameModeEnabled").map(|v| v.to_csharp_string().trim().parse::<i64>().map(|x| x != 0).unwrap_or(true)).unwrap_or(true),
        }
    }
    #[cfg(not(windows))]
    true
}

fn system() -> Value {
    let f = app::app().and_then(|a| a.facts.read().unwrap_or_else(|e| e.into_inner()).clone());
    json!({"windows": windows_text(), "game_mode": game_mode(), "facts": f.map(|f| serde_json::to_value(f).unwrap_or(Value::Null))})
}

// ---- dispatch ------------------------------------------------------------------------------------------------------

pub fn get(name: &str, a: &Value) -> Option<Value> {
    Some(match name {
        "topo" => topo_json(TopoBuilder::live().as_deref()),
        "ranges" => json!({"text": Masks::ranges(mask(a, "mask"))}),
        "eligibility" => json!({"text": app::hub().and_then(|h| h.eligibility(&s(a, "exe")))}),
        "system" => system(),
        "home" => home(),
        "home_chart" => home_chart(),
        "quick" => quick(),
        "game_threads" => game_threads(),
        _ => return None,
    })
}

pub fn do_(name: &str, a: &Value) -> Option<Value> {
    Some(match name {
        "configure" => {
            if let Some(h) = app::hub() {
                h.configure();
            }
            json!({})
        }
        "open_link" => {
            hardware::open_link(&s(a, "url"));
            json!({})
        }
        "tuner_dismiss" => {
            if let Some(h) = app::hub() {
                h.tuner_dismiss();
            }
            json!({})
        }
        "mask_toggle" => mask_toggle(),
        // HomeView's constructor: SelfThreads.Register(L.N("window")) on the UI thread (pages A, appended).
        "self_thread_window" => {
            crate::native::self_threads::SelfThreads::register(L::n("window"));
            json!({})
        }
        _ => return None,
    })
}

pub fn start(name: &str, _a: &Value) -> Option<u64> {
    Some(match name {
        "tuner_toggle" => tuner_toggle(),
        "tuner_apply" => {
            let Some(h) = app::hub() else { return Some(0) };
            queued(move || {
                h.tuner_apply_pending();
                json!({})
            })
        }
        "devices_optimize" => devices_optimize(),
        "devices_put_back" => devices_put_back(),
        _ => return None,
    })
}

