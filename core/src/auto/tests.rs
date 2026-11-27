//! Ports of the C# tests that run the Auto controller: AutoTests (17), the Auto part of Top5Tests (3), and the Auto
//! parts of RoadmapTests (6), SecurityTests (3), SafetyTests (1), AddendumTests (2) and SpecTests (1). Same inputs,
//! same expected values.

use super::controller::*;
use super::top5::Top5;
use crate::base::app_log::{AppLog, Sev};
use crate::base::ci::{self, CiSet, CiSetExt};
use crate::base::flags::Flags;
use crate::base::hooks::{lock, static_hooks};
use crate::base::settings::{BackendGuard, MemorySettingsBackend, Settings};
use crate::base::time::{DateTime, Kind, TimeSpan};
use crate::engine::engine::Engine;
use crate::engine::guard::{Guard, Op, NO_RUNNING};
use crate::engine::session::SessionData;
use crate::engine::soft_close::SoftClose;
use crate::etw::frames::IFrameSource;
use crate::native::fake::{FakeNative, P, T};
use crate::topo::builder::LIVE_HOOK;
use crate::topo::game_mask::{IRQ_HOOK, NOISY_HOOK};
use crate::topo::masks::Masks;
use crate::topo::model::Topo;
use crate::topo::reservation::{FakeReserved, Reservation};
use crate::topo::synth;
use indexmap::IndexSet;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

// ---- FakeProbe, FakeFrames (AutoTests.cs) ---------------------------------------------------------------------------

#[derive(Default)]
pub(crate) struct ProbeState {
    pub fg: u32,
    pub windows: IndexSet<u32>,
    pub graphics: HashSet<u32>,
    pub cpu: HashMap<u32, (i64, i64)>,
    pub session: HashMap<u32, u32>,
    pub graphics_checks: i32,
    pub unreadable: HashSet<u32>,
}

#[derive(Default)]
pub(crate) struct FakeProbe(Mutex<ProbeState>);

impl FakeProbe {
    pub fn s(&self) -> MutexGuard<'_, ProbeState> {
        lock(&self.0)
    }
}

impl GameProbe for FakeProbe {
    fn self_pid(&self) -> u32 {
        1
    }
    fn foreground_pid(&self) -> u32 {
        self.s().fg
    }
    fn window_pids(&self) -> IndexSet<u32> {
        self.s().windows.clone()
    }
    fn has_graphics_module(&self, pid: u32) -> bool {
        let mut s = self.s();
        s.graphics_checks += 1;
        s.graphics.contains(&pid)
    }
    fn modules_readable(&self, pid: u32) -> bool {
        !self.s().unreadable.contains(&pid)
    }
    fn cpu_time(&self, pid: u32, created: &mut i64, cpu: &mut i64) -> bool {
        *created = 0;
        *cpu = 0;
        match self.s().cpu.get(&pid) {
            None => false,
            Some(v) => {
                *created = v.0;
                *cpu = v.1;
                true
            }
        }
    }
    fn session_of(&self, pid: u32) -> u32 {
        self.s().session.get(&pid).copied().unwrap_or(1)
    }
}

pub(crate) struct FakeFrames {
    pub available: AtomicBool,
    pub watched: AtomicU32,
    pub frame_ms: Mutex<f64>,
}

impl Default for FakeFrames {
    fn default() -> Self {
        FakeFrames { available: AtomicBool::new(true), watched: AtomicU32::new(0), frame_ms: Mutex::new(4.0) }
    }
}

impl IFrameSource for FakeFrames {
    fn available(&self) -> bool {
        self.available.load(Ordering::SeqCst)
    }
    fn name(&self) -> String {
        "fake".into()
    }
    fn error(&self) -> String {
        String::new()
    }
    fn watch(&self, pid: u32) {
        self.watched.store(pid, Ordering::SeqCst);
    }
    fn intervals(&self, from: DateTime, to: DateTime) -> Vec<f64> {
        if self.watched.load(Ordering::SeqCst) == 0 {
            return Vec::new();
        }
        let ms = *lock(&self.frame_ms);
        vec![ms; ((to - from).total_ms() / ms) as i32 as usize]
    }
}

// ---- the fixture (the C# test class constructor and Dispose) ----------------------------------------------------------

type Rates = Vec<(u32, u64)>;

const AUTO_RATES: [(u32, u64); 6] =
    [(100, 4_000_000_000), (101, 2_000_000_000), (102, 300_000_000), (103, 900_000_000), (104, 800_000_000), (105, 1_200_000_000)];

struct Env {
    _g: MutexGuard<'static, ()>,
    _b: BackendGuard,
}

impl Drop for Env {
    fn drop(&mut self) {
        LIVE_HOOK.set(None);
        IRQ_HOOK.set(None);
        NOISY_HOOK.set(None);
        Reservation::set_reg(None);
    }
}

fn set_topo(f: fn() -> Topo) {
    let t = Arc::new(f());
    LIVE_HOOK.set(Some(Arc::new(move || t.clone())));
}

/// Topo.LiveHook = X7800X3D, no IRQ pins, no noisy hook, a fake reservation, settings in memory, reads reset.
fn env() -> Env {
    let g = static_hooks();
    set_topo(synth::x7800x3d);
    IRQ_HOOK.set(Some(Arc::new(Vec::new)));
    NOISY_HOOK.set(None);
    Reservation::set_reg(Some(Arc::new(FakeReserved::default())));
    let b = Settings::scoped_backend(Arc::new(MemorySettingsBackend::new()));
    Settings::reset_reads();
    Env { _g: g, _b: b }
}

fn t0() -> DateTime {
    DateTime::from_parts(2026, 1, 1, 12, 0, 0, Kind::Utc).unwrap()
}

struct Fx {
    _env: Env,
    f: Arc<FakeNative>,
    p: Arc<FakeProbe>,
    fr: Arc<FakeFrames>,
    e: Arc<Engine>,
    c: Arc<AutoController>,
    now: Arc<Mutex<DateTime>>,
    saved: Arc<Mutex<Vec<SessionData>>>,
    rates: Arc<Mutex<Rates>>,
}

