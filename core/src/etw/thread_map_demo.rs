//! Synthetic feed for the Thread map page where no kernel trace runs (Wine, screenshots): only with DOS_TM_DEMO=1 or
//! in the DOS_UI_SHOT build, never in a normal run. It plays a game at 250 fps (main thread, render thread, 12 workers,
//! audio, net, job queue) with a few programs around it, through the same `Feeder` and `tm_feed_*` calls as the
//! trace, with consistent per-cpu context switches, so every link kind and node state of the page shows up:
//! wakes (main -> render, main -> workers, render -> main, DPC -> render), lock waits (main waits for the workers'
//! lock), preemption (chrome holds the cores the job queue and, every 12 s for 1.2 s, the main thread need: the wait
//! spike of the time strip), IPC (render -> dwm, audio -> audiodg, main -> steam, Discord -> audiodg).
//! Deterministic (a fixed LCG), so tests and screenshots repeat.

use super::thread_map::{Feeder, Sink};
use threadmap::events::{via, CSwitch, LinkEvent, LinkKind, ReadyThread, READY_FLAG_DPC};
use threadmap::kwait::{self, state};

pub const GAME_PID: u32 = 2340;
const CPUS: usize = 12;
const FRAME_NS: i64 = 4_000_000; // 250 fps

const MAIN: u32 = 2400;
const RENDER: u32 = 2404;
const WORKER0: u32 = 2500; // 12 workers, tids 2500..=2544
const WORKERS: u32 = 12;
const AUDIO: u32 = 2600;
const NET: u32 = 2604;
const JOBS: u32 = 2608;
const DWM: u32 = 1104;
const AUDIODG: u32 = 1504;
const STEAM: u32 = 4104;
const DISCORD: u32 = 5204;
const CHROME: [u32; 2] = [6104, 6108];
const NVC: u32 = 3304;
const CSRSS: u32 = 704;

/// readier "the kernel, from a DPC" (GPU interrupt, timer).
const DPC: u32 = u32::MAX - 1;

pub struct Demo {
    freq: i64,
    base: i64,
    next_frame: i64,
    frame: u64,
    rng: u64,
    free: [i64; CPUS],
    started: bool,
}

impl Demo {
    /// `base` = the QPC tick of simulated time 0.
    pub fn new(freq: i64, base: i64) -> Self {
        Self { freq: freq.max(1), base, next_frame: 0, frame: 0, rng: 0x2545_F491_4F6C_DD1D, free: [0; CPUS], started: false }
    }

    fn tk(&self, ns: i64) -> i64 {
        self.base + ((ns as i128 * self.freq as i128) / 1_000_000_000) as i64
    }

    fn ns_of(&self, ticks: i64) -> i64 {
        (((ticks - self.base) as i128 * 1_000_000_000) / self.freq as i128) as i64
    }

    fn rnd(&mut self) -> f64 {
        self.rng = self.rng.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        (self.rng >> 11) as f64 / (1u64 << 53) as f64
    }

    /// `ns` with +-`spread` (share) of jitter.
    fn jit(&mut self, ns: i64, spread: f64) -> i64 {
        (ns as f64 * (1.0 + spread * (self.rnd() * 2.0 - 1.0))).max(1000.0) as i64
    }

    fn intro(&mut self, f: &mut Feeder, sink: &mut dyn Sink) {
        let t = self.tk(0);
        f.flush(sink);
        for (pid, img) in [
            (GAME_PID, "cs2.exe"),
            (1100, "dwm.exe"),
            (1500, "audiodg.exe"),
            (4100, "steam.exe"),
            (5200, "Discord.exe"),
            (6100, "chrome.exe"),
            (3300, "NVDisplay.Container.exe"),
            (700, "csrss.exe"),
        ] {
            sink.process_start(t, pid, img);
        }
        let mut th = vec![(MAIN, GAME_PID, "MainThread"), (RENDER, GAME_PID, "RenderThread"), (AUDIO, GAME_PID, "Audio"), (NET, GAME_PID, "Net"), (JOBS, GAME_PID, "Job Queue")];
        th.extend([(DWM, 1100, "DWM Master"), (AUDIODG, 1500, "Audio Engine"), (STEAM, 4100, "Overlay"), (DISCORD, 5200, "Voice"), (NVC, 3300, "Telemetry"), (CSRSS, 700, "CsrApiRequest")]);
        th.extend([(CHROME[0], 6100, "VideoDecode"), (CHROME[1], 6100, "Compositor")]);
        for (tid, pid, name) in th {
            sink.thread_start(t, tid, pid, Some(name), None, 0);
        }
        for w in 0..WORKERS {
            let name = format!("Worker {w}");
            sink.thread_start(t, WORKER0 + w * 4, GAME_PID, Some(&name), None, 0);
        }
    }

