//! Ground the water meets. Heights are in blocks relative to the resting
//! water level, so 0 is the waterline.
//!
//! The ground is either one of the generated demonstration scenes below or
//! a build brought in from a file, which stands in the sea as it is.
//! Rivers, streams and waterfalls are not part of a scene: they are laid
//! over whatever ground there is by `Terrain::add_stream`.

use crate::noise::{fbm, hash, lerp, smoothstep, value};
use crate::survey::{read_shore, ShoreKind};
use of_formats::model::{BlockState, Build};
use serde::{Deserialize, Serialize};
use std::f64::consts::TAU;
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ground {
    Sand,
    Grass,
    Rock,
    /// Running fresh water on the land.
    Stream,
    /// Water falling down a rock face.
    Fall,
    /// Still water filling a hollow of the land.
    Pond,
}

impl Ground {
    pub const ALL: [Ground; 6] = [Ground::Sand, Ground::Grass, Ground::Rock, Ground::Stream, Ground::Fall, Ground::Pond];
}


#[derive(Debug, Clone)]
pub struct TerrainBlocks {
    pub sand: String,
    pub grass: String,
    pub rock: String,
    pub stream: String,
    pub fall: String,
    pub pond: String,
}

impl Default for TerrainBlocks {
    fn default() -> Self {
        Self {
            sand: "yellow_concrete".into(),
            grass: "lime_concrete".into(),
            rock: "light_gray_concrete".into(),
            stream: "light_blue_concrete".into(),
            fall: "white_concrete".into(),
            pond: "blue_concrete".into(),
        }
    }
}

pub struct Terrain {
    pub width: i32,
    pub length: i32,
    /// Lowest level that is built. Ground deeper than this is left out.
    pub floor: i32,
    pub blocks: TerrainBlocks,
    height: Vec<f32>,
    kind: Vec<Ground>,
    /// Rock spanning over a column, as the levels of its lowest and highest
    /// block: an arch. Empty when the ground has none.
    roof: Vec<Option<(i32, i32)>>,
    /// Where running water enters the sea.
    mouths: Vec<Mouth>,
    /// Under a pond, the level of the ground its water stands on; empty
    /// when the land has no pond.
    bed: Vec<i32>,
    /// How each pond laid over the land came out.
    ponds: Vec<PondEnd>,
    /// The build this ground was read from, if it was. Its blocks are what
    /// is built; the heights above only tell the water where it can be.
    pub placed: Option<Placed>,
}

/// A build brought in from a file to stand in the sea.
pub struct Placed {
    pub build: Arc<Build>,
    /// Added to the place of a block in the build to give its column of the
    /// plot and its level relative to the resting water.
    pub shift: [i32; 3],
    /// Size of the build in blocks: width, height, length.
    pub size: [i32; 3],
    /// How many blocks of its height stand below the resting water.
    pub under: i32,
    /// Blocks of the build that fell outside the plot and are left out.
    pub cut: u64,
    /// Columns of the plot that rivers and ponds laid over the build changed,
    /// in order. Only these are built from the ground's heights; the rest
    /// copy the build as it is.
    pub reworked: Vec<u32>,
}

/// A place where running water enters the sea.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Mouth {
    /// Where the water leaves the land, in blocks from the north-west corner.
    pub at: (f64, f64),
    /// The direction it flows out, as a unit vector.
    pub toward: (f64, f64),
    /// Width of the outflow in blocks.
    pub width: f64,
    /// The water fell into the sea here rather than running into it.
    pub fell: bool,
}

impl Terrain {
    /// Ground given column by column, row after row from the north-west
    /// corner: its height relative to the resting water level and what it
    /// is made of.
    pub fn from_heights(width: i32, length: i32, floor: i32, height: Vec<f32>, kind: Vec<Ground>) -> Self {
        let cells = (width.max(0) * length.max(0)) as usize;
        assert!(height.len() == cells && kind.len() == cells, "the ground does not match the plot");
        Self { width, length, floor, blocks: TerrainBlocks::default(), height, kind, roof: Vec::new(), mouths: Vec::new(), bed: Vec::new(), ponds: Vec::new(), placed: None }
    }

