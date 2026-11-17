//! What land does to the waves: it slows and bends them over shallow water,
//! makes them go around it, shelters the water behind it, and breaks the
//! waves that are too tall for the depth.
//!
//! - Bending (refraction). A wave over shallow water runs slower, by the
//!   factor 1 / sqrt(tanh(k d)) (Eckart's approximation of the dispersion
//!   relation). The time the main wave needs to reach every column is found
//!   by letting one wave front sweep in from the open sea, each column
//!   passed at its own speed and land not passed at all (the eikonal
//!   equation, solved by fast marching). Crests lie along lines of equal
//!   arrival time, so they turn toward the shore, bunch up in the shallows
//!   and wrap around the back of islands, as real ones do.
//! - Shelter. How much of the sea reaches a column is found by looking
//!   upwind along a fan of lines as wide as the spread of wave directions:
//!   lines that meet land bring nothing. A confused sea therefore fills in
//!   behind an island sooner than a long swell.
//! - Breaking. Water can carry waves only up to a share of its depth
//!   (McCowan's 0.78; 0.6 to 0.8 in measurements). Along each line the
//!   shallowest water passed sets what is left of the wave.
//! - Rivers. Water running out of a river mouth spreads into the sea as a
//!   widening, fading current. A wave running against a current is slowed
//!   and shortened and grows taller; when the current reaches a quarter of
//!   the wave's own speed the wave can make no headway at all and breaks
//!   (Longuet-Higgins and Stewart 1961). Short waves are slow, so a river
//!   stops the chop long before it stops the swell, and its mouth is a
//!   tongue of smoother water with steep breaking waves along its edge.
//!   The slowing goes into the same sweep as the shallows, so crests also
//!   turn in toward the outflow.

use crate::build::fill_rows;
use crate::flow::Flow;
use crate::noise::smoothstep;
use crate::terrain::{Mouth, Terrain};
use crate::tuning::{NearLandTuning, OutflowTuning};
use crate::waves::{Near, WaveField};
use of_formats::Progress;
use std::cmp::Ordering;
use std::collections::BinaryHeap;
use std::f64::consts::TAU;

#[derive(Clone, Copy, Default)]
struct Cell {
    /// Height the long and the short waves keep.
    long: f32,
    short: f32,
    /// The long waves without the loss to breaking: what arrives, not what is left.
    open_long: f32,
    /// How far the arriving waves are too tall for the depth, 0 to 1.
    surf: f32,
}

/// The state of the waves at every column of a plot and a one-block rim around it.
pub struct Shore {
    row: usize,
    cells: Vec<Cell>,
    lag: Vec<f32>,
    squeeze: Vec<f32>,
    /// How strongly a river is running at each column, 0 to 1.
    plume: Vec<f32>,
}

/// How fast a wave travels through water that is itself moving, as a share
/// of its speed in still water. `current` is the water's speed along the
/// wave's path as a share of that still-water speed, negative against it.
fn pace(current: f64) -> f64 {
    0.5 * (1.0 + (1.0 + 4.0 * current).max(0.0).sqrt())
}

/// How much closer together a current brings the crests.
fn bunched(current: f64) -> f64 {
    pace(current.clamp(-0.2, 0.0)).powi(-2)
}

/// How much taller a current makes a wave. Against a current it grows, up
/// to `most`; past a quarter of its own speed it is stopped.
fn grown(current: f64, most: f64) -> f64 {
    let held = current.max(-0.2499);
    let c = pace(held);
    let taller = (c * (c + 2.0 * held)).max(1e-9).powf(-0.5);
    taller.min(most.max(1.0)) * smoothstep(-0.32, -0.24, current)
}