    /// Thread `tid` made ready at `ready` by `readier` runs `dur` on the least busy cpu of `cpus`, then waits with
    /// `reason`. Returns the end time.
    #[allow(clippy::too_many_arguments)]
    fn run(&mut self, f: &mut Feeder, sink: &mut dyn Sink, tid: u32, prio: i8, ready: i64, readier: u32, dur: i64, reason: u8, cpus: &[usize]) -> i64 {
        let cpu = *cpus.iter().min_by_key(|&&c| (self.free[c].max(ready), c)).unwrap_or(&0);
        let (readier_tid, flags) = if readier == DPC { (0, READY_FLAG_DPC) } else { (readier, 0) };
        f.push_ready(ReadyThread { ts: self.tk(ready), cpu: cpu as u32, readier_tid, target_tid: tid, flags, ..Default::default() }, sink);
        let start = self.free[cpu].max(ready + 3_000);
        f.push_cswitch(CSwitch { ts: self.tk(start), cpu: cpu as u32, new_tid: tid, old_tid: 0, new_priority: prio, old_state: state::RUNNING, ..Default::default() }, sink);
        let end = start + dur;
        f.push_cswitch(
            CSwitch { ts: self.tk(end), cpu: cpu as u32, new_tid: 0, old_tid: tid, old_priority: prio, old_wait_reason: reason, old_wait_mode: 1, old_state: state::WAITING, ..Default::default() },
            sink,
        );
        self.free[cpu] = end;
        end
    }

    fn ipc(&mut self, f: &mut Feeder, sink: &mut dyn Sink, at: i64, from: u32, to: u32) {
        f.push_link(LinkEvent { ts: self.tk(at), from_tid: from, to_tid: to, kind: LinkKind::Ipc as u8, via: via::ALPC, reserved: [0; 2], count: 1 }, sink);
    }

