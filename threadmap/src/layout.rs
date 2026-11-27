//! Deterministic force-directed layout (the reference page's physics): springs on links, pairwise repulsion,
//! threads pulled to their process, a weak pull to the centre, pinned and dragged nodes stay. The UI calls `step`
//! per frame and only draws; positions persist across snapshots by node id (also for nodes that disappear and come
//! back, up to `KEEP_MAX` remembered ids). No randomness: new nodes are placed from a hash of their id.

use crate::fx::FxMap;
use crate::graph::Snapshot;

/// Layout nodes cap (the snapshot's max_nodes is at most 600 too). O(n^2) repulsion: 300 nodes = 45 k pairs/step.
pub const LAYOUT_MAX: usize = 600;
const KEEP_MAX: usize = 8192;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Pos {
    pub id: u64,
    pub x: f32,
    pub y: f32,
    pub r: f32,
    /// 1 = pinned (dragged by the user).
    pub pinned: u32,
}

#[derive(Clone, Copy, Debug, Default)]
struct Body {
    x: f32,
    y: f32,
    vx: f32,
    vy: f32,
    pinned: bool,
}

#[derive(Clone, Debug)]
struct LNode {
    id: u64,
    r: f32,
    parent: Option<usize>,
    proc_key: u64,
    group: bool,
}

#[derive(Default)]
pub struct Layout {
    nodes: Vec<LNode>,
    bodies: Vec<Body>,
    index: FxMap<u64, usize>,
    edges: Vec<(usize, usize, bool)>,
    keep: FxMap<u64, Body>,
    salt: u64,
    /// Nodes of the last snapshot left out because of LAYOUT_MAX.
    pub cut: usize,
}

fn hash01(id: u64, salt: u64, k: u64) -> f32 {
    let mut h = id ^ salt.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ k.wrapping_mul(0xC2B2_AE3D_27D4_EB4F);
    h ^= h >> 33;
    h = h.wrapping_mul(0xff51_afd7_ed55_8ccd);
    h ^= h >> 33;
    h = h.wrapping_mul(0xc4ce_b9fe_1a85_ec53);
    h ^= h >> 33;
    (h >> 40) as f32 / (1u64 << 24) as f32
}