/// `Advance(ms)`: the clock, the game's cycle counters and 0.8 of a core for every probed process that still exists.
fn advance(now: &Mutex<DateTime>, f: &FakeNative, p: &FakeProbe, rates: &[(u32, u64)], ms: i32) {
    *lock(now) += TimeSpan::from_ms(ms as f64);
    f.run(rates, ms as f64 / 1000.0);
    let procs: HashSet<u32> = f.lock().procs.keys().copied().collect();
    let mut s = p.s();
    for (pid, v) in s.cpu.iter_mut() {
        if procs.contains(pid) {
            v.1 += (ms as f64 * 10_000.0 * 0.8) as i64;
        }
    }
}

struct Opt {
    frames: Option<Arc<dyn IFrameSource>>,
    gate: bool,
    autostart_empty: bool,
    watch: Option<Arc<Mutex<CiSet>>>,
}

impl Fx {
    fn with(env: Env, f: FakeNative, o: Opt) -> Fx {
        let f = Arc::new(f);
        let p = Arc::new(FakeProbe::default());
        let fr = Arc::new(FakeFrames::default());
        let now = Arc::new(Mutex::new(t0()));
        let rates = Arc::new(Mutex::new(AUTO_RATES.to_vec()));
        let e = Arc::new(Engine::new(f.clone()));
        let n1 = now.clone();
        e.set_now(Arc::new(move || *lock(&n1)));
        let (n2, f2, p2, r2) = (now.clone(), f.clone(), p.clone(), rates.clone());
        e.set_sleep(Arc::new(move |ms| {
            let r = lock(&r2).clone();
            advance(&n2, &f2, &p2, &r, ms)
        }));
        if o.gate {
            e.set_gate_hook(Some(Arc::new(|_pid, exe: &str| Guard::gate(exe, Op::Process, false, NO_RUNNING))));
        }
        let n3 = now.clone();
        let frames: Arc<dyn IFrameSource> = o.frames.unwrap_or_else(|| fr.clone());
        let c = Arc::new(AutoController::new(e.clone(), p.clone(), Some(frames), Some(Arc::new(move || *lock(&n3)))));
        let saved = Arc::new(Mutex::new(Vec::new()));
        let s2 = saved.clone();
        c.set_save_session(Some(Arc::new(move |s: &SessionData| {
            lock(&s2).push(s.clone());
            Ok(())
        })));
        if o.autostart_empty {
            c.set_autostart_line(Some(Arc::new(String::new)));
        }
        if let Some(w) = o.watch {
            c.set_watch_list(Some(Arc::new(move || lock(&w).clone())));
        }
        Fx { _env: env, f, p, fr, e, c, now, saved, rates }
    }

    /// AutoTests: FakeNative.Cs2() + GCC.exe (800), svchost.exe (801, another session), this program (1).
    fn auto() -> Fx {
        let env = env();
        let f = FakeNative::cs2(500);
        {
            let mut s = f.lock();
            s.procs.insert(800, P::named("GCC.exe", 5));
            s.procs.insert(801, P::named("svchost.exe", 6));
            s.procs.insert(1, P::named("DomainScheduler.exe", 1));
        }
        let fx = Fx::with(env, f, Opt { frames: None, gate: false, autostart_empty: false, watch: None });
        fx.p.s().session.insert(801, 0);
        fx
    }

    fn advance(&self, ms: i32) {
        let r = lock(&self.rates).clone();
        advance(&self.now, &self.f, &self.p, &r, ms);
    }

    fn seconds(&self, n: i32) {
        for _ in 0..n {
            self.advance(1000);
            self.c.tick();
        }
    }

    fn start_game(&self, pid: u32) {
        let created = self.f.lock().procs[&pid].created;
        let mut s = self.p.s();
        s.windows.insert(pid);
        s.graphics.insert(pid);
        s.cpu.insert(pid, (created, 0));
        s.fg = pid;
    }

    /// `GameToActive()` of Top5Tests and RoadmapTests.
    fn game_to_active(&self) {
        self.start_game(500);
        self.c.click();
        self.seconds(12);
        assert_eq!(AutoState::Active, self.c.state(), "{}", tail());
    }

    fn cls(&self, pid: u32) -> u32 {
        self.f.lock().procs[&pid].cls
    }

    fn last_saved(&self) -> SessionData {
        lock(&self.saved).last().cloned().expect("a saved session")
    }

    fn remove_game(&self) {
        self.f.lock().procs.shift_remove(&500);
        let mut s = self.p.s();
        s.windows.shift_remove(&500);
        s.cpu.remove(&500);
    }
}

fn tail() -> String {
    AppLog::tail(50).iter().map(|e| e.to_string()).collect::<Vec<_>>().join("\n")
}

fn logged(sev: Sev, part: &str) -> bool {
    AppLog::tail(500).iter().any(|e| e.sev == sev && e.msg.contains(part))
}

fn cs2_mods() -> Vec<crate::base::model::ModRange> {
    FakeNative::cs2(500).lock().procs[&500].mods.clone()
}

// ---- AutoTests ------------------------------------------------------------------------------------------------------

#[test]
fn click_without_game_arms_and_mutes_the_background() {
    let x = Fx::auto();
    x.c.click();
    assert_eq!(AutoState::Armed, x.c.state());
    assert_eq!("Waiting for the game…", x.c.status());
    x.seconds(12);
    assert_eq!(AutoState::Armed, x.c.state());
    assert_eq!(16384, x.cls(800));
    assert_eq!(32, x.cls(801)); // another session
    assert_eq!(32, x.cls(1)); // this program
    assert!(!lock(&x.saved).is_empty());
    assert!(!x.last_saved().clean_shutdown);
}