    /// A build standing in the sea. `waterline` is how far up the build the
    /// water rests, as a share of its height. The build is centred on the
    /// plot and then moved by `offset` blocks east and south.
    ///
    /// The water is told three things about every column: where the build
    /// stands in it from below the waterline (ground), where it hangs over
    /// it (a roof the water stays clear of), and whether the column lies
    /// inside the build. A hollow build is closed to the sea: columns the
    /// open sea cannot reach at the waterline count as land.
    pub fn from_build(width: i32, length: i32, build: &Arc<Build>, waterline: f64, offset: [i32; 2]) -> Result<Self, String> {
        let air: Vec<bool> = build.palette.states().iter().map(BlockState::is_air).collect();
        let filled = |v: u32| v != 0 && !air[(v - 1) as usize];
        // The build's cubes, gathered by cube column and sorted from the
        // lowest up. Cubes with no block in them are left out (checked once
        // per stored copy, since all-air cubes share one).
        // Why columns: walking each column of cells from the bottom up gives
        // its runs of blocks directly, without a list of every block (that
        // list took 20 bytes per block).
        let mut empty: std::collections::HashMap<*const u32, bool> = std::collections::HashMap::new();
        let mut columns: std::collections::HashMap<(i32, i32), Vec<(i32, usize, &[u32; 4096])>> = std::collections::HashMap::new();
        for (r, region) in build.regions.iter().enumerate() {
            for (corner, chunk) in region.cells.chunks() {
                if *empty.entry(chunk.as_ptr()).or_insert_with(|| !chunk.iter().any(|v| filled(*v))) {
                    continue;
                }
                columns.entry((corner[0], corner[2])).or_default().push((corner[1], r, chunk));
            }
        }
        drop(empty);
        for cubes in columns.values_mut() {
            cubes.sort_unstable_by_key(|c| c.0);
        }
        // Whether a cell holds a block, in any region that covers it.
        let block_at = |cubes: &[(i32, usize, &[u32; 4096])], k: usize, p: [i32; 3]| {
            let i = (((p[1] & 15) as usize) << 8) | (((p[2] & 15) as usize) << 4) | (p[0] & 15) as usize;
            cubes[k..].iter().take_while(|c| c.0 == p[1] >> 4).any(|c| filled(c.2[i]) && build.regions[c.1].contains(p))
        };
        // First pass: the box around every block.
        let mut bounds: Option<([i32; 3], [i32; 3])> = None;
        for (&(cx, cz), cubes) in &columns {
            let mut k = 0;
            while k < cubes.len() {
                let cy = cubes[k].0;
                for i in 0..4096usize {
                    let p = [(cx << 4) + (i & 15) as i32, (cy << 4) + (i >> 8) as i32, (cz << 4) + ((i >> 4) & 15) as i32];
                    if block_at(cubes, k, p) {
                        let (lo, hi) = bounds.get_or_insert((p, p));
                        for a in 0..3 {
                            lo[a] = lo[a].min(p[a]);
                            hi[a] = hi[a].max(p[a]);
                        }
                    }
                }
                while k < cubes.len() && cubes[k].0 == cy {
                    k += 1;
                }
            }
        }
        let Some((lo, hi)) = bounds else {
            return Err("the build file holds no blocks".into());
        };
        let size = [hi[0] - lo[0] + 1, hi[1] - lo[1] + 1, hi[2] - lo[2] + 1];
        let under = (waterline.clamp(0.0, 1.0) * size[1] as f64).round() as i32;
        let shift = [(width - size[0]).div_euclid(2) - lo[0] + offset[0], -(lo[1] + under), (length - size[2]).div_euclid(2) - lo[2] + offset[1]];
        let cells = (width.max(0) * length.max(0)) as usize;
        let floor = -under;
        // Where the build is not, the sea is deep.
        let open = floor as f32 - 1000.0;
        let mut height = vec![open; cells];
        let mut roof: Vec<Option<(i32, i32)>> = vec![None; cells];
        let mut highest = vec![i32::MIN; cells];
        let mut blocked = vec![false; cells];
        let mut any_roof = false;
        let mut cut = 0u64;
        // Second pass: each column of cells from the lowest up, as runs of
        // blocks standing on one another.
        for (&(cx, cz), cubes) in &columns {
            for dz in 0..16 {
                for dx in 0..16 {
                    let (bx, bz) = ((cx << 4) + dx, (cz << 4) + dz);
                    let (x, z) = (bx + shift[0], bz + shift[2]);
                    let outside = x < 0 || z < 0 || x >= width || z >= length;
                    let column = if outside { 0 } else { (z * width + x) as usize };
                    let mut run: Option<(i32, i32)> = None;
                    let mut end_run = |run: (i32, i32)| {
                        let (from, to) = run;
                        if from <= 0 {
                            height[column] = to as f32;
                        } else {
                            let low = roof[column].map_or(from, |r| r.0);
                            roof[column] = Some((low, to));
                            any_roof = true;
                        }
                        blocked[column] |= from <= 1 && to >= -1;
                        highest[column] = to;
                    };
                    let mut k = 0;
                    while k < cubes.len() {
                        let cy = cubes[k].0;
                        for dy in 0..16 {
                            let by = (cy << 4) + dy;
                            if !block_at(cubes, k, [bx, by, bz]) {
                                continue;
                            }
                            if outside {
                                cut += 1;
                                continue;
                            }
                            let level = by + shift[1];
                            run = match run {
                                Some((from, to)) if to + 1 == level => Some((from, level)),
                                Some(done) => {
                                    end_run(done);
                                    Some((level, level))
                                }
                                None => Some((level, level)),
                            };
                        }
                        while k < cubes.len() && cubes[k].0 == cy {
                            k += 1;
                        }
                    }
                    if let Some(done) = run {
                        end_run(done);
                    }
                }
            }
        }
        drop(columns);
        // The sea reaches every column it can get to from the edge of the
        // plot without passing through the build at the waterline.
        let mut sea = vec![false; cells];
        let mut queue: Vec<usize> = Vec::new();
        let (w, l) = (width as usize, length as usize);
        for z in 0..l {
            for x in 0..w {
                if (x == 0 || z == 0 || x == w - 1 || z == l - 1) && !blocked[z * w + x] {
                    sea[z * w + x] = true;
                    queue.push(z * w + x);
                }
            }
        }
        while let Some(i) = queue.pop() {
            let (x, z) = (i % w, i / w);
            let mut visit = |j: usize| {
                if !blocked[j] && !sea[j] {
                    sea[j] = true;
                    queue.push(j);
                }
            };
            if x > 0 {
                visit(i - 1);
            }
            if x + 1 < w {
                visit(i + 1);
            }
            if z > 0 {
                visit(i - w);
            }
            if z + 1 < l {
                visit(i + w);
            }
        }
        for i in 0..cells {
            if !blocked[i] && !sea[i] {
                height[i] = highest[i].max(1) as f32;
                roof[i] = None;
            }
        }
        Ok(Self {
            width,
            length,
            floor,
            blocks: TerrainBlocks::default(),
            height,
            kind: vec![Ground::Rock; cells],
            roof: if any_roof { roof } else { Vec::new() },
            mouths: Vec::new(),
            bed: Vec::new(),
            ponds: Vec::new(),
            placed: Some(Placed { build: build.clone(), shift, size, under, cut, reworked: Vec::new() }),
        })
    }

