//! Port of Optimizer.cs: the one worker thread every change goes through (in order), and the 1 s timer that queues
//! the tick. C# raised `Evaluate` on the UI thread after a job that reports; here that is a state version bump (the
//! app redraws). The DispatcherTimer is a thread of its own that sleeps a second between ticks.

use super::hub::Hub;
use super::state;
use crate::base::app_log::{AppLog, Sev};
use crate::base::hooks::lock;
use crate::lf;
use crate::native::self_threads::SelfThreads;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Condvar, Mutex, Weak};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

type Job = Box<dyn FnOnce() + Send>;

pub struct Optimizer {
    me: Weak<Optimizer>,
    h: Weak<Hub>,
    /// None after CompleteAdding.
    tx: Mutex<Option<Sender<Job>>>,
    rx: Mutex<Option<std::sync::mpsc::Receiver<Job>>>,
    worker: Mutex<Option<JoinHandle<()>>>,
    /// The worker ended (for Dispose's Join(5000) and RunNow's IsAlive).
    ended: Arc<(Mutex<bool>, Condvar)>,
    started: AtomicBool,
    timer_on: Arc<AtomicBool>,
    busy: AtomicI32,
    ticks: AtomicI32,
}

/// The message of a panic payload (the C# exception's Message).
pub fn panic_text(p: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = p.downcast_ref::<&str>() {
        s.to_string()
    } else if let Some(s) = p.downcast_ref::<String>() {
        s.clone()
    } else if let Some(e) = p.downcast_ref::<crate::auto::controller::Ex>() {
        e.message.clone()
    } else {
        "Unknown error".to_string()
    }
}

impl Optimizer {
    pub fn new(hub: Weak<Hub>) -> Arc<Optimizer> {
        let (tx, rx) = channel::<Job>();
        Arc::new_cyclic(|me| Optimizer {
            me: me.clone(),
            h: hub,
            tx: Mutex::new(Some(tx)),
            rx: Mutex::new(Some(rx)),
            worker: Mutex::new(None),
            ended: Arc::new((Mutex::new(false), Condvar::new())),
            started: AtomicBool::new(false),
            timer_on: Arc::new(AtomicBool::new(false)),
            busy: AtomicI32::new(0),
            ticks: AtomicI32::new(0),
        })
    }

    /// `Start()`: the worker (BelowNormal, "optimizer"), the 1 s timer, and a first tick.
    pub fn start(&self) {
        let Some(rx) = lock(&self.rx).take() else { return };
        let me = self.me.clone();
        let ended = self.ended.clone();
        let t = std::thread::Builder::new().name("optimizer".into()).spawn(move || {
            below_normal_thread();
            SelfThreads::register(crate::base::l::L::n("worker"));
            for item in rx.iter() {
                if let Err(p) = catch_unwind(AssertUnwindSafe(item)) {
                    AppLog::add_sev(lf!("Worker error: {0}", panic_text(&*p)), Sev::Error);
                }
                if let Some(o) = me.upgrade() {
                    o.busy.store(0, Ordering::SeqCst);
                }
            }
            let (m, c) = &*ended;
            *lock(m) = true;
            c.notify_all();
        });
        match t {
            Ok(h) => {
                *lock(&self.worker) = Some(h);
                self.started.store(true, Ordering::SeqCst);
            }
            Err(_) => {
                let (m, _) = &*self.ended;
                *lock(m) = true;
            }
        }
        self.timer_on.store(true, Ordering::SeqCst);
        let on = self.timer_on.clone();
        let me = self.me.clone();
        let _ = std::thread::Builder::new().name("optimizer-timer".into()).spawn(move || {
            SelfThreads::register(crate::base::l::L::n("worker timer"));
            loop {
                std::thread::sleep(Duration::from_secs(1));
                if !on.load(Ordering::SeqCst) {
                    return;
                }
                let Some(o) = me.upgrade() else { return };
                let m2 = o.me.clone();
                o.queue(Box::new(move || if let Some(o) = m2.upgrade() { o.tick() }), true, true);
            }
        });
        let m2 = self.me.clone();
        self.queue(Box::new(move || if let Some(o) = m2.upgrade() { o.tick() }), false, true);
    }

    fn alive(&self) -> bool {
        self.started.load(Ordering::SeqCst) && !*lock(&self.ended.0)
    }

