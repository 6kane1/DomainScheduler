//! Port of AutoCommand.cs: "--auto on|off|status" (1.16.0-test1.3), for Domain Bench: alternating measurements
//! without a person at the PC. The running Toolbox mirrors Auto into two manual-reset events
//! (Local\DomainScheduler.Auto.IsOn / .IsOff) and listens for two requests (Local\DomainScheduler.Auto.TurnOn /
//! .TurnOff). A request does exactly what the Auto button does, so everything Auto changes is reverted by Auto itself.
//! Session-local names: only programs of the same signed-in session can reach them, and an elevated Toolbox's objects
//! are not writable from a non-elevated one.
//! Exit codes of the command: 0 Auto on, 1 Auto off, 2 no answer in time, 3 Toolbox is not running, 4 unknown argument.

use super::win;
use crate::base::app_log::{AppLog, Sev};
use std::sync::Arc;
use std::time::{Duration, Instant};

pub const IS_ON_NAME: &str = "Local\\DomainScheduler.Auto.IsOn";
pub const IS_OFF_NAME: &str = "Local\\DomainScheduler.Auto.IsOff";
pub const ON_NAME: &str = "Local\\DomainScheduler.Auto.TurnOn";
pub const OFF_NAME: &str = "Local\\DomainScheduler.Auto.TurnOff";

pub struct AutoCommand {
    is_on: win::H,
    is_off: win::H,
    on: Arc<dyn Fn() -> bool + Send + Sync>,
}

impl AutoCommand {
    pub fn new(on: Arc<dyn Fn() -> bool + Send + Sync>, turn_on: Arc<dyn Fn() + Send + Sync>, turn_off: Arc<dyn Fn() + Send + Sync>) -> Arc<AutoCommand> {
        let is_on = win::create_event(true, false, Some(IS_ON_NAME));
        let is_off = win::create_event(true, true, Some(IS_OFF_NAME));
        let req_on = win::create_event(false, false, Some(ON_NAME));
        let req_off = win::create_event(false, false, Some(OFF_NAME));
        let me = Arc::new(AutoCommand { is_on, is_off, on });
        me.mirror();
        let m2 = me.clone();
        let _ = std::thread::Builder::new().name("auto-command".into()).spawn(move || {
            crate::native::self_threads::SelfThreads::register(crate::base::l::L::n("Auto command"));
            loop {
                let Some(i) = win::wait_any(&[req_on, req_off], win::INFINITE) else { return };
                if i == 0 {
                    AppLog::add_sev("Auto on (--auto on)", Sev::Info);
                    turn_on();
                } else if i == 1 {
                    AppLog::add_sev("Auto off (--auto off)", Sev::Info);
                    turn_off();
                }
                m2.mirror();
            }
        });
        me
    }

    pub fn mirror(&self) {
        if (self.on)() {
            win::reset_event(self.is_off);
            win::set_event(self.is_on);
        } else {
            win::reset_event(self.is_on);
            win::set_event(self.is_off);
        }
    }

    /// The command side: ask the running Toolbox and wait up to 20 s for the state to follow.
    pub fn send(what: Option<&str>) -> i32 {
        let what = crate::base::ci::lower(what.unwrap_or("status").trim());
        if !matches!(what.as_str(), "on" | "off" | "status") {
            return 4;
        }
        let (Some(is_on), Some(is_off)) = (win::open_event(IS_ON_NAME), win::open_event(IS_OFF_NAME)) else {
            return 3;
        };
        let r = (|| {
            if what == "status" {
                return if win::wait(is_on, 0) == win::WAIT_OBJECT_0 { 0 } else { 1 };
            }
            let want_on = what == "on";
            let target = if want_on { is_on } else { is_off };
            if win::wait(target, 0) == win::WAIT_OBJECT_0 {
                return if want_on { 0 } else { 1 };
            }
            match win::open_event(if want_on { ON_NAME } else { OFF_NAME }) {
                Some(req) => {
                    win::set_event(req);
                    win::close(req);
                }
                None => return 3,
            }
            let sw = Instant::now();
            while sw.elapsed() < Duration::from_millis(20000) {
                if win::wait(target, 200) == win::WAIT_OBJECT_0 {
                    return if want_on { 0 } else { 1 };
                }
            }
            2
        })();
        win::close(is_on);
        win::close(is_off);
        r
    }
}
