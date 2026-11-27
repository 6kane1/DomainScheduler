//! Port of Hub.cs: the long-lived objects and their wiring (Configure), the 1 s ticks of the live trace, the wait
//! probe and the black box, the automatic A/B tuner, the GPU overclock at start, the own-load numbers.
//! C# fields read by several threads become atomics or locks; the objects themselves are shared `Arc`s.

use crate::auto::burst_guard::BurstGuard;
use crate::auto::controller::AutoController;
use crate::auto::tuner::{AbTuner, Step, TunerVariant};
use crate::base::app_log::{AppLog, Sev};
use crate::base::ci::{CiMap, CiMapExt, CiSet, CiSetExt};
use crate::base::flags::Flags;
use crate::base::hooks::lock;
use crate::base::l::L;
use crate::base::model::{GameMaskInfo, Preset};
use crate::base::settings::Settings;
use crate::base::time::{DateTime, TimeSpan};
use crate::engine::{Engine, Guard, SoftClose, SysLayer, ThreadIni};
use crate::etw::{BlackBox, EtwFrames, EtwIrq, IFrameSource, ReadyProbe};
use crate::gpu::{AmdOc, GpuOc, GpuOcLogic, GpuProfiles, NvOc, Start};
use crate::lf;
use crate::native::self_threads::SelfThreads;
use crate::system::{BootGuard, Store};
use crate::topo::game_mask::{Edits, BUSY_HOOK, NOISY_HOOK};
use crate::topo::{GameMask, Topo, TopoBuilder};
use indexmap::IndexMap;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock, Weak};
use std::time::Instant;

const PART_NAMES: [&str; 4] = ["cpu load", "Auto", "soft close", "presets/background"];

/// The measured own load (C# fields under `lock (_self)`).
#[derive(Default)]
struct SelfStats {
    self_cpu_100ns: i64,
    self_at: Option<DateTime>,
    ui_ms_sum: f64,
    tick_ms_sum: f64,
    ui_count: i32,
    tick_count: i32,
    ui_skipped: i32,
    chart_ms_sum: f64,
    chart_count: i32,
    part_sum: [f64; 4],
    part_avg: [f64; 4],
    // the published numbers
    self_cpu: f64,
    self_by_thread: IndexMap<String, f64>,
    /// The busiest threads that did not register (label, share of one core): what the "rest" is made of.
    self_others: Vec<(String, f64)>,
    /// What the busy unregistered threads ran (module, share of the busy samples), from `SelfThreads::sample_tick`.
    self_work: Vec<(String, f64)>,
    ui_ms: f64,
    tick_ms: f64,
    ui_per_minute: i32,
    ui_skipped_per_minute: i32,
    chart_per_minute: i32,
    chart_ms: f64,
}

/// The tuner and what it leaves behind (C# `Tuner`, `_tunerOriginal`, `TunerPending`, `TunerOffer`, `TunerNoGain`).
pub struct TunerState {
    pub tuner: AbTuner,
    pub original: Option<TunerVariant>,
    pub pending: Option<TunerVariant>,
    pub offer: String,
    pub no_gain: bool,
}

pub struct Hub {
    me: Weak<Hub>,
    pub engine: Arc<Engine>,
    pub auto: Arc<AutoController>,
    pub sys: Arc<SysLayer>,
    pub frames: Option<Arc<EtwFrames>>,
    ui_visible: AtomicBool,
    pub soft: Arc<SoftClose>,
    /// 1.16.0 Test 1.10: caps heavy background programs during a game (see BurstGuard).
    pub burst: Arc<BurstGuard>,
    pub irq: Arc<EtwIrq>,
    /// 1.16.1 Test 1: the black box (flight recorder of the game; explains stutters).
    pub bbox: Arc<BlackBox>,
    /// 1.16.1 Test 2: short kernel-trace windows while Auto handles a game: the game threads' waits and hard faults.
    pub waits: Arc<ReadyProbe>,
    probe_on: AtomicBool,
    black_box_on: AtomicBool,
    bg_on: AtomicBool,
    game_mask_value: AtomicU64,
    irq_failed: AtomicBool,
    cpu_prev: Mutex<(Option<Vec<i64>>, Option<Vec<i64>>)>,
    stats: Mutex<SelfStats>,
    cpu_load: RwLock<Arc<Vec<f64>>>,
    presets: RwLock<Arc<CiMap<Preset>>>,
    background: RwLock<Arc<CiMap<String>>>,
    self_test: RwLock<Vec<String>>,
    paused: AtomicBool,
    reapply_ticks: AtomicI32,
    watch: RwLock<Arc<CiSet>>,
    pub tuner: Mutex<TunerState>,
    /// Live DPC/ISR trace while a game runs (latency watchdog); LiveIrqMask: Auto keeps the game off busy interrupt cores.
    live_irq_on: AtomicBool,
    live_irq_mask: AtomicBool,
    live_ticks: AtomicI32,
}

fn now_utc() -> DateTime {
    DateTime::utc_now()
}