    fn index(&self, x: i32, z: i32) -> usize {
        (z.clamp(0, self.length - 1) * self.width + x.clamp(0, self.width - 1)) as usize
    }
    /// Ground height at a column. Outside the plot the edge value repeats.
    pub fn height(&self, x: i32, z: i32) -> f32 {
        self.height[self.index(x, z)]
    }
    /// Level of the highest ground block, or none where the ground is below the floor.
    pub fn top(&self, x: i32, z: i32) -> Option<i32> {
        let top = self.height(x, z).round() as i32;
        (top >= self.floor).then_some(top)
    }
    pub fn kind(&self, x: i32, z: i32) -> Ground {
        self.kind[self.index(x, z)]
    }
    /// Rock spanning over a column of the plot: the levels of its lowest
    /// and highest block.
    pub fn roof(&self, x: i32, z: i32) -> Option<(i32, i32)> {
        if self.roof.is_empty() || x < 0 || z < 0 || x >= self.width || z >= self.length {
            return None;
        }
        self.roof[self.index(x, z)]
    }
    /// Where running water enters the sea: one place for a plain river
    /// mouth or a fall, several where a river has split over its delta.
    pub fn mouths(&self) -> &[Mouth] {
        &self.mouths
    }
    /// How each pond laid over the land came out, in the order they were laid.
    pub fn ponds(&self) -> &[PondEnd] {
        &self.ponds
    }
    /// The ground's heights and kinds, to tell afterwards which columns
    /// rivers and ponds changed (see `note_reworked`).
    pub(crate) fn shape(&self) -> (Vec<f32>, Vec<Ground>) {
        (self.height.clone(), self.kind.clone())
    }
    /// Note, for a build brought in, the columns that changed since `before`.
    pub(crate) fn note_reworked(&mut self, before: &(Vec<f32>, Vec<Ground>)) {
        let changed = (0..self.height.len()).filter(|&i| self.height[i] != before.0[i] || self.kind[i] != before.1[i]).map(|i| i as u32).collect();
        if let Some(placed) = &mut self.placed {
            placed.reworked = changed;
        }
    }
    /// What a ground block at `level` is made of: sand is a few blocks deep,
    /// grass is one block over rock, so every exposed drop shows a rock face.
    pub fn block_at(&self, x: i32, z: i32, level: i32) -> Ground {
        let top = self.height(x, z).round() as i32;
        match self.kind(x, z) {
            Ground::Sand if level > top - 3 => Ground::Sand,
            Ground::Grass if level == top => Ground::Grass,
            Ground::Stream if level == top => Ground::Stream,
            Ground::Pond if self.bed.get(self.index(x, z)).is_some_and(|bed| level > *bed) => Ground::Pond,
            // Falling water is white, streaked with clearer water.
            Ground::Fall if level >= 0 => {
                if value(x as f64 / 1.3 + z as f64 / 2.1, level as f64 / 5.0, 0xFA11) > 0.3 { Ground::Stream } else { Ground::Fall }
            }
            _ => Ground::Rock,
        }
    }
    pub fn name_of(&self, ground: Ground) -> &str {
        match ground {
            Ground::Sand => &self.blocks.sand,
            Ground::Grass => &self.blocks.grass,
            Ground::Rock => &self.blocks.rock,
            Ground::Stream => &self.blocks.stream,
            Ground::Fall => &self.blocks.fall,
            Ground::Pond => &self.blocks.pond,
        }
    }
}

/// A river or stream laid over the ground. It runs downhill along its
/// course, cutting a channel; where the ground drops away it falls, and it
/// ends where it meets the sea.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Stream {
    /// Where it rises and where it is headed, as shares of the plot's
    /// width (west to east) and length (north to south), each 0 to 1.
    pub from: [f64; 2],
    pub to: [f64; 2],
    /// Width in blocks.
    pub width: f64,
    /// 0 runs straight; 1 winds widely.
    pub winding: f64,
    /// How much sand it has laid down where it runs into the sea over low
    /// ground: 0 is a plain mouth, 1 a broad delta of bars and split
    /// channels. Where it falls into the sea there is never a delta.
    pub delta: f64,
}

impl Default for Stream {
    fn default() -> Self {
        Self { from: [0.5, 0.05], to: [0.5, 0.5], width: 5.0, winding: 0.5, delta: 0.5 }
    }
}

impl Stream {
    /// The line the water follows, as points half a block apart.
    pub fn course(&self, width: i32, length: i32, seed: u32) -> Vec<(f64, f64)> {
        let (w, l) = (width as f64, length as f64);
        let a = (self.from[0].clamp(0.0, 1.0) * w, self.from[1].clamp(0.0, 1.0) * l);
        let b = (self.to[0].clamp(0.0, 1.0) * w, self.to[1].clamp(0.0, 1.0) * l);
        let run = (b.0 - a.0).hypot(b.1 - a.1);
        if run < 2.0 {
            return Vec::new();
        }
        let (dx, dz) = ((b.0 - a.0) / run, (b.1 - a.1) / run);
        let steps = (run * 2.0).ceil() as usize;
        let swing = self.winding.clamp(0.0, 1.0) * (run * 0.14).min(16.0);
        (0..=steps)
            .map(|k| {
                let t = k as f64 / steps as f64;
                // It winds between its ends, which stay where they were put.
                let held = smoothstep(0.0, 0.2, t) * smoothstep(0.0, 0.2, 1.0 - t);
                let side = swing * held * fbm(t * run / 34.0, 0.37, seed ^ 0x51E, 2) * 1.6;
                (a.0 + dx * run * t - dz * side, a.1 + dz * run * t + dx * side)
            })
            .collect()
    }
}

/// A pond: still water filling a hollow of the land. The water finds the
/// lowest point of the hollow it is put in and rises from there as deep as
/// asked, unless the hollow is shallower: then it runs out over the lowest
/// point of its rim and stands at that level.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Pond {
    /// A point in the hollow, as shares of the plot's width (west to east)
    /// and length (north to south), each 0 to 1.
    pub at: [f64; 2],
    /// How deep the water is asked to stand over the lowest point of the
    /// hollow, in blocks.
    pub depth: f64,
    /// A round basin is dug this many blocks out from the point, so the
    /// water holds even on a slope. 0 fills only a hollow the land has.
    pub dig: f64,
}

impl Pond {
    /// A pond given as `x,z[,depth[,dig]]`: a point in its hollow as shares
    /// of the plot (0 to 1), the depth asked for and the basin dug, in blocks.
    pub fn parse(text: &str) -> Result<Pond, String> {
        let bad = || format!("expected x,z[,depth[,dig]] with numbers, got '{text}'");
        let n: Vec<f64> = text.split(',').map(|v| v.trim().parse::<f64>().map_err(|_| bad())).collect::<Result<_, _>>()?;
        if n.len() < 2 || n.len() > 4 {
            return Err(bad());
        }
        let standard = Pond::default();
        Ok(Pond { at: [n[0], n[1]], depth: n.get(2).copied().unwrap_or(standard.depth), dig: n.get(3).copied().unwrap_or(standard.dig) })
    }
}

