//! Port of the start-up and exit logic of App.xaml.cs: the command line switches, the single instance, the
//! crash-safe start (SafetyAtStart, the crashed session, RevertRetired), the long-lived objects in C# order, the clean
//! exit, the update hand-over (swap, start the new version, wait for its signal, put the previous one back).

use super::auto_command::AutoCommand;
use super::hub::Hub;
use super::optimizer::Optimizer;
use super::update_service::UpdateService;
use super::{hardware, state, win};
use crate::auto::controller::{AutoController, AutoState, NativeProbe};
use crate::base::app_log::{AppLog, Sev};
use crate::base::hooks::lock;
use crate::base::l::L;
use crate::base::settings::Settings;
use crate::base::state_dir::StateDir;
use crate::base::time::TimeSpan;
use crate::engine::{AutoLogic, DefenderGameExclusion, Engine, GamePower, Guard, SessionFile, SysLayer, Tables, ThreadIni};
use crate::etw::{BlackBox, EtwFrames, EtwIrq, IFrameSource, IrqAuto, ReadyProbe};
use crate::lf;
use crate::native::INative;
use crate::system::{deps, Autostart, BootGuard, Devices, LegacyImport, Maintenance, Store, Tweaks};
use crate::system::updater::{AppVersion, Updater};
use crate::topo::TopoBuilder;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::time::Duration;

pub const MUTEX_NAME: &str = "Local\\DomainScheduler.Instance";
pub const SHOW_EVENT_NAME: &str = "Local\\DomainScheduler.Show";
pub const UPDATE_OK_NAME: &str = "Local\\DomainScheduler.UpdateOk";

/// Everything App.xaml.cs keeps in fields.
pub struct App {
    pub hub: Arc<Hub>,
    pub opt: Arc<Optimizer>,
    pub auto_cmd: Mutex<Option<Arc<AutoCommand>>>,
    pub show_event: win::H,
    shutdown: AtomicBool,
    /// Started by an update (not the rolled-back one): its clean exit tells the old version all is well.
    after_update: AtomicBool,
    /// The instance mutex lives on a thread of its own (a mutex is released by the thread that owns it).
    mutex_release: Mutex<Option<Sender<()>>>,
    /// Hardware.Detect() of the shell's footer and "This system" (read once, off the UI thread).
    pub facts: RwLock<Option<hardware::Facts>>,
}

static APP: OnceLock<Arc<App>> = OnceLock::new();

pub fn app() -> Option<Arc<App>> {
    APP.get().cloned()
}

pub fn hub() -> Option<Arc<Hub>> {
    APP.get().map(|a| a.hub.clone())
}

fn has(args: &[String], a: &str) -> bool {
    args.iter().any(|x| x == a)
}

fn exe_path() -> PathBuf {
    std::env::current_exe().unwrap_or_default()
}

/// The calls into other areas that C# made directly (system::deps, the boot guard of IrqAuto, the start-up hooks).
pub fn install_hooks() {
    Tables::install_hooks();
    ThreadIni::install_hooks();
    crate::etw::irq_auto::MARK_PENDING_HOOK.set(Some(Arc::new(BootGuard::mark_pending)));
    deps::IRQ_AUTO_IMPORT_BACKUP.set(Some(Arc::new(|t: &str, e: Option<&str>, d: &mut Vec<String>| IrqAuto::import_backup(t, e, Some(d)))));
    deps::GAME_POWER_IMPORT_FILE.set(Some(Arc::new(|t: &str, d: &mut Vec<String>| GamePower::import_file(t, Some(d)))));
    deps::DEFENDER_IS_SAFE_PATH.set(Some(Arc::new(DefenderGameExclusion::is_safe_path)));
    deps::SESSION_IMPORT.set(Some(Arc::new(import_session)));
    deps::GPU_IMPORT.set(Some(Arc::new(import_gpu)));
    deps::TABLES_CHECK.set(Some(Arc::new(|t: &str| Tables::merge(&mut crate::engine::tables::Data::default(), t).map(|_| ()))));
    deps::IRQ_AUTO_REVERT_EXTRAS.set(Some(Arc::new(IrqAuto::revert_extras)));
    deps::GAME_POWER_RESTORE.set(Some(Arc::new(|| Ok(GamePower::restore()))));
    deps::DEFENDER_REMOVE_ALL.set(Some(Arc::new(|| Ok(DefenderGameExclusion::remove_all()))));
}