#[test]
fn game_is_laid_out_after_hysteresis() {
    let x = Fx::auto();
    x.c.click();
    x.start_game(500);
    x.seconds(3);
    assert_eq!(AutoState::Armed, x.c.state());
    assert_eq!("Detecting the game…", x.c.status());
    x.seconds(4);
    assert_eq!(AutoState::Active, x.c.state(), "{}", tail());
    assert_eq!("Game: cs2.exe · boost on", x.c.status());
    {
        let s = x.f.lock();
        assert_eq!(32768, s.procs[&500].cls);
        assert_eq!(1, s.procs[&500].eco_ctl);
        assert_eq!(0, s.procs[&500].eco_state);
    }
    // Warm-up gave rates to at least two threads before classification.
    assert!(x.c.warm_rates() >= 2);
    // Pins: at most cores−1, one per physical core, Main and Render apart in one domain.
    let ideals = x.e.ideals();
    let cand = Masks::ideal_candidates(&synth::x7800x3d(), true, 0xFFFF);
    assert!(ideals.len() <= cand.len() - 1);
    let distinct: HashSet<u32> = ideals.values().copied().collect();
    assert_eq!(ideals.len(), distinct.len());
    assert!(ideals.values().all(|lp| lp % 2 == 0 && *lp != 0));
    let before: Vec<(u32, i32)> = {
        let s = x.f.lock();
        let (main, render) = (s.threads[&100].ideal, s.threads[&101].ideal);
        assert_ne!(main, render);
        assert_ne!(main / 2, render / 2);
        assert_eq!(999, s.threads[&106].ideal); // anti-cheat thread
        assert_eq!(999, s.threads[&107].ideal); // cold aux thread
        s.threads.iter().map(|(k, t)| (*k, t.ideal_sets)).collect()
    };
    // The synthesized rules are numeric and the LPs do not move on later ticks.
    x.seconds(20);
    let after: Vec<(u32, i32)> = x.f.lock().threads.iter().map(|(k, t)| (*k, t.ideal_sets)).collect();
    assert_eq!(before, after);
}

#[test]
fn alt_tab_never_reverts() {
    let x = Fx::auto();
    x.c.click();
    x.start_game(500);
    x.seconds(7);
    assert_eq!(AutoState::Active, x.c.state());
    x.p.s().fg = 800;
    x.seconds(3);
    x.p.s().fg = 500;
    x.seconds(1);
    assert_eq!(AutoState::Active, x.c.state());
    x.p.s().fg = 800;
    x.seconds(20);
    assert_eq!(AutoState::Active, x.c.state()); // alt-tab no longer reverts: the changes stay while the game lives
}

#[test]
fn restarted_game_is_picked_up_without_a_click() {
    let x = Fx::auto();
    x.c.click();
    x.start_game(500);
    x.seconds(7);
    assert_eq!(AutoState::Active, x.c.state());
    x.remove_game();
    x.seconds(2);
    assert_eq!(AutoState::Armed, x.c.state());
    x.f.lock().procs.insert(500, P { name: "cs2.exe".into(), created: 99, mods: cs2_mods(), ..Default::default() });
    x.start_game(500);
    x.seconds(7);
    assert_eq!(AutoState::Active, x.c.state());
    assert_eq!(32768, x.cls(500));
}

#[test]
fn closing_game_that_loses_its_threads_first_keeps_auto_on() {
    let x = Fx::auto();
    x.c.click();
    x.start_game(500);
    x.seconds(7);
    assert_eq!(AutoState::Active, x.c.state());
    // The game is closing: its threads are gone, the process object still exists for a moment.
    x.f.lock().threads.retain(|_, t| t.pid != 500);
    x.seconds(12);
    assert_ne!(AutoState::Idle, x.c.state()); // a normal end, not a watchdog revert that switches Auto off
    x.remove_game();
    x.seconds(2);
    x.f.lock().procs.insert(500, P { name: "cs2.exe".into(), created: 99, mods: cs2_mods(), ..Default::default() });
    let fresh: Vec<(u32, T)> = FakeNative::cs2(500).lock().threads.iter().filter(|(_, t)| t.pid == 500).map(|(k, t)| (*k, t.clone())).collect();
    for (k, t) in fresh {
        x.f.lock().threads.insert(k, t);
    }
    x.start_game(500);
    x.seconds(7);
    assert_eq!(AutoState::Active, x.c.state()); // the next game is picked up without a click
}

#[test]
fn cs2_under_faceit_whose_memory_cannot_be_read_is_recognized_with_the_switch() {
    // 1.16.2: FACEIT takes the right to read cs2.exe's memory away; with the switch on, the name still counts
    let x = Fx::auto();
    x.f.lock().procs.insert(900, P::named("faceit.exe", 3));
    x.c.set_faceit_allowed(Some(Arc::new(|| true)));
    x.c.click();
    x.start_game(500);
    {
        let mut s = x.p.s();
        s.graphics.remove(&500);
        s.unreadable.insert(500);
    }
    x.seconds(7);
    assert_eq!(AutoState::Active, x.c.state(), "{}", tail());
}

#[test]
fn a_protected_game_is_left_alone_fail_closed() {
    // memory not readable and no allowed anti-cheat explains it (switch off, or no FACEIT): never touched
    let x = Fx::auto();
    x.c.set_faceit_allowed(Some(Arc::new(|| false)));
    x.c.click();
    x.start_game(500);
    {
        let mut s = x.p.s();
        s.graphics.remove(&500);
        s.unreadable.insert(500);
    }
    x.seconds(7);
    assert_ne!(AutoState::Active, x.c.state());
    assert!(x.c.protected(500)); // the engine's gate (App) refuses it from now on
    x.c.set_faceit_allowed(Some(Arc::new(|| true))); // switch on, but FACEIT is not running
    x.seconds(7);
    assert_ne!(AutoState::Active, x.c.state());
}

#[test]
fn a_known_game_readable_without_graphics_is_not_a_game() {
    let x = Fx::auto();
    x.c.click();
    x.start_game(500);
    x.p.s().graphics.remove(&500);
    x.seconds(7);
    assert_ne!(AutoState::Active, x.c.state());
}

#[test]
fn an_unknown_program_without_graphics_modules_is_not_a_game() {
    let x = Fx::auto();
    x.f.lock().procs.get_mut(&500).unwrap().name = "notagame.exe".into();
    x.c.click();
    x.start_game(500);
    x.p.s().graphics.remove(&500);
    x.seconds(7);
    assert_ne!(AutoState::Active, x.c.state());
}

