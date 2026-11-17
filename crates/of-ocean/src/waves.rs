//! The shape of the sea: many small wave components drawn from a measured
//! ocean wave spectrum and added together, evaluated in world coordinates
//! with absolute heights in blocks. Asking for a smaller area never rescales
//! or moves anything.
//!
//! The method follows how oceanographers and film studios synthesise a sea:
//!
//! - How much energy each wave frequency carries follows the JONSWAP
//!   spectrum (Hasselmann et al., 1973): a sharp peak at the dominant waves
//!   and a tail of shorter waves falling off with the fifth power.
//! - How far the directions of the waves fan out follows the cos^2s
//!   spreading of Mitsuyasu and Goda: narrowest at the peak, wider for the
//!   shorter waves.
//! - Each component gets a random phase. Their sum has the groups of taller
//!   and lower waves and the crests of limited length that real seas have,
//!   and it never repeats, because no two components share a wavelength.
//! - Crests are sharpened by moving the water sideways toward them, as in a
//!   trochoidal (Gerstner) wave and Tessendorf's "choppy waves". How hard
//!   the water is squeezed together at a crest also tells where a wave breaks.
//! - Every component moves at its own deep-water speed, so the sea can be
//!   asked what it looked like a moment ago. That is how foam knows where
//!   waves were breaking just before.

use crate::noise::{hash, smoothstep};
use crate::tuning::WaveTuning;
use std::f64::consts::{PI, TAU};

#[derive(Debug, Clone, PartialEq)]
pub struct WaveSettings {
    /// Trough to crest of the larger waves of the open sea, in blocks. Most
    /// waves are smaller and the rare tallest is about half as tall again.
    pub height: f64,
    /// Distance between the main crests, in blocks.
    pub peak_distance: f64,
    /// Direction the waves travel, degrees. 0 is +X, 90 is +Z.
    pub direction: f64,
    pub irregularity: f64,
    pub swell_detail: f64,
    pub crest_sharpness: f64,
    pub seed: u32,
    /// The moment that is built: how many main waves have passed since the
    /// sea of moment 0. Every wave moves on at its own speed.
    pub moment: f64,
    /// A swell from a storm far away, under the waves the wind raises here:
    /// its height (trough to crest of its larger waves), the distance
    /// between its crests, both in blocks, and the direction it travels.
    pub swell_height: f64,
    pub swell_length: f64,
    pub swell_direction: f64,
}

struct Wave {
    dir: (f64, f64),
    /// Radians of phase per block.
    k: f64,
    /// Half its height, in blocks.
    amp: f64,
    phase: f64,
    /// Radians of phase it advances while a main wave passes once.
    rate: f64,
    /// One of the long waves of the main sea, rather than the short waves on top.
    long: bool,
    /// How much of this wave is left once the sea is smoothed for colouring.
    tone: f64,
}

/// What land has done to the waves at one place.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Near {
    /// Blocks the waves have fallen behind, from slowing over shallow water
    /// and from having to go around land.
    pub lag: f64,
    /// Share of their height the long waves have kept.
    pub long: f64,
    /// Share of their height the short waves have kept.
    pub short: f64,
    /// How much closer together the crests are than in deep water.
    pub squeeze: f64,
}

impl Near {
    pub const OPEN: Near = Near { lag: 0.0, long: 1.0, short: 1.0, squeeze: 1.0 };
}

/// The water at one place.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sample {
    /// Blocks above the resting level.
    pub height: f64,
    /// How steep and squeezed together the water is here; see `WaveField::steepness`.
    pub steep: f64,
    /// The height with the short waves smoothed away. The colour of the
    /// water follows this, so it shows the swell and not every wavelet.
    pub smooth: f64,
}

pub struct WaveField {
    waves: Vec<Wave>,
    /// How far the water moves toward the crests, as a share of a full trochoid.
    chop: f64,
    lowest: f64,
    highest: f64,
    steepest: f64,
    peak_length: f64,
    heading: (f64, f64),
    spread: f64,
}

