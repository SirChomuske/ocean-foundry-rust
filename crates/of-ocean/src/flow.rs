//! A current running through the sea, and what land standing in it does
//! to the water.
//!
//! The current itself is the same everywhere and so leaves no mark on open
//! water: waves ride along with it. Land in its way does three things.
//!
//! - Slack water. Behind the land the current is lost. The slack water is
//!   carried downstream and mixes with the current at its sides, so the
//!   current is back a number of widths of the land farther on, whatever
//!   its speed: a wide obstacle leaves a long wake. Waves that cross it
//!   meet slower water and change with it, as they do at a river mouth,
//!   and the short waves are stilled.
//! - Water piled up. The current is stopped against the face that meets it
//!   and the water stands higher there, by up to the head of the current
//!   (its speed squared over twice gravity). Behind the land it is drawn down.
//! - Standing waves. Like a ship moving through still water, land standing
//!   in moving water leaves a wake that does not move: crests across the
//!   current behind it, one after another a fixed distance apart (two pi
//!   times the speed squared over gravity), and crests fanning out to each
//!   side, all inside a wedge of 19.5 degrees to each side (Kelvin 1887).
//!   Every stretch of shore that faces the current or lies in its lee is
//!   the start of one such wake, and the wakes are added up. The waves
//!   spread as they go and so grow lower, and are lost in the sea a few
//!   crests on. The distance between crests grows with the square of the
//!   speed, so a faster current leaves a longer wake.
//! - White water. The water is churned where the current strikes the land
//!   and behind it. Foam lasts a number of seconds and is carried along
//!   meanwhile, so it reaches as far as the current runs in that time,
//!   which is farther than the waves do.

use crate::build::fill_rows;
use crate::circ::Circ;
use crate::noise::smoothstep;
use crate::terrain::Terrain;
use crate::tuning::FlowTuning;
use of_formats::Progress;
use std::collections::HashMap;
use std::f64::consts::{FRAC_PI_4, TAU};

const GRAVITY: f64 = 9.81;

/// The current at every column of a plot and a one-block rim around it.
pub struct Flow {
    row: usize,
    /// The way the water runs, as a unit vector.
    pub toward: (f64, f64),
    /// Its speed in metres a second.
    pub speed: f64,
    /// How much of the current is lost at a column, 0 to 1.
    slack: Vec<f32>,
    /// How strongly the water is piled against land at a column, 0 to 1.
    piled: Vec<f32>,
    /// How much of the foam made at the land is left at a column, 0 to 1.
    trail: Vec<f32>,
    /// How far the water stands above or below its level, in blocks.
    lift: Vec<f32>,
    most_lost: f64,
    calm: f64,
    /// Where the land narrows the way, how much faster the water runs there.
    room: Option<Circ>,
    /// How much white water the current leaves, 0 to 1, by its speed alone.
    foam: f64,
}

/// One stretch of shore that starts a wake.
struct Source {
    at: (f64, f64),
    /// How strongly it does, positive where the shore faces the current and
    /// negative in its lee.
    weight: f64,
}

/// How something carried along the current fades on its way.
#[derive(Clone, Copy)]
enum Fade {
    /// Over about this many blocks.
    Blocks(f64),
    /// Over about this many widths of the land it came from.
    Widths(f64),
}

