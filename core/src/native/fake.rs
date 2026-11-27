//! An in-memory Windows for engine tests (port of `FakeNative.cs`); counts opens and writes.
//!
//! The state sits behind a mutex: `f.lock().procs.get_mut(&500).unwrap().cls = 64;`. Do not hold the guard while the
//! code under test calls the fake (it locks too). A process with `throw_` set panics in `get_eco` with
//! "Win32Exception", like the C# fake throws; the code under test catches it the way its C# original catches the
//! exception (`std::panic::catch_unwind`), or the test expects the panic.

use super::{Handle, INative};
use crate::base::hooks::lock;
use crate::base::model::ModRange;
use indexmap::IndexMap;
use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard};

#[derive(Clone, Debug)]
pub struct P {
    pub name: String,
    pub created: i64,
    pub parent: u32,
    pub cls: u32,
    pub aff: u64,
    pub sets: Vec<u32>,
    pub io: u32,
    pub mem: u32,
    pub eco_ctl: u32,
    pub eco_state: u32,
    pub boost_disabled: bool,
    pub protected: bool,
    pub refuse: bool,
    pub read_only: bool,
    pub throw_: bool,
    pub mods: Vec<ModRange>,
}

impl Default for P {
    fn default() -> Self {
        P {
            name: String::new(),
            created: 0,
            parent: 0,
            cls: 32,
            aff: u64::MAX,
            sets: Vec::new(),
            io: 2,
            mem: 5,
            eco_ctl: 0,
            eco_state: 0,
            boost_disabled: false,
            protected: false,
            refuse: false,
            read_only: false,
            throw_: false,
            mods: Vec::new(),
        }
    }
}

impl P {
    /// `new FakeNative.P { Name = name, Created = created }`.
    pub fn named(name: &str, created: i64) -> P {
        P { name: name.to_string(), created, ..Default::default() }
    }
}

#[derive(Clone, Debug)]
pub struct T {
    pub pid: u32,
    pub born: i64,
    pub cycles: u64,
    pub no_cycles: bool,
    pub start: u64,
    pub desc: String,
    pub prio: i32,
    pub boost_disabled: bool,
    pub ideal: u32,
    pub ideal_sets: i32,
    pub refuse: bool,
    pub refuse_sets: bool,
    pub sets: Vec<u32>,
    pub sets_writes: i32,
}

impl Default for T {
    fn default() -> Self {
        T {
            pid: 0,
            born: 0,
            cycles: 0,
            no_cycles: false,
            start: 0,
            desc: String::new(),
            prio: 0,
            boost_disabled: false,
            ideal: 999,
            ideal_sets: 0,
            refuse: false,
            refuse_sets: false,
            sets: Vec::new(),
            sets_writes: 0,
        }
    }
}

/// The fake's state (all public, like the C# fields).
pub struct FakeState {
    pub procs: IndexMap<u32, P>,
    pub threads: IndexMap<u32, T>,
    h: HashMap<Handle, (bool, u32)>,
    next: isize,
    pub thread_opens: i32,
    pub process_opens: i32,
    pub writes: i32,
    pub opened_pids: Vec<u32>,
    err: i32,
    /// 1 = power throttling is off in this Windows.
    pub eco_error: i32,
    pub cpu_idle: Vec<i64>,
    pub cpu_total: Vec<i64>,
    pub timer_coarsest: u32,
    pub timer_finest: u32,
    pub timer_current: u32,
    pub timer_held: bool,
    pub timer_fail: bool,
    pub timer_sets: i32,
    pub timer_releases: i32,
    pub process_lists: i32,
}

impl FakeState {
    fn new_handle(&mut self, thread: bool, id: u32) -> Handle {
        self.next += 1;
        let x = Handle(self.next);
        self.h.insert(x, (thread, id));
        x
    }

    fn pr(&mut self, x: Handle) -> &mut P {
        let id = self.h[&x].1;
        self.procs.get_mut(&id).expect("KeyNotFoundException: no such process")
    }

    fn th(&mut self, x: Handle) -> &mut T {
        let id = self.h[&x].1;
        self.threads.get_mut(&id).expect("KeyNotFoundException: no such thread")
    }

    fn w(&mut self, ok: bool, read_only: bool) -> bool {
        self.writes += 1;
        if read_only {
            self.err = 5;
            return false;
        }
        ok
    }