impl Hub {
    pub fn new(engine: Arc<Engine>, auto: Arc<AutoController>, sys: Arc<SysLayer>, frames: Option<Arc<EtwFrames>>) -> Arc<Hub> {
        Arc::new_cyclic(|me| Hub {
            me: me.clone(),
            engine,
            auto,
            sys,
            frames,
            ui_visible: AtomicBool::new(false),
            soft: Arc::new(SoftClose::new()),
            burst: Arc::new(BurstGuard::new(None, None)),
            irq: Arc::new(EtwIrq::new()),
            bbox: Arc::new(BlackBox::new()),
            waits: Arc::new(ReadyProbe::new()),
            probe_on: AtomicBool::new(true),
            black_box_on: AtomicBool::new(true),
            bg_on: AtomicBool::new(true),
            game_mask_value: AtomicU64::new(0),
            irq_failed: AtomicBool::new(false),
            cpu_prev: Mutex::new((None, None)),
            stats: Mutex::new(SelfStats::default()),
            cpu_load: RwLock::new(Arc::new(Vec::new())),
            presets: RwLock::new(Arc::new(CiMap::new())),
            background: RwLock::new(Arc::new(CiMap::new())),
            self_test: RwLock::new(Vec::new()),
            paused: AtomicBool::new(false),
            reapply_ticks: AtomicI32::new(10),
            watch: RwLock::new(Arc::new(CiSet::new())),
            tuner: Mutex::new(TunerState { tuner: AbTuner::new(), original: None, pending: None, offer: String::new(), no_gain: false }),
            live_irq_on: AtomicBool::new(true),
            live_irq_mask: AtomicBool::new(true),
            live_ticks: AtomicI32::new(0),
        })
    }

    // ---- the C# public fields --------------------------------------------------------------------------------

    pub fn ui_visible(&self) -> bool {
        self.ui_visible.load(Ordering::SeqCst)
    }
    pub fn set_ui_visible(&self, on: bool) {
        self.ui_visible.store(on, Ordering::SeqCst);
    }
    pub fn paused(&self) -> bool {
        self.paused.load(Ordering::SeqCst)
    }
    pub fn set_paused(&self, on: bool) {
        self.paused.store(on, Ordering::SeqCst);
    }
    pub fn bg_on(&self) -> bool {
        self.bg_on.load(Ordering::SeqCst)
    }
    pub fn game_mask_value(&self) -> u64 {
        self.game_mask_value.load(Ordering::SeqCst)
    }
    pub fn reapply_ticks(&self) -> i32 {
        self.reapply_ticks.load(Ordering::SeqCst)
    }
    pub fn probe_on(&self) -> bool {
        self.probe_on.load(Ordering::SeqCst)
    }
    pub fn black_box_on(&self) -> bool {
        self.black_box_on.load(Ordering::SeqCst)
    }
    pub fn live_irq_on(&self) -> bool {
        self.live_irq_on.load(Ordering::SeqCst)
    }
    pub fn live_irq_mask(&self) -> bool {
        self.live_irq_mask.load(Ordering::SeqCst)
    }
    pub fn cpu_load(&self) -> Arc<Vec<f64>> {
        self.cpu_load.read().unwrap_or_else(|e| e.into_inner()).clone()
    }
    pub fn presets(&self) -> Arc<CiMap<Preset>> {
        self.presets.read().unwrap_or_else(|e| e.into_inner()).clone()
    }
    pub fn background(&self) -> Arc<CiMap<String>> {
        self.background.read().unwrap_or_else(|e| e.into_inner()).clone()
    }
    pub fn self_test(&self) -> Vec<String> {
        self.self_test.read().unwrap_or_else(|e| e.into_inner()).clone()
    }
    pub fn set_self_test(&self, v: Vec<String>) {
        *self.self_test.write().unwrap_or_else(|e| e.into_inner()) = v;
    }
    /// `Watch` (the watchlist as Configure read it).
    pub fn watch(&self) -> Arc<CiSet> {
        self.watch.read().unwrap_or_else(|e| e.into_inner()).clone()
    }
    pub fn self_cpu(&self) -> f64 {
        lock(&self.stats).self_cpu
    }

    // ---- own load ----------------------------------------------------------------------------------------------

    pub fn note_ui(&self, ms: f64) {
        let mut s = lock(&self.stats);
        s.ui_ms_sum += ms;
        s.ui_count += 1;
    }

    pub fn note_chart(&self, ms: f64) {
        let mut s = lock(&self.stats);
        s.chart_ms_sum += ms;
        s.chart_count += 1;
    }

    pub fn note_ui_skipped(&self) {
        lock(&self.stats).ui_skipped += 1;
    }

    pub fn note_tick(&self, ms: f64) {
        let mut s = lock(&self.stats);
        s.tick_ms_sum += ms;
        s.tick_count += 1;
    }

    pub fn note_part(&self, i: usize, ms: f64) {
        lock(&self.stats).part_sum[i] += ms;
    }