#[test]
fn anti_cheat_game_is_never_opened() {
    let x = Fx::auto();
    x.f.lock().procs.insert(900, P::named("faceit.exe", 3));
    x.c.click();
    x.start_game(500);
    x.seconds(15);
    assert_eq!(AutoState::Armed, x.c.state());
    assert_eq!("Anti-cheat: the game is not changed, the background is muted", x.c.status());
    let s = x.f.lock();
    assert!(!s.opened_pids.contains(&500));
    assert!(!s.opened_pids.contains(&900));
    assert_eq!(16384, s.procs[&800].cls);
}

#[test]
fn low_confidence_sets_no_ideal_processor() {
    let x = Fx::auto();
    for t in x.f.lock().threads.values_mut() {
        t.desc = String::new();
        t.start = 0;
    }
    x.c.click();
    x.start_game(500);
    x.seconds(10);
    assert!(x.c.state() == AutoState::Active, "{}", tail());
    assert_eq!(0, x.f.lock().threads.values().map(|t| t.ideal_sets).sum::<i32>());
    assert!(x.c.status().ends_with("(basic)"));
    assert_eq!(32768, x.cls(500)); // mask + priority + EcoQoS still apply
}

#[test]
fn unsatisfiable_mask_reverts_within_one_tick() {
    let x = Fx::auto();
    set_topo(synth::x7950x3d);
    x.c.click();
    x.start_game(500);
    x.seconds(7);
    assert_eq!(AutoState::Active, x.c.state());
    assert_eq!(0xFFFF, x.f.lock().procs[&500].aff); // the V-Cache die
    x.c.set_mask_filter(Some(Arc::new(|mut mp| {
        mp.satisfied = false;
        mp.notes.push("injected".into());
        mp
    })));
    x.seconds(5); // one re-apply tick
    assert_eq!(AutoState::Idle, x.c.state());
    assert_eq!(32, x.cls(500));
    assert_eq!(u64::MAX, x.f.lock().procs[&500].aff);
    assert!(x.c.status().starts_with("Reverted:"));
}

#[test]
fn shutdown_reverts_everything() {
    let x = Fx::auto();
    x.c.click();
    x.start_game(500);
    x.seconds(7);
    x.c.shutdown();
    assert_eq!(AutoState::Idle, x.c.state());
    {
        let s = x.f.lock();
        assert_eq!(32, s.procs[&500].cls);
        assert_eq!(0, s.procs[&500].eco_ctl);
        assert_eq!(32, s.procs[&800].cls);
        assert_eq!(0, s.threads[&100].prio);
    }
    assert!(x.last_saved().clean_shutdown);
}

#[test]
fn crashed_session_is_restored_at_the_next_start() {
    let x = Fx::auto();
    x.c.click();
    x.start_game(500);
    x.seconds(7);
    let crashed = x.last_saved();
    assert!(!crashed.clean_shutdown);
    // A new process of the program: new engine and controller over the same Windows.
    let e2 = Arc::new(Engine::new(x.f.clone()));
    let n = x.now.clone();
    let c2 = AutoController::new(e2, x.p.clone(), Some(x.fr.clone()), Some(Arc::new(move || *lock(&n))));
    c2.set_save_session(Some(Arc::new(|_| Ok(()))));
    assert!(c2.recover_crashed_session(Some(&crashed)) > 0);
    assert_eq!(32, x.cls(800));
    assert_eq!(32, x.cls(500));
}

#[test]
fn auto_path_reads_no_settings_and_calls_no_banned_loaders() {
    use crate::system::devices::Devices;
    use crate::system::store::Store;
    let x = Fx::auto();
    Settings::reset_reads();
    let (st, lp, ar, res, dev) = (Store::load_calls(), Store::list_profiles_calls(), Settings::load_aff_rules_calls(), Reservation::apply_calls(), Devices::write_calls());
    x.c.click();
    x.start_game(500);
    x.seconds(30);
    x.c.shutdown();
    let product = Settings::product_keys();
    let reads = Settings::reads();
    assert!(reads.keys().all(|k| product.ci_contains(k)), "{:?}", reads);
    assert_eq!(st, Store::load_calls());
    assert_eq!(lp, Store::list_profiles_calls());
    assert_eq!(ar, Settings::load_aff_rules_calls());
    assert_eq!(res, Reservation::apply_calls());
    assert_eq!(dev, Devices::write_calls());
}

#[test]
fn worse_frame_time_never_reverts_and_no_frames_is_reported() {
    let x = Fx::auto();
    x.c.click();
    x.start_game(500);
    x.seconds(7);
    assert_eq!(AutoState::Active, x.c.state());
    *lock(&x.fr.frame_ms) = 8.0; // p99 doubled: there is no frame-time watchdog, Auto keeps its changes
    x.seconds(90);
    assert_eq!(AutoState::Active, x.c.state());
    assert!(x.c.status().contains("boost on"));
    x.fr.available.store(false, Ordering::SeqCst);
    assert!(x.c.report().contains("Frame time is not measured"));
}

#[test]
fn button_during_active_reverts_to_idle() {
    let x = Fx::auto();
    x.c.click();
    x.start_game(500);
    x.seconds(7);
    x.c.click();
    assert_eq!(AutoState::Idle, x.c.state());
    assert_eq!("Off", x.c.status());
    assert_eq!(32, x.cls(500));
}

// ---- Top5Tests (with Auto on a fake Windows) --------------------------------------------------------------------------

fn top5_fx() -> Fx {
    let env = env();
    AppLog::clear();
    let f = FakeNative::cs2(500);
    f.lock().procs.insert(1, P::named("DomainScheduler.exe", 1));
    let x = Fx::with(env, f, Opt { frames: Some(Arc::new(NoFrames::default())), gate: true, autostart_empty: true, watch: None });
    *lock(&x.rates) = vec![
        (100, 4_000_000_000),
        (101, 2_000_000_000),
        (102, 300_000_000),
        (103, 900_000_000),
        (104, 800_000_000),
        (105, 1_200_000_000),
        (106, 9_000_000_000),
    ];
    x
}