/// The current of the river mouths at every column of the sea, in units of
/// the speed at a mouth. Each outflow widens as it goes and fades with it.
fn outflows(row: usize, rows: usize, land: &[bool], mouths: &[Mouth], tuning: &OutflowTuning) -> Vec<(f32, f32)> {
    let mut run = vec![(0f32, 0f32); row * rows];
    let spread = tuning.spread.clamp(0.02, 2.0);
    for m in mouths {
        let share = if m.fell { tuning.fall_share.clamp(0.0, 1.0) } else { 1.0 };
        let half = (m.width * 0.5).max(1.0);
        let fade = tuning.reach.max(0.1) * 2.0 * half;
        let far = 4.0 * fade;
        // In columns of the plot with its rim.
        let (ax, az) = (m.at.0 + 1.0, m.at.1 + 1.0);
        // The water fans out as if from a point behind the mouth.
        let source = (ax - m.toward.0 * half / spread, az - m.toward.1 * half / spread);
        let around = far + half + spread * far;
        let (x0, x1) = (((ax - around).floor().max(0.0)) as usize, ((ax + around).ceil().max(0.0) as usize).min(row - 1));
        let (z0, z1) = (((az - around).floor().max(0.0)) as usize, ((az + around).ceil().max(0.0) as usize).min(rows - 1));
        for z in z0..=z1 {
            for x in x0..=x1 {
                let i = z * row + x;
                if land[i] {
                    continue;
                }
                let (vx, vz) = (x as f64 + 0.5 - ax, z as f64 + 0.5 - az);
                let along = vx * m.toward.0 + vz * m.toward.1;
                if along < -half || along > far {
                    continue;
                }
                let aside = vx * m.toward.1 - vz * m.toward.0;
                let out = along.max(0.0);
                let wide = half + spread * out;
                let strength = share * smoothstep(-half, half, along) * (half / wide).sqrt() * (-out / fade).exp() * (-(aside / wide).powi(2)).exp();
                if strength < 0.01 {
                    continue;
                }
                let (wx, wz) = (x as f64 + 0.5 - source.0, z as f64 + 0.5 - source.1);
                let len = wx.hypot(wz).max(1e-9);
                run[i].0 += (strength * wx / len) as f32;
                run[i].1 += (strength * wz / len) as f32;
            }
        }
    }
    // Outflows that meet do not add up to more than a river runs.
    for r in &mut run {
        let speed = r.0.hypot(r.1);
        if speed > 1.0 {
            *r = (r.0 / speed, r.1 / speed);
        }
    }
    run
}

