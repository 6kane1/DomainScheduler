//! Tests of the ffi area. The C# project has no tests of Hub.cs, App.xaml.cs, AutoCommand.cs, UpdateService.cs,
//! Optimizer.cs or Hardware.cs (they live in the WPF project); LatencyDefaultsTests reads App.xaml.cs and the sources,
//! which is ported as a check of this area's sources. The rest are new tests of the ported behaviour.

use super::auto_command::AutoCommand;
use super::hub::Hub;
use super::optimizer::Optimizer;
use super::pages;
use super::wmi::WVal;
use super::{app, hardware, state, win};
use crate::auto::controller::{AutoController, GameProbe};
use crate::base::hooks::static_hooks;
use crate::base::settings::{MemorySettingsBackend, Settings};
use crate::engine::{Engine, SysLayer};
use crate::native::fake::FakeNative;
use indexmap::IndexSet;
use serde_json::json;
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::{Arc, Mutex};

struct NoProbe;

impl GameProbe for NoProbe {
    fn self_pid(&self) -> u32 {
        1
    }
    fn foreground_pid(&self) -> u32 {
        0
    }
    fn window_pids(&self) -> IndexSet<u32> {
        IndexSet::new()
    }
    fn has_graphics_module(&self, _pid: u32) -> bool {
        false
    }
    fn cpu_time(&self, _pid: u32, _c: &mut i64, _u: &mut i64) -> bool {
        false
    }
    fn session_of(&self, _pid: u32) -> u32 {
        1
    }
}

fn hub_with(f: Arc<FakeNative>) -> Arc<Hub> {
    let engine = Arc::new(Engine::new(f));
    let auto = Arc::new(AutoController::new(engine.clone(), Arc::new(NoProbe), None, None));
    auto.set_save_session(None);
    let sys = Arc::new(SysLayer::new(engine.clone()));
    Hub::new(engine, auto, sys, None)
}

/// The state folder of a test: a scratch folder (never the real one), removed at the end.
struct TempStore(std::path::PathBuf);

impl TempStore {
    fn new(tag: &str) -> TempStore {
        let d = std::env::temp_dir().join(format!("dos-ffi-test-{}-{}", tag, std::process::id()));
        let _ = std::fs::create_dir_all(&d);
        let d2 = d.clone();
        crate::base::state_dir::STORE_DIR_HOOK.set(Some(Arc::new(move || d2.clone())));
        TempStore(d)
    }
}

