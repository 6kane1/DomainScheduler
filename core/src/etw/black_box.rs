//! Port of `BlackBox.cs`.
//!
//! The black box (1.16.1 Test 1): a flight recorder for the game. While Auto handles a game, Windows' own kernel trace
//! (context switches, ready threads, DPC/ISR) and the DXGI present events are kept in memory rings for the last
//! ~30 s; nothing is written to disk until a stutter happens. A stutter is a frame longer than 3x the median and
//! longer than two refreshes of the monitor. Then the frames and thread waits around it are explained in plain words,
//! with the same rules as Domain SchedTest's episode causes:
//!   other program - a main game thread (priority 8+) waited > 100 us for a core held by another program;
//!   driver        - a DPC/ISR > 100 us ran in the frame on a processor a main game thread used in that frame;
//!   cores         - a main game thread waited > 100 us for a core held by the game's own thread;
//!   game or GPU   - none of the above: the processor side was clean, the time went to the engine or the graphics card.
//! The report goes to the Black box page and to a small JSON file (last 50 kept). Nothing in the game or the system is
//! changed: the trace only reads.

use super::drivers::{driver_of, drivers};
use super::qpc;
use super::raw::{self, SessionSpec, DXGI_GUID, DXGI_U128, PERF_INFO_GUID, THREAD_GUID};
use super::stj;
use crate::base::app_log::AppLog;
use crate::base::ci::{self, CiSet, CiSetExt};
use crate::base::fmt::{num, Culture};
use crate::base::hooks::lock;
use crate::base::l::L;
use crate::base::time::{DateTime, TimeSpan};
use crate::lf;
use indexmap::IndexSet;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

/// `BlackBoxPolicy`.
pub struct BlackBoxPolicy;

impl BlackBoxPolicy {
    pub const HITCH_FACTOR: f64 = 3.0;
    pub const MAIN_PRIORITY: f64 = 8.0;
    pub const MAIN_WAIT_US: f64 = 100.0;
    pub const DPC_US: f64 = 100.0;
    pub const KEEP_DPC_US: f64 = 50.0;
    /// at most one report per 3 s (a burst of bad frames is one stutter)
    pub const MIN_GAP_MS: i32 = 3000;
    /// the analysis waits this long so the waits that end after the frame are in
    pub const AFTER_MS: i32 = 300;
    pub const KEEP_REPORTS: usize = 50;
    /// A frame this long is a load, a map change or the game window losing focus, not a stutter in play (Test 2).
    pub const LOADING_MS: f64 = 100.0;