/// LegacyImport.ImportSession.
pub fn import_session(text: &str, existing: Option<&str>, dropped: &mut Vec<String>) -> Result<Option<String>, String> {
    if existing.is_some() {
        return Ok(None);
    }
    let trimmed = text.trim();
    if trimmed == "null" {
        return Ok(None);
    }
    let Some(mut s) = SessionFile::from_json(text) else {
        return Err("The JSON value could not be converted to DomainScheduler.SessionData.".into());
    };
    if s.clean_shutdown {
        return Ok(None);
    }
    let procs = s.procs.len() + s.null_procs;
    let threads = s.threads.len() + s.null_threads;
    s.procs.retain(Engine::session_proc_valid);
    s.threads.retain(Engine::session_thread_valid);
    s.null_procs = 0;
    s.null_threads = 0;
    if s.procs.len() < procs {
        dropped.push(lf!("{0} processes", (procs - s.procs.len()) as i32));
    }
    if s.threads.len() < threads {
        dropped.push(lf!("{0} threads", (threads - s.threads.len()) as i32));
    }
    Ok(Some(SessionFile::to_json(&s)))
}

/// A scratch file inside the state folder (never in %TEMP%, which the user's programs can change).
fn scratch() -> Result<PathBuf, String> {
    use aes_gcm::aead::rand_core::RngCore;
    let mut b = [0u8; 16];
    aes_gcm::aead::OsRng.fill_bytes(&mut b);
    let hex: String = b.iter().map(|x| format!("{:02x}", x)).collect();
    Ok(Store::dir()?.join(format!("import-{}.tmp", hex)))
}

/// LegacyImport.ImportGpu.
pub fn import_gpu(text: &str, existing: Option<&str>, _dropped: &mut Vec<String>) -> Result<Option<String>, String> {
    if existing.is_some() {
        return Ok(None);
    }
    let tmp = scratch()?;
    let r = std::fs::write(&tmp, text).map_err(|e| e.to_string()).map(|_| crate::gpu::GpuProfiles::load(Some(&tmp))); // names cleaned, voltage clamped, duplicates dropped
    let _ = std::fs::remove_file(&tmp);
    let mut g = r?;
    if g.profiles.is_empty() {
        return Ok(None);
    }
    if !g.at_start.is_empty() {
        g.confirm = g.at_start.clone(); // not applied until the user says yes in the program's window
        g.at_start = String::new();
    }
    Ok(Some(g.to_json()))
}

/// Before anything reads its state (1.15.0 Test 1.3): the admin-only state folder is checked (and made safe),
/// an older version's data in the user's own folder is taken over once (only what passes the checks), and the
/// settings that moved to HKLM are copied over once. Everything is logged; nothing here stops the start.
fn prepare_state() {
    if let Err(e) = Store::dir() {
        AppLog::add_sev(e, Sev::Error);
    }
    for note in StateDir::take_notes() {
        AppLog::add_sev(note, Sev::Warn);
    }
    let account = win::user_sid();
    match LegacyImport::run(&Store::legacy_dir(), &account) {
        Ok(lines) => {
            for (text, sev) in lines {
                AppLog::add_sev(text, sev);
            }
        }
        Err(e) => AppLog::add_sev(lf!("Taking over the data of the older version: {0}", e), Sev::Error),
    }
    #[cfg(windows)]
    {
        let who = if account.is_empty() { win::user_name() } else { account.clone() };
        match crate::native::settings_backend::RegistrySettingsBackend::migrate_to_machine(&who) {
            Ok(lines) => {
                for line in lines {
                    AppLog::add_sev(line, Sev::Warn);
                }
            }
            Err(e) => AppLog::add_sev(lf!("Settings migration: {0}", e), Sev::Warn),
        }
    }
}