    /// Advances the cycle counters as if the game ran: per-thread cycles per second.
    pub fn run(&mut self, per_second: &[(u32, u64)], seconds: f64) {
        for (tid, rate) in per_second {
            if let Some(t) = self.threads.get_mut(tid) {
                t.cycles += (*rate as f64 * seconds) as u64;
            }
        }
    }
}

/// `FakeNative`.
pub struct FakeNative {
    st: Mutex<FakeState>,
}

impl Default for FakeNative {
    fn default() -> Self {
        Self::new()
    }
}

impl FakeNative {
    pub fn new() -> Self {
        FakeNative {
            st: Mutex::new(FakeState {
                procs: IndexMap::new(),
                threads: IndexMap::new(),
                h: HashMap::new(),
                next: 100,
                thread_opens: 0,
                process_opens: 0,
                writes: 0,
                opened_pids: Vec::new(),
                err: 0,
                eco_error: 0,
                cpu_idle: vec![0; 16],
                cpu_total: vec![0; 16],
                timer_coarsest: 156250,
                timer_finest: 5000,
                timer_current: 156250,
                timer_held: false,
                timer_fail: false,
                timer_sets: 0,
                timer_releases: 0,
                process_lists: 0,
            }),
        }
    }

    /// The state (C# reads and writes the fields directly).
    pub fn lock(&self) -> MutexGuard<'_, FakeState> {
        lock(&self.st)
    }

    /// A CS2-like game: MainThrd (first TID), render, audio, pool workers, an NVIDIA worker, one anti-cheat thread.
    pub fn cs2(pid: u32) -> FakeNative {
        let f = FakeNative::new();
        {
            let mut s = f.lock();
            let m = |base: u64, size: u64, name: &str| ModRange { base, size, name: name.to_string() };
            s.procs.insert(
                pid,
                P {
                    name: "cs2.exe".into(),
                    created: 77,
                    mods: vec![m(0x1000, 0x1000, "cs2.exe"), m(0x10000, 0x10000, "tier0.dll"), m(0x40000, 0x1000, "EasyAntiCheat_EOS.dll"), m(0x50000, 0x1000, "nvwgf2umx.dll")],
                    ..Default::default()
                },
            );
            let mut add = |tid: u32, start: u64, desc: &str, cycles: u64| {
                s.threads.insert(tid, T { pid, born: tid as i64 * 10, start, desc: desc.to_string(), cycles, ..Default::default() });
            };
            add(100, 0x1100, "MainThrd", 0);
            add(101, 0x10100, "D3D11RenderThread", 0);
            add(102, 0x10200, "AudioMixer", 0);
            add(103, 0x10300, "GlobPool/P0", 0);
            add(104, 0x10300, "GlobPool/P1", 0);
            add(105, 0x50010, "", 0);
            add(106, 0x40010, "", 0);
            add(107, 0x10400, "idle helper", 0);
        }
        f
    }

    /// `FakeNative.Cs2()` with the default pid 500.
    pub fn cs2_default() -> FakeNative {
        Self::cs2(500)
    }

    /// Advances the cycle counters as if the game ran: per-thread cycles per second.
    pub fn run(&self, per_second: &[(u32, u64)], seconds: f64) {
        self.lock().run(per_second, seconds);
    }
}

impl INative for FakeNative {
    fn enable_debug_privilege(&self) -> bool {
        true
    }

    fn open_process_graceful(&self, pid: u32, level: &mut i32) -> Handle {
        let mut s = self.lock();
        *level = -1;
        match s.procs.get(&pid) {
            None => return Handle::NULL,
            Some(p) if p.refuse => return Handle::NULL,
            _ => {}
        }
        s.process_opens += 1;
        s.opened_pids.push(pid);
        *level = 2;
        s.new_handle(false, pid)
    }

