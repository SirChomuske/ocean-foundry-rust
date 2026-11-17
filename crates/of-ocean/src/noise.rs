//! Seeded, repeatable noise. Every function depends only on its inputs, so
//! the same place in the world always gets the same value.

pub fn hash(x: i32, z: i32, seed: u32) -> u32 {
    let mut h = (x as u32).wrapping_mul(0x9E37_79B1) ^ (z as u32).wrapping_mul(0x85EB_CA77) ^ seed.wrapping_mul(0xC2B2_AE3D);
    h ^= h >> 15;
    h = h.wrapping_mul(0x2C1B_3C6D);
    h ^= h >> 12;
    h = h.wrapping_mul(0x297A_2D39);
    h ^= h >> 15;
    h
}

fn lattice(x: i32, z: i32, seed: u32) -> f64 {
    hash(x, z, seed) as f64 / u32::MAX as f64 * 2.0 - 1.0
}

/// Smooth noise between -1 and 1 that changes over about one unit.
pub fn value(x: f64, z: f64, seed: u32) -> f64 {
    let (x0, z0) = (x.floor(), z.floor());
    let ease = |t: f64| t * t * t * (t * (t * 6.0 - 15.0) + 10.0);
    let (sx, sz) = (ease(x - x0), ease(z - z0));
    let (ix, iz) = (x0 as i32, z0 as i32);
    let near = lattice(ix, iz, seed) + (lattice(ix + 1, iz, seed) - lattice(ix, iz, seed)) * sx;
    let far = lattice(ix, iz + 1, seed) + (lattice(ix + 1, iz + 1, seed) - lattice(ix, iz + 1, seed)) * sx;
    near + (far - near) * sz
}

/// Several layers of `value`, each finer and fainter. Stays within -1 to 1.
pub fn fbm(x: f64, z: f64, seed: u32, octaves: u32) -> f64 {
    let (mut sum, mut amp, mut norm, mut scale) = (0.0, 1.0, 0.0, 1.0);
    for o in 0..octaves {
        sum += amp * value(x * scale, z * scale, seed.wrapping_add(o.wrapping_mul(101)));
        norm += amp;
        amp *= 0.5;
        scale *= 2.03;
    }
    sum / norm
}

/// A mesh of cells about one unit across, like the bubbles of a foam: how
/// far a place is from the nearest border between two cells. 0 on a border,
/// rising to about 0.5 in the middle of a cell.
pub fn cell_edge(x: f64, z: f64, seed: u32) -> f64 {
    let (ix, iz) = (x.floor() as i32, z.floor() as i32);
    let (mut first, mut second) = (f64::MAX, f64::MAX);
    for dz in -1..=1 {
        for dx in -1..=1 {
            let (cx, cz) = (ix + dx, iz + dz);
            let h = hash(cx, cz, seed);
            let (px, pz) = (cx as f64 + (h & 0xFFFF) as f64 / 65536.0, cz as f64 + (h >> 16) as f64 / 65536.0);
            let away = (px - x).powi(2) + (pz - z).powi(2);
            if away < first {
                (first, second) = (away, first);
            } else if away < second {
                second = away;
            }
        }
    }
    second.sqrt() - first.sqrt()
}

/// 0 below `a`, 1 above `b`, a smooth ramp between.
pub fn smoothstep(a: f64, b: f64, x: f64) -> f64 {
    let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

pub fn lerp(a: f64, b: f64, t: f64) -> f64 {
    a + (b - a) * t
}