impl Drop for TempStore {
    fn drop(&mut self) {
        crate::base::state_dir::STORE_DIR_HOOK.set(None);
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn src(name: &str) -> String {
    std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/ffi").join(name)).unwrap()
}

// ---- LatencyDefaultsTests (the parts about App.xaml.cs and the sources) ------------------------------------------

#[test]
fn the_latency_code_is_gone_but_the_cleanup_stays() {
    let app_src = src("app.rs");
    assert!(app_src.contains("GamePower::restore()"));
    assert!(app_src.contains("DefenderGameExclusion::remove_all()"));
    let all = ["app.rs", "hub.rs", "optimizer.rs", "pages/home.rs", "pages/hw.rs", "pages/other.rs", "pages/mod.rs"].iter().map(|f| src(f)).collect::<String>();
    assert!(!all.contains("LatencyOptions"));
    assert!(!all.contains("GamePower::apply"));
    assert!(!all.contains("PowerTick"));
}

#[test]
fn the_latency_experiments_have_no_setting_and_no_toggle() {
    let all = ["app.rs", "hub.rs", "optimizer.rs", "pages/home.rs", "pages/hw.rs", "pages/other.rs"].iter().map(|f| src(f)).collect::<String>();
    for key in ["GameNoIdle", "SoftBackground", "HighBoost", "GameNoParking", "FrameWatchdog", "ThreadSetsOn", "Warmup\"", "DomainEngine"] {
        assert!(!all.contains(&format!("\"{}\"", key.trim_end_matches('"'))), "{}", key);
    }
}

// ---- Hub ---------------------------------------------------------------------------------------------------------

#[test]
fn mask_spec_text_and_edits_come_from_the_settings() {
    let _g = static_hooks();
    let _b = Settings::scoped_backend(Arc::new(MemorySettingsBackend::new()));
    Settings::set("MaskName", "game").unwrap();
    assert_eq!(Hub::mask_spec_text(), "game");
    Settings::set_bool("MaskNoSmt", true).unwrap();
    Settings::set_bool("MaskNoCore0", true).unwrap();
    assert_eq!(Hub::mask_spec_text(), "game+nosmt+nocore0");
    Settings::set("MaskAdd", "F0").unwrap();
    Settings::set("MaskDrop", "zz").unwrap();
    Settings::set("MaskSig", "sig").unwrap();
    let e = Hub::mask_edits();
    assert_eq!((e.add, e.drop, e.sig.as_str()), (0xF0, 0, "sig"));
}

#[test]
fn configure_reads_the_switches_and_clamps_the_reapply_time() {
    let _g = static_hooks();
    let _b = Settings::scoped_backend(Arc::new(MemorySettingsBackend::new()));
    let _s = TempStore::new("configure");
    let h = hub_with(Arc::new(FakeNative::new()));
    Settings::set("ReapplyMs", "500").unwrap();
    Settings::set_bool("BgOn", true).unwrap();
    Settings::set_bool("BlackBox", true).unwrap();
    Settings::set_bool("LiveIrq", false).unwrap();
    Settings::set_watch(&["cs2.exe", "CS2.EXE", "game.exe"]).unwrap();
    h.configure();
    assert_eq!(h.reapply_ticks(), 1);
    assert!(h.bg_on());
    assert!(h.black_box_on());
    assert!(!h.live_irq_on());
    assert!(h.probe_on());
    assert_eq!(h.watch().len(), 2);
    Settings::set("ReapplyMs", "abc").unwrap();
    h.configure();
    assert_eq!(h.reapply_ticks(), 10);
    Settings::set("ReapplyMs", "5000000").unwrap();
    h.configure();
    assert_eq!(h.reapply_ticks(), 1000);
}

#[test]
fn set_watched_adds_and_removes_ignoring_case() {
    let _g = static_hooks();
    let _b = Settings::scoped_backend(Arc::new(MemorySettingsBackend::new()));
    let _s = TempStore::new("watch");
    let h = hub_with(Arc::new(FakeNative::new()));
    h.set_watched("Game.exe", true);
    h.set_watched("other.exe", true);
    h.set_watched("GAME.EXE", false);
    let w = Settings::watch();
    assert!(w.iter().any(|x| x == "other.exe"));
    assert!(!w.iter().any(|x| x.eq_ignore_ascii_case("game.exe")));
    assert_eq!(h.watch().len(), w.len());
}

#[test]
fn sample_cpu_gives_the_busy_share_per_processor() {
    let f = Arc::new(FakeNative::new());
    let h = hub_with(f.clone());
    {
        let mut s = f.lock();
        s.cpu_idle = vec![0, 0];
        s.cpu_total = vec![0, 0];
    }
    h.sample_cpu();
    assert!(h.cpu_load().is_empty());
    {
        let mut s = f.lock();
        s.cpu_idle = vec![75, 100];
        s.cpu_total = vec![100, 100];
    }
    h.sample_cpu();
    assert_eq!(*h.cpu_load(), vec![0.25, 0.0]);
}

#[test]
fn self_line_before_the_first_minute() {
    let h = hub_with(Arc::new(FakeNative::new()));
    assert_eq!(h.self_line(), "Own load: measured after the first minute");
}

// ---- Optimizer ---------------------------------------------------------------------------------------------------

#[test]
fn optimizer_runs_jobs_in_order_and_run_now_waits() {
    let opt = Optimizer::new(std::sync::Weak::new());
    // Before Start the worker is not alive: RunNow runs the job inline.
    let n = Arc::new(AtomicI32::new(0));
    let n2 = n.clone();
    assert!(opt.run_now(Box::new(move || {
        n2.fetch_add(1, Ordering::SeqCst);
    }), 1000));
    assert_eq!(n.load(Ordering::SeqCst), 1);
    opt.start();
    let order = Arc::new(Mutex::new(Vec::new()));
    for k in 0..5 {
        let o = order.clone();
        opt.queue(Box::new(move || o.lock().unwrap().push(k)), false, false);
    }
    let o = order.clone();
    let got = Arc::new(Mutex::new(None));
    let g2 = got.clone();
    opt.run(move || Ok(o.lock().unwrap().len()), move |r: Option<usize>| *g2.lock().unwrap() = r);
    assert!(opt.run_now(Box::new(|| {}), 5000));
    assert_eq!(*order.lock().unwrap(), vec![0, 1, 2, 3, 4]);
    assert_eq!(*got.lock().unwrap(), Some(5));
    // An error of Run's work gives None to done.
    let g3 = got.clone();
    opt.run(|| Err::<usize, String>("boom".into()), move |r| *g3.lock().unwrap() = r);
    assert!(opt.run_now(Box::new(|| {}), 5000));
    assert_eq!(*got.lock().unwrap(), None);
    opt.dispose();
    // After Dispose no job is taken; RunNow runs inline.
    let n3 = n.clone();
    opt.queue(Box::new(move || {
        n3.fetch_add(10, Ordering::SeqCst);
    }), false, false);
    assert_eq!(n.load(Ordering::SeqCst), 1);
}

#[test]
fn run_now_times_out_when_the_worker_is_busy() {
    let opt = Optimizer::new(std::sync::Weak::new());
    opt.start();
    opt.queue(Box::new(|| std::thread::sleep(std::time::Duration::from_millis(400))), false, false);
    assert!(!opt.run_now(Box::new(|| {}), 50));
    opt.dispose();
}

// ---- AutoCommand -------------------------------------------------------------------------------------------------

#[test]
fn auto_command_exit_codes() {
    let _g = static_hooks();
    assert_eq!(AutoCommand::send(Some("maybe")), 4);
    let on = Arc::new(Mutex::new(false));
    let (o1, o2, o3) = (on.clone(), on.clone(), on.clone());
    let cmd = AutoCommand::new(
        Arc::new(move || *o1.lock().unwrap()),
        Arc::new(move || *o2.lock().unwrap() = true),
        Arc::new(move || *o3.lock().unwrap() = false),
    );
    assert_eq!(AutoCommand::send(Some(" STATUS ")), 1);
    assert_eq!(AutoCommand::send(Some("on")), 0);
    assert!(*on.lock().unwrap());
    assert_eq!(AutoCommand::send(None), 0);
    assert_eq!(AutoCommand::send(Some("off")), 1);
    assert!(!*on.lock().unwrap());
    cmd.mirror();
}

// ---- App.xaml.cs -------------------------------------------------------------------------------------------------

#[test]
fn auto_mask_switch_sets_the_setting() {
    let _g = static_hooks();
    let _b = Settings::scoped_backend(Arc::new(MemorySettingsBackend::new()));
    Settings::set_bool("AutoNoMask", true).unwrap();
    assert_eq!(app::mask_setting(), "off");
    let a = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    assert_eq!(app::early_switches(&a(&["--auto-mask", "ON"])), Some(0));
    assert_eq!(Settings::get("AutoMask"), "on");
    assert!(!Settings::get_bool("AutoNoMask"));
    assert_eq!(app::mask_setting(), "on");
    assert_eq!(app::early_switches(&a(&["--auto-mask", "never"])), Some(4));
    assert_eq!(app::early_switches(&a(&["--auto-mask"])), Some(4));
    assert_eq!(app::early_switches(&a(&["--autostart"])), None);
}

#[test]
fn import_session_keeps_only_valid_entries_of_a_crashed_session() {
    let mut dropped = Vec::new();
    assert_eq!(app::import_session("{}", Some("x"), &mut dropped).unwrap(), None);
    assert_eq!(app::import_session(r#"{"CleanShutdown":true}"#, None, &mut dropped).unwrap(), None);
    assert!(app::import_session("not json", None, &mut dropped).is_err());
    let r = app::import_session(r#"{"CleanShutdown":false,"Procs":[],"Threads":[]}"#, None, &mut dropped).unwrap();
    assert!(r.unwrap().contains("\"CleanShutdown\":false"));
    assert!(dropped.is_empty());
}

// ---- Hardware ----------------------------------------------------------------------------------------------------

#[test]
fn hardware_facts_from_wmi_rows() {
    let q = |cls: &str, _p: &[&str]| -> Vec<Vec<WVal>> {
        let s = |x: &str| WVal::S(x.into());
        match cls {
            "Win32_Processor" => vec![vec![s("  AMD Ryzen 7 7800X3D  "), s("AuthenticAMD")]],
            "Win32_BaseBoard" => vec![vec![s("Micro-Star International Co., Ltd."), s("MAG B650")]],
            "Win32_ComputerSystem" => vec![vec![s("System manufacturer"), s("System Product Name")]],
            "Win32_VideoController" => vec![
                vec![s("NVIDIA GeForce RTX 5060"), s("NVIDIA"), s("PCI\\VEN_10DE"), s("32.0.15.9"), s("20261101000000.000000-000"), WVal::I(240)],
                vec![s("Microsoft Basic Display Adapter"), s("(Standard display types)"), s("ROOT"), s("10.0"), s("2006"), WVal::Null],
                vec![s("Virtual Display"), s(""), s(""), s(""), s(""), WVal::I(60)],
            ],
            "Win32_PhysicalMemory" => vec![vec![WVal::I(6000), WVal::I(4800)], vec![WVal::I(6000), WVal::I(5200)]],
            "Win32_SoundDevice" => vec![vec![s("Realtek Audio")], vec![s("Virtual Audio")]],
            _ if cls.starts_with("Win32_NetworkAdapter") => vec![vec![s("Realtek 2.5GbE")], vec![s("Bluetooth Device")]],
            _ if cls.starts_with("Win32_PnPEntity") => vec![vec![s(""), s("PCI\\X")]],
            _ => Vec::new(),
        }
    };
    let f = hardware::detect_with(&q);
    assert_eq!(f.cpu, "AMD Ryzen 7 7800X3D");
    assert_eq!(f.cpu_vendor, "amd");
    assert_eq!(f.board, "Micro-Star International Co., Ltd. MAG B650");
    assert_eq!(f.board_vendor, "msi");
    assert_eq!(f.gpus.len(), 2);
    assert_eq!(f.gpus[0].vendor, "nvidia");
    assert_eq!(f.gpus[0].date, "2026-11-01");
    assert!(f.gpus[1].basic);
    assert_eq!(f.gpus[1].date, "");
    assert_eq!(f.main_gpu().unwrap().name, "NVIDIA GeForce RTX 5060");
    assert_eq!(f.refresh, 240);
    assert_eq!((f.ram_rated, f.ram_now), (6000, 4800));
    assert_eq!(f.audio, vec!["Realtek Audio".to_string()]);
    assert_eq!(f.net, vec!["Realtek 2.5GbE".to_string()]);
    assert_eq!(f.problems, vec!["PCI\\X".to_string()]);
}

#[test]
fn hardware_links_are_official_https_pages() {
    for v in ["asus", "msi", "gigabyte", "asrock", "biostar", "lenovo", "hp", "dell", "acer", "huawei", "samsung", "razer", "microsoft"] {
        assert!(hardware::board_link(v).starts_with("https://"), "{}", v);
    }
    assert_eq!(hardware::board_link("x"), "");
    assert_eq!(hardware::gpu_link("nvidia"), "https://www.nvidia.com/en-us/drivers/");
    assert_eq!(hardware::chipset_link("intel"), "https://www.intel.com/content/www/us/en/support/detect.html");
    assert_eq!(hardware::vendor("HP Inc.", &[("ASUS", "asus"), ("HP|Hewlett", "hp")]), "hp");
    hardware::open_link("http://example.com"); // not https: nothing happens
}

#[test]
fn power_plans_are_read_from_powercfg_output() {
    let out = "Existing Power Schemes (* Active)\n-----------------------------------\nPower Scheme GUID: 381b4222-f694-41f0-9685-ff5bb260df2e  (Balanced) *\nPower Scheme GUID: 8c5e7fda-e8bf-4a96-9a85-a6e23a8c635c  (High performance)\n";
    let p = pages::hw_parse_plans(out);
    assert_eq!(p.len(), 2);
    assert_eq!(p[0], ("381b4222-f694-41f0-9685-ff5bb260df2e".to_string(), "Balanced".to_string(), true));
    assert_eq!(p[1].1, "High performance");
    assert!(!p[1].2);
}

// ---- ABI plumbing ------------------------------------------------------------------------------------------------

#[test]
fn jobs_are_polled_once() {
    let id = state::job_new();
    assert_eq!(state::job_take(id), None);
    state::job_done(id, json!({"x": 1}));
    assert_eq!(state::job_take(id), Some(Some(json!({"x": 1}))));
    assert_eq!(state::job_take(id), Some(None));
}

#[test]
fn version_grows_and_tray_notices_queue() {
    let v = state::version();
    state::tray("info", "hello");
    assert!(state::version() > v);
    let t = state::take_tray();
    assert!(t.iter().any(|x| x.0 == "info" && x.1 == "hello"));
}

#[test]
fn fmt_formats_like_string_format() {
    assert_eq!(pages::fmt_args("{0} of {1}", &json!([3, "x"])), "3 of x");
    assert_eq!(pages::fmt_args("{0:+0;-0;0} MHz", &json!([-5])), "-5 MHz");
    assert_eq!(pages::hex(0x1F), "0x1F");
    assert_eq!(pages::mask(&json!({"m": "0x10"}), "m"), 16);
    assert_eq!(pages::mask(&json!({"m": 7}), "m"), 7);
}

#[test]
fn getters_without_a_running_app_answer() {
    let s = pages::shell_state();
    assert_eq!(s["state"], 0);
    assert!(pages::get("nothing", &json!({}))["error"].is_string());
    assert_eq!(pages::start("nothing", Arc::new(json!({}))), 0);
    // With no app, jobs that need the worker are refused.
    assert_eq!(pages::start("processes_list", Arc::new(json!({}))), 0);
}

#[test]
fn abi_strings_round_trip() {
    let p = super::abi::out("a\0b");
    // SAFETY: a string this library returned, freed once.
    unsafe {
        assert_eq!(std::ffi::CStr::from_ptr(p).to_str().unwrap(), "a b");
        super::abi::dos_free_str(p);
    }
    let _ = win::INFINITE;
}