/// The parts of OnStartup before the single-instance check. Some(code) = the process ends with that code.
pub fn early_switches(args: &[String]) -> Option<i32> {
    if has(args, "--revert-devices") {
        // Without the window and without the single-instance check: usable from Safe Mode or a script.
        let err = Devices::revert_all();
        match &err {
            None => AppLog::add_sev(L::t("Device interrupt changes reverted (--revert-devices); restart Windows"), Sev::Info),
            Some(e) => AppLog::add_sev(lf!("Devices: {0}", e), Sev::Error),
        }
        AppLog::flush();
        return Some(if err.is_none() { 0 } else { 1 });
    }
    // --auto on|off|status (1.16.0-test1.3): for Domain Bench. Talks to the running Toolbox and exits; never starts one.
    // --auto-mask on|off (1.16.0-test1.4, test setting AutoNoMask): off = Auto keeps everything else but gives the game
    // every processor. Read by the running Toolbox at its next pass; switch it while Auto is off.
    if let Some(i) = args.iter().position(|a| a == "--auto-mask") {
        let v = args.get(i + 1).map(|s| crate::base::ci::lower(s)).unwrap_or_default();
        let ok = matches!(v.as_str(), "on" | "off" | "auto");
        if ok {
            let _ = Settings::set("AutoMask", &v);
            let _ = Settings::set_bool("AutoNoMask", false);
            AppLog::add_sev(format!("Auto mask: {} (--auto-mask)", v), Sev::Info);
            AppLog::flush();
        }
        return Some(if ok { 0 } else { 4 });
    }
    if let Some(i) = args.iter().position(|a| a == "--auto") {
        return Some(AutoCommand::send(Some(args.get(i + 1).map(String::as_str).unwrap_or("status"))));
    }
    None
}

/// `new Mutex(true, name, out createdNew)` (+ the wait of --after-update) on a thread that keeps owning it.
fn take_instance(after_update: bool) -> (bool, Option<Sender<()>>) {
    let (tx, rx) = channel::<bool>();
    let (rel_tx, rel_rx) = channel::<()>();
    let t = std::thread::Builder::new().name("instance".into()).spawn(move || {
        let (h, mut created) = win::create_mutex(MUTEX_NAME);
        // After an update the old version is still closing: wait for it to let go (up to 60 s).
        if !created && after_update && h != 0 {
            let r = win::wait(h, 60_000);
            created = r == win::WAIT_OBJECT_0 || r == win::WAIT_ABANDONED;
        }
        let _ = tx.send(created);
        if !created {
            win::close(h);
            return;
        }
        if rel_rx.recv().is_ok() {
            win::release_mutex(h);
        }
        // The handle stays open until the process ends (C#: _mutex.Dispose in OnExit).
        loop {
            std::thread::park();
        }
    });
    if t.is_err() {
        return (false, None);
    }
    let created = rx.recv().unwrap_or(false);
    (created, created.then_some(rel_tx))
}

/// Settings.GetBool("AutoNoMask") ? "off" : Settings.Get("AutoMask") ?? "auto".
pub fn mask_setting() -> String {
    if Settings::get_bool("AutoNoMask") {
        return "off".into();
    }
    let v = Settings::get("AutoMask");
    if v.is_empty() { "auto".into() } else { v }
}

/// The product objects (Windows). Other platforms have no WinNative; the app does not run there.
#[cfg(windows)]
fn product() -> Option<(Arc<dyn INative>, Arc<dyn crate::native::IGameProbe>)> {
    Some((Arc::new(crate::native::win_native::WinNative::new()), Arc::new(crate::native::win_probe::WinGameProbe::new())))
}

#[cfg(not(windows))]
fn product() -> Option<(Arc<dyn INative>, Arc<dyn crate::native::IGameProbe>)> {
    None
}

