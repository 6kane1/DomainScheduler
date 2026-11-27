//! The one-button mode (sections 12 and 17, port of `AutoController.cs`): the button arms the tool, a game is detected
//! with hysteresis, laid out, watched, and everything is reverted on the button, exit or a watchdog trigger. All
//! transitions run under one lock and never re-enter.
//!
//! Locking: the C# `lock (_lock)` is [`Gate`], a re-entrant lock that the warm-up pause can let go of completely (C#
//! `Monitor.Exit` in a loop). The controller's fields sit in a separate mutex that is only held for short stretches
//! and never while the `changed` handlers run, so a handler may read the controller again (re-entrant, like C#).
//! C# exceptions are panics here: every C# `catch` is a `catch_unwind` at the same place, and a C# `throw` that
//! leaves a method lets the panic go on.

use super::burst_guard::{BurstGuard, BurstPolicy};
use super::top5::{Top5, Top5Policy};
use crate::base::app_log::{AppLog, Sev};
use crate::base::ci::{self, CiMapExt, CiSet, CiSetExt};
use crate::base::fmt::Culture;
use crate::base::hooks::lock;
use crate::base::l::L;
use crate::base::model::{ApplyOptions, ThreadSnap};
use crate::base::time::DateTime;
use crate::engine::auto_logic::{AutoLogic, AutoPolicy, Classified, MaskPlan, ThreadRole};
use crate::engine::background_deny::BackgroundDeny;
use crate::engine::engine::{Engine, Procs};
use crate::engine::guard::{Guard, Op};
use crate::engine::session::{SessionData, SessionFile};
use crate::engine::soft_close::SoftClose;
use crate::engine::tables::Tables;
use crate::etw::frames::IFrameSource;
use crate::lf;
use crate::native::IGameProbe;
use crate::topo::builder::TopoBuilder;
use crate::topo::masks::Masks;
use crate::topo::model::Topo;
use crate::topo::reservation::Reservation;
use indexmap::{IndexMap, IndexSet};
use std::any::Any;
use std::collections::{HashMap, HashSet};
use std::panic::{catch_unwind, resume_unwind, AssertUnwindSafe};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock, RwLock, Weak};
use std::thread::ThreadId;

// ---- the probe and the frame source ---------------------------------------------------------------------------------

/// What the Auto mode needs to know about running programs (C# `IGameProbe`, with the 1.16.2 `ModulesReadable`).
///
/// The native area's `IGameProbe` has no `modules_readable` yet (see the report); [`NativeProbe`] adapts it.
pub trait GameProbe: Send + Sync {
    fn self_pid(&self) -> u32;
    fn foreground_pid(&self) -> u32;
    /// Processes owning a visible, unowned top-level window (a main window).
    fn window_pids(&self) -> IndexSet<u32>;
    /// d3d11/d3d12/vulkan-1/opengl32 is loaded (opens the process for reading).
    fn has_graphics_module(&self, pid: u32) -> bool;
    /// 1.16.2: false when the process cannot be opened for reading at all (a kernel anti-cheat protects it).
    fn modules_readable(&self, _pid: u32) -> bool {
        true
    }
    fn cpu_time(&self, pid: u32, created: &mut i64, cpu100ns: &mut i64) -> bool;
    fn session_of(&self, pid: u32) -> u32;
}

/// The native area's probe (`WinGameProbe`) as a [`GameProbe`].
///
/// `modules_readable`: the C# `WinGameProbe` remembers whether its last `OpenProcess(QUERY_LIMITED | VM_READ)` in
/// `HasGraphicsModule` failed. The native port has no such method yet, so the adapter makes the same open itself when
/// asked (only for window processes without graphics modules, and for `protected`); see the report.
pub struct NativeProbe(pub Arc<dyn IGameProbe>);

impl GameProbe for NativeProbe {
    #[cfg(windows)]
    fn modules_readable(&self, pid: u32) -> bool {
        use crate::native::win::{close_handle, open_process, PROCESS_QUERY_LIMITED_INFORMATION};
        const PROCESS_VM_READ: u32 = 0x0010;
        let h = open_process(PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_VM_READ, pid);
        if h.is_null() {
            return false;
        }
        close_handle(h);
        true
    }
    fn self_pid(&self) -> u32 {
        self.0.self_pid()
    }
    fn foreground_pid(&self) -> u32 {
        self.0.foreground_pid()
    }
    fn window_pids(&self) -> IndexSet<u32> {
        self.0.window_pids()
    }
    fn has_graphics_module(&self, pid: u32) -> bool {
        self.0.has_graphics_module(pid)
    }
    fn cpu_time(&self, pid: u32, created: &mut i64, cpu100ns: &mut i64) -> bool {
        self.0.cpu_time(pid, created, cpu100ns)
    }
    fn session_of(&self, pid: u32) -> u32 {
        self.0.session_of(pid)
    }
}

/// No frame source: `available()` = false.
#[derive(Clone, Debug, Default)]
pub struct NoFrames {
    pub error: String,
}

impl IFrameSource for NoFrames {
    fn available(&self) -> bool {
        false
    }
    fn name(&self) -> String {
        String::new()
    }
    fn error(&self) -> String {
        self.error.clone()
    }
    fn watch(&self, _pid: u32) {}
    fn intervals(&self, _from: DateTime, _to: DateTime) -> Vec<f64> {
        Vec::new()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AutoState {
    Idle,
    Armed,
    Active,
    Reverting,
    Faulted,
}

// ---- exceptions -------------------------------------------------------------------------------------------------

/// A C# exception carried by a panic: `message` is `ex.Message`, `described` is `AppLog.Describe(ex)`.
#[derive(Clone, Debug)]
pub struct Ex {
    pub message: String,
    pub described: String,
}

impl Ex {
    /// The NullReferenceException C# throws where the port finds a None (no topology).
    fn null_ref() -> Ex {
        let m = "Object reference not set to an instance of an object.";
        Ex { message: m.into(), described: format!("NullReferenceException: {}", m) }
    }
}

fn ex_message(p: &(dyn Any + Send)) -> String {
    match p.downcast_ref::<Ex>() {
        Some(e) => e.message.clone(),
        None => AppLog::describe_panic(p),
    }
}

fn ex_describe(p: &(dyn Any + Send)) -> String {
    match p.downcast_ref::<Ex>() {
        Some(e) => e.described.clone(),
        None => AppLog::describe_panic(p),
    }
}

type Caught = Box<dyn Any + Send>;

fn catch<R>(f: impl FnOnce() -> R) -> Result<R, Caught> {
    catch_unwind(AssertUnwindSafe(f))
}

// ---- the re-entrant lock ------------------------------------------------------------------------------------------

/// The C# `lock (_lock)`: re-entrant, and the warm-up pause can let go of every level (`Monitor.Exit` in a loop).
#[derive(Default)]
pub struct Gate {
    m: Mutex<(Option<ThreadId>, usize)>,
    cv: Condvar,
}

pub struct GateGuard<'a>(&'a Gate);

impl Gate {
    pub fn enter(&self) -> GateGuard<'_> {
        self.take(1);
        GateGuard(self)
    }

    fn take(&self, depth: usize) {
        let me = std::thread::current().id();
        let mut g = lock(&self.m);
        loop {
            match g.0 {
                None => {
                    *g = (Some(me), depth);
                    return;
                }
                Some(o) if o == me => {
                    g.1 += depth;
                    return;
                }
                _ => g = self.cv.wait(g).unwrap_or_else(|e| e.into_inner()),
            }
        }
    }

    /// Lets go of every level this thread holds; the number of levels is given back to [`Gate::restore`].
    fn release_all(&self) -> usize {
        let me = std::thread::current().id();
        let mut g = lock(&self.m);
        if g.0 != Some(me) {
            return 0;
        }
        let d = g.1;
        *g = (None, 0);
        self.cv.notify_all();
        d
    }

    fn restore(&self, depth: usize) {
        if depth > 0 {
            self.take(depth);
        }
    }
}

impl Drop for GateGuard<'_> {
    fn drop(&mut self) {
        let mut g = lock(&self.0.m);
        if g.1 > 0 {
            g.1 -= 1;
        }
        if g.1 == 0 {
            g.0 = None;
            self.0.cv.notify_all();
        }
    }
}

// ---- the controller ---------------------------------------------------------------------------------------------

pub type NowFn = dyn Fn() -> DateTime + Send + Sync;
/// `SessionFile.Write` (crash safety); Err is the exception text.
pub type SaveFn = dyn Fn(&SessionData) -> Result<(), String> + Send + Sync;
pub type LineFn = dyn Fn() -> String + Send + Sync;
pub type MaskFilterFn = dyn Fn(MaskPlan) -> MaskPlan + Send + Sync;
pub type BoolFn = dyn Fn() -> bool + Send + Sync;
/// 22.6: the heat policy of the game's own preset (HotFirst, HotOthers).
pub type HeatFn = dyn Fn(&str) -> (String, String) + Send + Sync;
/// The watchlist (section 10).
pub type WatchFn = dyn Fn() -> CiSet + Send + Sync;
pub type ChangedFn = dyn Fn() + Send + Sync;

