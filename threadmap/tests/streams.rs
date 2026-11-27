//! Synthetic event streams for every link kind and the edge cases. Timestamps: QPC 10 MHz, helpers take microseconds.

use threadmap::kwait::{self, state};
use threadmap::*;

const F: u64 = 10_000_000;
fn us(t: f64) -> i64 {
    (t * 10.0).round() as i64
}

struct S {
    m: ThreadMap,
}

impl S {
    fn new() -> Self {
        Self { m: ThreadMap::new(Config::new(F)) }
    }
    fn proc(&mut self, pid: u32, image: &str) {
        self.m.process_start(&ProcessInfo { ts: 0, pid, image: image.into() });
    }
    fn th(&mut self, tid: u32, pid: u32, desc: Option<&str>) {
        self.th_at(0.0, tid, pid, desc);
    }
    fn th_at(&mut self, t: f64, tid: u32, pid: u32, desc: Option<&str>) {
        self.m.thread_start(&ThreadInfo { ts: us(t), tid, pid, description: desc.map(Into::into), ..Default::default() });
    }
    fn sw(&mut self, t: f64, cpu: u32, new: u32, old: u32, old_state: u8, reason: u8) {
        self.m.push_cswitch(&CSwitch {
            ts: us(t),
            cpu,
            new_tid: new,
            old_tid: old,
            new_priority: 10,
            old_priority: 9,
            old_wait_reason: reason,
            old_state,
            ..Default::default()
        });
    }
    fn ready(&mut self, t: f64, cpu: u32, readier: u32, target: u32) {
        self.m.push_ready(&ReadyThread { ts: us(t), cpu, readier_tid: readier, target_tid: target, ..Default::default() });
    }
    fn ready_dpc(&mut self, t: f64, cpu: u32, target: u32) {
        self.m.push_ready(&ReadyThread { ts: us(t), cpu, readier_tid: READIER_UNKNOWN, target_tid: target, flags: READY_FLAG_DPC, ..Default::default() });
    }
    fn snap(&mut self, q: &Query) -> Snapshot {
        self.m.flush();
        self.m.snapshot(q)
    }
}

fn q_all() -> Query {
    Query { min_rate: 0.0, top_n: 0, hide_system: false, min_cpu: 0.0, ..Default::default() }
}

fn node<'a>(s: &'a Snapshot, label: &str) -> &'a Node {
    s.nodes.iter().find(|n| n.label == label).unwrap_or_else(|| panic!("no node {label}: {:?}", s.nodes.iter().map(|n| &n.label).collect::<Vec<_>>()))
}
fn has_node(s: &Snapshot, label: &str) -> bool {
    s.nodes.iter().any(|n| n.label == label)
}
fn link<'a>(s: &'a Snapshot, from: &str, to: &str, kind: &str) -> Option<&'a Link> {
    let (f, t) = (s.nodes.iter().find(|n| n.label == from)?.id, s.nodes.iter().find(|n| n.label == to)?.id);
    s.links.iter().find(|l| l.from == f && l.to == t && l.kind == kind)
}

/// Game 100 (MainThread 1000, Render 1004), background 200 (Helper 2000).
fn base() -> S {
    let mut s = S::new();
    s.proc(100, "game.exe");
    s.proc(200, "helper.exe");
    s.th(1000, 100, Some("MainThread"));
    s.th(1004, 100, Some("RenderThread"));
    s.th(2000, 200, Some("Helper"));
    s.m.set_game(Some(100));
    s
}

