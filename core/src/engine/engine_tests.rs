//! Ports of the C# engine tests (EngineTests.cs and the engine parts of SafetyTests, SpecTests, AddendumTests,
//! RoadmapTests and SecurityTests).

use super::*;
use crate::base::hooks::static_hooks;
use crate::base::settings::{BackendGuard, MemorySettingsBackend, Settings};
use crate::base::time::Kind;
use crate::engine::guard::{Op, NO_RUNNING};
use crate::engine::soft_close::auto_is_system;
use crate::native::fake::{FakeNative, P};
use crate::topo::builder::LIVE_HOOK;
use crate::topo::game_mask::IRQ_HOOK;
use crate::topo::reservation::FakeReserved;
use crate::topo::synth;

struct Env {
    _g: MutexGuard<'static, ()>,
    _b: BackendGuard,
}

impl Drop for Env {
    fn drop(&mut self) {
        LIVE_HOOK.set(None);
        IRQ_HOOK.set(None);
        Reservation::set_reg(None);
    }
}

/// The test class constructor: Topo.LiveHook = X7800X3D, no IRQ pins, a fake reservation, settings in memory.
fn env() -> Env {
    let g = static_hooks();
    let t = Arc::new(synth::x7800x3d());
    LIVE_HOOK.set(Some(Arc::new(move || t.clone())));
    IRQ_HOOK.set(Some(Arc::new(Vec::new)));
    Reservation::set_reg(Some(Arc::new(FakeReserved::default())));
    Env { _g: g, _b: Settings::scoped_backend(Arc::new(MemorySettingsBackend::new())) }
}

fn t0() -> DateTime {
    DateTime::from_parts(2026, 1, 1, 12, 0, 0, Kind::Utc).unwrap()
}

fn synth2(rules: &[(u32, &str, &str, &str)]) -> Preset {
    let mut p = Preset { exe: "cs2.exe".into(), class: "above".into(), io: "normal".into(), mem: "normal".into(), eco: "off".into(), ..Default::default() };
    for (tid, ideal, prio, boost) in rules {
        p.threads.push(ThreadRule { match_: "tid".into(), pattern: tid.to_string(), ideal: ideal.to_string(), prio: prio.to_string(), boost: boost.to_string(), ..Default::default() });
    }
    p
}

fn opts(warmup: bool) -> ApplyOptions {
    ApplyOptions { do_mask: true, do_threads: true, warmup, ..Default::default() }
}

/// `Make()`: the CS2 fake, a clock, and Sleep that advances the clock and runs the game.
fn make() -> (Arc<Engine>, Arc<FakeNative>, Arc<Mutex<DateTime>>) {
    let f = Arc::new(FakeNative::cs2(500));
    let now = Arc::new(Mutex::new(t0()));
    let e = Arc::new(Engine::new(f.clone()));
    let n1 = now.clone();
    e.set_now(Arc::new(move || *lock(&n1)));
    let (n2, f2) = (now.clone(), f.clone());
    e.set_sleep(Arc::new(move |ms| {
        *lock(&n2) += TimeSpan::from_ms(ms as f64);
        f2.run(&[(100, 4_000_000_000), (101, 2_000_000_000), (103, 1_000_000_000)], ms as f64 / 1000.0);
    }));
    (e, f, now)
}

fn add_secs(now: &Mutex<DateTime>, s: f64) {
    *lock(now) += TimeSpan::from_secs(s);
}

fn logged(sev: Sev, part: &str) -> bool {
    AppLog::tail(500).iter().any(|e| e.sev == sev && e.msg.contains(part))
}

// ---- EngineTests ---------------------------------------------------------------------------------------------------

#[test]
fn eco_qos_off_in_windows_is_not_a_failure_and_is_not_retried() {
    let _e = env();
    let (e, f, now) = make();
    f.lock().eco_error = 1;
    let preset = synth2(&[(100, "2", "above", "on")]);
    let r = e.apply_auto(500, &preset, 0xFFFC, &opts(true));
    assert!(r.failures.is_empty(), "{:?}", r.failures);
    let sets = e.set_calls();
    add_secs(&now, 5.0);
    e.apply_auto(500, &preset, 0xFFFC, &opts(false));
    assert_eq!(sets, e.set_calls());
}