/// What is carried along the current from land: 1 at the land, fading
/// with distance and spreading to the sides. `toward` is the way it is carried.
fn carried(row: usize, rows: usize, land: &[bool], toward: (f64, f64), fade: Fade, spread: f64) -> Vec<f32> {
    let mut out = vec![0f32; row * rows];
    // One step is one column along the axis the current mostly follows,
    // and a part of a column across it.
    let along_x = toward.0.abs() >= toward.1.abs();
    let (main, cross) = if along_x { (toward.0, toward.1) } else { (toward.1, toward.0) };
    let slide = cross / main.abs();
    // How far one step is along the current, in blocks.
    let stride = 1.0 / main.abs();
    let (lines, across) = if along_x { (row, rows) } else { (rows, row) };
    let index = |m: usize, c: usize| if along_x { c * row + m } else { m * row + c };
    let sigma = spread.clamp(0.0, 0.33) as f32;
    // The amount on the line before this one and on this one, and with
    // each the width of the land it came from.
    let (mut before, mut line) = (vec![0f32; across], vec![0f32; across]);
    let (mut wide_before, mut wide) = (vec![0f32; across], vec![0f32; across]);
    for step in 0..lines {
        let m = if main > 0.0 { step } else { lines - 1 - step };
        // What reaches each column from the line before: the amount, and
        // the width that came with most of it.
        let reaching = |c: usize| -> (f32, f32) {
            let from = c as f64 - slide;
            let (c0, part) = (from.floor() as i64, (from - from.floor()) as f32);
            let shares = [(1.0 - part) * sigma, (1.0 - part) * (1.0 - 2.0 * sigma) + part * sigma, (1.0 - part) * sigma + part * (1.0 - 2.0 * sigma), part * sigma];
            let (mut amount, mut width) = (0f32, 0f32);
            for (n, share) in shares.iter().enumerate() {
                let k = c0 - 1 + n as i64;
                if k >= 0 && k < across as i64 {
                    amount += share * before[k as usize];
                    width += share * before[k as usize] * wide_before[k as usize];
                }
            }
            (amount, if amount > 0.0 { width / amount } else { 0.0 })
        };
        let mut c = 0;
        while c < across {
            if land[index(m, c)] {
                // A stretch of land across the current: as wide as it is
                // here, or as the land upstream of it was.
                let start = c;
                while c < across && land[index(m, c)] {
                    c += 1;
                }
                let here = ((c - start) as f64 * main.abs()) as f32;
                for k in start..c {
                    line[k] = 1.0;
                    wide[k] = if step == 0 { here } else { here.max(reaching(k).1) };
                }
                continue;
            }
            let (amount, width) = if step == 0 { (0.0, 0.0) } else { reaching(c) };
            let length = match fade {
                Fade::Blocks(blocks) => blocks,
                Fade::Widths(widths) => widths * width.max(1.0) as f64,
            };
            let value = amount * (-stride / length.max(0.5)).exp() as f32;
            line[c] = value;
            wide[c] = width;
            out[index(m, c)] = value;
            c += 1;
        }
        std::mem::swap(&mut before, &mut line);
        std::mem::swap(&mut wide_before, &mut wide);
    }
    out
}