#[test]
fn dynamic_mode_places_follows_the_load_and_puts_everything_back() {
    let x = top5_fx();
    x.c.set_top5(true);
    x.game_to_active();
    x.seconds(6);
    let mut view = x.c.dynamic().view();
    assert_eq!(5, view.len());
    assert_eq!(vec![100, 101], view.iter().take(2).map(|s| s.tid).collect::<Vec<_>>());
    assert_eq!(5, view.iter().map(|s| s.lp).collect::<HashSet<_>>().len());
    assert!(!view.iter().any(|s| s.lp == 0));
    assert!(!view.iter().any(|s| s.tid == 106)); // the anti-cheat thread is the hottest of all and is never a candidate
    {
        let s = x.f.lock();
        assert_eq!(0, s.threads[&106].prio);
        assert_eq!(999, s.threads[&106].ideal);
        assert_eq!(1, s.threads[&100].prio);
        assert_eq!(1, s.threads[&101].prio);
        assert_eq!(2, s.threads[&102].prio); // audio
        assert_eq!(view.iter().find(|s| s.tid == 100).unwrap().lp, s.threads[&100].ideal);
        assert!(view.iter().any(|s| s.tid == 103));
        assert_eq!(0, s.threads[&103].prio); // a worker keeps its own priority
        assert_eq!(32768, s.procs[&500].cls); // class and mask still come from Auto
    }
    assert!(x.c.status().contains("dynamic Top-5"));

    // A steady load: no writes at all, and no thread is opened again.
    let sets = x.e.set_calls();
    let opens = x.f.lock().thread_opens;
    x.seconds(20);
    assert_eq!(sets, x.e.set_calls());
    let reopened = x.f.lock().thread_opens - opens;
    assert!(reopened <= 8, "threads were reopened: {}", reopened); // at most the half-minute refresh of the names

    // The load changes: the idle helper becomes the hottest thread and takes over.
    lock(&x.rates).push((107, 20_000_000_000));
    x.seconds(20);
    view = x.c.dynamic().view();
    assert_eq!(107, view[0].tid);
    {
        let s = x.f.lock();
        assert_eq!(1, s.threads[&107].prio);
        assert_eq!(view[0].lp, s.threads[&107].ideal);
        let out_now: Vec<u32> = [100u32, 101, 102, 103, 105].iter().copied().filter(|t| view.iter().all(|v| v.tid != *t)).collect();
        assert_eq!(1, out_now.len()); // exactly one thread lost its slot…
        assert_eq!(0, s.threads[&out_now[0]].prio); // …and got its own priority and processor back
        assert_eq!(999, s.threads[&out_now[0]].ideal);
    }
    assert!(x.c.dynamic().moves() > 5);
    assert!(x.c.report().contains("dynamic Top-5"));

    // Off: everything is as it was before the program.
    x.c.click();
    assert_eq!(AutoState::Idle, x.c.state());
    {
        let s = x.f.lock();
        for t in s.threads.values().filter(|t| t.pid == 500) {
            assert_eq!(0, t.prio);
            assert_eq!(999, t.ideal);
        }
        assert_eq!(32, s.procs[&500].cls);
    }
    assert!(!x.c.dynamic().running());
}

#[test]
fn switching_the_mode_on_an_active_game_changes_the_plan_cleanly() {
    let x = top5_fx();
    x.game_to_active();
    assert!(!x.c.top5_mode());
    assert!(!x.c.plan_view().is_empty());
    assert!(!x.c.dynamic().running());

    x.c.set_top5(true);
    assert_eq!(AutoState::Active, x.c.state());
    assert!(x.c.plan_view().is_empty());
    assert!(x.c.dynamic().running());
    x.seconds(6);
    assert_eq!(5, x.c.dynamic().view().len());

    x.c.set_top5(false);
    assert!(!x.c.dynamic().running());
    assert!(!x.c.plan_view().is_empty());
    assert_eq!(1, x.f.lock().threads[&100].prio); // the role plan is back
    x.seconds(3);
    x.c.click();
    assert!(x.f.lock().threads.values().filter(|t| t.pid == 500).all(|t| t.prio == 0));
}

#[test]
fn a_denylisted_game_is_never_started() {
    let x = top5_fx();
    let top = Top5::new(x.e.clone());
    {
        let mut s = x.f.lock();
        s.procs.insert(900, P::named("lsass.exe", 3));
        s.threads.insert(901, T { pid: 900, born: 1, ..Default::default() });
    }
    assert!(!top.start(900, "lsass.exe", Some(&synth::x7800x3d()), 0xFFFF));
    let now = *lock(&x.now);
    assert_eq!(0, top.tick(now, Some(&[901])));
    let s = x.f.lock();
    assert!(!s.opened_pids.contains(&900));
    assert_eq!(0, s.writes);
}

// ---- RoadmapTests ---------------------------------------------------------------------------------------------------

fn roadmap_fx() -> Fx {
    let env = env();
    AppLog::clear();
    let f = FakeNative::cs2(500);
    {
        let mut s = f.lock();
        s.procs.insert(800, P::named("GCC.exe", 5));
        s.procs.insert(801, P::named("notepad.exe", 6));
        s.procs.insert(802, P::named("lsass.exe", 7));
        s.procs.insert(803, P::named("dwm.exe", 8));
        s.procs.insert(1, P::named("DomainScheduler.exe", 1));
    }
    let watch = Arc::new(Mutex::new(ci::set_of(["cs2.exe"])));
    Fx::with(env, f, Opt { frames: Some(Arc::new(NoFrames::default())), gate: true, autostart_empty: true, watch: Some(watch) })
}

