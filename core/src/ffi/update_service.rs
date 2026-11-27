//! Port of UpdateService.cs (and the decisions of its UpdateCard): update checks one minute after start and then
//! once a day (an hour after a failed check), only while the setting "CheckUpdates" is on (default on). Nothing is
//! installed by itself: the home page and the About page show a card, the tray says it once per version, and the
//! person presses "Update". While a game is running (Auto active) nothing is shown or installed.
//! C# ran the state on the UI thread; here it is one lock, and `Changed` is a state version bump.

use super::state;
use crate::base::app_log::{AppLog, Sev};
use crate::base::hooks::lock;
use crate::base::l::L;
use crate::base::settings::Settings;
use crate::base::time::{DateTime, TimeSpan};
use crate::lf;
use crate::system::updater::{AppVersion, TesterKey, UpdateCheck, UpdateFile, Updater};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

/// The playbook's download page on the website (the language Toolbox speaks).
pub const SITE_EN: &str = "https://6kane1.github.io/DomainOS/index.html#download";
pub const SITE_RU: &str = "https://6kane1.github.io/DomainOS/ru/index.html#download";

/// `App.InstallUpdate(content, version, failed)`.
pub type InstallFn = dyn Fn(Vec<u8>, String, Box<dyn Fn(String) + Send + Sync>) + Send + Sync;

#[derive(Default)]
struct St {
    last: Option<UpdateCheck>,
    last_at: Option<DateTime>,
    checking: bool,
    busy: Option<String>,
    error: Option<String>,
    playbook_file: Option<String>,
    poked_in_game: bool,
}

pub struct UpdateService {
    st: Mutex<St>,
    in_game: Arc<dyn Fn() -> bool + Send + Sync>,
    notice: Arc<dyn Fn(&str) + Send + Sync>,
    install: Arc<InstallFn>,
}

static INSTANCE: OnceLock<Arc<UpdateService>> = OnceLock::new();

fn fire() {
    state::bump();
}

impl UpdateService {
    /// `UpdateService.Instance`.
    pub fn instance() -> Option<Arc<UpdateService>> {
        INSTANCE.get().cloned()
    }

    pub fn enabled() -> bool {
        Settings::get_bool_or("CheckUpdates", true)
    }

    /// Creates the instance and its timer (first tick after a minute, then every 10 minutes).
    pub fn new(in_game: Arc<dyn Fn() -> bool + Send + Sync>, notice: Arc<dyn Fn(&str) + Send + Sync>, install: Arc<InstallFn>, start_timer: bool) -> Arc<UpdateService> {
        let s = Arc::new(UpdateService { st: Mutex::new(St::default()), in_game, notice, install });
        let _ = INSTANCE.set(s.clone());
        if start_timer {
            let me = Arc::downgrade(&s);
            let _ = std::thread::Builder::new().name("update-timer".into()).spawn(move || {
                crate::native::self_threads::SelfThreads::register(crate::base::l::L::n("update timer"));
                let mut every = Duration::from_secs(60);
                loop {
                    std::thread::sleep(every);
                    every = Duration::from_secs(600);
                    let Some(s) = me.upgrade() else { return };
                    s.tick();
                }
            });
        }
        s
    }

    pub fn in_game(&self) -> bool {
        (self.in_game)()
    }

    pub fn checking(&self) -> bool {
        lock(&self.st).checking
    }

    fn tick(self: &Arc<Self>) {
        if !Self::enabled() || self.checking() {
            return;
        }
        // A failed check (no network yet, the site down) is tried again after an hour, not a day.
        let (due, _) = {
            let st = lock(&self.st);
            let every = if st.last.as_ref().and_then(|l| l.error.as_ref()).is_some() { TimeSpan::from_hours(1.0) } else { TimeSpan::from_hours(24.0) };
            let due = match st.last_at {
                None => true,
                Some(at) => DateTime::now() - at >= every,
            };
            (due, ())
        };
        if due {
            self.check_now();
        } else {
            self.notify_if_due();
        }
    }

    pub fn check_now(self: &Arc<Self>) {
        {
            let mut st = lock(&self.st);
            if st.checking {
                return;
            }
            st.checking = true;
            st.error = None;
        }
        fire();
        let me = self.clone();
        let _ = std::thread::Builder::new().name("update-check".into()).spawn(move || {
            let r = Updater::check();
            {
                let mut st = lock(&me.st);
                st.checking = false;
                st.last_at = Some(DateTime::now());
                if let Some(e) = &r.error {
                    st.error = Some(e.clone());
                    AppLog::add_sev(lf!("Update check: {0}", e), Sev::Info);
                }
                st.last = Some(r);
            }
            fire();
            me.notify_if_due();
        });
    }