impl Flow {
    /// The current around a piece of land, or none where there is no
    /// current or nothing stands in it. `speed` is in metres a second,
    /// `direction` in degrees as for the waves, and `metres` is the size of
    /// one block.
    pub fn new(
        terrain: &Terrain,
        speed: f64,
        direction: f64,
        metres: f64,
        tuning: &FlowTuning,
        wake_length: f64,
        wake_scale: f64,
        progress: &Progress,
        span: (f32, f32),
    ) -> Result<Option<Self>, String> {
        if speed <= 0.0 {
            return Ok(None);
        }
        let (row, rows) = ((terrain.width + 2) as usize, (terrain.length + 2) as usize);
        let count = row * rows;
        let land: Vec<bool> = (0..count).map(|i| terrain.height((i % row) as i32 - 1, (i / row) as i32 - 1).round() >= 0.0).collect();
        if !land.iter().any(|l| *l) {
            return Ok(None);
        }
        let metres = metres.max(1e-3);
        let (sin, cos) = direction.to_radians().sin_cos();
        let toward = (cos, sin);
        let mut slack = carried(row, rows, &land, toward, Fade::Widths(tuning.slack_widths.max(0.1)), tuning.slack_spread);
        // A shadow whose edge is a block wide draws a straight line across
        // the sea and turns the waves at it into a crease. Spread over a
        // few blocks, both read as the swell they are.
        let soften = ((tuning.slack_widths.max(0.1) * 0.5).round() as usize).clamp(1, 8);
        soften_field(&mut slack, row, rows, soften);
        // A lee is carried off faster than the water fills back in: the
        // current's own mixing takes time, and the further downstream a
        // column sits the more of the current it has back. The deficit
        // falls as (1 + t/tau)^(-2/3), t being the time the water took to
        // reach there, so a faster current draws out the same recovery
        // over more blocks and the wake behind a rock ends instead of
        // reaching the horizon.
        let (away, _) = crate::survey::distances(row, rows, &|i| land[i]);
        let tau = 1.5;
        for (i, s) in slack.iter_mut().enumerate() {
            let t = away[i] as f64 / speed.max(0.05);
            let left = (1.0 + t / tau).powf(-2.0 / 3.0);
            *s = (*s as f64 * left) as f32;
        }
        let piled = carried(row, rows, &land, (-toward.0, -toward.1), Fade::Blocks(tuning.pile_reach), tuning.slack_spread);
        // Foam is carried as far as the current runs while the foam lasts,
        // or as far as "Trail length" stretches that. "Wake size" stretches
        // it both ways at once: drawn out further and fanned out wider.
        let size = wake_scale.clamp(0.25, 4.0);
        let life = speed * tuning.foam_life.max(0.0) * wake_length.clamp(0.0, 4.0) * size / metres;
        let mut trail = carried(row, rows, &land, toward, Fade::Blocks(life), tuning.slack_spread * size);
        // Where the land leaves the water less room than it has further along,
        // the same current runs faster and drags its foam out with it.
        let room = Circ::new(row, rows, &land, &away, toward, tuning);
        if let Some(room) = &room {
            room.draw_out(&mut trail, toward, tuning);
            // Stirred twice: one pass only nudges the edge of a wake.
            room.mix(&mut trail, (-toward.1, toward.0), tuning);
            room.mix(&mut trail, (-toward.1, toward.0), tuning);
        }

        // The head of the current and the distance between its standing
        // crests, both in blocks.
        let head = speed * speed / (2.0 * GRAVITY) / metres;
        let apart = TAU * speed * speed / GRAVITY / metres;

        // Where wakes start: land with water on the side the current comes
        // from, or on the side it leaves by. Shore that lies close together
        // counts as one place.
        let step = (toward.0.round() as i64, toward.1.round() as i64);
        let water = |x: i64, z: i64| x >= 0 && z >= 0 && x < row as i64 && z < rows as i64 && !land[z as usize * row + x as usize];
        let mut edge: Vec<(i64, i64, bool)> = Vec::new();
        for (i, is_land) in land.iter().enumerate() {
            if *is_land {
                let (x, z) = ((i % row) as i64, (i / row) as i64);
                if water(x - step.0, z - step.1) {
                    edge.push((x, z, true));
                }
                if water(x + step.0, z + step.1) {
                    edge.push((x, z, false));
                }
            }
        }
        let most = tuning.sources.clamp(1, 512) as usize;
        let mut size = 4i64;
        let groups = loop {
            let mut groups: HashMap<(i64, i64, bool), (f64, f64, f64)> = HashMap::new();
            for (x, z, facing) in &edge {
                let g = groups.entry((x / size, z / size, *facing)).or_insert((0.0, 0.0, 0.0));
                *g = (g.0 + *x as f64 + 0.5, g.1 + *z as f64 + 0.5, g.2 + 1.0);
            }
            if groups.len() <= most || size > 4096 {
                break groups;
            }
            size *= 2;
        };
        // A stretch as wide as half the distance between crests starts a
        // full wake; a narrower one a weaker wake.
        let full = (0.5 * apart).max(1.0);
        let mut sources: Vec<Source> = groups
            .iter()
            .map(|((_, _, facing), (x, z, n))| Source { at: (x / n, z / n), weight: (n / full).min(1.0) * if *facing { 1.0 } else { -1.0 } })
            .collect();
        // In the same order every time, so the same settings give the same sea.
        sources.sort_by(|a, b| a.at.partial_cmp(&b.at).unwrap_or(std::cmp::Ordering::Equal));
        progress.step(span.0 + (span.1 - span.0) * 0.2).map_err(|p| p.key.to_string())?;

        let mut lift = vec![0f32; count];
        let tall = head * tuning.wave_height.max(0.0);
        let (shortest, full_from) = (tuning.shortest[0].max(1.0), tuning.shortest[1].max(tuning.shortest[0].max(1.0) + 1e-6));
        if tall > 0.02 && apart >= shortest {
            let k0 = TAU / apart;
            let near = 0.5 * apart;
            let lost_over = tuning.wave_reach.max(0.1) * apart;
            let fan_lost_over = tuning.fan_reach.max(0.1) * apart;
            let soft = tuning.wedge_soft.clamp(0.001, 1.0);
            let divergent = tuning.divergent.max(0.0);
            let (land, sources) = (&land, &sources);
            fill_rows(&mut lift, row, progress, (span.0 + (span.1 - span.0) * 0.2, span.1), &|gz, out| {
                for (gx, cell) in out.iter_mut().enumerate() {
                    if land[gz * row + gx] {
                        continue;
                    }
                    let mut sum = 0.0;
                    for s in sources {
                        let (rx, rz) = (gx as f64 + 0.5 - s.at.0, gz as f64 + 0.5 - s.at.1);
                        let down = rx * toward.0 + rz * toward.1;
                        if down <= 0.5 {
                            continue;
                        }
                        let aside = (rz * toward.0 - rx * toward.1) / down;
                        // Inside the wedge the wake keeps to.
                        let inside = 1.0 - 8.0 * aside * aside;
                        if inside <= 0.0 {
                            continue;
                        }
                        let root = inside.sqrt();
                        let reach = down * (1.0 + aside * aside).sqrt();
                        // Tallest along the edges of the wedge. The waves grow
                        // lower as they spread, and are lost in the sea a few crests on.
                        let fading = (near / (reach + near)).sqrt() * (-reach / lost_over).exp();
                        if fading < 0.01 {
                            continue;
                        }
                        let strength = s.weight * fading * inside.powf(-0.25).min(1.8) * smoothstep(0.0, soft, inside);
                        // Waves too short to build from blocks are left out.
                        let built = |length: f64| smoothstep(shortest, full_from, length);
                        // The crests across the current.
                        let lean = -2.0 * aside / (1.0 + root);
                        let cos = 1.0 / (1.0 + lean * lean).sqrt();
                        let mut wave = built(apart * cos * cos) * (k0 * down * (1.0 + aside * lean) / cos + FRAC_PI_4).cos();
                        // The crests that fan out to the sides.
                        if aside.abs() > 1e-6 && divergent > 0.0 {
                            let lean = -(1.0 + root) / (4.0 * aside);
                            let cos = 1.0 / (1.0 + lean * lean).sqrt();
                            let length = apart * cos * cos;
                            if length > shortest {
                                let fan = (-reach / fan_lost_over).exp();
                                wave += divergent * fan * built(length) * (k0 * down * (1.0 + aside * lean) / cos - FRAC_PI_4).cos();
                            }
                        }
                        sum += strength * wave;
                    }
                    *cell = (tall * sum) as f32;
                }
            })?;
        }
        let limit = (head * tuning.lift_most.max(0.0)) as f32;
        let (pile, drop) = ((head * tuning.pile_height.max(0.0)) as f32, (head * tuning.lee_drop.max(0.0)) as f32);
        for i in 0..count {
            lift[i] = (lift[i] + pile * piled[i] - drop * slack[i] * slack[i]).clamp(-limit, limit);
        }
        // Where two shores squeeze the stream it runs faster, and stands a
        // little higher for it.
        if let Some(room) = &room {
            let rise = (head * tuning.squeeze_lift.max(0.0)) as f32;
            // Only where the land is close enough to be squeezing the water:
            // open sea reads a little narrower than the water further along it
            // for no better reason than the way the room is measured.
            let near = (tuning.squeeze_reach.max(1.0) * 3.0) as f32;
            for (i, lift) in lift.iter_mut().enumerate().take(count) {
                // Both fields are kept in the same order, rim and all.
                let gate = 1.0 - smoothstep(0.0, near as f64, away[i] as f64) as f32;
                *lift += rise * (room.at(i) as f32 - 1.0) * gate;
            }
        }
        let from = tuning.foam_from.max(0.01);
        Ok(Some(Self {
            row,
            toward,
            speed,
            slack,
            piled,
            trail,
            lift,
            most_lost: tuning.slack_most.clamp(0.0, 1.0),
            calm: tuning.calm.clamp(0.0, 1.0),
            room,
            foam: tuning.foam.max(0.0) * smoothstep(from, 3.0 * from, speed),
        }))
    }