    fn frame(&mut self, f: &mut Feeder, sink: &mut dyn Sink, t0: i64) {
        let n = self.frame;
        // cpus: 2-3 main, 4-5 render, 0-1 and 6-9 workers, 10-11 chrome; small helpers wherever a core is free.
        let all: [usize; 6] = [0, 1, 6, 7, 8, 9];
        // Every 12 s for 1.2 s: chrome also takes the main thread's cores (the wait spike of the strip).
        let spike = (t0 / 1_000_000) % 12_000 >= 9_000 && (t0 / 1_000_000) % 12_000 < 10_200;
        // Background: chrome decodes video on cpus 10 and 11 (and 2, 3 in the spike).
        let d0 = self.jit(2_600_000, 0.2);
        self.run(f, sink, CHROME[0], 8, t0, DPC, d0, kwait::WR_USER_REQUEST, &[10]);
        let d1 = self.jit(1_800_000, 0.3);
        self.run(f, sink, CHROME[1], 8, t0 + 200_000, CHROME[0], d1, kwait::WR_QUEUE, &[11]);
        if spike {
            let d = self.jit(2_000_000, 0.2);
            self.run(f, sink, CHROME[0], 8, t0, DPC, d, kwait::WR_USER_REQUEST, &[2]);
            let d = self.jit(1_600_000, 0.2);
            self.run(f, sink, CHROME[1], 8, t0, CHROME[0], d, kwait::WR_QUEUE, &[3]);
        }
        // Discord's voice thread every 20 ms on cpu 1 (a worker readied there waits for it).
        if n % 5 == 0 {
            let d = self.jit(1_200_000, 0.2);
            self.run(f, sink, DISCORD, 10, t0 + 100_000, DPC, d, kwait::WR_USER_REQUEST, &[1]);
            self.ipc(f, sink, t0 + 400_000, DISCORD, AUDIODG);
        }
        // The frame: the render thread's "present done" readies the main thread.
        let md = self.jit(1_500_000, 0.15);
        let main_end = self.run(f, sink, MAIN, 12, t0, RENDER, md, kwait::WR_ALERT_BY_THREAD_ID, &[2, 3]);
        // Main hands work to 6 of the 12 workers (rotating); they release the lock main waits on.
        let mut last = (main_end, WORKER0);
        for k in 0..6u32 {
            let w = WORKER0 + ((n as u32 * 6 + k) % WORKERS) * 4;
            let cpu = [all[(k as usize) % 6]];
            let d = self.jit(700_000, 0.3);
            let e = self.run(f, sink, w, 10, t0 + 300_000 + k as i64 * 40_000, MAIN, d, kwait::WR_QUEUE, &cpu);
            if e > last.0 {
                last = (e, w);
            }
        }
        let d = self.jit(400_000, 0.2);
        let main2 = self.run(f, sink, MAIN, 12, last.0 + 5_000, last.1, d, kwait::WR_USER_REQUEST, &[2, 3]);
        // Render: woken by main, submits to the GPU (dwm), waits; the GPU's DPC wakes it for the present.
        let rd = self.jit(1_100_000, 0.15);
        let r_end = self.run(f, sink, RENDER, 12, main2 - 200_000, MAIN, rd, kwait::WR_USER_REQUEST, &[4, 5]);
        self.ipc(f, sink, r_end - 100_000, RENDER, DWM);
        let d = self.jit(150_000, 0.3);
        self.run(f, sink, DWM, 14, r_end, RENDER, d, kwait::WR_USER_REQUEST, &all);
        let d = self.jit(120_000, 0.3);
        self.run(f, sink, RENDER, 12, r_end + 600_000, DPC, d, kwait::WR_USER_REQUEST, &[4, 5]);
        // The job queue every 2nd frame on cpus 10/11, where chrome runs: it waits for a core.
        if n % 2 == 0 {
            let d = self.jit(500_000, 0.3);
            self.run(f, sink, JOBS, 10, t0 + 900_000, MAIN, d, kwait::WR_QUEUE, &[10, 11]);
        }
        // Audio every 10 ms: audiodg wakes the game's audio thread, which answers over ALPC.
        if n % 5 == 0 || n % 5 == 2 {
            let d = self.jit(100_000, 0.2);
            let ae = self.run(f, sink, AUDIODG, 15, t0 + 50_000, DPC, d, kwait::WR_USER_REQUEST, &all);
            let d = self.jit(250_000, 0.2);
            let e = self.run(f, sink, AUDIO, 15, ae, AUDIODG, d, kwait::WR_USER_REQUEST, &all);
            self.ipc(f, sink, e - 50_000, AUDIO, AUDIODG);
        }
        // Net about 64 times a second, then it wakes the main thread's queue.
        if n % 4 == 1 {
            let d = self.jit(60_000, 0.3);
            self.run(f, sink, NET, 10, t0 + 2_000_000, DPC, d, kwait::WR_QUEUE, &all);
        }
        // Steam overlay ~6 times a second (main asks it over ALPC and is answered).
        if n % 40 == 3 {
            self.ipc(f, sink, t0 + 1_000_000, MAIN, STEAM);
            let d = self.jit(300_000, 0.3);
            self.run(f, sink, STEAM, 9, t0 + 1_000_000, MAIN, d, kwait::WR_LPC_RECEIVE, &all);
        }
        if n % 125 == 7 {
            let d = self.jit(200_000, 0.3);
            self.run(f, sink, NVC, 8, t0 + 1_500_000, DPC, d, kwait::WR_USER_REQUEST, &all);
        }
        if n % 250 == 11 {
            self.ipc(f, sink, t0 + 1_200_000, MAIN, CSRSS);
            let d = self.jit(80_000, 0.3);
            self.run(f, sink, CSRSS, 13, t0 + 1_200_000, MAIN, d, kwait::WR_LPC_RECEIVE, &all);
        }
    }

