//! What the current does where the way narrows.
//!
//! The sea has one current, running one way across the whole plot. Land does
//! not stop it: where two shores leave less room than the water is used to,
//! the same water has to pass through a smaller gap, so it goes faster, and
//! the faster it goes the further the foam it carries is drawn out. That is
//! the whole of what this module reads - how much room the water has now
//! against how much room it has further along - and what it does with it:
//! a speed for each column, and foam drawn out down the narrow part.
//!
//! It is not a flow solver. The room is measured from the distance to land
//! on both sides, so a bay, a strait between two rocks and the mouth of a
//! river all read the same way: narrower than what lies up and down the
//! current, therefore faster.

use crate::tuning::FlowTuning;

/// The room the water has, and the speed it therefore runs at.
#[derive(Debug, Clone)]
pub(crate) struct Circ {
    row: usize,
    rows: usize,
    speed: Vec<f32>,
}

impl Circ {
    /// Read the plot's land and the direction the current runs. `None` when
    /// the tuning turns the squeeze off, or when there is no land to make
    /// any gap at all.
    pub(crate) fn new(row: usize, rows: usize, land: &[bool], clearance: &[f32], toward: (f64, f64), tuning: &FlowTuning) -> Option<Self> {
        let strength = tuning.squeeze.max(0.0);
        if (strength <= 0.0 && tuning.stagnation <= 0.0) || !land.iter().any(|l| *l) {
            return None;
        }
        let reach = tuning.squeeze_reach.max(1.0).round();
        let stagnation = tuning.stagnation.clamp(0.0, 1.0);
        // The most the current can be made to run faster than open water.
        let most = 1.5;
        let (nx, nz) = (-toward.1, toward.0);
        let at = |x: f64, z: f64| -> f64 {
            let (xi, zi) = (x.round() as i64, z.round() as i64);
            if xi < 0 || zi < 0 || xi >= row as i64 || zi >= rows as i64 {
                // Off the plot is open water: it only ever widens the sea.
                return f64::INFINITY;
            }
            clearance[zi as usize * row + xi as usize] as f64
        };
        // How far the water reaches to its sides at a column: the offset it
        // was read at, plus the distance from there to the land. That is the
        // half-width of the way it has now, and by the same reading further
        // up and down the current.
        let half = |x: f64, z: f64, offset: f64| -> f64 { offset + at(x, z) };
        let mut speed = vec![1f32; row * rows];
        for i in 0..row * rows {
            if land[i] {
                continue;
            }
            let (x, z) = ((i % row) as f64, (i / row) as f64);
            let sides = half(x + nx * reach, z + nz * reach, reach) + half(x - nx * reach, z - nz * reach, reach);
            // What the water is used to: the widest room the same channel has
            // within a few lengths of the gap. A strait only reads as narrow
            // against the sea it opens into, which can be many blocks away.
            let mut open: f64 = 0.0;
            for step in 1..=3 {
                let along = 2.0 * reach * (step * step) as f64;
                let wide = half(x + toward.0 * along, z + toward.1 * along, along) + half(x - toward.0 * along, z - toward.1 * along, along);
                if wide.is_finite() && wide > open {
                    open = wide;
                }
            }
            if !sides.is_finite() || !open.is_finite() || sides <= 1e-6 {
                continue;
            }
            // Where the water runs into a face the way ahead is much narrower
            // than the way it came from: the same water has nowhere to go but
            // up and aside, so the current is stopped there and piles instead
            // of running on.
            let mut blocked = 0.0;
            if stagnation > 0.0 {
                // How soon the land closes in along the way the water runs:
                // a face close by leaves the current nowhere to go.
                let mut soon = f64::INFINITY;
                let mut t = 1.0;
                while t <= reach {
                    let room = at(x + toward.0 * t, z + toward.1 * t);
                    if room < soon {
                        soon = room;
                    }
                    t += 1.0;
                }
                // Only land closing in counts: a gap the water is already in
                // narrows the way without stopping it.
                let here = at(x, z);
                // Land has to be within reach for the water to be stopped by
                // it; a shore a long way off narrows the sea without stopping
                // anything.
                if soon <= reach && soon.is_finite() && here.is_finite() {
                    blocked = ((here - soon) / here.max(1.0)).clamp(0.0, 1.0);
                }
            }
            // The same water through a smaller gap goes faster in the same
            // measure: the way is `sides` wide here against `open` elsewhere.
            // It does not grow without end - a shore is not a nozzle, and a
            // current that ran ten times faster at every gap would look wrong.
            // A face the water runs into is not a gap either: the open water
            // beyond it is the sea the current never reaches, so it does not
            // count as room.
            let speed_up = ((open / sides).clamp(1.0, 1.0 + most) - 1.0) * (1.0 - blocked);
            let mut runs = 1.0 + strength * speed_up;
            runs *= 1.0 - stagnation * blocked;
            speed[i] = runs as f32;
        }
        // The room is measured in whole blocks, so its edges would otherwise
        // show as one-block steps in the speed.
        soften(&mut speed, row, rows, 2);
        Some(Self { row, rows, speed })
    }