    fn index(&self, x: i32, z: i32) -> usize {
        (z + 1) as usize * self.row + (x + 1) as usize
    }

    /// How far the water stands above or below its level at a column, in
    /// blocks. Columns run from -1 to the width or length, so the rim is included.
    pub fn lift(&self, x: i32, z: i32) -> f64 {
        self.lift[self.index(x, z)] as f64
    }

    /// How strongly the current strikes land at a column: the water piled
    /// against the face. 0 to 1.
    pub fn strike(&self, x: i32, z: i32) -> f64 {
        self.foam * self.piled[self.index(x, z)] as f64
    }

    /// How much foam the current carries away from the land at a column,
    /// drawn out downstream. 0 to 1.
    pub fn wake(&self, x: i32, z: i32) -> f64 {
        self.foam * self.trail[self.index(x, z)] as f64
    }

    /// How fresh the foam a current carries is at a column: 1 where the land
    /// threw it, falling as it is drawn downstream. 0 to 1.
    pub fn fresh(&self, x: i32, z: i32) -> f64 {
        self.trail[self.index(x, z)] as f64
    }

    /// How much white water the current leaves at a column: where it
    /// strikes land, and carried along behind the land. 0 to 1.
    #[cfg(test)]
    fn churn(&self, x: i32, z: i32) -> f64 {
        self.strike(x, z).max(self.wake(x, z))
    }