impl Layout {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.nodes.len()
    }
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// Takes the nodes and links of a new snapshot. Known ids keep position, speed and pin; new threads start near
    /// their process, other new nodes on a hash-placed spot.
    pub fn sync(&mut self, s: &Snapshot) {
        for (i, n) in self.nodes.iter().enumerate() {
            self.keep.insert(n.id, self.bodies[i]);
        }
        if self.keep.len() > KEEP_MAX {
            self.keep.clear();
            for (i, n) in self.nodes.iter().enumerate() {
                self.keep.insert(n.id, self.bodies[i]);
            }
        }
        self.nodes.clear();
        self.bodies.clear();
        self.index.clear();
        let take = s.nodes.len().min(LAYOUT_MAX);
        self.cut = s.nodes.len() - take;
        for n in &s.nodes[..take] {
            let parent = n.parent.and_then(|p| self.index.get(&p).copied());
            let body = match self.keep.get(&n.id) {
                Some(b) => *b,
                None => {
                    let (jx, jy) = (hash01(n.id, self.salt, 1) - 0.5, hash01(n.id, self.salt, 2) - 0.5);
                    match parent {
                        Some(p) => Body { x: self.bodies[p].x + jx * 60.0, y: self.bodies[p].y + jy * 60.0, ..Default::default() },
                        None => Body { x: jx * 400.0, y: jy * 260.0, ..Default::default() },
                    }
                }
            };
            self.index.insert(n.id, self.nodes.len());
            self.nodes.push(LNode { id: n.id, r: n.r, parent, proc_key: n.parent.unwrap_or(n.id), group: n.expanded });
            self.bodies.push(Body { vx: 0.0, vy: 0.0, ..body });
        }
        self.edges = s
            .links
            .iter()
            .filter_map(|l| {
                let a = *self.index.get(&l.from)?;
                let b = *self.index.get(&l.to)?;
                Some((a, b, self.nodes[a].proc_key == self.nodes[b].proc_key))
            })
            .collect();
    }

    /// Advances the simulation by `dt` seconds (the reference runs one step per 60 Hz frame; dt is turned into
    /// 1..=4 such steps so a slow frame does not explode). Returns the kinetic energy left: the UI can stop asking
    /// for frames when it is below ~0.05 per node.
    pub fn step(&mut self, dt: f32) -> f32 {
        let n = ((dt * 60.0).round() as i32).clamp(1, 4);
        let mut e = 0.0;
        for _ in 0..n {
            e = self.step1();
        }
        e
    }

    /// Runs `steps` single steps at once (first placement before the first draw; the reference uses 260).
    pub fn warm(&mut self, steps: u32) -> f32 {
        let mut e = 0.0;
        for _ in 0..steps.min(2000) {
            e = self.step1();
        }
        e
    }

    fn step1(&mut self) -> f32 {
        let n = self.nodes.len();
        let b = &mut self.bodies;
        for i in 0..n {
            for j in i + 1..n {
                let (mut dx, mut dy) = (b[j].x - b[i].x, b[j].y - b[i].y);
                if dx == 0.0 && dy == 0.0 {
                    // Same spot: split deterministically.
                    dx = (hash01(self.nodes[i].id, self.nodes[j].id, 3) - 0.5) * 0.1;
                    dy = (hash01(self.nodes[j].id, self.nodes[i].id, 4) - 0.5) * 0.1;
                }
                let d2 = dx * dx + dy * dy + 0.01;
                let d = d2.sqrt();
                let min = self.nodes[i].r + self.nodes[j].r + 26.0;
                let f = if d < min { 1.2 * (min - d) / d } else { 0.0 } + 2200.0 / d2;
                let (fx, fy) = (dx * f / d * 0.5, dy * f / d * 0.5);
                b[i].vx -= fx;
                b[i].vy -= fy;
                b[j].vx += fx;
                b[j].vy += fy;
            }
        }
        for &(i, j, same) in &self.edges {
            let (dx, dy) = (b[j].x - b[i].x, b[j].y - b[i].y);
            let d = (dx * dx + dy * dy).sqrt() + 0.01;
            let want = if same { 70.0 } else { 170.0 };
            let f = (d - want) * 0.012;
            b[i].vx += dx / d * f;
            b[i].vy += dy / d * f;
            b[j].vx -= dx / d * f;
            b[j].vy -= dy / d * f;
        }
        // Threads held near their process. The reference pulls only the thread; here the process gets the opposite
        // pull too, so the forces cancel in sum and the graph does not keep rotating (it then settles).
        for i in 0..n {
            if let Some(p) = self.nodes[i].parent {
                let (fx, fy) = ((b[p].x - b[i].x) * 0.012, (b[p].y - b[i].y) * 0.012);
                b[i].vx += fx;
                b[i].vy += fy;
                b[p].vx -= fx;
                b[p].vy -= fy;
            }
        }
        let mut energy = 0.0;
        for i in 0..n {
            b[i].vx += -b[i].x * 0.0015;
            b[i].vy += -b[i].y * 0.0015;
            if b[i].pinned {
                b[i].vx = 0.0;
                b[i].vy = 0.0;
                continue;
            }
            b[i].vx *= 0.82;
            b[i].vy *= 0.82;
            b[i].x += b[i].vx;
            b[i].y += b[i].vy;
            energy += b[i].vx * b[i].vx + b[i].vy * b[i].vy;
        }
        energy
    }

    /// Moves a node to (x, y) and pins it (mouse drag). false: unknown id.
    pub fn drag(&mut self, id: u64, x: f32, y: f32) -> bool {
        let Some(&i) = self.index.get(&id) else { return false };
        self.bodies[i].x = x;
        self.bodies[i].y = y;
        self.bodies[i].vx = 0.0;
        self.bodies[i].vy = 0.0;
        self.bodies[i].pinned = true;
        true
    }

    pub fn set_pinned(&mut self, id: u64, pinned: bool) -> bool {
        let Some(&i) = self.index.get(&id) else { return false };
        self.bodies[i].pinned = pinned;
        true
    }

    /// "Lay out again": unpins everything and scatters the nodes to new hash-placed spots.
    pub fn relayout(&mut self) {
        self.salt = self.salt.wrapping_add(1);
        self.keep.clear();
        for i in 0..self.nodes.len() {
            let id = self.nodes[i].id;
            self.bodies[i] = Body { x: (hash01(id, self.salt, 1) - 0.5) * 400.0, y: (hash01(id, self.salt, 2) - 0.5) * 260.0, ..Default::default() };
        }
    }

    pub fn positions(&self) -> Vec<Pos> {
        self.nodes
            .iter()
            .zip(&self.bodies)
            .map(|(n, b)| Pos { id: n.id, x: b.x, y: b.y, r: n.r, pinned: b.pinned as u32 })
            .collect()
    }

    pub fn get(&self, id: u64) -> Option<Pos> {
        let &i = self.index.get(&id)?;
        let (n, b) = (&self.nodes[i], &self.bodies[i]);
        Some(Pos { id, x: b.x, y: b.y, r: n.r, pinned: b.pinned as u32 })
    }

    /// Group (expanded process) nodes are not drawn as circles but take part in the physics.
    pub fn is_group(&self, id: u64) -> bool {
        self.index.get(&id).is_some_and(|&i| self.nodes[i].group)
    }
}
