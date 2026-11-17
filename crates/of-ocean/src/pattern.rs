//! Where the small waves come from. Small waves are never built as blocks:
//! they only tilt the shading of the surface. They can be the wind's own
//! ripples (the standard), a pattern from the library made here, or a
//! normal map (or a grey height map) brought in from an image file.
//!
//! Kept on its own so the pattern sources can be polished later without
//! touching the bake.

use crate::noise::hash;
use std::f64::consts::TAU;
use std::sync::Arc;

/// Where the ripples come from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// The wind's own ripples, worked out from the wave settings.
    Wind,
    /// One of the named patterns made here.
    Library,
    /// A normal map or height map brought in from an image.
    Image,
    /// Many small waves drawn from a sea's wave spectrum: their sizes
    /// gather about one peak and their directions fan out about the main one.
    Spectrum,
}

/// Which way the green channel of a normal map points.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlipY {
    /// Worked out from the image: the sign that makes its slopes fit a real surface.
    Auto,
    /// Green points up the picture (OpenGL style).
    Up,
    /// Green points down the picture (DirectX style).
    Down,
}

/// The patterns in the library, by key. The first is the standard.
pub const LIBRARY: [&str; 5] = ["broken_crests", "fine_chop", "long_ripples", "cross_sea", "calm_swirls"];

/// A tile of slopes: how much the surface rises per unit across (u) and
/// down (v) the tile, scaled so that their typical size is 1.
#[derive(Debug, Clone, PartialEq)]
pub struct NormalMap {
    pub width: usize,
    pub height: usize,
    slope: Vec<[f32; 2]>,
    /// True when the tile repeats without a seam; otherwise it is mirrored.
    seamless: bool,
}

impl NormalMap {
    fn from_heights(width: usize, height: usize, h: &[f64], seamless: bool) -> Self {
        let at = |x: isize, y: isize| -> f64 {
            let (x, y) = if seamless {
                (x.rem_euclid(width as isize) as usize, y.rem_euclid(height as isize) as usize)
            } else {
                (x.clamp(0, width as isize - 1) as usize, y.clamp(0, height as isize - 1) as usize)
            };
            h[y * width + x]
        };
        let mut slope = Vec::with_capacity(width * height);
        for y in 0..height as isize {
            for x in 0..width as isize {
                slope.push([((at(x + 1, y) - at(x - 1, y)) * 0.5) as f32, ((at(x, y + 1) - at(x, y - 1)) * 0.5) as f32]);
            }
        }
        Self::normalised(width, height, slope, seamless)
    }

    fn normalised(width: usize, height: usize, mut slope: Vec<[f32; 2]>, seamless: bool) -> Self {
        let n = slope.len().max(1) as f64;
        let mean: [f64; 2] = [0, 1].map(|a| slope.iter().map(|s| s[a] as f64).sum::<f64>() / n);
        for s in &mut slope {
            s[0] -= mean[0] as f32;
            s[1] -= mean[1] as f32;
        }
        let rms = (slope.iter().map(|s| (s[0] as f64).powi(2) + (s[1] as f64).powi(2)).sum::<f64>() / n).sqrt();
        if rms > 1e-9 {
            for s in &mut slope {
                s[0] = (s[0] as f64 / rms) as f32;
                s[1] = (s[1] as f64 / rms) as f32;
            }
        }
        Self { width, height, slope, seamless }
    }