    /// Windows' own processes: never "close it" (closing them restarts or breaks Windows).
    pub fn system_processes() -> &'static CiSet {
        static S: OnceLock<CiSet> = OnceLock::new();
        S.get_or_init(|| {
            let mut s = CiSet::new();
            for n in [
                "System", "Registry", "Secure System", "Memory Compression", "smss.exe", "csrss.exe", "wininit.exe", "winlogon.exe", "services.exe",
                "lsass.exe", "lsaiso.exe", "svchost.exe", "dwm.exe", "audiodg.exe", "fontdrvhost.exe", "MsMpEng.exe", "NisSrv.exe",
                "MpDefenderCoreService.exe", "SecurityHealthService.exe", "SearchIndexer.exe", "spoolsv.exe", "taskhostw.exe", "WmiPrvSE.exe",
                "TiWorker.exe", "TrustedInstaller.exe", "conhost.exe", "sihost.exe", "ctfmon.exe", "explorer.exe", "RuntimeBroker.exe", "dllhost.exe",
            ] {
                s.ci_add(n);
            }
            s
        })
    }

    /// A frame counts as a stutter: longer than 3x the median and than two refreshes (or 8 ms if the refresh is unknown).
    pub fn is_stutter(frame_ms: f64, median_ms: f64, refresh_hz: f64) -> bool {
        median_ms > 0.0 && frame_ms > Self::HITCH_FACTOR * median_ms && frame_ms > (if refresh_hz > 0.0 { 2000.0 / refresh_hz } else { 8.0 })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct BbWait {
    pub run: i64,
    pub ticks: i64,
    pub cpu: u16,
    pub tid: u32,
    pub priority: u8,
    pub old_tid: u32,
    pub old_pid: u32,
}

impl BbWait {
    pub fn new(run: i64, ticks: i64, cpu: u16, tid: u32, priority: u8, old_tid: u32, old_pid: u32) -> Self {
        BbWait { run, ticks, cpu, tid, priority, old_tid, old_pid }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct BbDpc {
    pub cpu: u16,
    pub start: i64,
    pub end: i64,
    pub routine: u64,
}

impl BbDpc {
    pub fn new(cpu: u16, start: i64, end: i64, routine: u64) -> Self {
        BbDpc { cpu, start, end, routine }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BbCause {
    Game,
    Cores,
    Driver,
    Other,
    Loading,
}

impl BbCause {
    /// `Cause.ToString()`.
    pub fn name(self) -> &'static str {
        match self {
            BbCause::Game => "Game",
            BbCause::Cores => "Cores",
            BbCause::Driver => "Driver",
            BbCause::Other => "Other",
            BbCause::Loading => "Loading",
        }
    }
}

/// One explained stutter.
#[derive(Clone, Debug, PartialEq)]
pub struct BbReport {
    pub when: DateTime,
    pub game: String,
    pub frame_ms: f64,
    pub median_ms: f64,
    pub bad_frames: i32,
    pub cause: BbCause,
    pub who: String,
    pub wait_us: f64,
    pub dpc_us: f64,
    pub main_waits: i32,
    pub text_ru: String,
    pub text_en: String,
    pub advice_ru: String,
    pub advice_en: String,
}

/// A fixed-size ring (overwrites the oldest). Not thread-safe by itself: the trace holds one lock.
pub struct Ring<T: Copy + Default> {
    a: Vec<T>,
    next: usize,
    full: bool,
}

impl<T: Copy + Default> Ring<T> {
    pub fn new(capacity: usize) -> Self {
        Ring { a: vec![T::default(); capacity], next: 0, full: false }
    }
    pub fn count(&self) -> usize {
        if self.full { self.a.len() } else { self.next }
    }
    pub fn add(&mut self, x: T) {
        self.a[self.next] = x;
        self.next += 1;
        if self.next == self.a.len() {
            self.next = 0;
            self.full = true;
        }
    }
    pub fn clear(&mut self) {
        self.next = 0;
        self.full = false;
    }
    /// The items in the order they were added.
    pub fn to_list(&self) -> Vec<T> {
        let mut l = Vec::with_capacity(self.count());
        if self.full {
            l.extend_from_slice(&self.a[self.next..]);
        }
        l.extend_from_slice(&self.a[..self.next]);
        l
    }
}

/// Finds stutters in the present times and explains them (pure: tests drive it with synthetic data).
pub struct BlackBoxAnalyzer;

impl BlackBoxAnalyzer {
    pub fn median(ms: &[f64]) -> f64 {
        if ms.is_empty() {
            return 0.0;
        }
        let mut s = ms.to_vec();
        s.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        s[s.len() / 2]
    }

    /// Explains the stutter that ends at present index `hitch`: frames are the present times (ticks), waits the game's
    /// thread waits, dpcs the long DPC/ISRs, game_pids the game's process ids.
    #[allow(clippy::too_many_arguments)]
    pub fn explain(
        game: &str,
        when: DateTime,
        presents: &[i64],
        hitch: usize,
        ticks_per_ms: f64,
        waits: &[BbWait],
        dpcs: &[BbDpc],
        game_pids: &IndexSet<u32>,
        name_of: &dyn Fn(u32) -> String,
        driver_of: &dyn Fn(u64) -> String,
        refresh_hz: f64,
    ) -> BbReport {
        let mut ms = Vec::new();
        for i in 1..presents.len() {
            ms.push((presents[i] - presents[i - 1]) as f64 / ticks_per_ms);
        }
        let median = Self::median(&ms);
        // the bad frames: this one and the stutters within 100 ms before it
        let mut bad: Vec<usize> = Vec::new();
        let mut i = hitch;
        while i >= 1 {
            if (presents[hitch] - presents[i]) as f64 / ticks_per_ms > 100.0 {
                break;
            }
            if BlackBoxPolicy::is_stutter(ms[i - 1], median, refresh_hz) || i == hitch {
                bad.push(i);
            }
            i -= 1;
        }
        let worst = bad.iter().map(|i| ms[i - 1]).fold(f64::NEG_INFINITY, f64::max);
        let (mut other, mut driver, mut cores) = (false, false, false);
        let mut who = String::new();
        let mut drv = String::new();
        let (mut wait_us, mut dpc_us) = (0.0f64, 0.0f64);
        let mut main_waits = 0i32;
        let us_per_tick = 1000.0 / ticks_per_ms;
        let main: Vec<&BbWait> = waits.iter().filter(|w| w.priority as f64 >= BlackBoxPolicy::MAIN_PRIORITY).collect();
        for &i in &bad {
            let (b, e) = (presents[i - 1], presents[i]);
            let mut cpus: IndexSet<u16> = IndexSet::new();
            for w in &main {
                if w.run >= b && w.run <= e {
                    cpus.insert(w.cpu);
                }
                let us = w.ticks as f64 * us_per_tick;
                if us <= BlackBoxPolicy::MAIN_WAIT_US || w.run - w.ticks > e || w.run < b {
                    continue;
                }
                main_waits += 1;
                let owner_game = game_pids.contains(&w.old_pid);
                if w.old_tid == 0 || w.old_pid == 0 {
                    continue; // the core was idle: woken from sleep
                }
                if !owner_game {
                    if us > wait_us || !other {
                        who = name_of(w.old_pid);
                    }
                    other = true;
                    wait_us = wait_us.max(us);
                } else {
                    cores = true;
                    if !other {
                        wait_us = wait_us.max(us);
                    }
                }
            }
            for d in dpcs {
                let us = (d.end - d.start) as f64 * us_per_tick;
                if us <= BlackBoxPolicy::DPC_US || d.start > e || d.end < b || !cpus.contains(&d.cpu) {
                    continue;
                }
                if us > dpc_us {
                    dpc_us = us;
                    drv = driver_of(d.routine);
                }
                driver = true;
            }
        }
        let cause = if worst >= BlackBoxPolicy::LOADING_MS {
            BbCause::Loading
        } else if other {
            BbCause::Other
        } else if driver {
            BbCause::Driver
        } else if cores {
            BbCause::Cores
        } else {
            BbCause::Game
        };
        let sys = BlackBoxPolicy::system_processes().ci_contains(&who);
        let ci = Culture::invariant();
        let head = num(worst, "0.0", &ci);
        let med = num(median, "0.0", &ci);
        let us_ru = |us: f64| if us >= 1000.0 { format!("{} мс", num(us / 1000.0, "0.0", &ci)) } else { format!("{} мкс", num(us, "0", &ci)) };
        let us_en = |us: f64| if us >= 1000.0 { format!("{} ms", num(us / 1000.0, "0.0", &ci)) } else { format!("{} µs", num(us, "0", &ci)) };
        let (ru, en, aru, aen): (String, String, String, String) = match cause {
            BbCause::Loading => (
                format!("Кадр шёл {} мс. Так долго игра стоит при загрузке, смене карты, подключении к серверу или когда окно игры теряет фокус: это не рывок в бою.", head),
                format!("The frame took {} ms. A game stops this long while loading, changing the map, connecting to a server or when its window loses focus: not a stutter in play.", head),
                "Ничего делать не нужно. Если такие паузы случаются посреди раунда, пришлите файл этого рывка.".into(),
                "Nothing to do. If such pauses happen in the middle of a round, send the file of this stutter.".into(),
            ),
            BbCause::Other if sys => (
                format!("Кадр шёл {} мс (обычно {} мс). Главный поток игры {} ждал ядро, которое занимал системный процесс Windows: {}.", head, med, us_ru(wait_us), who),
                format!("The frame took {} ms (usually {} ms). A main game thread waited {} for a core held by a Windows process: {}.", head, med, us_en(wait_us), who),
                format!("{} это часть Windows, закрывать его нельзя. Разовый всплеск бывает при входе, проверке безопасности или обслуживании. Если он повторяется в каждой игре, пришлите файлы рывков: разберём, что Windows делала в это время.", who),
                format!("{} is part of Windows and must not be closed. A one-off burst happens at sign-in, a security check or maintenance. If it repeats in every game, send the stutter files and we will look at what Windows was doing.", who),
            ),
            BbCause::Other => (
                format!("Кадр шёл {} мс (обычно {} мс). Главный поток игры {} ждал ядро, которое занимала другая программа: {}.", head, med, us_ru(wait_us), who),
                format!("The frame took {} ms (usually {} ms). A main game thread waited {} for a core held by another program: {}.", head, med, us_en(wait_us), who),
                format!("Закройте {} на время игры или проверьте, что он не делает работу в фоне (обновления, синхронизация, запись).", who),
                format!("Close {} while you play, or check that it does no background work (updates, sync, recording).", who),
            ),
            BbCause::Driver => (
                format!("Кадр шёл {} мс (обычно {} мс). На ядре главного потока игры драйвер {} работал {} подряд, и игра ждала.", head, med, drv, us_ru(dpc_us)),
                format!("The frame took {} ms (usually {} ms). On a core of the game's main thread the driver {} ran for {} in one go and the game waited.", head, med, drv, us_en(dpc_us)),
                Self::driver_advice_ru(&drv).into(),
                Self::driver_advice_en(&drv).into(),
            ),
            BbCause::Cores => (
                format!("Кадр шёл {} мс (обычно {} мс). Главный поток игры {} ждал ядро, занятое другими потоками самой игры: в этот момент игре не хватило свободных ядер.", head, med, us_ru(wait_us)),
                format!("The frame took {} ms (usually {} ms). A main game thread waited {} for a core held by the game's own threads: the game ran out of free cores at that moment.", head, med, us_en(wait_us)),
                "Если у Toolbox включена маска ядер, проверьте замером вариант без неё (Настройки, маска auto или off): на 8 ядрах всплески задач игры могут упираться в маску.".into(),
                "If Toolbox's core mask is on, measure the game without it (Settings, mask auto or off): on 8 cores the game's job bursts can hit the mask.".into(),
            ),
            BbCause::Game => (
                format!("Кадр шёл {} мс (обычно {} мс). Со стороны процессора всё было чисто: главные потоки игры не ждали ядер, драйверы не мешали. Время ушло на работу самой игры или видеокарты.", head, med),
                format!("The frame took {} ms (usually {} ms). The processor side was clean: the game's main threads did not wait for cores and no driver got in the way. The time went to the game itself or the graphics card.", head, med),
                "Это не система: так бывает при подгрузке, взрывах, дымах, смене сцены. Если такие рывки частые, проверьте температуру и частоты видеокарты и настройки графики.".into(),
                "Not the system: this happens on loading, explosions, smokes, scene changes. If they are frequent, check the graphics card's temperature and clocks and the graphics settings.".into(),
            ),
        };
        BbReport {
            when,
            game: game.to_string(),
            frame_ms: worst,
            median_ms: median,
            bad_frames: bad.len() as i32,
            cause,
            who: if cause == BbCause::Driver { drv } else { who },
            wait_us,
            dpc_us,
            main_waits,
            text_ru: ru,
            text_en: en,
            advice_ru: aru,
            advice_en: aen,
        }
    }

    fn gpu(d: &str) -> bool {
        ci::starts_with(d, "nvlddmkm") || ci::starts_with(d, "amdkmdag") || ci::starts_with(d, "atikmdag") || ci::starts_with(d, "igdkmd") || ci::starts_with(d, "dxgkrnl")
    }
    fn net(d: &str) -> bool {
        ci::starts_with(d, "ndis") || ci::starts_with(d, "tcpip") || ci::contains(d, "rt640") || ci::contains(d, "e1d") || ci::contains(d, "netwtw")
    }
    fn usb(d: &str) -> bool {
        ci::starts_with(d, "usb") || ci::starts_with(d, "hidclass") || ci::starts_with(d, "Wdf01000")
    }
    fn driver_advice_ru(d: &str) -> &'static str {
        if Self::gpu(d) {
            "Драйвер видеокарты: поставьте чистую последнюю версию драйвера. Toolbox уже старается держать игру подальше от ядер этого драйвера (живые DPC/ISR в настройках)."
        } else if Self::net(d) {
            "Сетевой драйвер: обновите драйвер сетевой карты с сайта производителя платы; на Wi-Fi такие задержки бывают чаще, чем по кабелю."
        } else if Self::usb(d) {
            "Драйвер USB: подключите мышь в порт прямо на материнской плате, без хабов; уберите лишние USB-устройства, обновите драйвер чипсета."
        } else {
            "Обновите этот драйвер с сайта производителя устройства или платы; если устройство не нужно для игры, его можно отключить в диспетчере устройств."
        }
    }
    fn driver_advice_en(d: &str) -> &'static str {
        if Self::gpu(d) {
            "Graphics driver: install the latest driver cleanly. Toolbox already keeps the game away from this driver's cores (live DPC/ISR in Settings)."
        } else if Self::net(d) {
            "Network driver: update the network card driver from the board maker's site; Wi-Fi has such delays more often than a cable."
        } else if Self::usb(d) {
            "USB driver: plug the mouse straight into the motherboard, without hubs; remove extra USB devices, update the chipset driver."
        } else {
            "Update this driver from the device or board maker's site; if the device is not needed for the game, you can disable it in Device Manager."
        }
    }
}

// ---- the recorder ---------------------------------------------------------------------------------------------------

const KERNEL_NAME: &str = "DomainScheduler-BlackBox";
const FRAMES_NAME: &str = "DomainScheduler-BlackBoxFrames";
const KERNEL_GUID: [u8; 16] = raw::guid_bytes(0xd4a1f6c2_58b3_4e7a_a0c9_2f6b8e1d7c35);
const FRAMES_GUID: [u8; 16] = raw::guid_bytes(0x7c2e9b14_a6d3_4f58_9e01_b3c84d2f6a97);
/// PROCESS | THREAD | CSWITCH | DPC | INTERRUPT | DISPATCHER
const K_FLAGS: u32 = 0x1 | 0x2 | 0x10 | 0x20 | 0x40 | 0x800;

struct State {
    /// ~30 s at 2000 FPS
    presents: Ring<i64>,
    /// the game's thread waits
    waits: Ring<BbWait>,
    /// DPC/ISR >= 50 us, any processor
    dpcs: Ring<BbDpc>,
    tid_pid: HashMap<u32, u32>,
    game_tids: IndexSet<u32>,
    ready: HashMap<u32, i64>,
    /// recent frame times for the running median
    recent: Vec<f64>,
    median: f64,
    since_median: i32,
    last_present: i64,
    last_report: i64,
    pending_at: i64,
    pending: i32,
    pid: u32,
    game: String,
    reports: Vec<BbReport>,
    refresh_hz: f64,
}

#[derive(Default)]
struct Run {
    k_s: u64,
    k_t: u64,
    f_s: u64,
    f_t: u64,
    k_th: Option<std::thread::JoinHandle<()>>,
    f_th: Option<std::thread::JoinHandle<()>>,
}

pub(crate) struct Inner {
    gate: Mutex<State>,
    tpm: f64,
    drivers: Mutex<Arc<Vec<(u64, String)>>>,
    running: AtomicBool,
    error: Mutex<String>,
    /// where report files go (None: not written)
    folder: Mutex<Option<PathBuf>>,
    run: Mutex<Run>,
}

/// `BlackBox` (the recorder; IDisposable: call `dispose`).
pub struct BlackBox {
    inner: Arc<Inner>,
}

impl Default for BlackBox {
    fn default() -> Self {
        Self::new()
    }
}

impl BlackBox {
    pub fn new() -> Self {
        BlackBox {
            inner: Arc::new(Inner {
                gate: Mutex::new(State {
                    presents: Ring::new(1 << 16),
                    waits: Ring::new(1 << 19),
                    dpcs: Ring::new(1 << 14),
                    tid_pid: HashMap::new(),
                    game_tids: IndexSet::new(),
                    ready: HashMap::new(),
                    recent: Vec::new(),
                    median: 0.0,
                    since_median: 0,
                    last_present: 0,
                    last_report: 0,
                    pending_at: 0,
                    pending: -1,
                    pid: 0,
                    game: String::new(),
                    reports: Vec::new(),
                    refresh_hz: 0.0,
                }),
                tpm: qpc::frequency() as f64 / 1000.0,
                drivers: Mutex::new(Arc::new(Vec::new())),
                running: AtomicBool::new(false),
                error: Mutex::new(String::new()),
                folder: Mutex::new(None),
                run: Mutex::new(Run::default()),
            }),
        }
    }

    pub fn running(&self) -> bool {
        self.inner.running.load(Ordering::SeqCst)
    }
    pub fn error(&self) -> String {
        lock(&self.inner.error).clone()
    }
    pub fn game_pid(&self) -> u32 {
        lock(&self.inner.gate).pid
    }
    pub fn refresh_hz(&self) -> f64 {
        lock(&self.inner.gate).refresh_hz
    }
    pub fn set_refresh_hz(&self, hz: f64) {
        lock(&self.inner.gate).refresh_hz = hz;
    }
    pub fn folder(&self) -> Option<PathBuf> {
        lock(&self.inner.folder).clone()
    }
    pub fn set_folder(&self, f: Option<PathBuf>) {
        *lock(&self.inner.folder) = f;
    }

    pub fn reports(&self) -> Vec<BbReport> {
        lock(&self.inner.gate).reports.clone()
    }

    pub fn start(&self, pid: u32, game: &str) {
        if self.running() && pid == self.game_pid() {
            return;
        }
        self.stop();
        {
            let mut st = lock(&self.inner.gate);
            st.pid = pid;
            st.game = game.to_string();
            st.presents.clear();
            st.waits.clear();
            st.dpcs.clear();
            st.ready.clear();
            st.game_tids.clear();
            st.tid_pid.clear();
            st.recent.clear();
            st.median = 0.0;
            st.last_present = 0;
            st.pending = -1;
            st.last_report = 0;
        }
        *lock(&self.inner.drivers) = Arc::new(drivers());
        let ok = self.start_sessions();
        self.inner.running.store(ok, Ordering::SeqCst);
        if ok {
            AppLog::add(lf!("Black box: recording {0}", game));
        }
    }

    pub fn stop(&self) {
        if !self.running() {
            return;
        }
        self.inner.running.store(false, Ordering::SeqCst);
        self.stop_sessions();
        AppLog::add(L::t("Black box: stopped"));
    }

    pub fn dispose(&self) {
        self.stop();
    }

    /// The refresh rate of the main monitor (Hz), 0 when Windows does not say.
    #[cfg(windows)]
    pub fn monitor_hz() -> f64 {
        use windows_sys::Win32::Graphics::Gdi::{EnumDisplaySettingsW, DEVMODEW, ENUM_CURRENT_SETTINGS};
        // SAFETY: DEVMODEW is plain data; dmSize set as C# does (220).
        unsafe {
            let mut dm: DEVMODEW = std::mem::zeroed();
            dm.dmSize = 220;
            if EnumDisplaySettingsW(std::ptr::null(), ENUM_CURRENT_SETTINGS, &mut dm) == 0 {
                return 0.0;
            }
            let hz = dm.dmDisplayFrequency;
            if hz > 1 && hz < 1000 { hz as f64 } else { 0.0 }
        }
    }

    #[cfg(not(windows))]
    pub fn monitor_hz() -> f64 {
        0.0
    }

    // ---- events (called on the trace threads) ------------------------------------------------------------------
    pub fn on_thread(&self, pid: u32, tid: u32) {
        self.inner.on_thread(pid, tid);
    }
    pub fn on_ready(&self, tid: u32, ts: i64) {
        self.inner.on_ready(tid, ts);
    }
    pub fn on_switch(&self, ts: i64, cpu: u16, new_tid: u32, new_prio: u8, old_tid: u32) {
        self.inner.on_switch(ts, cpu, new_tid, new_prio, old_tid);
    }
    pub fn on_dpc(&self, cpu: u16, start: i64, end: i64, routine: u64) {
        self.inner.on_dpc(cpu, start, end, routine);
    }
    pub fn on_present(&self, pid: u32, ts: i64) {
        self.inner.on_present(pid, ts);
    }

    /// Called about every 100-1000 ms by the hub: explains a stutter once its trailing data is in.
    pub fn tick(&self) -> Option<BbReport> {
        let tpm = self.inner.tpm;
        let (p, w, d, at, pids, game, hz) = {
            let mut st = lock(&self.inner.gate);
            if st.pending < 0 || ((qpc::timestamp() - st.pending_at) as f64 / tpm) < BlackBoxPolicy::AFTER_MS as f64 {
                return None;
            }
            st.pending = -1;
            let at = st.pending_at;
            let from = at - (2000.0 * tpm) as i64;
            let to = at + (500.0 * tpm) as i64;
            let p: Vec<i64> = st.presents.to_list().into_iter().filter(|x| *x >= from && *x <= to).collect();
            let w: Vec<BbWait> = st.waits.to_list().into_iter().filter(|x| x.run >= from && x.run <= to).collect();
            let d: Vec<BbDpc> = st.dpcs.to_list().into_iter().filter(|x| x.end >= from && x.start <= to).collect();
            let mut pids = IndexSet::new();
            pids.insert(st.pid);
            (p, w, d, at, pids, st.game.clone(), st.refresh_hz)
        };
        let hitch = p.iter().position(|x| *x == at)?;
        if hitch < 1 {
            return None;
        }
        let when = DateTime::now() - TimeSpan::from_ms((qpc::timestamp() - at) as f64 / tpm);
        let drivers = lock(&self.inner.drivers).clone();
        let r = BlackBoxAnalyzer::explain(&game, when, &p, hitch, tpm, &w, &d, &pids, &name_of, &|a| driver_of(&drivers, a), hz);
        {
            let mut st = lock(&self.inner.gate);
            st.reports.insert(0, r.clone());
            if st.reports.len() > BlackBoxPolicy::KEEP_REPORTS {
                st.reports.pop();
            }
        }
        self.save(&r, &p);
        Some(r)
    }

    fn save(&self, r: &BbReport, presents: &[i64]) {
        let Some(folder) = self.folder() else { return };
        let _ = (|| -> std::io::Result<()> {
            std::fs::create_dir_all(&folder)?;
            let tpm = self.inner.tpm;
            let mut frames = Vec::new();
            for i in 1..presents.len() {
                frames.push(round3((presents[i] - presents[i - 1]) as f64 / tpm));
            }
            let f = folder.join(format!("{}.json", r.when.format("yyyyMMdd-HHmmss-fff", &Culture::invariant())));
            std::fs::write(&f, report_json(r, &frames))?;
            let mut old: Vec<PathBuf> = std::fs::read_dir(&folder)?
                .filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| p.is_file() && p.extension().is_some_and(|x| ci::eq(&x.to_string_lossy(), "json")))
                .collect();
            old.sort();
            old.reverse();
            for o in old.into_iter().skip(BlackBoxPolicy::KEEP_REPORTS) {
                std::fs::remove_file(o)?;
            }
            Ok(())
        })();
    }

    fn start_sessions(&self) -> bool {
        raw::stop_by_name(KERNEL_NAME);
        raw::stop_by_name(FRAMES_NAME);
        let (rc, ks) = raw::start(KERNEL_NAME, &SessionSpec { guid: KERNEL_GUID, mode: 0x100 | 0x02000000, buffer_kb: 1024, min_buffers: 64, max_buffers: 512, flags: 0 });
        lock(&self.inner.run).k_s = ks;
        if rc != 0 {
            *lock(&self.inner.error) = if rc == 5 { L::t("no administrator rights").to_string() } else { lf!("the kernel trace did not start (error {0})", rc) };
            return false;
        }
        let r2 = raw::set_flags(ks, K_FLAGS);
        if r2 != 0 {
            raw::stop_by_name(KERNEL_NAME);
            *lock(&self.inner.error) = lf!("the kernel trace did not start (error {0})", r2);
            return false;
        }
        let ctx = Arc::as_ptr(&self.inner) as *const core::ffi::c_void;
        let kt = raw::open(KERNEL_NAME, on_kernel, ctx);
        lock(&self.inner.run).k_t = kt;
        let (frc, fs) = raw::start(FRAMES_NAME, &SessionSpec { guid: FRAMES_GUID, mode: 0x100, buffer_kb: 64, min_buffers: 16, max_buffers: 128, flags: 0 });
        if frc == 0 {
            lock(&self.inner.run).f_s = fs;
            raw::enable(fs, DXGI_U128, 4, u64::MAX, &[]);
            let ft = raw::open(FRAMES_NAME, on_frames, ctx);
            lock(&self.inner.run).f_t = ft;
        }
        let ft = lock(&self.inner.run).f_t;
        let kth = raw::spawn_process("blackbox-k", -1, Some(L::n("Black box trace")), kt, self.inner.clone());
        lock(&self.inner.run).k_th = kth;
        if ft != 0 && ft != u64::MAX {
            let fth = raw::spawn_process("blackbox-f", -1, Some(L::n("Black box frames")), ft, self.inner.clone());
            lock(&self.inner.run).f_th = fth;
        }
        // Running is set before the flusher starts (C#: Running = true; _flush.Start()).
        self.inner.running.store(true, Ordering::SeqCst);
        let inner = self.inner.clone();
        let _ = std::thread::Builder::new().name("blackbox-flush".into()).spawn(move || {
            while inner.running.load(Ordering::SeqCst) {
                std::thread::sleep(std::time::Duration::from_millis(250));
                raw::flush(FRAMES_NAME);
            }
        });
        true
    }

    fn stop_sessions(&self) {
        raw::stop_by_name(KERNEL_NAME);
        raw::stop_by_name(FRAMES_NAME);
        let (kt, ft, kth, fth) = {
            let mut r = lock(&self.inner.run);
            (r.k_t, r.f_t, r.k_th.take(), r.f_th.take())
        };
        if kt != 0 && kt != u64::MAX {
            raw::close(kt);
        }
        if ft != 0 && ft != u64::MAX {
            raw::close(ft);
        }
        raw::join_timeout(kth, 2000);
        raw::join_timeout(fth, 2000);
        let mut r = lock(&self.inner.run);
        r.k_t = 0;
        r.f_t = 0;
    }

    /// A session left by a killed copy of the program is stopped (called once at start).
    pub fn stop_orphan() {
        raw::stop_by_name(KERNEL_NAME);
        raw::stop_by_name(FRAMES_NAME);
    }
}

/// `Math.Round(v, 3)` (banker's rounding, as .NET's default MidpointRounding.ToEven).
fn round3(v: f64) -> f64 {
    if v.abs() >= 1e16 {
        return v;
    }
    (v * 1000.0).round_ties_even() / 1000.0
}

/// The report file, as `JsonSerializer.Serialize(new { r.When, r.Game, ..., FramesMs = frames })` writes it.
fn report_json(r: &BbReport, frames: &[f64]) -> String {
    let fr: Vec<String> = frames.iter().map(|f| stj::double(*f)).collect();
    format!(
        "{{\"When\":{},\"Game\":{},\"FrameMs\":{},\"MedianMs\":{},\"BadFrames\":{},\"Cause\":{},\"Who\":{},\"WaitUs\":{},\"DpcUs\":{},\"MainWaits\":{},\"TextRu\":{},\"TextEn\":{},\"AdviceRu\":{},\"AdviceEn\":{},\"FramesMs\":[{}]}}",
        stj::date(r.when),
        stj::string(&r.game),
        stj::double(r.frame_ms),
        stj::double(r.median_ms),
        r.bad_frames,
        stj::string(r.cause.name()),
        stj::string(&r.who),
        stj::double(r.wait_us),
        stj::double(r.dpc_us),
        r.main_waits,
        stj::string(&r.text_ru),
        stj::string(&r.text_en),
        stj::string(&r.advice_ru),
        stj::string(&r.advice_en),
        fr.join(",")
    )
}

/// `NameOf(pid)`: the process's name + ".exe" (`Process.ProcessName` has no ".exe"), cached; "PID n" when it is gone
/// ("System" for pid 4).
fn name_of(pid: u32) -> String {
    static NAMES: OnceLock<Mutex<HashMap<u32, String>>> = OnceLock::new();
    let names = NAMES.get_or_init(|| Mutex::new(HashMap::new()));
    let mut g = lock(names);
    if let Some(n) = g.get(&pid) {
        return n.clone();
    }
    let n = match process_name(pid) {
        Some(p) => p + ".exe",
        None => {
            if pid == 4 {
                "System".to_string()
            } else {
                format!("PID {}", pid)
            }
        }
    };
    if g.len() > 4000 {
        g.clear();
    }
    g.insert(pid, n.clone());
    n
}

/// `Process.GetProcessById(pid).ProcessName`: the image name without a ".exe" ending; None when there is no such process.
#[cfg(windows)]
fn process_name(pid: u32) -> Option<String> {
    if pid == 0 {
        return Some("Idle".into());
    }
    let procs = crate::native::win::processes();
    let (name, _) = procs.get(&pid)?;
    Some(strip_exe(name))
}

#[cfg(not(windows))]
fn process_name(_pid: u32) -> Option<String> {
    None
}

#[allow(dead_code)]
fn strip_exe(name: &str) -> String {
    if name.len() >= 4 && ci::ends_with(name, ".exe") { name[..name.len() - 4].to_string() } else { name.to_string() }
}

impl Inner {
    fn on_thread(&self, pid: u32, tid: u32) {
        let mut st = lock(&self.gate);
        st.tid_pid.insert(tid, pid);
        if pid == st.pid {
            st.game_tids.insert(tid);
        }
    }

    fn on_ready(&self, tid: u32, ts: i64) {
        let mut st = lock(&self.gate);
        if st.game_tids.contains(&tid) {
            st.ready.entry(tid).or_insert(ts);
        }
    }

    fn on_switch(&self, ts: i64, cpu: u16, new_tid: u32, new_prio: u8, old_tid: u32) {
        let mut st = lock(&self.gate);
        if new_tid != 0 {
            if let Some(at) = st.ready.remove(&new_tid) {
                if ts >= at {
                    let old_pid = if old_tid == 0 { 0 } else { st.tid_pid.get(&old_tid).copied().unwrap_or(0) };
                    st.waits.add(BbWait::new(ts, ts - at, cpu, new_tid, new_prio, old_tid, old_pid));
                }
            }
        }
    }

    fn on_dpc(&self, cpu: u16, start: i64, end: i64, routine: u64) {
        if (end - start) as f64 / self.tpm * 1000.0 < BlackBoxPolicy::KEEP_DPC_US {
            return;
        }
        lock(&self.gate).dpcs.add(BbDpc::new(cpu, start, end, routine));
    }

    fn on_present(&self, pid: u32, ts: i64) {
        let mut st = lock(&self.gate);
        if pid != st.pid {
            return;
        }
        st.presents.add(ts);
        if st.last_present != 0 {
            let ms = (ts - st.last_present) as f64 / self.tpm;
            st.recent.push(ms);
            if st.recent.len() > 600 {
                st.recent.remove(0);
            }
            st.since_median += 1;
            if st.since_median >= 120 || st.median == 0.0 {
                st.median = BlackBoxAnalyzer::median(&st.recent);
                st.since_median = 0;
            }
            if st.recent.len() >= 120
                && st.pending < 0
                && BlackBoxPolicy::is_stutter(ms, st.median, st.refresh_hz)
                && ((ts - st.last_report) as f64 / self.tpm) > BlackBoxPolicy::MIN_GAP_MS as f64
            {
                st.pending = 1;
                st.pending_at = ts;
                st.last_report = ts;
            }
        }
        st.last_present = ts;
    }
}

unsafe extern "system" fn on_kernel(record: *mut core::ffi::c_void) {
    let r = raw::rec(record as *const u8);
    if r.context.is_null() {
        return;
    }
    let inner = &*(r.context as *const Inner);
    let (op, len, u) = (r.opcode, r.len, r.user);
    if u.is_null() {
        return;
    }
    let ts = r.stamp;
    let cpu = r.cpu;
    if r.provider == THREAD_GUID {
        match op {
            1 | 3 => {
                if len >= 8 {
                    inner.on_thread(raw::rd(u, 0), raw::rd(u, 4)); // Start / DCStart: ProcessId, TThreadId
                }
            }
            50 => {
                if len >= 4 {
                    inner.on_ready(raw::rd(u, 0), ts); // ReadyThread
                }
            }
            36 => {
                if len >= 10 {
                    inner.on_switch(ts, cpu, raw::rd(u, 0), raw::rd(u, 8), raw::rd(u, 4)); // CSwitch: New, Old, NewPrio
                }
            }
            _ => {}
        }
        return;
    }
    if r.provider == PERF_INFO_GUID && (op == 66 || op == 67 || op == 68 || op == 69 || op == 70 || op == 50) && len >= 16 {
        inner.on_dpc(cpu, raw::rd(u, 0), ts, raw::rd(u, 8)); // InitialTime, Routine
    }
}

unsafe extern "system" fn on_frames(record: *mut core::ffi::c_void) {
    let r = raw::rec(record as *const u8);
    if r.context.is_null() || r.provider != DXGI_GUID {
        return;
    }
    if r.id != 42 {
        return;
    }
    let inner = &*(r.context as *const Inner);
    inner.on_present(r.pid, r.stamp);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 10 MHz clock: 10 000 ticks per ms
    const TPM: f64 = 10_000.0;
    const GAME: u32 = 100;
    const STEAM: u32 = 900;

    fn frames(hitch_ms: f64) -> (Vec<i64>, usize, i64, i64) {
        let mut p = Vec::new();
        let mut t: i64 = 0;
        for _ in 0..300 {
            t += (1.4 * TPM) as i64;
            p.push(t);
        }
        let b = t;
        t += (hitch_ms * TPM) as i64;
        p.push(t);
        let h = p.len() - 1;
        for _ in 0..100 {
            t += (1.4 * TPM) as i64;
            p.push(t);
        }
        let e = p[h];
        (p, h, b, e)
    }

    fn explain(p: &[i64], h: usize, w: &[BbWait], d: &[BbDpc]) -> BbReport {
        let mut pids = IndexSet::new();
        pids.insert(GAME);
        BlackBoxAnalyzer::explain(
            "cs2.exe",
            DateTime::now(),
            p,
            h,
            TPM,
            w,
            d,
            &pids,
            &|pid| (if pid == STEAM { "steamwebhelper.exe" } else if pid == 800 { "lsass.exe" } else { "?" }).to_string(),
            &|_| "nvlddmkm.sys".to_string(),
            240.0,
        )
    }

    #[test]
    fn stutter_needs_three_times_the_median_and_two_refreshes() {
        assert!(BlackBoxPolicy::is_stutter(9.0, 1.4, 240.0));
        assert!(!BlackBoxPolicy::is_stutter(4.0, 1.4, 240.0)); // 3x median but less than two refreshes (8.3 ms)
        assert!(!BlackBoxPolicy::is_stutter(9.0, 4.0, 240.0)); // not 3x the median
        assert!(BlackBoxPolicy::is_stutter(9.0, 2.0, 0.0)); // refresh unknown: 8 ms
    }

    #[test]
    fn clean_processor_side_is_game_or_gpu() {
        let (p, h, b, _) = frames(12.0);
        let w = vec![BbWait::new(b + (2.0 * TPM) as i64, (0.002 * TPM) as i64, 3, 11, 12, 0, 0)];
        let r = explain(&p, h, &w, &[]);
        assert_eq!(BbCause::Game, r.cause);
        assert!((r.frame_ms - 12.0).abs() < 0.05);
        assert!(r.text_ru.contains("игры или видеокарты"));
    }

    #[test]
    fn another_program_holding_the_core_is_named() {
        let (p, h, b, _) = frames(12.0);
        let w = vec![BbWait::new(b + (5.0 * TPM) as i64, (1.5 * TPM) as i64, 3, 11, 12, 77, STEAM)];
        let r = explain(&p, h, &w, &[]);
        assert_eq!(BbCause::Other, r.cause);
        assert_eq!("steamwebhelper.exe", r.who);
        assert!(r.text_ru.contains("steamwebhelper.exe"));
        assert_eq!(1500.0, r.wait_us.round());
    }

    #[test]
    fn driver_counts_only_on_a_core_the_game_used() {
        let (p, h, b, _) = frames(12.0);
        let w = vec![BbWait::new(b + (4.0 * TPM) as i64, (0.002 * TPM) as i64, 5, 11, 12, 0, 0)]; // main thread ran on cpu 5
        let off = explain(&p, h, &w, &[BbDpc::new(14, b + (2.0 * TPM) as i64, b + (2.5 * TPM) as i64, 1)]);
        assert_eq!(BbCause::Game, off.cause); // DPC on a device core
        let on = explain(&p, h, &w, &[BbDpc::new(5, b + (2.0 * TPM) as i64, b + (2.5 * TPM) as i64, 1)]);
        assert_eq!(BbCause::Driver, on.cause);
        assert_eq!("nvlddmkm.sys", on.who);
        assert!(on.advice_ru.contains("видеокарты"));
    }

    #[test]
    fn the_games_own_threads_mean_not_enough_cores_and_background_threads_never_decide() {
        let (p, h, b, _) = frames(12.0);
        let cores = explain(&p, h, &[BbWait::new(b + (5.0 * TPM) as i64, (0.8 * TPM) as i64, 3, 11, 12, 55, GAME)], &[]);
        assert_eq!(BbCause::Cores, cores.cause);
        let bg = explain(&p, h, &[BbWait::new(b + (5.0 * TPM) as i64, (3.0 * TPM) as i64, 3, 11, 2, 77, STEAM)], &[]); // priority 2: a worker
        assert_eq!(BbCause::Game, bg.cause);
    }

    #[test]
    fn ring_keeps_the_newest_in_order() {
        let mut r: Ring<i32> = Ring::new(4);
        for i in 1..=6 {
            r.add(i);
        }
        assert_eq!(vec![3, 4, 5, 6], r.to_list());
        r.clear();
        r.add(9);
        assert_eq!(vec![9], r.to_list());
    }

    #[test]
    fn long_pauses_are_loading_and_windows_processes_are_never_told_to_close() {
        let (p, h, b, _) = frames(210.0);
        let r = explain(&p, h, &[BbWait::new(b + (5.0 * TPM) as i64, (20.0 * TPM) as i64, 3, 11, 12, 77, STEAM)], &[]);
        assert_eq!(BbCause::Loading, r.cause); // a 210 ms frame: load or map change
        let (p2, h2, b2, _) = frames(12.0);
        let sys = explain(&p2, h2, &[BbWait::new(b2 + (5.0 * TPM) as i64, (1.5 * TPM) as i64, 3, 11, 12, 66, 800)], &[]);
        assert_eq!(BbCause::Other, sys.cause);
        assert_eq!("lsass.exe", sys.who);
        assert!(!sys.advice_ru.contains("Закройте"));
        assert!(sys.advice_ru.contains("закрывать его нельзя"));
    }

    #[test]
    fn the_recorder_flags_a_stutter_and_writes_its_file() {
        let bb = BlackBox::new();
        let tpm = bb.inner.tpm;
        lock(&bb.inner.gate).pid = GAME;
        bb.on_thread(GAME, 11);
        let base = qpc::timestamp() - (5000.0 * tpm) as i64;
        let mut t = base;
        for _ in 0..200 {
            t += (2.0 * tpm) as i64;
            bb.on_present(GAME, t);
        }
        let hitch_at = t + (30.0 * tpm) as i64;
        bb.on_present(GAME, hitch_at);
        bb.on_present(GAME + 1, hitch_at + 10); // another process: ignored
        assert_eq!(lock(&bb.inner.gate).pending, 1);
        let dir = std::env::temp_dir().join(format!("ds-bb-{}", std::process::id()));
        bb.set_folder(Some(dir.clone()));
        let r = bb.tick().expect("a report");
        assert_eq!(r.cause, BbCause::Game);
        assert!((r.frame_ms - 30.0).abs() < 0.01);
        assert_eq!(bb.reports().len(), 1);
        let files: Vec<_> = std::fs::read_dir(&dir).unwrap().collect();
        assert_eq!(files.len(), 1);
        let text = std::fs::read_to_string(files[0].as_ref().unwrap().path()).unwrap();
        let v: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["Cause"], "Game");
        assert_eq!(v["Game"], "");
        assert!(v["TextRu"].as_str().unwrap().starts_with("Кадр шёл 30.0 мс"));
        assert!(text.contains("\\u041A")); // escaped like System.Text.Json
        let _ = std::fs::remove_dir_all(&dir);
        assert!(bb.tick().is_none());
    }

    #[test]
    fn rounding_and_names() {
        assert_eq!(round3(1.4), 1.4);
        assert_eq!(round3(1.23456), 1.235);
        assert_eq!(strip_exe("svchost.EXE"), "svchost");
        assert_eq!(strip_exe("System"), "System");
    }
}