#[test]
fn second_apply_writes_nothing() {
    let _e = env();
    let (e, f, now) = make();
    let preset = synth2(&[(100, "2", "above", "on"), (101, "4", "above", "off"), (103, "6", "", "off")]);
    let r = e.apply_auto(500, &preset, 0xFFFC, &opts(true));
    assert!(r.failures.is_empty(), "{:?}", r.failures);
    {
        let s = f.lock();
        assert_eq!(32768, s.procs[&500].cls);
        assert_eq!(1, s.procs[&500].eco_ctl);
        assert_eq!(0xFFFC, s.procs[&500].aff);
        assert!(!s.procs[&500].sets.is_empty());
        assert_eq!(2, s.threads[&100].ideal);
        assert_eq!(4, s.threads[&101].ideal);
        assert_eq!(1, s.threads[&100].prio);
        assert!(s.threads[&101].boost_disabled);
    }
    let sets = e.set_calls();
    add_secs(&now, 5.0);
    e.apply_auto(500, &preset, 0xFFFC, &opts(false));
    assert_eq!(sets, e.set_calls());
    assert_eq!(1, f.lock().threads[&100].ideal_sets);
    // The game changes a value itself: only that is written again.
    f.lock().procs.get_mut(&500).unwrap().cls = 32;
    e.apply_auto(500, &preset, 0xFFFC, &opts(false));
    assert_eq!(sets + 1, e.set_calls());
}

#[test]
fn auto_ideal_is_never_reassigned() {
    let _e = env();
    let (e, f, _) = make();
    let preset = Preset {
        exe: "cs2.exe".into(),
        class: String::new(),
        io: String::new(),
        mem: String::new(),
        eco: String::new(),
        threads: vec![ThreadRule { match_: "module".into(), pattern: "tier0.dll".into(), ideal: "auto".into(), ..Default::default() }],
        ..Default::default()
    };
    e.apply_auto(500, &preset, 0xFFFF, &opts(false));
    let tids = [101u32, 102, 103, 104, 107];
    let lps: Vec<u32> = tids.iter().map(|t| f.lock().threads[t].ideal).collect();
    let distinct: HashSet<u32> = lps.iter().copied().collect();
    assert_eq!(lps.len(), distinct.len());
    assert!(lps.iter().all(|lp| lp % 2 == 0)); // first hardware thread of each core
    e.apply_auto(500, &preset, 0xFFFF, &opts(false));
    assert!(tids.iter().all(|t| f.lock().threads[t].ideal_sets == 1));
}

#[test]
fn tid_match_has_no_false_hits() {
    let rows = vec![
        ThreadRow { tid: 100, desc: "101".into(), sym: "101.dll+0x1".into(), ..Default::default() },
        ThreadRow { tid: 101, desc: "x".into(), ..Default::default() },
    ];
    let hits = Engine::rule_hits(&ThreadRule { match_: "tid".into(), pattern: "101".into(), ..Default::default() }, &rows);
    assert_eq!(1, hits.len());
    assert_eq!(101, hits[0].tid);
    assert!(Engine::rule_hits(&ThreadRule { match_: "tid".into(), pattern: "10*".into(), ..Default::default() }, &rows).is_empty());
    assert_eq!(1, Engine::rule_hits(&ThreadRule { match_: "module".into(), pattern: "1*".into(), ..Default::default() }, &rows).len());
}

#[test]
fn anti_cheat_threads_are_skipped() {
    let _e = env();
    let (e, f, _) = make();
    e.apply_auto(500, &synth2(&[(106, "6", "highest", "")]), 0xFFFF, &opts(false));
    assert_eq!(0, f.lock().threads[&106].prio);
    assert_eq!(999, f.lock().threads[&106].ideal);
    assert!(e.advice().iter().any(|a| a.contains("EasyAntiCheat")));
}