    /// A named pattern from the library, as a seamless 256 x 256 tile.
    /// Unknown names give the standard one.
    pub fn library(name: &str, seed: u32) -> Self {
        const SIZE: usize = 256;
        // (count, shortest and longest wave in tile widths' fractions as
        // whole waves per tile, spread of directions in degrees about the
        // main one, a second direction, crest sharpening)
        let (count, waves, spread, second, sharpen) = match name {
            "fine_chop" => (48, (10, 28), 80.0, None, 0.2),
            "long_ripples" => (20, (3, 9), 18.0, None, 0.0),
            "cross_sea" => (32, (5, 14), 20.0, Some(70.0), 0.25),
            "calm_swirls" => (24, (2, 7), 180.0, None, 0.0),
            _ => (40, (4, 16), 55.0, None, 0.55),
        };
        let rand = |i: u32, salt: u32| hash(i as i32, salt as i32, seed ^ 0x5EED) as f64 / (u32::MAX as f64 + 1.0);
        let mut parts = Vec::new();
        for i in 0..count {
            let base = match second {
                Some(turn) if i % 2 == 1 => turn,
                _ => 0.0,
            };
            let angle = (base + (rand(i, 1) - 0.5) * spread).to_radians();
            let cycles = waves.0 as f64 + rand(i, 2) * (waves.1 - waves.0) as f64;
            // Whole waves across the tile both ways, so it repeats without a seam.
            let (kx, ky) = ((cycles * angle.cos()).round(), (cycles * angle.sin()).round());
            if kx == 0.0 && ky == 0.0 {
                continue;
            }
            let amp = 1.0 / (kx * kx + ky * ky).sqrt().powf(1.2);
            parts.push((kx, ky, amp, rand(i, 3) * TAU));
        }
        let mut h = vec![0f64; SIZE * SIZE];
        for (y, row) in h.chunks_mut(SIZE).enumerate() {
            for (x, cell) in row.iter_mut().enumerate() {
                let (u, v) = (x as f64 / SIZE as f64, y as f64 / SIZE as f64);
                *cell = parts.iter().map(|(kx, ky, a, p)| a * (TAU * (kx * u + ky * v) + p).sin()).sum();
            }
        }
        if sharpen > 0.0 {
            // Narrow crests and broad troughs, as on a wind sea.
            let top = h.iter().fold(1e-9f64, |m, v| m.max(v.abs()));
            for v in &mut h {
                let t = *v / top;
                *v = t + sharpen * (t * t - 0.33);
            }
        }
        Self::from_heights(SIZE, SIZE, &h, true)
    }

    /// A seamless 256 x 256 tile of many waves drawn from a wind sea's
    /// spectrum. `spread` is how far, in degrees, their directions fan out
    /// about the main one; `peak` (1 or more) how much their sizes gather
    /// about the peak size, as the peak enhancement of a JONSWAP spectrum.
    pub fn spectrum(seed: u32, spread: f64, peak: f64) -> Self {
        const SIZE: usize = 256;
        const COUNT: u32 = 160;
        // The peak size, in whole waves per tile.
        const KP: f64 = 8.0;
        let rand = |i: u32, salt: u32| hash(i as i32, salt as i32, seed ^ 0x5BEC) as f64 / (u32::MAX as f64 + 1.0);
        let (spread, gamma) = (spread.clamp(0.0, 180.0), peak.max(1.0));
        let mut parts = Vec::new();
        for i in 0..COUNT {
            // Sizes from 2 to 48 waves per tile, even in proportion, so every
            // octave gets its share of the waves.
            let k = 2.0 * 24f64.powf(rand(i, 1));
            // Directions from a bell about the main one, within the spread.
            let angle = ((rand(i, 2) + rand(i, 3) - 1.0) * spread).to_radians();
            let (kx, ky) = ((k * angle.cos()).round(), (k * angle.sin()).round());
            if kx == 0.0 && ky == 0.0 {
                continue;
            }
            let k = (kx * kx + ky * ky).sqrt();
            // A wind sea's spectrum over wave number: little below the peak,
            // falling off as k^-3 above it, sharpened at the peak.
            let sigma = if k <= KP { 0.07 } else { 0.09 };
            let r = (-((k / KP).sqrt() - 1.0).powi(2) / (2.0 * sigma * sigma)).exp();
            let energy = k.powi(-3) * (-1.25 * (KP / k).powi(2)).exp() * gamma.powf(r);
            // Sizes are drawn evenly in proportion, so each stands for a
            // band as wide as itself.
            let amp = (energy * k).sqrt();
            parts.push((kx, ky, amp, rand(i, 4) * TAU));
        }
        let mut h = vec![0f64; SIZE * SIZE];
        for (y, row) in h.chunks_mut(SIZE).enumerate() {
            for (x, cell) in row.iter_mut().enumerate() {
                let (u, v) = (x as f64 / SIZE as f64, y as f64 / SIZE as f64);
                *cell = parts.iter().map(|(kx, ky, a, p)| a * (TAU * (kx * u + ky * v) + p).sin()).sum();
            }
        }
        Self::from_heights(SIZE, SIZE, &h, true)
    }