    pub fn measure_self(&self) {
        // The stack sampler pauses our own threads for a few microseconds: diagnostics only, off unless the
        // "SelfSample" setting is true (owner decision for 1.17.0: nothing in the release pauses threads by default).
        if crate::base::settings::Settings::get_bool_or("SelfSample", false) {
            SelfThreads::sample_tick();
        }
        let utc_now = now_utc();
        {
            let s = lock(&self.stats);
            if let Some(at) = s.self_at {
                if (utc_now - at).total_seconds() < 60.0 {
                    return;
                }
            }
        }
        let total = super::win::self_cpu_100ns();
        let mut heavy = None;
        let mut s = lock(&self.stats);
        if let Some(at) = s.self_at {
            let secs = 1.0f64.max((utc_now - at).total_seconds());
            s.self_cpu = TimeSpan::from_ticks(total - s.self_cpu_100ns).total_seconds() / secs;
            s.self_by_thread = SelfThreads::shares(&SelfThreads::now(), secs);
            s.self_others = SelfThreads::others_shares(&SelfThreads::others_now(), secs);
            s.self_work = SelfThreads::work_shares().0;
            heavy = Self::heavy_rest(&s);
            s.ui_ms = if s.ui_count > 0 { s.ui_ms_sum / s.ui_count as f64 } else { 0.0 };
            s.tick_ms = if s.tick_count > 0 { s.tick_ms_sum / s.tick_count as f64 } else { 0.0 };
            s.ui_per_minute = s.ui_count;
            let tc = s.tick_count;
            let sums = s.part_sum;
            s.part_avg = sums.map(|x| if tc <= 0 { 0.0 } else { x / tc as f64 });
            s.ui_skipped_per_minute = s.ui_skipped;
            s.chart_per_minute = s.chart_count;
            s.chart_ms = if s.chart_count > 0 { s.chart_ms_sum / s.chart_count as f64 } else { 0.0 };
        }
        if s.self_at.is_none() {
            SelfThreads::shares(&SelfThreads::now(), 0.0);
            SelfThreads::others_shares(&SelfThreads::others_now(), 0.0);
            SelfThreads::work_shares();
        }
        s.self_cpu_100ns = total;
        s.self_at = Some(utc_now);
        s.ui_ms_sum = 0.0;
        s.tick_ms_sum = 0.0;
        s.part_sum = [0.0; 4];
        s.ui_count = 0;
        s.tick_count = 0;
        s.ui_skipped = 0;
        s.chart_ms_sum = 0.0;
        s.chart_count = 0;
        drop(s);
        if let Some(m) = heavy {
            AppLog::add_sev(m, Sev::Warn);
        }
    }

    /// "; by thread: window 0.40%, worker 0.20%, …, rest 0.10%" (rest = threads that did not register).
    fn threads_part(s: &SelfStats) -> String {
        let t = &s.self_by_thread;
        if t.is_empty() {
            return String::new();
        }
        let rest = 0.0f64.max(s.self_cpu - t.values().sum::<f64>());
        let mut v: Vec<(&String, &f64)> = t.iter().collect();
        v.sort_by(|a, b| b.1.partial_cmp(a.1).unwrap_or(std::cmp::Ordering::Equal));
        let parts: Vec<String> = v.iter().map(|(k, x)| lf!("{0} {1:0.00}%", L::t(k), **x * 100.0)).collect();
        format!("; {}: {}, {}{}", L::t("by thread"), parts.join(", "), lf!("{0} {1:0.00}%", L::t("rest"), rest * 100.0), Self::rest_part(s, rest))
    }

    /// " (of it: d3d10warp.dll+0x1a2b 95.00%, probe-close 1.20%; threads that ended 3.00%)": the busiest unregistered
    /// threads by name or start module, and what is left (threads that ended within the minute). "" when the rest is
    /// small or nothing was read.
    fn rest_part(s: &SelfStats, rest: f64) -> String {
        if rest < 0.01 || s.self_others.is_empty() {
            return String::new();
        }
        let named: f64 = s.self_others.iter().map(|x| x.1).sum();
        let parts: Vec<String> = s.self_others.iter().map(|(k, x)| lf!("{0} {1:0.00}%", k.as_str(), x * 100.0)).collect();
        let ended = (rest - named).max(0.0);
        let mut out = parts.join(", ");
        if ended >= 0.01 {
            out += &format!("; {}", lf!("threads that ended or were not read {0:0.00}%", ended * 100.0));
        }
        out += &Self::work_part(s);
        format!(" ({}: {})", L::t("of it"), out)
    }

    /// "; what they run: d3d10warp.dll 80%, ntdll.dll 20%" (the sampled work of the busy unregistered threads; ntdll
    /// alone = waiting in the thread pool or in a system call). "" when nothing was sampled.
    fn work_part(s: &SelfStats) -> String {
        if s.self_work.is_empty() {
            return String::new();
        }
        let parts: Vec<String> = s.self_work.iter().map(|(k, x)| format!("{k} {:.0}%", x * 100.0)).collect();
        format!("; {}: {}", L::t("what they run"), parts.join(", "))
    }

    /// The log gets the names too when the unregistered threads use a quarter of a core or more (at most every 10
    /// minutes), so the cause is in the log even when nobody opened Settings.
    fn heavy_rest(s: &SelfStats) -> Option<String> {
        static LAST: Mutex<Option<std::time::Instant>> = Mutex::new(None);
        let rest = 0.0f64.max(s.self_cpu - s.self_by_thread.values().sum::<f64>());
        if rest < 0.25 || s.self_others.is_empty() {
            return None;
        }
        {
            let mut l = lock(&LAST);
            if l.is_some_and(|t| t.elapsed() < std::time::Duration::from_secs(600)) {
                return None;
            }
            *l = Some(std::time::Instant::now());
        }
        let list: Vec<String> = s.self_others.iter().map(|(k, x)| format!("{k} {:.2}%", x * 100.0)).collect();
        Some(lf!("Own load: threads outside the list use {0:0.00}% of one core: {1}", rest * 100.0, list.join(", ")) + &Self::work_part(s))
    }