#[test]
fn bg_on_off_restores_every_background_baseline() {
    let x = roadmap_fx();
    assert_eq!(0, x.c.standalone_background(true, 0) - x.e.background_count());
    {
        let s = x.f.lock();
        assert_eq!(16384, s.procs[&800].cls); // lowered while the watched game runs
        assert_eq!(16384, s.procs[&801].cls);
        assert_eq!(1, s.procs[&800].io);
        assert_eq!(32, s.procs[&500].cls); // the game is not background
        assert_eq!(32, s.procs[&802].cls); // denylist
        assert_eq!(32, s.procs[&803].cls); // system
        assert!(!s.opened_pids.contains(&802));
    }
    assert!(x.e.background_count() >= 2);

    x.c.standalone_background(false, 0); // the switch goes off: BgRelease
    {
        let s = x.f.lock();
        assert_eq!(32, s.procs[&800].cls);
        assert_eq!(32, s.procs[&801].cls);
        assert_eq!(2, s.procs[&800].io);
        assert_eq!(5, s.procs[&800].mem);
        assert_eq!(0, s.procs[&800].eco_state);
    }
    assert_eq!(0, x.e.background_count());
    assert!(logged(Sev::Info, "BgRelease"));

    x.c.standalone_background(true, 0);
    assert_eq!(16384, x.cls(800));
    {
        let mut s = x.f.lock();
        s.procs.shift_remove(&500); // the game ended: released too
        s.threads.retain(|_, t| t.pid != 500);
    }
    x.c.standalone_background(true, 0);
    assert_eq!(32, x.cls(800));
}

#[test]
fn thread_cpu_sets_of_the_pin_heat_policy_have_a_baseline_a_revert_and_degrade_to_ideal_only() {
    let x = roadmap_fx();
    {
        let mut s = x.f.lock();
        s.threads.get_mut(&100).unwrap().sets = vec![7]; // what the thread had before
        s.threads.get_mut(&101).unwrap().refuse_sets = true; // Windows refuses this one
    }
    x.c.set_heat(Some(Arc::new(|_| ("pin".to_string(), "pin".to_string())))); // the game's own preset asks for hard limits
    x.game_to_active();
    let pins = x.e.thread_pins();
    assert!(pins.contains_key("500:100"));
    let t = synth::x7800x3d();
    let lp = pins["500:100"];
    {
        let s = x.f.lock();
        assert_eq!(vec![t.sets.iter().find(|s| s.lp as u32 == lp).unwrap().id], s.threads[&100].sets);
        assert_eq!(lp, s.threads[&100].ideal);
        assert_eq!(0, s.threads[&106].sets_writes); // the denylisted module's thread is never touched

        // The refused thread keeps its ideal processor; one warning; no failure, the pins stay.
        assert!(!pins.contains_key("500:101"));
        assert_ne!(999, s.threads[&101].ideal);
    }
    assert!(logged(Sev::Warn, "thread CPU Sets were refused"));
    assert!(!logged(Sev::Error, "CPU Set"));
    assert_eq!(AutoState::Active, x.c.state());
    assert!(x.last_saved().threads.iter().any(|t| t.tid == 100 && t.sets_set && t.sets == vec![7]));

    let before = x.e.set_calls();
    x.seconds(10);
    assert_eq!(before, x.e.set_calls()); // a repeated apply writes nothing, the refused one is not retried

    x.c.click(); // revert
    {
        let s = x.f.lock();
        assert_eq!(vec![7], s.threads[&100].sets);
        assert!(s.threads[&103].sets.is_empty());
    }
    assert!(x.e.thread_pins().is_empty());
}

#[test]
fn without_the_pin_heat_policy_no_thread_cpu_set_is_written() {
    let x = roadmap_fx();
    x.game_to_active();
    assert!(x.f.lock().threads.values().all(|t| t.sets_writes == 0));
    assert!(x.e.thread_pins().is_empty());
}

/// The Auto part (the engine's crash part is in engine_tests).
#[test]
fn timer_resolution_is_requested_on_active_released_on_revert_and_after_a_crash() {
    let x = roadmap_fx();
    x.c.set_timer_res(true);
    assert!(!x.e.timer_request()); // the flag is closed: refused
    assert_eq!(0, x.f.lock().timer_sets);
    assert!(logged(Sev::Warn, "timerres"));

    Flags::set(Flags::TIMER_RES, true).unwrap();
    x.game_to_active();
    {
        let s = x.f.lock();
        assert!(s.timer_held);
        assert_eq!(5000, s.timer_current); // the finest one, whatever the API calls it
        assert_eq!(1, s.timer_sets);
    }
    assert!(lock(&x.saved).iter().any(|s| s.timer_requested && !s.clean_shutdown));
    x.seconds(10);
    assert_eq!(1, x.f.lock().timer_sets);

    x.c.click();
    {
        let s = x.f.lock();
        assert!(!s.timer_held);
        assert_eq!(1, s.timer_releases);
    }
    assert!(!x.last_saved().timer_requested);
    assert!(x.last_saved().clean_shutdown);

    // A crashed run left the flag: the next start releases the timer before anything else.
    let f2 = Arc::new(FakeNative::cs2(500));
    let e2 = Engine::new(f2.clone());
    e2.restore_session(Some(&SessionData { clean_shutdown: false, timer_requested: true, ..Default::default() }));
    assert_eq!(1, f2.lock().timer_releases);
    e2.restore_session(Some(&SessionData { clean_shutdown: false, ..Default::default() }));
    assert_eq!(1, f2.lock().timer_releases);
}

#[test]
fn timer_is_released_when_the_game_ends() {
    let x = roadmap_fx();
    Flags::set(Flags::TIMER_RES, true).unwrap();
    x.c.set_timer_res(true);
    x.game_to_active();
    assert!(x.f.lock().timer_held);
    {
        let mut s = x.p.s();
        s.cpu.remove(&500);
        s.windows.shift_remove(&500);
        s.fg = 0;
    }
    x.seconds(2);
    assert_eq!(AutoState::Armed, x.c.state());
    assert!(!x.f.lock().timer_held);
}