/// App.OnStartup. Returns -1 when the window must be created, else the exit code.
pub fn start(args: Vec<String>, show: &mut bool) -> i32 {
    // AssemblyInfo.cs: every library is loaded from System32 only (DefaultDllImportSearchPaths). Done first.
    crate::native::sys::harden_dll_search();
    install_hooks();
    L::load(&Settings::language());
    prepare_state();
    if let Some(code) = early_switches(&args) {
        return code;
    }
    let (created, release) = take_instance(has(&args, "--after-update"));
    if !created {
        if let Some(e) = win::open_event(SHOW_EVENT_NAME) {
            win::set_event(e);
            win::close(e);
        }
        return 0;
    }
    win::below_normal();
    #[cfg(windows)]
    if let Err(e) = crate::native::settings_backend::RegistrySettingsBackend::migrate_from_legacy() {
        AppLog::add_sev(lf!("Settings migration: {0}", e), Sev::Warn);
    }
    L::load(&Settings::language());
    // From here on every error is shown in a window (ErrorPopup; there is no on-screen journal).
    AppLog::on_error_added(Arc::new(|e: &crate::base::app_log::Entry| {
        let t = e.utc.to_local().format("HH:mm:ss", &crate::base::fmt::Culture::invariant());
        state::add_error(t, e.msg.clone());
    }));
    let Some((native, probe)) = product() else {
        AppLog::add_sev("This platform cannot run Domain OS Toolbox", Sev::Error);
        return 1;
    };
    let engine = Arc::new(Engine::new(native));
    let auto_slot: Arc<OnceLock<std::sync::Weak<AutoController>>> = Arc::new(OnceLock::new());
    {
        let e2 = Arc::downgrade(&engine);
        let slot = auto_slot.clone();
        engine.set_gate_hook(Some(Arc::new(move |pid: u32, exe: &str| {
            let names: Vec<String> = e2.upgrade().map(|e| e.processes_now().values().map(|v| v.0.clone()).collect()).unwrap_or_default();
            Guard::gate(exe, crate::engine::guard::Op::Process, false, Some(&names)).or_else(|| {
                let a = slot.get().and_then(|w| w.upgrade());
                if a.map(|a| a.protected(pid)).unwrap_or(false) {
                    Some(lf!("{0} is protected by an anti-cheat", exe))
                } else {
                    None
                }
            })
        })));
    }
    if has(&args, "--factory-reset") || has(&args, "--uninstall") {
        for item in factory_reset_lines(&engine) {
            AppLog::add(item);
        }
        AppLog::flush();
        return 0;
    }
    crate::topo::game_mask::IRQ_SOURCE.set(Some(Arc::new(Devices::pinned_irqs)));
    EtwIrq::stop_orphan();
    BlackBox::stop_orphan();
    ReadyProbe::stop_orphan();
    crate::etw::thread_map::ThreadMapTrace::stop_orphan();
    EtwIrq::stop_orphan_sched();
    let _ = std::thread::Builder::new().name("safety-at-start".into()).spawn(safety_at_start);
    let _ = std::thread::Builder::new().name("restore-at-start".into()).spawn(|| {
        let s = DefenderGameExclusion::remove_all();
        if !s.is_empty() {
            AppLog::add(s);
        }
        let p = GamePower::restore(); // power values a test build left behind
        if !p.is_empty() {
            AppLog::add(p);
        }
    });
    let frames = Arc::new(EtwFrames::new());
    frames.start();
    if !frames.available() {
        AppLog::add_sev(lf!("Frame time is not measured: {0}", frames.error()), Sev::Warn);
    }
    let fs: Arc<dyn IFrameSource> = frames.clone();
    let auto = Arc::new(AutoController::new(engine.clone(), Arc::new(NativeProbe(probe)), Some(fs), None));
    let _ = auto_slot.set(Arc::downgrade(&auto));
    // 1.16.0-test1.5: on 8 cores or fewer the game keeps every processor (AutoLogic.KeepsAllCores); setting AutoMask
    // auto|on|off, --auto-mask. The test1.4 setting AutoNoMask=true still means "off".
    let topo_at_start = TopoBuilder::live();
    {
        let t = topo_at_start.clone();
        auto.set_mask_filter(Some(Arc::new(move |mp| {
            if AutoLogic::keeps_all_cores(t.as_deref(), &mask_setting()) {
                AutoLogic::all_cores(t.as_deref(), mp)
            } else {
                mp
            }
        })));
    }
    let cores = topo_at_start.as_ref().map(|t| t.physical_count).unwrap_or(0);
    AppLog::add_sev(
        if AutoLogic::keeps_all_cores(topo_at_start.as_deref(), &mask_setting()) {
            lf!("Auto: the game keeps every processor ({0} cores; setting {1})", cores, mask_setting())
        } else {
            lf!("Auto: the game mask is used ({0} cores; setting {1})", cores, mask_setting())
        },
        Sev::Info,
    );
    let sys = Arc::new(SysLayer::new(engine.clone()));
    let hub = Hub::new(engine.clone(), auto.clone(), sys, Some(frames));
    {
        let h = Arc::downgrade(&hub);
        auto.set_watch_list(Some(Arc::new(move || h.upgrade().map(|h| (*h.watch()).clone()).unwrap_or_default())));
    }
    let self_test = ThreadIni::self_test();
    auto.set_self_test_problems(self_test.clone());
    for item in &self_test {
        AppLog::add_sev(format!("SelfTest: {}", item), Sev::Error);
    }
    hub.set_self_test(self_test);
    if let Some(s) = SessionFile::read() {
        if !s.clean_shutdown {
            auto.recover_crashed_session(Some(&s));
        }
        SessionFile::delete();
    }
    Autostart::ensure_on_first_run(&exe_path().to_string_lossy());
    if let Err(e) = hub.load_presets() {
        AppLog::add_sev(lf!("Presets: {0}", e), Sev::Error);
    }
    hub.configure();
    auto.set_top5(true); // 1.16.0 Test 1.9: dynamic Top-5 is the only thread mode (the role plan is gone)
    {
        let h = Arc::downgrade(&hub);
        auto.set_self_load_line(Some(Arc::new(move || h.upgrade().map(|h| h.self_line()).unwrap_or_default())));
    }
    hub.gpu_oc_startup();
    let opt = Optimizer::new(Arc::downgrade(&hub));
    let show_event = win::create_event(false, false, Some(SHOW_EVENT_NAME));
    let app = Arc::new(App {
        hub: hub.clone(),
        opt: opt.clone(),
        auto_cmd: Mutex::new(None),
        show_event,
        shutdown: AtomicBool::new(false),
        after_update: AtomicBool::new(false),
        mutex_release: Mutex::new(release),
        facts: RwLock::new(None),
    });
    let _ = APP.set(app.clone());
    // Opt.Evaluate += Shell.Refresh: every reporting job bumps the state version (optimizer.rs).
    let a1 = auto.clone();
    let a2 = auto.clone();
    let a3 = auto.clone();
    let cmd = AutoCommand::new(
        Arc::new(move || a1.on()),
        Arc::new(move || {
            if !a2.on() {
                a2.click();
            }
        }),
        Arc::new(move || {
            if a3.on() {
                a3.click();
            }
        }),
    );
    *lock(&app.auto_cmd) = Some(cmd.clone());
    {
        let c = Arc::downgrade(&cmd);
        auto.on_changed(Arc::new(move || {
            if let Some(c) = c.upgrade() {
                c.mirror();
            }
        }));
        auto.on_changed(Arc::new(|| {
            state::bump();
            // redraws the cards only when a game started or ended
            if let Some(u) = UpdateService::instance() {
                u.poke();
            }
        }));
    }
    opt.start();
    {
        let a = auto.clone();
        UpdateService::new(
            Arc::new(move || a.state() == AutoState::Active),
            Arc::new(|t: &str| state::tray("info", t)),
            Arc::new(|content: Vec<u8>, version: String, failed: Box<dyn Fn(String) + Send + Sync>| install_update(content, version, failed)),
            true,
        );
    }
    after_update(&args);
    *show = !has(&args, "--autostart");
    AppLog::add(lf!("Started, CPU: {0}", crate::native::sys::cpu_name()));
    // Unpacked libraries of other builds (an update leaves the old ones): removed once the window's are in use.
    let _ = std::thread::Builder::new().name("clean-old".into()).spawn(|| {
        std::thread::sleep(Duration::from_secs(60));
        crate::native::native_home::NativeHome::clean_old();
    });
    // The graphics card name for the footer and "This system": read once, off the UI thread (Shell's constructor).
    let a = app.clone();
    let _ = std::thread::Builder::new().name("hardware".into()).spawn(move || {
        let f = hardware::detect();
        *a.facts.write().unwrap_or_else(|e| e.into_inner()) = Some(f);
        state::bump();
    });
    install_panic_hook();
    -1
}