fn system_deny() -> &'static CiSet {
    static V: OnceLock<CiSet> = OnceLock::new();
    V.get_or_init(|| {
        ci::set_of([
            "System", "Registry", "smss.exe", "csrss.exe", "wininit.exe", "winlogon.exe", "services.exe", "lsass.exe", "dwm.exe",
            "audiodg.exe", "fontdrvhost.exe", "MsMpEng.exe", "SearchIndexer.exe", "spoolsv.exe", "taskhostw.exe",
        ])
    })
}

fn launcher_deny() -> &'static CiSet {
    static V: OnceLock<CiSet> = OnceLock::new();
    V.get_or_init(|| {
        ci::set_of([
            "steam.exe", "steamwebhelper.exe", "EpicGamesLauncher.exe", "EpicWebHelper.exe", "Battle.net.exe", "RiotClientServices.exe",
            "RiotClientUx.exe", "EADesktop.exe", "Origin.exe", "upc.exe", "UbisoftConnect.exe", "GalaxyClient.exe", "explorer.exe",
            "ShellExperienceHost.exe", "StartMenuExperienceHost.exe", "SearchHost.exe", "TextInputHost.exe", "ApplicationFrameHost.exe",
        ])
    })
}

fn role_name(r: ThreadRole) -> &'static str {
    match r {
        ThreadRole::Main => "Main",
        ThreadRole::Render => "Render",
        ThreadRole::Audio => "Audio",
        ThreadRole::Worker => "Worker",
        ThreadRole::Aux => "Aux",
        ThreadRole::Skip => "Skip",
    }
}

fn note(s: impl AsRef<str>) {
    AppLog::add_sev(s, Sev::Info);
}

fn note_sev(s: impl AsRef<str>, sev: Sev) {
    AppLog::add_sev(s, sev);
}

/// What the BeforeMutate hook needs (it runs inside the engine, under the controller's lock).
struct Shared {
    engine: Weak<Engine>,
    now: Arc<NowFn>,
    session_start: Mutex<DateTime>,
    save: RwLock<Option<Arc<SaveFn>>>,
}

impl Shared {
    fn save_fn(&self) -> Option<Arc<SaveFn>> {
        self.save.read().unwrap_or_else(|e| e.into_inner()).clone()
    }

    fn write_session(&self) {
        let r = catch(|| -> Result<(), String> {
            let Some(e) = self.engine.upgrade() else { return Ok(()) };
            let mut s = e.export();
            let start = *lock(&self.session_start);
            s.started_utc = if start == DateTime::MIN { (self.now)() } else { start };
            s.clean_shutdown = false;
            match self.save_fn() {
                Some(f) => f(&s),
                None => Ok(()),
            }
        });
        let msg = match r {
            Ok(Ok(())) => return,
            Ok(Err(m)) => m,
            Err(p) => ex_message(p.as_ref()),
        };
        note(lf!("Session file: {0}", msg));
    }
}

#[derive(Default)]
struct Cfg {
    autostart_line: Option<Arc<LineFn>>,
    self_test_problems: Vec<String>,
    self_load_line: Option<Arc<LineFn>>,
    mask_filter: Option<Arc<MaskFilterFn>>,
    faceit_allowed: Option<Arc<BoolFn>>,
    timer_res: bool,
    heat: Option<Arc<HeatFn>>,
    soft: Option<Arc<SoftClose>>,
    burst: Option<Arc<BurstGuard>>,
    watch_list: Option<Arc<WatchFn>>,
}

struct St {
    state: AutoState,
    status: String,
    busy: bool,
    // detection
    cpu: IndexMap<u32, (i64, i64, DateTime)>,
    share: IndexMap<u32, f64>,
    candidate: u32,
    candidate_since: DateTime,
    arming_start: DateTime,
    blocked: String,
    unrecognized: bool,
    not_watched: String,
    by_name: CiSet,
    bg_standalone: bool,
    top5_meta_at: DateTime,
    top5_mode: bool,
    // active game
    game: u32,
    game_created: i64,
    game_exe: String,
    absent_since: DateTime,
    mask: Option<MaskPlan>,
    plan: Vec<Classified>,
    plan_tids: IndexSet<u32>,
    confidence: f64,
    pins: bool,
    /// processes that were the game: never background
    games: HashSet<(u32, i64)>,
    warm_rates: i32,
    last_thread_count: i32,
    apply_errors: i32,
    tick: i32,
    // frame time before the changes (shown on the overview and in the report; Auto never reverts on it)
    baseline_p99: f64,
    baseline_frames: i32,
    applied_at: DateTime,
    last_revert_reason: String,
    bg_fail_logged: DateTime,
}

/// `AutoController`.
pub struct AutoController {
    gate: Gate,
    st: Mutex<St>,
    e: Arc<Engine>,
    probe: Arc<dyn GameProbe>,
    frames: Arc<dyn IFrameSource>,
    now: Arc<NowFn>,
    shared: Arc<Shared>,
    cfg: RwLock<Cfg>,
    protected: Mutex<HashSet<u32>>,
    /// The last game Auto laid out (pid, created).
    last_game: Mutex<Option<(u32, i64)>>,
    dynamic: Arc<Top5>,
    changed: RwLock<Vec<Arc<ChangedFn>>>,
}

/// Sets `busy` back when a guarded section ends (also when it panics: the C# `finally { _busy = false; }`).
struct BusyGuard<'a>(&'a AutoController);

impl Drop for BusyGuard<'_> {
    fn drop(&mut self) {
        lock(&self.0.st).busy = false;
    }
}

impl AutoController {
    pub fn is_system(exe: &str) -> bool {
        system_deny().ci_contains(exe)
    }

    /// `new AutoController(engine, probe, frames, now)`: frames None = [`NoFrames`], now None = `DateTime.UtcNow`.
    pub fn new(engine: Arc<Engine>, probe: Arc<dyn GameProbe>, frames: Option<Arc<dyn IFrameSource>>, now: Option<Arc<NowFn>>) -> AutoController {
        let now: Arc<NowFn> = now.unwrap_or_else(|| Arc::new(DateTime::utc_now));
        let default_save: Arc<SaveFn> = Arc::new(SessionFile::write);
        let shared = Arc::new(Shared {
            engine: Arc::downgrade(&engine),
            now: now.clone(),
            session_start: Mutex::new(DateTime::MIN),
            save: RwLock::new(Some(default_save)),
        });
        let sh = shared.clone();
        engine.set_before_mutate(Some(Arc::new(move || sh.write_session())));
        let dynamic = Arc::new(Top5::new(engine.clone()));
        let default_autostart: Arc<LineFn> = Arc::new(crate::system::autostart::Autostart::report_line);
        AutoController {
            gate: Gate::default(),
            st: Mutex::new(St {
                state: AutoState::Idle,
                status: L::t("Off").to_string(),
                busy: false,
                cpu: IndexMap::new(),
                share: IndexMap::new(),
                candidate: 0,
                candidate_since: DateTime::MIN,
                arming_start: DateTime::MIN,
                blocked: String::new(),
                unrecognized: false,
                not_watched: String::new(),
                by_name: CiSet::new(),
                bg_standalone: false,
                top5_meta_at: DateTime::MIN,
                top5_mode: false,
                game: 0,
                game_created: 0,
                game_exe: String::new(),
                absent_since: DateTime::MIN,
                mask: None,
                plan: Vec::new(),
                plan_tids: IndexSet::new(),
                confidence: 0.0,
                pins: false,
                games: HashSet::new(),
                warm_rates: 0,
                last_thread_count: 0,
                apply_errors: 0,
                tick: 0,
                baseline_p99: 0.0,
                baseline_frames: 0,
                applied_at: DateTime::MIN,
                last_revert_reason: String::new(),
                bg_fail_logged: DateTime::MIN,
            }),
            e: engine,
            probe,
            frames: frames.unwrap_or_else(|| Arc::new(NoFrames::default())),
            now,
            shared,
            cfg: RwLock::new(Cfg { autostart_line: Some(default_autostart), ..Default::default() }),
            protected: Mutex::new(HashSet::new()),
            last_game: Mutex::new(None),
            dynamic,
            changed: RwLock::new(Vec::new()),
        }
    }

