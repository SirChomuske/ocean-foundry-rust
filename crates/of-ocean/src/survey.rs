//! Reading the shore from the shape of the ground.
//!
//! Waves do different things on different shores, so before the sea is made
//! the ground is looked over and every stretch of shore is told apart:
//!
//! - a beach: low ground that rises gently out of shallow water. Waves
//!   shoal, break well before the land and run up it as a thin line of foam.
//! - a rocky shore: steeper or broken ground, or a low shelf of rock. Waves
//!   break against it in a broad, patchy band and are partly thrown back.
//! - a cliff: a face that stands well above the waves. Little of a wave is
//!   lost on it; most is thrown back, and the sea in front of it is the
//!   criss-cross of waves arriving and waves leaving.
//! - a river channel: narrow water reaching far into the land.
//!
//! None of this is read from what the ground is made of, except that a low
//! shelf of rock is not taken for a beach, so it works on any ground:
//! the demonstration scenes now and imported terrain later. The same
//! reading says how much room a river has to build a delta where it
//! reaches the sea: plenty on a low beach with shallow water in front,
//! little on rock, none under a cliff or where the bed drops away.

use crate::noise::{lerp, smoothstep};
use crate::render::Picture;
use crate::shore::blurred;
use crate::terrain::{Ground, Terrain};
use std::collections::VecDeque;

/// A face counts as a cliff when it rises at least this steeply, in blocks
/// per block, and stands at least this high within a few blocks of the water.
const CLIFF_SLOPE: f64 = 1.0;
const CLIFF_HEIGHT: f64 = 4.0;
/// A beach rises no more than this five and ten blocks inland, and the
/// water six blocks out is no deeper than this.
const BEACH_RISE: f64 = 3.0;
const BEACH_BACK: f64 = 6.5;
const BEACH_DEPTH: f64 = 5.0;
/// A shore this crooked (shore columns within four blocks) is broken rock.
const BROKEN: usize = 22;
/// Water no wider than this that reaches at least this far in from the
/// open sea is a river channel.
const CHANNEL_WIDTH: f64 = 26.0;
const CHANNEL_LENGTH: u32 = 30;
/// Far inside a channel, of this many looks around no more than
/// `CHANNEL_OPEN` find open water within twice that length.
const CHANNEL_LOOKS: u32 = 16;
const CHANNEL_OPEN: usize = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShoreKind {
    Beach,
    Rocky,
    Cliff,
}

impl ShoreKind {
    pub const ALL: [ShoreKind; 3] = [ShoreKind::Beach, ShoreKind::Rocky, ShoreKind::Cliff];

    pub fn key(self) -> &'static str {
        match self {
            ShoreKind::Beach => "beach",
            ShoreKind::Rocky => "rocky",
            ShoreKind::Cliff => "cliff",
        }
    }
}

/// What the ground is like at one place on the shore.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Reading {
    pub kind: ShoreKind,
    /// How steeply the land rises from the water, in blocks per block.
    pub face: f64,
    /// Height of the land ten blocks inland and depth of the water twelve
    /// blocks out.
    pub back: f64,
    pub depth: f64,
    /// How much room a river has to build a delta here, 0 to 1.
    pub delta_room: f64,
}

/// Read the shore at a point on the waterline. `out` is the way out to sea
/// as a unit vector.
pub fn read_shore(terrain: &Terrain, at: (f64, f64), out: (f64, f64)) -> Reading {
    let h = |s: f64| terrain.height((at.0 + out.0 * s).floor() as i32, (at.1 + out.1 * s).floor() as i32) as f64;
    let face = [0.0, 1.0, 2.0, 3.0, 5.0].into_iter().map(|s| h(-s) / (s + 1.0)).fold(0.0, f64::max);
    let tall = h(-3.0).max(h(-5.0));
    let (rise, back) = (h(-5.0), h(-10.0));
    let (foot, depth) = (-h(6.0), -h(12.0));
    let rock = terrain.kind(at.0.floor() as i32, at.1.floor() as i32) == Ground::Rock;
    let kind = if face >= CLIFF_SLOPE && tall >= CLIFF_HEIGHT {
        ShoreKind::Cliff
    } else if rise <= BEACH_RISE && back <= BEACH_BACK && foot <= BEACH_DEPTH && !rock {
        ShoreKind::Beach
    } else {
        ShoreKind::Rocky
    };
    let ground = match kind {
        ShoreKind::Beach => 1.0,
        ShoreKind::Rocky => 0.3,
        ShoreKind::Cliff => 0.0,
    };
    // Sand needs low ground to spread over and shallow water to stand in.
    let delta_room = ground * (1.0 - smoothstep(3.5, 7.0, h(-8.0))) * (1.0 - smoothstep(4.0, 10.0, depth));
    Reading { kind, face, back, depth, delta_room }
}