#[test]
fn gate_never_opens_the_process() {
    let _e = env();
    let (e, f, _) = make();
    e.set_gate_hook(Some(Arc::new(|_pid, exe: &str| if exe == "cs2.exe" { Some("cs2.exe runs under FACEIT".to_string()) } else { None })));
    let r = e.apply_auto(500, &synth2(&[(100, "2", "above", "")]), 0xFFFF, &opts(false));
    assert!(!r.failures.is_empty());
    assert!(!f.lock().opened_pids.contains(&500));
    assert_eq!(0, f.lock().writes);
}

#[test]
fn revert_is_idempotent_and_skips_reused_ids() {
    let _e = env();
    let (e, f, _) = make();
    f.lock().procs.get_mut(&500).unwrap().aff = 0xFFFF;
    e.apply_auto(500, &synth2(&[(100, "2", "above", "on"), (101, "4", "above", "off")]), 0xFFFC, &opts(true));
    f.lock().threads.get_mut(&101).unwrap().born = 9999; // thread 101 ended, its ID reused
    assert!(e.revert(500) > 0);
    {
        let s = f.lock();
        assert_eq!(32, s.procs[&500].cls);
        assert_eq!(0xFFFF, s.procs[&500].aff);
        assert!(s.procs[&500].sets.is_empty());
        assert_eq!(0, s.procs[&500].eco_ctl);
        assert_eq!(0, s.threads[&100].prio);
        assert!(s.threads[&101].boost_disabled); // not the same thread: left alone
    }
    let w = f.lock().writes;
    assert_eq!(0, e.revert(500));
    assert_eq!(0, e.revert_all());
    assert_eq!(w, f.lock().writes);
}

#[test]
fn background_only_lowers_and_reverts() {
    let _e = env();
    let (e, f, _) = make();
    {
        let mut s = f.lock();
        s.procs.insert(800, P::named("notepad.exe", 5));
        s.procs.insert(801, P { name: "idle.exe".into(), created: 6, cls: 64, io: 0, mem: 1, eco_ctl: 1, eco_state: 1, ..Default::default() });
        s.procs.insert(802, P::named("FACEITService.exe", 7));
    }
    let saved = Arc::new(AtomicI32::new(0));
    let s2 = saved.clone();
    e.set_before_mutate(Some(Arc::new(move || {
        s2.fetch_add(1, Ordering::SeqCst);
    })));
    assert_eq!(2, e.apply_background(&[800, 801, 802], 0x3, true, None)); // 801 is already low but still gets the background mask
    assert!(saved.load(Ordering::SeqCst) > 0);
    {
        let s = f.lock();
        let d = &s.procs[&800];
        assert_eq!(16384, d.cls);
        assert_eq!(1, d.io); // I/O low, never "very low"
        assert_eq!(2, d.mem);
        assert_eq!(5, d.eco_state); // EcoQoS + timer requests ignored
        assert_eq!(0x3, d.aff);
        assert_eq!(64, s.procs[&801].cls); // never raised
        assert_eq!(0x3, s.procs[&801].aff);
        assert_eq!(32, s.procs[&802].cls); // anti-cheat: never touched
    }
    assert_eq!(0, e.apply_background(&[800, 801], 0x3, true, None));
    e.revert_background();
    let s = f.lock();
    let d = &s.procs[&800];
    assert_eq!(32, d.cls);
    assert_eq!(2, d.io);
    assert_eq!(5, d.mem);
    assert_eq!(u64::MAX, d.aff);
}