/// The last chance of a fatal error (C#: AppDomain.UnhandledException): logged, everything put back, the log written.
fn install_panic_hook() {
    if !cfg!(panic = "abort") {
        return; // a panic that unwinds is caught where C# caught the exception
    }
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let msg = info.payload().downcast_ref::<&str>().map(|s| s.to_string()).or_else(|| info.payload().downcast_ref::<String>().cloned()).unwrap_or_default();
        AppLog::add_sev(lf!("Unhandled error: {0}", msg), Sev::Error);
        emergency_revert();
        prev(info);
    }));
}

/// Maintenance.FactoryReset(engine): a crashed session is put back through the engine first.
pub fn factory_reset_lines(engine: &Arc<Engine>) -> Vec<String> {
    let e = engine.clone();
    let restore = move || -> Option<i32> {
        let s = SessionFile::read()?;
        if s.clean_shutdown {
            return None;
        }
        Some(e.restore_session(Some(&s)))
    };
    Maintenance::factory_reset(Some(&restore))
}

/// At every start, off the UI thread: the device boot guard (reverts the interrupt changes when Windows did not
/// start fine after them), values older versions wrote and 1.12.0 no longer writes, and the removed protection
/// switches put back from their backups. Then the device changes are confirmed after 2 minutes of uptime.
fn safety_at_start() {
    for line in BootGuard::on_start(&Devices::revert_all) {
        AppLog::add_sev(line, Sev::Warn);
    }
    crate::topo::GameMask::invalidate_irq();
    // First start of 1.12.0: an old automatic layout is reverted and the safe one written again (boot guard on).
    if !Settings::get_bool("SafeDeviceLayout112") {
        let _ = Settings::set_bool("SafeDeviceLayout112", true);
        match IrqAuto::redo_old_layout() {
            Ok(lines) => {
                for (text, ok) in lines {
                    AppLog::add_sev(text, if ok { Sev::Warn } else { Sev::Error });
                }
                crate::topo::GameMask::invalidate_irq();
            }
            Err(e) => AppLog::add_sev(lf!("Device layout update: {0}", e), Sev::Error),
        }
    }
    let guess = !Settings::get_bool("LegacyRawMouseChecked");
    if let Ok(lines) = IrqAuto::cleanup_legacy(guess) {
        for line in lines {
            AppLog::add(line);
        }
        let _ = Settings::set_bool("LegacyRawMouseChecked", true);
    }
    for line in Tweaks::revert_retired() {
        AppLog::add_sev(line, Sev::Warn);
    }
    if BootGuard::load().map(|s| s.pending) != Some(true) {
        return;
    }
    let up = || TimeSpan::from_ms(crate::native::sys::tick_count64() as f64);
    let wait = BootGuard::CONFIRM_UPTIME - up() + TimeSpan::from_secs(1.0);
    if wait > TimeSpan::ZERO {
        std::thread::sleep(wait.to_std());
    }
    if let Some(confirmed) = BootGuard::confirm_if_due(up()) {
        AppLog::add(confirmed);
    }
}