/// The shore of a whole plot, read once.
pub struct Survey {
    width: i32,
    length: i32,
    wet: Vec<bool>,
    /// The kind of each column that is land beside water.
    shore: Vec<Option<ShoreKind>>,
    /// Room for a delta at each such column.
    room: Vec<f32>,
    /// The nearest shore column to every column, or none on a plot without shore.
    nearest: Vec<u32>,
    /// How much of the shore near each column is cliff and how much rocky;
    /// the rest is beach.
    cliff: Vec<f32>,
    rocky: Vec<f32>,
    /// Distance to the shore: positive over water, negative over land.
    off: Vec<f32>,
    /// From each column of the sea to its image behind the shore, and how
    /// surely the shore faces it (0 where shores face each other).
    mirror: Vec<(f32, f32)>,
    firm: Vec<f32>,
    channel: Vec<bool>,
}

/// Distance of every column from the nearest marked one, and which that is.
///
/// Each column takes, from its neighbours, the marked column nearest to
/// them, and measures its own straight-line distance to it. Adding up steps
/// between neighbours instead would give distances that grow in octagons
/// rather than circles, whose slope turns in jumps at the diagonals from
/// every corner of the land; the waves thrown back off a shore follow that
/// slope and would show the jumps as creases.
pub(crate) fn distances(width: usize, length: usize, marked: &dyn Fn(usize) -> bool) -> (Vec<f32>, Vec<u32>) {
    let count = width * length;
    let mut away = vec![f32::INFINITY; count];
    let mut from = vec![u32::MAX; count];
    for i in (0..count).filter(|i| marked(*i)) {
        (away[i], from[i]) = (0.0, i as u32);
    }
    let pass = |i: usize, n: usize, away: &mut Vec<f32>, from: &mut Vec<u32>| {
        let site = from[n];
        if site == u32::MAX || site == from[i] {
            return;
        }
        let (dx, dz) = ((i % width) as f32 - (site as usize % width) as f32, (i / width) as f32 - (site as usize / width) as f32);
        let d = dx.hypot(dz);
        if d < away[i] {
            (away[i], from[i]) = (d, site);
        }
    };
    // Two sweeps each way: the second catches the few columns whose nearest
    // marked column reached them only round a corner of the first.
    for _ in 0..2 {
        for z in 0..length {
            for x in 0..width {
                let i = z * width + x;
                if x > 0 {
                    pass(i, i - 1, &mut away, &mut from);
                }
                if z > 0 {
                    pass(i, i - width, &mut away, &mut from);
                    if x > 0 {
                        pass(i, i - width - 1, &mut away, &mut from);
                    }
                    if x + 1 < width {
                        pass(i, i - width + 1, &mut away, &mut from);
                    }
                }
            }
            for x in (0..width.saturating_sub(1)).rev() {
                pass(z * width + x, z * width + x + 1, &mut away, &mut from);
            }
        }
        for z in (0..length).rev() {
            for x in (0..width).rev() {
                let i = z * width + x;
                if x + 1 < width {
                    pass(i, i + 1, &mut away, &mut from);
                }
                if z + 1 < length {
                    pass(i, i + width, &mut away, &mut from);
                    if x + 1 < width {
                        pass(i, i + width + 1, &mut away, &mut from);
                    }
                    if x > 0 {
                        pass(i, i + width - 1, &mut away, &mut from);
                    }
                }
            }
            for x in 1..width {
                pass(z * width + x, z * width + x - 1, &mut away, &mut from);
            }
        }
    }
    (away, from)
}