#[test]
fn timer_guard_ignores_timer_requests_and_reverts() {
    let _e = env();
    let (e, f, _) = make();
    {
        let mut s = f.lock();
        s.procs.insert(810, P::named("GCC.exe", 5)); // user session: full background
        s.procs.insert(811, P::named("chrome.exe", 6)); // browser (media): EcoQoS only
        s.procs.insert(812, P { name: "CorsairDeviceControlService.exe".into(), created: 7, cls: 32, ..Default::default() }); // service: timer flag only
        s.procs.insert(813, P::named("svchost.exe", 8)); // never
    }
    e.apply_background(&[810, 811], 0, false, Some(&[812, 813]));
    {
        let s = f.lock();
        assert_eq!((5, 5), (s.procs[&810].eco_ctl, s.procs[&810].eco_state));
        assert_eq!((1, 1), (s.procs[&811].eco_ctl, s.procs[&811].eco_state));
        assert_eq!((4, 4), (s.procs[&812].eco_ctl, s.procs[&812].eco_state));
        assert_eq!(32, s.procs[&812].cls); // the service keeps its priority
        assert_eq!((0, 0), (s.procs[&813].eco_ctl, s.procs[&813].eco_state));
    }
    let w = f.lock().writes;
    e.apply_background(&[810, 811], 0, false, Some(&[812, 813]));
    assert_eq!(w, f.lock().writes); // nothing written twice
    e.revert_background();
    let s = f.lock();
    assert_eq!((0, 0), (s.procs[&810].eco_ctl, s.procs[&810].eco_state));
    assert_eq!((0, 0), (s.procs[&812].eco_ctl, s.procs[&812].eco_state));
}

#[test]
fn one_failing_process_does_not_stop_the_background_layer() {
    let _e = env();
    let (e, f, _) = make();
    AppLog::clear();
    {
        let mut s = f.lock();
        s.procs.insert(820, P { name: "odd.exe".into(), created: 5, throw_: true, ..Default::default() });
        s.procs.insert(821, P::named("GCC.exe", 6));
        s.procs.insert(822, P { name: "oddservice.exe".into(), created: 7, throw_: true, ..Default::default() });
        s.procs.insert(823, P::named("Corsair.exe", 8));
    }
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {})); // the fake's "exceptions" are expected here
    e.apply_background(&[820, 821], 0, false, Some(&[822, 823]));
    std::panic::set_hook(hook);
    {
        let s = f.lock();
        assert_eq!(16384, s.procs[&821].cls); // the others are still handled
        assert_eq!((4, 4), (s.procs[&823].eco_ctl, s.procs[&823].eco_state));
    }
    assert!(AppLog::tail(50).iter().any(|x| x.msg.contains("odd.exe") && x.msg.contains("Win32Exception")));
    e.revert_background();
    assert_eq!(32, f.lock().procs[&821].cls);
}

#[test]
fn timer_guard_off_keeps_the_old_behaviour() {
    let _e = env();
    let (e, f, _) = make();
    e.set_timer_guard(false);
    {
        let mut s = f.lock();
        s.procs.insert(810, P::named("GCC.exe", 5));
        s.procs.insert(812, P::named("Corsair.exe", 7));
    }
    e.apply_background(&[810], 0, false, Some(&[812]));
    let s = f.lock();
    assert_eq!((1, 1), (s.procs[&810].eco_ctl, s.procs[&810].eco_state));
    assert_eq!((0, 0), (s.procs[&812].eco_ctl, s.procs[&812].eco_state));
}

#[test]
fn prune_leaves_nothing_after_the_game_ends() {
    let _e = env();
    let (e, f, now) = make();
    let preset = synth2(&[(100, "2", "above", "on"), (101, "4", "above", "off")]);
    for i in 0..120 {
        add_secs(&now, 5.0);
        e.apply_auto(500, &preset, 0xFFFF, &opts(false));
        if i % 12 == 0 {
            e.prune(None);
        }
    }
    assert!(e.sizes().1 > 0);
    f.lock().procs.shift_remove(&500);
    e.prune(None);
    assert_eq!((0, 0, 0, 0), e.sizes());
}

#[test]
fn cycle_fallback_is_reported() {
    let _e = env();
    let (e, f, _) = make();
    f.lock().threads.get_mut(&103).unwrap().no_cycles = true;
    let s = e.snapshot(500);
    assert!(s.cycle_fallback_tids.contains(&103));
    assert_eq!("cs2.exe+0x100", s.rows.iter().find(|r| r.tid == 100).unwrap().sym);
}