    fn process_created(&self, process: Handle) -> i64 {
        self.lock().pr(process).created
    }
    fn is_protected(&self, process: Handle) -> bool {
        self.lock().pr(process).protected
    }
    fn close(&self, handle: Handle) {
        self.lock().h.remove(&handle);
    }
    fn get_priority_class(&self, p: Handle, cls: &mut u32) -> bool {
        *cls = self.lock().pr(p).cls;
        true
    }
    fn set_priority_class(&self, p: Handle, cls: u32) -> bool {
        let mut s = self.lock();
        let ro = s.pr(p).read_only;
        if !s.w(true, ro) {
            return false;
        }
        s.pr(p).cls = cls;
        true
    }
    fn get_affinity(&self, p: Handle, mask: &mut u64) -> bool {
        *mask = self.lock().pr(p).aff;
        true
    }
    fn set_affinity(&self, p: Handle, mask: u64) -> bool {
        let mut s = self.lock();
        let ro = s.pr(p).read_only;
        if !s.w(true, ro) {
            return false;
        }
        s.pr(p).aff = mask;
        true
    }
    fn get_cpu_sets(&self, p: Handle, ids: &mut Vec<u32>) -> bool {
        *ids = self.lock().pr(p).sets.clone();
        true
    }
    fn set_cpu_sets(&self, p: Handle, ids: &[u32]) -> bool {
        let mut s = self.lock();
        let ro = s.pr(p).read_only;
        if !s.w(true, ro) {
            return false;
        }
        s.pr(p).sets = ids.to_vec();
        true
    }
    fn get_io(&self, p: Handle, v: &mut u32) -> bool {
        *v = self.lock().pr(p).io;
        true
    }
    fn set_io(&self, p: Handle, v: u32) -> bool {
        let mut s = self.lock();
        let ro = s.pr(p).read_only;
        if !s.w(true, ro) {
            return false;
        }
        s.pr(p).io = v;
        true
    }
    fn get_mem(&self, p: Handle, v: &mut u32) -> bool {
        *v = self.lock().pr(p).mem;
        true
    }
    fn set_mem(&self, p: Handle, v: u32) -> bool {
        let mut s = self.lock();
        let ro = s.pr(p).read_only;
        if !s.w(true, ro) {
            return false;
        }
        s.pr(p).mem = v;
        true
    }
    fn get_eco(&self, p: Handle, c: &mut u32, st: &mut u32) -> bool {
        let (throw_, ctl, state) = {
            let mut s = self.lock();
            let pr = s.pr(p);
            (pr.throw_, pr.eco_ctl, pr.eco_state)
        };
        if throw_ {
            panic!("Win32Exception");
        }
        *c = ctl;
        *st = state;
        true
    }
    fn set_eco(&self, p: Handle, c: u32, st: u32) -> bool {
        let mut s = self.lock();
        if s.eco_error != 0 {
            s.err = s.eco_error;
            return false;
        }
        let ro = s.pr(p).read_only;
        if !s.w(true, ro) {
            return false;
        }
        let pr = s.pr(p);
        pr.eco_ctl = c;
        pr.eco_state = st;
        true
    }
    fn get_boost_disabled(&self, p: Handle, d: &mut bool) -> bool {
        *d = self.lock().pr(p).boost_disabled;
        true
    }
    fn set_boost_disabled(&self, p: Handle, d: bool) -> bool {
        let mut s = self.lock();
        let ro = s.pr(p).read_only;
        if !s.w(true, ro) {
            return false;
        }
        s.pr(p).boost_disabled = d;
        true
    }
    fn modules(&self, p: Handle) -> Vec<ModRange> {
        self.lock().pr(p).mods.clone()
    }

    fn open_thread_of(&self, pid: u32, tid: u32) -> Handle {
        let mut s = self.lock();
        match s.threads.get(&tid) {
            Some(t) if t.pid == pid && !t.refuse => {}
            _ => return Handle::NULL,
        }
        s.thread_opens += 1;
        s.new_handle(true, tid)
    }