#[test]
fn wake_links_thread_and_process_level() {
    let mut s = base();
    // 1 s: every 10 ms the helper readies RenderThread, MainThread readies RenderThread too.
    for i in 0..100 {
        let t = i as f64 * 10_000.0;
        s.ready(t, 0, 2000, 1004);
        s.sw(t + 10.0, 1, 1004, 0, state::RUNNING, 0);
        s.sw(t + 500.0, 1, 0, 1004, state::WAITING, kwait::WR_USER_REQUEST);
        s.ready(t + 600.0, 2, 1000, 1004);
        s.sw(t + 610.0, 1, 1004, 0, state::RUNNING, 0);
        s.sw(t + 900.0, 1, 0, 1004, state::WAITING, kwait::WR_USER_REQUEST);
    }
    // Thread level: game expanded.
    let g = s.snap(&Query { expand: vec![100], ..q_all() });
    let l = link(&g, "helper.exe", "RenderThread", "wake").expect("helper -> render wake");
    assert_eq!(l.count, 100);
    assert!((l.rate - 100.0 / g.window.seconds).abs() < 1e-6);
    assert_eq!(link(&g, "MainThread", "RenderThread", "wake").unwrap().count, 100);
    let r = node(&g, "RenderThread");
    assert!((r.wakes_in - 200.0 / g.window.seconds).abs() < 1e-6);
    assert_eq!(r.parent, Some(node(&g, "game.exe").id));
    assert!(node(&g, "game.exe").expanded);
    // Process level: one link helper -> game (100), the intra-game wake is a self-link and gone.
    let p = s.m.snapshot(&q_all());
    assert_eq!(link(&p, "helper.exe", "game.exe", "wake").unwrap().count, 100);
    assert_eq!(p.links.iter().filter(|l| l.kind == "wake").count(), 1);
    assert!(node(&p, "game.exe").wakes_in > 0.0, "wakes of its threads are still counted on the node");
}

#[test]
fn preempt_direct_switch_out_ready() {
    let mut s = base();
    for i in 0..50 {
        let t = i as f64 * 20_000.0;
        s.sw(t, 0, 1000, 0, state::RUNNING, 0);
        // Helper takes cpu 0 while MainThread is still runnable.
        s.sw(t + 1000.0, 0, 2000, 1000, state::READY, kwait::WR_PREEMPTED);
        s.sw(t + 1030.0, 0, 1000, 2000, state::WAITING, kwait::WR_USER_REQUEST);
        // A yield is not a preemption.
        s.sw(t + 2000.0, 0, 1004, 1000, state::READY, kwait::WR_YIELD_EXECUTION);
        s.sw(t + 2100.0, 0, 0, 1004, state::WAITING, kwait::WR_USER_REQUEST);
    }
    let g = s.snap(&Query { expand: vec![100], ..q_all() });
    assert_eq!(link(&g, "helper.exe", "MainThread", "preempt").unwrap().count, 50);
    assert!(link(&g, "RenderThread", "MainThread", "preempt").is_none());
    // MainThread waited 30 us each time after being preempted (and ran 1000 us + ...).
    let m = node(&g, "MainThread");
    assert!(m.waits >= 50);
    assert!(m.wait_p99_us.is_some());
}

#[test]
fn preempt_held_core_while_victim_waited() {
    let mut s = base();
    s.proc(300, "other.exe");
    s.th(3000, 300, Some("Other"));
    for i in 0..40 {
        let t = i as f64 * 25_000.0;
        // cpu 2: Other runs t..t+3000; MainThread is readied at t+500 (by Helper) and gets cpu 2 at t+3000.
        s.sw(t, 2, 3000, 0, state::RUNNING, 0);
        s.ready(t + 500.0, 1, 2000, 1000);
        s.sw(t + 3000.0, 2, 1000, 3000, state::WAITING, kwait::WR_USER_REQUEST);
        s.sw(t + 4000.0, 2, 0, 1000, state::WAITING, kwait::WR_USER_REQUEST);
        // Short wait (10 us < 50 us): RenderThread readied at t+10000 runs at t+10010 after the Other segment.
        s.sw(t + 9000.0, 3, 3000, 0, state::RUNNING, 0);
        s.ready(t + 10_000.0, 1, 2000, 1004);
        s.sw(t + 10_010.0, 3, 1004, 3000, state::WAITING, kwait::WR_USER_REQUEST);
        s.sw(t + 11_000.0, 3, 0, 1004, state::WAITING, kwait::WR_USER_REQUEST);
    }
    let g = s.snap(&Query { expand: vec![100], ..q_all() });
    assert_eq!(link(&g, "other.exe", "MainThread", "preempt").unwrap().count, 40);
    assert!(link(&g, "other.exe", "RenderThread", "preempt").is_none(), "10 us wait is below preempt_min_wait_us");
    let m = node(&g, "MainThread");
    // 2500 us waits: p99 rounded up to its bin (<= +19 %).
    let p = m.wait_p99_us.unwrap();
    assert!((2500.0..=2500.0 * 1.2).contains(&p), "{p}");
}