    /// A tile from an image's pixels, three bytes each. A grey image is read
    /// as heights, anything else as a normal map. Returns the tile and
    /// whether the green channel was read as pointing down.
    pub fn from_rgb(width: usize, height: usize, rgb: &[u8], flip: FlipY) -> Result<(Self, bool), String> {
        if width < 2 || height < 2 || rgb.len() < width * height * 3 {
            return Err("the pattern image is too small".into());
        }
        let px = |i: usize| [rgb[3 * i] as f64 / 255.0, rgb[3 * i + 1] as f64 / 255.0, rgb[3 * i + 2] as f64 / 255.0];
        let grey = (0..width * height).step_by(7).all(|i| {
            let c = px(i);
            (c[0] - c[1]).abs() < 0.02 && (c[1] - c[2]).abs() < 0.02
        });
        if grey {
            let h: Vec<f64> = (0..width * height).map(|i| px(i)[0] * 8.0).collect();
            return Ok((Self::from_heights(width, height, &h, false), false));
        }
        let mut slope = Vec::with_capacity(width * height);
        for i in 0..width * height {
            let c = px(i);
            let (nx, ny, nz) = (2.0 * c[0] - 1.0, 2.0 * c[1] - 1.0, (2.0 * c[2] - 1.0).max(0.05));
            // The green channel of an OpenGL map points up the picture, but
            // v runs down it.
            slope.push([(-nx / nz) as f32, (ny / nz) as f32]);
        }
        let down = match flip {
            FlipY::Up => false,
            FlipY::Down => true,
            FlipY::Auto => {
                // A real surface has slopes with no twist: the change of the
                // across-slope going down equals the change of the
                // down-slope going across. Read the green channel the way
                // that fits that better.
                let (mut kept, mut turned) = (0.0, 0.0);
                for y in 1..height - 1 {
                    for x in 1..width - 1 {
                        let s = |x: usize, y: usize| slope[y * width + x];
                        let du_dv = (s(x, y + 1)[0] - s(x, y - 1)[0]) as f64;
                        let dv_du = (s(x + 1, y)[1] - s(x - 1, y)[1]) as f64;
                        kept += (du_dv - dv_du).abs();
                        turned += (du_dv + dv_du).abs();
                    }
                }
                turned < kept
            }
        };
        if down {
            for s in &mut slope {
                s[1] = -s[1];
            }
        }
        Ok((Self::normalised(width, height, slope, false), down))
    }