    /// Generates every frame that starts before `until` (QPC ticks).
    pub fn run_until(&mut self, until: i64, f: &mut Feeder, sink: &mut dyn Sink) {
        if !self.started {
            self.started = true;
            self.intro(f, sink);
        }
        let end = self.ns_of(until);
        while self.next_frame < end {
            let t0 = self.next_frame;
            self.frame(f, sink, t0);
            self.frame += 1;
            self.next_frame += FRAME_NS;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::thread_map::{TmSink, BATCH};
    use super::*;
    use std::ffi::{CStr, CString};
    use threadmap::ffi as tm;

    fn json(p: *mut std::ffi::c_char) -> serde_json::Value {
        assert!(!p.is_null());
        let s = unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned();
        tm::tm_free_str(p);
        serde_json::from_str(&s).unwrap()
    }

    /// The demo through the real map: the game and its links of every kind, the wait spike in the strip.
    #[test]
    fn demo_feed_shows_every_link_kind_and_the_wait_spike() {
        let freq = 10_000_000i64;
        let h = tm::tm_new(freq as u64);
        tm::tm_set_game(h, GAME_PID);
        let mut d = Demo::new(freq, 1_000_000);
        let mut f = Feeder::new(BATCH, freq / 20);
        let mut sink = TmSink(h);
        let caps = f.capacity();
        d.run_until(1_000_000 + 30 * freq, &mut f, &mut sink);
        f.flush(&mut sink);
        assert_eq!(f.capacity(), caps);
        tm::tm_flush(h);
        let q = CString::new(r#"{"focus_pid":2340,"expand":[2340],"span":50,"hide_system":false}"#).unwrap();
        let v = json(tm::tm_snapshot_json(h, q.as_ptr()));
        let nodes = v["nodes"].as_array().unwrap();
        let label = |id: &serde_json::Value| nodes.iter().find(|n| n["id"] == *id).map(|n| n["label"].as_str().unwrap_or("").to_string()).unwrap_or_default();
        for l in ["cs2.exe", "MainThread", "RenderThread", "chrome.exe", "dwm.exe"] {
            assert!(nodes.iter().any(|n| n["label"] == l), "no {l}: {:?}", nodes.iter().map(|n| n["label"].clone()).collect::<Vec<_>>());
        }
        assert!(nodes.iter().any(|n| n["kind"] == "group" && n["label"].as_str().unwrap().starts_with("Worker")));
        let links = v["links"].as_array().unwrap();
        let has = |from: &str, to: &str, kind: &str| links.iter().any(|l| label(&l["from"]).starts_with(from) && label(&l["to"]).starts_with(to) && l["kind"] == kind);
        assert!(has("MainThread", "RenderThread", "wake"));
        assert!(has("MainThread", "Worker", "lock"), "{links:?}");
        assert!(has("chrome.exe", "Job Queue", "preempt"));
        assert!(has("RenderThread", "dwm.exe", "ipc"));
        let main = nodes.iter().find(|n| n["label"] == "MainThread").unwrap();
        assert!(main["cpu"].as_f64().unwrap() > 0.3);
        let s = json(tm::tm_series_json(h));
        let wait: Vec<f64> = s["wait_ms"].as_array().unwrap().iter().map(|x| x.as_f64().unwrap()).collect();
        let load: Vec<f64> = s["load"].as_array().unwrap().iter().map(|x| x.as_f64().unwrap()).collect();
        assert!(load.iter().filter(|&&l| l > 1.0).count() > 200, "game load in cores");
        let max = wait.iter().cloned().fold(0.0, f64::max);
        let med = {
            let mut w = wait.clone();
            w.sort_by(|a, b| a.partial_cmp(b).unwrap());
            w[150]
        };
        assert!(max > 3.0 * med.max(0.01), "a wait spike: max {max} median {med}");
        tm::tm_free(h);
    }

    #[test]
    fn demo_is_deterministic_and_frames_only_up_to_the_time_asked() {
        #[derive(Default)]
        struct Count(usize, i64);
        impl Sink for Count {
            fn cswitch(&mut self, ev: &[CSwitch]) {
                self.0 += ev.len();
                self.1 = self.1.max(ev.iter().map(|e| e.ts).max().unwrap_or(0));
            }
            fn ready(&mut self, ev: &[ReadyThread]) {
                self.0 += ev.len();
            }
            fn link(&mut self, ev: &[LinkEvent]) {
                self.0 += ev.len();
            }
            fn process_start(&mut self, _: i64, _: u32, _: &str) {}
            fn process_end(&mut self, _: i64, _: u32) {}
            fn thread_start(&mut self, _: i64, _: u32, _: u32, _: Option<&str>, _: Option<&str>, _: u64) {}
            fn thread_end(&mut self, _: i64, _: u32) {}
            fn thread_name(&mut self, _: i64, _: u32, _: &str) {}
        }
        let mut a = (Demo::new(1_000_000_000, 0), Feeder::new(64, 50_000_000), Count::default());
        let mut b = (Demo::new(1_000_000_000, 0), Feeder::new(64, 50_000_000), Count::default());
        a.0.run_until(100_000_000, &mut a.1, &mut a.2);
        b.0.run_until(50_000_000, &mut b.1, &mut b.2);
        b.0.run_until(100_000_000, &mut b.1, &mut b.2);
        a.1.flush(&mut a.2);
        b.1.flush(&mut b.2);
        assert_eq!(a.2 .0, b.2 .0);
        assert_eq!(a.0.frame, 25); // 100 ms at 250 fps
        assert!(a.2 .1 < 100_000_000 + FRAME_NS * 3);
    }
}