    /// The tray says it once per version (Toolbox and the playbook each), and never during a game.
    fn notify_if_due(&self) {
        if self.in_game() {
            return;
        }
        let (t, p) = {
            let st = lock(&self.st);
            (st.last.as_ref().and_then(|l| l.toolbox.clone()), st.last.as_ref().and_then(|l| l.playbook.clone()))
        };
        if let Some(t) = t {
            if Settings::get("UpdateNotified") != t.version {
                let _ = Settings::set("UpdateNotified", &t.version);
                (self.notice)(&lf!("Toolbox {0} is available. Open the program to see what is new.", AppVersion::describe(Some(&t.version))));
            }
        }
        if let Some(p) = p {
            if Settings::get("PlaybookNotified") != p.version {
                let _ = Settings::set("PlaybookNotified", &p.version);
                (self.notice)(&lf!("Domain OS playbook {0} is out. Download it from the website or from Toolbox's home page.", &p.version));
            }
        }
    }

    fn close_game_first() -> String {
        L::t("Close the game first: Toolbox does not update while a game is running.").to_string()
    }

    pub fn install_toolbox(self: &Arc<Self>) {
        let t = {
            let st = lock(&self.st);
            let t = st.last.as_ref().and_then(|l| l.toolbox.clone());
            if t.is_none() || st.busy.is_some() {
                return;
            }
            t
        };
        let Some(t) = t else { return };
        if self.in_game() {
            lock(&self.st).error = Some(Self::close_game_first());
            fire();
            return;
        }
        {
            let mut st = lock(&self.st);
            st.busy = Some(L::t("Downloading…").to_string());
            st.error = None;
        }
        fire();
        let me = self.clone();
        let _ = std::thread::Builder::new().name("update-download".into()).spawn(move || {
            let m2 = me.clone();
            let progress = move |p: f64| {
                lock(&m2.st).busy = Some(lf!("Downloading {0} %", (p * 100.0) as i32));
                fire();
            };
            // Checked in memory; these exact bytes become the program file (nothing is read back from disk).
            match Updater::download_bytes(&t, Some(&progress)) {
                Ok(content) => {
                    if me.in_game() {
                        let mut st = lock(&me.st);
                        st.busy = None;
                        st.error = Some(Self::close_game_first());
                        drop(st);
                        fire();
                        return;
                    }
                    lock(&me.st).busy = Some(L::t("Installing…").to_string());
                    fire();
                    let m3 = me.clone();
                    (me.install)(
                        content,
                        t.version.clone(),
                        Box::new(move |err: String| {
                            let mut st = lock(&m3.st);
                            st.busy = None;
                            st.error = Some(err);
                            drop(st);
                            fire();
                        }),
                    );
                }
                Err(e) => {
                    {
                        let mut st = lock(&me.st);
                        st.busy = None;
                        st.error = Some(e.clone());
                    }
                    AppLog::add_sev(lf!("Update: {0}", e), Sev::Warn);
                    fire();
                }
            }
        });
    }

    /// The playbook is downloaded and checked, then shown in Explorer: AME Wizard installs it.
    pub fn download_playbook(self: &Arc<Self>) {
        let p = {
            let st = lock(&self.st);
            let p = st.last.as_ref().and_then(|l| l.playbook.clone());
            if p.is_none() || st.busy.is_some() {
                return;
            }
            p
        };
        let Some(p) = p else { return };
        {
            let mut st = lock(&self.st);
            st.busy = Some(L::t("Downloading…").to_string());
            st.error = None;
        }
        fire();
        let me = self.clone();
        let _ = std::thread::Builder::new().name("playbook-download".into()).spawn(move || {
            let m2 = me.clone();
            let progress = move |x: f64| {
                lock(&m2.st).busy = Some(lf!("Downloading {0} %", (x * 100.0) as i32));
                fire();
            };
            let r = Updater::download_bytes(&p, Some(&progress))
                .and_then(|data| Updater::write_new(&data, &format!("Domain-OS-{}.apbx", p.version), None).map_err(|e| e.message().to_string()));
            match r {
                Ok(file) => {
                    let f = file.to_string_lossy().into_owned();
                    {
                        let mut st = lock(&me.st);
                        st.busy = None;
                        st.playbook_file = Some(f.clone());
                    }
                    fire();
                    Self::show_file(&f);
                }
                Err(e) => {
                    let mut st = lock(&me.st);
                    st.busy = None;
                    st.error = Some(e);
                    drop(st);
                    fire();
                }
            }
        });
    }