/// The JONSWAP spectrum against frequency relative to the peak.
fn jonswap(x: f64, enhancement: f64) -> f64 {
    if x <= 0.0 {
        return 0.0;
    }
    let width = if x <= 1.0 { 0.07 } else { 0.09 };
    let bump = (-(x - 1.0) * (x - 1.0) / (2.0 * width * width)).exp();
    x.powi(-5) * (-1.25 * x.powi(-4)).exp() * enhancement.max(1.0).powf(bump)
}

fn unit(a: i32, b: i32, seed: u32) -> f64 {
    hash(a, b, seed) as f64 / (u32::MAX as f64 + 1.0)
}

/// Split a spectrum between `lo` and `hi` into `count` bands and pick one
/// frequency in each. Returns each frequency with the energy of its band.
/// Bands are narrow where the spectrum is strong, so the peak is finely
/// resolved, and no two frequencies are evenly spaced, so the sum never repeats.
fn bands(spectrum: &dyn Fn(f64) -> f64, lo: f64, hi: f64, count: usize, seed: u32) -> Vec<(f64, f64)> {
    const STEPS: usize = 4096;
    if count == 0 || hi <= lo {
        return Vec::new();
    }
    let dx = (hi - lo) / STEPS as f64;
    let (mut share, mut energy) = (vec![0.0; STEPS + 1], vec![0.0; STEPS + 1]);
    for i in 0..STEPS {
        let s = spectrum(lo + (i as f64 + 0.5) * dx).max(0.0);
        share[i + 1] = share[i] + s.powf(0.6);
        energy[i + 1] = energy[i] + s * dx;
    }
    let total = share[STEPS];
    if total <= 0.0 {
        return Vec::new();
    }
    // Where along the axis a share of the total lies, counted in steps.
    let place = |part: f64| -> f64 {
        let want = part.clamp(0.0, 1.0) * total;
        let i = share.partition_point(|s| *s < want).clamp(1, STEPS);
        let span = share[i] - share[i - 1];
        (i - 1) as f64 + if span > 0.0 { ((want - share[i - 1]) / span).clamp(0.0, 1.0) } else { 0.5 }
    };
    let energy_to = |pos: f64| -> f64 {
        let i = (pos.floor() as usize).min(STEPS - 1);
        energy[i] + (energy[i + 1] - energy[i]) * (pos - i as f64)
    };
    (0..count)
        .map(|b| {
            let (from, to) = (place(b as f64 / count as f64), place((b + 1) as f64 / count as f64));
            let pick = place((b as f64 + 0.15 + 0.7 * unit(b as i32, 3, seed)) / count as f64);
            (lo + pick * dx, energy_to(to) - energy_to(from))
        })
        .filter(|(_, e)| *e > 0.0)
        .collect()
}

/// The angle away from the main direction below which a share `part` of a
/// cos^2s spreading lies.
fn spread_angle(s: f64, part: f64) -> f64 {
    const STEPS: usize = 720;
    let weight = |i: usize| {
        let angle = -PI + (i as f64 + 0.5) * TAU / STEPS as f64;
        (angle * 0.5).cos().abs().powf(2.0 * s)
    };
    let total: f64 = (0..STEPS).map(weight).sum();
    let (mut run, want) = (0.0, part.clamp(0.0, 1.0) * total);
    for i in 0..STEPS {
        let w = weight(i);
        if run + w >= want {
            return -PI + (i as f64 + if w > 0.0 { (want - run) / w } else { 0.5 }) * TAU / STEPS as f64;
        }
        run += w;
    }
    0.0
}

impl WaveField {
    pub fn new(s: &WaveSettings) -> Self {
        Self::with(s, &WaveTuning::default())
    }

