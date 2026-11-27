//! Own load per thread (1.16.0 Test 1, port of `SelfThreads.cs`): the program's long-lived threads register
//! themselves by name, and once a minute the processor time each one used is read with GetThreadTimes (query rights
//! on our own threads only). The About page and the log then say where the program's own load goes (window, worker,
//! DPC/ISR trace, frame trace), so it is cut where it is measured, not guessed.
//!
//! Native 1.17.0: the "rest" (threads that did not register) is no longer a bare number. `others_now` lists every
//! other thread of this process with its name (GetThreadDescription: Rust and C++ threads of this program carry one)
//! or its start "module+offset" (NtQueryInformationThread ThreadQuerySetWin32StartAddress: system, driver and graphics
//! threads), and `busiest` names the ones that used the most. A registered thread is matched by id AND creation time,
//! so a thread that ended (its id reused by another thread) is not read as "0.00%" any more.
//!
//! Threads that end keep their name: `ended(name)` on the ending thread adds what it used since the last reading to
//! that name (the wait probe's per-window closer, a trace thread whose session stopped), so short-lived threads of
//! the program no longer show up as "threads that ended". Unregistered threads with the same label (thread-pool
//! workers all start at ntdll TppWorkerThread) are summed into one entry with their count, and `sample_tick` names
//! what the busy ones run (`thread_sample`).

use crate::base::hooks::lock;
use indexmap::IndexMap;
use std::collections::{HashMap, HashSet};
use std::sync::{LazyLock, Mutex};

struct State {
    /// name -> (thread id, creation FILETIME; 0 = unknown)
    tids: IndexMap<String, (u32, i64)>,
    last: IndexMap<String, (u32, i64)>,
    /// The unregistered threads at the previous reading: tid -> (creation FILETIME, processor time).
    others: HashMap<u32, (i64, i64)>,
    others_base: bool,
    /// name -> processor time of its threads that ended (100 ns), the part not yet counted by a reading
    ended: IndexMap<String, i64>,
    /// the sampler: busy samples per work module since the last reading, all busy samples, cycles per thread id at
    /// the previous sample, calls since the last sampling round
    work: IndexMap<String, u32>,
    work_samples: u32,
    #[cfg_attr(not(windows), allow(dead_code))]
    cycles: HashMap<u32, u64>,
    sample_calls: u32,
}

static GATE: LazyLock<Mutex<State>> = LazyLock::new(|| {
    Mutex::new(State {
        tids: IndexMap::new(),
        last: IndexMap::new(),
        others: HashMap::new(),
        others_base: false,
        ended: IndexMap::new(),
        work: IndexMap::new(),
        work_samples: 0,
        cycles: HashMap::new(),
        sample_calls: 0,
    })
});

/// A thread of this process that did not register, as read now.
#[derive(Clone, Debug, PartialEq)]
pub struct Other {
    pub tid: u32,
    /// Creation FILETIME (tells a reused thread id from the thread read a minute ago).
    pub created: i64,
    /// Kernel + user time, 100 ns units.
    pub cpu: i64,
    /// GetThreadDescription ("" when it has none).
    pub name: String,
    /// "module+0x1a2b" of its start address ("" when unknown).
    pub start: String,
}

impl Other {
    /// What the self-check shows: "name (module+0x..)", the name or the start alone, else "thread 1234".
    pub fn label(&self) -> String {
        match (self.name.trim(), self.start.as_str()) {
            ("", "") => format!("thread {}", self.tid),
            ("", st) => st.to_string(),
            (n, "") => n.to_string(),
            (n, st) => format!("{n} ({st})"),
        }
    }
}

/// "module+0x1a2b" of an address in one of the modules (base, size, name), "" when it is in none.
pub fn module_offset(addr: u64, mods: &[crate::base::model::ModRange]) -> String {
    if addr == 0 {
        return String::new();
    }
    mods.iter().find(|m| addr >= m.base && addr - m.base < m.size).map(|m| format!("{}+0x{:x}", m.name, addr - m.base)).unwrap_or_else(|| format!("0x{addr:x}"))
}