    fn thread_cycles(&self, th: Handle, cycles: &mut u64) -> bool {
        let mut s = self.lock();
        let t = s.th(th);
        *cycles = t.cycles;
        !t.no_cycles
    }
    fn thread_times(&self, th: Handle, created: &mut i64, ku: &mut u64) -> bool {
        let mut s = self.lock();
        let t = s.th(th);
        *created = t.born;
        *ku = t.cycles / 100;
        true
    }
    fn thread_start(&self, th: Handle) -> u64 {
        self.lock().th(th).start
    }
    fn thread_desc(&self, th: Handle) -> String {
        self.lock().th(th).desc.clone()
    }
    fn get_thread_prio(&self, th: Handle, prio: &mut i32) -> bool {
        *prio = self.lock().th(th).prio;
        true
    }
    fn set_thread_prio(&self, th: Handle, prio: i32) -> bool {
        let mut s = self.lock();
        s.w(true, false);
        s.th(th).prio = prio;
        true
    }
    fn get_thread_boost_disabled(&self, th: Handle, d: &mut bool) -> bool {
        *d = self.lock().th(th).boost_disabled;
        true
    }
    fn set_thread_boost_disabled(&self, th: Handle, d: bool) -> bool {
        let mut s = self.lock();
        s.w(true, false);
        s.th(th).boost_disabled = d;
        true
    }
    fn set_thread_ideal(&self, th: Handle, lp: u32) -> bool {
        let mut s = self.lock();
        s.w(true, false);
        let t = s.th(th);
        t.ideal = lp;
        t.ideal_sets += 1;
        true
    }
    fn get_thread_ideal(&self, th: Handle, lp: &mut u32) -> bool {
        *lp = self.lock().th(th).ideal;
        true
    }
    fn cpu_times(&self, idle: &mut Vec<i64>, total: &mut Vec<i64>) -> bool {
        let s = self.lock();
        *idle = s.cpu_idle.clone();
        *total = s.cpu_total.clone();
        true
    }
    fn get_thread_cpu_sets(&self, th: Handle, ids: &mut Vec<u32>) -> bool {
        *ids = self.lock().th(th).sets.clone();
        true
    }
    fn set_thread_cpu_sets(&self, th: Handle, ids: &[u32]) -> bool {
        let mut s = self.lock();
        s.w(true, false);
        if s.th(th).refuse_sets {
            s.err = 87;
            return false;
        }
        let t = s.th(th);
        t.sets = ids.to_vec();
        t.sets_writes += 1;
        true
    }

    fn timer_query(&self, coarsest: &mut u32, finest: &mut u32, current: &mut u32) -> bool {
        let s = self.lock();
        *coarsest = s.timer_coarsest;
        *finest = s.timer_finest;
        *current = s.timer_current;
        !s.timer_fail
    }
    fn timer_set(&self, resolution: u32, set: bool, actual: &mut u32) -> bool {
        let mut s = self.lock();
        *actual = s.timer_current;
        if s.timer_fail {
            return false;
        }
        if set {
            s.timer_sets += 1;
            s.timer_held = true;
            s.timer_current = resolution;
        } else {
            s.timer_releases += 1;
            s.timer_held = false;
            s.timer_current = s.timer_coarsest;
        }
        *actual = s.timer_current;
        true
    }

    fn threads_by_pid(&self) -> IndexMap<u32, Vec<u32>> {
        // GroupBy(Pid) in first-seen order, only for known processes, the TIDs in ascending order.
        let s = self.lock();
        let mut d: IndexMap<u32, Vec<u32>> = IndexMap::new();
        for (tid, t) in &s.threads {
            d.entry(t.pid).or_default().push(*tid);
        }
        d.retain(|pid, _| s.procs.contains_key(pid));
        for v in d.values_mut() {
            v.sort();
        }
        d
    }

    fn processes(&self) -> IndexMap<u32, (String, u32)> {
        let mut s = self.lock();
        s.process_lists += 1;
        s.procs.iter().map(|(k, p)| (*k, (p.name.clone(), p.parent))).collect()
    }

    fn last_error(&self) -> i32 {
        self.lock().err
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fake_behaves_like_the_csharp_one() {
        let f = FakeNative::cs2(500);
        let mut lvl = 0;
        let ph = f.open_process_graceful(500, &mut lvl);
        assert_eq!(lvl, 2);
        assert_eq!(ph, Handle(101));
        let mut cls = 0;
        assert!(f.get_priority_class(ph, &mut cls) && cls == 32);
        assert!(f.set_priority_class(ph, 128));
        assert_eq!(f.lock().procs[&500].cls, 128);
        assert_eq!(f.lock().writes, 1);
        let th = f.open_thread_of(500, 100);
        assert!(!th.is_null());
        assert!(f.open_thread_of(501, 100).is_null());
        f.run(&[(100, 1_000_000)], 2.0);
        let mut c = 0;
        assert!(f.thread_cycles(th, &mut c) && c == 2_000_000);
        assert_eq!(f.threads_by_pid()[&500], vec![100, 101, 102, 103, 104, 105, 106, 107]);
        f.lock().procs.get_mut(&500).unwrap().read_only = true;
        assert!(!f.set_io(ph, 0));
        assert_eq!(f.last_error(), 5);
        let mut a = 0;
        assert!(f.timer_set(5000, true, &mut a) && a == 5000 && f.lock().timer_held);
        f.close(ph);
        f.close(th);
        assert_eq!(f.processes().len(), 1);
    }
}