impl Survey {
    pub fn new(terrain: &Terrain) -> Self {
        let (w, l) = (terrain.width.max(0) as usize, terrain.length.max(0) as usize);
        let count = w * l;
        let at = |x: i32, z: i32| z as usize * w + x as usize;
        let inside = |x: i32, z: i32| x >= 0 && z >= 0 && (x as usize) < w && (z as usize) < l;
        let wet: Vec<bool> = (0..count).map(|i| terrain.height((i % w) as i32, (i / w) as i32).round() < 0.0).collect();
        let wet_at = |x: i32, z: i32| inside(x, z) && wet[at(x, z)];

        // Each column of land beside water is read along the way out to
        // sea, which is where most of the water around it lies.
        let mut shore: Vec<Option<ShoreKind>> = vec![None; count];
        let mut room = vec![0f32; count];
        let mut edge = Vec::new();
        for z in 0..l as i32 {
            for x in 0..w as i32 {
                let beside = (-1..=1).any(|dz| (-1..=1).any(|dx| wet_at(x + dx, z + dz)));
                if wet[at(x, z)] || !beside {
                    continue;
                }
                let (mut ox, mut oz) = (0.0, 0.0);
                for dz in -4..=4i32 {
                    for dx in -4..=4i32 {
                        if inside(x + dx, z + dz) {
                            let side = if wet[at(x + dx, z + dz)] { 1.0 } else { -1.0 };
                            ox += side * dx as f64;
                            oz += side * dz as f64;
                        }
                    }
                }
                let len = ox.hypot(oz);
                if len < 1e-6 {
                    // Water all round: a rock standing in the sea.
                    shore[at(x, z)] = Some(if terrain.height(x, z) >= CLIFF_HEIGHT as f32 { ShoreKind::Cliff } else { ShoreKind::Rocky });
                } else {
                    let read = read_shore(terrain, (x as f64 + 0.5, z as f64 + 0.5), (ox / len, oz / len));
                    shore[at(x, z)] = Some(read.kind);
                    room[at(x, z)] = read.delta_room as f32;
                }
                edge.push((x, z));
            }
        }
        // A beach does not have a broken edge; and a single column that
        // reads differently from all its neighbours is taken with them.
        let around = |shore: &Vec<Option<ShoreKind>>, x: i32, z: i32, r: i32| {
            let mut tally = [0usize; 3];
            for dz in -r..=r {
                for dx in -r..=r {
                    if let Some(kind) = inside(x + dx, z + dz).then(|| shore[at(x + dx, z + dz)]).flatten() {
                        tally[kind as usize] += 1;
                    }
                }
            }
            tally
        };
        let read = shore.clone();
        for (x, z) in &edge {
            if read[at(*x, *z)] == Some(ShoreKind::Beach) && around(&read, *x, *z, 4).iter().sum::<usize>() >= BROKEN {
                shore[at(*x, *z)] = Some(ShoreKind::Rocky);
                room[at(*x, *z)] *= 0.3;
            }
        }
        let read = shore.clone();
        for (x, z) in &edge {
            let tally = around(&read, *x, *z, 3);
            let own = read[at(*x, *z)].unwrap();
            let most = ShoreKind::ALL.into_iter().max_by_key(|k| tally[*k as usize]).unwrap();
            if tally[most as usize] > 2 * tally[own as usize] {
                shore[at(*x, *z)] = Some(most);
            }
        }

        let (_, nearest) = distances(w, l, &|i| shore[i].is_some());
        let (to_land, _) = distances(w, l, &|i| !wet[i]);
        let (to_sea, _) = distances(w, l, &|i| wet[i]);
        let off: Vec<f32> = (0..count)
            .map(|i| {
                let d = if wet[i] { to_land[i] - 0.5 } else { 0.5 - to_sea[i] };
                if d.is_finite() { d } else { 0.0 }
            })
            .collect();
        // Close to a shore its own shape counts; farther out, where the sea
        // lies between several shores, what is known of them is taken from
        // a wider and wider stretch, so that nothing changes at a line
        // half-way between two shores.
        let wide = |values: &[f64]| -> (Vec<f64>, Vec<f64>) { (blurred(values, w.max(1), 2), blurred(&blurred(values, w.max(1), 9), w.max(1), 9)) };
        let far = |i: usize| smoothstep(3.0, 18.0, off[i] as f64);
        let share = |kind: ShoreKind| -> Vec<f32> {
            let is: Vec<f64> = nearest.iter().map(|n| if *n != u32::MAX && shore[*n as usize] == Some(kind) { 1.0 } else { 0.0 }).collect();
            let (close, broad) = wide(&is);
            (0..count).map(|i| lerp(close[i], broad[i], far(i)) as f32).collect()
        };
        let (cliff, rocky) = (share(ShoreKind::Cliff), share(ShoreKind::Rocky));

        // The image of each column of the sea behind the shore, for the
        // waves the shore throws back: straight across the shore it faces.
        // Between two shores, where it faces neither, nothing is thrown back.
        let (close, broad) = wide(&off.iter().map(|d| *d as f64).collect::<Vec<_>>());
        let mut mirror = vec![(0f32, 0f32); count];
        let mut firm = vec![0f32; count];
        for z in 0..l {
            for x in 0..w {
                let i = z * w + x;
                if !wet[i] || nearest[i] == u32::MAX {
                    continue;
                }
                let slope = |field: &[f64]| {
                    (
                        (field[z * w + (x + 1).min(w - 1)] - field[z * w + x.saturating_sub(1)]) * 0.5,
                        (field[(z + 1).min(l - 1) * w + x] - field[z.saturating_sub(1) * w + x]) * 0.5,
                    )
                };
                let (near, wider) = (slope(&close), slope(&broad));
                let (gx, gz) = (lerp(near.0, wider.0, far(i)), lerp(near.1, wider.1, far(i)));
                let steep = gx.hypot(gz);
                if steep > 1e-6 {
                    let reach = 2.0 * (off[i] as f64).max(0.0) / steep;
                    mirror[i] = ((-gx * reach) as f32, (-gz * reach) as f32);
                    firm[i] = smoothstep(0.3, 0.8, steep) as f32;
                }
            }
        }
        // Half-way between two shores the distance field turns a corner, and
        // the column of sea a wave comes back from jumps across it. Left
        // sharp, that jump is a crease in the water a block wide, running
        // across the whole stretch the two shores share; it shows in the
        // picture as a hair of light lying across the waves. The image is
        // spread over a few blocks here instead, so the sea turns from one
        // shore's answer to the other's over a stretch rather than at a line.
        {
            let (wsize, lsize) = (w.max(1) as i32, l.max(1) as i32);
            let spread = |mirror: &mut Vec<(f32, f32)>, firm: &mut Vec<f32>| {
                let mut temp = vec![(0f32, 0f32); count];
                let mut temp_firm = vec![0f32; count];
                let radius = 3;
                for z in 0..lsize {
                    for x in 0..wsize {
                        let (mut sx, mut sz, mut sf, mut n) = (0f32, 0f32, 0f32, 0f32);
                        for d in -radius..=radius {
                            let j = (z * wsize + (x + d).clamp(0, wsize - 1)) as usize;
                            sx += mirror[j].0;
                            sz += mirror[j].1;
                            sf += firm[j];
                            n += 1.0;
                        }
                        let k = (z * wsize + x) as usize;
                        temp[k] = (sx / n, sz / n);
                        temp_firm[k] = sf / n;
                    }
                }
                for z in 0..lsize {
                    for x in 0..wsize {
                        let (mut sx, mut sz, mut sf, mut n) = (0f32, 0f32, 0f32, 0f32);
                        for d in -radius..=radius {
                            let j = ((z + d).clamp(0, lsize - 1) * wsize + x) as usize;
                            sx += temp[j].0;
                            sz += temp[j].1;
                            sf += temp_firm[j];
                            n += 1.0;
                        }
                        let k = (z * wsize + x) as usize;
                        mirror[k] = (sx / n, sz / n);
                        firm[k] = sf / n;
                    }
                }
            };
            spread(&mut mirror, &mut firm);
        }

        let channel = channels(w, l, &wet, &to_land);
        Self { width: w as i32, length: l as i32, wet, shore, room, nearest, cliff, rocky, off, mirror, firm, channel }
    }