    /// The speed reading at a place in the field's own order, which carries a
    /// one-block rim as the flow's fields do. 1 where the water has all the
    /// room it wants.
    pub(crate) fn at(&self, i: usize) -> f64 {
        self.speed.get(i).copied().unwrap_or(1.0) as f64
    }

    /// How fast the current runs at a column, against its open speed: 1 where
    /// the water has all the room it wants.
    #[cfg(test)]
    pub(crate) fn speed(&self, x: i32, z: i32) -> f64 {
        if x < 0 || z < 0 || x as usize >= self.row || z as usize >= self.rows {
            return 1.0;
        }
        self.speed[z as usize * self.row + x as usize] as f64
    }

    /// Draw the foam out where the current is squeezed: the water carries it
    /// further in the same time, so the same foam covers a longer stretch and
    /// reads as streaks along the way through. Foam already there is kept.
    pub(crate) fn draw_out(&self, trail: &mut [f32], toward: (f64, f64), tuning: &FlowTuning) {
        let stretch = tuning.foam_stretch.clamp(0.0, 1.0);
        if stretch <= 0.0 {
            return;
        }
        let extra: Vec<f32> = {
            let mut out = vec![0f32; trail.len()];
            let taps = 5;
            let gap = 2.0;
            for i in 0..trail.len() {
                let fast = self.speed[i] - 1.0;
                if fast <= 0.01 {
                    continue;
                }
                let (x, z) = ((i % self.row) as f64, (i / self.row) as f64);
                let mut sum = 0.0f64;
                let mut weight = 0.8f64;
                for tap in 1..=taps {
                    let (sx, sz) = (x - toward.0 * gap * tap as f64, z - toward.1 * gap * tap as f64);
                    let (xi, zi) = (sx.round() as i64, sz.round() as i64);
                    if xi < 0 || zi < 0 || xi >= self.row as i64 || zi >= self.rows as i64 {
                        break;
                    }
                    sum += weight * trail[zi as usize * self.row + xi as usize] as f64;
                    weight *= 0.75;
                }
                out[i] = (fast as f64 * sum * stretch) as f32;
            }
            out
        };
        for (t, e) in trail.iter_mut().zip(extra) {
            *t = t.max(e);
        }
    }
}