#[test]
fn session_restores_only_matching_processes() {
    let _e = env();
    let (e, f, _) = make();
    f.lock().procs.insert(800, P::named("notepad.exe", 5));
    e.apply_auto(500, &synth2(&[(100, "2", "above", "")]), 0xFFFC, &opts(false));
    e.apply_background(&[800], 0, false, None);
    let s = e.export();
    assert_eq!(2, s.procs.len());
    assert_eq!(1, s.threads.len());
    // Crash: a new engine restores from the session; PID 800 now belongs to another process.
    f.lock().procs.get_mut(&800).unwrap().created = 999;
    let e2 = Engine::new(f.clone());
    assert!(e2.restore_session(Some(&s)) > 0);
    let st = f.lock();
    assert_eq!(32, st.procs[&500].cls);
    assert_eq!(0, st.threads[&100].prio);
    assert_eq!(16384, st.procs[&800].cls); // someone else's PID: skipped
}

// ---- SafetyTests ---------------------------------------------------------------------------------------------------

/// SafetyTests.AudioStreamingVoiceAndVrAreNeverChanged
#[test]
fn audio_streaming_voice_and_vr_are_never_changed() {
    let _e = env();
    for exe in [
        "obs64.exe", "Discord.exe", "discord.exe", "voicemeeter8x64.exe", "VoiceMeeterPro.exe", "audiodg.exe", "DolbyDAX3API.exe", "vrserver.exe",
        "OVRServer_x64.exe", "VirtualDesktop.Streamer.exe", "NVIDIA Broadcast.exe", "ts3client_win64.exe", "WaveLink.exe", "NahimicSvc64.exe",
        "RtkAudUService64.exe", "medal.exe", "nvsphelper64.exe",
    ] {
        assert!(BackgroundDeny::contains(exe), "{}", exe);
    }
    assert!(!BackgroundDeny::contains("GCC.exe"));
    assert!(!BackgroundDeny::contains("nvcontainer.exe"));
    assert!(!BackgroundDeny::contains(""));
    assert!(!BackgroundDeny::contains("voicemeeter.txt"));

    let f = Arc::new(FakeNative::cs2(500));
    let e = Engine::new(f.clone());
    e.set_now(Arc::new(t0));
    {
        let mut s = f.lock();
        s.procs.insert(900, P::named("obs64.exe", 5));
        s.procs.insert(901, P::named("voicemeeter8x64.exe", 6));
        s.procs.insert(902, P::named("NahimicSvc64.exe", 7)); // a service: the timer-only path
        s.procs.insert(903, P::named("GCC.exe", 8));
    }
    e.apply_background(&[900, 901, 903], 0x3, true, Some(&[902]));
    let s = f.lock();
    for pid in [900u32, 901, 902] {
        let p = &s.procs[&pid];
        assert_eq!(32, p.cls);
        assert_eq!(2, p.io);
        assert_eq!((0, 0), (p.eco_ctl, p.eco_state));
        assert_eq!(u64::MAX, p.aff);
    }
    assert_eq!(16384, s.procs[&903].cls);
    assert_eq!(1, s.procs[&903].io); // low, never very low
}

// ---- SpecTests -----------------------------------------------------------------------------------------------------

/// SpecTests.ManualApisRefuseDenylistedProcessesWithoutOpeningThem
#[test]
fn manual_apis_refuse_denylisted_processes_without_opening_them() {
    let _e = env();
    let f = crate::engine::sys_layer::tests::system();
    let e = Engine::new(f.clone());
    e.set_gate_hook(Some(Arc::new(|_pid, exe: &str| Guard::gate(exe, Op::Process, false, NO_RUNNING))));
    let mut log = Vec::new();
    assert!(!e.process_class_manual(700, "above", Some(&mut log)));
    assert!(!e.process_boost(700, true, Some(&mut log)));
    assert!(e.thread_manual(700, 701, "above", "", "").contains("system-critical"));
    assert!(e.read_process(700, "csrss.exe").denied);
    assert!(!f.lock().opened_pids.contains(&700));
    assert_eq!(0, f.lock().writes);
    // A thread of a denylisted module inside an allowed process is not changed either.
    e.snapshot(500);
    assert!(e.thread_manual(500, 106, "above", "", "").contains("denylisted"));
    assert_eq!(0, f.lock().threads[&106].prio);
}