    fn index(&self, x: i32, z: i32) -> usize {
        (z.clamp(0, self.length - 1) * self.width + x.clamp(0, self.width - 1)) as usize
    }

    /// The kind of shore at a column that is land beside water.
    pub fn shore(&self, x: i32, z: i32) -> Option<ShoreKind> {
        self.shore[self.index(x, z)]
    }

    /// The kind of the shore nearest to a column, and the room for a delta there.
    pub fn nearest(&self, x: i32, z: i32) -> Option<(ShoreKind, f64)> {
        let n = self.nearest[self.index(x, z)];
        (n != u32::MAX).then(|| (self.shore[n as usize].unwrap(), self.room[n as usize] as f64))
    }

    /// How much of the shore near a column is cliff and how much is rocky,
    /// each 0 to 1; what is left is beach.
    pub fn character(&self, x: i32, z: i32) -> (f64, f64) {
        let i = self.index(x, z);
        (self.cliff[i] as f64, self.rocky[i] as f64)
    }

    /// Blocks to the shore: positive over water, negative over land.
    pub fn off(&self, x: i32, z: i32) -> f64 {
        self.off[self.index(x, z)] as f64
    }

    /// For a column of the sea: the way to its image behind the shore, in
    /// blocks, and how surely the shore faces it, 0 to 1.
    pub fn mirror(&self, x: i32, z: i32) -> Option<((f64, f64), f64)> {
        let i = self.index(x, z);
        (self.firm[i] > 0.0).then(|| ((self.mirror[i].0 as f64, self.mirror[i].1 as f64), self.firm[i] as f64))
    }