/// The unregistered threads that used the most since the previous reading: (label, share of one processor), at least
/// `min_share`, the `top` busiest, busiest first. Threads with the same label are one entry, "label ×N" (the workers
/// of the thread pool all start at the same address). A thread that is new (no reading before, or the id now belongs
/// to a thread created at another time) counts from zero: it started within the window.
pub fn busiest(prev: &HashMap<u32, (i64, i64)>, now: &[Other], seconds: f64, min_share: f64, top: usize) -> Vec<(String, f64)> {
    if seconds <= 0.0 {
        return Vec::new();
    }
    let mut groups: IndexMap<String, (f64, usize)> = IndexMap::new();
    for t in now {
        let before = match prev.get(&t.tid) {
            Some(&(c, cpu)) if c == t.created => cpu,
            _ => 0,
        };
        let g = groups.entry(t.label()).or_insert((0.0, 0));
        g.0 += (t.cpu - before).max(0) as f64 / 1e7 / seconds;
        g.1 += 1;
    }
    let mut v: Vec<(String, f64)> = groups
        .into_iter()
        .map(|(k, (x, n))| (if n > 1 { format!("{k} ×{n}") } else { k }, x))
        .filter(|(_, x)| *x >= min_share)
        .collect();
    v.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    v.truncate(top);
    v
}

/// `SelfThreads` (static class).
pub struct SelfThreads;

impl SelfThreads {
    /// Call on the thread itself; a new thread under the same name replaces the old one.
    pub fn register(name: &str) {
        let tid = super::sys::current_thread_id();
        lock(&GATE).tids.insert(name.to_string(), (tid, own_created(tid)));
    }

    /// Call at the very end of a thread: what it used since the last reading stays under `name` (a registered thread
    /// under its own name, a short-lived helper under the name of what it works for) instead of vanishing into
    /// "threads that ended". A registered thread is also unregistered, so its id is not read again.
    pub fn ended(name: &str) {
        let tid = super::sys::current_thread_id();
        let cpu = own_cpu();
        let mut g = lock(&GATE);
        let counted = match (g.tids.get(name), g.last.get(name)) {
            (Some(&(t, _)), Some(&(lt, lcpu))) if t == tid && lt == tid => lcpu,
            _ => 0,
        };
        if g.tids.get(name).is_some_and(|v| v.0 == tid) {
            g.tids.shift_remove(name);
        }
        *g.ended.entry(name.to_string()).or_insert(0) += (cpu - counted).max(0);
    }

    /// The registered thread ids (the self-check leaves them out of the "rest").
    pub fn registered_tids() -> HashSet<u32> {
        lock(&GATE).tids.values().map(|v| v.0).collect()
    }

    /// Processor time (100 ns units, kernel + user) of each registered thread now; a thread that ended is left out
    /// (also when its id now belongs to another thread: the creation time differs).
    pub fn now() -> IndexMap<String, (u32, i64)> {
        let list: Vec<(String, (u32, i64))> = lock(&GATE).tids.iter().map(|(k, v)| (k.clone(), *v)).collect();
        #[allow(unused_mut)]
        let mut now = IndexMap::new();
        #[cfg(windows)]
        for (name, (tid, created)) in list {
            let h = super::win::open_thread_of(std::process::id(), tid);
            if h.is_null() {
                continue;
            }
            if let Some((c, exit, k, u)) = super::win::get_thread_times(h) {
                if (created == 0 || c == created) && exit == 0 {
                    now.insert(name, (tid, k + u));
                }
            }
            super::win::close_handle(h);
        }
        #[cfg(not(windows))]
        let _ = list;
        now
    }

    /// Every thread of this process that did not register, with its times, name and start (empty off Windows).
    pub fn others_now() -> Vec<Other> {
        let registered = Self::registered_tids();
        others_now(&registered)
    }

    /// The busiest unregistered threads since the previous call (see `busiest`); the first call only sets the base.
    pub fn others_shares(now: &[Other], seconds: f64) -> Vec<(String, f64)> {
        let mut g = lock(&GATE);
        let out = if g.others_base { busiest(&g.others, now, seconds, 0.005, 5) } else { Vec::new() };
        g.others = now.iter().map(|t| (t.tid, (t.created, t.cpu))).collect();
        g.others_base = true;
        out
    }