    /// `Tick()`: once a second on the worker.
    fn tick(&self) {
        let Some(h) = self.h.upgrade() else { return };
        let num = self.ticks.fetch_add(1, Ordering::SeqCst);
        let flag = !h.ui_visible() && h.auto.state() == crate::auto::AutoState::Idle && h.auto.game_pid() == 0 && !h.auto.background_standalone();
        if !flag || num % 5 == 0 {
            let starting = Instant::now();
            let mut t0 = starting;
            let mut part = |i: usize| {
                let now = Instant::now();
                h.note_part(i, (now - t0).as_secs_f64() * 1000.0);
                t0 = now;
            };
            if h.ui_visible() {
                h.sample_cpu();
            }
            part(0);
            h.live_tick();
            h.auto.tick();
            part(1);
            let on = h.auto.on();
            let mut num2 = if on { 5.clamp(1, 1000) } else if flag { 30.max(h.reapply_ticks()) } else { h.reapply_ticks() };
            if flag {
                num2 = (num2 + 4) / 5 * 5;
            }
            let eng = h.engine.clone();
            h.soft.tick(&|pid: u32| eng.processes_now().contains_key(&pid));
            part(2);
            if num % num2 == 0 && !h.paused() {
                h.sys.pass(h.auto.game_pid());
            }
            if !on && num % (if flag { 10 } else { h.reapply_ticks() }) == 0 && !h.paused() {
                h.auto.standalone_background(h.bg_on(), h.game_mask_value());
            }
            part(3);
            if num % 60 == 55 && !on {
                h.engine.prune(None);
            }
            h.note_tick(starting.elapsed().as_secs_f64() * 1000.0);
            h.measure_self();
        }
    }

    /// `Queue(job, skipIfBusy, reports)`.
    pub fn queue(&self, job: Job, skip_if_busy: bool, reports: bool) {
        let g = lock(&self.tx);
        let Some(tx) = g.as_ref() else { return };
        if skip_if_busy && self.busy.load(Ordering::SeqCst) != 0 {
            return;
        }
        self.busy.store(1, Ordering::SeqCst);
        let _ = tx.send(Box::new(move || {
            job();
            if reports {
                state::bump();
            }
        }));
    }

    /// `Run<T>(work, done)`: the work on the worker; an error is logged ("Error: {0}") and done gets None; then the
    /// UI redraws. `done` runs on the worker right after (C#: on the UI thread).
    pub fn run<T: 'static>(&self, work: impl FnOnce() -> Result<T, String> + Send + 'static, done: impl FnOnce(Option<T>) + Send + 'static) {
        self.queue(
            Box::new(move || {
                let r = match catch_unwind(AssertUnwindSafe(work)) {
                    Ok(Ok(v)) => Some(v),
                    Ok(Err(e)) => {
                        AppLog::add_sev(lf!("Error: {0}", e), Sev::Error);
                        None
                    }
                    Err(p) => {
                        AppLog::add_sev(lf!("Error: {0}", panic_text(&*p)), Sev::Error);
                        None
                    }
                };
                done(r);
                state::bump();
            }),
            false,
            false,
        );
    }

    /// `RunNow(job, timeoutMs)`: runs the job on the worker and waits; false when it did not finish in time (it may
    /// still run).
    pub fn run_now(&self, job: Job, timeout_ms: u64) -> bool {
        let tx = lock(&self.tx).clone();
        let Some(tx) = tx.filter(|_| self.alive()) else {
            job();
            return true;
        };
        let done = Arc::new((Mutex::new(false), Condvar::new()));
        let d2 = done.clone();
        if tx
            .send(Box::new(move || {
                struct Set(Arc<(Mutex<bool>, Condvar)>);
                impl Drop for Set {
                    fn drop(&mut self) {
                        *lock(&self.0 .0) = true;
                        self.0 .1.notify_all();
                    }
                }
                let _s = Set(d2);
                job();
            }))
            .is_err()
        {
            return true;
        }
        let (m, c) = &*done;
        let end = Instant::now() + Duration::from_millis(timeout_ms);
        let mut g = lock(m);
        while !*g {
            let now = Instant::now();
            if now >= end {
                return false;
            }
            g = c.wait_timeout(g, end - now).unwrap_or_else(|e| e.into_inner()).0;
        }
        true
    }

    /// `Dispose()`: the timer stops, no more jobs are taken, the worker gets up to 5 s to finish.
    pub fn dispose(&self) {
        self.timer_on.store(false, Ordering::SeqCst);
        lock(&self.tx).take();
        if self.alive() {
            let (m, c) = &*self.ended;
            let end = Instant::now() + Duration::from_millis(5000);
            let mut g = lock(m);
            while !*g {
                let now = Instant::now();
                if now >= end {
                    break;
                }
                g = c.wait_timeout(g, end - now).unwrap_or_else(|e| e.into_inner()).0;
            }
        }
    }
}

/// `Priority = ThreadPriority.BelowNormal` of the worker thread.
fn below_normal_thread() {
    #[cfg(windows)]
    // SAFETY: the pseudo handle of the calling thread.
    unsafe {
        use windows_sys::Win32::System::Threading::{GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_BELOW_NORMAL};
        SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_BELOW_NORMAL);
    }
}