    /// Whether a column is water in a river channel.
    pub fn channel(&self, x: i32, z: i32) -> bool {
        self.channel[self.index(x, z)]
    }

    /// How many columns of shore there are of each kind, and how many
    /// columns of river channel.
    pub fn tally(&self) -> ([usize; 3], usize) {
        let mut kinds = [0; 3];
        for kind in self.shore.iter().flatten() {
            kinds[*kind as usize] += 1;
        }
        (kinds, self.channel.iter().filter(|c| **c).count())
    }

    /// A map of the reading, north up: beach yellow, beach with room for a
    /// delta green, rocky shore brown, cliff red, river channels and
    /// running water turquoise; the sea paler where it is shallow.
    pub fn picture(&self, terrain: &Terrain, scale: u32) -> Picture {
        let scale = scale.max(1);
        let (w, l) = (self.width as u32, self.length as u32);
        let mut rgba = vec![255u8; (w * scale * l * scale * 4) as usize];
        for z in 0..l {
            for x in 0..w {
                let i = (z * w + x) as usize;
                let h = terrain.height(x as i32, z as i32) as f64;
                let colour = if self.wet[i] {
                    if self.channel[i] {
                        [40, 185, 180]
                    } else {
                        let deep = smoothstep(0.0, 14.0, -h);
                        [(176.0 - 96.0 * deep) as u8, (208.0 - 88.0 * deep) as u8, (236.0 - 56.0 * deep) as u8]
                    }
                } else if matches!(terrain.kind(x as i32, z as i32), Ground::Stream | Ground::Fall) {
                    [40, 185, 180]
                } else if self.off[i] > -2.6 && self.nearest[i] != u32::MAX {
                    let n = self.nearest[i] as usize;
                    match self.shore[n].unwrap() {
                        ShoreKind::Beach if self.room[n] >= 0.5 => [70, 185, 80],
                        ShoreKind::Beach => [244, 196, 48],
                        ShoreKind::Rocky => [150, 98, 58],
                        ShoreKind::Cliff => [205, 52, 52],
                    }
                } else {
                    let shade = (214.0 - 2.2 * h.clamp(0.0, 30.0)) as u8;
                    [shade, shade, shade.saturating_sub(8)]
                };
                for pz in 0..scale {
                    for px in 0..scale {
                        let p = (((z * scale + pz) * w * scale + x * scale + px) * 4) as usize;
                        rgba[p..p + 3].copy_from_slice(&colour);
                    }
                }
            }
        }
        Picture { width: w * scale, height: l * scale, rgba }
    }
}