    /// Share of one processor each thread used since the previous call (0..1), from two readings over seconds.
    /// A thread that was replaced (another id under the same name) counts from zero: it started within the window.
    pub fn shares(now: &IndexMap<String, (u32, i64)>, seconds: f64) -> IndexMap<String, f64> {
        let mut shares = IndexMap::new();
        let mut g = lock(&GATE);
        let ended = std::mem::take(&mut g.ended);
        if seconds > 0.0 {
            for (name, v) in now {
                let before = match g.last.get(name) {
                    Some(l) if l.0 == v.0 => l.1,
                    _ => 0,
                };
                if !g.last.is_empty() || before > 0 {
                    shares.insert(name.clone(), (v.1 - before).max(0) as f64 / 1e7 / seconds);
                }
            }
            // threads that ended within the window, under their names
            for (name, cpu) in ended {
                *shares.entry(name).or_insert(0.0) += cpu.max(0) as f64 / 1e7 / seconds;
            }
        }
        g.last.clear();
        for (k, v) in now {
            g.last.insert(k.clone(), *v);
        }
        shares
    }
}

impl SelfThreads {
    /// Once a second (the worker's tick): every 5th call, each unregistered thread of this process that used the
    /// processor since the previous round is sampled once (`thread_sample`), and the module that gave it its work is
    /// counted. A few dozen microseconds per round.
    pub fn sample_tick() {
        {
            let mut g = lock(&GATE);
            g.sample_calls += 1;
            if g.sample_calls < 5 {
                return;
            }
            g.sample_calls = 0;
        }
        sample_round();
    }

    /// What the busy unregistered threads ran since the previous call: (module, share of the busy samples), the 3
    /// most frequent, and the number of busy samples. Resets the counts.
    pub fn work_shares() -> (Vec<(String, f64)>, u32) {
        let mut g = lock(&GATE);
        let n = g.work_samples;
        let work = std::mem::take(&mut g.work);
        g.work_samples = 0;
        if n == 0 {
            return (Vec::new(), 0);
        }
        let mut v: Vec<(String, f64)> = work.into_iter().map(|(k, c)| (k, c as f64 / n as f64)).collect();
        v.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        v.truncate(3);
        (v, n)
    }

    /// Tests: counts a busy sample of `module` (None: only the thread plumbing was seen).
    pub fn note_work(module: Option<&str>) {
        let mut g = lock(&GATE);
        g.work_samples += 1;
        let k = module.unwrap_or("ntdll.dll").to_string();
        *g.work.entry(k).or_insert(0) += 1;
    }
}

#[cfg(windows)]
fn sample_round() {
    use super::thread_sample::{sample, work_module, STACK_WORDS};
    use super::win;
    let pid = std::process::id();
    let registered = SelfThreads::registered_tids();
    let tids = win::threads_by_pid().swap_remove(&pid).unwrap_or_default();
    let me = super::sys::current_thread_id();
    // cycles of every unregistered thread now; only those that ran since the previous round are sampled
    let mut now_cycles: HashMap<u32, u64> = HashMap::new();
    for &tid in tids.iter().filter(|t| !registered.contains(t) && **t != me) {
        let h = win::open_thread_of(pid, tid);
        if h.is_null() {
            continue;
        }
        let mut c: u64 = 0;
        // SAFETY: a thread handle with query rights.
        if unsafe { windows_sys::Win32::System::WindowsProgramming::QueryThreadCycleTime(win::h(h), &mut c) } != 0 {
            now_cycles.insert(tid, c);
        }
        win::close_handle(h);
    }
    let busy: Vec<u32> = {
        let mut g = lock(&GATE);
        let b = now_cycles.iter().filter(|(t, c)| g.cycles.get(t).is_some_and(|p| **c > *p)).map(|(t, _)| *t).collect();
        g.cycles = now_cycles;
        b
    };
    if busy.is_empty() {
        return;
    }
    let mods = win::modules(win::get_current_process());
    let mut stack = Box::new([0u64; STACK_WORDS]);
    let mut hits: Vec<Option<String>> = Vec::with_capacity(busy.len());
    for tid in busy {
        if let Some((rip, n)) = sample(tid, &mut stack) {
            hits.push(work_module(rip, &stack[..n], &mods).map(str::to_string));
        }
    }
    for h in hits {
        SelfThreads::note_work(h.as_deref());
    }
}