// ---- AddendumTests -------------------------------------------------------------------------------------------------

/// AddendumTests.ManualApisChangeTheTargetAndAutoNeverCallsThem (the manual part; the Auto part needs AutoController)
#[test]
fn manual_apis_change_the_target() {
    let _e = env();
    let f = Arc::new(FakeNative::cs2(500));
    let e = Engine::new(f.clone());
    let mut log = Vec::new();
    assert!(e.process_class_manual(500, "above", Some(&mut log)));
    assert_eq!(32768, f.lock().procs[&500].cls);
    assert!(!e.process_class_manual(500, "realtime", Some(&mut log)));
    assert!(e.process_boost(500, true, Some(&mut log)));
    assert!(f.lock().procs[&500].boost_disabled);
    assert!(e.thread_manual(500, 100, "highest", "4", "off").contains("priority 2"));
    assert_eq!(2, f.lock().threads[&100].prio);
    assert_eq!(4, f.lock().threads[&100].ideal);
    assert!(e.revert(500) > 0);
    assert_eq!(32, f.lock().procs[&500].cls);
    assert_eq!(0, f.lock().threads[&100].prio);
    assert_eq!(4, e.manual_calls());
}

/// AddendumTests.AppLogLevels (the engine part)
#[test]
fn app_log_levels() {
    let _e = env();
    AppLog::clear();
    let f = Arc::new(FakeNative::cs2(500));
    let e = Engine::new(f.clone());
    e.set_gate_hook(Some(Arc::new(|_pid, _exe: &str| Some("cs2.exe runs under FACEIT".to_string()))));
    e.apply_auto(500, &Preset { exe: "cs2.exe".into(), ..Default::default() }, 0, &ApplyOptions::default());
    let tail = AppLog::tail(10);
    assert!(tail.iter().any(|x| x.sev == Sev::Warn && x.msg.contains("FACEIT"))); // AdviceOnce
    // report.Failures: a process that cannot be changed is skipped: a warning in the log file, never an error window
    assert!(!tail.iter().any(|x| x.sev == Sev::Error));
}

// ---- RoadmapTests --------------------------------------------------------------------------------------------------

