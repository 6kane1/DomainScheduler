//! Small benchmark: 10 000 threads in 100 processes on 16 cpus, 2 s of CSwitch + ReadyThread (320 k events,
//! 160 k context switches per second). Run: `cargo test --release --test bench -- --nocapture`.
//!
//! - typical: every thread runs, wakes follow fixed partners (a thread is woken by its neighbour in the same process
//!   or by the same-index thread of the next process), like real wake chains. Snapshot must build in < 5 ms.
//! - worst case: readier and target are random among all 10 000 threads (~256 k distinct thread pairs per second,
//!   all 100 x 100 process pairs). Only reported, with a loose bound.
//!
//! The debug build only prints the times.

use std::time::{Duration, Instant};
use threadmap::kwait::{self, state};
use threadmap::*;

fn lcg(x: &mut u64) -> u64 {
    *x = x.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
    *x >> 33
}

fn tid(i: u32) -> u32 {
    10_000 + i * 4
}

fn build(random_pairs: bool) -> (ThreadMap, Duration, u64) {
    let mut m = ThreadMap::new(Config::new(10_000_000));
    for p in 0..100u32 {
        m.process_start(&ProcessInfo { ts: 0, pid: 1000 + p * 4, image: format!("proc{p}.exe") });
        for t in 0..100u32 {
            let i = p * 100 + t;
            m.thread_start(&ThreadInfo { ts: 0, tid: tid(i), pid: 1000 + p * 4, description: Some(format!("Worker {t}")), ..Default::default() });
        }
    }
    m.set_game(Some(1000));
    let mut seed = 7u64;
    let t0 = Instant::now();
    let mut events = 0u64;
    let mut cur = [0u32; 16];
    let slice = 2000i64; // 200 us in 100 ns ticks
    for step in 0..10_000i64 {
        let ts = step * slice;
        for (cpu, c) in cur.iter_mut().enumerate() {
            let ni = (lcg(&mut seed) % 10_000) as u32;
            let ri = if random_pairs {
                (lcg(&mut seed) % 10_000) as u32
            } else if lcg(&mut seed) % 4 != 0 {
                (ni / 100) * 100 + (ni % 100 + 1) % 100
            } else {
                ((ni / 100 + 1) % 100) * 100 + ni % 100
            };
            let next = tid(ni);
            m.push_ready(&ReadyThread { ts: ts - 50, cpu: cpu as u32, readier_tid: tid(ri), target_tid: next, ..Default::default() });
            let st = if lcg(&mut seed) % 4 == 0 { state::READY } else { state::WAITING };
            m.push_cswitch(&CSwitch { ts: ts + cpu as i64, cpu: cpu as u32, new_tid: next, old_tid: *c, old_state: st, old_wait_reason: kwait::WR_QUEUE, ..Default::default() });
            *c = next;
            events += 2;
        }
    }
    m.flush();
    (m, t0.elapsed(), events)
}

/// Median of 9 builds (the machine may be busy with other work; the median is the honest figure).
fn time(q: &Query, m: &ThreadMap) -> (Duration, usize, usize) {
    let _ = m.snapshot(q); // warm caches
    let mut v = Vec::new();
    let mut s = m.snapshot(q);
    for _ in 0..9 {
        let t = Instant::now();
        s = m.snapshot(q);
        v.push(t.elapsed());
    }
    v.sort();
    (v[4], s.nodes.len(), s.links.len())
}

fn run(random_pairs: bool, limit: f64) {
    let (m, feed, events) = build(random_pairs);
    let st = m.stats();
    let name = if random_pairs { "worst case (random pairs)" } else { "typical (partner pairs)" };
    println!("== {name}");
    println!("fed {events} events in {feed:?} ({:.0} ns/event), {} threads, history {} KiB", feed.as_nanos() as f64 / events as f64, st.threads, st.history_bytes / 1024);
    let qs = [
        ("1 s, all processes", Query::default()),
        ("1 s, game focus + expanded, merged", Query { focus_pid: Some(1000), expand: vec![1000], ..Default::default() }),
        ("1 s, game focus + expanded, unmerged", Query { focus_pid: Some(1000), expand: vec![1000], merge: false, ..Default::default() }),
        ("1 s, past window (scrub)", Query { end_age: 5, ..Default::default() }),
        ("30 s window", Query { span: 300, ..Default::default() }),
    ];
    for (i, (label, q)) in qs.iter().enumerate() {
        let (d, n, l) = time(q, &m);
        println!("snapshot {label}: {d:?} ({n} nodes, {l} links)");
        assert!(n > 0);
        if !cfg!(debug_assertions) && i < 4 {
            assert!(d.as_secs_f64() < limit, "{name}: snapshot {label} took {d:?}");
        }
    }
    let mut lay = Layout::new();
    let s = m.snapshot(&Query { focus_pid: Some(1000), expand: vec![1000], merge: false, min_rate: 0.0, top_n: 0, ..Default::default() });
    lay.sync(&s);
    let t = Instant::now();
    for _ in 0..10 {
        lay.step(1.0 / 60.0);
    }
    let dl = t.elapsed() / 10;
    println!("layout step, {} nodes, {} links: {dl:?}; snapshot JSON {} KiB", lay.len(), s.links.len(), serde_json::to_string(&s).unwrap().len() / 1024);
    let ser = Instant::now();
    let _ = m.series();
    println!("series: {:?}", ser.elapsed());
    if !cfg!(debug_assertions) {
        assert!(dl.as_secs_f64() < 0.005, "layout step took {dl:?}");
    }
}

#[test]
fn ten_thousand_threads_typical() {
    run(false, 0.005);
}

#[test]
fn ten_thousand_threads_worst_case() {
    run(true, 0.030);
}
