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
use crate::noise::{smoothstep, value};
use crate::survey::distances;
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
        // How far every column lies from land, for fading a shadow out.
        let (from_land, _) = distances(row, rows, &|i| land[i]);
        let mut against = vec![0f32; count];
        let mut plume = vec![0f32; count];
        let rivers = !terrain.mouths().is_empty() && outflow.speed > 0.0;
        if rivers || flow.is_some() {
            let run = if rivers { outflows(row, rows, &land, terrain.mouths(), outflow) } else { vec![(0f32, 0f32); count] };
            let speed = outflow.speed * (outflow.speed_for_length.max(1.0) / field.peak_length()).sqrt();
            // The speed of the main waves themselves, in metres a second.
            let own = (9.81 * field.peak_length() * metres.max(1e-3) / TAU).sqrt();
            let rise = |a: usize, b: usize| if arrival[a].is_finite() && arrival[b].is_finite() { arrival[b] - arrival[a] } else { 0.0 };
            // A current's shadow has an edge a block or two wide, and a wave
            // bending across so sharp a step leaves a crease in the sea.
            // Softened over a part of a wave length, the bend reads as a swell.
            let soften = ((field.peak_length() / 12.0).round() as usize).clamp(1, 12);
            let lost_smooth: Option<Vec<f64>> = flow.map(|f| {
                let rough: Vec<f64> = (0..count).map(|i| f.lost(i)).collect();
                blurred(&rough, row, soften)
            });
            for i in 0..count {
                let (mut ux, mut uz) = (speed * run[i].0 as f64, speed * run[i].1 as f64);
                plume[i] = (run[i].0 as f64).hypot(run[i].1 as f64).min(1.0) as f32;
                // Behind land a current is slack. The waves rode in on the
                // current, so to them slack water runs the other way.
                if let Some(f) = flow {
                    // Bending the waves this hard drew creases where the
                    // shadows end, so the waves turn only a little.
                    const BEND: f64 = 0.25;
                    // A wake opens at 19.47 degrees either side of the run
                    // and thins out with distance as x^(-1/3), so the
                    // V dies away into the plain sea instead of ruling
                    // lines to the far edge.
                    let span = 2.5 * field.peak_length().max(1.0);
                    let thin = (1.0 + from_land[i] as f64 / span).powf(-1.0 / 3.0);
                    let lost = lost_smooth.as_ref().map_or(0.0, |v| v[i]) * BEND * thin / own;
                    ux -= lost * f.toward.0;
                    uz -= lost * f.toward.1;
                    // A wake opens at 19.47 degrees either side of the
                    // run of the water, whatever its speed: crests that
                    // would turn further are not in the pattern, so the
                    // turn is held inside that wedge.
                    const WEDGE: f64 = 0.3536;
                    let along = ux * f.toward.0 + uz * f.toward.1;
                    let across = -ux * f.toward.1 + uz * f.toward.0;
                    let limit = WEDGE * along.abs().max(own * 0.05);
                    if across.abs() > limit {
                        let fix = across.signum() * limit - across;
                        ux -= fix * f.toward.1;
                        uz += fix * f.toward.0;
                    }
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
        // A wave that went round land arrives late, and the delay it picked up
        // is carried downstream as it is: the crests behind the land stay
        // shifted as a whole, and the shift ends in a step where the sheltered
        // water meets the open sea. That step rules a straight seam across the
        // water to its edge, and no fading of the waves takes it away, because
        // a shift of the crests is still a shift when the crests are small.
        // The sea fills in behind land again, so the delay is let go of over a
        // few wavelengths, and smoothed over a part of one so the crests bend
        // back into the open sea instead of breaking off at a line.
        let relax = 3.0 * field.peak_length().max(1.0);
        let soften = ((field.peak_length() / 10.0).round() as usize).clamp(1, 8);
        let held: Vec<f64> = (0..count)
            .map(|i| {
                let along = (i % row) as f64 * heading.0 + (i / row) as f64 * heading.1;
                let hold = if arrival[i].is_finite() { (arrival[i] - along).max(0.0) } else { 0.0 };
                hold * (-(from_land[i] as f64) / relax).exp()
            })
            .collect();
        let mut lag: Vec<f32> = blurred(&held, row, soften).into_iter().map(|l| l as f32).collect();
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
        const STEP: f64 = 3.0;
        let steps = ((tuning.shelter_reach.max(0.0) * field.peak_length() / STEP) as usize).max(1);
        // Each line stands for the wedge of sea between it and the next,
        // which widens with distance. Land passing through part of a wedge
        // shelters that part, so the shelter fades in across the wedge
        // rather than switching on at the line itself: a line that is
        // either blocked or not would draw its edge through every corner
        // of the land as a straight streak across the water.
        let half_wedge = if rays > 1 { (fan / (rays - 1) as f64).min(1.2).tan() } else { 1.0 };
        let (to_land, _) = distances(row, rows, &|i| land[i]);
        let (to_sea, _) = distances(row, rows, &|i| !land[i]);
        // How far into the water (above 0) or into the land (below) each
        // column lies, read between columns so it changes smoothly.
        let side: Vec<f32> = (0..count).map(|i| if land[i] { 0.5 - to_sea[i] } else { to_land[i] - 0.5 }.clamp(-1e6, 1e6)).collect();
        let side_at = |px: f64, pz: f64| -> f64 {
            let (fx, fz) = ((px - 0.5).clamp(0.0, (row - 1) as f64), (pz - 0.5).clamp(0.0, (rows - 1) as f64));
            let (x0, z0) = (fx as usize, fz as usize);
            let (x1, z1) = ((x0 + 1).min(row - 1), (z0 + 1).min(rows - 1));
            let (tx, tz) = (fx - x0 as f64, fz - z0 as f64);
            let v = |x: usize, z: usize| side[z * row + x] as f64;
            let top = v(x0, z0) + (v(x1, z0) - v(x0, z0)) * tx;
            let bottom = v(x0, z1) + (v(x1, z1) - v(x0, z1)) * tx;
            top + (bottom - top) * tz
        };
        let ratio = tuning.breaker_ratio.max(0.01);
        // How much of the long waves the depth at each column lets through,
        // averaged over the width of a wedge where a line passes. Read
        // along the line alone, the shallows would switch on and off at
        // its edge as sharply as land does. Land itself counts as passing
        // all: what it stops is counted above.
        let shoal: Vec<f64> = (0..count).map(|i| if land[i] { 1.0 } else { eased(ratio * depth[i] / tallest.max(1e-9)) }).collect();
        let (srow, mut summed) = (row + 1, vec![0f64; (row + 1) * (rows + 1)]);
        for z in 0..rows {
            for x in 0..row {
                summed[(z + 1) * srow + x + 1] = shoal[z * row + x] + summed[z * srow + x + 1] + summed[(z + 1) * srow + x] - summed[z * srow + x];
            }
        }
        let square = |x: usize, z: usize, r: usize| -> f64 {
            let (x0, z0, x1, z1) = (x.saturating_sub(r), z.saturating_sub(r), (x + r + 1).min(row), (z + r + 1).min(rows));
            let total = summed[z1 * srow + x1] - summed[z0 * srow + x1] - summed[z1 * srow + x0] + summed[z0 * srow + x0];
            total / ((x1 - x0) * (z1 - z0)) as f64
        };
        // How far each column lies from land or from a sudden change in
        // depth, such as the edge of a shelf. Where a wedge is narrower than
        // that, the land cannot reach into it and the depth across it is
        // as at its middle, so most of the sea takes one look a step, as
        // fast as a bare line.
        let sharp = |i: usize| {
            let (x, z) = (i % row, i / row);
            [(x > 0).then(|| i - 1), (x + 1 < row).then(|| i + 1), (z > 0).then(|| i - row), (z + 1 < rows).then(|| i + row)]
                .into_iter()
                .flatten()
                .any(|n| (shoal[n] - shoal[i]).abs() > 0.01)
        };
        let (clear, _) = distances(row, rows, &|i| land[i] || sharp(i));
        let shoal_at = |px: f64, pz: f64, wide: f64| -> f64 {
            let (x, z) = (px as usize, pz as usize);
            let r = (wide - 0.5).max(0.0);
            let (lo, t) = (r.floor() as usize, r.fract());
            square(x, z, lo) * (1.0 - t) + square(x, z, lo + 1) * t
        };
        let (keep, floor, regrow) = (tuning.height_at_shore.clamp(0.0, 1.0), tuning.shelter_floor.clamp(0.0, 1.0), tuning.chop_regrow.max(0.1));

        let mut cells = vec![Cell { long: 1.0, short: 1.0, open_long: 1.0, surf: 0.0 }; count];
        if tallest > 1e-9 {
            fill_rows(&mut cells, row, progress, (span.0 + (span.1 - span.0) * 0.3, span.1), &|gz, out| {
                for (gx, cell) in out.iter_mut().enumerate() {
                    let here = depth[gz * row + gx];
                    let (mut long, mut open_long, mut open_short) = (0.0, 0.0, 0.0);
                    // The lines are turned a little, this way and that,
                    // so a shadow wanders as it runs out instead of
                    // ruling a straight edge across the water.
                    let turn = value(gx as f64 / 26.0, gz as f64 / 26.0, 0x51A7) * 0.07;
                    let (turn_s, turn_c) = turn.sin_cos();
                    for (j, line) in lines.iter().enumerate() {
                        let line = (line.0 * turn_c - line.1 * turn_s, line.0 * turn_s + line.1 * turn_c);
                        let (mut px, mut pz) = (gx as f64 + 0.5, gz as f64 + 0.5);
                        // The share of the wedge that gets through, and how
                        // far off the land that took the rest of it lies.
                        let (mut shoaled, mut through, mut distance) = (eased(ratio * here / tallest), 1.0f64, 0.0);
                        for n in 1..=steps {
                            px += line.0 * STEP;
                            pz += line.1 * STEP;
                            if px < 0.0 || pz < 0.0 || px >= row as f64 || pz >= rows as f64 {
                                break;
                            }
                            let away = n as f64 * STEP;
                            let wide = (away * half_wedge).max(0.5);
                            let i = pz as usize * row + px as usize;
                            if clear[i] as f64 > wide + 2.5 {
                                shoaled = shoaled.min(shoal[i]);
                                continue;
                            }
                            let left = (0.5 + side_at(px, pz) / (2.0 * wide)).clamp(0.0, 1.0);
                            if left < through {
                                (through, distance) = (left, away);
                            }
                            if through < 0.005 {
                                break;
                            }
                            shoaled = shoaled.min(shoal_at(px, pz, wide));
                        }
                        // Short waves are raised again by the wind behind land.
                        let raised = if distance > 0.0 { (1.0 - (-distance / regrow).exp()).powi(2) } else { 0.0 };
                        open_long += for_long[j] * through;
                        open_short += for_short[j] * (through + (1.0 - through) * raised);
                        long += for_long[j] * through * shoaled.powi(2);
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
        // A shadow does not run to the horizon. Diffraction fills a lee back
        // in, the height there falling about as 1/sqrt(1 + x/L) with L a
        // few wavelengths, so the sea behind land looks itself again soon.
        let span = 2.5 * field.peak_length().max(1.0);
        for (i, cell) in cells.iter_mut().enumerate() {
            let left = 1.0 / (1.0 + clear[i] as f64 / span).sqrt();
            cell.long = (1.0 - (1.0 - cell.long as f64) * left) as f32;
            cell.short = (1.0 - (1.0 - cell.short as f64) * left) as f32;
            cell.open_long = (1.0 - (1.0 - cell.open_long as f64) * left) as f32;
        }
        // A low wall of a build is washed over: the crests run on across it
        // instead of ending at a vertical face, so it takes the waves of the
        // water beside it. A natural shore keeps its own edge.
        const LOW_TOP: i32 = 3;
        if terrain.placed.is_some() {
            let low: Vec<bool> = (0..count)
                .map(|i| land[i] && ground(i).round() as i32 <= LOW_TOP)
                .collect();
            let mut water_near = vec![None; count];
            for i in 0..count {
                if !low[i] {
                    continue;
                }
                let (x, z) = ((i % row) as i32, (i / row) as i32);
                let mut best = f64::MAX;
                let mut pick = None;
                for dz in -6..=6i32 {
                    for dx in -6..=6i32 {
                        let (nx, nz) = (x + dx, z + dz);
                        if nx < 0 || nz < 0 || nx >= row as i32 || nz >= rows as i32 {
                            continue;
                        }
                        let j = nz as usize * row + nx as usize;
                        if land[j] {
                            continue;
                        }
                        let far = (dx * dx + dz * dz) as f64;
                        if far < best {
                            best = far;
                            pick = Some(j);
                        }
                    }
                }
                water_near[i] = pick;
            }
            for i in 0..count {
                if let Some(j) = water_near[i] {
                    cells[i] = cells[j];
                    lag[i] = lag[j];
                }
            }
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