/// RoadmapTests.HeatNoneNoneIsTheReferenceAndPinOverridesTheFirstSlot (the engine part)
#[test]
fn heat_none_none_is_the_reference_and_pin_overrides_the_first_slot() {
    let _e = env();
    let fa = Arc::new(FakeNative::cs2(500));
    let fb = Arc::new(FakeNative::cs2(500));
    let plan = vec![
        Classified { row: ThreadRow { tid: 100, born: 1000, ..Default::default() }, role: ThreadRole::Main, lp: 2, ..Default::default() },
        Classified { row: ThreadRow { tid: 101, born: 1010, ..Default::default() }, role: ThreadRole::Render, lp: 4, ..Default::default() },
        Classified { row: ThreadRow { tid: 103, born: 1030, ..Default::default() }, role: ThreadRole::Worker, lp: 6, ..Default::default() },
    ];
    let reference = AutoLogic::build_auto_preset("cs2.exe", &plan, true, "none", "none");
    let none = AutoLogic::build_auto_preset("cs2.exe", &plan, true, "none", "none");
    assert_eq!(reference, none); // Store.Same
    let o = ApplyOptions { do_mask: true, do_threads: true, ..Default::default() };
    Engine::new(fa.clone()).apply_auto(500, &reference, 0xFFFF, &o);
    Engine::new(fb.clone()).apply_auto(500, &none, 0xFFFF, &o);
    assert_eq!(fa.lock().writes, fb.lock().writes);
    assert!(fb.lock().threads.values().all(|t| t.sets_writes == 0));

    // A preset of the user's: the hottest matched thread is pinned, the next ones get an ideal processor.
    let f = Arc::new(FakeNative::cs2(500));
    let now = Arc::new(Mutex::new(t0()));
    let e = Engine::new(f.clone());
    let n1 = now.clone();
    e.set_now(Arc::new(move || *lock(&n1)));
    let (n2, f2) = (now.clone(), f.clone());
    e.set_sleep(Arc::new(move |ms| {
        *lock(&n2) += TimeSpan::from_ms(ms as f64);
        f2.run(&[(100, 4_000_000_000), (101, 2_000_000_000), (103, 900_000_000), (106, 9_000_000_000)], ms as f64 / 1000.0);
    }));
    let preset = Preset {
        exe: "cs2.exe".into(),
        class: String::new(),
        io: String::new(),
        mem: String::new(),
        eco: String::new(),
        hot_first: "pin".into(),
        hot_others: "ideal".into(),
        ..Default::default()
    };
    e.apply(500, &preset, &ApplyOptions { do_threads: true, warmup: true, ..Default::default() }, None);
    {
        let s = f.lock();
        assert_eq!(1, s.threads[&100].sets.len()); // the hottest eligible thread
        assert_ne!(999, s.threads[&101].ideal);
        assert!(s.threads[&101].sets.is_empty());
        assert_eq!(999, s.threads[&106].ideal); // the denylisted module's thread is the hottest of all and is skipped
        assert_eq!(0, s.threads[&106].sets_writes);
        assert_eq!(999, s.threads[&107].ideal); // idle: not ranked
    }

    let same = Arc::new(FakeNative::cs2(500));
    Engine::new(same.clone()).apply(
        500,
        &Preset { exe: "cs2.exe".into(), class: String::new(), io: String::new(), mem: String::new(), eco: String::new(), ..Default::default() },
        &ApplyOptions { do_threads: true, ..Default::default() },
        None,
    );
    assert_eq!(0, same.lock().writes); // none/none and no rules: nothing at all
}

/// RoadmapTests.ClosedFlagsRefuseTheirActionsAndOnlyCoreDevicesAndGpuAreOpenByDefault (the engine's timer part) and
/// TimerResolutionIsRequestedOnActiveReleasedOnRevertAndAfterACrash (the engine parts)
#[test]
fn timer_request_follows_its_flag_and_a_crashed_request_is_released() {
    let _e = env();
    AppLog::clear();
    let (e, f, _) = make();
    assert!(!e.timer_request()); // the flag is closed: refused
    assert_eq!(0, f.lock().timer_sets);
    assert!(logged(Sev::Warn, "timerres"));
    Flags::set(Flags::TIMER_RES, true).unwrap();
    assert!(Flags::open(Flags::TIMER_RES));
    assert!(e.timer_request());
    assert!(f.lock().timer_held);
    assert_eq!(5000, f.lock().timer_current); // the finest one
    assert!(e.export().timer_requested);
    assert!(e.timer_request()); // held: not asked twice
    assert_eq!(1, f.lock().timer_sets);
    assert!(e.timer_release());
    assert!(!f.lock().timer_held);
    assert!(!e.timer_release());
    Flags::set(Flags::TIMER_RES, false).unwrap();
    assert!(!e.timer_request());

    // A crashed run left the flag: the next start releases the timer before anything else.
    let f2 = Arc::new(FakeNative::cs2(500));
    let e2 = Engine::new(f2.clone());
    e2.restore_session(Some(&SessionData { clean_shutdown: false, timer_requested: true, ..Default::default() }));
    assert_eq!(1, f2.lock().timer_releases);
    e2.restore_session(Some(&SessionData { clean_shutdown: false, ..Default::default() }));
    assert_eq!(1, f2.lock().timer_releases);
}

// ---- SecurityTests -------------------------------------------------------------------------------------------------

