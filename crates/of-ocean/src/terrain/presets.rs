//! The demonstration lands: a coast, and a cliff coast. They are part of the
//! development build only (feature "dev"); the package has neither.

use super::*;

/// A demonstration coast: a mainland along the north edge whose shore runs
/// from a sandy beach in the west into a cliff sea front in the east; an
/// island with a beach on its sheltered side and rocks where the waves
/// arrive; and open sea to the south and east.
///
/// `wave_direction` is the direction the waves travel (0 is +X, 90 is +Z).
pub fn coast_scene(width: i32, length: i32, seed: u32, wave_direction: f64) -> Terrain {
    let (w, l) = (width as f64, length as f64);
    // Designed at 320 x 224; other plot sizes scale the layout to fit.
    let u = (w / 320.0).min(l / 224.0).max(0.3);
    let from = (wave_direction + 180.0).to_radians();
    let (from_x, from_z) = (from.cos(), from.sin());
    let cells = (width * length) as usize;
    let mut height = vec![0f32; cells];
    let mut rockiness = vec![0f32; cells];
    let mut forced_rock = vec![false; cells];

    for z in 0..length {
        for x in 0..width {
            let (fx, fz) = (x as f64 + 0.5, z as f64 + 0.5);
            let across = fx / w;

            // ---- Mainland. `d` is blocks inland from the waterline.
            let cliff = smoothstep(0.36, 0.60, across);
            let coast = l * 0.27 + l * 0.04 * (across * 4.4 + 0.6).sin() + 5.0 * u * fbm(fx / (55.0 * u), 3.3, seed ^ 11, 2);
            let d = (coast - fz) / u + cliff * 6.0 * fbm(fx / (15.0 * u), fz / (15.0 * u), seed ^ 12, 3);
            let mut main = if d < 0.0 {
                // The sea bed shelves gently off the beach and drops away below the cliff.
                d * lerp(0.10, 0.5, cliff) - 0.004 * d * d * (1.0 - cliff)
            } else {
                let beach = (0.085 * d).min(2.6 + 0.02 * d);
                let beach_width = 18.0 * (1.0 - cliff);
                let rise = lerp(12.0, 2.2, cliff);
                let hills = 2.2 * fbm(fx / (38.0 * u), fz / (38.0 * u), seed ^ 13, 3) + 0.035 * (d - beach_width).max(0.0);
                beach.max(smoothstep(beach_width, beach_width + rise, d) * (lerp(6.0, 19.0, cliff) + hills))
            };
            let mut main_forced = false;
            // Sea stacks standing off the cliff.
            let band = cliff * smoothstep(-30.0, -18.0, d) * (1.0 - smoothstep(-9.0, -4.0, d));
            let stack = (value(fx / (7.0 * u), fz / (7.0 * u), seed ^ 14) - 0.56) * 34.0 * band;
            if stack > 0.5 {
                main = main.max(stack.min(6.0 + 3.0 * value(fx / 9.0, fz / 9.0, seed ^ 15)));
                main_forced = true;
            }

            // ---- Island. `di` is blocks inland from its waterline.
            let (ex, ez) = (fx - w * 0.30, fz - l * 0.70);
            let (sin, cos) = 0.35f64.sin_cos();
            let (rx, rz) = (ex * cos + ez * sin, ez * cos - ex * sin);
            let (ra, rb) = (52.0 * u, 36.0 * u);
            let reach = ((rx / ra).powi(2) + (rz / rb).powi(2)).sqrt();
            let bearing = ez.atan2(ex);
            let outline = 1.0 + 0.2 * fbm(3.0 + 1.6 * bearing.cos(), 7.0 + 1.6 * bearing.sin(), seed ^ 21, 3);
            let di = (outline - reach) * rb / u;
            // 1 on the side the waves strike, -1 on the sheltered side.
            let exposure = (ex * from_x + ez * from_z) / (ex.hypot(ez) + 1e-6);
            let rock = smoothstep(-0.2, 0.35, exposure);
            let mut isle = if di < 0.0 {
                di * lerp(0.10, 0.42, rock) - 0.004 * di * di * (1.0 - rock)
            } else {
                let beach = (0.10 * di).min(2.4 + 0.02 * di);
                // A low, broken rock shelf instead of sand where the sea is rough.
                let shelf = 0.6 + 0.22 * di + 1.5 * value(fx / (3.5 * u), fz / (3.5 * u), seed ^ 24).abs();
                let shore_width = lerp(7.0, 8.0, rock);
                let hill = lerp(8.5, 10.0, rock) + 2.0 * fbm(fx / (20.0 * u), fz / (20.0 * u), seed ^ 22, 3);
                lerp(beach, shelf, rock).max(smoothstep(shore_width, shore_width + lerp(9.0, 8.0, rock), di) * hill)
            };
            let mut isle_forced = false;
            // Boulders in and just above the water along the rocky shore.
            let band = rock * smoothstep(-9.0, -3.0, di) * (1.0 - smoothstep(3.0, 7.0, di));
            let boulder = (value(fx / (4.2 * u), fz / (4.2 * u), seed ^ 23) - 0.18) * 9.0 * band;
            if boulder > 0.4 {
                isle = isle.max(boulder.min(4.5));
                isle_forced = true;
            }

            let i = (z * width + x) as usize;
            let (h, r, forced) = if isle > main { (isle, rock, isle_forced) } else { (main, cliff, main_forced) };
            height[i] = h as f32;
            rockiness[i] = r as f32;
            forced_rock[i] = forced;
        }
    }

    let mut kind = vec![Ground::Rock; cells];
    let at = |x: i32, z: i32| (z.clamp(0, length - 1) * width + x.clamp(0, width - 1)) as usize;
    for z in 0..length {
        for x in 0..width {
            let i = at(x, z);
            let h = height[i];
            let slope = ((height[at(x + 1, z)] - height[at(x - 1, z)]) * 0.5).hypot((height[at(x, z + 1)] - height[at(x, z - 1)]) * 0.5);
            // Break up the line where sand gives way to rock.
            let rocky = rockiness[i] + 0.22 * value(x as f64 / (6.0 * u), z as f64 / (6.0 * u), seed ^ 31) as f32;
            kind[i] = if forced_rock[i] {
                Ground::Rock
            } else if rocky < 0.5 {
                if h < 3.2 {
                    Ground::Sand
                } else if slope > 1.2 {
                    Ground::Rock
                } else {
                    Ground::Grass
                }
            } else if h < 2.4 || slope > 1.1 {
                Ground::Rock
            } else {
                Ground::Grass
            };
        }
    }
    Terrain { width, length, floor: -10, blocks: TerrainBlocks::default(), height, kind, roof: Vec::new(), mouths: Vec::new(), bed: Vec::new(), ponds: Vec::new(), placed: None }
}