    /// How fast the current runs at a column against its open speed: 1 where
    /// nothing narrows the way, more where two shores squeeze it.
    pub fn squeezed(&self, x: i32, z: i32) -> f64 {
        self.room.as_ref().map_or(1.0, |r| r.at(self.index(x, z)))
    }

    /// The speed the current has lost at a place of the plot with its rim,
    /// in metres a second.
    pub(crate) fn lost(&self, place: usize) -> f64 {
        self.speed * self.most_lost * self.slack[place] as f64
    }

    /// How much of their height short waves keep at a place of the plot
    /// with its rim: slack water behind land is smoother than the sea.
    pub(crate) fn ruffled(&self, place: usize) -> f64 {
        1.0 - self.calm * self.slack[place] as f64
    }
}

/// Average a field of the plot with its rim over a square of `radius` blocks
/// to each side, by running sums along each row and then along each column.
fn soften_field(values: &mut [f32], row: usize, rows: usize, radius: usize) {
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
    use crate::terrain::Ground;

    /// A square pillar in the middle of open water.
    fn pillar(size: i32, wide: i32) -> Terrain {
        let (from, to) = ((size - wide) / 2, (size + wide) / 2);
        let height = (0..size * size).map(|i| if (from..to).contains(&(i % size)) && (from..to).contains(&(i / size)) { 30.0 } else { -40.0 }).collect();
        Terrain::from_heights(size, size, -40, height, vec![Ground::Rock; (size * size) as usize])
    }

    fn flow(speed: f64, direction: f64) -> Option<Flow> {
        Flow::new(&pillar(240, 16), speed, direction, 0.25, &FlowTuning::default(), 1.0, 1.0, &Progress::none(), (0.0, 1.0)).unwrap()
    }

    #[test]
    fn no_current_or_no_land_leaves_the_sea_alone() {
        assert!(flow(0.0, 0.0).is_none());
        let open = Terrain::from_heights(64, 64, -40, vec![-40.0; 64 * 64], vec![Ground::Rock; 64 * 64]);
        assert!(Flow::new(&open, 5.0, 0.0, 0.25, &FlowTuning::default(), 1.0, 1.0, &Progress::none(), (0.0, 1.0)).unwrap().is_none());
    }

    /// Two long walls with a straight channel between them.
    fn channel(size: i32, mouth: (i32, i32)) -> Terrain {
        let (a, b) = mouth;
        let height: Vec<f32> = (0..size * size)
            .map(|i| {
                let (x, z) = (i % size, i / size);
                let wall = (40..160).contains(&x) && (((a - 6)..a).contains(&z) || (b..b + 6).contains(&z));
                if wall { 30.0 } else { -40.0 }
            })
            .collect();
        Terrain::from_heights(size, size, -40, height, vec![Ground::Rock; (size * size) as usize])
    }

    #[test]
    fn water_stands_higher_where_two_shores_squeeze_it() {
        let f = Flow::new(&channel(200, (90, 110)), 5.0, 0.0, 0.25, &FlowTuning::default(), 1.0, 1.0, &Progress::none(), (0.0, 1.0)).unwrap().unwrap();
        let (squeezed, open) = (f.lift(100, 100), f.lift(20, 20));
        println!("lift in the narrows {squeezed:.2}, in the open {open:.2}, speed {:.2}", f.squeezed(100, 100));
        assert!(f.squeezed(100, 100) > 1.05, "the narrows should run faster than the open sea");
        assert!(squeezed > open + 0.02, "the narrows should stand a little higher than the open sea");
    }

    #[test]
    fn water_piles_up_in_front_of_land_and_is_slack_behind_it() {
        let f = flow(5.0, 0.0).unwrap();
        let line: Vec<String> = (60..240).step_by(4).map(|x| format!("{x}:{:.1}", f.lift(x, 120))).collect();
        println!("lift along the current: {}", line.join(" "));
        let side: Vec<String> = (60..180).step_by(4).map(|z| format!("{z}:{:.1}", f.lift(170, z))).collect();
        println!("lift across the wake 42 blocks behind: {}", side.join(" "));
        let lost: Vec<String> = (128..240).step_by(8).map(|x| format!("{x}:{:.1}", f.lost(121 * 242 + x as usize + 1))).collect();
        println!("current lost behind the land: {}", lost.join(" "));
        let foam: Vec<String> = (128..240).step_by(8).map(|x| format!("{x}:{:.2}", f.churn(x, 120))).collect();
        println!("white water behind the land: {}", foam.join(" "));
        // The current runs east: the west face is struck, the east side is sheltered.
        assert!(f.lift(110, 120) > 1.5, "{}", f.lift(110, 120));
        assert!(f.churn(110, 120) > 0.5 && f.churn(130, 120) > 0.5);
        // The slack shadow is spread over a few blocks so it has no seam, so
        // it peaks lower than a hard one did: ten blocks behind the land the
        // current is still about two and a half times as far down.
        assert!(f.lost(121 * 242 + 131) > 1.8, "{}", f.lost(121 * 242 + 131));
        assert!(f.lost(121 * 242 + 100) == 0.0);
        // Far to the side nothing has changed.
        assert_eq!(f.lift(120, 10), 0.0);
        assert_eq!(f.churn(120, 10), 0.0);
        // The slack water closes within a number of widths of the land, and
        // the foam is carried farther than that.
        assert!(f.lost(121 * 242 + 235) < 0.5 * f.lost(121 * 242 + 131));
        assert!(f.churn(234, 120) > 0.1);
        // A wider pillar leaves a longer wake.
        let wide = Flow::new(&pillar(240, 32), 5.0, 0.0, 0.25, &FlowTuning::default(), 1.0, 1.0, &Progress::none(), (0.0, 1.0)).unwrap().unwrap();
        assert!(wide.lost(121 * 242 + 235) > 1.3 * f.lost(121 * 242 + 235));
        // A faster current carries its foam farther.
        let fast = flow(8.0, 0.0).unwrap();
        assert!(fast.churn(234, 120) > f.churn(234, 120));
        // The wake lies behind the land whichever way the current runs.
        let north = flow(5.0, 270.0).unwrap();
        assert!(north.lost(100 * 242 + 121) > 1.8 && north.lost(140 * 242 + 121) == 0.0);
    }

    #[test]
    fn the_trail_recovers_and_its_length_can_be_set() {
        let tuned = |wake_length: f64| {
            Flow::new(&pillar(240, 16), 5.0, 0.0, 0.25, &FlowTuning::default(), wake_length, 1.0, &Progress::none(), (0.0, 1.0)).unwrap().unwrap()
        };
        let (short, long) = (tuned(0.1), tuned(3.0));
        // The foam is thrown up where the water strikes the land, and is
        // carried away behind it rather than in front of it.
        assert!(short.strike(110, 120) > 0.5, "{}", short.strike(110, 120));
        assert!(short.wake(110, 120) < 0.2, "{}", short.wake(110, 120));
        assert!(short.wake(140, 120) > short.strike(140, 120));
        // A short trail is used up quickly, so the sea recovers behind land.
        assert!(short.wake(234, 120) < 0.05, "{}", short.wake(234, 120));
        // Stretching the trail carries foam farther, and is all that does.
        assert!(long.wake(234, 120) > 3.0 * short.wake(234, 120));
        assert_eq!(short.strike(234, 120), long.strike(234, 120));
    }

    #[test]
    fn the_wake_grows_with_its_size() {
        let sized = |wake_scale: f64| {
            Flow::new(&pillar(240, 16), 5.0, 0.0, 0.25, &FlowTuning::default(), 1.0, wake_scale, &Progress::none(), (0.0, 1.0)).unwrap().unwrap()
        };
        let (small, normal, big) = (sized(0.5), sized(1.0), sized(3.0));
        // Size multiplies the trail length, so foam reaches farther.
        assert!(big.wake(234, 120) > 1.1 * normal.wake(234, 120), "{} {}", big.wake(234, 120), normal.wake(234, 120));
        assert!(small.wake(234, 120) < normal.wake(234, 120), "{}", small.wake(234, 120));
        // It also fans the trail out, so it is wider a little way behind.
        assert!(big.wake(170, 100) > 2.0 * normal.wake(170, 100), "{} {}", big.wake(170, 100), normal.wake(170, 100));
        // And it leaves the waves themselves alone.
        assert_eq!(small.lift(110, 120), big.lift(110, 120));
        assert_eq!(small.strike(110, 120), big.strike(110, 120));
    }
}