#[test]
fn no_roadmap_function_touches_the_denylist_or_sets_realtime() {
    let x = roadmap_fx();
    Flags::set(Flags::TIMER_RES, true).unwrap();
    Flags::set(Flags::SOFT_CLOSE, true).unwrap();
    x.c.set_timer_res(true);
    x.c.set_heat(Some(Arc::new(|_| ("pin".to_string(), "pin".to_string()))));
    let soft = Arc::new(SoftClose::new());
    soft.set_on(true);
    soft.set_close_window(Arc::new(|_| Ok(true)));
    soft.set_list(ci::set_of(["lsass.exe", "notepad.exe"]));
    x.c.set_soft(Some(soft));
    x.c.standalone_background(true, 0);
    x.game_to_active();
    x.seconds(10);
    {
        let s = x.f.lock();
        assert!(!s.opened_pids.contains(&802)); // lsass: never opened by anything
        assert!(s.procs.values().all(|p| p.cls != 256)); // nobody is Realtime
        assert!(s.procs.values().all(|p| p.cls != 128));
        assert_eq!(0, s.threads[&106].sets_writes); // the denylisted module's thread
        assert_eq!(0, s.threads[&106].prio);
    }
    assert_eq!(1, x.c.soft().unwrap().asked());
    x.c.click();
    let s = x.f.lock();
    assert_eq!(32, s.procs[&500].cls);
    assert!(s.threads.values().filter(|t| t.pid == 500).all(|t| t.sets.is_empty()));
    assert!(!s.timer_held);
}

// ---- SecurityTests (CleanShutdownTests) -----------------------------------------------------------------------------

fn clean_fx(f: FakeNative, gate: bool, watch: bool) -> Fx {
    let env = env();
    AppLog::clear();
    let w = if watch { Some(Arc::new(Mutex::new(ci::set_of(["cs2.exe"])))) } else { None };
    Fx::with(env, f, Opt { frames: Some(Arc::new(NoFrames::default())), gate, autostart_empty: true, watch: w })
}

#[test]
fn a_failed_revert_keeps_its_baseline_and_the_session_is_not_clean() {
    let f = FakeNative::cs2(500);
    f.lock().procs.insert(801, P::named("notepad.exe", 6));
    f.lock().procs.insert(805, P::named("steamless.exe", 8));
    let x = clean_fx(f, false, false);
    x.e.apply_background(&[801, 805], 0, false, None);
    assert_eq!(16384, x.cls(801));
    x.f.lock().procs.get_mut(&801).unwrap().read_only = true; // Windows refuses to put it back now

    x.c.revert_everything("test");
    let last = x.last_saved();
    assert!(!last.clean_shutdown);
    assert!(last.procs.iter().any(|p| p.pid == 801));
    assert!(!last.procs.iter().any(|p| p.pid == 805)); // the other process was put back all the same
    assert_eq!(32, x.cls(805));
    assert!(x.e.pending());

    x.f.lock().procs.get_mut(&801).unwrap().read_only = false; // the next revert gets it
    x.c.revert_everything("test");
    assert_eq!(32, x.cls(801));
    assert!(x.last_saved().clean_shutdown);
    assert!(!x.e.pending());
}

#[test]
fn the_standalone_background_pass_reads_the_process_list_once() {
    let f = FakeNative::cs2(500);
    for pid in 900u32..960 {
        f.lock().procs.insert(pid, P::named(&format!("bg{}.exe", pid), pid as i64));
    }
    let x = clean_fx(f, false, true);
    let weak = Arc::downgrade(&x.e);
    x.e.set_gate_hook(Some(Arc::new(move |_pid, exe: &str| {
        let e = weak.upgrade()?;
        let names: Vec<String> = e.processes_now().values().map(|v| v.0.clone()).collect();
        Guard::gate(exe, Op::Process, false, Some(&names))
    })));
    x.f.lock().process_lists = 0;
    x.c.standalone_background(true, 0);
    assert!(x.e.background_count() >= 60);
    let lists = x.f.lock().process_lists;
    assert!(lists <= 2, "{} process lists for one pass", lists);
    x.c.standalone_background(false, 0);
}

#[test]
fn the_warmup_pause_does_not_hold_the_lock() {
    let x = clean_fx(FakeNative::cs2(500), false, true);
    let probes: Arc<Mutex<Vec<bool>>> = Arc::new(Mutex::new(Vec::new()));
    let slot: Arc<Mutex<Option<std::sync::Weak<AutoController>>>> = Arc::new(Mutex::new(None));
    let (pr, sl, n, f) = (probes.clone(), slot.clone(), x.now.clone(), x.f.clone());
    x.e.set_sleep(Arc::new(move |ms| {
        // The window reads the state under the controller's lock: it must get it during the pause.
        let c = lock(&sl).as_ref().and_then(|w| w.upgrade());
        if let Some(c) = c {
            let (tx, rx) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                let _ = tx.send(c.button_enabled());
            });
            lock(&pr).push(rx.recv_timeout(std::time::Duration::from_millis(2000)).unwrap_or(false));
        }
        *lock(&n) += TimeSpan::from_ms(ms as f64);
        f.run(&[(100, 4_000_000_000), (101, 2_000_000_000), (103, 900_000_000)], ms as f64 / 1000.0);
    }));
    *lock(&slot) = Some(Arc::downgrade(&x.c));
    let created = x.f.lock().procs[&500].created;
    {
        let mut s = x.p.s();
        s.windows.insert(500);
        s.graphics.insert(500);
        s.cpu.insert(500, (created, 0));
        s.fg = 500;
    }
    x.c.click();
    for _ in 0..12 {
        *lock(&x.now) += TimeSpan::from_secs(1.0);
        let mut s = x.p.s();
        let v = s.cpu[&500];
        s.cpu.insert(500, (created, v.1 + 8_000_000));
        drop(s);
        x.c.tick();
    }
    assert_eq!(AutoState::Active, x.c.state());
    let p = lock(&probes);
    assert!(!p.is_empty());
    assert!(p.iter().all(|b| *b));
}

// ---- SafetyTests ----------------------------------------------------------------------------------------------------