/// A demonstration cliff coast in the manner of Etretat in Normandy: a tall
/// cliff sea front along the north edge; a headland in the west that ends in
/// a rock arch, with a needle of rock standing in the sea beyond it; other
/// large rocks standing alone offshore; a small bay with a shingle beach
/// where the cliffs come down; and in the east a cove walled in by rock.
pub fn cliff_scene(width: i32, length: i32, seed: u32) -> Terrain {
    let (w, l) = (width as f64, length as f64);
    // Designed at 320 x 224; other plot sizes scale the layout to fit.
    let u = (w / 320.0).min(l / 224.0).max(0.3);
    let tall = u.clamp(0.5, 1.0);
    let cells = (width * length) as usize;
    let mut height = vec![0f32; cells];
    let mut rockiness = vec![0f32; cells];
    let mut forced: Vec<Option<Ground>> = vec![None; cells];
    let mut roof: Vec<Option<(i32, i32)>> = vec![None; cells];

    let bay = |px: f64| smoothstep(150.0, 176.0, px) * (1.0 - smoothstep(220.0, 246.0, px));
    // A cove cut into the eastern cliffs.
    let cove = |px: f64| smoothstep(258.0, 267.0, px) * (1.0 - smoothstep(295.0, 304.0, px));
    let coast_at = |px: f64| 62.0 - 16.0 * bay(px) - 22.0 * cove(px) + 5.0 * (px / 64.0 + 0.4).sin() + 4.0 * fbm(px / 45.0, 1.7, seed ^ 41, 2);
    // The headland: a wall of rock from `root` out to sea along `out`.
    let root = (112.0, coast_at(112.0) - 6.0);
    let out = {
        let (x, z) = (-0.82f64, 0.57f64);
        (x / x.hypot(z), z / x.hypot(z))
    };
    const HEADLAND: f64 = 54.0;
    // The opening of the arch, measured along the headland, and its height.
    const OPENING: (f64, f64) = (36.0, 9.0);
    const ARCH: f64 = 15.0;
    // Rocks standing alone: centre, half-sizes, turn, height, how flat the
    // top is (large is a table, about 1 a spire), and whether bare rock.
    let needle = (root.0 + out.0 * (HEADLAND + 21.0), root.1 + out.1 * (HEADLAND + 21.0));
    let rocks: [(f64, f64, f64, f64, f64, f64, f64, bool); 5] = [
        (needle.0, needle.1, 6.5, 4.4, out.1.atan2(out.0), 24.0, 1.35, true),
        (236.0, 152.0, 14.0, 9.5, 0.4, 15.0, 7.0, false),
        (264.0, 122.0, 5.5, 4.5, 0.0, 7.0, 2.2, true),
        (142.0, 172.0, 7.5, 4.6, -0.5, 9.5, 2.6, true),
        (96.0, 150.0, 6.0, 5.0, 0.2, 12.0, 1.7, true),
    ];

    for z in 0..length {
        for x in 0..width {
            let (fx, fz) = (x as f64 + 0.5, z as f64 + 0.5);
            // Place on the 320 x 224 design.
            let (px, pz) = ((fx - w * 0.5) / u + 160.0, fz / u);
            let i = (z * width + x) as usize;

            // ---- Mainland. `d` is blocks inland from the waterline.
            let cliff = 1.0 - bay(px);
            let d = coast_at(px) - pz + cliff * 3.0 * fbm(px / 14.0, pz / 14.0, seed ^ 42, 3);
            let crown = lerp(26.0, 22.0, smoothstep(200.0, 250.0, px));
            let hills = 2.0 * fbm(px / 40.0, pz / 40.0, seed ^ 43, 3) + 0.03 * d.max(0.0);
            let mut h = if d < 0.0 {
                // Deep water under the cliffs, a shelving bed off the beach.
                lerp(0.10 * d - 0.003 * d * d, -13.0 * (1.0 - (d / 9.0).exp()) + 0.16 * d, cliff)
            } else {
                let beach = (0.09 * d).min(2.6 + 0.02 * d);
                let valley = beach.max(smoothstep(16.0, 34.0, d) * (7.0 + hills));
                lerp(valley, smoothstep(0.0, 2.2, d) * (crown + hills), cliff)
            };
            let bed = h.min(0.0);
            // Fallen blocks at the foot of the cliffs.
            let band = cliff * smoothstep(-12.0, -6.0, d) * (1.0 - smoothstep(-3.0, -1.0, d));
            let boulder = (value(px / 4.5, pz / 4.5, seed ^ 44) - 0.35) * 10.0 * band;
            if boulder > 0.5 {
                h = h.max(boulder.min(4.0));
                forced[i] = Some(Ground::Rock);
            }

            // ---- The headland and its arch.
            let (hx, hz) = (px - root.0, pz - root.1);
            let (along, across) = (hx * out.0 + hz * out.1, hx * out.1 - hz * out.0);
            if along > 0.0 && along < HEADLAND {
                let half = lerp(9.0, 5.5, along / HEADLAND) + 1.2 * fbm(along / 9.0, 4.2, seed ^ 45, 2);
                let within = (half - across.abs()).min(HEADLAND - along);
                let top = lerp(26.0, 22.5, along / HEADLAND) * smoothstep(0.0, 2.0, within);
                let s = (along - OPENING.0) / OPENING.1;
                let under = if s.abs() < 1.0 { ARCH * (1.0 - s * s).sqrt() } else { 0.0 };
                if within <= 0.0 {
                    // Beside the headland: open sea.
                } else if under > 2.5 {
                    // Open water with rock spanning over it.
                    let (lo, hi) = ((under * tall).ceil() as i32, (top * tall).round() as i32);
                    if hi > lo {
                        roof[i] = Some((lo, hi));
                    }
                    forced[i] = Some(Ground::Rock);
                } else if top > h {
                    h = top;
                    // Bare rock toward the sea, turf where it joins the land.
                    if along > 0.45 * HEADLAND {
                        forced[i] = Some(Ground::Rock);
                    }
                }
            }

            // ---- Rocks standing alone in the sea.
            for (cx, cz, ra, rb, turn, top, flat, bare) in rocks {
                let (ex, ez) = (px - cx, pz - cz);
                let (sin, cos) = turn.sin_cos();
                let (rx, rz) = (ex * cos + ez * sin, ez * cos - ex * sin);
                let bearing = ez.atan2(ex);
                let outline = 1.0 + 0.14 * fbm(5.0 + 1.5 * bearing.cos(), 9.0 + 1.5 * bearing.sin(), seed ^ (51 + cx as u32), 3);
                let reach = (rx / ra).hypot(rz / rb) / outline;
                if reach < 1.0 {
                    let stands = top * (1.0 - reach.powf(flat)) + if bare { 0.0 } else { 0.8 * fbm(px / 6.0, pz / 6.0, seed ^ 46, 2) };
                    if stands > h {
                        h = stands;
                        // Turf only on the flat top of a table rock.
                        if bare || stands < 0.8 * top {
                            forced[i] = Some(Ground::Rock);
                        }
                    }
                } else if reach < 2.2 {
                    // Rubble around the foot, under water.
                    h = h.max((-1.5 - 9.0 * (reach - 1.0)).max(bed));
                }
            }

            height[i] = (h * tall) as f32;
            rockiness[i] = cliff as f32;
        }
    }

    let mut kind = vec![Ground::Rock; cells];
    let at = |x: i32, z: i32| (z.clamp(0, length - 1) * width + x.clamp(0, width - 1)) as usize;
    for z in 0..length {
        for x in 0..width {
            let i = at(x, z);
            let h = height[i];
            let slope = ((height[at(x + 1, z)] - height[at(x - 1, z)]) * 0.5).hypot((height[at(x, z + 1)] - height[at(x, z - 1)]) * 0.5);
            let rocky = rockiness[i] + 0.22 * value(x as f64 / (6.0 * u), z as f64 / (6.0 * u), seed ^ 31) as f32;
            kind[i] = match forced[i] {
                Some(ground) => ground,
                None if rocky < 0.5 && h < 3.2 => Ground::Sand,
                None if h < 2.4 || slope > 1.15 => Ground::Rock,
                None => Ground::Grass,
            };
        }
    }
    Terrain { width, length, floor: -12, blocks: TerrainBlocks::default(), height, kind, roof, mouths: Vec::new(), bed: Vec::new(), ponds: Vec::new(), placed: None }
}