    pub fn self_line(&self) -> String {
        let s = lock(&self.stats);
        if s.ui_per_minute + s.ui_skipped_per_minute != 0 || s.tick_ms != 0.0 {
            let mut out = lf!(
                "Own load (last minute): {0:0.00}% of one core; background tick {1:0.0} ms; window drawn {2} times, {3:0.0} ms each; skipped while hidden {4}; GPU not used (software drawing)",
                s.self_cpu * 100.0,
                s.tick_ms,
                s.ui_per_minute,
                s.ui_ms,
                s.ui_skipped_per_minute
            );
            if s.chart_per_minute > 0 {
                out += &format!("; {}", lf!("home chart updated {0} times, {1:0.00} ms each", s.chart_per_minute, s.chart_ms));
            }
            if s.tick_ms > 0.0 {
                let parts: Vec<String> = PART_NAMES.iter().enumerate().map(|(i, n)| lf!("{0} {1:0.0} ms", *n, s.part_avg[i])).collect();
                out += &format!("; tick parts: {}", parts.join(", "));
            }
            out += &Self::threads_part(&s);
            return out;
        }
        L::t("Own load: measured after the first minute").to_string()
    }

    pub fn sample_cpu(&self) {
        let (mut idle, mut total) = (Vec::new(), Vec::new());
        if !self.engine.native().cpu_times(&mut idle, &mut total) {
            return;
        }
        let mut prev = lock(&self.cpu_prev);
        if let (Some(pi), Some(pt)) = (&prev.0, &prev.1) {
            if pi.len() == idle.len() && pt.len() == total.len() {
                let mut a = vec![0.0; idle.len()];
                for i in 0..idle.len() {
                    let num = total[i] - pt[i];
                    let num2 = idle[i] - pi[i];
                    a[i] = if num > 0 { (1.0 - num2 as f64 / num as f64).clamp(0.0, 1.0) } else { 0.0 };
                }
                *self.cpu_load.write().unwrap_or_else(|e| e.into_inner()) = Arc::new(a);
            }
        }
        *prev = (Some(idle), Some(total));
    }

    // ---- GPU overclock ---------------------------------------------------------------------------------------------

    /// Writes the voltage boost and both clock offsets and reads each back: the lines say what is in force.
    /// guard: remember when the values were set; if Windows crashes after that (blue screen or sudden power-off),
    /// the profile is not applied at the next start. On an AMD card (no NVIDIA card answers) the clock offsets, the
    /// power limit and the voltage offset go through AmdOc instead; the crash guard is the same.
    pub fn gpu_oc_apply(&self, core_khz: i32, mem_khz: i32, volt_pct: i32, guard: bool, power_pct: i32, volt_mv: i32) -> Vec<String> {
        gpu_oc_apply(core_khz, mem_khz, volt_pct, guard, power_pct, volt_mv)
    }

    pub fn gpu_profiles_load() -> GpuProfiles {
        gpu_profiles_load()
    }

    pub fn gpu_oc_startup(&self) {
        let mut g = gpu_profiles_load();
        // Skipped only when Windows went down unexpectedly after the last apply (blue screen, freeze with a power-off):
        // a normal restart or shutdown keeps the profile.
        let applied = DateTime::parse_iso(&Settings::get("GpuOcAppliedUtc"), false);
        let pending = applied.map(BootGuard::unexpected_shutdown_since).unwrap_or(false);
        let applied_utc = applied.unwrap_or(DateTime::MIN);
        ask_old_start_mark(&mut g);
        let (what, profile) = GpuOcLogic::decide_start(Flags::open("gpu-oc"), Some(&g), pending);
        match (what, profile) {
            (Start::SkipCrash, Some(p)) => {
                AppLog::add_sev(lf!("GPU profile \"{0}\" was NOT re-applied: Windows crashed after it was set (a blue screen or a sudden power-off). Lower the values, apply them by hand and mark the profile for the start again.", &p.name), Sev::Warn);
                g.at_start = String::new();
                g.skipped = p.name.clone();
                g.skipped_utc = now_utc();
                g.record_crash(Some(&p), applied_utc);
                let _ = Settings::set("GpuOcAppliedUtc", "");
                let _ = g.save(None);
            }
            (Start::Apply, Some(p)) => {
                // Only on the maker's card the profile was saved for: an NVIDIA profile is never applied to an AMD card by itself.
                let vendor = GpuOc::detect();
                if !GpuOcLogic::fits(Some(&p), &vendor) {
                    AppLog::add_sev(
                        if vendor.is_empty() {
                            lf!("GPU profile \"{0}\" was not applied at start: no graphics card whose driver offers overclocking answered", &p.name)
                        } else {
                            lf!("GPU profile \"{0}\" was not applied at start: it was saved for a card of another maker", &p.name)
                        },
                        Sev::Warn,
                    );
                    return;
                }
                AppLog::add(lf!("GPU profile \"{0}\" applied at start", &p.name));
                for item in gpu_oc_apply(p.core_mhz * 1000, p.mem_mhz * 1000, p.volt_pct, true, p.power_pct, p.volt_mv) {
                    AppLog::add(item);
                }
            }
            _ => {}
        }
    }

    // ---- presets and masks -------------------------------------------------------------------------------------

    pub fn load_presets(&self) -> Result<(), String> {
        let loaded = Store::load(None)?;
        for item in &loaded.notes {
            AppLog::add_sev(item, Sev::Warn);
        }
        let mut presets = loaded.presets;
        let builtins: Vec<Preset> = ThreadIni::load_bundled().presets.values().cloned().collect();
        for item2 in Store::mark_builtin(&mut presets, &builtins) {
            let show = if !item2.contains(".exe: built-in preset added") { true } else { Store::presets_path()?.exists() };
            if show {
                AppLog::add(item2);
            }
        }
        *self.presets.write().unwrap_or_else(|e| e.into_inner()) = Arc::new(presets);
        *self.background.write().unwrap_or_else(|e| e.into_inner()) = Arc::new(loaded.bg);
        Ok(())
    }