    pub fn with(s: &WaveSettings, tuning: &WaveTuning) -> Self {
        let irregularity = s.irregularity.clamp(0.0, 1.0);
        let detail = s.swell_detail.clamp(0.0, 1.0);
        let peak_length = s.peak_distance.max(2.0);
        let peak_k = TAU / peak_length;

        // Frequencies are measured against the peak. In deep water the
        // wavelength goes with one over the frequency squared.
        let shortest = tuning.shortest_length.max(1.0);
        let top = (peak_length / shortest).sqrt().clamp(1.05, 4.0);
        let tail_from = tuning.tail_from.clamp(1.05, 3.0);
        let enhancement = tuning.peak_enhancement.max(1.0);
        // A regular swell keeps its energy close to the peak; an irregular
        // sea spreads it over the whole width of the spectrum.
        let narrowing = 1.0 + tuning.narrowing_at_calm.max(0.0) * (1.0 - irregularity).powi(3);
        let split = |x: f64| smoothstep(tail_from * 0.9, tail_from * 1.2, x);
        let peak = |x: f64| jonswap(x, enhancement).powf(narrowing) * (1.0 - split(x));
        let tail = |x: f64| jonswap(x, enhancement) * split(x);

        let count = (tuning.components as usize).clamp(4, 256);
        let tail_energy = if top > tail_from { (tuning.tail_energy.max(0.0) * detail).min(0.9) } else { 0.0 };
        let tail_count =
            if tail_energy > 0.0 { ((count as f64 * tuning.tail_components.clamp(0.0, 0.9)).round() as usize).clamp(1, count - 2) } else { 0 };
        let main = bands(&peak, 0.6, (tail_from * 1.2).min(top).max(1.04), count - tail_count, s.seed ^ 0x51ED_270B);
        let riding = bands(&tail, tail_from * 0.9, top, tail_count, s.seed ^ 0x2F0B_3A91);
        let main_total: f64 = main.iter().map(|b| b.1).sum::<f64>().max(1e-300);
        let riding_total: f64 = riding.iter().map(|b| b.1).sum::<f64>().max(1e-300);
        let tail_energy = if riding.is_empty() { 0.0 } else { tail_energy };

        // Directions: narrow for a regular swell, wide for a confused sea,
        // and always wider for the shorter waves.
        let (calm, wild) = (tuning.spread_at_calm.max(1.0), tuning.spread_at_wild.max(1.0));
        let spread_peak = (calm.ln() + (wild.ln() - calm.ln()) * irregularity).exp();
        let heading = s.direction.to_radians();
        let mut waves = Vec::with_capacity(count);
        // Colour is read over an area, not block by block: waves much
        // shorter than this are averaged out of it.
        let footprint = (tuning.tone_smoothing.max(0.0) * peak_length).max(tuning.tone_smoothing_least.max(0.0));
        let all = main.iter().map(|b| (b, (1.0 - tail_energy) / main_total, true)).chain(riding.iter().map(|b| (b, tail_energy / riding_total, false)));
        for (i, (&(x, energy), scale, long)) in all.enumerate() {
            let spread = (spread_peak * if x < 1.0 { x.powi(5) } else { x.powf(-2.5) }).max(1.0);
            // Spread evenly through the fan, in an order that keeps waves of
            // nearly the same length well apart in direction.
            let part = (0.5 + (i as f64 + 1.0) * 0.618_033_988_75 + 0.2 * unit(i as i32, 5, s.seed)).fract();
            let angle = heading + spread_angle(spread, 0.02 + 0.96 * part);
            waves.push(Wave {
                dir: (angle.cos(), angle.sin()),
                k: peak_k * x * x,
                amp: (2.0 * energy * scale).sqrt(),
                phase: unit(i as i32, 17, s.seed) * TAU - TAU * x * s.moment,
                rate: TAU * x,
                long,
                tone: (-0.5 * (peak_k * x * x * footprint).powi(2)).exp(),
            });
        }

        let mut field = Self {
            waves,
            chop: 0.0,
            lowest: f64::MIN,
            highest: f64::MAX,
            steepest: 0.0,
            peak_length,
            heading: (heading.cos(), heading.sin()),
            spread: spread_peak,
        };
        // Scale the sea so its larger waves are as tall as asked for. Wave
        // heights in a random sea are spread around its standard deviation:
        // the highest third average 4 of them, the highest tenth 5.1.
        let deviation = (field.waves.iter().map(|w| w.amp * w.amp).sum::<f64>() * 0.5).sqrt();
        let gain = if deviation > 1e-300 { s.height.max(0.0) / (tuning.height_in_deviations.max(0.1) * deviation) } else { 0.0 };
        for w in &mut field.waves {
            w.amp *= gain;
        }
        // The swell: a few long, even waves, nearly in step and nearly in
        // one direction. It keeps its own height whatever the wind sea does.
        if s.swell_height > 0.0 {
            const PARTS: usize = 5;
            let deviation = s.swell_height / tuning.height_in_deviations.max(0.1);
            for i in 0..PARTS {
                let part = i as f64 / (PARTS - 1) as f64 - 0.5;
                let k = TAU / (s.swell_length.max(4.0) * (1.0 + 0.16 * part));
                let angle = s.swell_direction.to_radians() + 0.14 * (unit(i as i32, 23, s.seed) - 0.5);
                let x = (k / peak_k).sqrt();
                field.waves.push(Wave {
                    dir: (angle.cos(), angle.sin()),
                    k,
                    amp: deviation * (2.0 / PARTS as f64).sqrt(),
                    phase: unit(i as i32, 29, s.seed) * TAU - TAU * x * s.moment,
                    rate: TAU * x,
                    long: true,
                    tone: (-0.5 * (k * footprint).powi(2)).exp(),
                });
            }
        }
        let (low, high, steepest) = field.measure(s.seed);
        // A little room beyond what was measured; heights are held inside it.
        (field.lowest, field.highest) = (low * 1.25 - 1e-9, high * 1.25 + 1e-9);
        field.steepest = steepest;
        // Sharpen the crests, at most until the steepest ones are nearly cusps.
        let wanted = s.crest_sharpness.clamp(0.0, 1.0) * tuning.chop_most.max(0.0);
        field.chop = if steepest > 1e-9 { wanted.min(tuning.cusp_limit.clamp(0.0, 0.95) / steepest) } else { 0.0 };
        field
    }