#[test]
fn lock_waiter_to_owner_and_executive_is_not_lock() {
    let mut s = base();
    for i in 0..30 {
        let t = i as f64 * 30_000.0;
        s.sw(t, 0, 1004, 0, state::RUNNING, 0);
        s.sw(t + 100.0, 0, 0, 1004, state::WAITING, kwait::WR_ALERT_BY_THREAD_ID);
        s.ready(t + 2000.0, 1, 1000, 1004); // MainThread releases the SRW lock
        s.sw(t + 2010.0, 0, 1004, 0, state::RUNNING, 0);
        s.sw(t + 2100.0, 0, 0, 1004, state::WAITING, kwait::EXECUTIVE);
        s.ready(t + 5000.0, 1, 2000, 1004); // an Executive wait: only a wake
        s.sw(t + 5010.0, 0, 1004, 0, state::RUNNING, 0);
        s.sw(t + 5100.0, 0, 0, 1004, state::WAITING, kwait::WR_PUSH_LOCK);
    }
    let g = s.snap(&Query { expand: vec![100], ..q_all() });
    assert_eq!(link(&g, "RenderThread", "MainThread", "lock").unwrap().count, 30);
    assert_eq!(link(&g, "MainThread", "RenderThread", "wake").unwrap().count, 30);
    assert!(link(&g, "RenderThread", "helper.exe", "lock").is_none());
    assert_eq!(link(&g, "helper.exe", "RenderThread", "wake").unwrap().count, 30);
    let r = node(&g, "RenderThread");
    assert_eq!(r.state, "wait_lock");
    assert_eq!(r.wait_reason_name, Some("WrPushLock"));
}

#[test]
fn ipc_from_lpc_waits_and_external_alpc_events() {
    let mut s = base();
    s.proc(400, "server.exe");
    s.th(4000, 400, Some("Server"));
    for i in 0..20 {
        let t = i as f64 * 40_000.0;
        // Server waits for a request; RenderThread (client) sends it and readies the server.
        s.sw(t, 3, 0, 4000, state::WAITING, kwait::WR_LPC_RECEIVE);
        s.sw(t + 10.0, 0, 1004, 0, state::RUNNING, 0);
        s.ready(t + 100.0, 0, 1004, 4000);
        s.sw(t + 110.0, 0, 0, 1004, state::WAITING, kwait::WR_LPC_REPLY);
        s.sw(t + 120.0, 3, 4000, 0, state::RUNNING, 0);
        // Server replies: readies the client that waited WrLpcReply.
        s.ready(t + 300.0, 3, 4000, 1004);
        s.sw(t + 310.0, 3, 0, 4000, state::WAITING, kwait::WR_LPC_RECEIVE);
        s.sw(t + 320.0, 0, 1004, 0, state::RUNNING, 0);
        s.sw(t + 400.0, 0, 0, 1004, state::WAITING, kwait::WR_USER_REQUEST);
    }
    // ALPC events from a later source: MainThread -> server.
    for i in 0..10 {
        s.m.push_link(&LinkEvent { ts: us(i as f64 * 50_000.0 + 7.0), from_tid: 1000, to_tid: 4000, kind: LinkKind::Ipc as u8, via: threadmap::events::via::ALPC, count: 2, ..Default::default() });
    }
    let g = s.snap(&Query { expand: vec![100], ..q_all() });
    let l = link(&g, "RenderThread", "server.exe", "ipc").unwrap();
    assert_eq!(l.count, 40, "20 requests + 20 replies, both client -> server");
    assert_eq!(l.via, vec!["lpc-wait"]);
    let a = link(&g, "MainThread", "server.exe", "ipc").unwrap();
    assert_eq!(a.count, 20);
    assert_eq!(a.via, vec!["alpc"]);
    assert!(link(&g, "server.exe", "RenderThread", "ipc").is_none());
}