impl Default for Pond {
    fn default() -> Self {
        Self { at: [0.5, 0.25], depth: 3.0, dig: 8.0 }
    }
}

/// Where a pond's water runs out when its hollow is too shallow.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Spill {
    /// Into the sea.
    Sea,
    /// Off the edge of the plot.
    Edge,
    /// Into a river or a fall.
    River,
}

/// How a pond came out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PondEnd {
    /// It was put in the sea, not on the land.
    Dry,
    /// The water stands as deep as asked: `depth` blocks at the deepest,
    /// with `size` blocks of surface.
    Held { depth: i32, size: usize },
    /// The hollow is shallower than asked: the water runs out at `to` and
    /// stands `depth` blocks deep. A depth of 0 means there is no hollow.
    Spills { depth: i32, size: usize, to: Spill },
}

impl Terrain {
    /// Lay a pond over the ground. See `Pond`.
    pub fn add_pond(&mut self, pond: &Pond) -> PondEnd {
        let end = self.fill_pond(pond);
        self.ponds.push(end);
        end
    }

    fn fill_pond(&mut self, pond: &Pond) -> PondEnd {
        use std::cmp::Reverse;
        use std::collections::BinaryHeap;
        /// Ground lower than this is under the sea.
        const SEA: f32 = 0.5;
        let (w, l) = (self.width, self.length);
        if w < 3 || l < 3 {
            return PondEnd::Dry;
        }
        let cells = (w * l) as usize;
        let mut x = ((pond.at[0].clamp(0.0, 1.0) * w as f64) as i32).min(w - 1);
        let mut z = ((pond.at[1].clamp(0.0, 1.0) * l as f64) as i32).min(l - 1);
        if self.height(x, z) < SEA {
            return PondEnd::Dry;
        }
        let depth = (pond.depth.round() as i32).clamp(1, 64);
        self.dig_basin(x, z, pond.dig, depth);
        // Down to the lowest point of the hollow.
        for _ in 0..4 * (w + l) {
            let mut lowest = (self.height(x, z), x, z);
            for (dx, dz) in [(-1, 0), (1, 0), (0, -1), (0, 1), (-1, -1), (1, -1), (-1, 1), (1, 1)] {
                let (nx, nz) = (x + dx, z + dz);
                if nx >= 0 && nz >= 0 && nx < w && nz < l && self.dry_land(nx, nz) && self.height(nx, nz) < lowest.0 {
                    lowest = (self.height(nx, nz), nx, nz);
                }
            }
            if (lowest.1, lowest.2) == (x, z) {
                break;
            }
            (x, z) = (lowest.1, lowest.2);
        }
        let top = |t: &Self, i: usize| t.height[i].round() as i32;
        let outlet = |t: &Self, i: usize| -> Option<Spill> {
            let (x, z) = ((i as i32) % w, (i as i32) / w);
            if t.height[i] < SEA {
                Some(Spill::Sea)
            } else if matches!(t.kind[i], Ground::Stream | Ground::Fall) {
                Some(Spill::River)
            } else if x == 0 || z == 0 || x == w - 1 || z == l - 1 {
                Some(Spill::Edge)
            } else {
                None
            }
        };
        let start = self.index(x, z);
        let bottom = top(self, start);
        let wanted = bottom + depth;
        // The water rises from the bottom; the ground is met lowest first,
        // so the first way out it comes to is the lowest point of the rim.
        let mut heap = BinaryHeap::new();
        let mut seen = vec![false; cells];
        seen[start] = true;
        heap.push(Reverse((bottom, start)));
        let (mut highest, mut spill) = (bottom, None);
        while let Some(Reverse((h, i))) = heap.pop() {
            highest = highest.max(h);
            if highest >= wanted {
                break;
            }
            if let Some(to) = outlet(self, i) {
                spill = Some(to);
                break;
            }
            let (cx, cz) = ((i as i32) % w, (i as i32) / w);
            for (dx, dz) in [(-1, 0), (1, 0), (0, -1), (0, 1)] {
                let (nx, nz) = (cx + dx, cz + dz);
                if nx >= 0 && nz >= 0 && nx < w && nz < l {
                    let n = (nz * w + nx) as usize;
                    if !seen[n] {
                        seen[n] = true;
                        heap.push(Reverse((top(self, n), n)));
                    }
                }
            }
        }
        let level = highest.min(wanted);
        // The water: every column below its level that it reaches from the
        // bottom without passing over the rim.
        let mut water = Vec::new();
        if level > bottom {
            let mut inside = vec![false; cells];
            let mut queue = vec![start];
            inside[start] = true;
            while let Some(i) = queue.pop() {
                water.push(i);
                let (cx, cz) = ((i as i32) % w, (i as i32) / w);
                for (dx, dz) in [(-1, 0), (1, 0), (0, -1), (0, 1)] {
                    let (nx, nz) = (cx + dx, cz + dz);
                    if nx >= 0 && nz >= 0 && nx < w && nz < l {
                        let n = (nz * w + nx) as usize;
                        if !inside[n] && top(self, n) < level && outlet(self, n).is_none() {
                            inside[n] = true;
                            queue.push(n);
                        }
                    }
                }
            }
        }
        if !water.is_empty() && self.bed.is_empty() {
            self.bed = vec![i32::MAX; cells];
        }
        for &i in &water {
            if self.kind[i] != Ground::Pond {
                self.bed[i] = top(self, i);
            }
            self.kind[i] = Ground::Pond;
            self.height[i] = level as f32;
        }
        let (depth, size) = ((level - bottom).max(0), water.len());
        match spill {
            Some(to) => PondEnd::Spills { depth, size, to },
            None => PondEnd::Held { depth, size },
        }
    }
}