/// SecurityTests.CrashRestoreChecksValuesNamesAndTheGuard
#[test]
fn crash_restore_checks_values_names_and_the_guard() {
    let _e = env();
    let f = Arc::new(FakeNative::cs2(500));
    {
        let mut s = f.lock();
        s.procs.insert(801, P::named("notepad.exe", 6));
        s.procs.insert(802, P::named("lsass.exe", 7));
        s.procs.insert(804, P::named("game.exe", 9));
    }
    let e = Engine::new(f.clone());
    e.set_gate_hook(Some(Arc::new(|_pid, exe: &str| {
        Guard::gate(exe, Op::Process, false, NO_RUNNING).or_else(|| if auto_is_system(exe) { Some("system".to_string()) } else { None })
    })));
    let created500 = f.lock().procs[&500].created;
    let born100 = f.lock().threads[&100].born;
    let s = SessionData {
        procs: vec![
            SessionProc { pid: 801, name: "notepad.exe".into(), created: 6, prio: 256, prio_set: true, ..Default::default() }, // realtime: never written
            SessionProc { pid: 802, name: "lsass.exe".into(), created: 7, prio: 16384, prio_set: true, ..Default::default() }, // the guard refuses it
            SessionProc { pid: 804, name: "other.exe".into(), created: 9, prio: 16384, prio_set: true, ..Default::default() }, // another program now
            SessionProc { pid: 500, name: "cs2.exe".into(), created: created500, prio: 16384, prio_set: true, ..Default::default() },
        ],
        threads: vec![SessionThread { pid: 500, tid: 100, born: born100, prio: 15, prio_set: true, ..Default::default() }], // time-critical: refused
        ..Default::default()
    };
    e.restore_session(Some(&s));
    let st = f.lock();
    assert_eq!(32, st.procs[&801].cls);
    assert_eq!(32, st.procs[&802].cls);
    assert_eq!(32, st.procs[&804].cls);
    assert_eq!(16384, st.procs[&500].cls);
    assert_ne!(15, st.threads[&100].prio);
    assert!(!Engine::session_thread_valid(&SessionThread { ideal: 4000, ideal_set: true, ..Default::default() }));
}

// ---- port checks ---------------------------------------------------------------------------------------------------

#[test]
fn wildcard_is_the_csharp_regex() {
    assert!(Engine::wildcard("*Render*").is_match("D3D11RenderThread"));
    assert!(Engine::wildcard("tier0").is_match("TIER0"));
    assert!(Engine::wildcard("a?c").is_match("abc"));
    assert!(!Engine::wildcard("a?c").is_match("a\nc"));
    assert!(Engine::wildcard("abc").is_match("abc\n")); // $ before a final line break
    assert!(!Engine::wildcard("a*").is_match("ab\nc"));
    assert!(Engine::wildcard("a.b").is_match("a.b") && !Engine::wildcard("a.b").is_match("axb"));
    assert_eq!("module", Engine::match_for(" x.DLL "));
    assert_eq!("desc", Engine::match_for("MainThrd"));
}

#[test]
fn the_reentrant_gate_lets_hooks_call_back() {
    let _e = env();
    let (e, f, _) = make();
    let e2 = Arc::downgrade(&e);
    let seen = Arc::new(AtomicI32::new(0));
    let s2 = seen.clone();
    // BeforeMutate saves the session (Export) and the gate reads the process list: both re-enter the engine
    e.set_before_mutate(Some(Arc::new(move || {
        if let Some(e) = e2.upgrade() {
            s2.fetch_add(e.export().procs.len() as i32, Ordering::SeqCst);
        }
    })));
    let e3 = Arc::downgrade(&e);
    e.set_gate_hook(Some(Arc::new(move |_pid, exe: &str| {
        let running: Vec<String> = e3.upgrade().map(|e| e.processes_now().values().map(|v| v.0.clone()).collect()).unwrap_or_default();
        Guard::gate(exe, Op::Process, false, Some(&running[..]))
    })));
    e.apply_auto(500, &synth2(&[(100, "2", "above", "on")]), 0xFFFC, &opts(false));
    assert!(seen.load(Ordering::SeqCst) > 0);
    assert_eq!(32768, f.lock().procs[&500].cls);
}