#[test]
fn dpc_and_idle_readiers_go_to_the_kernel_node() {
    let mut s = base();
    for i in 0..10 {
        let t = i as f64 * 10_000.0;
        s.ready_dpc(t, 5, 1004);
        s.sw(t + 10.0, 5, 1004, 0, state::RUNNING, 0);
        s.sw(t + 100.0, 5, 0, 1004, state::WAITING, kwait::WR_USER_REQUEST);
        // Unknown readier on cpu 5 while it is idle: kernel work too.
        s.ready(t + 200.0, 5, READIER_UNKNOWN, 1000);
        s.sw(t + 210.0, 5, 1000, 0, state::RUNNING, 0);
        // Unknown readier on cpu 5 while MainThread runs there: MainThread readied the render thread.
        s.ready(t + 300.0, 5, READIER_UNKNOWN, 1004);
        s.sw(t + 400.0, 5, 1004, 1000, state::WAITING, kwait::WR_USER_REQUEST);
        s.sw(t + 500.0, 5, 0, 1004, state::WAITING, kwait::WR_USER_REQUEST);
    }
    let g = s.snap(&Query { expand: vec![100], ..q_all() });
    let k = node(&g, "DPC/ISR");
    assert_eq!(k.class, "kernel");
    assert_eq!(k.pid, PID_KERNEL);
    assert_eq!(link(&g, "DPC/ISR", "RenderThread", "wake").unwrap().count, 10);
    assert_eq!(link(&g, "DPC/ISR", "MainThread", "wake").unwrap().count, 10);
    assert_eq!(link(&g, "MainThread", "RenderThread", "wake").unwrap().count, 10);
    // Hidden with the system filter.
    let h = s.m.snapshot(&Query { expand: vec![100], hide_system: true, ..q_all() });
    assert!(!has_node(&h, "DPC/ISR"));
}

#[test]
fn node_metrics_cpu_states_and_p99() {
    let mut s = base();
    s.th(1008, 100, Some("Spinner"));
    s.th(1012, 100, Some("Sleeper"));
    s.th(1016, 100, Some("Queued"));
    // MainThread: every 1 ms readied (20 us wait), runs 500 us -> 0.5 cores.
    for i in 0..1000 {
        let t = i as f64 * 1000.0;
        s.ready(t, 0, 2000, 1000);
        s.sw(t + 20.0, 1, 1000, 0, state::RUNNING, 0);
        s.sw(t + 520.0, 1, 0, 1000, state::WAITING, kwait::WR_USER_REQUEST);
    }
    s.sw(0.0, 2, 1008, 0, state::RUNNING, 0); // Spinner runs all the time on cpu 2
    s.sw(10.0, 3, 1012, 0, state::RUNNING, 0);
    s.sw(50.0, 3, 0, 1012, state::WAITING, kwait::DELAY_EXECUTION);
    s.ready(990_000.0, 0, 2000, 1016); // Queued: ready at the end, never got a core
    s.sw(999_900.0, 4, 0, 0, state::RUNNING, 0);
    let g = s.snap(&Query { expand: vec![100], ..q_all() });
    let m = node(&g, "MainThread");
    assert!((m.cpu - 0.5).abs() < 0.01, "cpu {}", m.cpu);
    assert_eq!(m.waits, 1000);
    let p = m.wait_p99_us.unwrap();
    assert!((20.0..=24.0).contains(&p), "{p}");
    assert!((m.wait_ms - 20.0).abs() < 0.01);
    assert_eq!(m.state, "waiting");
    assert_eq!(m.prio, Some(9));
    let sp = node(&g, "Spinner");
    assert!((sp.cpu - 1.0).abs() < 0.01, "spinner {}", sp.cpu);
    assert_eq!(sp.state, "running");
    assert_eq!(node(&g, "Sleeper").state, "sleeping");
    let qd = node(&g, "Queued");
    assert_eq!(qd.state, "ready");
    let rf = qd.ready_for_us.unwrap();
    assert!((9_000.0..10_100.0).contains(&rf), "{rf}");
    let game = node(&g, "game.exe");
    assert_eq!(game.state, "running");
    assert!((game.cpu - 1.5).abs() < 0.02);
    // Series: game load ~1.5 cores per bucket, waits per bucket 100 x 20 us = 2 ms.
    let ser = s.m.series();
    let last_full = ser.load.len() - 2;
    assert!((ser.load[last_full] - 1.5).abs() < 0.02, "{}", ser.load[last_full]);
    assert!((ser.wait_ms[last_full] - 2.0).abs() < 0.01, "{}", ser.wait_ms[last_full]);
    assert_eq!(ser.waits[last_full], 100);
    assert!(ser.valid[0] == 0 && ser.valid[last_full] == 1);
}