    pub fn mask_edits() -> Edits {
        let hex = |k: &str| u64::from_str_radix(Settings::get(k).trim(), 16).unwrap_or(0);
        Edits { add: hex("MaskAdd"), drop: hex("MaskDrop"), sig: Settings::get("MaskSig") }
    }

    pub fn mask_spec_text() -> String {
        let mut text = Settings::get("MaskName");
        if Settings::get_bool("MaskNoSmt") {
            text += "+nosmt";
        }
        if Settings::get_bool("MaskNoCore0") {
            text += "+nocore0";
        }
        text
    }

    pub fn mask_info(t: &Topo) -> GameMaskInfo {
        GameMask::build(t, &Self::mask_spec_text(), Some(&Self::mask_edits()), &Settings::get("IrqMode"))
    }

    pub fn configure(&self) {
        let watch: CiSet = {
            let mut s = CiSet::new();
            for w in Settings::watch() {
                s.ci_add(&w);
            }
            s
        };
        let watch = Arc::new(watch);
        *self.watch.write().unwrap_or_else(|e| e.into_inner()) = watch.clone();
        self.engine.set_timer_guard(Settings::get_bool_or("TimerGuard", true));
        GameMask::set_mode(&Settings::get("IrqMode"));
        // Live DPC/ISR runs only while a game is handled (LiveTick); the old per-processor hook is not used.
        NOISY_HOOK.set(None);
        self.live_irq_on.store(Settings::get_bool_or("LiveIrq", true), Ordering::SeqCst);
        self.black_box_on.store(Settings::get_bool_or("BlackBox", false), Ordering::SeqCst);
        self.probe_on.store(Settings::get_bool_or("WaitProbe", true), Ordering::SeqCst);
        Guard::set_faceit_cs2_allowed(Settings::get_bool_or("FaceitCs2", true));
        self.live_irq_mask.store(Settings::get_bool_or("LiveIrqMask", true), Ordering::SeqCst);
        let mut mask_name = "auto".to_string();
        self.game_mask_value.store(0, Ordering::SeqCst);
        if let Some(topo) = TopoBuilder::live() {
            let info = Self::mask_info(&topo);
            mask_name = GameMask::manual_rule(&info);
            self.game_mask_value.store(info.final_, Ordering::SeqCst);
        }
        let watch_list: Vec<String> = watch.iter().map(|w| w.0.clone()).collect();
        let presets = self.presets();
        self.sys.configure(&presets, &watch_list, &Settings::load_aff_rules(), &mask_name, Settings::get_bool("SysPresets"));
        self.bg_on.store(Settings::get_bool("BgOn"), Ordering::SeqCst);
        self.auto.set_timer_res(false);
        let me = self.me.clone();
        self.auto.set_heat(Some(Arc::new(move |exe: &str| {
            let Some(h) = me.upgrade() else { return ("none".to_string(), "none".to_string()) };
            let p = h.presets();
            match p.ci_get(exe) {
                Some(v) if !v.builtin && v.enabled => (v.hot_first.clone(), v.hot_others.clone()),
                _ => ("none".to_string(), "none".to_string()),
            }
        })));
        self.soft.set_on(false);
        let mut list = CiSet::new();
        for x in Settings::watch_list(&Settings::get("ClosableList")) {
            list.ci_add(&x);
        }
        self.soft.set_list(list);
        self.auto.set_soft(Some(self.soft.clone()));
        self.burst.set_on(Settings::get_bool_or("BurstGuard", true));
        self.burst.set_soft(Settings::get_bool_or("BurstGuardSoft", true));
        if self.burst.state_path().is_none() {
            if let Ok(d) = Store::dir() {
                self.burst.set_state_path(Some(d.join("burstguard.json")));
                self.burst.recover_after_crash();
            }
        }
        self.auto.set_burst(Some(self.burst.clone()));
        let ms = Settings::get("ReapplyMs").trim().parse::<i32>().unwrap_or(10000);
        self.reapply_ticks.store((ms / 1000).clamp(1, 1000), Ordering::SeqCst);
    }

    // ---- the automatic A/B tuner -------------------------------------------------------------------------------

    fn frames_available(&self) -> bool {
        self.frames.as_ref().map(|f| f.available()).unwrap_or(false)
    }

    /// Starts the tuner while Auto handles a game; returns a message for the log.
    pub fn tuner_start(&self) -> String {
        if lock(&self.tuner).tuner.running() {
            return L::t("Auto-tune is already running").to_string();
        }
        if !self.auto.on() || self.auto.game_pid() == 0 {
            return L::t("Auto-tune: turn Auto on and start the game first, then press the button again").to_string();
        }
        if !self.frames_available() {
            return L::t("Auto-tune needs the frame-time measurement, which is not available here").to_string();
        }
        let live = self.live_irq_on();
        // 1.16.0 Test 1.9: the role plan is gone, so auto-tune compares only the interrupt-core rule (needs live DPC/ISR).
        if !live {
            return L::t("Auto-tune compares the interrupt-core rule: turn on \"Watch driver latency\" in Settings first").to_string();
        }
        let variants = vec![
            TunerVariant::new(L::t("Dynamic Top-5"), true, false),
            TunerVariant::new(L::t("Dynamic Top-5 + interrupt cores avoided"), true, true),
        ];
        let cur_live = live && self.live_irq_mask();
        let top5 = self.auto.top5_mode();
        let baseline = variants.iter().position(|v| v.top5 == top5 && v.live_mask == cur_live).unwrap_or(0);
        let (first, minutes) = {
            let mut t = lock(&self.tuner);
            t.original = Some(variants[baseline].clone());
            t.pending = None;
            t.offer = String::new();
            t.no_gain = false;
            let first = t.tuner.start(&variants, baseline as i32, now_utc());
            (first, t.tuner.total().total_minutes().ceil() as i32)
        };
        if let Some(f) = first {
            self.apply_variant(&f);
        }
        lf!("Auto-tune started: {0} combinations, about {1} minutes. Keep playing as usual; the game must stay in front.", variants.len() as i32, minutes)
    }