    fn s(&self) -> MutexGuard<'_, St> {
        lock(&self.st)
    }

    fn cfg(&self) -> std::sync::RwLockReadGuard<'_, Cfg> {
        self.cfg.read().unwrap_or_else(|e| e.into_inner())
    }

    fn cfg_mut(&self) -> std::sync::RwLockWriteGuard<'_, Cfg> {
        self.cfg.write().unwrap_or_else(|e| e.into_inner())
    }

    fn now_(&self) -> DateTime {
        (self.now)()
    }

    // ---- public fields and properties ---------------------------------------------------------------------------

    pub fn state(&self) -> AutoState {
        self.s().state
    }
    pub fn status(&self) -> String {
        self.s().status.clone()
    }
    /// `event Action Changed`: called after every status change (on the thread that changed it).
    pub fn on_changed(&self, f: Arc<ChangedFn>) {
        self.changed.write().unwrap_or_else(|e| e.into_inner()).push(f);
    }
    /// `SaveSession` (default `SessionFile.Write`); None = not saved.
    pub fn set_save_session(&self, f: Option<Arc<SaveFn>>) {
        *self.shared.save.write().unwrap_or_else(|e| e.into_inner()) = f;
    }
    /// `AutostartLine` (default `Autostart.ReportLine`; the Task Scheduler is read only when the Report is built).
    pub fn set_autostart_line(&self, f: Option<Arc<LineFn>>) {
        self.cfg_mut().autostart_line = f;
    }
    /// Problems SelfTest found at start (empty = OK).
    pub fn set_self_test_problems(&self, p: Vec<String>) {
        self.cfg_mut().self_test_problems = p;
    }
    /// The program's own load, measured by the application (a line of the Report).
    pub fn set_self_load_line(&self, f: Option<Arc<LineFn>>) {
        self.cfg_mut().self_load_line = f;
    }
    /// Test hook: lets a test alter the mask plan (e.g. make it unsatisfiable).
    pub fn set_mask_filter(&self, f: Option<Arc<MaskFilterFn>>) {
        self.cfg_mut().mask_filter = f;
    }
    /// The "CS2 under FACEIT" switch (default `Guard.FaceitCs2Allowed`; tests set their own).
    pub fn set_faceit_allowed(&self, f: Option<Arc<BoolFn>>) {
        self.cfg_mut().faceit_allowed = f;
    }
    /// 22.5: the fine timer resolution is requested while a game is active.
    pub fn set_timer_res(&self, on: bool) {
        self.cfg_mut().timer_res = on;
    }
    pub fn timer_res(&self) -> bool {
        self.cfg().timer_res
    }
    /// 22.6: the heat policy of the game's own preset (HotFirst, HotOthers); None = none.
    pub fn set_heat(&self, f: Option<Arc<HeatFn>>) {
        self.cfg_mut().heat = f;
    }
    /// 22.3: soft closing of the user's list when a game becomes active.
    pub fn set_soft(&self, s: Option<Arc<SoftClose>>) {
        self.cfg_mut().soft = s;
    }
    pub fn soft(&self) -> Option<Arc<SoftClose>> {
        self.cfg().soft.clone()
    }
    /// The burst guard (1.16.0 Test 1.10): caps heavy background programs during the game. Set by the hub.
    pub fn set_burst(&self, b: Option<Arc<BurstGuard>>) {
        self.cfg_mut().burst = b;
    }
    pub fn burst(&self) -> Option<Arc<BurstGuard>> {
        self.cfg().burst.clone()
    }
    /// The watchlist (section 10): only a process on it may become the game. None = no list (tests).
    pub fn set_watch_list(&self, f: Option<Arc<WatchFn>>) {
        self.cfg_mut().watch_list = f;
    }

    fn faceit_allowed(&self) -> bool {
        let f = self.cfg().faceit_allowed.clone();
        match f {
            Some(f) => f(),
            None => Guard::faceit_cs2_allowed(),
        }
    }

    fn watch_list(&self) -> Option<CiSet> {
        let f = self.cfg().watch_list.clone();
        f.map(|f| f())
    }

    /// A window process whose memory could not be read and that no allowed anti-cheat explains: nothing may open it.
    pub fn protected(&self, pid: u32) -> bool {
        let p = lock(&self.protected);
        p.contains(&pid) && !self.probe.modules_readable(pid)
    }

    /// The dynamic Top-5 mode: instead of the role plan, five slots are re-ranked every second.
    pub fn top5_mode(&self) -> bool {
        self.s().top5_mode
    }
    pub fn dynamic(&self) -> Arc<Top5> {
        self.dynamic.clone()
    }

    /// Threads with a measured rate in the snapshot the plan was built from.
    pub fn warm_rates(&self) -> i32 {
        self.s().warm_rates
    }

    /// Switches between the role plan and the dynamic Top-5; an active game is laid out anew.
    pub fn set_top5(&self, on: bool) {
        let _g = self.gate.enter();
        if self.s().top5_mode == on {
            return;
        }
        self.s().top5_mode = on;
        note(if on { L::t("Thread mode: dynamic Top-5") } else { L::t("Thread mode: role plan") });
        {
            let s = self.s();
            if s.state != AutoState::Active || s.busy {
                return;
            }
        }
        self.s().busy = true;
        let _b = BusyGuard(self);
        let r = catch(|| {
            self.dynamic.stop(false);
            let game = self.s().game;
            self.e.revert(game);
            {
                let mut s = self.s();
                s.plan = Vec::new();
                s.plan_tids = IndexSet::new();
                s.pins = false;
                s.confidence = 0.0;
            }
            let now = self.now_();
            self.in_pass(|| {
                self.reapply(now, true);
            });
        });
        if let Err(p) = r {
            self.fault_locked(p.as_ref());
        }
    }

    /// The role plan as it is now (for the threads page): role, thread, name, processor, pinned.
    pub fn plan_view(&self) -> Vec<(ThreadRole, u32, String, u32, bool)> {
        let _g = self.gate.enter();
        let s = self.s();
        s.plan
            .iter()
            .map(|c| (c.role, c.row.tid, if !c.row.desc.is_empty() { c.row.desc.clone() } else { Engine::module_of(&c.row.sym).to_string() }, c.lp, s.pins))
            .collect()
    }

    pub fn confidence(&self) -> f64 {
        let _g = self.gate.enter();
        self.s().confidence
    }

    /// Read-only facts for the overview: the game's mask (0 = none).
    pub fn mask_now(&self) -> u64 {
        let _g = self.gate.enter();
        let s = self.s();
        match &s.mask {
            Some(m) if s.state == AutoState::Active && m.satisfied => m.mask,
            _ => 0,
        }
    }
    /// The p99 before the changes.
    pub fn baseline_p99(&self) -> f64 {
        let _g = self.gate.enter();
        let s = self.s();
        if s.state == AutoState::Active { s.baseline_p99 } else { 0.0 }
    }
    /// When the changes were applied.
    pub fn applied_at(&self) -> DateTime {
        let _g = self.gate.enter();
        let s = self.s();
        if s.state == AutoState::Active { s.applied_at } else { DateTime::MIN }
    }
    /// Why the foreground game is not handled ("" = nothing blocks).
    pub fn blocked(&self) -> String {
        let _g = self.gate.enter();
        let s = self.s();
        if !s.blocked.is_empty() {
            s.blocked.clone()
        } else if !s.not_watched.is_empty() {
            lf!("{0} is not on the watchlist", &s.not_watched)
        } else {
            String::new()
        }
    }

    /// The last game Auto laid out still runs (also after a revert or an error switched Auto off). Read without the
    /// lock: the error window asks it so that it never opens over a game that is still being played.
    pub fn last_game_running(&self) -> bool {
        let g = *lock(&self.last_game);
        let Some((pid, created)) = g else { return false };
        catch(|| self.process_alive(pid, created)).unwrap_or(false)
    }

    /// The game Auto is laying out now (0 = none): the preset layer leaves it alone.
    pub fn game_pid(&self) -> u32 {
        let _g = self.gate.enter();
        let s = self.s();
        if s.state == AutoState::Active { s.game } else { 0 }
    }
    pub fn game_exe(&self) -> String {
        let _g = self.gate.enter();
        self.s().game_exe.clone()
    }

    pub fn button_enabled(&self) -> bool {
        let _g = self.gate.enter();
        matches!(self.s().state, AutoState::Idle | AutoState::Armed | AutoState::Active)
    }

    pub fn on(&self) -> bool {
        let _g = self.gate.enter();
        matches!(self.s().state, AutoState::Armed | AutoState::Active)
    }

    fn set_status(&self, s: impl Into<String>) {
        self.s().status = s.into();
        let hs = self.changed.read().unwrap_or_else(|e| e.into_inner()).clone();
        for h in hs {
            h();
        }
    }

    fn set_state(&self, st: AutoState) {
        self.s().state = st;
    }

    fn session_start(&self) -> DateTime {
        *lock(&self.shared.session_start)
    }

    /// `BeginPass(); try { f(); } finally { EndPass(); }`
    fn in_pass<R>(&self, f: impl FnOnce() -> R) -> R {
        self.e.begin_pass();
        let r = catch(f);
        self.e.end_pass();
        match r {
            Ok(v) => v,
            Err(p) => resume_unwind(p),
        }
    }

    /// Restores what a crashed run left (session.json with cleanShutdown = false).
    pub fn recover_crashed_session(&self, s: Option<&SessionData>) -> i32 {
        let Some(s) = s else { return 0 };
        if s.clean_shutdown {
            return 0;
        }
        let n = self.e.restore_session(Some(s));
        note(lf!("Previous run did not finish: {0} values restored", n));
        n
    }

    // ---- button -------------------------------------------------------------------------------------------------

    pub fn click(&self) {
        let _g = self.gate.enter();
        if self.s().busy {
            return;
        }
        self.s().busy = true;
        let _b = BusyGuard(self);
        let r = catch(|| {
            let state = self.state();
            if state == AutoState::Idle {
                self.set_state(AutoState::Armed);
                self.s().bg_standalone = false; // from here the background belongs to Auto (BgOn is forced)
                *lock(&self.shared.session_start) = self.now_();
                self.s().candidate = 0;
                let now = self.now_();
                {
                    let mut s = self.s();
                    s.arming_start = now;
                    s.tick = 0;
                }
                note(L::t("Armed"));
                self.set_status(L::t("Waiting for the game…"));
                // The background pass is extra: if it fails, Auto still waits for the game.
                self.e.begin_pass();
                if let Err(p) = catch(|| self.pass_background(None, None)) {
                    note_sev(lf!("Background pass failed: {0}", ex_describe(p.as_ref())), Sev::Warn);
                }
                self.e.end_pass();
            } else if matches!(state, AutoState::Armed | AutoState::Active) {
                self.revert_all_locked(L::t("Button"));
            }
        });
        if let Err(p) = r {
            self.fault_locked(p.as_ref());
        }
    }

    /// Window closed, exit, Windows session ending: everything back.
    pub fn shutdown(&self) {
        let _g = self.gate.enter();
        if self.state() == AutoState::Idle {
            return;
        }
        self.revert_all_locked(L::t("Exit"));
    }

    /// Program exit or a "revert everything" request: also what the preset layer changed goes back.
    pub fn revert_everything(&self, why: &str) -> i32 {
        let _g = self.gate.enter();
        if self.state() != AutoState::Idle {
            self.revert_all_locked(why);
            return 0;
        }
        self.e.timer_release();
        self.s().bg_standalone = false;
        let n = self.e.revert_all();
        if n > 0 {
            note(lf!("Reverted ({0}): {1} values", why, n));
        }
        self.save_after_revert(self.session_start());
        self.s().last_revert_reason = why.to_string();
        n
    }

    /// After a revert the session is saved with what is left. It is marked clean only when nothing is left (every
    /// value put back, the timer released); otherwise the baselines stay in the file and the next start restores them.
    fn save_after_revert(&self, started: DateTime) {
        let r = catch(|| -> Result<(), String> {
            let mut s = self.e.export();
            if started != DateTime::MIN {
                s.started_utc = started;
            }
            s.clean_shutdown = !self.e.pending();
            if !s.clean_shutdown {
                note_sev(L::t("Not every value could be put back: they stay saved and are restored at the next start"), Sev::Warn);
            }
            match self.shared.save_fn() {
                Some(f) => f(&s),
                None => Ok(()),
            }
        });
        let msg = match r {
            Ok(Ok(())) => return,
            Ok(Err(m)) => m,
            Err(p) => ex_message(p.as_ref()),
        };
        note(lf!("Session file: {0}", msg));
    }

    fn revert_all_locked(&self, why: &str) {
        self.set_state(AutoState::Reverting);
        let r = catch(|| {
            self.set_status(L::t("Reverting…"));
            if let Err(p) = catch(|| self.e.timer_release()) {
                note_sev(lf!("Revert error: {0}", ex_message(p.as_ref())), Sev::Warn);
            }
            self.s().bg_standalone = false;
            let mut n = 0;
            match catch(|| self.e.revert_all()) {
                Ok(x) => n = x,
                Err(p) => note_sev(lf!("Revert error: {0}", ex_message(p.as_ref())), Sev::Warn),
            }
            let _ = catch(|| self.frames.watch(0));
            note(lf!("Reverted ({0}): {1} values", why, n));
            self.save_after_revert(self.session_start());
            self.clear_game();
            self.s().last_revert_reason = why.to_string();
        });
        // Never stays "Reverting": that state ignores the button and the ticks.
        self.set_state(AutoState::Idle);
        self.set_status(L::t("Off"));
        if let Err(p) = r {
            resume_unwind(p);
        }
    }

    fn fault_locked(&self, ex: &(dyn Any + Send)) {
        self.set_state(AutoState::Faulted);
        let r = catch(|| {
            note_sev(lf!("Error: {0}", ex_describe(ex)), Sev::Error);
            let _ = catch(|| self.e.timer_release());
            if let Err(p) = catch(|| self.e.revert_all()) {
                note(lf!("Revert error: {0}", ex_message(p.as_ref())));
            }
            self.save_after_revert(DateTime::MIN);
            let _ = catch(|| self.frames.watch(0));
            self.clear_game();
        });
        self.set_state(AutoState::Idle);
        self.set_status(L::t("Error. Everything was put back. Open the Report."));
        if let Err(p) = r {
            resume_unwind(p);
        }
    }

    fn clear_game(&self) {
        if let Some(b) = self.burst() {
            if let Err(p) = catch(|| b.release()) {
                note_sev(lf!("Burst guard: {0}", ex_describe(p.as_ref())), Sev::Warn);
            }
        }
        self.dynamic.stop(false); // the handles are closed; the values were put back by the engine's revert
        let mut s = self.s();
        s.game = 0;
        s.game_created = 0;
        s.game_exe = String::new();
        s.plan = Vec::new();
        s.plan_tids = IndexSet::new();
        s.mask = None;
        s.pins = false;
        s.confidence = 0.0;
        s.apply_errors = 0;
        s.last_thread_count = 0;
        s.baseline_p99 = 0.0;
        s.baseline_frames = 0;
    }

    // ---- tick ---------------------------------------------------------------------------------------------------

    /// Every second (the Optimizer's timer).
    pub fn tick(&self) {
        let _g = self.gate.enter();
        {
            let s = self.s();
            if s.busy || matches!(s.state, AutoState::Idle | AutoState::Reverting | AutoState::Faulted) {
                return;
            }
        }
        self.s().busy = true;
        let _b = BusyGuard(self);
        let r = catch(|| {
            let tick = {
                let mut s = self.s();
                s.tick += 1;
                s.tick
            };
            if let Some(soft) = self.soft() {
                soft.tick(&|pid| {
                    let (mut c, mut u) = (0i64, 0i64);
                    self.probe.cpu_time(pid, &mut c, &mut u)
                });
            }
            self.in_pass(|| self.tick_locked());
            if tick % 60 == 0 {
                self.e.prune(None);
            }
        });
        if let Err(p) = r {
            self.fault_locked(p.as_ref());
        }
    }

    fn reapply_tick(&self) -> bool {
        self.s().tick % (AutoPolicy::REAPPLY_MS / 1000).clamp(1, 1000) == 0
    }

    fn tick_locked(&self) {
        let now = self.now_();
        let target = self.detect(now);

        if self.state() == AutoState::Armed {
            if target != 0 {
                let (candidate, since, arming) = {
                    let s = self.s();
                    (s.candidate, s.candidate_since, s.arming_start)
                };
                if target != candidate {
                    {
                        let mut s = self.s();
                        s.candidate = target;
                        s.candidate_since = now;
                        s.arming_start = now;
                    }
                    self.frames.watch(target); // the frametime baseline is collected before the first change
                    self.set_status(L::t("Detecting the game…"));
                } else if (now - since).total_ms() >= AutoPolicy::ENTER_STABLE_MS as f64 {
                    if (now - arming).total_ms() > (AutoPolicy::ARMING_TIMEOUT_MS + AutoPolicy::ENTER_STABLE_MS) as f64 {
                        self.s().candidate = 0; // the attempt took too long: stay armed and try again
                        self.set_status(L::t("Waiting for the game…"));
                    } else {
                        self.activate(target, now);
                    }
                }
            } else {
                let status = {
                    let mut s = self.s();
                    s.candidate = 0;
                    if !s.blocked.is_empty() {
                        L::t("Anti-cheat: the game is not changed, the background is muted").to_string()
                    } else if !s.not_watched.is_empty() {
                        lf!("{0} is not on the watchlist: tick it on the Processes page", &s.not_watched)
                    } else if s.unrecognized {
                        L::t("Game not recognized. Background muted.").to_string()
                    } else {
                        L::t("Waiting for the game…").to_string()
                    }
                };
                self.set_status(status);
            }
            if self.reapply_tick() {
                self.pass_background(None, None);
            }
            return;
        }

        // Active.
        let (game, created, exe, tick) = {
            let s = self.s();
            (s.game, s.game_created, s.game_exe.clone(), s.tick)
        };
        if !self.process_alive(game, created) {
            note(lf!("{0} ended", &exe));
            self.leave_game(false);
            return;
        }
        // The changes stay while the game process lives, also when it is not in front (alt-tab):
        // they are reverted when the game ends, on the Revert button, or when Auto is switched off.
        self.s().absent_since = DateTime::MIN;
        if self.top5_mode() && self.dynamic.running() {
            let tids = self.e.threads_now(game);
            self.dynamic.tick(now, Some(&tids));
        }
        if let Some(b) = self.burst() {
            if b.on() && tick % BurstPolicy::SAMPLE_SECONDS == 0 {
                let r = catch(|| {
                    let procs = self.e.processes_now();
                    let c = self.background_candidates(&procs, None);
                    b.tick(game, created, true, &c, self.probe.foreground_pid());
                });
                if let Err(p) = r {
                    note_sev(lf!("Burst guard: {0}", ex_describe(p.as_ref())), Sev::Warn);
                }
            }
        }
        if self.reapply_tick() {
            if !self.reapply(now, false) {
                return;
            }
            self.pass_background(None, None);
        }
    }

    // ---- detection (12.4) ---------------------------------------------------------------------------------------

    fn process_alive(&self, pid: u32, created: i64) -> bool {
        let (mut c, mut u) = (0i64, 0i64);
        pid != 0 && self.probe.cpu_time(pid, &mut c, &mut u) && c == created
    }

    fn cpu_share(&self, pid: u32, now: DateTime) -> f64 {
        let (mut created, mut cpu) = (0i64, 0i64);
        if !self.probe.cpu_time(pid, &mut created, &mut cpu) {
            return 0.0;
        }
        let mut s = self.s();
        let mut share = s.share.get(&pid).copied().unwrap_or(0.0);
        if let Some(prev) = s.cpu.get(&pid).copied() {
            if prev.0 == created {
                let dt = (now - prev.2).total_seconds();
                if dt > 0.2 {
                    share = ((cpu - prev.1) as f64 / 1e7 / dt).max(0.0);
                } else {
                    return share;
                }
            }
        }
        s.cpu.insert(pid, (created, cpu, now));
        s.share.insert(pid, share);
        share
    }

    fn running_exes(procs: &Procs) -> Vec<String> {
        procs.values().map(|p| p.0.clone()).collect()
    }

    fn looks_like_game(&self, pid: u32, exe: &str, procs: &Procs, windows: &IndexSet<u32>, now: DateTime) -> bool {
        if pid == 0 || pid == 4 || pid == self.probe.self_pid() {
            return false;
        }
        if system_deny().ci_contains(exe) || launcher_deny().ci_contains(exe) {
            return false;
        }
        let running = Self::running_exes(procs);
        if Guard::gate_with(exe, Op::Process, false, Some(&running), self.faceit_allowed()).is_some() {
            return false;
        }
        if !windows.contains(&pid) {
            return false;
        }
        // 1.16.2: a known game (tables.json) counts without the graphics-module check. A kernel anti-cheat (FACEIT) takes
        // the right to read the game's memory away from other programs, so its module list cannot be read at all; the
        // name, the window and the CPU load still tell it is the game.
        // Fail-closed (1.16.2): checked at every detection, so an anti-cheat that arrives with a game update is seen at the
        // next start. Readable without graphics modules: not a game. Not readable at all: a kernel anti-cheat protects
        // the process, and it is left alone, except CS2 under FACEIT when its switch is on (FACEIT allowed it in writing).
        if !self.probe.has_graphics_module(pid) {
            if self.probe.modules_readable(pid) {
                return false;
            }
            let faceit_cs2 = self.faceit_allowed()
                && ci::eq(exe, "cs2.exe")
                && Tables::known_games().ci_contains(exe)
                && Guard::faceit_running(Some(&running));
            if !faceit_cs2 {
                lock(&self.protected).insert(pid);
                let first = self.s().by_name.ci_add(&format!("!{}", exe));
                if first {
                    note_sev(lf!("{0}: its memory is protected (an anti-cheat?); it is left alone", exe), Sev::Info);
                }
                if pid == self.probe.foreground_pid() {
                    self.s().blocked = lf!("{0} is protected by an anti-cheat", exe);
                }
                return false;
            }
            let first = self.s().by_name.ci_add(exe);
            if first {
                note_sev(lf!("{0}: its modules cannot be read (anti-cheat protection); recognized as a known game by its name and window", exe), Sev::Info);
            }
        }
        if self.cpu_share(pid, now) <= AutoPolicy::GAME_CPU_SHARE {
            return false;
        }
        if let Some(watch) = self.watch_list() {
            if !watch.ci_contains(exe) {
                self.s().not_watched = exe.to_string(); // looks like a game, but the user did not put it on the watchlist
                return false;
            }
        }
        true
    }

    fn detect(&self, now: DateTime) -> u32 {
        let procs = self.e.processes_now();
        let mut windows = self.probe.window_pids();
        let fg = self.probe.foreground_pid();
        let fg_exe = procs.get(&fg).map(|p| p.0.clone()).unwrap_or_default();
        let blocked = if fg != 0 && !fg_exe.is_empty() {
            let running = Self::running_exes(&procs);
            Guard::gate_with(&fg_exe, Op::Process, false, Some(&running), self.faceit_allowed()).unwrap_or_default()
        } else {
            String::new()
        };
        {
            let mut s = self.s();
            s.blocked = blocked;
            s.unrecognized = false;
            s.not_watched = String::new();
        }
        windows.retain(|pid| !(*pid != fg && !procs.contains_key(pid)));
        // CPU shares of window owners only (cheap: one GetProcessTimes each).
        for &pid in &windows {
            self.cpu_share(pid, now);
        }
        {
            let mut s = self.s();
            let gone: Vec<u32> = s.cpu.keys().copied().filter(|p| !windows.contains(p)).collect();
            for pid in gone {
                s.cpu.shift_remove(&pid);
                s.share.shift_remove(&pid);
            }
            if !s.blocked.is_empty() {
                return 0;
            }
        }
        if fg != 0 && self.looks_like_game(fg, &fg_exe, &procs, &windows, now) {
            return fg;
        }
        // The only other case that may pick a game: nothing or this program in front (the button was just pressed).
        if fg == 0 || fg == self.probe.self_pid() {
            let eligible: Vec<u32> = windows
                .iter()
                .copied()
                .filter(|p| match procs.get(p) {
                    Some(pr) => self.looks_like_game(*p, &pr.0, &procs, &windows, now),
                    None => false,
                })
                .collect();
            if eligible.len() == 1 {
                return eligible[0];
            }
        }
        if fg != 0
            && !fg_exe.is_empty()
            && windows.contains(&fg)
            && !system_deny().ci_contains(&fg_exe)
            && !launcher_deny().ci_contains(&fg_exe)
            && self.probe.has_graphics_module(fg)
        {
            let mut s = self.s();
            s.unrecognized = (now - s.arming_start).total_ms() > AutoPolicy::ARMING_TIMEOUT_MS as f64;
        }
        0
    }

    // ---- activation and re-apply --------------------------------------------------------------------------------

    fn activate(&self, pid: u32, now: DateTime) {
        let procs = self.e.processes_now();
        let (mut created, mut cpu) = (0i64, 0i64);
        self.probe.cpu_time(pid, &mut created, &mut cpu);
        *lock(&self.last_game) = Some((pid, created));
        let exe = procs.get(&pid).map(|p| p.0.clone()).unwrap_or_default();
        {
            let mut s = self.s();
            s.game = pid;
            s.game_created = created;
            s.games.insert((pid, created));
            s.game_exe = exe.clone();
            s.absent_since = DateTime::MIN;
        }
        // Frametime baseline: the frames of the detection period, before any change.
        if self.frames.available() {
            let iv = self.frames.intervals(now.add_ms(-(AutoPolicy::BASELINE_MS as f64)), now);
            let mut s = self.s();
            s.baseline_frames = iv.len() as i32;
            s.baseline_p99 = p99(&iv);
        }
        self.set_state(AutoState::Active);
        note(lf!("Game: {0} (PID {1})", &exe, pid));
        if self.timer_res() {
            self.e.timer_request();
        }
        if let Some(soft) = self.soft() {
            let watch = self.watch_list();
            soft.begin(&procs, Some(&[pid][..]), watch.as_ref(), self.probe.self_pid());
        }
        if !self.reapply(now, true) {
            return;
        }
        self.pass_background(None, None);
        let at = self.now_();
        self.s().applied_at = at;
    }

    fn leave_game(&self, revert_game: bool) {
        self.e.timer_release();
        let game = self.s().game;
        if revert_game && game != 0 {
            self.e.revert(game);
        }
        self.frames.watch(0);
        self.clear_game();
        let now = self.now_();
        {
            let mut s = self.s();
            s.candidate = 0;
            s.arming_start = now;
        }
        self.set_state(AutoState::Armed);
        self.set_status(L::t("Waiting for the game…"));
    }

    /// One application to the game; false when a watchdog trigger reverted everything.
    fn reapply(&self, now: DateTime, first: bool) -> bool {
        let t = TopoBuilder::live();
        match catch(|| self.reapply_core(t.as_deref(), now, first)) {
            Ok(r) => r,
            Err(p) => {
                let n = {
                    let mut s = self.s();
                    s.apply_errors += 1;
                    s.apply_errors
                };
                note(lf!("Apply error {0}/{1}: {2}", n, AutoPolicy::MAX_APPLY_ERRORS, ex_message(p.as_ref())));
                if n >= AutoPolicy::MAX_APPLY_ERRORS {
                    resume_unwind(p);
                }
                true
            }
        }
    }

    /// The thread-count watchdog of both modes: false = go on, Some(result) = return it.
    fn thread_drop(&self, snap: &ThreadSnap, first: bool) -> Option<bool> {
        let last = self.s().last_thread_count;
        let count = snap.rows.len() as i32;
        if !first && last > 0 && count * AutoPolicy::THREAD_DROP_FACTOR < last {
            return Some(if self.game_closing(snap) {
                false
            } else {
                self.watchdog(&lf!("The game's thread count fell from {0} to {1}", last, count))
            });
        }
        self.s().last_thread_count = count;
        None
    }

    fn reapply_core(&self, t: Option<&Topo>, now: DateTime, first: bool) -> bool {
        let Some(t) = t else { std::panic::panic_any(Ex::null_ref()) };
        // Mask (12.5).
        let mut mp = AutoLogic::build_mask(t, Reservation::reserved());
        let filter = self.cfg().mask_filter.clone();
        if let Some(f) = filter {
            mp = f(mp);
        }
        let (prev_satisfied, prev_plan_mask) = {
            let s = self.s();
            (s.mask.as_ref().map(|m| m.satisfied).unwrap_or(false), s.mask.as_ref().map(|m| m.mask).unwrap_or(0))
        };
        if !first && prev_satisfied && !mp.satisfied {
            return self.watchdog(L::t("The mask became unsatisfiable"));
        }
        if !mp.satisfied {
            for n in &mp.notes {
                note(lf!("Mask not applied: {0}", n));
            }
        }
        let prev_mask = if prev_satisfied { prev_plan_mask } else { 0 };
        let mask = if mp.satisfied { mp.mask } else { 0 };
        self.s().mask = Some(mp);
        // The mask moved (live interrupt cores changed): threads placed on processors that left it are re-planned.
        let mask_moved = !first && mask != prev_mask;

        let top5 = self.top5_mode();
        if top5 && mask_moved && self.dynamic.running() {
            self.dynamic.stop(true); // slots move to the new mask's processors
        }
        if top5 {
            return self.reapply_top5(t, mask, now, first);
        }

        let game = self.s().game;
        // Threads: warm-up on the first application, plan only when needed (17.2).
        let snap = if first {
            self.e.snapshot(game);
            let mut snap = ThreadSnap::default();
            for _ in 0..AutoPolicy::WARMUP_ROUNDS {
                if !self.warmup_sleep() {
                    return false;
                }
                snap = self.e.snapshot(game);
                if snap.rows.iter().filter(|r| r.rate > 0).count() >= 2 {
                    break;
                }
            }
            snap
        } else {
            self.e.snapshot(game)
        };
        if !snap.ok {
            return if self.game_closing(&snap) { false } else { self.watchdog(L::t("The game process cannot be opened")) };
        }
        if let Some(r) = self.thread_drop(&snap, first) {
            return r;
        }

        let hot = AutoLogic::hot_threshold(&snap);
        let now_tids: Vec<u32> = snap.rows.iter().filter(|r| r.rate > hot).map(|r| r.tid).collect();
        let (plan_empty, plan_tids) = {
            let s = self.s();
            (s.plan.is_empty(), s.plan_tids.iter().copied().collect::<Vec<u32>>())
        };
        let rebuild = first || mask_moved || plan_empty || AutoLogic::composition_change(&plan_tids, &now_tids) > AutoPolicy::PLAN_CHANGE_FRACTION;
        if rebuild {
            let (prev, old_pins) = {
                let s = self.s();
                (s.plan.iter().map(|c| (c.row.tid, c.lp)).collect::<HashMap<u32, u32>>(), s.pins)
            };
            let plan = AutoLogic::plan(t, &snap, mask, Some(&prev));
            let warm = snap.rows.iter().filter(|r| r.rate > 0).count() as i32;
            let conf = AutoLogic::confidence(&snap, &plan);
            let pins = conf >= AutoPolicy::CONFIDENCE_FLOOR;
            let same = !first && pins == old_pins && plan.len() == prev.len() && plan.iter().all(|c| prev.get(&c.row.tid) == Some(&c.lp));
            let count = plan.len();
            {
                let mut s = self.s();
                s.warm_rates = warm;
                s.plan = plan;
                s.plan_tids = now_tids.iter().copied().collect();
                s.confidence = conf;
                s.pins = pins;
            }
            if !same {
                // the log line only when the threads or their cores really changed
                let tail = if pins { String::new() } else { format!(" — {}", L::t("below the floor, no pins")) };
                note(lf!("Plan: {0} threads, confidence {1:0.00}{2}", count, conf, tail));
            }
        } else {
            let (conf, pins) = {
                let s = self.s();
                (AutoLogic::confidence(&snap, &s.plan), s.pins)
            };
            if pins && conf < AutoPolicy::CONFIDENCE_FLOOR {
                self.s().pins = false;
                self.e.drop_pins(game);
                note(lf!("Confidence fell to {0:0.00}: pins removed, the mask stays", conf));
            }
            self.s().confidence = conf;
        }
        let exe = self.s().game_exe.clone();
        let heat_fn = self.cfg().heat.clone();
        let heat = match heat_fn {
            Some(h) => h(&exe),
            None => ("none".to_string(), "none".to_string()),
        };
        let (plan, pins) = {
            let s = self.s();
            (s.plan.clone(), s.pins)
        };
        let preset = AutoLogic::build_auto_preset(&exe, &plan, pins, &heat.0, &heat.1);
        let o = ApplyOptions { do_mask: mask != 0, do_threads: true, warmup: false, game: true, ..Default::default() };
        let rep = self.e.apply_auto(game, &preset, mask, &o);
        if !rep.failures.is_empty() && rep.handle_level < 0 {
            return self.watchdog(L::t("The game process cannot be opened"));
        }
        let denied = self.e.last_denied_share();
        if denied > AutoPolicy::DENIED_SHARE_LIMIT {
            return self.watchdog(&lf!("Access denied for {0:0}% of the game's threads", denied * 100.0));
        }
        self.s().apply_errors = 0;
        self.set_status(if pins && mask != 0 { lf!("Game: {0} · boost on", &exe) } else { lf!("Game: {0} · boost on (basic)", &exe) });
        true
    }

    /// The dynamic mode's part of an application: class and mask as always, no thread rules; the threads belong to
    /// the Top-5 runner. A full snapshot (it opens every thread) is taken only at the start and when the names are
    /// older than MetaRefreshMs or new threads appeared.
    fn reapply_top5(&self, t: &Topo, mask: u64, now: DateTime, first: bool) -> bool {
        let (game, meta_at) = {
            let s = self.s();
            (s.game, s.top5_meta_at)
        };
        let mut snap: Option<ThreadSnap> = None;
        let since = (now - meta_at).total_ms();
        let meta = first
            || !self.dynamic.running()
            || self.dynamic.needs_meta() && since >= AutoPolicy::REAPPLY_MS as f64
            || since >= Top5Policy::META_REFRESH_MS as f64;
        if meta {
            if first {
                self.e.snapshot(game);
                if !self.warmup_sleep() {
                    return false;
                }
            }
            let sn = self.e.snapshot(game);
            if !sn.ok {
                return if self.game_closing(&sn) { false } else { self.watchdog(L::t("The game process cannot be opened")) };
            }
            if let Some(r) = self.thread_drop(&sn, first) {
                return r;
            }
            self.s().top5_meta_at = now;
            snap = Some(sn);
        }
        {
            let mut s = self.s();
            s.plan = Vec::new();
            s.plan_tids = IndexSet::new();
            s.pins = false;
        }
        let exe = self.s().game_exe.clone();
        let preset = AutoLogic::build_auto_preset(&exe, &[], false, "none", "none");
        let rep = self.e.apply_auto(game, &preset, mask, &ApplyOptions { do_mask: mask != 0, do_threads: false, warmup: false, game: true, ..Default::default() });
        if !rep.failures.is_empty() && rep.handle_level < 0 {
            return self.watchdog(L::t("The game process cannot be opened"));
        }
        if !self.dynamic.running() {
            if !self.dynamic.start(game, &exe, Some(t), mask) {
                note_sev(L::t("Dynamic Top-5 could not start for this game: only the mask and the priority class are set"), Sev::Warn);
            } else {
                note(lf!("Dynamic Top-5: {0} slots on processors of the game mask", Top5Policy::SLOTS.min(Masks::ideal_candidates(t, true, mask).len())));
            }
        }
        if let Some(sn) = &snap {
            self.dynamic.set_meta(Some(sn));
        }
        self.s().apply_errors = 0;
        self.set_status(lf!("Game: {0} · dynamic Top-5", &exe));
        true
    }

    /// The warm-up pause of the first application, with the lock let go: the window (which reads the state under the
    /// same lock) does not freeze while the game starts. Nothing else runs a pass meanwhile (busy is set); the button,
    /// exit or a revert can, so false is returned when the game is no longer the active one afterwards.
    fn warmup_sleep(&self) -> bool {
        let (game, created, top5) = {
            let s = self.s();
            (s.game, s.game_created, s.top5_mode)
        };
        let depth = self.gate.release_all();
        let r = catch(|| self.e.sleep(AutoPolicy::WARMUP_MS));
        self.gate.restore(depth);
        if let Err(p) = r {
            resume_unwind(p);
        }
        let s = self.s();
        s.state == AutoState::Active && s.game == game && s.game_created == created && s.top5_mode == top5
    }

    /// A game that is being closed loses its threads before the process is gone. That is a normal end (Auto stays on
    /// and waits for the next game), not a watchdog revert that switches Auto off.
    fn game_closing(&self, snap: &ThreadSnap) -> bool {
        let (game, created, exe) = {
            let s = self.s();
            (s.game, s.game_created, s.game_exe.clone())
        };
        let closing = !self.process_alive(game, created) || snap.ok && snap.rows.is_empty();
        if !closing {
            return false;
        }
        note(lf!("{0} ended", &exe));
        self.leave_game(false);
        true
    }

    fn watchdog(&self, why: &str) -> bool {
        note_sev(lf!("Watchdog: {0}", why), Sev::Warn);
        self.revert_all_locked(&lf!("watchdog: {0}", why));
        self.set_status(lf!("Reverted: {0}. Open the Report.", why));
        false
    }

    // ---- background (12.8) --------------------------------------------------------------------------------------

    /// The background layer is on because a watched program runs (Auto itself off).
    pub fn background_standalone(&self) -> bool {
        self.s().bg_standalone
    }

    /// Section 22.1: the background layer on its own (BgOn and Auto off). While a watched program runs, the
    /// background set is lowered by the same rules as in Auto; when the switch goes off or the game ends, BgRelease
    /// puts every background baseline back.
    pub fn standalone_background(&self, on: bool, game_mask: u64) -> i32 {
        let _g = self.gate.enter();
        {
            let s = self.s();
            if s.busy || s.state != AutoState::Idle {
                return 0;
            }
        }
        // One process list for the whole pass (the guard asks for it per process; without a pass every call took a new snapshot).
        self.in_pass(|| self.standalone_background_locked(on, game_mask))
    }

    fn standalone_background_locked(&self, on: bool, game_mask: u64) -> i32 {
        let procs = self.e.processes_now();
        let watch = self.watch_list();
        let games: Vec<u32> = match &watch {
            None => Vec::new(),
            Some(w) => procs.iter().filter(|(_, p)| w.ci_contains(&p.0)).map(|(k, _)| *k).collect(),
        };
        if let Some(soft) = self.soft() {
            soft.tick(&|pid| self.e.processes_now().contains_key(&pid));
        }
        if !on || games.is_empty() {
            if !self.s().bg_standalone {
                return 0;
            }
            self.s().bg_standalone = false;
            let n = self.e.revert_background();
            note(lf!("BgRelease ({0}): {1} background values put back", if on { L::t("the game ended") } else { L::t("switched off") }, n));
            return n;
        }
        if !self.s().bg_standalone {
            self.s().bg_standalone = true;
            note(L::t("Background layer: on (a watched program is running)"));
            if let Some(soft) = self.soft() {
                soft.begin(&procs, Some(&games), watch.as_ref(), self.probe.self_pid());
            }
        }
        self.pass_background(Some(&games), Some(game_mask));
        self.e.background_count()
    }

    /// The background pass never stops Auto: a failure is logged (at most once a minute) and the game goes on.
    fn pass_background(&self, games: Option<&[u32]>, mask_override: Option<u64>) {
        if let Err(p) = catch(|| self.pass_background_core(games, mask_override)) {
            let now = self.now_();
            let last = self.s().bg_fail_logged;
            if (now - last).total_seconds() >= 60.0 {
                self.s().bg_fail_logged = self.now_();
                note_sev(lf!("Background pass failed: {0}", ex_describe(p.as_ref())), Sev::Warn);
            }
        }
    }

    fn pass_background_core(&self, games: Option<&[u32]>, mask_override: Option<u64>) {
        let t = TopoBuilder::live();
        let procs = self.e.processes_now();
        let mut list = Vec::new();
        let mut timer_only = Vec::new();
        self.classify(&procs, games, &mut list, &mut timer_only, None);
        let game_mask = mask_override.unwrap_or_else(|| self.s().mask.as_ref().map(|m| m.mask).unwrap_or(0));
        let mut bg = match &t {
            Some(t) => Masks::bg_mask(t, game_mask),
            None => 0,
        };
        if let Some(t) = &t {
            if bg == t.full_mask() {
                bg = 0; // nothing to separate from
            }
        }
        self.e.apply_background(&list, bg, true, Some(&timer_only));
    }

    /// The background programs of the user's session the background layer may change (pid, exe): the burst guard's
    /// candidates.
    fn background_candidates(&self, procs: &Procs, games: Option<&[u32]>) -> Vec<(u32, String)> {
        let mut named = Vec::new();
        self.classify(procs, games, &mut Vec::new(), &mut Vec::new(), Some(&mut named));
        named
    }

    /// Sorts the processes for the background layer: `list` are lowered, `timer_only` (other sessions) only lose
    /// their timer requests.
    fn classify(&self, procs: &Procs, games: Option<&[u32]>, list: &mut Vec<u32>, timer_only: &mut Vec<u32>, mut named: Option<&mut Vec<(u32, String)>>) {
        let me = self.probe.self_pid();
        let my_session = self.probe.session_of(me);
        let fg = self.probe.foreground_pid();
        let mut self_tree: HashSet<u32> = HashSet::new();
        self_tree.insert(me);
        let mut grew = true;
        while grew {
            grew = false;
            for (pid, p) in procs {
                if !self_tree.contains(pid) && self_tree.contains(&p.1) {
                    grew = self_tree.insert(*pid);
                }
            }
        }
        let running = Self::running_exes(procs);
        let (game, seen_games, not_watched) = {
            let s = self.s();
            (s.game, s.games.clone(), s.not_watched.clone())
        };
        let faceit = self.faceit_allowed();
        for (pid, p) in procs {
            let pid = *pid;
            let name = p.0.as_str();
            if pid == 0 || pid == 4 || pid == game || self_tree.contains(&pid) {
                continue;
            }
            if games.map(|g| g.contains(&pid)).unwrap_or(false) {
                continue;
            }
            if seen_games.contains(&(pid, self.created_of(pid))) {
                continue;
            }
            // A game the user did not put on the watchlist is nobody's background: it is left alone.
            if !not_watched.is_empty() && ci::eq(name, &not_watched) {
                continue;
            }
            if pid == fg {
                continue; // what the user is working with right now is not lowered
            }
            // System processes, launchers and the shell (explorer, Start, Steam…), and audio / voice / streaming / VR
            // (BackgroundDeny) are never lowered, never on EcoQoS and keep their timer requests.
            if system_deny().ci_contains(name) || launcher_deny().ci_contains(name) || BackgroundDeny::contains(name) || Guard::is_anti_cheat_process(name) {
                continue;
            }
            if Guard::gate_with(name, Op::Process, false, Some(&running), faceit).is_some() {
                continue;
            }
            if self.probe.session_of(pid) != my_session {
                timer_only.push(pid);
                continue;
            }
            list.push(pid);
            if let Some(n) = named.as_deref_mut() {
                n.push((pid, name.to_string()));
            }
        }
    }

    fn created_of(&self, pid: u32) -> i64 {
        let (mut c, mut u) = (0i64, 0i64);
        if self.probe.cpu_time(pid, &mut c, &mut u) { c } else { 0 }
    }

    // ---- report (12.12, 16) -------------------------------------------------------------------------------------

    pub fn report(&self) -> String {
        let _g = self.gate.enter();
        const NL: &str = super::check_stats::NL;
        let t = TopoBuilder::live();
        let (game, baseline_frames) = {
            let s = self.s();
            (s.game, s.baseline_frames)
        };
        let measured = self.frames.available() && (game == 0 || baseline_frames >= AutoPolicy::MIN_FRAMES_FOR_BASELINE);
        let mut sb = String::new();
        let mut line = |s: &str| {
            sb.push_str(s);
            sb.push_str(NL);
        };
        line(&format!("{}  {}", L::t("DomainScheduler — report"), self.now_().to_local().format("yyyy-MM-dd HH:mm:ss", &Culture::invariant())));
        line("");
        if let Some(t) = &t {
            line(&lf!("CPU: {0}", &t.name));
            line(&lf!(
                "Topology: {0} logical, {1} physical, SMT {2}, groups {3}, L3 domains {4}",
                t.logical_count,
                t.physical_count,
                if t.smt { L::t("on") } else { L::t("off") },
                t.groups,
                t.llc_groups.len()
            ));
            for &l in &t.llc_groups {
                let mb = t.l3_by_llc.get(&l).copied().unwrap_or(0) >> 20;
                let lps = Masks::ranges(TopoBuilder::mask(t.sets.iter().filter(|s| s.llc == l)));
                line(&format!(
                    "  {}{}",
                    lf!("L3 domain {0}: {1} MB, processors {2}", l, mb, lps),
                    if t.big_cache_llc == Some(l) { " (V-Cache)" } else { "" }
                ));
            }
            let sp = t.split();
            line(&lf!(
                "Split: {0}",
                if sp.can_split() {
                    format!("{} — {}", sp.label(), lf!("game {0}, non-game {1}", Masks::ranges(sp.game), Masks::ranges(sp.non_game)))
                } else {
                    sp.why()
                }
            ));
        } else {
            line(L::t("CPU: the topology could not be read"));
        }
        line(&lf!("Auto: {0}", self.state_text()));
        line(&lf!("Status: {0}", self.status()));
        line("");
        let mp = self.s().mask.clone();
        if let Some(mp) = &mp {
            line(&if mp.satisfied {
                lf!("Game mask: {0} → {1} ({2})", &mp.spec, format!("0x{:x}", mp.mask), Masks::ranges(mp.mask))
            } else {
                lf!("Game mask: not applied ({0})", mp.notes.join("; "))
            });
            if let Some(irq) = &mp.irq {
                line(&if irq.irq_drop != 0 {
                    lf!("Cores excluded for IRQ: {0}", Masks::ranges(irq.irq_drop))
                } else if irq.irq_ignored {
                    L::t("Cores excluded for IRQ: none (the exclusion would cut more than half)").to_string()
                } else {
                    L::t("Cores excluded for IRQ: none").to_string()
                });
                for d in &irq.irq_devices {
                    line(&format!("  {}", lf!("{0}: processors {1}", &d.0, Masks::ranges(d.1))));
                }
                for d in &irq.irq_wide {
                    line(&format!("  {}", lf!("{0}: processors {1} (several cores, not excluded)", &d.0, Masks::ranges(d.1))));
                }
            }
        }
        if game != 0 {
            let snap = self.e.last_snap();
            let named = snap.as_ref().map(|s| s.rows.iter().filter(|r| !r.desc.is_empty()).count()).unwrap_or(0);
            let (exe, conf, pins, plan) = {
                let s = self.s();
                (s.game_exe.clone(), s.confidence, s.pins, s.plan.clone())
            };
            line(&lf!(
                "Game: {0}, PID {1}, threads {2}, named {3}, confidence {4:0.00}",
                &exe,
                game,
                snap.as_ref().map(|s| s.rows.len()).unwrap_or(0),
                named,
                conf
            ));
            if pins {
                for c in &plan {
                    let name = if !c.row.desc.is_empty() { c.row.desc.clone() } else { Engine::module_of(&c.row.sym).to_string() };
                    line(&format!("  {}", lf!("{0} {1} ({2}) → processor {3}, core {4}", role_name(c.role), c.row.tid, name, c.lp, c.core)));
                }
            } else {
                line(&format!("  {}", L::t("Pins: none (confidence below the floor or no plan)")));
            }
            line(&lf!("Priority: {0}, EcoQoS: off", L::t("Above normal")));
        } else {
            line(L::t("Game: not active"));
        }
        line(&lf!("Background processes muted: {0}", self.e.background_count()));
        let reserved = Reservation::reserved();
        line(&if reserved == 0 { L::t("Core reservation: not set").to_string() } else { lf!("Core reservation: processors {0}", Masks::ranges(reserved)) });
        if let Some(watch) = self.watch_list() {
            line(&lf!(
                "Watchlist: {0}",
                if watch.is_empty() { L::t("empty").to_string() } else { watch.iter().map(|w| w.as_str().to_string()).collect::<Vec<_>>().join(", ") }
            ));
        }
        let autostart = self.cfg().autostart_line.clone();
        let auto = match catch(|| autostart.map(|f| f()).unwrap_or_default()) {
            Ok(s) => s,
            Err(p) => lf!("Autostart: unknown ({0})", ex_message(p.as_ref())),
        };
        if !auto.is_empty() {
            line(&auto);
        }
        let problems = self.cfg().self_test_problems.clone();
        line(&if problems.is_empty() { L::t("SelfTest: OK").to_string() } else { lf!("SelfTest: {0} problems ({1})", problems.len(), problems.join("; ")) });
        line(if measured { L::t("Frametime: measured") } else { L::t("Frametime: not measured") });
        line(L::t("Manual API: present, not active in Auto"));
        let own_fn = self.cfg().self_load_line.clone();
        let own = catch(|| own_fn.map(|f| f())).ok().flatten();
        if let Some(own) = own {
            if !own.is_empty() {
                line(&own);
            }
        }
        let top5 = self.top5_mode();
        line(&if top5 { lf!("Thread mode: dynamic Top-5, {0} moves so far", self.dynamic.moves()) } else { L::t("Thread mode: role plan").to_string() });
        if top5 {
            for s in self.dynamic.view() {
                let prio = match s.prio {
                    Some(p) => format!("+{}", p),
                    None => L::t("own").to_string(),
                };
                line(&format!(
                    "  {}",
                    lf!("slot {0}: {1} ({2}) on processor {3}, priority {4}, {5:0}% of the game's load", s.index + 1, &s.name, s.tid, s.lp, prio, s.share * 100.0)
                ));
            }
        }
        if let Some(t) = &t {
            let sp = t.split();
            if sp.family == "x3d" {
                line(&lf!("Big-cache domain: {0} half of the processor numbers ({1})", if sp.upper_half { L::t("upper") } else { L::t("lower") }, Masks::ranges(sp.game)));
            }
        }
        let pins = self.e.thread_pins().len();
        if pins > 0 {
            line(&lf!("Thread CPU Sets: {0}", lf!("{0} threads limited to their processor", pins)));
        }
        line(&lf!(
            "Timer resolution: {0}",
            if self.e.timer_requested() {
                L::t("requested (released on revert)")
            } else if self.timer_res() {
                L::t("on, not requested now")
            } else {
                L::t("off")
            }
        ));
        let soft = self.soft();
        line(&lf!(
            "Soft close: {0}",
            match &soft {
                Some(s) if s.on() => lf!("on, {0} programs asked to close; closing cannot be undone and is not part of the crash recovery", s.asked()),
                _ => L::t("off").to_string(),
            }
        ));
        line(L::t("IRQ devices: not changed"));
        let baseline = self.s().baseline_p99;
        line(&if measured {
            if game != 0 && baseline > 0.0 {
                lf!("Frame time: measured ({0}); baseline p99 {1:0.00} ms", self.frames.name(), baseline)
            } else {
                lf!("Frame time: measured ({0})", self.frames.name())
            }
        } else {
            let err = self.frames.error();
            lf!(
                "Frame time is not measured{0}",
                if !err.is_empty() {
                    format!(" ({})", err)
                } else if self.frames.available() {
                    format!(" ({})", L::t("the game sends no DXGI Present"))
                } else {
                    String::new()
                }
            )
        });
        let saved = self.e.export().procs.len();
        line(&lf!("Baseline: {0}", if saved > 0 { lf!("saved for {0} processes", self.e.export().procs.len()) } else { L::t("nothing changed").to_string() }));
        let last = self.s().last_revert_reason.clone();
        if !last.is_empty() {
            line(&lf!("Last revert: {0}", last));
        }
        line("");
        line(L::t("Honest limits:"));
        line(&format!("  {}", L::t("The ideal processor is a hint: the scheduler may ignore it.")));
        line(&format!("  {}", L::t("Thread classification is a heuristic; games without thread names are recognized worse and fall back to the mask.")));
        line(&format!("  {}", L::t("On X3D the V-Cache die is a statistical choice, not the truth for every game.")));
        line(&format!("  {}", L::t("On an ordinary Intel there is nothing to split.")));
        line(&format!("  {}", L::t("The goal is an even p99 frame time, not the average FPS; no average FPS gain is guaranteed.")));
        let tail = AppLog::tail(80);
        if !tail.is_empty() {
            line("");
            line(L::t("Log:"));
            for e in &tail {
                line(&format!("  {}", e));
            }
        }
        sb
    }

    /// One line for the Quick page: what is changed right now.
    pub fn summary(&self) -> String {
        let _g = self.gate.enter();
        let s = self.s();
        if matches!(s.state, AutoState::Idle | AutoState::Reverting | AutoState::Faulted) {
            return if !s.last_revert_reason.is_empty() {
                lf!("Nothing is changed. Last revert: {0}", &s.last_revert_reason)
            } else {
                L::t("Nothing is changed.").to_string()
            };
        }
        let mut parts = Vec::new();
        if s.game != 0 {
            parts.push(match &s.mask {
                Some(mp) if mp.satisfied => lf!("{0}: processors {1}", &s.game_exe, Masks::ranges(mp.mask)),
                _ => lf!("{0}: no mask", &s.game_exe),
            });
            parts.push(if s.pins { lf!("{0} threads placed", s.plan.len()) } else { L::t("threads not placed").to_string() });
        }
        drop(s);
        parts.push(lf!("background processes lowered: {0}", self.e.background_count()));
        parts.join(" · ")
    }

    fn state_text(&self) -> &'static str {
        match self.state() {
            AutoState::Idle => L::t("off"),
            AutoState::Armed => L::t("armed"),
            AutoState::Active => L::t("active"),
            AutoState::Reverting => L::t("reverting"),
            _ => L::t("fault"),
        }
    }
}

// ---- frame time (12.11): measured for the overview only -------------------------------------------------------------

fn p99(iv: &[f64]) -> f64 {
    if iv.is_empty() {
        return 0.0;
    }
    let mut s = iv.to_vec();
    s.sort_by(|a, b| a.total_cmp(b));
    s[(s.len() - 1).min((s.len() as f64 * 0.99).ceil() as usize - 1)]
}
