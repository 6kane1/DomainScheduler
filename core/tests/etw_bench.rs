//! Per-event cost of the live ETW callbacks (wait probe, DPC/ISR trace, frame trace) at the owner's rates, on
//! synthetic EVENT_RECORDs fed straight into the real callbacks (no session). Run explicitly:
//!   cargo test --release --features testsupport --test etw_bench -- --ignored --nocapture
//! It prints ns per event and heap allocations per event; the plain test run only checks that the hot paths do not
//! allocate.

use dos_core::etw::raw::{PAGE_FAULT_GUID, PERF_INFO_GUID, THREAD_GUID, DXGI_GUID};
use dos_core::etw::{EtwFrames, EtwIrq, ReadyProbe};
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

/// An x64 EVENT_RECORD (112 bytes) with its user data behind it.
struct Rec {
    buf: Vec<u64>,
}

impl Rec {
    fn new(provider: [u8; 16], opcode: u8, id: u16, pid: u32, cpu: u8, user: &[u8], ctx: *const core::ffi::c_void) -> Rec {
        let mut buf = vec![0u64; 14 + user.len().div_ceil(8) + 1];
        let base = buf.as_mut_ptr() as *mut u8;
        unsafe {
            let put = |off: usize, b: &[u8]| std::ptr::copy_nonoverlapping(b.as_ptr(), base.add(off), b.len());
            put(12, &pid.to_le_bytes());
            put(24, &provider);
            put(40, &id.to_le_bytes());
            put(45, &[opcode]);
            put(80, &[cpu]);
            put(86, &(user.len() as u16).to_le_bytes());
            put(112, user);
            let up = base.add(112) as u64;
            put(96, &up.to_le_bytes());
            put(104, &(ctx as u64).to_le_bytes());
        }
        Rec { buf }
    }
    fn ptr(&mut self) -> *mut core::ffi::c_void {
        self.buf.as_mut_ptr() as *mut core::ffi::c_void
    }
    fn set_stamp(&mut self, ts: i64) {
        unsafe { (self.buf.as_mut_ptr() as *mut u8).add(16).cast::<i64>().write_unaligned(ts) };
    }
    fn set_u32(&mut self, user_off: usize, v: u32) {
        unsafe { (self.buf.as_mut_ptr() as *mut u8).add(112 + user_off).cast::<u32>().write_unaligned(v) };
    }
    fn set_u64(&mut self, user_off: usize, v: u64) {
        unsafe { (self.buf.as_mut_ptr() as *mut u8).add(112 + user_off).cast::<u64>().write_unaligned(v) };
    }
}

struct Result {
    ns: f64,
    allocs: f64,
}

/// One 250 ms wait-probe window at the owner's rate: ~40k CSwitch + ReadyThread events system-wide, the game's
/// 150 threads among 3000.
fn probe_window(p: &ReadyProbe, events: usize) -> Result {
    let ctx = p.bench_context();
    let game = 4242u32;
    p.bench_open(game); // the game's pid is known before the thread rundown (start(pid))
    // thread rundown: 3000 threads, 150 of them the game's
    let mut start = Rec::new(THREAD_GUID, 3, 0, 0, 0, &[0u8; 8], ctx);
    for t in 0..3000u32 {
        start.set_u32(0, if t % 20 == 0 { game } else { 100 + t % 300 });
        start.set_u32(4, 1000 + t * 4);
        unsafe { ReadyProbe::bench_event(start.ptr()) };
    }
    p.bench_open(game);
    let mut ready = Rec::new(THREAD_GUID, 50, 0, 0, 0, &[0u8; 8], ctx);
    let mut cs = Rec::new(THREAD_GUID, 36, 0, 0, 0, &[0u8; 24], ctx);
    let mut fault = Rec::new(PAGE_FAULT_GUID, 32, 0, 0, 0, &[0u8; 48], ctx);
    let mut ts: i64 = 1_000_000_000_000;
    let a0 = ALLOCS.load(Ordering::Relaxed);
    let t0 = Instant::now();
    let mut x: u32 = 12345;
    for i in 0..events / 2 {
        x = x.wrapping_mul(1103515245).wrapping_add(12345);
        let tid = 1000 + (x >> 8) % 3000 * 4;
        ts += 6000;
        ready.set_u32(0, tid);
        ready.set_stamp(ts);
        unsafe { ReadyProbe::bench_event(ready.ptr()) };
        ts += 3000;
        cs.set_u32(0, tid);
        cs.set_stamp(ts);
        unsafe { ReadyProbe::bench_event(cs.ptr()) };
        if i % 500 == 0 {
            fault.set_u32(32, tid);
            unsafe { ReadyProbe::bench_event(fault.ptr()) };
        }
    }
    let el = t0.elapsed();
    let a1 = ALLOCS.load(Ordering::Relaxed);
    p.bench_close();
    Result { ns: el.as_nanos() as f64 / events as f64, allocs: (a1 - a0) as f64 / events as f64 }
}