#[test]
fn merging_workers_and_modules() {
    let mut s = base();
    for w in 0..12u32 {
        s.th(5000 + w * 4, 100, Some(&format!("Worker {w}")));
    }
    for w in 0..3u32 {
        s.m.thread_start(&ThreadInfo { ts: 0, tid: 6000 + w * 4, pid: 100, description: None, start_module: Some("engine.dll".into()), start_offset: 0x1234 });
    }
    for i in 0..10 {
        let t = i as f64 * 50_000.0;
        for w in 0..12u32 {
            s.ready(t + w as f64, 0, 1000, 5000 + w * 4);
            s.sw(t + 100.0 + w as f64 * 10.0, 1 + w % 4, 5000 + w * 4, 0, state::RUNNING, 0);
            s.sw(t + 2000.0 + w as f64 * 10.0, 1 + w % 4, 0, 5000 + w * 4, state::WAITING, kwait::WR_QUEUE);
        }
        for w in 0..3u32 {
            s.ready(t + 3000.0, 0, 2000, 6000 + w * 4);
            s.sw(t + 3100.0, 6 + w, 6000 + w * 4, 0, state::RUNNING, 0);
            s.sw(t + 3500.0, 6 + w, 0, 6000 + w * 4, state::WAITING, kwait::WR_QUEUE);
        }
    }
    let g = s.snap(&Query { expand: vec![100], ..q_all() });
    let w = node(&g, "Worker x12");
    assert_eq!(w.kind, "group");
    assert_eq!(w.count, 12);
    assert_eq!(w.members.len(), 12);
    assert_eq!(w.members[0].label, "Worker 0");
    assert_eq!(link(&g, "MainThread", "Worker x12", "wake").unwrap().count, 120);
    let e = node(&g, "engine.dll+0x1234 x3");
    assert_eq!(e.count, 3);
    assert!(!has_node(&g, "Worker 3"));
    // Unmerge the workers: 12 thread nodes, the module group stays.
    let g2 = s.m.snapshot(&Query { expand: vec![100], unmerge: vec![w.id], ..q_all() });
    assert!(has_node(&g2, "Worker 3") && has_node(&g2, "engine.dll+0x1234 x3"));
    assert_eq!(link(&g2, "MainThread", "Worker 3", "wake").unwrap().count, 10);
    // No merging at all.
    let g3 = s.m.snapshot(&Query { expand: vec![100], merge: false, ..q_all() });
    assert!(has_node(&g3, "Worker 11") && has_node(&g3, "engine.dll+0x1234"));
    // Ids are stable across rebuilds.
    let g4 = s.m.snapshot(&Query { expand: vec![100], ..q_all() });
    assert_eq!(node(&g4, "Worker x12").id, w.id);
}

fn filter_world() -> S {
    let mut s = base();
    s.proc(500, "csrss.exe");
    s.th(5000, 500, Some("Csrss"));
    s.proc(600, "far.exe");
    s.th(6000, 600, Some("Far"));
    s.proc(700, "far2.exe");
    s.th(7000, 700, Some("Far2"));
    for i in 0..100 {
        let t = i as f64 * 10_000.0;
        s.ready(t, 0, 2000, 1000); // helper -> game: 100/s
        s.ready(t + 1.0, 0, 5000, 1004); // csrss -> game: 100/s
        s.ready(t + 2.0, 0, 6000, 7000); // far -> far2: not linked to the game
        if i % 50 == 0 {
            s.ready(t + 3.0, 0, 1000, 6000); // game -> far: 2/s
        }
        if i % 2 == 0 {
            s.ready(t + 4.0, 0, 2000, 6000); // helper -> far: 50/s
        }
    }
    s.sw(999_000.0, 0, 0, 0, state::RUNNING, 0);
    s
}