/// `ExitApp`: everything is put back, then the app quits.
pub fn exit_app() -> bool {
    clean_up()
}

/// Everything the program changed is put back and its parts stopped (exit and update). False when already done.
pub fn clean_up() -> bool {
    let Some(a) = app() else { return true };
    if a.shutdown.swap(true, Ordering::SeqCst) {
        return false;
    }
    let auto = a.hub.auto.clone();
    let finished = a.opt.run_now(
        Box::new(move || {
            auto.revert_everything(L::t("Exit"));
        }),
        15000,
    );
    if !finished {
        // The session file is then not marked clean: the next start puts back whatever is left.
        AppLog::add_sev(L::t("Revert at exit did not finish in time: what is left is put back at the next start"), Sev::Warn);
    }
    // The show event: C# set it to end its listener and closed it. The app waits on it itself, so it stays open and
    // unset here (a set event would show the window again); it goes with the process.
    // A version started by an update that closes cleanly (the user chose Exit) is not a failed update.
    if a.after_update.load(Ordering::SeqCst) {
        if let Some(ok) = win::open_event(UPDATE_OK_NAME) {
            win::set_event(ok);
            win::close(ok);
        }
    }
    a.opt.dispose();
    if let Some(f) = &a.hub.frames {
        f.dispose();
    }
    a.hub.irq.dispose();
    a.hub.bbox.dispose();
    a.hub.waits.dispose();
    crate::etw::thread_map::ThreadMapTrace::global().stop();
    let d = DefenderGameExclusion::remove_all();
    if !d.is_empty() {
        AppLog::add(d);
    }
    let p = GamePower::restore();
    if !p.is_empty() {
        AppLog::add(p);
    }
    AppLog::flush();
    true
}