    /// Lowest and highest water and the greatest steepness over a fixed
    /// stretch of the world, ten main wavelengths on a side. The stretch
    /// does not depend on the area being built, so the size of a build
    /// never changes its waves.
    fn measure(&self, seed: u32) -> (f64, f64, f64) {
        const N: i32 = 96;
        let step = 10.0 * self.peak_length / N as f64;
        let (mut lo, mut hi, mut steep) = (0.0f64, 0.0f64, 0.0f64);
        for j in 0..N {
            for i in 0..N {
                // Jittered, so the samples never fall in step with the waves.
                let (jx, jz) = (unit(i, j, seed ^ 0x2545_F491), unit(j, i, seed ^ 0x9E37_79B9));
                let (h, s, _, _, _) = self.sum((i as f64 + jx) * step, (j as f64 + jz) * step, &Near::OPEN);
                (lo, hi, steep) = (lo.min(h), hi.max(h), steep.max(s));
            }
        }
        (lo, hi, steep)
    }

    /// The components added up at a place where the water has not been moved
    /// sideways: height, steepness, how far the water there is moved, and
    /// the smoothed height.
    fn sum(&self, x: f64, z: f64, near: &Near) -> (f64, f64, f64, f64, f64) {
        let (mut height, mut steep, mut dx, mut dz, mut smooth) = (0.0, 0.0, 0.0, 0.0, 0.0);
        for w in &self.waves {
            let along = x * w.dir.0 + z * w.dir.1 - near.lag;
            let (sin, cos) = (along * w.k + w.phase).sin_cos();
            let amp = w.amp * if w.long { near.long } else { near.short };
            height += amp * cos;
            smooth += amp * w.tone * cos;
            steep += amp * w.k * cos;
            dx -= amp * sin * w.dir.0;
            dz -= amp * sin * w.dir.1;
        }
        (height, steep * near.squeeze, dx * self.chop, dz * self.chop, smooth)
    }