#[cfg(not(windows))]
fn sample_round() {}

/// Processor time (kernel + user, 100 ns) of the calling thread (0 off Windows).
fn own_cpu() -> i64 {
    #[cfg(windows)]
    {
        // SAFETY: the pseudo handle of the calling thread.
        let h = super::win::hnd(unsafe { windows_sys::Win32::System::Threading::GetCurrentThread() });
        super::win::get_thread_times(h).map(|t| t.2 + t.3).unwrap_or(0)
    }
    #[cfg(not(windows))]
    0
}

/// Creation FILETIME of a thread of this process (0 when unknown).
fn own_created(_tid: u32) -> i64 {
    #[cfg(windows)]
    {
        let h = super::win::open_thread_of(std::process::id(), _tid);
        if h.is_null() {
            return 0;
        }
        let c = super::win::get_thread_times(h).map(|t| t.0).unwrap_or(0);
        super::win::close_handle(h);
        c
    }
    #[cfg(not(windows))]
    0
}

#[cfg(windows)]
fn others_now(registered: &HashSet<u32>) -> Vec<Other> {
    use super::win;
    let pid = std::process::id();
    let tids = win::threads_by_pid().swap_remove(&pid).unwrap_or_default();
    let mut mods: Option<Vec<crate::base::model::ModRange>> = None;
    let mut out = Vec::new();
    for tid in tids {
        if registered.contains(&tid) {
            continue;
        }
        let h = win::open_thread_of(pid, tid);
        if h.is_null() {
            continue;
        }
        if let Some((c, _, k, u)) = win::get_thread_times(h) {
            let addr = win::thread_start_address(h);
            let m = mods.get_or_insert_with(|| win::modules(win::get_current_process()));
            out.push(Other { tid, created: c, cpu: k + u, name: win::thread_description(h), start: module_offset(addr, m) });
        }
        win::close_handle(h);
    }
    out
}