/// Installs a downloaded and checked Toolbox (1.15.0): everything is put back as on exit first; only then the
/// program file is swapped (the running one becomes "DomainScheduler.exe.prev", the checked bytes are written as the
/// program file) and the new version is started. The new one signals when it runs fine (after 20 s, or when it is
/// closed cleanly before that); if it ends with an error before that, the previous file is put back and started
/// again. Never called while a game is running (the card refuses it).
pub fn install_update(content: Vec<u8>, version: String, failed: Box<dyn Fn(String) + Send + Sync>) {
    let exe = exe_path();
    let e1 = exe.clone();
    let e2 = exe.clone();
    handover(
        Box::new(move || Updater::swap(&e1, &content).map_err(|e| e.message().to_string())),
        Box::new(move || Updater::swap_back(&e2).map_err(|e| e.message().to_string())),
        version,
        failed,
    );
}

/// "Return to the previous version" on the About page: the ".prev" file is swapped in directly.
pub fn roll_back() {
    let exe = exe_path();
    if !prev_exists() {
        return;
    }
    let e1 = exe.clone();
    let e2 = exe.clone();
    handover(
        Box::new(move || Updater::swap_to_previous(&e1).map_err(|e| e.message().to_string())),
        Box::new(move || Updater::undo_swap_to_previous(&e2).map_err(|e| e.message().to_string())),
        "previous".into(),
        Box::new(|err: String| AppLog::add_sev(err, Sev::Warn)),
    );
}

pub fn prev_exists() -> bool {
    let mut p = exe_path().into_os_string();
    p.push(".prev");
    PathBuf::from(p).exists()
}

fn release_mutex() {
    if let Some(a) = app() {
        if let Some(tx) = lock(&a.mutex_release).take() {
            let _ = tx.send(());
        }
    }
}

type Step = Box<dyn FnOnce() -> Result<(), String> + Send>;