impl Terrain {
    /// Dig a round basin `radius` blocks out from a column: its rim is the
    /// lowest ground around it, its bottom `depth` blocks below that, and
    /// ground higher than the rim inside it is cut down to it, so on a
    /// slope the basin is let into the hillside. The sea and running water
    /// are left as they are.
    /// Land the sea and running water have not taken.
    fn dry_land(&self, x: i32, z: i32) -> bool {
        self.height(x, z) >= 0.5 && !matches!(self.kind(x, z), Ground::Stream | Ground::Fall)
    }

    fn dig_basin(&mut self, cx: i32, cz: i32, radius: f64, depth: i32) {
        let r = radius.clamp(0.0, 64.0);
        if r < 1.5 {
            return;
        }
        let reach = r.ceil() as i32 + 1;
        let mut rim = i32::MAX;
        for z in cz - reach..=cz + reach {
            for x in cx - reach..=cx + reach {
                let d = ((x - cx) as f64).hypot((z - cz) as f64);
                if (d - r).abs() <= 0.75 && x >= 0 && z >= 0 && x < self.width && z < self.length && self.dry_land(x, z) {
                    rim = rim.min(self.height(x, z).round() as i32);
                }
            }
        }
        if rim == i32::MAX {
            return;
        }
        let rim = rim as f64;
        for z in (cz - reach).max(0)..=(cz + reach).min(self.length - 1) {
            for x in (cx - reach).max(0)..=(cx + reach).min(self.width - 1) {
                let d = ((x - cx) as f64).hypot((z - cz) as f64) / r;
                let i = self.index(x, z);
                if d >= 1.0 || !self.dry_land(x, z) {
                    continue;
                }
                // Never dug below the sea, which would flood it.
                let floor = (rim - depth as f64 * (1.0 - d * d)).max(1.0);
                self.height[i] = self.height[i].min(floor as f32);
            }
        }
    }
}

/// How a river or stream ended.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Outcome {
    /// There was no land for it to run over.
    Dry,
    /// It ends on the land, short of the sea.
    Inland,
    /// It runs into the sea through a plain mouth on this kind of shore.
    Mouth(ShoreKind),
    /// It runs into the sea over a delta of this size, 0 to 1.
    Delta(f64),
    /// It falls into the sea.
    Fall,
}

/// What the water is doing at one point of a stream's course.
#[derive(Clone, Copy, PartialEq)]
enum Reach {
    /// Running in its channel, the water's top at this level.
    Run(f64),
    /// Churned white below a fall, the water's top at this level.
    Pool(f64),
    /// Falling freely in front of a cliff from this level.
    Curtain(f64),
    /// White water tumbling down a slope.
    Cascade,
}

fn unit(x: f64, z: f64) -> (f64, f64) {
    let len = x.hypot(z);
    if len > 1e-9 { (x / len, z / len) } else { (0.0, 0.0) }
}