#[test]
fn launchers_and_the_shell_are_not_lowered_by_auto() {
    let f = FakeNative::cs2(500);
    {
        let mut s = f.lock();
        s.procs.insert(1, P::named("DomainScheduler.exe", 1));
        s.procs.insert(810, P::named("explorer.exe", 5));
        s.procs.insert(811, P::named("steamwebhelper.exe", 6));
        s.procs.insert(812, P::named("Discord.exe", 7));
        s.procs.insert(813, P::named("GCC.exe", 8));
    }
    let x = Fx::with(env(), f, Opt { frames: None, gate: false, autostart_empty: false, watch: None });
    let n = x.now.clone();
    x.e.set_sleep(Arc::new(move |ms| *lock(&n) += TimeSpan::from_ms(ms as f64)));
    x.c.click();
    for _ in 0..12 {
        *lock(&x.now) += TimeSpan::from_secs(1.0);
        x.c.tick();
    }
    assert_eq!(16384, x.cls(813));
    {
        let s = x.f.lock();
        for pid in [810u32, 811, 812] {
            assert_eq!(32, s.procs[&pid].cls);
            assert_eq!((0, 0), (s.procs[&pid].eco_ctl, s.procs[&pid].eco_state));
        }
    }
    x.c.click();
}

// ---- AddendumTests --------------------------------------------------------------------------------------------------

#[test]
fn self_test_problems_reach_the_report_and_auto_still_works() {
    let _env = env();
    let f = Arc::new(FakeNative::cs2(500));
    let e = Arc::new(Engine::new(f));
    let c = AutoController::new(e, Arc::new(FakeProbe::default()), Some(Arc::new(NoFrames::default())), Some(Arc::new(DateTime::utc_now)));
    c.set_save_session(Some(Arc::new(|_| Ok(()))));
    c.set_autostart_line(Some(Arc::new(|| "Autostart: task DomainScheduler (on)".to_string())));
    c.set_self_test_problems(vec!["bad.exe/x: bad prio".into()]);
    c.click();
    assert_eq!(AutoState::Armed, c.state());
    let rep = c.report();
    assert!(rep.contains("SelfTest: 1 problems"));
    assert!(rep.contains("Autostart: task DomainScheduler (on)"));
    assert!(rep.contains("Manual API: present, not active in Auto"));
    assert!(rep.contains("Frametime: not measured"));
    c.shutdown();
}

/// The Auto part (the manual part is in engine_tests): the Auto path makes zero manual calls.
#[test]
fn manual_apis_change_the_target_and_auto_never_calls_them() {
    let _env = env();
    let f2 = Arc::new(FakeNative::cs2(500));
    let e2 = Arc::new(Engine::new(f2));
    let p = Arc::new(FakeProbe::default());
    let now = Arc::new(Mutex::new(DateTime::utc_now()));
    let n = now.clone();
    let c = AutoController::new(e2.clone(), p.clone(), Some(Arc::new(NoFrames::default())), Some(Arc::new(move || *lock(&n))));
    c.set_save_session(Some(Arc::new(|_| Ok(()))));
    c.set_autostart_line(Some(Arc::new(String::new)));
    c.click();
    {
        let mut s = p.s();
        s.windows.insert(500);
        s.graphics.insert(500);
        s.fg = 500;
        s.cpu.insert(500, (77, 0));
    }
    for _ in 0..20 {
        *lock(&now) += TimeSpan::from_secs(1.0);
        {
            let mut s = p.s();
            let v = s.cpu[&500];
            s.cpu.insert(500, (77, v.1 + 8_000_000));
        }
        c.tick();
    }
    c.shutdown();
    assert_eq!(0, e2.manual_calls());
}

// ---- SpecTests ------------------------------------------------------------------------------------------------------

#[test]
fn auto_picks_only_a_game_on_the_watchlist() {
    let _env = env();
    let f = Arc::new(FakeNative::cs2(500));
    let now = Arc::new(Mutex::new(t0()));
    let p = Arc::new(FakeProbe::default());
    {
        let mut s = p.s();
        s.fg = 500;
        s.windows.insert(500);
        s.graphics.insert(500);
        s.cpu.insert(500, (77, 0));
    }
    let adv = {
        let (n, f, p) = (now.clone(), f.clone(), p.clone());
        Arc::new(move |ms: i32| {
            *lock(&n) += TimeSpan::from_ms(ms as f64);
            f.run(&[(100, 4_000_000_000), (101, 2_000_000_000), (103, 900_000_000)], ms as f64 / 1000.0);
            let mut s = p.s();
            let v = s.cpu[&500];
            s.cpu.insert(500, (77, v.1 + (ms as f64 * 10_000.0 * 0.8) as i64));
        })
    };
    let e = Arc::new(Engine::new(f.clone()));
    let n1 = now.clone();
    e.set_now(Arc::new(move || *lock(&n1)));
    let a1 = adv.clone();
    e.set_sleep(Arc::new(move |ms| a1(ms)));
    let watch = Arc::new(Mutex::new(CiSet::new()));
    let n2 = now.clone();
    let c = AutoController::new(e, p, Some(Arc::new(NoFrames::default())), Some(Arc::new(move || *lock(&n2))));
    c.set_save_session(Some(Arc::new(|_| Ok(()))));
    c.set_autostart_line(Some(Arc::new(String::new)));
    let w = watch.clone();
    c.set_watch_list(Some(Arc::new(move || lock(&w).clone())));
    c.click();
    for _ in 0..12 {
        adv(1000);
        c.tick();
    }
    assert_eq!(AutoState::Armed, c.state());
    assert!(c.status().contains("watchlist"));
    assert_eq!(32, f.lock().procs[&500].cls);
    lock(&watch).ci_add("cs2.exe");
    for _ in 0..12 {
        adv(1000);
        c.tick();
    }
    assert_eq!(AutoState::Active, c.state());
    assert_eq!(500, c.game_pid());
    assert_eq!(32768, f.lock().procs[&500].cls);
    c.shutdown();
    assert_eq!(32, f.lock().procs[&500].cls);
}

/// Not a C# test: the copy of `AutoController.IsSystem` that SoftClose keeps (engine area) is the same list.
#[test]
fn is_system_matches_the_soft_close_copy() {
    for exe in ["System", "lsass.exe", "DWM.EXE", "taskhostw.exe", "notepad.exe", "cs2.exe", ""] {
        assert_eq!(AutoController::is_system(exe), crate::engine::soft_close::auto_is_system(exe), "{}", exe);
    }
}
