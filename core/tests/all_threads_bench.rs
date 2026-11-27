//! 1.17.1: the cost of the all-threads layer's tick (auto::all_threads::AllPlan::step) with 150 game threads on the
//! owner's topology (7800X3D, Top-5 on 2, 4, 6, 8, 10, graphics driver on 12-15). The plain test run checks that the
//! steady tick allocates nothing; the timing runs explicitly:
//!   cargo test --release --features testsupport --test all_threads_bench -- --ignored --nocapture

use dos_core::auto::all_threads::{AllCpus, AllPlan, AllThread};
use dos_core::base::time::{DateTime, Kind};
use dos_core::topo::synth;
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

struct Counting;
static ALLOCS: AtomicU64 = AtomicU64::new(0);

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        ALLOCS.fetch_add(1, Ordering::Relaxed);
        System.alloc(l)
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        System.dealloc(p, l)
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 {
        ALLOCS.fetch_add(1, Ordering::Relaxed);
        System.realloc(p, l, n)
    }
}

#[global_allocator]
static A: Counting = Counting;

fn setup() -> (AllPlan, Vec<AllThread>, [f64; 64], Vec<f64>, DateTime) {
    let mut p = AllPlan::new(AllCpus::new(&synth::x7800x3d(), 0));
    let mut ts = Vec::new();
    for (i, lp) in [2, 4, 6, 8, 10].iter().enumerate() {
        ts.push(AllThread { tid: 10 + i as u32, born: 1, load: 0.9 - i as f64 * 0.15, ideal: *lp, top5: true, allowed: true });
    }
    for i in 0..150u32 {
        let load = match i {
            0..=4 => 0.08,
            5..=29 => 0.01,
            _ => 0.0005,
        };
        ts.push(AllThread { tid: 100 + i, born: 1, load, ideal: (i % 16) as i32, top5: false, allowed: true });
    }
    let mut irq = [0.0; 64];
    for x in irq.iter_mut().take(16).skip(12) {
        *x = 0.06;
    }
    let busy: Vec<f64> = (0..16).map(|x| if x % 2 == 0 && x < 12 { 0.7 } else { 0.1 }).collect();
    let mut now = DateTime::from_parts(2026, 1, 1, 12, 0, 0, Kind::Utc).unwrap();
    // settle: the first placement and the follow-up moves are written (as Windows would)
    for _ in 0..60 {
        p.step(now, &ts, &busy, &irq, 0, 0x554);
        let w = p.writes.clone();
        for (k, x) in w.iter().enumerate() {
            ts[x.idx].ideal = x.to as i32;
            p.confirm(now, k, true);
        }
        now = now.add_seconds(1.0);
    }
    (p, ts, irq, busy, now)
}

#[test]
fn the_steady_tick_allocates_nothing() {
    let (mut p, ts, irq, busy, mut now) = setup();
    let before = ALLOCS.load(Ordering::Relaxed);
    for _ in 0..100 {
        p.step(now, &ts, &busy, &irq, 0, 0x554);
        assert!(p.writes.is_empty());
        now = now.add_seconds(1.0);
    }
    assert_eq!(0, ALLOCS.load(Ordering::Relaxed) - before);
}

#[test]
#[ignore]
fn tick_cost_with_150_threads() {
    let (mut p, ts, irq, busy, mut now) = setup();
    let n = 20_000;
    let t = Instant::now();
    for _ in 0..n {
        p.step(now, &ts, &busy, &irq, 0, 0x554);
        now = now.add_seconds(1.0);
    }
    let ns = t.elapsed().as_nanos() as f64 / n as f64;
    println!("all-threads tick, 155 threads (150 + 5 slots): {:.1} us per tick, {:.4}% of one core at 1 tick/s", ns / 1000.0, ns / 1e9 * 100.0);
}