impl Terrain {
    /// Lay a river or stream over the ground. Works on any ground: the
    /// water never runs uphill (it cuts through rises instead), falls where
    /// the ground drops away, and stops at the sea. Where it runs into the
    /// sea it lays down a delta, as large as was asked for and as the shore
    /// there has room for: the shore is read from its shape (see `survey`),
    /// and there is most room on a low beach with shallow water in front,
    /// little on rock and none under a cliff. See `lay_delta`.
    pub fn add_stream(&mut self, stream: &Stream, seed: u32) -> Outcome {
        /// How far below its banks the water runs, in blocks.
        const CUT: f64 = 1.5;
        /// Ground lower than this is under the sea whatever the waves do.
        const SEA: f64 = -1.2;
        /// Samples of the course per block.
        const PER: f64 = 2.0;
        let course = stream.course(self.width, self.length, seed);
        let n = course.len();
        if n < 2 {
            return Outcome::Dry;
        }
        let width = stream.width.clamp(1.2, 48.0);
        let half = width * 0.5;
        let g: Vec<f64> = course.iter().map(|p| self.height(p.0.floor() as i32, p.1.floor() as i32) as f64).collect();
        let ahead = |i: usize, blocks: f64| g[(i + (blocks * PER) as usize).min(n - 1)];
        let back = |i: usize, blocks: f64| i.saturating_sub((blocks * PER) as usize);

        let mut reach: Vec<Option<Reach>> = vec![None; n];
        let Some(mut i) = g.iter().position(|h| *h >= 0.5) else { return Outcome::Dry };
        let mut level = (g[i] - CUT).max(0.0);
        // Where it fell into the sea, if it did.
        let mut fell_at = None;
        // It runs on across the wet sand until the sea covers the ground,
        // coming down to the sea's own level, so the two always join.
        while i < n && g[i] >= SEA {
            level = level.min((g[i] - CUT).max(0.0));
            reach[i] = Some(Reach::Run(level));
            if ahead(i, 2.5) >= level - 3.0 {
                i += 1;
                continue;
            }
            // The ground drops away ahead. Run on to the lip...
            while i + 1 < n && g[i + 1] >= level - 1.0 {
                i += 1;
                reach[i] = Some(Reach::Run(level));
            }
            let lip = i;
            // ...and find where the water lands: the sea, or ground that
            // has stopped dropping.
            let mut land = lip + 1;
            while land < n - 1 && g[land] >= 0.0 && !(g[land] - ahead(land, 2.0) < 1.4 && g[land] < level - 2.5) {
                land += 1;
            }
            if land >= n {
                break;
            }
            let into_sea = g[land] < 0.0;
            let drop = level - g[land].max(0.0);
            let steep = drop / ((land - lip) as f64 / PER).max(0.5) >= 1.5;
            // Down a cliff it falls freely, a little out from the face;
            // down a slope it tumbles over the ground.
            let over = if steep { (land + (1.5 * PER) as usize).min(n - 1) } else { land - 1 };
            for mark in reach.iter_mut().take(over + 1).skip(lip + 1) {
                *mark = Some(if steep { Reach::Curtain(level) } else { Reach::Cascade });
            }
            if into_sea {
                fell_at = Some(land);
                break;
            }
            // A churned pool where it lands, then the stream runs on.
            i = over + 1;
            if i >= n {
                break;
            }
            level = level.min((g[i] - CUT * 0.5).max(0.0));
            let pool = i + ((drop * 0.35).clamp(2.0, 7.0) * PER) as usize;
            while i < n && i < pool && g[i] >= 0.0 && ahead(i, 2.5) >= level - 3.0 {
                level = level.min((g[i] - CUT * 0.5).max(0.0));
                reach[i] = Some(Reach::Pool(level));
                i += 1;
            }
        }

        // How it meets the sea. Running out over low ground it slows and
        // spreads, so its channel widens toward the mouth, and the sand it
        // carried is laid down in front of it.
        let running = |k: usize| matches!(reach[k], Some(Reach::Run(_)));
        // A course that ends in the shallows has reached the sea as well.
        let ran_out = fell_at.is_none() && i > 0 && running(i - 1) && if i < n { g[i] < SEA } else { g[n - 1] < 0.3 };
        let size = stream.delta.clamp(0.0, 1.0);
        // The course point at the top of the beach face, how far the
        // channel has widened there, and where the widening began.
        let (mut apex, mut flare, mut flare_from) = (n, 0.0, n);
        // The way out to sea, the kind of shore it meets, and the size of
        // the delta it builds there: what was asked for, as far as the
        // shore has room for it.
        let (mut toward, mut shore, mut built) = ((0.0, 0.0), ShoreKind::Beach, 0.0);
        if ran_out {
            apex = (0..i).rev().find(|j| g[*j] >= 0.35 || !running(*j)).map_or(0, |j| j + 1).min(i - 1);
            toward = self.seaward(course[apex], course[back(apex, 6.0)], width);
            let read = read_shore(self, course[apex], toward);
            shore = read.kind;
            let low = (back(apex, 8.0)..=apex).all(|j| running(j) && g[j] < 5.0);
            if low && size * read.delta_room >= 0.12 {
                built = size * read.delta_room;
                flare = 0.35 * built;
                flare_from = back(apex, 3.0 * width).min(apex.saturating_sub(1));
                // The delta's own channels carry the water on from here.
                for mark in reach.iter_mut().skip(apex + 1) {
                    *mark = None;
                }
            }
        }
        let half_at = |k: usize| half * (1.0 + if k > flare_from { flare * smoothstep(flare_from as f64, apex as f64, k as f64) } else { 0.0 });

        // Each column takes after the nearest point of the course.
        const BANK: f64 = 10.0;
        let cells = (self.width * self.length) as usize;
        let mut nearest: Vec<(f32, u32)> = vec![(f32::INFINITY, 0); cells];
        for (k, (p, mark)) in course.iter().zip(&reach).enumerate() {
            let Some(mark) = mark else { continue };
            let r = half_at(k) + if matches!(mark, Reach::Run(_) | Reach::Pool(_)) { BANK } else { 0.0 };
            let (x0, x1) = (((p.0 - r).floor() as i32).max(0), ((p.0 + r).ceil() as i32).min(self.width - 1));
            let (z0, z1) = (((p.1 - r).floor() as i32).max(0), ((p.1 + r).ceil() as i32).min(self.length - 1));
            for z in z0..=z1 {
                for x in x0..=x1 {
                    let away = (x as f64 + 0.5 - p.0).hypot(z as f64 + 0.5 - p.1) as f32;
                    let cell = &mut nearest[(z * self.width + x) as usize];
                    if away <= r as f32 && away < cell.0 {
                        *cell = (away, k as u32);
                    }
                }
            }
        }
        for (i, (away, k)) in nearest.into_iter().enumerate() {
            if !away.is_finite() || self.roof.get(i).is_some_and(|r| r.is_some()) {
                continue;
            }
            let (away, h, half) = (away as f64, self.height[i] as f64, half_at(k as usize));
            match reach[k as usize] {
                Some(Reach::Run(level) | Reach::Pool(level)) if away <= half => {
                    if h > SEA - 0.5 {
                        self.height[i] = level as f32;
                        self.kind[i] = if matches!(reach[k as usize], Some(Reach::Pool(_))) { Ground::Fall } else { Ground::Stream };
                    }
                }
                Some(Reach::Run(level) | Reach::Pool(level)) => {
                    // Banks cut down to the water.
                    let bank = level + 0.6 + 0.9 * (away - half);
                    if h >= 0.0 && bank < h && !matches!(self.kind[i], Ground::Stream | Ground::Fall) {
                        self.height[i] = bank as f32;
                        if h - bank > 1.6 && self.kind[i] == Ground::Grass {
                            self.kind[i] = Ground::Rock;
                        }
                    }
                }
                Some(Reach::Curtain(level)) if away <= half => {
                    self.height[i] = (level - 0.5) as f32;
                    self.kind[i] = Ground::Fall;
                }
                Some(Reach::Cascade) if away <= half && h >= 0.5 => self.kind[i] = Ground::Fall,
                _ => {}
            }
        }

        // Where its water enters the sea, for the waves to meet.
        if let Some(land) = fell_at {
            let from = course[back(land, 4.0)];
            let toward = unit(course[land].0 - from.0, course[land].1 - from.1);
            self.mouths.push(Mouth { at: course[land], toward, width, fell: true });
            Outcome::Fall
        } else if !ran_out {
            Outcome::Inland
        } else if built > 0.0 {
            self.lay_delta(course[apex], toward, half * (1.0 + flare), built, width, seed);
            Outcome::Delta(built)
        } else {
            let waterline = (apex..i).find(|j| g[*j] < 0.0).unwrap_or(i);
            self.mouths.push(Mouth { at: course[waterline], toward, width, fell: false });
            Outcome::Mouth(shore)
        }
    }

    /// The way out to sea from a point on the shore: partly the way the
    /// water was already running, partly the way the ground falls.
    fn seaward(&self, at: (f64, f64), from: (f64, f64), width: f64) -> (f64, f64) {
        let run = unit(at.0 - from.0, at.1 - from.1);
        let ring = (2.0 * width).max(10.0);
        let (mut sx, mut sz) = (0.0, 0.0);
        for n in 0..24 {
            let (sin, cos) = (n as f64 / 24.0 * TAU).sin_cos();
            let pull = -(self.height((at.0 + cos * ring).floor() as i32, (at.1 + sin * ring).floor() as i32) as f64).clamp(-4.0, 4.0);
            sx += cos * pull;
            sz += sin * pull;
        }
        let sea = unit(sx, sz);
        let out = unit(0.45 * run.0 + 0.55 * sea.0, 0.45 * run.1 + 0.55 * sea.1);
        if out == (0.0, 0.0) { run } else { out }
    }

