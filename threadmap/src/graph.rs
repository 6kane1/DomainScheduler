//! Snapshot building: sums a bucket range of the history, maps threads to the visible nodes (process, thread or
//! merged "Worker x12" group), aggregates links between them, applies the filters and the node cap.

use serde::{Deserialize, Serialize};

use crate::events::{via, LinkKind, LINK_KINDS};
use crate::fx::{fnv64, FxMap, FxSet};
use crate::history::{p99_hist, BINS, BUCKETS, BUCKET_NS};
use crate::kwait;
use crate::map::*;

/// Node ids: the kind in bits 48..50, the payload below. All ids stay below 2^53 (exact in JSON doubles).
pub const ID_PROCESS: u64 = 1 << 48;
pub const ID_THREAD: u64 = 2 << 48;
pub const ID_GROUP: u64 = 3 << 48;
const ID_MASK: u64 = (1 << 48) - 1;

/// Wait samples a node needs before it gets a p99 (ReadyProbe ProbePolicy.MinWaits).
pub const MIN_WAITS: u32 = 3;

/// Windows parts hidden by `hide_system` (compared case-insensitively with the image name). pid 0 and 4 and the
/// "DPC/ISR" pseudo process are always system.
pub const DEFAULT_SYSTEM: &[&str] = &[
    "Idle", "System", "Registry", "Secure System", "Memory Compression", "smss.exe", "csrss.exe", "wininit.exe",
    "winlogon.exe", "services.exe", "lsass.exe", "LsaIso.exe", "fontdrvhost.exe", "svchost.exe", "dwm.exe",
    "audiodg.exe", "WmiPrvSE.exe", "spoolsv.exe", "sihost.exe", "ctfmon.exe", "dllhost.exe", "taskhostw.exe",
    "RuntimeBroker.exe", "SearchIndexer.exe", "MsMpEng.exe", "NisSrv.exe", "MemCompression",
];

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(default)]
pub struct Query {
    /// Last bucket of the window as an age: 0 = now (the open bucket, live), 10 = one second ago.
    pub end_age: u32,
    /// Window length in buckets of 100 ms (1..=300).
    pub span: u32,
    /// Focus process (normally the game pid): only it and its direct neighbours are shown.
    pub focus_pid: Option<u32>,
    /// With a focus: also show the processes linked to it (false: the focus process alone).
    pub neighbours: bool,
    /// Links below this rate (per second) are dropped.
    pub min_rate: f64,
    pub hide_system: bool,
    /// Replaces DEFAULT_SYSTEM when set.
    pub system_names: Option<Vec<String>>,
    /// Keep only the N strongest links of each node (a link stays when it is in the top N of either end); 0 = all.
    pub top_n: u32,
    /// Merge similar threads of an expanded process into one "Worker xN" node.
    pub merge: bool,
    /// Processes (pids) shown as their threads.
    pub expand: Vec<u32>,
    /// Merged group ids to show as single threads again.
    pub unmerge: Vec<u64>,
    /// Bit mask of link kinds (1 << LinkKind): 0b1111 = all.
    pub kinds: u8,
    /// Visible nodes cap (process group nodes count too); at most 600.
    pub max_nodes: u32,
    /// Without a focus, nodes without links are shown only from this cpu use (cores).
    pub min_cpu: f64,
}

impl Default for Query {
    fn default() -> Self {
        Self {
            end_age: 0,
            span: 10,
            focus_pid: None,
            neighbours: true,
            min_rate: 1.0,
            hide_system: true,
            system_names: None,
            top_n: 6,
            merge: true,
            expand: Vec::new(),
            unmerge: Vec::new(),
            kinds: 0b1111,
            max_nodes: 300,
            min_cpu: 0.01,
        }
    }
}