impl Circ {
    /// Spread the foam across the way it is running, further the further it
    /// has come: water behind an obstacle mixes sideways into the stream
    /// beside it, so a wake widens downwind instead of staying the width of
    /// what made it. Along the current the foam is left where it was - that
    /// way it is drawn out, this way it is stirred.
    pub(crate) fn mix(&self, trail: &mut [f32], across: (f64, f64), tuning: &FlowTuning) {
        let mix = tuning.mix.clamp(0.0, 1.0);
        if mix <= 0.0 || trail.is_empty() {
            return;
        }
        let reach = tuning.mix_spread.max(0.0).round() as i64;
        if reach <= 0 {
            return;
        }
        let spread: Vec<f32> = {
            let mut out = Vec::with_capacity(trail.len());
            let mut weight = vec![0.0f64];
            weight.resize(2 * reach as usize + 1, 0.0);
            for (k, w) in weight.iter_mut().enumerate() {
                let d = k as i64 - reach;
                *w = 1.0 / (1.0 + d.abs() as f64);
            }
            for i in 0..trail.len() {
                let (x, z) = ((i % self.row) as f64, (i / self.row) as f64);
                let mut sum = 0.0f64;
                let mut used = 0.0f64;
                for (k, w) in weight.iter().enumerate() {
                    let d = k as i64 - reach;
                    let (sx, sz) = (x + across.0 * d as f64, z + across.1 * d as f64);
                    let (xi, zi) = (sx.round() as i64, sz.round() as i64);
                    if xi < 0 || zi < 0 || xi >= self.row as i64 || zi >= self.rows as i64 {
                        continue;
                    }
                    sum += w * trail[zi as usize * self.row + xi as usize] as f64;
                    used += w;
                }
                let here = trail[i] as f64;
                let there = if used > 1e-9 { sum / used } else { here };
                out.push((here + mix * (there - here)) as f32);
            }
            out
        };
        trail.copy_from_slice(&spread);
    }
}