    pub fn tuner_stop(&self, why: &str) {
        let original = {
            let mut t = lock(&self.tuner);
            if !t.tuner.running() {
                return;
            }
            t.tuner.cancel();
            t.original.clone()
        };
        if let Some(o) = original {
            self.apply_variant(&o);
        }
        AppLog::add_sev(lf!("Auto-tune stopped ({0}): the previous settings are back", why), Sev::Warn);
    }

    fn tuner_offer_text(t: &AbTuner, winner: i32) -> String {
        let (wl, wh) = t.stats(winner);
        let (bl, bh) = t.stats(t.baseline());
        lf!(
            "Auto-tune found a better combination: {0}. 1% low {1:0} → {2:0} FPS, hitches {3} → {4}.",
            &t.variants()[winner as usize].name,
            bl,
            wl,
            bh,
            wh
        )
    }

    /// On the worker: applies and saves the combination the auto-tune found.
    pub fn tuner_apply_pending(&self) {
        let (v, running) = {
            let t = lock(&self.tuner);
            (t.pending.clone(), t.tuner.running())
        };
        let Some(v) = v else { return };
        if !running {
            self.apply_variant(&v);
        }
        let _ = Settings::set_bool("Top5On", v.top5);
        if self.live_irq_on() {
            let _ = Settings::set_bool("LiveIrqMask", v.live_mask);
        }
        {
            let mut t = lock(&self.tuner);
            t.pending = None;
            t.offer = String::new();
        }
        AppLog::add(lf!("Auto-tune result applied: {0}", &v.name));
    }

    pub fn tuner_dismiss(&self) {
        let mut t = lock(&self.tuner);
        if t.pending.is_some() {
            AppLog::add(L::t("Auto-tune result not applied: the current settings stay"));
        }
        t.pending = None;
        t.offer = String::new();
        t.no_gain = false;
    }

    fn apply_variant(&self, v: &TunerVariant) {
        if self.auto.top5_mode() != v.top5 {
            self.auto.set_top5(v.top5);
        }
        self.live_irq_mask.store(v.live_mask, Ordering::SeqCst);
        if self.live_irq_on() && self.live_irq_mask() && self.irq.available() {
            self.set_busy_hook();
        } else {
            BUSY_HOOK.set(None);
        }
    }

    fn set_busy_hook(&self) {
        let irq = self.irq.clone();
        BUSY_HOOK.set(Some(Arc::new(move |t: &Topo| irq.busy_cores_if_ready(Some(t)))));
    }

    /// Once a second on the worker.
    pub fn tuner_tick(&self) {
        if !lock(&self.tuner).tuner.running() {
            return;
        }
        if !self.auto.on() || self.auto.game_pid() == 0 {
            self.tuner_stop(L::t("the game ended or Auto was turned off"));
            return;
        }
        let frames = self.frames.clone();
        let (step, next) = {
            let mut t = lock(&self.tuner);
            let mut f = |from: DateTime, to: DateTime| frames.as_ref().map(|x| x.intervals(from, to)).unwrap_or_default();
            t.tuner.tick(now_utc(), &mut f)
        };
        if step == Step::Switch {
            if let Some(n) = next {
                self.apply_variant(&n);
                let t = lock(&self.tuner);
                AppLog::add(lf!("Auto-tune {0}/{1}: {2}", t.tuner.step_index() + 1, t.tuner.step_count(), &n.name));
            }
        } else if step == Step::Finished {
            // The current settings come back; a better combination is only offered (TunerPending), the user applies it.
            let original = lock(&self.tuner).original.clone();
            if let Some(o) = original {
                self.apply_variant(&o);
            }
            let (table, verdict, winner) = {
                let mut t = lock(&self.tuner);
                let w = t.tuner.winner();
                let pending = if w >= 0 && w != t.tuner.baseline() { Some(t.tuner.variants()[w as usize].clone()) } else { None };
                t.offer = if pending.is_none() { String::new() } else { Self::tuner_offer_text(&t.tuner, w) };
                t.no_gain = pending.is_none();
                t.pending = pending;
                (t.tuner.table(), t.tuner.verdict().to_string(), w)
            };
            AppLog::add(format!("{}\n{}", L::t("Auto-tune results (1% low per round):"), table));
            AppLog::add_sev(&verdict, if winner >= 0 { Sev::Info } else { Sev::Warn });
            // The result also goes to its own file in the state folder, next to the log.
            let r = (|| -> Result<String, String> {
                let dir = Store::dir()?;
                let stamp = DateTime::now().format("yyyyMMdd-HHmm", &crate::base::fmt::Culture::invariant());
                let file = dir.join(format!("autotune-{}.txt", stamp));
                std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
                let nl = "\r\n";
                let text = format!("\u{feff}{}{nl}{}{nl}{nl}{}{nl}", L::t("Auto-tune results (1% low per round):"), table, verdict);
                std::fs::write(&file, text).map_err(|e| e.to_string())?;
                Ok(file.to_string_lossy().into_owned())
            })();
            match r {
                Ok(f) => AppLog::add(lf!("Auto-tune results saved: {0}", f)),
                Err(e) => AppLog::add_sev(lf!("Auto-tune results could not be saved: {0}", e), Sev::Warn),
            }
        }
    }