#[derive(Clone, Debug, Serialize, Default)]
pub struct Window {
    pub from_age: u32,
    pub to_age: u32,
    pub buckets: u32,
    pub seconds: f64,
    pub live: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct Member {
    pub id: u64,
    pub tid: u32,
    pub label: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct Node {
    pub id: u64,
    /// "process" | "thread" | "group" (merged threads).
    pub kind: &'static str,
    /// "game" | "background" | "system" | "kernel" | "unknown".
    pub class: &'static str,
    pub pid: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tid: Option<u32>,
    /// The process node of a thread or group node.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent: Option<u64>,
    pub label: String,
    /// Process image ("cs2.exe").
    pub image: String,
    /// Threads this node stands for that were active in the window.
    pub count: u32,
    /// Process nodes: live threads known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub threads_alive: Option<u32>,
    /// Process nodes: shown as their threads (the UI draws a halo, not a circle).
    pub expanded: bool,
    /// Cores used over the window (run time / window).
    pub cpu: f64,
    /// "running" | "ready" (ready, no core) | "wait_lock" | "wait_ipc" | "waiting" | "sleeping" | "gone" | "unknown".
    pub state: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wait_reason: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wait_reason_name: Option<&'static str>,
    /// Live and ready: how long it has been waiting for a core so far.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ready_for_us: Option<f64>,
    /// p99 of waits for a core (ready -> running) in the window, us, rounded up to its histogram bin.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wait_p99_us: Option<f64>,
    pub waits: u32,
    pub wait_ms: f64,
    /// Wakes per second received / sent (all wake links of its threads, also inside the node).
    pub wakes_in: f64,
    pub wakes_out: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prio: Option<i8>,
    /// Single-thread nodes: the logical cpu it ran on last.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub core: Option<u32>,
    /// Members running / ready at the end of the window.
    pub running: u32,
    pub ready: u32,
    /// Radius for drawing and the layout.
    pub r: f32,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub members: Vec<Member>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Link {
    pub from: u64,
    pub to: u64,
    pub kind: &'static str,
    /// Per second over the window.
    pub rate: f64,
    pub count: u64,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub via: Vec<&'static str>,
}

#[derive(Clone, Debug, Serialize, Default)]
pub struct Cut {
    /// Nodes left out by max_nodes.
    pub nodes: u32,
    pub links_min_rate: u32,
    pub links_top_n: u32,
    /// Links to hidden (system, not a neighbour) nodes.
    pub links_hidden: u32,
    /// Links to nodes cut by max_nodes.
    pub links_cut_nodes: u32,
    pub max_nodes: u32,
}

#[derive(Clone, Debug, Serialize, Default)]
pub struct Snapshot {
    pub window: Window,
    pub game_pid: Option<u32>,
    pub cpu_count: u32,
    pub nodes: Vec<Node>,
    pub links: Vec<Link>,
    pub cut: Cut,
    pub stats: Stats,
}

#[derive(Default, Clone, Copy)]
struct Acc {
    run: u64,
    ready: u64,
    waits: u32,
    wakes_in: u64,
    wakes_out: u64,
}

struct Cand {
    f: u32,
    t: u32,
    k: u8,
    v: u8,
    c: u64,
    rate: f64,
    idf: u64,
    idt: u64,
}

struct Leaf {
    id: u64,
    kind: &'static str,
    proc: u32,
    label: String,
    members: Vec<u32>, // dense thread indices
    acc: Acc,
    count: u32,
}

impl Acc {
    #[inline]
    fn add(&mut self, run: impl Into<u64>, ready: impl Into<u64>, waits: u32) {
        self.run += run.into();
        self.ready += ready.into();
        self.waits += waits;
    }
}

/// Unit flag: a process unit (else a dense thread index).
const PU: u32 = 1 << 31;

/// Collapsed processes seen in the window.
#[derive(Default)]
struct Procs {
    idx: FxMap<u32, u32>,
    serial: Vec<u32>,
    acc: Vec<Acc>,
    /// Max threads with records in one bucket.
    count: Vec<u32>,
}

impl Procs {
    #[inline]
    fn idx(&mut self, p: u32) -> u32 {
        let next = self.serial.len() as u32;
        let e = self.idx.entry(p).or_insert(next);
        if *e == next {
            self.serial.push(p);
            self.acc.push(Acc::default());
            self.count.push(0);
        }
        *e
    }
}

/// The unit of a thread record: its dense index when its process is expanded, else its process.
#[inline]
fn unit(d: &mut Dense, pr: &mut Procs, exp: &FxSet<u32>, serial: u32, proc: u32) -> u32 {
    if proc == NONE {
        NONE
    } else if exp.contains(&proc) {
        d.idx(serial)
    } else {
        PU | pr.idx(proc)
    }
}

/// State summary of a node: the most telling state of its threads.
#[derive(Clone, Copy)]
struct Sum {
    best: &'static str,
    reason: u8,
    running: u32,
    ready: u32,
    ready_for: Option<f64>,
}

impl Default for Sum {
    fn default() -> Self {
        Self { best: "unknown", reason: 0, running: 0, ready: 0, ready_for: None }
    }
}

impl Sum {
    fn add(&mut self, st: RunState, reason: u8, ready_for: Option<f64>) {
        let name = state_name(st, reason);
        if rank(name) > rank(self.best) {
            self.best = name;
            self.reason = reason;
        }
        match st {
            RunState::Running => self.running += 1,
            RunState::Ready => self.ready += 1,
            _ => {}
        }
        if let Some(v) = ready_for {
            self.ready_for = Some(self.ready_for.map_or(v, |x: f64| x.max(v)));
        }
    }
}

fn rank(state: &'static str) -> u8 {
    match state {
        "running" => 7,
        "ready" => 6,
        "wait_lock" => 5,
        "wait_ipc" => 4,
        "waiting" => 3,
        "sleeping" => 2,
        "gone" => 1,
        _ => 0,
    }
}

fn state_name(st: RunState, reason: u8) -> &'static str {
    match st {
        RunState::Running => "running",
        RunState::Ready => "ready",
        RunState::Waiting if kwait::is_lock(reason) => "wait_lock",
        RunState::Waiting if kwait::is_ipc(reason) => "wait_ipc",
        RunState::Waiting if kwait::is_sleep(reason) => "sleeping",
        RunState::Waiting => "waiting",
        RunState::Gone => "gone",
        RunState::Unknown => "unknown",
    }
}

/// "Worker 12" -> "Worker", "TaskThread_3" -> "TaskThread", "pool#07" -> "pool"; None when nothing is left.
pub fn name_prefix(desc: &str) -> Option<&str> {
    let p = desc.trim_end_matches(|c: char| c.is_ascii_digit()).trim_end_matches([' ', '_', '-', '#', ':', '.', '/', '(', '[']);
    let p = p.trim();
    (!p.is_empty()).then_some(p)
}

pub(crate) fn thread_label(t: &Thread) -> String {
    if let Some(d) = t.desc.as_deref() {
        return d.to_string();
    }
    if let Some(m) = t.start_module.as_deref() {
        return format!("{}+0x{:x}", m, t.start_offset);
    }
    format!("tid {}", t.tid)
}

/// Merge key and group label: the description prefix, else the start module+offset.
fn merge_key(t: &Thread) -> Option<(String, String)> {
    if let Some(d) = t.desc.as_deref() {
        if let Some(p) = name_prefix(d) {
            return Some((format!("d:{p}"), p.to_string()));
        }
    }
    let m = t.start_module.as_deref()?;
    let l = format!("{}+0x{:x}", m, t.start_offset);
    Some((format!("m:{l}"), l))
}

fn proc_class(p: &Proc, game: Option<u32>, system: &FxSet<String>) -> &'static str {
    if Some(p.pid) == game {
        return "game";
    }
    if p.pid == PID_KERNEL {
        return "kernel";
    }
    if p.pid == PID_UNKNOWN {
        return "unknown";
    }
    if p.pid == 0 || p.pid == 4 || p.image.as_deref().is_some_and(|i| system.contains(&i.to_ascii_lowercase())) {
        return "system";
    }
    "background"
}

fn proc_label(p: &Proc) -> String {
    match p.pid {
        PID_UNKNOWN => "unknown threads".into(),
        _ => p.image.clone().unwrap_or_else(|| format!("pid {}", p.pid)),
    }
}

fn radius(kind: &str, expanded: bool, cpu: f64) -> f32 {
    match kind {
        "process" if expanded => 26.0,
        "process" => 14.0 + (cpu.max(0.0).sqrt() * 12.0).min(18.0) as f32,
        "group" => 17.0,
        _ => 13.0 + (cpu.clamp(0.0, 1.0) * 8.0) as f32,
    }
}

impl ThreadMap {
    /// Builds the graph for a window of the history. Pure read: call `pump()` before for live data.
    pub fn snapshot(&self, q: &Query) -> Snapshot {
        build(self, q)
    }
}

fn build(m: &ThreadMap, q: &Query) -> Snapshot {
    let mut snap = Snapshot { game_pid: m.game_pid, cpu_count: m.cpu_count(), stats: m.stats(), ..Default::default() };
    let max_nodes = q.max_nodes.clamp(1, 600);
    snap.cut.max_nodes = max_nodes;
    let (Some(newest), Some(oldest)) = (m.ring.newest, m.ring.oldest()) else { return snap };
    let k1 = newest - q.end_age.min(BUCKETS as u32 - 1) as i64;
    if k1 < oldest {
        return snap;
    }
    let k0 = (k1 - q.span.clamp(1, BUCKETS as u32) as i64 + 1).max(oldest);
    let live = k1 == newest;
    let window_ns = if live {
        (m.now_ns - k1 * BUCKET_NS).clamp(1_000_000, BUCKET_NS) + (k1 - k0) * BUCKET_NS
    } else {
        (k1 - k0 + 1) * BUCKET_NS
    };
    let secs = window_ns as f64 / 1e9;
    snap.window = Window { from_age: (newest - k0) as u32, to_age: (newest - k1) as u32, buckets: (k1 - k0 + 1) as u32, seconds: secs, live };

    // 1. Sum the buckets into units: a thread of an expanded process (dense index) or a whole process (PU | index).
    //    Stamped buckets give collapsed processes straight from their per-process sums; thread records are read
    //    only for expanded processes. Buckets with stale stamps resolve each record's process (slow path).
    let system: FxSet<String> = match &q.system_names {
        Some(v) => v.iter().map(|s| s.to_ascii_lowercase()).collect(),
        None => DEFAULT_SYSTEM.iter().map(|s| s.to_ascii_lowercase()).collect(),
    };
    let exp: FxSet<u32> = m.procs.iter().filter(|(_, p)| q.expand.contains(&p.pid)).map(|(&s, _)| s).collect();
    let mut exp_list: Vec<u32> = exp.iter().copied().collect();
    exp_list.sort_unstable();
    let unmerge: FxSet<u64> = q.unmerge.iter().copied().collect();
    let resolve = |s: u32| m.threads.get(&s).map(|t| t.proc).unwrap_or(NONE);
    let epoch = m.proc_epoch;
    let mut d = Dense::new(m);
    let mut pr = Procs::default();
    let mut twaits: Vec<(u32, u8, u32)> = Vec::new(); // dense, bin, count
    let mut pwaits: Vec<(u32, u8, u32)> = Vec::new(); // proc unit, bin, count
    let mut ulinks: Vec<(u32, u32, u8, u8, u32)> = Vec::new(); // unit from, unit to, kind, via, count
    let mut bcount: FxMap<u32, u32> = FxMap::default();
    for k in k0..=k1 {
        let Some(b) = m.ring.get(k) else { continue };
        if b.stamped(epoch) {
            b.for_p_th(|r| {
                if r.proc != NONE && !exp.contains(&r.proc) {
                    let i = pr.idx(r.proc) as usize;
                    pr.acc[i].add(r.run_ns, r.ready_ns, r.waits);
                    pr.count[i] = pr.count[i].max(r.threads);
                }
            });
            b.for_p_waits(|w| {
                if w.proc != NONE && !exp.contains(&w.proc) {
                    pwaits.push((pr.idx(w.proc), w.bin, w.count));
                }
            });
            b.for_p_links(|l| {
                if l.pf != NONE && l.pt != NONE && !exp.contains(&l.pf) && !exp.contains(&l.pt) {
                    ulinks.push((PU | pr.idx(l.pf), PU | pr.idx(l.pt), l.kind, l.via_mask, l.count));
                }
            });
            for &p in &exp_list {
                b.for_th_of(p, |r| {
                    let i = d.idx(r.serial);
                    if i != NONE {
                        d.accs[i as usize].add(r.run_ns, r.ready_ns, r.waits);
                    }
                });
                b.for_waits_of(p, |w| {
                    let i = d.idx(w.serial);
                    if i != NONE {
                        twaits.push((i, w.bin, w.count));
                    }
                });
            }
            if !exp_list.is_empty() {
                b.for_links_touching(&exp_list, |l| {
                    let f = unit(&mut d, &mut pr, &exp, l.from, l.pf);
                    let t = unit(&mut d, &mut pr, &exp, l.to, l.pt);
                    if f != NONE && t != NONE {
                        ulinks.push((f, t, l.kind, l.via_mask, l.count));
                    }
                });
            }
        } else {
            bcount.clear();
            b.for_th(|r| {
                let p = resolve(r.serial);
                if p == NONE {
                    return;
                }
                if exp.contains(&p) {
                    let i = d.idx(r.serial);
                    if i != NONE {
                        d.accs[i as usize].add(r.run_ns, r.ready_ns, r.waits);
                    }
                } else {
                    let i = pr.idx(p);
                    pr.acc[i as usize].add(r.run_ns, r.ready_ns, r.waits);
                    *bcount.entry(i).or_default() += 1;
                }
            });
            for (&i, &c) in &bcount {
                pr.count[i as usize] = pr.count[i as usize].max(c);
            }
            b.for_waits(|w| {
                let p = resolve(w.serial);
                let u = unit(&mut d, &mut pr, &exp, w.serial, p);
                if u == NONE {
                } else if u & PU != 0 {
                    pwaits.push((u & !PU, w.bin, w.count));
                } else {
                    twaits.push((u, w.bin, w.count));
                }
            });
            b.for_links(|l| {
                let f = unit(&mut d, &mut pr, &exp, l.from, resolve(l.from));
                let t = unit(&mut d, &mut pr, &exp, l.to, resolve(l.to));
                if f != NONE && t != NONE {
                    ulinks.push((f, t, l.kind, l.via_mask, l.count));
                }
            });
        }
    }
    if live {
        for s in m.live_active() {
            unit(&mut d, &mut pr, &exp, s, resolve(s));
        }
    }
    let serials = std::mem::take(&mut d.serials);
    let taccs = std::mem::take(&mut d.accs);

    // 2. Leaves: collapsed processes, and the threads (or merged groups) of expanded ones.
    let mut leaves: Vec<Leaf> = Vec::new();
    let mut leaf_p: Vec<u32> = vec![NONE; pr.serial.len()];
    for (i, &p) in pr.serial.iter().enumerate() {
        leaf_p[i] = leaves.len() as u32;
        leaves.push(Leaf { id: ID_PROCESS | p as u64, kind: "process", proc: p, label: String::new(), members: Vec::new(), acc: pr.acc[i], count: pr.count[i] });
    }
    let n = serials.len();
    let proc_of: Vec<u32> = serials.iter().map(|&s| resolve(s)).collect();
    let mut gkey: Vec<Option<(u64, String)>> = vec![None; n];
    let mut gcount: FxMap<u64, u32> = FxMap::default();
    if q.merge {
        for i in 0..n {
            let p = proc_of[i];
            let Some(t) = m.threads.get(&serials[i]) else { continue };
            if p == NONE {
                continue;
            }
            if let Some((key, label)) = merge_key(t) {
                let id = ID_GROUP | (fnv64(&[&p.to_le_bytes(), key.as_bytes()]) & ID_MASK);
                if !unmerge.contains(&id) {
                    *gcount.entry(id).or_default() += 1;
                    gkey[i] = Some((id, label));
                }
            }
        }
    }
    let mut leaf_of_id: FxMap<u64, u32> = FxMap::default();
    let mut leaf_t: Vec<u32> = vec![NONE; n];
    for i in 0..n {
        let p = proc_of[i];
        let Some(t) = m.threads.get(&serials[i]) else { continue };
        if p == NONE {
            continue;
        }
        let (id, kind, label) = match &gkey[i] {
            Some((gid, label)) if gcount.get(gid).copied().unwrap_or(0) >= 2 => (*gid, "group", label.clone()),
            _ => (ID_THREAD | serials[i] as u64, "thread", thread_label(t)),
        };
        let li = *leaf_of_id.entry(id).or_insert_with(|| {
            leaves.push(Leaf { id, kind, proc: p, label, members: Vec::new(), acc: Acc::default(), count: 0 });
            (leaves.len() - 1) as u32
        });
        let l = &mut leaves[li as usize];
        l.members.push(i as u32);
        l.count += 1;
        let a = taccs[i];
        l.acc.add(a.run, a.ready, a.waits);
        leaf_t[i] = li;
    }
    let leaf_u = |u: u32| if u & PU != 0 { leaf_p[(u & !PU) as usize] } else { leaf_t[u as usize] };

    // 3. Links between leaves (self-links dropped), rate and kind filters. Up to 512 leaves the sums go into a
    //    dense leaf x leaf x kind matrix (no hashing); above that into a hash map.
    let nl = leaves.len();
    let use_matrix = nl <= 512;
    let cells = if use_matrix { nl * nl * LINK_KINDS } else { 0 };
    let mut mcnt: Vec<u64> = vec![0; cells];
    let mut mvia: Vec<u8> = vec![0; cells];
    let mut ll: FxMap<u64, (u64, u8)> = FxMap::default();
    for &(f, t, kind, via, count) in &ulinks {
        let (lf, lt) = (leaf_u(f), leaf_u(t));
        if lf == NONE || lt == NONE {
            continue;
        }
        if kind == LinkKind::Wake as u8 {
            leaves[lf as usize].acc.wakes_out += count as u64;
            leaves[lt as usize].acc.wakes_in += count as u64;
        }
        if lf == lt || kind as usize >= LINK_KINDS || q.kinds & (1 << kind) == 0 {
            continue;
        }
        if use_matrix {
            let i = (lf as usize * nl + lt as usize) * LINK_KINDS + kind as usize;
            mcnt[i] += count as u64;
            mvia[i] |= via;
        } else {
            let e = ll.entry((lf as u64) << 36 | (lt as u64) << 8 | kind as u64).or_default();
            e.0 += count as u64;
            e.1 |= via;
        }
    }
    let mut cand: Vec<Cand> = Vec::new();
    let mut add = |f: u32, t: u32, k: u8, c: u64, v: u8, cut: &mut Cut| {
        let rate = c as f64 / secs;
        if rate < q.min_rate {
            cut.links_min_rate += 1;
        } else {
            cand.push(Cand { f, t, k, v, c, rate, idf: leaves[f as usize].id, idt: leaves[t as usize].id });
        }
    };
    if use_matrix {
        for (i, &c) in mcnt.iter().enumerate() {
            if c > 0 {
                let (pair, k) = (i / LINK_KINDS, (i % LINK_KINDS) as u8);
                add((pair / nl) as u32, (pair % nl) as u32, k, c, mvia[i], &mut snap.cut);
            }
        }
    } else {
        for (&key, &(c, v)) in &ll {
            add((key >> 36) as u32, ((key >> 8) & ((1 << 28) - 1)) as u32, (key & 0xff) as u8, c, v, &mut snap.cut);
        }
    }
    drop(add);
    cand.sort_unstable_by(|a, b| b.rate.total_cmp(&a.rate).then(a.idf.cmp(&b.idf)).then(a.idt.cmp(&b.idt)).then(a.k.cmp(&b.k)));

    // 4. Visibility: hidden system processes, focus + neighbours.
    let class_of = |p: u32| m.procs.get(&p).map(|pr| proc_class(pr, m.game_pid, &system)).unwrap_or("unknown");
    let focus: FxSet<u32> = match q.focus_pid {
        Some(fp) => m.procs.iter().filter(|(_, p)| p.pid == fp).map(|(&s, _)| s).collect(),
        None => FxSet::default(),
    };
    let hidden = |p: u32| q.hide_system && !focus.contains(&p) && matches!(class_of(p), "system" | "kernel");
    let mut visible_proc: FxSet<u32> = FxSet::default();
    if q.focus_pid.is_some() {
        visible_proc.extend(focus.iter().copied());
        if q.neighbours {
            for c in &cand {
                let (pf, pt) = (leaves[c.f as usize].proc, leaves[c.t as usize].proc);
                if focus.contains(&pf) && !hidden(pt) {
                    visible_proc.insert(pt);
                }
                if focus.contains(&pt) && !hidden(pf) {
                    visible_proc.insert(pf);
                }
            }
        }
    } else {
        for l in &leaves {
            if !hidden(l.proc) {
                visible_proc.insert(l.proc);
            }
        }
    }
    let before = cand.len();
    cand.retain(|c| visible_proc.contains(&leaves[c.f as usize].proc) && visible_proc.contains(&leaves[c.t as usize].proc));
    snap.cut.links_hidden = (before - cand.len()) as u32;

    // 5. Top-N links per node.
    if q.top_n > 0 {
        let mut per: FxMap<u32, u32> = FxMap::default();
        let mut keep = vec![false; cand.len()];
        for (i, c) in cand.iter().enumerate() {
            // cand is sorted strongest first, so the first N seen per node are its top N.
            for end in [c.f, c.t] {
                let e = per.entry(end).or_default();
                if *e < q.top_n {
                    *e += 1;
                    keep[i] = true;
                }
            }
        }
        let before = cand.len();
        let mut i = 0;
        cand.retain(|_| {
            i += 1;
            keep[i - 1]
        });
        snap.cut.links_top_n = (before - cand.len()) as u32;
    }

    // 6. Candidate leaves and the node cap.
    let mut link_rate = vec![0f64; leaves.len()];
    for c in &cand {
        link_rate[c.f as usize] += c.rate;
        link_rate[c.t as usize] += c.rate;
    }
    let has_link: FxSet<u32> = cand.iter().flat_map(|c| [c.f, c.t]).collect();
    let mut order: Vec<(u32, f64)> = Vec::new();
    for (li, l) in leaves.iter().enumerate() {
        if !visible_proc.contains(&l.proc) {
            continue;
        }
        let cpu = l.acc.run as f64 / window_ns as f64;
        let in_focus = focus.contains(&l.proc);
        let linked = has_link.contains(&(li as u32));
        if !(in_focus || linked || (q.focus_pid.is_none() && cpu >= q.min_cpu)) {
            continue;
        }
        let score = if in_focus { 1e15 + cpu } else { link_rate[li] + cpu * 100.0 };
        order.push((li as u32, score));
    }
    order.sort_by(|a, b| b.1.total_cmp(&a.1).then(leaves[a.0 as usize].id.cmp(&leaves[b.0 as usize].id)));
    let mut kept: FxSet<u32> = FxSet::default();
    let mut group_nodes: FxSet<u32> = FxSet::default(); // expanded procs with a kept leaf
    let mut used = 0u32;
    for &(li, _) in &order {
        let l = &leaves[li as usize];
        let extra = if l.kind != "process" && !group_nodes.contains(&l.proc) { 2 } else { 1 };
        if used + extra > max_nodes {
            snap.cut.nodes += 1;
            continue;
        }
        used += extra;
        if l.kind != "process" {
            group_nodes.insert(l.proc);
        }
        kept.insert(li);
    }
    let before = cand.len();
    cand.retain(|c| kept.contains(&c.f) && kept.contains(&c.t));
    snap.cut.links_cut_nodes = (before - cand.len()) as u32;

    // 7. States at the end of the window (live: current; past: the newest state record at or before the end).
    //    Thread leaves: per member. Collapsed processes: the most telling state of their threads.
    let mut need_t: FxSet<u32> = FxSet::default();
    let mut need_p: FxSet<u32> = FxSet::default();
    for &li in &kept {
        let l = &leaves[li as usize];
        if l.kind == "process" {
            need_p.insert(l.proc);
        }
        for &i in &l.members {
            need_t.insert(serials[i as usize]);
        }
    }
    let now = m.now_ns;
    let mut psum: FxMap<u32, Sum> = FxMap::default();
    let mut past_t: FxMap<u32, (u8, u8)> = FxMap::default();
    if live {
        if !need_p.is_empty() {
            let from = k0 * BUCKET_NS;
            for t in m.threads.values() {
                if t.ended.is_none() && t.last_seen >= from && need_p.contains(&t.proc) {
                    let rf = t.ready_since.filter(|_| t.state == RunState::Ready).map(|r| (now - r) as f64 / 1000.0);
                    psum.entry(t.proc).or_default().add(t.state, t.reason, rf);
                }
            }
        }
    } else {
        let mut k = k1;
        let mut done: FxSet<u32> = FxSet::default();
        while k >= oldest && (past_t.len() < need_t.len() || done.len() < need_p.len()) {
            if let Some(b) = m.ring.get(k) {
                let stamped = b.stamped(epoch);
                let mut seen: FxMap<u32, Sum> = FxMap::default();
                b.for_states(|r| {
                    if need_t.contains(&r.serial) {
                        past_t.entry(r.serial).or_insert((r.st, r.reason));
                    }
                    let p = if stamped && r.proc != NONE { r.proc } else { resolve(r.serial) };
                    if need_p.contains(&p) && !done.contains(&p) {
                        seen.entry(p).or_default().add(RunState::from_u8(r.st), r.reason, None);
                    }
                });
                for (p, s) in seen {
                    done.insert(p);
                    psum.insert(p, s);
                }
            }
            k -= 1;
        }
    }
    let state_of = |s: u32| -> (RunState, u8) {
        if live {
            m.threads.get(&s).map(|t| (t.state, t.reason)).unwrap_or((RunState::Unknown, 0))
        } else {
            past_t.get(&s).map(|&(st, r)| (RunState::from_u8(st), r)).unwrap_or((RunState::Unknown, 0))
        }
    };

    // 8. p99 per kept leaf and per expanded process (histograms only for kept nodes).
    let mut lslot = vec![NONE; leaves.len()];
    let mut gslot: FxMap<u32, u32> = FxMap::default();
    let mut hl: Vec<[u32; BINS]> = Vec::new();
    let mut hg: Vec<[u32; BINS]> = Vec::new();
    let mut kept_sorted: Vec<u32> = kept.iter().copied().collect();
    kept_sorted.sort_unstable();
    for &li in &kept_sorted {
        lslot[li as usize] = hl.len() as u32;
        hl.push([0; BINS]);
        let p = leaves[li as usize].proc;
        if group_nodes.contains(&p) && !gslot.contains_key(&p) {
            gslot.insert(p, hg.len() as u32);
            hg.push([0; BINS]);
        }
    }
    for &(i, b, c) in &twaits {
        let li = leaf_t[i as usize];
        if li == NONE || lslot[li as usize] == NONE {
            continue;
        }
        hl[lslot[li as usize] as usize][b as usize] += c;
        if let Some(&g) = gslot.get(&leaves[li as usize].proc) {
            hg[g as usize][b as usize] += c;
        }
    }
    for &(i, b, c) in &pwaits {
        let li = leaf_p[i as usize];
        if li != NONE && lslot[li as usize] != NONE {
            hl[lslot[li as usize] as usize][b as usize] += c;
        }
    }
    let p99l: FxMap<u32, f64> =
        kept_sorted.iter().filter_map(|&li| p99_hist(&hl[lslot[li as usize] as usize], MIN_WAITS).map(|v| (li, v))).collect();
    let p99p: FxMap<u32, f64> = gslot.iter().filter_map(|(&p, &sl)| p99_hist(&hg[sl as usize], MIN_WAITS).map(|v| (p, v))).collect();

    // Live threads per process.
    let mut alive: FxMap<u32, u32> = FxMap::default();
    for t in m.threads.values() {
        if t.ended.is_none() {
            *alive.entry(t.proc).or_default() += 1;
        }
    }

    // 9. Nodes, ordered: game first, then by cpu; an expanded process before its threads.
    let mut procs_out: FxMap<u32, Vec<u32>> = FxMap::default();
    for &li in &kept {
        procs_out.entry(leaves[li as usize].proc).or_default().push(li);
    }
    let mut proc_list: Vec<(u32, f64)> = procs_out
        .iter()
        .map(|(&p, ls)| (p, ls.iter().map(|&li| leaves[li as usize].acc.run as f64).sum::<f64>() / window_ns as f64))
        .collect();
    proc_list.sort_by(|a, b| {
        let ga = (class_of(a.0) != "game") as u8;
        let gb = (class_of(b.0) != "game") as u8;
        ga.cmp(&gb).then(b.1.total_cmp(&a.1)).then(a.0.cmp(&b.0))
    });
    let make = |li: u32, parent: Option<u64>| -> Node {
        let l = &leaves[li as usize];
        let pr = m.procs.get(&l.proc);
        let cpu = l.acc.run as f64 / window_ns as f64;
        let mut sum = if l.kind == "process" { psum.get(&l.proc).copied().unwrap_or_default() } else { Sum::default() };
        let mut members = Vec::new();
        for &i in &l.members {
            let s = serials[i as usize];
            let (st, reason) = state_of(s);
            let rf = (live && st == RunState::Ready).then(|| m.ready_since(s)).flatten().map(|r| (now - r) as f64 / 1000.0);
            sum.add(st, reason, rf);
            if l.kind == "group" {
                if let Some(t) = m.threads.get(&s) {
                    members.push(Member { id: ID_THREAD | s as u64, tid: t.tid, label: thread_label(t) });
                }
            }
        }
        members.sort_by_key(|x| x.tid);
        let waiting = matches!(sum.best, "wait_lock" | "wait_ipc" | "waiting" | "sleeping");
        let single = (l.kind == "thread").then(|| m.threads.get(&serials[l.members[0] as usize])).flatten();
        let label = match l.kind {
            "group" => format!("{} x{}", l.label, l.members.len()),
            "thread" => l.label.clone(),
            _ => pr.map(proc_label).unwrap_or_default(),
        };
        Node {
            id: l.id,
            kind: l.kind,
            class: class_of(l.proc),
            pid: pr.map(|p| p.pid).unwrap_or(PID_UNKNOWN),
            tid: single.map(|t| t.tid),
            parent,
            label,
            image: pr.map(proc_label).unwrap_or_default(),
            count: l.count,
            threads_alive: (l.kind == "process").then(|| alive.get(&l.proc).copied().unwrap_or(0)),
            expanded: false,
            cpu,
            state: sum.best,
            wait_reason: waiting.then_some(sum.reason),
            wait_reason_name: waiting.then(|| kwait::reason_name(sum.reason)),
            ready_for_us: sum.ready_for,
            wait_p99_us: p99l.get(&li).copied(),
            waits: l.acc.waits,
            wait_ms: l.acc.ready as f64 / 1e6,
            wakes_in: l.acc.wakes_in as f64 / secs,
            wakes_out: l.acc.wakes_out as f64 / secs,
            prio: single.map(|t| t.prio),
            core: single.and_then(|t| (t.last_cpu != NONE).then_some(t.last_cpu)),
            running: sum.running,
            ready: sum.ready,
            r: radius(l.kind, false, cpu),
            members,
        }
    };
    for &(p, _) in &proc_list {
        let mut ls = procs_out.remove(&p).unwrap_or_default();
        ls.sort_by(|a, b| {
            let (la, lb) = (&leaves[*a as usize], &leaves[*b as usize]);
            lb.acc.run.cmp(&la.acc.run).then(la.id.cmp(&lb.id))
        });
        if group_nodes.contains(&p) {
            // The expanded process itself: sums of all its kept leaves.
            let kids: Vec<Node> = ls.iter().map(|&li| make(li, Some(ID_PROCESS | p as u64))).collect();
            let pr = m.procs.get(&p);
            let cpu: f64 = kids.iter().map(|k| k.cpu).sum();
            let best = kids.iter().max_by_key(|k| rank(k.state));
            snap.nodes.push(Node {
                id: ID_PROCESS | p as u64,
                kind: "process",
                class: class_of(p),
                pid: pr.map(|x| x.pid).unwrap_or(PID_UNKNOWN),
                tid: None,
                parent: None,
                label: pr.map(proc_label).unwrap_or_default(),
                image: pr.map(proc_label).unwrap_or_default(),
                count: kids.iter().map(|k| k.count).sum(),
                threads_alive: Some(alive.get(&p).copied().unwrap_or(0)),
                expanded: true,
                cpu,
                state: best.map(|k| k.state).unwrap_or("unknown"),
                wait_reason: best.and_then(|k| k.wait_reason),
                wait_reason_name: best.and_then(|k| k.wait_reason_name),
                ready_for_us: kids.iter().filter_map(|k| k.ready_for_us).reduce(f64::max),
                wait_p99_us: p99p.get(&p).copied(),
                waits: kids.iter().map(|k| k.waits).sum(),
                wait_ms: kids.iter().map(|k| k.wait_ms).sum(),
                wakes_in: kids.iter().map(|k| k.wakes_in).sum(),
                wakes_out: kids.iter().map(|k| k.wakes_out).sum(),
                prio: None,
                core: None,
                running: kids.iter().map(|k| k.running).sum(),
                ready: kids.iter().map(|k| k.ready).sum(),
                r: radius("process", true, cpu),
                members: Vec::new(),
            });
            snap.nodes.extend(kids);
        } else {
            for li in ls {
                snap.nodes.push(make(li, None));
            }
        }
    }

    // 10. Links out.
    // cand is still sorted strongest first (retain keeps the order).
    snap.links = cand
        .iter()
        .map(|c| Link {
            from: c.idf,
            to: c.idt,
            kind: LinkKind::from_u8(c.k).map(|x| x.name()).unwrap_or("?"),
            rate: c.rate,
            count: c.c,
            via: (0..via::NAMES.len()).filter(|&b| b != via::SCHED as usize && c.v & (1 << b) != 0).map(|b| via::NAMES[b]).collect(),
        })
        .collect();
    snap
}

/// Thread serial -> dense index. Serials are slab slots, so an array over them replaces hashing.
struct Dense {
    arr: Vec<u32>,
    serials: Vec<u32>,
    accs: Vec<Acc>,
}

impl Dense {
    fn new(m: &ThreadMap) -> Self {
        Self { arr: vec![NONE; m.threads.slots()], serials: Vec::new(), accs: Vec::new() }
    }
    /// Index of a serial, added on first sight; NONE for a serial the map does not know.
    #[inline]
    fn idx(&mut self, s: u32) -> u32 {
        let next = self.serials.len() as u32;
        let Some(slot) = self.arr.get_mut(s as usize) else { return NONE };
        if *slot == NONE {
            *slot = next;
            self.serials.push(s);
            self.accs.push(Acc::default());
        }
        *slot
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn prefixes() {
        assert_eq!(name_prefix("Worker 12"), Some("Worker"));
        assert_eq!(name_prefix("TaskThread_3"), Some("TaskThread"));
        assert_eq!(name_prefix("pool#07"), Some("pool"));
        assert_eq!(name_prefix("RenderThread"), Some("RenderThread"));
        assert_eq!(name_prefix("1234"), None);
        assert_eq!(name_prefix(""), None);
    }
    /// The stamped fast path (per-process sums) and the slow path (resolve every record) give the same graph.
    #[test]
    fn fast_and_slow_paths_agree() {
        use crate::events::*;
        use crate::kwait::state;
        let mut m = ThreadMap::new(crate::map::Config::new(10_000_000));
        for p in 0..6u32 {
            m.process_start(&ProcessInfo { ts: 0, pid: 100 + p * 4, image: format!("p{p}.exe") });
            for t in 0..8u32 {
                m.thread_start(&ThreadInfo { ts: 0, tid: 1000 + (p * 8 + t) * 4, pid: 100 + p * 4, description: Some(format!("W {t}")), ..Default::default() });
            }
        }
        m.set_game(Some(100));
        let mut x = 1u64;
        let mut rnd = |n: u64| {
            x = x.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (x >> 33) % n
        };
        let mut cur = [0u32; 4];
        for i in 1..6000i64 {
            let ts = i * 1000;
            for (cpu, c) in cur.iter_mut().enumerate() {
                let next = 1000 + rnd(48) as u32 * 4;
                m.push_ready(&ReadyThread { ts: ts - 30, cpu: cpu as u32, readier_tid: 1000 + rnd(48) as u32 * 4, target_tid: next, ..Default::default() });
                let st = [state::READY, state::WAITING][rnd(2) as usize];
                let reason = [crate::kwait::WR_QUEUE, crate::kwait::WR_ALERT_BY_THREAD_ID, crate::kwait::WR_LPC_REPLY][rnd(3) as usize];
                m.push_cswitch(&CSwitch { ts: ts + cpu as i64, cpu: cpu as u32, new_tid: next, old_tid: *c, old_state: st, old_wait_reason: reason, ..Default::default() });
                *c = next;
            }
        }
        m.flush();
        let qs = [
            Query { min_rate: 0.0, top_n: 0, ..Default::default() },
            Query { expand: vec![100, 108], min_rate: 0.0, top_n: 0, span: 40, ..Default::default() },
            Query { expand: vec![100], focus_pid: Some(100), end_age: 7, ..Default::default() },
            Query { expand: vec![104], merge: false, span: 300, ..Default::default() },
        ];
        let strip = |s: &Snapshot| {
            let mut v = serde_json::to_value(s).unwrap();
            v.as_object_mut().unwrap().remove("stats");
            v
        };
        let fast: Vec<_> = qs.iter().map(|q| strip(&m.snapshot(q))).collect();
        assert!(fast[1]["links"].as_array().unwrap().len() > 10);
        m.proc_epoch += 1; // every bucket now looks stale: slow path
        for (q, f) in qs.iter().zip(&fast) {
            assert_eq!(&strip(&m.snapshot(q)), f, "{q:?}");
        }
    }

    #[test]
    fn query_json_defaults() {
        let q: Query = serde_json::from_str(r#"{"focus_pid": 2340, "expand": [2340]}"#).unwrap();
        assert_eq!(q.focus_pid, Some(2340));
        assert_eq!(q.span, 10);
        assert!(q.merge && q.hide_system);
    }
}