struct Front(f64, usize);
impl PartialEq for Front {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}
impl Eq for Front {}
impl PartialOrd for Front {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for Front {
    // Earliest first out of the heap.
    fn cmp(&self, other: &Self) -> Ordering {
        other.0.total_cmp(&self.0)
    }
}

/// `r` where it is small, levelling off at 1.
fn eased(r: f64) -> f64 {
    let r = r.max(0.0);
    r / (1.0 + r.powi(4)).powf(0.25)
}

impl Shore {
    /// `tallest` is the height of the tallest waves of the open sea, in blocks.
    pub fn new(
        terrain: &Terrain,
        field: &WaveField,
        tallest: f64,
        tuning: &NearLandTuning,
        outflow: &OutflowTuning,
        flow: Option<&Flow>,
        metres: f64,
        progress: &Progress,
        span: (f32, f32),
    ) -> Result<Self, String> {
        let (row, rows) = ((terrain.width + 2) as usize, (terrain.length + 2) as usize);
        let count = row * rows;
        let ground = |i: usize| terrain.height((i % row) as i32 - 1, (i / row) as i32 - 1) as f64;
        let land: Vec<bool> = (0..count).map(|i| ground(i).round() >= 0.0).collect();

        // Depth as the waves feel it: averaged a little, with a cliff
        // counting as a wall rather than as very shallow water.
        let raw: Vec<f64> = (0..count).map(|i| (-ground(i)).max(-0.5)).collect();
        let depth = blurred(&raw, row, tuning.depth_smoothing.min(16) as usize);
        let deepest = depth.iter().cloned().fold(0.0, f64::max);

        // How much slower than in the deepest water of the plot the main
        // wave runs over each column.
        let k = TAU / field.peak_length();
        let (least, slowest) = (tuning.least_depth.max(0.05), tuning.slowest.max(1.0));
        let slow = |d: f64| (1.0 / (k * d.max(least)).tanh().sqrt()).min(slowest);
        let reference = slow(deepest);
        let mut squeeze: Vec<f32> = depth.iter().map(|d| (slow(*d) / reference).max(1.0) as f32).collect();

        let heading = field.heading();
        let mut arrival = arrivals(row, rows, &land, &squeeze, &depth, heading);

        // Rivers running out into the sea. What matters to a wave is the
        // current along its own path, as a share of its own speed: found
        // here for the main waves, from the way the crests already run.
        let mut against = vec![0f32; count];
        let mut plume = vec![0f32; count];
        let rivers = !terrain.mouths().is_empty() && outflow.speed > 0.0;
        if rivers || flow.is_some() {
            let run = if rivers { outflows(row, rows, &land, terrain.mouths(), outflow) } else { vec![(0f32, 0f32); count] };
            let speed = outflow.speed * (outflow.speed_for_length.max(1.0) / field.peak_length()).sqrt();
            // The speed of the main waves themselves, in metres a second.
            let own = (9.81 * field.peak_length() * metres.max(1e-3) / TAU).sqrt();
            let rise = |a: usize, b: usize| if arrival[a].is_finite() && arrival[b].is_finite() { arrival[b] - arrival[a] } else { 0.0 };
            for i in 0..count {
                let (mut ux, mut uz) = (speed * run[i].0 as f64, speed * run[i].1 as f64);
                plume[i] = (run[i].0 as f64).hypot(run[i].1 as f64).min(1.0) as f32;
                // Behind land a current is slack. The waves rode in on the
                // current, so to them slack water runs the other way.
                if let Some(f) = flow {
                    let lost = f.lost(i) / own;
                    ux -= lost * f.toward.0;
                    uz -= lost * f.toward.1;
                }
                if ux == 0.0 && uz == 0.0 {
                    continue;
                }
                let (x, z) = (i % row, i / row);
                let gx = rise(if x > 0 { i - 1 } else { i }, if x + 1 < row { i + 1 } else { i });
                let gz = rise(if z > 0 { i - row } else { i }, if z + 1 < rows { i + row } else { i });
                let way = if gx.hypot(gz) > 1e-6 { (gx / gx.hypot(gz), gz / gx.hypot(gz)) } else { heading };
                against[i] = (ux * way.0 + uz * way.1) as f32;
                // Slowed by the current, the crests bunch up and turn into it.
                squeeze[i] = (squeeze[i] as f64 * bunched(against[i] as f64)).min(slowest.max(squeeze[i] as f64)) as f32;
            }
            arrival = arrivals(row, rows, &land, &squeeze, &depth, heading);
        }
        // The short waves are slower, so the same current counts for more.
        let short_slower = (field.peak_length() / field.short_length().max(1.0)).sqrt().max(1.0);
        let most = outflow.gain_most;
        let against = &against;
        let lag: Vec<f32> = (0..count)
            .map(|i| {
                let along = (i % row) as f64 * heading.0 + (i / row) as f64 * heading.1;
                if arrival[i].is_finite() { (arrival[i] - along).max(0.0) as f32 } else { 0.0 }
            })
            .collect();
        progress.step(span.0 + (span.1 - span.0) * 0.3).map_err(|p| p.key.to_string())?;

        // The fan of lines looked along, and how much of the sea each stands for.
        let rays = tuning.shelter_rays.clamp(1, 90) as usize;
        let fan = tuning.shelter_fan.clamp(0.0, 170.0).to_radians();
        let bend = tuning.bend_share.clamp(0.0, 1.0);
        let weights = |s: f64| -> Vec<f64> {
            let raw: Vec<f64> = (0..rays).map(|j| ((offset(j, rays, fan) * 0.5).cos().abs()).powf(2.0 * s)).collect();
            let total: f64 = raw.iter().sum::<f64>().max(1e-300);
            raw.iter().map(|w| (1.0 - bend) * w / total + bend / rays as f64).collect()
        };
        let (for_long, for_short) = (weights(field.spread()), weights((field.spread() * 0.25).max(1.0)));
        let up = heading.1.atan2(heading.0) + std::f64::consts::PI;
        let lines: Vec<(f64, f64)> = (0..rays).map(|j| (up + offset(j, rays, fan)).sin_cos()).map(|(s, c)| (c, s)).collect();
        const STEP: f64 = 1.5;
        let steps = ((tuning.shelter_reach.max(0.0) * field.peak_length() / STEP) as usize).max(1);
        let ratio = tuning.breaker_ratio.max(0.01);
        let (keep, floor, regrow) = (tuning.height_at_shore.clamp(0.0, 1.0), tuning.shelter_floor.clamp(0.0, 1.0), tuning.chop_regrow.max(0.1));

        let mut cells = vec![Cell { long: 1.0, short: 1.0, open_long: 1.0, surf: 0.0 }; count];
        if tallest > 1e-9 {
            fill_rows(&mut cells, row, progress, (span.0 + (span.1 - span.0) * 0.3, span.1), &|gz, out| {
                for (gx, cell) in out.iter_mut().enumerate() {
                    let here = depth[gz * row + gx];
                    let (mut long, mut open_long, mut open_short) = (0.0, 0.0, 0.0);
                    for (j, line) in lines.iter().enumerate() {
                        let (mut px, mut pz) = (gx as f64 + 0.5, gz as f64 + 0.5);
                        let (mut shallowest, mut blocked) = (here, None);
                        for n in 1..=steps {
                            px += line.0 * STEP;
                            pz += line.1 * STEP;
                            if px < 0.0 || pz < 0.0 || px >= row as f64 || pz >= rows as f64 {
                                break;
                            }
                            let i = pz as usize * row + px as usize;
                            if land[i] {
                                blocked = Some(n as f64 * STEP);
                                break;
                            }
                            shallowest = shallowest.min(depth[i]);
                        }
                        match blocked {
                            // Short waves are raised again by the wind behind land.
                            Some(distance) => open_short += for_short[j] * (1.0 - (-distance / regrow).exp()).powi(2),
                            None => {
                                open_long += for_long[j];
                                open_short += for_short[j];
                                long += for_long[j] * eased(ratio * shallowest / tallest).powi(2);
                            }
                        }
                    }
                    let (open_long, open_short) = (open_long.sqrt(), open_short.sqrt());
                    let long = long.sqrt().max(floor * eased(ratio * here / tallest));
                    let short = open_short * eased(ratio * here / (0.4 * tallest));
                    // A river's current makes them taller, as far as the
                    // depth lets them be, or stops them.
                    let current = against[gz * row + gx] as f64;
                    let (grow_long, grow_short) = (grown(current, most), grown(current * short_slower, most));
                    let met = |kept: f64, grow: f64, room: f64| if grow > 1.0 { (kept * grow).min(kept.max(room)) } else { kept * grow };
                    let arriving = tallest * open_long * grow_long.max(1.0);
                    let surf = smoothstep(0.0, 0.5, 1.0 - ratio * here / (arriving + 1e-9)) * smoothstep(0.5, 1.5, arriving);
                    *cell = Cell {
                        long: met(keep + (1.0 - keep) * long, grow_long, ratio * here / tallest) as f32,
                        short: met(keep + (1.0 - keep) * short, grow_short, ratio * here / (0.4 * tallest)) as f32,
                        open_long: open_long as f32,
                        surf: surf as f32,
                    };
                }
            })?;
        }
        if let Some(f) = flow {
            for (i, cell) in cells.iter_mut().enumerate() {
                cell.short *= f.ruffled(i) as f32;
            }
        }
        Ok(Self { row, cells, lag, squeeze, plume })
    }