/// A second of DPC/ISR events (30k), most from one graphics routine, spread over 16 processors.
fn irq_second(irq: &EtwIrq, events: usize) -> Result {
    let ctx = irq.bench_context();
    irq.set_drivers(vec![(0xfffff800_10000000, "nvlddmkm.sys".into()), (0xfffff800_20000000, "ndis.sys".into())]);
    let mut dpc = Rec::new(PERF_INFO_GUID, 66, 0, 0, 0, &[0u8; 24], ctx);
    let mut ts: i64 = 1_000_000_000_000;
    let a0 = ALLOCS.load(Ordering::Relaxed);
    let t0 = Instant::now();
    for i in 0..events {
        ts += 33_000;
        dpc.set_u64(0, (ts - 20_000) as u64);
        dpc.set_u64(8, if i % 4 == 0 { 0xfffff800_20000100 } else { 0xfffff800_10000100 + (i as u64 % 8) * 16 });
        dpc.set_stamp(ts);
        unsafe { (dpc.buf.as_mut_ptr() as *mut u8).add(80).write((i % 16) as u8) };
        unsafe { EtwIrq::bench_event(dpc.ptr()) };
    }
    let el = t0.elapsed();
    let a1 = ALLOCS.load(Ordering::Relaxed);
    Result { ns: el.as_nanos() as f64 / events as f64, allocs: (a1 - a0) as f64 / events as f64 }
}

/// Present starts of the game at 700 FPS (and a few of other processes the filter lets through).
fn frames_run(f: &EtwFrames, events: usize) -> Result {
    let ctx = f.bench_context(4242);
    let mut pr = Rec::new(DXGI_GUID, 0, 42, 4242, 0, &[0u8; 32], ctx);
    let mut ts: i64 = 1_000_000_000_000;
    // fill the ring first: the steady state drops the oldest frame for every new one
    for _ in 0..41_000 {
        ts += 1_428_571;
        pr.set_stamp(ts);
        unsafe { EtwFrames::bench_event(pr.ptr()) };
    }
    let a0 = ALLOCS.load(Ordering::Relaxed);
    let t0 = Instant::now();
    for _ in 0..events {
        ts += 1_428_571;
        pr.set_stamp(ts);
        unsafe { EtwFrames::bench_event(pr.ptr()) };
    }
    let el = t0.elapsed();
    let a1 = ALLOCS.load(Ordering::Relaxed);
    Result { ns: el.as_nanos() as f64 / events as f64, allocs: (a1 - a0) as f64 / events as f64 }
}

#[test]
#[ignore]
fn etw_callbacks_per_event_cost() {
    let p = ReadyProbe::new();
    probe_window(&p, 40_000); // warm up (maps grow to their working size)
    let mut probe = Vec::new();
    for _ in 0..20 {
        probe.push(probe_window(&p, 40_000));
    }
    let irq = EtwIrq::new();
    irq.mark_available_for_tests();
    irq_second(&irq, 30_000);
    let mut ir = Vec::new();
    for _ in 0..10 {
        ir.push(irq_second(&irq, 30_000));
    }
    let f = EtwFrames::new();
    let mut fr = Vec::new();
    for _ in 0..10 {
        fr.push(frames_run(&f, 700));
    }
    let med = |v: &mut Vec<Result>| {
        v.sort_by(|a, b| a.ns.partial_cmp(&b.ns).unwrap());
        (v[v.len() / 2].ns, v.iter().map(|r| r.allocs).fold(0.0, f64::max))
    };
    let (pn, pa) = med(&mut probe);
    let (inn, ia) = med(&mut ir);
    let (fnn, fa) = med(&mut fr);
    println!("wait probe : {pn:7.1} ns/event, {pa:.4} allocs/event, {:.2} ms per 250 ms window (40k events), {:.3}% of a core", pn * 40_000.0 / 1e6, pn * 40_000.0 / 2.0 / 1e7);
    println!("DPC/ISR    : {inn:7.1} ns/event, {ia:.4} allocs/event, {:.2} ms per second (30k events), {:.3}% of a core", inn * 30_000.0 / 1e6, inn * 30_000.0 / 1e7);
    println!("frames     : {fnn:7.1} ns/event, {fa:.4} allocs/event, {:.3} ms per second (700 presents), {:.4}% of a core", fnn * 700.0 / 1e6, fnn * 700.0 / 1e7);
}

/// The hot paths do not allocate once their maps reached their working size.
#[test]
fn etw_hot_paths_do_not_allocate() {
    let p = ReadyProbe::new();
    probe_window(&p, 40_000);
    let r = probe_window(&p, 40_000);
    assert!(r.allocs < 0.001, "wait probe: {} allocations per event", r.allocs);
    let irq = EtwIrq::new();
    irq.mark_available_for_tests();
    irq_second(&irq, 30_000);
    let r = irq_second(&irq, 30_000);
    assert!(r.allocs < 0.001, "DPC/ISR: {} allocations per event", r.allocs);
    let f = EtwFrames::new();
    let r = frames_run(&f, 700);
    assert_eq!(r.allocs, 0.0, "frames: {} allocations per event", r.allocs);
}