fn handover(swap: Step, swap_back: Step, version: String, failed: Box<dyn Fn(String) + Send + Sync>) {
    let exe = exe_path();
    let from = AppVersion::full();
    if !clean_up() {
        return;
    }
    if let Err(e) = swap() {
        // Everything was already put back and stopped: the same version starts again (no change on disk).
        AppLog::add_sev(lf!("Update: the program file could not be replaced: {0}", &e), Sev::Warn);
        failed(lf!("The program file could not be replaced: {0}. Nothing was changed.", &e));
        restart_same(&exe);
        return;
    }
    AppLog::add(lf!("Update: {0} → {1}, starting the new version", &from, &version));
    let _ = Settings::set("UpdateFrom", &from);
    AppLog::flush();
    win::hide_windows(state::ui_thread());
    let ok = win::create_event(true, false, Some(UPDATE_OK_NAME));
    win::reset_event(ok); // an event left signalled by an earlier hand-over must not count for this one
    release_mutex();
    let _ = std::thread::Builder::new().name("update".into()).spawn(move || {
        let mut good = false;
        let mut child: Option<std::process::Child> = None;
        match std::process::Command::new(&exe).arg("--after-update").spawn() {
            Ok(mut p) => {
                for _ in 0..180 {
                    if good {
                        break;
                    }
                    good = win::wait(ok, 500) == win::WAIT_OBJECT_0;
                    if !good {
                        if let Ok(Some(st)) = p.try_wait() {
                            // A clean exit (code 0) is the user closing it, or a second copy handing over: not a failed start.
                            good = win::wait(ok, 0) == win::WAIT_OBJECT_0 || st.code() == Some(0);
                            break;
                        }
                    }
                }
                child = Some(p);
            }
            Err(e) => AppLog::add_sev(lf!("Update: the new version did not start: {0}", e.to_string()), Sev::Warn),
        }
        // Still running after 90 s without the signal: it is left alone (nothing in Toolbox ends a process) and
        // counts as started. Only a new version that ended with an error is replaced with the previous one.
        if !good {
            if let Some(p) = child.as_mut() {
                if matches!(p.try_wait(), Ok(None)) {
                    good = true;
                    AppLog::add_sev(lf!("Update: {0} runs but did not confirm its start", &version), Sev::Warn);
                }
            }
        }
        if !good {
            match swap_back() {
                Ok(()) => {
                    AppLog::add_sev(lf!("Update: {0} did not start, the previous version was put back", &version), Sev::Warn);
                    AppLog::flush();
                    if let Err(e) = std::process::Command::new(&exe).args(["--after-update", "--rolled-back"]).spawn() {
                        AppLog::add_sev(lf!("Update: putting the previous version back failed: {0}", e.to_string()), Sev::Error);
                        AppLog::flush();
                    }
                }
                Err(e) => {
                    AppLog::add_sev(lf!("Update: putting the previous version back failed: {0}", e), Sev::Error);
                    AppLog::flush();
                }
            }
        }
        state::request_quit();
    });
}

/// After a failed swap (everything already stopped): the unchanged program starts again and this copy closes.
fn restart_same(exe: &std::path::Path) {
    release_mutex();
    AppLog::flush();
    if let Err(e) = std::process::Command::new(exe).args(["--after-update", "--update-failed"]).spawn() {
        AppLog::add_sev(lf!("Update: {0}", e.to_string()), Sev::Error);
    }
    state::request_quit();
}

/// Started by an update: tell the old version all is well (after 20 s of running), say what happened.
fn after_update(args: &[String]) {
    if !has(args, "--after-update") {
        return;
    }
    if has(args, "--rolled-back") {
        AppLog::add_sev(L::t("The update did not start: the previous version was put back"), Sev::Warn);
        state::tray("info", L::t("The update did not start, so the previous version was put back. Nothing else changed."));
        return;
    }
    if has(args, "--update-failed") {
        state::tray("info", L::t("The update could not be installed: the program file could not be replaced. Nothing was changed."));
        return;
    }
    if let Some(a) = app() {
        a.after_update.store(true, Ordering::SeqCst);
    }
    let _ = std::thread::Builder::new().name("after-update".into()).spawn(|| {
        std::thread::sleep(Duration::from_secs(20));
        if let Some(ok) = win::open_event(UPDATE_OK_NAME) {
            win::set_event(ok);
            win::close(ok);
        }
        let from = Settings::get("UpdateFrom");
        AppLog::add(lf!("Updated: {0} → {1}", if from.is_empty() { "?".to_string() } else { from }, AppVersion::full()));
        state::tray("info", &lf!("Toolbox was updated to {0}.", AppVersion::display()));
    });
}

pub fn emergency_revert() {
    if let Some(h) = hub() {
        h.auto.revert_everything(L::t("Error"));
    }
    AppLog::flush();
}

/// Settings page "Factory reset": revert through the worker, Maintenance.FactoryReset, ExitApp.
pub fn factory_reset() {
    let Some(a) = app() else { return };
    let auto = a.hub.auto.clone();
    a.opt.run_now(
        Box::new(move || {
            auto.revert_everything(L::t("Factory reset"));
        }),
        15000,
    );
    for item in factory_reset_lines(&a.hub.engine) {
        AppLog::add(item);
    }
    if exit_app() {
        state::request_quit();
    }
}