    /// The sand a river lays down where it runs into the sea over low
    /// ground. Nothing here is a shape laid over the shore: the channels are
    /// traced first and the sand follows from them, the way a delta grows.
    ///
    /// - Leaving its banks the water spreads and slows and drops its sand
    ///   in the middle of the flow; a bar grows there and the channel splits
    ///   around it (Wright 1977; Fagherazzi et al. 2015). The channels here
    ///   wander as they go and divide unevenly, the larger share turning
    ///   less, about 70 degrees apart as river forks tend to be.
    /// - Sand settles beside the water that carried it: highest along the
    ///   banks, thinning away from them and toward the sea. Where channels
    ///   run close their banks join into bars; where they part, a bay is
    ///   left between them. The old beach shows wherever it stands higher.
    /// - Past the dry sand the channels run on under water across a shallow
    ///   platform, and a shoal lies off the end of each, where waves coming
    ///   in feel the bottom and break. The platform's front falls steeply.
    ///
    /// `apex` is where the river leaves the beach, `mouth` its half-width
    /// there, `size` runs from 0 to 1 and `width` is the river's own width.
    fn lay_delta(&mut self, apex: (f64, f64), toward: (f64, f64), mouth: f64, size: f64, width: f64, seed: u32) {
        /// Shares of the way out at which the banks go under water and at
        /// which they have come down to the platform.
        const DRY: f64 = 0.5;
        const SHELF: f64 = 0.72;
        /// Where along a channel its water counts as having joined the sea.
        const OPEN: f64 = 0.62;
        /// Length of one step along a channel, in blocks.
        const STEP: f64 = 0.5;
        let reach = ((3.0 + 7.0 * size) * width).clamp(10.0, 110.0);
        let across = (-toward.1, toward.0);
        let turned = |angle: f64| {
            let (sin, cos) = angle.sin_cos();
            (toward.0 * cos + across.0 * sin, toward.1 * cos + across.1 * sin)
        };
        let roll = |n: u32| hash(n as i32, 77, seed ^ 0xA2B3) as f64 / u32::MAX as f64;
        // How many times the channel divides.
        let forks = match size {
            s if s < 0.2 => 0,
            s if s <= 0.45 => 1,
            s if s <= 0.8 => 2,
            _ => 3,
        };

        // The channels, as spots half a block apart: where, how wide, and
        // how far along the way out to sea.
        struct Spot {
            at: (f64, f64),
            half: f64,
            far: f64,
        }
        struct Branch {
            at: (f64, f64),
            angle: f64,
            half: f64,
            far: f64,
            forked: u32,
            id: u32,
        }
        struct End {
            at: (f64, f64),
            angle: f64,
            half: f64,
        }
        let mut spots: Vec<Spot> = Vec::new();
        let mut ends: Vec<End> = Vec::new();
        let mut open: Vec<End> = Vec::new();
        let mut waiting = vec![Branch { at: apex, angle: 0.25 * (roll(1) - 0.5), half: mouth, far: 0.0, forked: 0, id: 1 }];
        let mut born = 1;
        while let Some(branch) = waiting.pop() {
            let last = branch.forked >= forks || branch.half < 1.3;
            let lot = roll(branch.id * 7 + 2);
            let length = if last {
                // The last stretch runs out under the sea.
                ((0.82 + 0.14 * lot - branch.far) * reach).max(0.3 * reach)
            } else if branch.forked == 0 {
                (0.1 + 0.08 * lot) * reach
            } else {
                (0.2 + 0.16 * lot) * reach
            };
            let steps = (length / STEP).ceil().max(2.0) as usize;
            let (mut at, mut angle) = (branch.at, branch.angle);
            let mut joined = false;
            for k in 0..=steps {
                let t = k as f64 / steps as f64;
                let far = branch.far + k as f64 * STEP / reach;
                // It narrows a little as it goes; its last stretch opens out.
                let half = branch.half * if last { lerp(1.0, 0.85, (t / 0.6).min(1.0)) * (1.0 + 0.3 * smoothstep(0.6, 1.0, t)) } else { lerp(1.0, 0.9, t) };
                spots.push(Spot { at, half, far });
                if last && !joined && far >= OPEN {
                    joined = true;
                    open.push(End { at, angle, half });
                }
                if k == steps {
                    break;
                }
                // It wanders, without ever turning back for the land.
                angle += STEP * 0.13 * value(k as f64 * STEP / 8.0, branch.id as f64 * 9.1, seed ^ 0xC0A5);
                angle = angle.clamp(-1.3, 1.3);
                let way = turned(angle);
                at = (at.0 + way.0 * STEP, at.1 + way.1 * STEP);
            }
            if last {
                if !joined {
                    open.push(End { at, angle, half: branch.half });
                }
                ends.push(End { at, angle, half: branch.half * 0.85 * 1.3 });
                continue;
            }
            // It divides: unevenly, and the larger share turns the less.
            let share = 0.34 + 0.32 * roll(branch.id * 7 + 3);
            let part = 0.5 + 0.25 * roll(branch.id * 7 + 4);
            let side = if roll(branch.id * 7 + 5) < 0.5 { -1.0 } else { 1.0 };
            let far = branch.far + steps as f64 * STEP / reach;
            for (share, turn) in [(share, side * 2.0 * part * (1.0 - share)), (1.0 - share, -side * 2.0 * part * share)] {
                born += 1;
                waiting.push(Branch {
                    at,
                    angle: (angle + turn).clamp(-1.3, 1.3),
                    half: (branch.half * 0.85 * share.sqrt()).max(0.8),
                    // No two arms have built out equally far.
                    far: far + 0.16 * (roll(born * 7 + 6) - 0.5),
                    forked: branch.forked + 1,
                    id: born,
                });
            }
        }

        // Sand falls away from the banks by one block in this many.
        let run_at = |far: f64| (0.3 * reach * (1.0 - 0.6 * far.min(1.0))).max(2.5);
        let margin = 3.5 * run_at(0.0) + 8.0;
        let low = spots.iter().fold((f64::INFINITY, f64::INFINITY), |m, s| (m.0.min(s.at.0), m.1.min(s.at.1)));
        let high = spots.iter().fold((f64::NEG_INFINITY, f64::NEG_INFINITY), |m, s| (m.0.max(s.at.0), m.1.max(s.at.1)));
        let (x0, x1) = (((low.0 - margin).floor() as i32).max(0), ((high.0 + margin).ceil() as i32).min(self.width - 1));
        let (z0, z1) = (((low.1 - margin).floor() as i32).max(0), ((high.1 + margin).ceil() as i32).min(self.length - 1));
        // The nearest bank to a point: how far, and which spot.
        let bank = |p: (f64, f64)| {
            let mut best = (f64::INFINITY, 0);
            for (n, s) in spots.iter().enumerate() {
                let away = (p.0 - s.at.0).hypot(p.1 - s.at.1) - s.half;
                if away < best.0 {
                    best = (away, n);
                }
            }
            best
        };
        for z in z0..=z1 {
            for x in x0..=x1 {
                let i = (z * self.width + x) as usize;
                if self.roof.get(i).is_some_and(|r| r.is_some()) {
                    continue;
                }
                let p = (x as f64 + 0.5, z as f64 + 0.5);
                let h = self.height[i] as f64;
                let (beside, spot) = bank(p);
                if beside <= 0.0 {
                    // A channel runs level with the sea between dry banks,
                    // then deepens where the banks go under.
                    let bed = lerp(0.0, -1.6, smoothstep(DRY, SHELF, spots[spot].far));
                    if bed > -0.5 && h < 2.5 {
                        self.height[i] = bed as f32;
                        self.kind[i] = Ground::Stream;
                    } else if h < 1.5 && bed < h {
                        // Out on the platform the channel fades into the bed.
                        self.height[i] = lerp(bed, h, smoothstep(0.76, 0.9, spots[spot].far)) as f32;
                    }
                    continue;
                }
                if matches!(self.kind[i], Ground::Stream | Ground::Fall) {
                    continue;
                }
                // Sand lies ragged, not along drawn lines: each point looks
                // for its bank from a little way off.
                let shift = 1.8;
                let q = (
                    p.0 + shift * fbm(p.0 / 7.0, p.1 / 7.0, seed ^ 0x51F7, 2),
                    p.1 + shift * fbm(p.0 / 7.0 + 31.7, p.1 / 7.0 - 12.3, seed ^ 0x51F8, 2),
                );
                let (beside, spot) = bank(q);
                let far = spots[spot].far;
                let crest = lerp(lerp(1.1, 0.6, (far / DRY).min(1.0)), -1.0, smoothstep(DRY, SHELF, far));
                let lumps = 0.3 * fbm(p.0 / 5.0, p.1 / 5.0, seed ^ 0x5A2D, 2) + 0.3 * value(p.0 / 13.0, p.1 / 13.0, seed ^ 0x5A2E);
                let mut sand = crest - beside.max(0.0) / run_at(far) + lumps;
                // Below the reach of the waves the front stands steep.
                if sand < -1.2 {
                    sand = -1.2 + (sand + 1.2) * 4.0;
                }
                if sand > h {
                    self.height[i] = sand as f32;
                    self.kind[i] = Ground::Sand;
                } else if h < sand + 1.0 && h < 1.5 && sand > -0.5 {
                    // Low rock among the bars is buried too.
                    self.kind[i] = Ground::Sand;
                }
            }
        }

        // A shoal off the end of each channel, wider across the flow than
        // along it; some stand dry at the top, most stay just under.
        for (n, end) in ends.iter().enumerate() {
            let way = turned(end.angle);
            let side = (-way.1, way.0);
            let centre = (end.at.0 + way.0 * (1.5 * end.half + 2.0), end.at.1 + way.1 * (1.5 * end.half + 2.0));
            let (along, wide) = (1.2 * end.half + 1.5, 2.0 * end.half + 2.5);
            let peak = -0.9 + 0.9 * roll(900 + n as u32);
            let r = wide.ceil() as i32 + 1;
            for z in (centre.1 as i32 - r).max(0)..=(centre.1 as i32 + r).min(self.length - 1) {
                for x in (centre.0 as i32 - r).max(0)..=(centre.0 as i32 + r).min(self.width - 1) {
                    let i = (z * self.width + x) as usize;
                    let p = (x as f64 + 0.5, z as f64 + 0.5);
                    let bend = 0.3 * wide * value(p.0 / 6.0 + 5.5, p.1 / 6.0, seed ^ 0xBA3);
                    let (vx, vz) = (p.0 - centre.0 + way.0 * bend, p.1 - centre.1 + way.1 * bend);
                    let off = ((vx * way.0 + vz * way.1) / along).powi(2) + ((vx * side.0 + vz * side.1) / wide).powi(2);
                    let h = self.height[i] as f64;
                    if off >= 1.0 || h < -5.0 || matches!(self.kind[i], Ground::Stream | Ground::Fall) || self.roof.get(i).is_some_and(|r| r.is_some()) {
                        continue;
                    }
                    let top = peak - 1.4 * off + 0.35 * fbm(x as f64 / 4.0, z as f64 / 4.0, seed ^ 0xBA2, 2);
                    if top > h {
                        self.height[i] = top as f32;
                        self.kind[i] = Ground::Sand;
                    }
                }
            }
        }
        for end in open {
            self.mouths.push(Mouth { at: end.at, toward: turned(end.angle), width: 2.0 * end.half, fell: false });
        }
    }
}

/// The demonstration coast and cliff coast: in the development build only.
#[cfg(feature = "dev")]
mod presets;
#[cfg(feature = "dev")]
pub use presets::{cliff_scene, coast_scene};