    // ---- live trace, wait probe, black box ---------------------------------------------------------------------

    /// Once a second on the worker: starts the live trace when a game is handled, stops it when none is.
    pub fn live_tick(&self) {
        self.box_tick();
        self.probe_tick();
        let game = self.auto.game_pid() != 0 || self.auto.background_standalone();
        if !self.live_irq_on() || !game {
            BUSY_HOOK.set(None);
            if self.irq.available() {
                self.report_long();
                self.irq.pause();
            }
            self.irq_failed.store(false, Ordering::SeqCst);
            self.live_ticks.store(0, Ordering::SeqCst);
            return;
        }
        if !self.irq.available() && !self.irq_failed.load(Ordering::SeqCst) {
            self.irq_failed.store(!self.irq.start(), Ordering::SeqCst);
        }
        if !self.irq.available() {
            BUSY_HOOK.set(None);
            return;
        }
        if self.live_irq_mask() {
            self.set_busy_hook();
        } else {
            BUSY_HOOK.set(None);
        }
        let n = self.live_ticks.fetch_add(1, Ordering::SeqCst) + 1;
        if n % 60 == 0 {
            self.report_long();
        }
    }

    /// Once a second: the wait probe runs while Auto handles a game and feeds Dynamic Top-5 (waits, interrupt shares)
    /// and the burst guard (hard faults). Off or unavailable: Top-5 ranks by load only and the guard uses free memory.
    fn probe_tick(&self) {
        let pid = self.auto.game_pid();
        let dyn_ = self.auto.dynamic();
        let irq_shares = || -> Option<Arc<crate::auto::top5::IrqSharesFn>> {
            if self.irq.available() {
                let irq = self.irq.clone();
                Some(Arc::new(move || Some(irq.shares().to_vec())))
            } else {
                None
            }
        };
        if !self.probe_on() || pid == 0 {
            if self.waits.running() {
                self.waits.stop();
            }
            dyn_.set_wait_of(None);
            self.burst.set_game_hard_faults(None);
            dyn_.set_irq_shares(irq_shares());
            return;
        }
        if !self.waits.running() || self.waits.game_pid() != pid {
            self.waits.start(pid);
            if !self.waits.running() {
                self.probe_on.store(false, Ordering::SeqCst);
                AppLog::add_sev(lf!("Wait probe is not available: {0}", self.waits.error()), Sev::Warn);
                return;
            }
            AppLog::add(L::t("Wait probe: on (Top-5 ranks the game's threads by their waits too)"));
        }
        self.waits.tick();
        let w = self.waits.clone();
        dyn_.set_wait_of(Some(Arc::new(move |tid: u32| w.wait_p99_ms(tid))));
        dyn_.set_irq_shares(irq_shares());
        let w = self.waits.clone();
        self.burst.set_game_hard_faults(Some(Arc::new(move || w.hard_faults_per_sec())));
    }

    /// Once a second: the black box records while Auto handles a game and explains a stutter when one is found.
    fn box_tick(&self) {
        let pid = self.auto.game_pid();
        if !self.black_box_on() || pid == 0 {
            if self.bbox.running() {
                self.bbox.stop();
            }
            return;
        }
        if !self.bbox.running() || self.bbox.game_pid() != pid {
            self.bbox.set_refresh_hz(BlackBox::monitor_hz());
            if self.bbox.folder().is_none() {
                match Store::dir() {
                    Ok(d) => self.bbox.set_folder(Some(d.join("blackbox"))),
                    Err(e) => {
                        AppLog::add_sev(lf!("Black box: {0}", e), Sev::Warn);
                        return;
                    }
                }
            }
            self.bbox.start(pid, &self.auto.game_exe());
            if !self.bbox.running() {
                self.black_box_on.store(false, Ordering::SeqCst);
                AppLog::add_sev(lf!("Black box is not available: {0}", self.bbox.error()), Sev::Warn);
                return;
            }
        }
        if let Some(r) = self.bbox.tick() {
            AppLog::add(lf!("Black box: stutter {0:0.0} ms, {1}", r.frame_ms, if L::lang() == "ru" { &r.text_ru } else { &r.text_en }));
            // C# BoxReport event: no subscriber; the page reads Box.Reports at its tick.
            super::state::bump();
        }
    }

    /// The latency watchdog: long DPCs/interrupts of the last minute, per driver, into the log.
    fn report_long(&self) {
        for (driver, lng, very_long, max_ms) in self.irq.take_long().into_iter().take(5) {
            if very_long > 0 {
                AppLog::add_sev(lf!("Driver latency: {0} — {1} time(s) 1 ms or longer, longest {2:0.00} ms (last minute)", driver, very_long, max_ms), Sev::Warn);
            } else if lng >= 5 {
                AppLog::add(lf!("Driver latency: {0} — {1} time(s) 0.5–1 ms, longest {2:0.00} ms (last minute)", driver, lng, max_ms));
            }
        }
    }

    pub fn set_watched(&self, exe: &str, on: bool) {
        let mut list = Settings::watch();
        list.retain(|x| !crate::base::ci::eq(x, exe));
        if on {
            list.push(exe.to_string());
        }
        let _ = Settings::set_watch(&list);
        self.configure();
        AppLog::add(if on { lf!("{0} added to the watchlist", exe) } else { lf!("{0} removed from the watchlist", exe) });
    }