    /// The slope at a point of the tile, `u` and `v` in tiles. A tile that
    /// does not repeat cleanly is mirrored at each edge, which turns its
    /// slopes round there.
    pub fn slope(&self, u: f64, v: f64) -> [f64; 2] {
        let (mut fx, mut fy) = (u * self.width as f64, v * self.height as f64);
        let (mut sx, mut sy) = (1.0, 1.0);
        if !self.seamless {
            let mirror = |f: f64, n: f64| -> (f64, f64) {
                let p = f.rem_euclid(2.0 * n);
                if p < n { (p, 1.0) } else { (2.0 * n - p, -1.0) }
            };
            (fx, sx) = mirror(fx, self.width as f64);
            (fy, sy) = mirror(fy, self.height as f64);
        }
        let (w, h) = (self.width as isize, self.height as isize);
        let (x0, y0) = ((fx - 0.5).floor(), (fy - 0.5).floor());
        let (tx, ty) = (fx - 0.5 - x0, fy - 0.5 - y0);
        let get = |x: isize, y: isize| {
            let (x, y) = if self.seamless { (x.rem_euclid(w), y.rem_euclid(h)) } else { (x.clamp(0, w - 1), y.clamp(0, h - 1)) };
            self.slope[(y * w + x) as usize]
        };
        let (x0, y0) = (x0 as isize, y0 as isize);
        let mut out = [0.0; 2];
        for (a, o) in out.iter_mut().enumerate() {
            let top = get(x0, y0)[a] as f64 * (1.0 - tx) + get(x0 + 1, y0)[a] as f64 * tx;
            let bottom = get(x0, y0 + 1)[a] as f64 * (1.0 - tx) + get(x0 + 1, y0 + 1)[a] as f64 * tx;
            *o = top * (1.0 - ty) + bottom * ty;
        }
        [out[0] * sx, out[1] * sy]
    }
}

/// The ripple pattern of a bake.
#[derive(Debug, Clone, PartialEq)]
pub struct Pattern {
    pub source: Source,
    /// Library pattern key.
    pub name: String,
    /// Blocks one tile of the pattern spans; its shape is kept.
    pub span: f64,
    /// How strongly the pattern tilts the shading, against the wind's ripples.
    pub strength: f64,
    /// Turn of the pattern from the wave direction, degrees.
    pub rotation: f64,
    /// Turn the pattern's hills into hollows.
    pub invert: bool,
    pub flip: FlipY,
    /// The image tile, read by the caller when the source is an image.
    pub image: Option<Arc<NormalMap>>,
    /// For a spectrum: how far the directions fan out, degrees.
    pub spread: f64,
    /// For a spectrum: how much the sizes gather about the peak (1 or more).
    pub peak: f64,
}

impl Default for Pattern {
    fn default() -> Self {
        Self {
            source: Source::Wind,
            name: LIBRARY[0].into(),
            span: 64.0,
            strength: 1.0,
            rotation: 0.0,
            invert: false,
            flip: FlipY::Auto,
            image: None,
            spread: 40.0,
            peak: 3.3,
        }
    }
}

/// A pattern made ready for the bake.
pub struct Ready {
    map: Arc<NormalMap>,
    span: f64,
    gain: f64,
    /// The way the tile's u axis runs over the world.
    turn: (f64, f64),
}

impl Pattern {
    /// The tile to use, or `None` for the wind's own ripples. `heading` is
    /// the way the waves run. An image source with no image falls back to
    /// the wind's ripples.
    pub fn ready(&self, seed: u32, heading: (f64, f64)) -> Option<Ready> {
        let map = match self.source {
            Source::Wind => return None,
            Source::Library => Arc::new(NormalMap::library(&self.name, seed)),
            Source::Image => self.image.clone()?,
            Source::Spectrum => Arc::new(NormalMap::spectrum(seed, self.spread, self.peak)),
        };
        let (s, c) = self.rotation.to_radians().sin_cos();
        let turn = (heading.0 * c - heading.1 * s, heading.0 * s + heading.1 * c);
        Some(Ready { map, span: self.span.max(1.0), gain: self.strength * if self.invert { -1.0 } else { 1.0 }, turn })
    }
}