/// Water in river channels: narrow, and reaching far in from the open sea.
fn channels(w: usize, l: usize, wet: &[bool], to_land: &[f32]) -> Vec<bool> {
    let count = w * l;
    let reach = (CHANNEL_WIDTH * 0.75) as i32;
    let hit = |x: usize, z: usize, dx: i32, dz: i32| -> Option<f64> {
        let step = ((dx * dx + dz * dz) as f64).sqrt();
        for n in 1..=reach {
            let (px, pz) = (x as i32 + dx * n, z as i32 + dz * n);
            if px < 0 || pz < 0 || px >= w as i32 || pz >= l as i32 {
                return None;
            }
            if !wet[pz as usize * w + px as usize] {
                return Some(n as f64 * step);
            }
        }
        None
    };
    let narrow: Vec<bool> = (0..count)
        .map(|i| {
            if !wet[i] || to_land[i] as f64 > CHANNEL_WIDTH * 0.5 + 1.0 {
                return false;
            }
            let (x, z) = (i % w, i / w);
            [(1, 0), (0, 1), (1, 1), (1, -1)].into_iter().any(|(dx, dz)| match (hit(x, z, dx, dz), hit(x, z, -dx, -dz)) {
                (Some(a), Some(b)) => a + b - 1.0 <= CHANNEL_WIDTH,
                _ => false,
            })
        })
        .collect();
    // A river is shut in by land. Water between a headland and a stack,
    // under an arch or among rocks off a shore is narrow too, but it lies
    // open to the sea: from there, many looks around find open water.
    let shut_in = |i: usize| -> bool {
        let (x, z) = ((i % w) as f64 + 0.5, (i / w) as f64 + 0.5);
        let mut open = 0;
        for k in 0..CHANNEL_LOOKS {
            let (dz, dx) = (k as f64 * std::f64::consts::TAU / CHANNEL_LOOKS as f64).sin_cos();
            let mut last = i;
            let mut clear = true;
            for n in 1..=2 * CHANNEL_LENGTH {
                let (px, pz) = ((x + dx * n as f64).floor(), (z + dz * n as f64).floor());
                if px < 0.0 || pz < 0.0 || px >= w as f64 || pz >= l as f64 {
                    // Past the edge of the plot a channel is taken to run
                    // on, and the sea to stay the sea.
                    clear = !narrow[last];
                    break;
                }
                last = pz as usize * w + px as usize;
                if !wet[last] {
                    clear = false;
                    break;
                }
            }
            if clear {
                open += 1;
                if open > CHANNEL_OPEN {
                    return false;
                }
            }
        }
        true
    };
    let inner: Vec<bool> = (0..count).map(|i| narrow[i] && shut_in(i)).collect();
    // How far in from open water each such column lies, going by water.
    let mut far = vec![u32::MAX; count];
    let mut queue = VecDeque::new();
    let beside = |i: usize| {
        let (x, z) = (i % w, i / w);
        let mut list = [usize::MAX; 4];
        if x > 0 {
            list[0] = i - 1;
        }
        if x + 1 < w {
            list[1] = i + 1;
        }
        if z > 0 {
            list[2] = i - w;
        }
        if z + 1 < l {
            list[3] = i + w;
        }
        list
    };
    for i in (0..count).filter(|i| inner[*i]) {
        if beside(i).into_iter().any(|n| n != usize::MAX && wet[n] && !inner[n]) {
            far[i] = 0;
            queue.push_back(i);
        }
    }
    while let Some(i) = queue.pop_front() {
        for n in beside(i) {
            if n != usize::MAX && inner[n] && far[n] == u32::MAX {
                far[n] = far[i] + 1;
                queue.push_back(n);
            }
        }
    }
    // A stretch of it is a channel if some of it lies far in: walk back out
    // from those columns toward the sea...
    let mut channel = vec![false; count];
    let mut queue: VecDeque<usize> = (0..count).filter(|i| far[*i] != u32::MAX && far[*i] >= CHANNEL_LENGTH).collect();
    for i in &queue {
        channel[*i] = true;
    }
    while let Some(i) = queue.pop_front() {
        for n in beside(i) {
            if n != usize::MAX && !channel[n] && far[n] != u32::MAX && far[n] < far[i] {
                channel[n] = true;
                queue.push_back(n);
            }
        }
    }
    // ...and on between its banks to its mouth, where the sea is in view.
    let mut queue: VecDeque<(usize, u32)> = (0..count).filter(|i| channel[*i]).map(|i| (i, 0)).collect();
    while let Some((i, gone)) = queue.pop_front() {
        if gone as f64 >= CHANNEL_WIDTH * 0.5 {
            continue;
        }
        for n in beside(i) {
            if n != usize::MAX && !channel[n] && narrow[n] {
                channel[n] = true;
                queue.push_back((n, gone + 1));
            }
        }
    }
    channel
}