    pub fn show_file(file: &str) {
        let explorer = std::path::Path::new(&super::win::windows_dir()).join("explorer.exe");
        let _ = std::process::Command::new(explorer).arg(format!("/select,\"{}\"", file)).spawn();
    }

    pub fn playbook_file(&self) -> Option<String> {
        lock(&self.st).playbook_file.clone()
    }

    /// Auto changed: the cards are redrawn only when a game started or ended (they hide or show the Update button).
    pub fn poke(&self) {
        let g = self.in_game();
        {
            let mut st = lock(&self.st);
            if g == st.poked_in_game {
                return;
            }
            st.poked_in_game = g;
        }
        fire();
    }

    fn notes(f: &UpdateFile) -> String {
        let n = if L::lang() == "ru" && !f.notes_ru.is_empty() { &f.notes_ru } else { &f.notes_en };
        let chars: Vec<char> = n.chars().collect();
        // C# n[..600] counts UTF-16 units; the notes are plain text where both agree.
        if n.encode_utf16().count() > 600 {
            let mut out = String::new();
            let mut units = 0;
            for c in chars {
                if units + c.len_utf16() > 600 {
                    break;
                }
                units += c.len_utf16();
                out.push(c);
            }
            out + "…"
        } else {
            n.clone()
        }
    }

    /// UpdateCard.Refresh as data (see dos_core.h, get "update"). `prev`: the ".prev" file exists.
    pub fn card(&self, prev: bool) -> Value {
        let in_game = self.in_game();
        let st = lock(&self.st);
        let t = st.last.as_ref().and_then(|l| l.toolbox.clone());
        let p = st.last.as_ref().and_then(|l| l.playbook.clone());
        let chan = if AppVersion::is_test() { L::t("Test: every new test build and every stable release") } else { L::t("Stable: only checked releases") };
        let toolbox = t.as_ref().map(|t| {
            json!({
                "title": lf!("Toolbox {0} is available", AppVersion::describe(Some(&t.version))),
                "notes": Self::notes(t),
                "can_update": st.busy.is_none() && !in_game,
                "note": if in_game { Self::close_game_first() } else { L::t("Toolbox puts back what it changed, closes and starts the new version. If the new one does not start, the previous version comes back by itself.").to_string() },
            })
        });
        let playbook = p.as_ref().map(|p| {
            json!({
                "title": lf!("Domain OS playbook {0} is available", &p.version),
                "notes": Self::notes(p),
                "file": st.playbook_file,
                "note": if st.playbook_file.is_some() {
                    L::t("Downloaded and checked. Open it with AME Wizard and press Install, the same way as the first time.")
                } else {
                    L::t("The playbook is not installed by Toolbox: download the file here (it is checked) or from the website, then install it with AME Wizard.")
                },
            })
        });
        let when = if st.checking {
            L::t("Checking…").to_string()
        } else {
            match st.last_at {
                None => L::t("Not checked yet").to_string(),
                Some(at) => {
                    if t.is_none() && p.is_none() && st.error.is_none() {
                        lf!("Up to date · checked {0:t}", at)
                    } else {
                        lf!("Checked {0:t}", at)
                    }
                }
            }
        };
        json!({
            "show_home": (t.is_some() || p.is_some()) && !in_game,
            "this_text": lf!("This is {0} · channel {1}", AppVersion::display(), chan),
            "toolbox": toolbox,
            "playbook": playbook,
            "busy": st.busy,
            "error": st.error.as_ref().map(|e| L::t(e).to_string()),
            "when": when,
            "prev": prev,
            "in_game": in_game,
            "enabled": Self::enabled(),
        })
    }
}

/// UpdatePage.TesterCard's Show(): the key, the channel and the state text.
pub fn tester_json() -> Value {
    let key = TesterKey::current().is_some();
    let on = AppVersion::channel() == "test";
    let state = if on {
        L::t("Test channel on: this PC receives the test builds.")
    } else if key {
        L::t("Key saved, stable releases only: test builds are not offered. A test build already installed stays until a newer stable release comes out.")
    } else {
        L::t("No key: stable channel.")
    };
    json!({
        "key": key,
        "on": on,
        "state": state,
        "channel_visible": key,
        "channel_button": if on { L::t("Stable releases only") } else { L::t("Receive test builds") },
    })
}