    fn index(&self, x: i32, z: i32) -> usize {
        (z + 1) as usize * self.row + (x + 1) as usize
    }

    /// The waves at a column, for the shape of the water. Columns run from
    /// -1 to the width or length, so the rim is included.
    pub fn near(&self, x: i32, z: i32) -> Near {
        let i = self.index(x, z);
        Near { lag: self.lag[i] as f64, long: self.cells[i].long as f64, short: self.cells[i].short as f64, squeeze: self.squeeze[i] as f64 }
    }

    /// How strongly a river is running out at a column: 1 at its mouth,
    /// fading to 0 where the sea has taken it in.
    pub fn plume(&self, x: i32, z: i32) -> f64 {
        self.plume[self.index(x, z)] as f64
    }

    /// How far the arriving waves are too tall for the depth, 0 to 1.
    pub fn surf(&self, x: i32, z: i32) -> f64 {
        self.cells[self.index(x, z)].surf as f64
    }

    /// How open a column is to the sea: 1 faces it fully, 0 is in complete shelter.
    pub fn open(&self, x: i32, z: i32) -> f64 {
        self.cells[self.index(x, z)].open_long as f64
    }
}

/// Angle of line `j` of `count`, fanned out by `fan` to each side.
fn offset(j: usize, count: usize, fan: f64) -> f64 {
    if count < 2 { 0.0 } else { fan * (2.0 * j as f64 / (count - 1) as f64 - 1.0) }
}

/// Average over a square of `radius` blocks to each side. Near an edge the
/// square shrinks to what fits evenly around the block: a square cut off on
/// one side would pull every slope toward the middle of the plot and leave
/// a crease `radius` blocks in from the edge.
pub(crate) fn blurred(values: &[f64], row: usize, radius: usize) -> Vec<f64> {
    if radius == 0 {
        return values.to_vec();
    }
    let rows = values.len() / row;
    let pass = |from: &[f64], along_rows: bool| -> Vec<f64> {
        let mut out = vec![0.0; from.len()];
        let (lines, length) = if along_rows { (rows, row) } else { (row, rows) };
        for line in 0..lines {
            let at = |i: usize| if along_rows { line * row + i } else { i * row + line };
            for i in 0..length {
                let fits = radius.min(i).min(length - 1 - i);
                let (lo, hi) = (i - fits, i + fits);
                out[at(i)] = (lo..=hi).map(|n| from[at(n)]).sum::<f64>() / (hi - lo + 1) as f64;
            }
        }
        out
    };
    pass(&pass(values, true), false)
}

/// When the main wave front reaches every column, in blocks of deep-water
/// travel. Columns it cannot reach stay at infinity.
fn arrivals(row: usize, rows: usize, land: &[bool], slow: &[f32], depth: &[f64], heading: (f64, f64)) -> Vec<f64> {
    let count = row * rows;
    let mut arrival = vec![f64::INFINITY; count];
    let mut settled = vec![false; count];
    let mut heap = BinaryHeap::new();

    // The front comes in over the edges that face the open sea: each such
    // edge as a line of columns, with the share of the heading that crosses it.
    let mut edges: Vec<(Vec<usize>, f64)> = Vec::new();
    if heading.0.abs() > 1e-9 {
        let x = if heading.0 > 0.0 { 0 } else { row - 1 };
        edges.push(((0..rows).map(|z| z * row + x).collect(), heading.0.abs()));
    }
    if heading.1.abs() > 1e-9 {
        let z = if heading.1 > 0.0 { 0 } else { rows - 1 };
        edges.push(((0..row).map(|x| z * row + x).collect(), heading.1.abs()));
    }
    let deepest = edges.iter().flat_map(|(line, _)| line).filter(|i| !land[**i]).map(|i| depth[*i]).fold(0.0, f64::max);
    for (line, across) in &edges {
        // Where the edge is deep the front arrives undisturbed.
        let deep: Vec<bool> = line.iter().map(|i| !land[*i] && depth[*i] >= 0.6 * deepest).collect();
        for (n, i) in line.iter().enumerate() {
            if deep[n] {
                arrival[*i] = (i % row) as f64 * heading.0 + (i / row) as f64 * heading.1;
            }
        }
        // Shallow stretches of the edge get the front as it would be if
        // the sea bed went on unchanged beyond the plot: already slowed and
        // turned, by the same law that bends light (Snell's). Without this
        // the crests would bend the wrong way where a shore leaves the plot.
        let mut pass = |from: usize, to: usize| {
            let (a, b) = (line[from], line[to]);
            if !deep[to] && !land[b] && arrival[a].is_finite() {
                let pace = ((slow[b] as f64).powi(2) - across * across).max(0.0).sqrt();
                arrival[b] = arrival[b].min(arrival[a] + pace);
            }
        };
        for n in 1..line.len() {
            pass(n - 1, n);
        }
        for n in (1..line.len()).rev() {
            pass(n, n - 1);
        }
    }
    for (line, _) in &edges {
        for i in line {
            if arrival[*i].is_finite() {
                heap.push(Front(arrival[*i], *i));
            }
        }
    }

    while let Some(Front(time, i)) = heap.pop() {
        if settled[i] || time > arrival[i] {
            continue;
        }
        settled[i] = true;
        let (x, z) = (i % row, i / row);
        let mut visit = |n: usize| {
            if settled[n] || land[n] {
                return;
            }
            let (nx, nz) = (n % row, n / row);
            let earliest = |a: Option<usize>, b: Option<usize>| -> f64 {
                [a, b].into_iter().flatten().filter(|c| settled[*c]).map(|c| arrival[c]).fold(f64::INFINITY, f64::min)
            };
            let a = earliest((nx > 0).then(|| n - 1), (nx + 1 < row).then(|| n + 1));
            let b = earliest((nz > 0).then(|| n - row), (nz + 1 < rows).then(|| n + row));
            let pace = slow[n] as f64;
            let (first, second) = (a.min(b), a.max(b));
            let reached = if second - first >= pace {
                first + pace
            } else {
                0.5 * (first + second + (2.0 * pace * pace - (second - first).powi(2)).sqrt())
            };
            if reached < arrival[n] {
                arrival[n] = reached;
                heap.push(Front(reached, n));
            }
        };
        if x > 0 {
            visit(i - 1);
        }
        if x + 1 < row {
            visit(i + 1);
        }
        if z > 0 {
            visit(i - row);
        }
        if z + 1 < rows {
            visit(i + row);
        }
    }
    arrival
}