#[test]
fn filters_focus_neighbours_system_rate_topn_kinds() {
    let mut s = filter_world();
    s.m.flush();
    let all = s.m.snapshot(&q_all());
    assert!(has_node(&all, "far2.exe") && has_node(&all, "csrss.exe"));
    // Focus: game + direct neighbours; far2 is two hops away.
    let f = s.m.snapshot(&Query { focus_pid: Some(100), ..q_all() });
    assert!(has_node(&f, "helper.exe") && has_node(&f, "far.exe") && has_node(&f, "csrss.exe"));
    assert!(!has_node(&f, "far2.exe"));
    // hide system: csrss gone (unless it is the focus).
    let h = s.m.snapshot(&Query { focus_pid: Some(100), hide_system: true, ..q_all() });
    assert!(!has_node(&h, "csrss.exe") && has_node(&h, "helper.exe"));
    let hf = s.m.snapshot(&Query { focus_pid: Some(500), hide_system: true, ..q_all() });
    assert!(has_node(&hf, "csrss.exe"));
    // Own system list.
    let own = s.m.snapshot(&Query { hide_system: true, system_names: Some(vec!["HELPER.EXE".into()]), ..q_all() });
    assert!(!has_node(&own, "helper.exe") && has_node(&own, "csrss.exe"));
    // Minimum rate: the 2/s game -> far link is cut, so far is no neighbour any more.
    let r = s.m.snapshot(&Query { focus_pid: Some(100), min_rate: 10.0, ..q_all() });
    assert!(!has_node(&r, "far.exe"));
    assert!(r.cut.links_min_rate >= 1);
    // Top-1 per node: the game keeps only its strongest link... but a link stays if it is in the top of either end.
    // game: helper (100/s) is its top link; far: helper (50/s) is its top link -> game -> far (2/s) is in neither.
    let t = s.m.snapshot(&Query { focus_pid: Some(100), top_n: 1, ..q_all() });
    assert_eq!(t.cut.links_top_n, 1);
    assert!(link(&t, "game.exe", "far.exe", "wake").is_none());
    assert!(link(&t, "helper.exe", "far.exe", "wake").is_some());
    // Kind mask: only preempt -> no links, only the focus.
    let k = s.m.snapshot(&Query { focus_pid: Some(100), kinds: 1 << (LinkKind::Preempt as u8), ..q_all() });
    assert!(k.links.is_empty());
    assert_eq!(k.nodes.len(), 1);
}

#[test]
fn node_cap_reports_cut() {
    let mut s = S::new();
    for p in 0..50u32 {
        let pid = 1000 + p * 4;
        s.proc(pid, &format!("p{p}.exe"));
        s.th(10_000 + p * 4, pid, None);
    }
    for i in 0..50u32 {
        // a chain p0 -> p1 -> ... with growing rates
        for r in 0..=i {
            s.ready(r as f64 * 100.0 + i as f64, 0, 10_000 + i * 4, 10_000 + ((i + 1) % 50) * 4);
        }
    }
    s.sw(500_000.0, 0, 0, 0, state::RUNNING, 0);
    let g = s.snap(&Query { max_nodes: 10, ..q_all() });
    assert_eq!(g.nodes.len(), 10);
    assert_eq!(g.cut.nodes, 40);
    assert!(g.cut.links_cut_nodes > 0);
    let ids: std::collections::HashSet<u64> = g.nodes.iter().map(|n| n.id).collect();
    assert!(g.links.iter().all(|l| ids.contains(&l.from) && ids.contains(&l.to)));
}