/// Average a field over a square of `radius` blocks to each side, by running
/// sums along each row and then along each column.
fn soften(values: &mut [f32], row: usize, rows: usize, radius: usize) {
    if radius == 0 || values.is_empty() {
        return;
    }
    let mut line = vec![0f32; row.max(rows)];
    for z in 0..rows {
        let base = z * row;
        let mut sum = 0.0f32;
        for x in 0..row {
            sum += values[base + x];
            line[x] = sum;
        }
        for x in 0..row {
            let lo = x.saturating_sub(radius);
            let hi = (x + radius).min(row - 1);
            let before = if lo > 0 { line[lo - 1] } else { 0.0 };
            values[base + x] = (line[hi] - before) / (hi - lo + 1) as f32;
        }
    }
    for x in 0..row {
        let mut sum = 0.0f32;
        for z in 0..rows {
            sum += values[z * row + x];
            line[z] = sum;
        }
        for z in 0..rows {
            let lo = z.saturating_sub(radius);
            let hi = (z + radius).min(rows - 1);
            let before = if lo > 0 { line[lo - 1] } else { 0.0 };
            values[z * row + x] = (line[hi] - before) / (hi - lo + 1) as f32;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tuning::FlowTuning;

    /// A plot of water with a wall across it and a gap in the middle, so the
    /// current has to pass through a strait.
    fn strait(width: usize, length: usize, gap: usize) -> Vec<bool> {
        let mut land = vec![false; width * length];
        let middle = width / 2;
        for z in 0..length / 2 {
            for x in 0..width {
                // Two walls meeting at the rim, leaving `gap` open in the middle.
                if x + gap / 2 < middle || x > middle + gap / 2 {
                    land[z * width + x] = true;
                }
            }
        }
        land
    }

    /// A plot of water with a wall straight across it, so the current runs
    /// into the face.
    fn wall(width: usize, length: usize, at: usize) -> Vec<bool> {
        let mut land = vec![false; width * length];
        for z in at..(at + 2).min(length) {
            for x in 0..width {
                land[z * width + x] = true;
            }
        }
        land
    }

    #[test]
    fn the_current_is_stopped_where_it_runs_into_a_face() {
        let (row, rows) = (64usize, 64usize);
        let land = wall(row, rows, 32);
        let (clearance, _) = crate::survey::distances(row, rows, &|i| land[i]);
        let circ = Circ::new(row, rows, &land, &clearance, (0.0, 1.0), &FlowTuning::default()).expect("a wall stops the current");
        // Just up the current from the face, and well away from it.
        let facing = circ.speed(31, 30);
        let open = circ.speed(31, 6);
        assert!(facing < 0.95, "the current is stopped at the face: {facing:.2}");
        assert!(open > facing + 0.2, "open water runs on: {open:.2} against {facing:.2}");
    }

    #[test]
    fn the_current_runs_faster_through_a_narrow_gap() {
        let (row, rows) = (64usize, 64usize);
        let land = strait(row, rows, 8);
        let toward = (0.0, 1.0);
        let tuning = FlowTuning::default();
        let (clearance, _) = crate::survey::distances(row, rows, &|i| land[i]);
        let circ = Circ::new(row, rows, &land, &clearance, toward, &tuning).expect("a strait squeezes the current");
        // In the gap, just downstream of the walls.
        let gap = circ.speed(32 - 1, 40);
        // Out in the open sea on the same row, far from the walls.
        let open = circ.speed(4, 40);
        assert!(gap > 1.15, "the gap runs faster than open water: {gap:.2} against {open:.2}");
        assert!((open - 1.0).abs() < 0.06, "open water is left alone: {open:.2}");
    }

    #[test]
    fn a_wide_gap_squeezes_far_less_than_a_narrow_one() {
        // Pinned, so the shape of the answer does not move when the default
        // strength is tuned: this test is about the ratio, not the amount.
        let tuning = FlowTuning { squeeze: 0.6, ..FlowTuning::default() };
        let fastest = |gap: usize| {
            let (row, rows) = (64usize, 64usize);
            let land = strait(row, rows, gap);
            let (clearance, _) = crate::survey::distances(row, rows, &|i| land[i]);
            let circ = Circ::new(row, rows, &land, &clearance, (0.0, 1.0), &tuning);
            // Read in the middle of the gap itself.
            circ.map_or(1.0, |c| c.speed(31, 30))
        };
        let (narrow, wide) = (fastest(8), fastest(48));
        assert!(wide < 1.45, "a gap almost the width of the sea squeezes it little: {wide:.3}");
        assert!(narrow > wide + 0.2, "the narrow gap is the faster one: {narrow:.3} against {wide:.3}");
    }

    #[test]
    fn the_wake_widens_across_the_current() {
        let (row, rows) = (64usize, 64usize);
        let land = wall(row, rows, 32);
        let (clearance, _) = crate::survey::distances(row, rows, &|i| land[i]);
        let tuning = FlowTuning::default();
        let circ = Circ::new(row, rows, &land, &clearance, (0.0, 1.0), &tuning).expect("a wall makes a wake");
        // Foam laid down in one column just below the wall.
        let mut trail = vec![0f32; row * rows];
        for z in 34..60 {
            trail[z * row + 32] = 1.0;
        }
        let before_beside = trail[40 * row + 34];
        // Two passes, as the flow makes: mixing is spread, not smeared once.
        circ.mix(&mut trail, (1.0, 0.0), &tuning);
        circ.mix(&mut trail, (1.0, 0.0), &tuning);
        let after_beside = trail[40 * row + 34];
        assert!(before_beside < 0.01, "the column beside starts empty: {before_beside:.3}");
        assert!(after_beside > 0.05, "the foam spreads beside the column: {after_beside:.3}");
        assert!(after_beside > trail[40 * row + 40], "it thins the further it goes");
        assert!(trail[40 * row + 32] < 1.0, "the column it came from gives some up");
        assert!(trail[40 * row + 32] > after_beside, "and keeps most of it");
    }

    #[test]
    fn foam_is_drawn_out_down_the_narrow_part() {
        let (row, rows) = (64usize, 64usize);
        let land = strait(row, rows, 8);
        let toward = (0.0, 1.0);
        let tuning = FlowTuning::default();
        let (clearance, _) = crate::survey::distances(row, rows, &|i| land[i]);
        let circ = Circ::new(row, rows, &land, &clearance, toward, &tuning).expect("a strait");
        // A short patch of foam just downstream of the gap.
        let mut trail = vec![0f32; row * rows];
        for z in 40..44 {
            for x in 30..34 {
                trail[z * row + x] = 1.0;
            }
        }
        let before: f32 = trail.iter().sum();
        circ.draw_out(&mut trail, toward, &tuning);
        let after: f32 = trail.iter().sum();
        assert!(after > before * 1.2, "foam is drawn out: {before:.1} to {after:.1}");
        // It reaches downstream of where it was made ...
        assert!(trail[46 * row + 31] > 0.0, "foam runs on down the current");
        // ... and does not run against it.
        assert_eq!(trail[36 * row + 31], 0.0, "foam does not run against the current");
    }
}