#[cfg(not(windows))]
fn others_now(_registered: &HashSet<u32>) -> Vec<Other> {
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::base::hooks::static_hooks;

    fn m(items: &[(&str, (u32, i64))]) -> IndexMap<String, (u32, i64)> {
        items.iter().map(|(k, v)| (k.to_string(), *v)).collect()
    }

    /// Release1160Tests.ThreadSharesCountOnlyWithinTheWindow
    #[test]
    fn thread_shares_count_only_within_the_window() {
        let _g = static_hooks();
        SelfThreads::shares(&IndexMap::new(), 0.0); // forget earlier readings
        let first = m(&[("worker", (10, 5_000_000)), ("window", (11, 20_000_000))]);
        assert!(SelfThreads::shares(&first, 0.0).is_empty()); // the first reading only sets the base
        let second = m(&[("worker", (10, 5_000_000 + 3_000_000)), ("window", (11, 20_000_000 + 6_000_000)), ("frame trace", (12, 1_200_000))]);
        let s = SelfThreads::shares(&second, 60.0);
        let near = |a: f64, b: f64| (a - b).abs() < 5e-7;
        assert!(near(0.005, s["worker"])); // 0.3 s in 60 s = 0.5 % of one processor
        assert!(near(0.01, s["window"]));
        assert!(near(0.002, s["frame trace"])); // started within the window: counted from zero
        let third = m(&[("worker", (99, 600_000))]); // another thread under the same name
        assert!(near(0.001, SelfThreads::shares(&third, 60.0)["worker"]));
    }

    fn other(tid: u32, created: i64, cpu: i64, name: &str, start: &str) -> Other {
        Other { tid, created, cpu, name: name.into(), start: start.into() }
    }

    /// The "rest" is named: the busiest unregistered threads, new ones from zero, a reused id from zero, the quiet
    /// ones left out.
    #[test]
    fn busiest_names_the_unregistered_threads() {
        let prev: HashMap<u32, (i64, i64)> = [(10, (1, 1_000_000)), (11, (5, 2_000_000)), (12, (7, 0))].into_iter().collect();
        let now = vec![
            other(10, 1, 1_000_000 + 600_000_000, "", "d3d10warp.dll+0x1a2b"), // 60 s in 60 s: one whole core
            other(11, 6, 300_000_000, "probe-close", ""),                      // id reused: from zero (30 s)
            other(12, 7, 100_000, "etw-frames-flush", "DomainScheduler.exe+0x10"), // 0.01 s: below the floor
            other(13, 9, 12_000_000, "", ""),                                  // new: 1.2 s = 2 %
        ];
        let b = busiest(&prev, &now, 60.0, 0.005, 5);
        let near = |a: f64, b: f64| (a - b).abs() < 1e-9;
        assert_eq!(b.iter().map(|x| x.0.as_str()).collect::<Vec<_>>(), vec!["d3d10warp.dll+0x1a2b", "probe-close", "thread 13"]);
        assert!(near(b[0].1, 1.0) && near(b[1].1, 0.5) && near(b[2].1, 0.02));
        assert_eq!(busiest(&prev, &now, 60.0, 0.005, 1).len(), 1);
        assert!(busiest(&prev, &now, 0.0, 0.0, 5).is_empty());
        assert_eq!(other(1, 0, 0, "check-kernel", "x.dll+0x1").label(), "check-kernel (x.dll+0x1)");
    }

    #[test]
    fn start_addresses_resolve_to_module_and_offset() {
        use crate::base::model::ModRange;
        let mods = vec![ModRange { base: 0x1000, size: 0x100, name: "a.dll".into() }, ModRange { base: 0x8000, size: 0x1000, name: "ntdll.dll".into() }];
        assert_eq!(module_offset(0x1010, &mods), "a.dll+0x10");
        assert_eq!(module_offset(0x8fff, &mods), "ntdll.dll+0xfff");
        assert_eq!(module_offset(0x9000, &mods), "0x9000");
        assert_eq!(module_offset(0, &mods), "");
    }

    /// Pool workers share one start address: they are one entry with their count and their summed load.
    #[test]
    fn threads_with_one_label_are_summed() {
        let prev: HashMap<u32, (i64, i64)> = HashMap::new();
        let now: Vec<Other> = (0..5).map(|i| other(30 + i, 1, 21_600_000, "", "ntdll.dll+0x24990")).chain([other(40, 1, 6_000_000, "probe-close", "")]).collect();
        let b = busiest(&prev, &now, 60.0, 0.005, 5);
        assert_eq!(b[0].0, "ntdll.dll+0x24990 ×5");
        assert!((b[0].1 - 0.18).abs() < 1e-9); // 5 × 3.6 %
        assert_eq!(b[1].0, "probe-close");
    }

    /// A thread that ends keeps its name: what it used after the last reading is counted under it once.
    #[test]
    fn ended_threads_count_under_their_name() {
        let _g = static_hooks();
        SelfThreads::shares(&IndexMap::new(), 0.0);
        lock(&GATE).ended.insert("Wait probe".into(), 1_800_000);
        let s = SelfThreads::shares(&m(&[("worker", (10, 600_000))]), 60.0);
        assert!((s["Wait probe"] - 0.003).abs() < 1e-9); // 0.18 s in a minute
        let s = SelfThreads::shares(&m(&[("worker", (10, 1_200_000))]), 60.0);
        assert!(!s.contains_key("Wait probe")); // counted once
        SelfThreads::note_work(Some("d3d10warp.dll"));
        SelfThreads::note_work(Some("d3d10warp.dll"));
        SelfThreads::note_work(None);
        let (w, n) = SelfThreads::work_shares();
        assert_eq!(n, 3);
        assert_eq!(w[0].0, "d3d10warp.dll");
        assert!((w[0].1 - 2.0 / 3.0).abs() < 1e-9);
        assert_eq!(w[1].0, "ntdll.dll");
        assert_eq!(SelfThreads::work_shares().1, 0);
    }

    /// The first reading of the other threads only sets the base.
    #[test]
    fn others_shares_start_with_a_base() {
        let _g = static_hooks();
        lock(&GATE).others_base = false;
        let first = vec![other(20, 1, 0, "x", "")];
        assert!(SelfThreads::others_shares(&first, 60.0).is_empty());
        let second = vec![other(20, 1, 600_000_000, "x", "")];
        assert_eq!(SelfThreads::others_shares(&second, 60.0), vec![("x".to_string(), 1.0)]);
    }
}