#[test]
fn history_scrub_and_ring_wrap() {
    let mut s = base();
    // 0..5 s: helper wakes the game 10/s; 5.0..5.5 s: a burst of 1000/s preempts; then quiet until 8 s.
    for i in 0..50 {
        s.ready(i as f64 * 100_000.0, 0, 2000, 1000);
    }
    for i in 0..500 {
        let t = 5_000_000.0 + i as f64 * 1000.0;
        s.sw(t, 0, 1000, 0, state::RUNNING, 0);
        s.sw(t + 400.0, 0, 2000, 1000, state::READY, kwait::WR_PREEMPTED);
        s.sw(t + 900.0, 0, 1000, 2000, state::WAITING, kwait::WR_USER_REQUEST);
        s.sw(t + 950.0, 0, 0, 1000, state::WAITING, kwait::WR_USER_REQUEST);
    }
    s.sw(8_000_000.0, 1, 0, 0, state::RUNNING, 0);
    s.m.flush();
    let now = s.m.snapshot(&q_all());
    assert!(now.window.live);
    assert!(link(&now, "helper.exe", "game.exe", "preempt").is_none());
    // Scrub: the window ending 2.6 s ago (bucket 54 = 5.4..5.5 s), 5 buckets long.
    let past = s.m.snapshot(&Query { end_age: 26, span: 5, ..q_all() });
    assert!(!past.window.live);
    assert!((past.window.seconds - 0.5).abs() < 1e-9);
    let p = link(&past, "helper.exe", "game.exe", "preempt").unwrap();
    assert_eq!(p.count, 500);
    assert!((p.rate - 1000.0).abs() < 1e-6);
    // State at the end of that past window: MainThread waiting (it switched out waiting at 5.4999 s).
    assert_eq!(node(&past, "game.exe").state, "waiting");
    // The time strip shows the game wait burst there.
    let ser = s.m.series();
    let i = ser.load.len() - 1 - 26;
    assert!(ser.waits[i] == 100 && ser.valid[i] == 1);
    assert!(ser.load[i] > 0.4);
    // 40 s later the ring has wrapped: the burst is gone, oldest age is 299.
    s.sw(48_000_000.0, 1, 0, 0, state::RUNNING, 0);
    s.m.flush();
    let gone = s.m.snapshot(&Query { end_age: 299, span: 300, ..q_all() });
    assert!(gone.links.is_empty());
    let ser = s.m.series();
    assert!(ser.valid.iter().all(|&v| v == 1));
    assert!(ser.waits.iter().all(|&w| w == 0));
}

#[test]
fn tid_reuse_and_missing_names() {
    let mut s = base();
    s.proc(900, "later.exe");
    // tid 3000 belongs to old.exe, ends, then the same tid starts in later.exe.
    s.proc(800, "old.exe");
    s.th(3000, 800, Some("OldThread"));
    for i in 0..10 {
        s.ready(i as f64 * 1000.0, 0, 3000, 1000);
    }
    s.m.thread_end(us(20_000.0), 3000);
    s.sw(20_010.0, 3, 0, 3000, state::TERMINATED, 0); // final switch-out after Thread End
    s.th_at(30_000.0, 3000, 900, Some("NewThread"));
    for i in 0..5 {
        s.ready(40_000.0 + i as f64 * 1000.0, 0, 3000, 1000);
    }
    // tid 7777 is never named: lands in "unknown threads".
    for i in 0..4 {
        s.ready(60_000.0 + i as f64 * 1000.0, 0, 7776, 1000);
    }
    // tid 5000 ends, then shows up again without any start event: a new (unknown) thread.
    s.th(5000, 200, Some("Gone"));
    s.m.thread_end(us(70_000.0), 5000);
    s.ready(80_000.0, 0, 1000, 5000);
    s.sw(90_000.0, 0, 0, 0, state::RUNNING, 0);
    let g = s.snap(&Query { expand: vec![800, 900, 200], ..q_all() });
    assert_eq!(link(&g, "OldThread", "game.exe", "wake").unwrap().count, 10);
    assert_eq!(link(&g, "NewThread", "game.exe", "wake").unwrap().count, 5);
    assert_eq!(node(&g, "OldThread").state, "gone");
    assert_eq!(link(&g, "unknown threads", "game.exe", "wake").unwrap().count, 4);
    assert_eq!(node(&g, "unknown threads").class, "unknown");
    assert!(link(&g, "game.exe", "unknown threads", "wake").is_some(), "reused tid 5000 without a start event");
    assert!(link(&g, "game.exe", "Gone", "wake").is_none());
    // Naming later moves the unknown thread to its process, also for the history.
    s.th_at(95_000.0, 7776, 200, Some("LateNamed"));
    s.sw(96_000.0, 0, 0, 0, state::RUNNING, 0);
    let g = s.snap(&Query { expand: vec![200], ..q_all() });
    assert_eq!(link(&g, "LateNamed", "game.exe", "wake").unwrap().count, 4);
}