impl Ready {
    /// The slope the pattern gives at a world point, x and z, at strength 1.
    pub fn slope(&self, wx: f64, wz: f64) -> (f64, f64) {
        let (a, b) = self.turn;
        let (u, v) = ((wx * a + wz * b) / self.span, (wz * a - wx * b) / self.span);
        let s = self.map.slope(u, v);
        // From slopes along the tile's axes to slopes along x and z.
        let (su, sv) = (s[0] * self.gain, s[1] * self.gain);
        (su * a - sv * b, su * b + sv * a)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn library_tiles_repeat_and_are_scaled() {
        for name in LIBRARY {
            let m = NormalMap::library(name, 3);
            let a = m.slope(0.25, 0.4);
            let b = m.slope(1.25, -0.6);
            assert!((a[0] - b[0]).abs() < 1e-9 && (a[1] - b[1]).abs() < 1e-9, "{name} repeats");
            let rms = (m.slope.iter().map(|s| (s[0] as f64).powi(2) + (s[1] as f64).powi(2)).sum::<f64>() / m.slope.len() as f64).sqrt();
            assert!((rms - 1.0).abs() < 1e-3, "{name} rms {rms}");
        }
    }

    #[test]
    fn spectrum_tiles_repeat_and_follow_the_spread() {
        // Mean square slope across the main direction, against along it.
        let across = |m: &NormalMap| {
            let (a, b): (f64, f64) = m.slope.iter().fold((0.0, 0.0), |(a, b), s| (a + (s[1] as f64).powi(2), b + (s[0] as f64).powi(2)));
            a / b
        };
        let narrow = NormalMap::spectrum(5, 0.0, 3.3);
        let wide = NormalMap::spectrum(5, 90.0, 3.3);
        let (a, b) = (wide.slope(0.3, 0.7), wide.slope(-0.7, 2.7));
        assert!((a[0] - b[0]).abs() < 1e-9 && (a[1] - b[1]).abs() < 1e-9, "the tile repeats");
        assert!(across(&narrow) < 1e-6, "no spread runs one way only");
        assert!(across(&wide) > 0.2, "a wide spread turns waves across: {}", across(&wide));
        let rms = (wide.slope.iter().map(|s| (s[0] as f64).powi(2) + (s[1] as f64).powi(2)).sum::<f64>() / wide.slope.len() as f64).sqrt();
        assert!((rms - 1.0).abs() < 1e-3);
        assert_ne!(NormalMap::spectrum(5, 40.0, 1.0), NormalMap::spectrum(5, 40.0, 8.0), "the peak setting matters");
    }

    #[test]
    fn green_direction_is_found() {
        // A normal map of a bump, written the OpenGL way, then the DirectX way.
        let n = 64;
        let mut up = Vec::new();
        let mut down = Vec::new();
        for y in 0..n {
            for x in 0..n {
                let (u, v) = (x as f64 / n as f64 * TAU, y as f64 / n as f64 * TAU);
                // h = sin u * sin v: dh/du and dh/dv, with v down the picture.
                let (du, dv) = (u.cos() * v.sin(), u.sin() * v.cos());
                let len = (du * du + dv * dv + 1.0).sqrt();
                let (nx, ny_up, nz) = (-du / len, dv / len, 1.0 / len);
                let byte = |f: f64| ((f * 0.5 + 0.5) * 255.0).round() as u8;
                up.extend([byte(nx), byte(ny_up), byte(nz)]);
                down.extend([byte(nx), byte(-ny_up), byte(nz)]);
            }
        }
        assert!(!NormalMap::from_rgb(n, n, &up, FlipY::Auto).unwrap().1);
        assert!(NormalMap::from_rgb(n, n, &down, FlipY::Auto).unwrap().1);
    }

    #[test]
    fn mirrored_tiles_have_no_jump_at_the_edge() {
        let h: Vec<f64> = (0..16 * 16).map(|i| (i % 16) as f64).collect();
        let m = NormalMap::from_heights(16, 16, &h, false);
        let (a, b) = (m.slope(0.999, 0.5), m.slope(1.001, 0.5));
        assert!((a[0] + b[0]).abs() < 0.2, "slope turns round at a mirror: {a:?} {b:?}");
    }
}