    /// Water height at a world position, in blocks above the resting level.
    pub fn height(&self, x: f64, z: f64) -> f64 {
        self.sample(x, z, &Near::OPEN).height
    }

    /// The water at a world position, given what land has done to the waves there.
    pub fn sample(&self, x: f64, z: f64, near: &Near) -> Sample {
        // Find the water that has been moved to this place.
        let (mut px, mut pz) = (x, z);
        if self.chop > 0.0 {
            for _ in 0..3 {
                let (_, _, dx, dz, _) = self.sum(px, pz, near);
                (px, pz) = (x - dx, z - dz);
            }
        }
        let (height, steep, _, _, smooth) = self.sum(px, pz, near);
        Sample { height: height.clamp(self.lowest, self.highest), steep, smooth }
    }

    /// How steep and squeezed together the water was at a place, `ago` main
    /// wave periods before the moment that is built. About 0 on level water
    /// and highest on the crests: for one plain wave it is half the height
    /// times 2 pi over the wavelength, and waves break as it nears 0.44.
    /// The short waves riding on the sea count for `short_share` of theirs.
    pub fn steepness(&self, x: f64, z: f64, ago: f64, near: &Near, short_share: f64) -> f64 {
        let (long, short) = (near.long, near.short * short_share);
        let mut steep = 0.0;
        for w in &self.waves {
            let amp = w.amp * if w.long { long } else { short };
            if amp != 0.0 {
                let along = x * w.dir.0 + z * w.dir.1 - near.lag;
                steep += amp * w.k * (along * w.k + w.phase + w.rate * ago).cos();
            }
        }
        steep * near.squeeze
    }

    /// How much `steepness` varies over the open sea: its standard deviation.
    pub fn steepness_deviation(&self, short_share: f64) -> f64 {
        (self.waves.iter().map(|w| (w.amp * w.k * if w.long { 1.0 } else { short_share }).powi(2)).sum::<f64>() * 0.5).sqrt()
    }

    /// The lowest the water can ever be, relative to the resting level.
    pub fn lowest(&self) -> f64 {
        self.lowest
    }
    /// The highest the water can ever be, relative to the resting level.
    pub fn highest(&self) -> f64 {
        self.highest
    }
    /// The greatest steepness found in open sea; see `steepness`.
    pub fn steepest(&self) -> f64 {
        self.steepest
    }
    /// How far crests are sharpened, as a share of a full trochoid.
    pub fn chop(&self) -> f64 {
        self.chop
    }
    /// Distance between the main crests, in blocks.
    pub fn peak_length(&self) -> f64 {
        self.peak_length
    }
    /// A typical length of the short waves riding on the sea, in blocks.
    pub fn short_length(&self) -> f64 {
        let (mut k, mut energy) = (0.0, 0.0);
        for w in self.waves.iter().filter(|w| !w.long) {
            k += w.k * w.amp * w.amp;
            energy += w.amp * w.amp;
        }
        if energy > 1e-300 { TAU * energy / k } else { 0.4 * self.peak_length }
    }
    /// The direction the waves travel, as a unit vector.
    pub fn heading(&self) -> (f64, f64) {
        self.heading
    }
    /// How narrowly the directions of the main waves are bunched (the "s" of
    /// the cos^2s spreading).
    pub fn spread(&self) -> f64 {
        self.spread
    }
}