/// One stream; `skew` delays each cpu's events by cpu * skew us in the feed order (each cpu stays in order).
fn mixed_stream(skew: f64) -> Vec<(f64, CSwitch, Option<ReadyThread>)> {
    let mut v = Vec::new();
    for i in 0..2000 {
        let t = i as f64 * 500.0;
        let cpu = (i % 4) as u32;
        let tid = 1000 + (i % 2) * 4;
        let r = ReadyThread { ts: us(t), cpu, readier_tid: 2000, target_tid: tid as u32, ..Default::default() };
        let a = CSwitch { ts: us(t + 30.0), cpu, new_tid: tid as u32, old_tid: 0, old_state: state::RUNNING, ..Default::default() };
        let b = CSwitch { ts: us(t + 300.0), cpu, new_tid: 0, old_tid: tid as u32, old_state: state::WAITING, old_wait_reason: kwait::WR_USER_REQUEST, ..Default::default() };
        let d = cpu as f64 * skew;
        v.push((t + d, a, Some(r)));
        v.push((t + 300.0 + d, b, None));
    }
    v.sort_by(|x, y| x.0.total_cmp(&y.0));
    v
}

fn run_mixed(skew: f64, reorder_ms: u32) -> (Snapshot, Stats) {
    let mut s = base();
    s.m.set_reorder_ms(reorder_ms);
    for (n, (_, c, r)) in mixed_stream(skew).into_iter().enumerate() {
        if let Some(r) = r {
            s.m.push_ready(&r);
        }
        s.m.push_cswitch(&c);
        if n % 97 == 0 {
            s.m.pump();
        }
    }
    let g = s.snap(&Query { expand: vec![100], span: 300, ..q_all() });
    let st = s.m.stats();
    (g, st)
}

#[test]
fn out_of_order_across_cpus() {
    let (sorted, st0) = run_mixed(0.0, 250);
    let (mixed, st1) = run_mixed(60_000.0, 250); // up to 180 ms skew, inside the 250 ms reorder window
    assert_eq!(st0.late_dropped, 0);
    assert_eq!(st1.late_dropped, 0);
    let strip = |s: &Snapshot| {
        let mut v = serde_json::to_value(s).unwrap();
        v.as_object_mut().unwrap().remove("stats");
        v
    };
    assert_eq!(strip(&sorted), strip(&mixed));
    let m = node(&mixed, "MainThread");
    assert_eq!(m.waits, 1000);
    // A reorder window shorter than the skew drops late events (counted).
    let (_, st2) = run_mixed(60_000.0, 10);
    assert!(st2.late_dropped > 0);
}

#[test]
fn layout_is_deterministic_persistent_and_pins() {
    let mut s = base();
    for i in 0..100 {
        let t = i as f64 * 10_000.0;
        s.ready(t, 0, 2000, 1004);
        s.ready(t + 1.0, 0, 1000, 1004);
    }
    let g = s.snap(&Query { expand: vec![100], ..q_all() });
    let mut a = Layout::new();
    let mut b = Layout::new();
    a.sync(&g);
    b.sync(&g);
    a.warm(200);
    b.warm(200);
    assert_eq!(a.positions(), b.positions());
    let e = a.warm(300);
    assert!(e < 0.05 * a.len() as f32, "settles: {e}");
    // Threads stay near their process node.
    let game = a.get(node(&g, "game.exe").id).unwrap();
    let main = a.get(node(&g, "MainThread").id).unwrap();
    assert!(((game.x - main.x).powi(2) + (game.y - main.y).powi(2)).sqrt() < 200.0);
    // Pinned nodes stay; positions persist across a rebuild by id.
    let hid = node(&g, "helper.exe").id;
    assert!(a.drag(hid, 300.0, -200.0));
    a.step(1.0 / 60.0);
    a.step(0.5);
    assert_eq!((a.get(hid).unwrap().x, a.get(hid).unwrap().y), (300.0, -200.0));
    let before = a.get(main.id).unwrap();
    let g2 = s.m.snapshot(&Query { expand: vec![100], ..q_all() });
    a.sync(&g2);
    assert_eq!(a.get(main.id).unwrap().x, before.x);
    assert_eq!(a.get(hid).unwrap().pinned, 1);
    // Collapse the game, expand again: the thread comes back where it was.
    let g3 = s.m.snapshot(&q_all());
    a.sync(&g3);
    assert!(a.get(main.id).is_none());
    a.sync(&g2);
    assert_eq!(a.get(main.id).unwrap().x, before.x);
    a.relayout();
    assert_eq!(a.get(hid).unwrap().pinned, 0);
}