    pub fn eligibility(&self, exe: &str) -> Option<String> {
        let names: Vec<String> = self.engine.processes_now().values().map(|v| v.0.clone()).collect();
        Guard::eligibility(exe, Some(&self.watch()), Some(&names))
    }

    /// For a stopwatch-like measure (C# Stopwatch.GetTimestamp pairs).
    pub fn time_ms<R>(f: impl FnOnce() -> R) -> (R, f64) {
        let t = Instant::now();
        let r = f();
        (r, t.elapsed().as_secs_f64() * 1000.0)
    }
}

// ---- GPU overclock (static parts of Hub.cs) ------------------------------------------------------------------------

pub fn gpu_oc_apply(core_khz: i32, mem_khz: i32, volt_pct: i32, guard: bool, power_pct: i32, volt_mv: i32) -> Vec<String> {
    let mut list = Vec::new();
    if guard {
        let _ = Settings::set("GpuOcAppliedUtc", &now_utc().to_o());
    }
    if GpuOc::detect() == GpuOc::AMD {
        return amd_apply(core_khz / 1000, mem_khz / 1000, volt_pct, power_pct, volt_mv);
    }
    let num = NvOc::set_voltage(volt_pct);
    list.push(match num {
        Some(v) => lf!("Voltage boost: {0}%", v),
        None => lf!("Voltage boost: {0}", NvOc::error()),
    });
    let num2 = NvOc::set(0, core_khz);
    list.push(match num2 {
        Some(v) => lf!("Core clock offset: {0:+0;-0;0} MHz", v / 1000),
        None => lf!("Core clock: {0}", NvOc::error()),
    });
    let num3 = NvOc::set(4, mem_khz);
    list.push(match num3 {
        Some(v) => lf!("Memory clock offset: {0:+0;-0;0} MHz", v / 1000),
        None => lf!("Memory clock: {0}", NvOc::error()),
    });
    list
}

/// The AMD write: one apply through the driver, then one line per value with what is in force (read back).
fn amd_apply(core_mhz: i32, mem_mhz: i32, volt_pct: i32, power_pct: i32, volt_mv: i32) -> Vec<String> {
    let mut list = Vec::new();
    let Some(s) = AmdOc::apply(core_mhz, mem_mhz, power_pct, volt_mv) else {
        list.push(lf!("GPU overclock: {0}", AmdOc::error()));
        return list;
    };
    if s.core.editable {
        list.push(lf!("Core clock offset: {0:+0;-0;0} MHz", s.core.offset()));
    }
    if s.mem.editable {
        list.push(lf!("Memory clock offset: {0:+0;-0;0} MHz", s.mem.offset()));
    }
    if s.power.editable {
        list.push(lf!("Power limit: {0:+0;-0;0} %", s.power.offset()));
    }
    if s.volt.editable {
        list.push(lf!("Voltage offset: {0:+0;-0;0} mV", s.volt.offset()));
    }
    if volt_pct > 0 {
        list.push(L::t("This graphics card or driver does not offer a voltage boost").to_string());
    }
    let err = AmdOc::error();
    if !err.is_empty() {
        list.push(lf!("GPU overclock: {0}", err));
    }
    list
}

/// The saved profiles; the "apply at start" values of 1.12 and older become a profile the first time.
pub fn gpu_profiles_load() -> GpuProfiles {
    let exists = match GpuProfiles::file_path() {
        Ok(p) => p.exists(),
        Err(_) => true, // the state folder cannot be used: nothing is migrated (or saved)
    };
    let mut g = GpuProfiles::load(None);
    if GpuOcLogic::migrate(
        Some(&mut g),
        exists,
        Settings::get_bool("GpuOcAtStart"),
        Settings::get_int("GpuOcCoreKHz", 0),
        Settings::get_int("GpuOcMemKHz", 0),
        Settings::get_int("GpuOcVoltPct", 0),
        L::t("Saved values"),
    ) && g.save(None).is_ok()
    {
        let _ = Settings::set_bool("GpuOcAtStart", false);
    }
    g
}

/// The "apply at start" mark of a version before 1.15.0 Test 1.3 (GpuProfiles.Confirm): asked once, in this
/// administrator window (other programs of the user cannot click it), with the profile's values shown. Yes keeps the
/// mark (the start below then applies the profile, with the usual crash guard); No, or closing the question, drops
/// it. The profile stays in the list either way. Asked only while GPU overclocking is turned on.
fn ask_old_start_mark(g: &mut GpuProfiles) {
    let Some(p) = g.confirm_profile().cloned() else { return };
    if !Flags::open("gpu-oc") {
        return;
    }
    let text = lf!("Toolbox has moved its data into the protected folder.\n\nIn the previous version the GPU profile \"{0}\" (core {1:+0;-0;0} MHz, memory {2:+0;-0;0} MHz, voltage boost {3} %) was applied at every start. Keep applying it at start?\n\nYes: apply it now and at every start. No: do not apply it at start (the profile stays in the list).", &p.name, p.core_mhz, p.mem_mhz, p.volt_pct);
    let yes = super::win::ask_yes_no(&text, L::t("Domain OS Toolbox: GPU profile"), false);
    g.set_at_start(if yes { Some(p.name.as_str()) } else { None });
    match g.save(None) {
        Ok(()) => AppLog::add(if yes {
            lf!("GPU profile \"{0}\": apply at start kept (confirmed after the update)", &p.name)
        } else {
            lf!("GPU profile \"{0}\": apply at start off (answered after the update)", &p.name)
        }),
        Err(e) => AppLog::add_sev(lf!("GPU overclock: {0}", e), Sev::Warn),
    }
}
