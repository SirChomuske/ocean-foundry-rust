//! Lava: a field of dark crust and molten rock, made into blocks.
//!
//! Lava has no water in it and is not simulated. A pattern gives the relief
//! of the surface; where that relief is read as hot the rock is molten, and
//! elsewhere it has a crust. The colours run from the crust colour through
//! the molten colour to the hottest colour, and the molten rock throws a
//! glow on the crust beside it. The blocks are then chosen and built the
//! same way as for the sea: a foundation, a coloured surface, and stained
//! glass over it if asked for.
//!
//! This is the first, plain version of the lava method, kept in a module
//! of its own so it can be worked on without touching the sea.

use crate::build::{fill_rows, Baked, Stats};
use crate::colour::{self, Care, Matcher, NO_GLASS};
use crate::noise::{cell_edge, fbm, lerp, smoothstep};
use crate::tuning::Tuning;
use of_core::params::{self, ParamValues, Value};
use of_formats::catalogue::Catalogue;
use of_formats::export::{Layer, LayeredBuild};
use of_formats::model::BlockState;
use of_formats::Progress;
use std::collections::{BTreeMap, HashMap};

/// The built-in patterns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pattern {
    /// Plates of crust with cracks between them.
    Plates,
    /// Ropes of rock drawn out along the flow.
    Ropes,
    /// Broad pools and banks.
    Pools,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LavaSettings {
    pub origin: [i32; 2],
    pub width: i32,
    pub length: i32,
    pub placement_y: i32,
    pub pattern: Pattern,
    /// Size of the pattern in blocks, its turn in degrees and its seed.
    pub scale: f64,
    pub rotation: f64,
    pub seed: u32,
    /// How far the pattern is drawn out and bent along the flow, 0 to 1.
    pub distortion: f64,
    /// Swap hot and cold.
    pub invert: bool,
    /// Share of the surface that has a crust, 0 to 1.
    pub coverage: f64,
    /// How strongly molten rock lights the crust beside it, 0 to 1.
    pub glow: f64,
    /// Read low places as hot (valleys and fissures) rather than steep ones.
    pub fissures: bool,
    pub crust: [u8; 3],
    pub molten: [u8; 3],
    pub hot: [u8; 3],
    /// Height of the relief in blocks.
    pub relief: i32,
    pub foundation: String,
    pub glass_layers: u32,
    pub max_solids: usize,
    pub max_glass: usize,
    pub dither: f64,
}

impl LavaSettings {
    pub fn from_params(values: &ParamValues) -> Self {
        let get = |id: &str| params::find(id).map(|d| values.get(d));
        let num = |id: &str| match get(id) {
            Some(Value::Float(x)) => x,
            Some(Value::Int(i)) => i as f64,
            _ => 0.0,
        };
        let choice = |id: &str| match get(id) {
            Some(Value::Choice(c)) => c,
            _ => String::new(),
        };
        let colour = |id: &str| params::parse_colour(&choice(id)).unwrap_or([0; 3]);
        Self {
            origin: [0, 0],
            width: num("lava.width") as i32,
            length: num("lava.length") as i32,
            placement_y: num("lava.placement_y") as i32,
            pattern: match choice("lava.pattern").as_str() {
                "ropes" => Pattern::Ropes,
                "pools" => Pattern::Pools,
                _ => Pattern::Plates,
            },
            scale: num("lava.map_scale").max(1.0),
            rotation: num("lava.rotation"),
            seed: num("lava.seed") as u32,
            distortion: num("lava.flow_distortion"),
            invert: matches!(get("lava.invert"), Some(Value::Bool(true))),
            coverage: num("lava.crust_coverage"),
            glow: num("lava.glow"),
            fissures: choice("lava.interpretation") == "fissures",
            crust: colour("lava.crust_colour"),
            molten: colour("lava.molten_colour"),
            hot: colour("lava.hot_colour"),
            relief: num("lava.relief") as i32,
            foundation: choice("lava.foundation_colour"),
            glass_layers: num("lava.glass_layers") as u32,
            max_solids: num("lava.max_solids") as usize,
            max_glass: num("lava.max_glass") as usize,
            dither: num("lava.dither_strength"),
        }
    }
}

/// The relief of the surface at a place of the pattern, 0 (low) to 1 (high).
fn relief(pattern: Pattern, u: f64, v: f64, seed: u32) -> f64 {
    let fine = 0.5 + 0.5 * fbm(u * 3.1, v * 3.1, seed ^ 0x51, 3);
    let broad = match pattern {
        Pattern::Plates => smoothstep(0.02, 0.3, cell_edge(u, v, seed)),
        Pattern::Ropes => (1.0 - 2.4 * fbm(u * 0.7, v * 2.6, seed ^ 0x77, 3).abs()).clamp(0.0, 1.0),
        Pattern::Pools => smoothstep(0.2, 0.8, 0.5 + 0.8 * fbm(u, v, seed ^ 0x99, 4)),
    };
    (0.8 * broad + 0.2 * fine).clamp(0.0, 1.0)
}

/// Soften a field over about `radius` cells.
fn softened(field: &[f32], w: usize, l: usize, radius: usize) -> Vec<f32> {
    let pass = |from: &[f32], along_x: bool| -> Vec<f32> {
        let mut out = vec![0f32; from.len()];
        let (lines, len) = if along_x { (l, w) } else { (w, l) };
        let at = |line: usize, k: usize| if along_x { line * w + k } else { k * w + line };
        for line in 0..lines {
            let mut sum = 0f32;
            let mut count = 0usize;
            for k in 0..len.min(radius + 1) {
                sum += from[at(line, k)];
                count += 1;
            }
            for k in 0..len {
                out[at(line, k)] = sum / count.max(1) as f32;
                if k + radius + 1 < len {
                    sum += from[at(line, k + radius + 1)];
                    count += 1;
                }
                if k >= radius {
                    sum -= from[at(line, k - radius)];
                    count -= 1;
                }
            }
        }
        out
    };
    let mut out = field.to_vec();
    for _ in 0..2 {
        out = pass(&pass(&out, true), false);
    }
    out
}

/// Bake a field of lava. A cancelled bake returns `of_formats::CANCELLED`.
pub fn bake_lava(name: &str, s: &LavaSettings, catalogue: &Catalogue, tuning: &Tuning, progress: &Progress) -> Result<Baked, String> {
    let (w, l) = (s.width, s.length);
    if w < 1 || l < 1 {
        return Err("the area must be at least one block each way".into());
    }
    let step = |fraction: f32| progress.step(fraction).map_err(|p| p.key.to_string());
    let cells = (w * l) as usize;
    let at = |x: i32, z: i32| (z * w + x) as usize;
    let inside = |x: i32, z: i32| x >= 0 && z >= 0 && x < w && z < l;

    // ---- The pattern: relief and heat at every column.
    let (sin, cos) = s.rotation.to_radians().sin_cos();
    let scale = s.scale.max(1.0);
    let bend = s.distortion.clamp(0.0, 1.0);
    let place = |x: f64, z: f64| -> (f64, f64) {
        let (wx, wz) = (s.origin[0] as f64 + x + 0.5, s.origin[1] as f64 + z + 0.5);
        ((wx * cos + wz * sin) / scale, (wz * cos - wx * sin) / scale)
    };
    let mut field = vec![(0f32, 0f32); cells];
    fill_rows(&mut field, w as usize, progress, (0.0, 0.45), &|z, out| {
        for (x, cell) in out.iter_mut().enumerate() {
            let (u, v) = place(x as f64, z as f64);
            // The flow bends the pattern and draws it out along itself.
            let pull = (bend * 0.9 * fbm(u * 0.6 + 7.3, v * 0.6, s.seed ^ 0xA1, 3), bend * 0.9 * fbm(u * 0.6, v * 0.6 - 4.1, s.seed ^ 0xA2, 3));
            let stretch = 1.0 + 1.5 * bend;
            let high = |du: f64, dv: f64| relief(s.pattern, (u + du + pull.0) / stretch, v + dv + pull.1, s.seed);
            let here = high(0.0, 0.0);
            let heat = if s.fissures {
                1.0 - here
            } else {
                let step = 1.0 / scale;
                let (dx, dz) = (high(step, 0.0) - here, high(0.0, step) - here);
                (dx.hypot(dz) * scale * 0.35).min(1.0)
            };
            *cell = (here as f32, if s.invert { 1.0 - heat } else { heat } as f32);
        }
    })?;
    // The crust covers the coldest share of the surface.
    let mut counts = vec![0u32; 1024];
    for (_, heat) in &field {
        counts[((*heat * 1023.0) as usize).min(1023)] += 1;
    }
    let part = |share: f64| -> f32 {
        let wanted = (share.clamp(0.0, 1.0) * cells as f64) as u64;
        let mut seen = 0u64;
        for (bin, n) in counts.iter().enumerate() {
            seen += *n as u64;
            if seen > wanted {
                return bin as f32 / 1023.0;
            }
        }
        1.0 + 1e-3
    };
    let (from, to) = (part(s.coverage), part(0.995).max(part(s.coverage) + 1e-3));
    let molten: Vec<f32> = field.iter().map(|(_, heat)| if s.coverage >= 1.0 { 0.0 } else { ((heat - from) / (to - from)).clamp(0.0, 1.0) }).collect();
    let lit = softened(&molten.iter().map(|m| if *m > 0.0 { 1.0 } else { 0.0 }).collect::<Vec<f32>>(), w as usize, l as usize, (scale * 0.05).clamp(1.0, 10.0) as usize);
    step(0.55)?;

    // ---- Shape: the relief in blocks, as a shell two blocks thick.
    let top: Vec<i32> = field.iter().map(|(high, _)| (s.relief.max(0) as f32 * high).round() as i32).collect();
    const AROUND: [(i32, i32); 8] = [(-1, 0), (1, 0), (0, -1), (0, 1), (-1, -1), (1, -1), (-1, 1), (1, 1)];
    let mut bottom = vec![0i32; cells];
    let mut lowest = i32::MAX;
    for z in 0..l {
        for x in 0..w {
            let mut low = top[at(x, z)];
            for (dx, dz) in AROUND {
                if inside(x + dx, z + dz) {
                    low = low.min(top[at(x + dx, z + dz)]);
                }
            }
            bottom[at(x, z)] = low - 1;
            lowest = lowest.min(low - 1);
        }
    }
    let sea_level = -lowest;

    // ---- Colour: crust, molten rock, and the glow between them.
    let paint = &tuning.colour;
    let (crust, fire, hottest) = (colour::to_oklab(s.crust), colour::to_oklab(s.molten), colour::to_oklab(s.hot));
    let mut targets: Vec<([f32; 3], f32)> = Vec::new();
    let mut index: HashMap<[i16; 3], u32> = HashMap::new();
    let mut target_of = vec![0u32; cells];
    for i in 0..cells {
        // The crust is a little paler where it stands high.
        let tone = lerp(0.8, 1.12, field[i].0 as f64) as f32;
        let mut lab = [crust[0] * tone, crust[1] * tone, crust[2] * tone];
        lab = colour::mix(lab, fire, (s.glow.clamp(0.0, 1.0) as f32 * 0.8 * lit[i]).min(0.85));
        if molten[i] > 0.0 {
            let m = molten[i] as f64;
            let rock = colour::mix(fire, hottest, smoothstep(0.25, 1.0, m) as f32);
            lab = colour::mix(lab, rock, smoothstep(0.0, 0.12, m) as f32);
        }
        let key = [(lab[0] * 160.0).round() as i16, (lab[1] * 160.0).round() as i16, (lab[2] * 160.0).round() as i16];
        let t = *index.entry(key).or_insert_with(|| {
            targets.push(([key[0] as f32 / 160.0, key[1] as f32 / 160.0, key[2] as f32 / 160.0], 0.0));
            targets.len() as u32 - 1
        });
        targets[t as usize].1 += 1.0;
        target_of[i] = t;
    }
    step(0.65)?;

    // ---- Blocks: the stacks nearest the colours, dithered between tones.
    let glass_layers = s.glass_layers.min(3);
    let matcher = Matcher::new(catalogue, glass_layers, paint.texture_most as f32, true, paint.glass_apart as f32);
    if matcher.solids.is_empty() {
        return Err("the block catalogue has no blocks suitable for a coloured surface".into());
    }
    let care = &Care { hue: paint.hue_care as f32, washed: paint.washed_care as f32, from: [paint.care_from[0] as f32, paint.care_from[1] as f32] };
    let pairs = matcher.choose(&targets, s.max_solids.max(1), s.max_glass.max(1), care, paint.block_gain as f32);
    step(0.85)?;
    let own: Vec<usize> = targets.iter().map(|(colour, _)| Matcher::nearest(&pairs, colour, care)).collect();
    let mut tones: Vec<f32> = own.iter().map(|p| pairs[*p].lab[0]).collect();
    tones.sort_by(|a, b| a.total_cmp(b));
    tones.dedup_by(|a, b| (*a - *b).abs() < 1e-4);
    const BAYER: [u8; 16] = [0, 8, 2, 10, 12, 4, 14, 6, 3, 11, 1, 9, 15, 7, 13, 5];
    let reach = (s.dither * paint.dither_reach).max(0.0) as f32;
    let mut pair_of = vec![0u32; cells];
    for i in 0..cells {
        let colour = targets[target_of[i] as usize].0;
        let mut chosen = own[target_of[i] as usize];
        let above = tones.partition_point(|tone| *tone < colour[0]);
        if reach > 0.0 && above > 0 && above < tones.len() {
            let gap = (tones[above] - tones[above - 1]).min(paint.dither_most.max(0.0) as f32);
            let (bx, bz) = (s.origin[0] + i as i32 % w, s.origin[1] + i as i32 / w);
            let fine = BAYER[(bz.rem_euclid(4) * 4 + bx.rem_euclid(4)) as usize] as f32 / 16.0;
            chosen = Matcher::nearest(&pairs, &[colour[0] + reach * gap * (fine - 0.47), colour[1], colour[2]], care);
        }
        pair_of[i] = chosen as u32;
    }
    let target = target_of.iter().map(|t| colour::from_oklab(targets[*t as usize].0)).collect();
    let matched = pair_of.iter().map(|p| colour::from_oklab(pairs[*p as usize].lab)).collect();
    step(0.9)?;

    let mut build = LayeredBuild::new(name);
    let mut used_ids: HashMap<u32, u64> = HashMap::new();
    let mut stats = Stats::default();
    let layer_index = |layer: Layer| Layer::ALL.iter().position(|l| *l == layer).unwrap();
    let foundation = build.palette.intern(BlockState::new(&format!("{}_concrete", s.foundation)));
    if !catalogue.blocks.contains_key(&build.palette.get(foundation).name) {
        return Err(format!("'{}' is not a concrete colour", s.foundation));
    }
    let solid_ids: Vec<u32> = matcher.solids.iter().map(|b| build.palette.intern(BlockState::new(&b.name))).collect();
    let glass_ids: Vec<u32> = matcher.glasses.iter().map(|b| build.palette.intern(BlockState::new(&b.name))).collect();
    const GLASS: [Layer; 3] = [Layer::Glass1, Layer::Glass2, Layer::Glass3];
    let mut range = (i32::MAX, i32::MIN);
    for z in 0..l {
        for x in 0..w {
            let i = at(x, z);
            range = (range.0.min(top[i]), range.1.max(top[i]));
            // The coloured blocks cover the top and any side that stands
            // above a neighbour; the edge of the area counts as level.
            let mut open_from = top[i];
            for (dx, dz) in &AROUND[..4] {
                if inside(x + dx, z + dz) {
                    open_from = open_from.min(top[at(x + dx, z + dz)] + 1);
                }
            }
            let pair = &pairs[pair_of[i] as usize];
            for level in bottom[i]..=top[i] {
                let (layer, id) = if level >= open_from { (Layer::Surface, solid_ids[pair.solid as usize]) } else { (Layer::Foundation, foundation) };
                build.layer_mut(layer).set([x, level + sea_level, z], id);
                *used_ids.entry(id).or_default() += 1;
                stats.blocks[layer_index(layer)] += 1;
            }
            for (k, layer) in GLASS.iter().enumerate().take(glass_layers as usize).filter(|(k, _)| pair.glass[*k] != NO_GLASS) {
                let id = glass_ids[pair.glass[k] as usize];
                build.layer_mut(*layer).set([x, top[i] + 1 + k as i32 + sea_level, z], id);
                *used_ids.entry(id).or_default() += 1;
                stats.blocks[layer_index(*layer)] += 1;
            }
        }
    }
    step(0.99)?;
    stats.water_columns = cells as u64;
    stats.range = range;
    let mut used: BTreeMap<String, u64> = BTreeMap::new();
    for (id, n) in used_ids {
        *used.entry(build.palette.get(id).name.clone()).or_default() += n;
    }
    stats.used = used.into_iter().collect();
    stats.used.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    Ok(Baked {
        blocks: crate::blocks::Blocks::Layers(build),
        sea_level,
        export_y: s.placement_y - sea_level,
        width: w,
        length: l,
        water_top: top.iter().map(|t| t + sea_level).collect(),
        water_bottom: bottom.iter().map(|b| b + sea_level).collect(),
        target,
        matched,
        maps: Default::default(),
        stats,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use of_formats::catalogue;

    fn settings() -> LavaSettings {
        let mut s = LavaSettings::from_params(&ParamValues::default());
        (s.width, s.length) = (96, 80);
        s
    }

    fn bake(s: &LavaSettings) -> Baked {
        bake_lava("lava", s, catalogue::default(), &Tuning::default(), &Progress::none()).unwrap()
    }

    /// How many columns were meant to be brighter than the crust.
    fn bright(b: &Baked, crust: [u8; 3]) -> usize {
        let dark = colour::to_oklab(crust)[0] * 1.5 + 0.08;
        b.target.iter().filter(|c| colour::to_oklab(**c)[0] > dark).count()
    }

    #[test]
    fn lava_bakes_the_same_every_time_and_fills_the_area() {
        let s = settings();
        let (a, b) = (bake(&s), bake(&s));
        assert_eq!(a.target, b.target);
        assert_eq!(a.matched, b.matched);
        assert_eq!(a.stats.water_columns, 96 * 80);
        assert_eq!(a.stats.used, b.stats.used);
        // Every column has a coloured top, and nothing hangs in the air.
        assert!(a.stats.blocks[Layer::ALL.iter().position(|l| *l == Layer::Surface).unwrap()] >= 96 * 80);
        assert!(a.water_bottom.iter().zip(&a.water_top).all(|(b, t)| b < t));
    }

    #[test]
    fn more_crust_leaves_less_molten_rock_and_the_relief_sets_the_height() {
        let mut s = settings();
        s.glow = 0.0;
        s.coverage = 0.2;
        let open = bright(&bake(&s), s.crust);
        s.coverage = 0.9;
        let closed = bright(&bake(&s), s.crust);
        s.coverage = 1.0;
        let none = bright(&bake(&s), s.crust);
        assert!(open > 2 * closed && closed > none, "{open} {closed} {none}");
        assert_eq!(none, 0);
        s.relief = 0;
        assert_eq!(bake(&s).stats.range, (0, 0));
        s.relief = 6;
        let tall = bake(&s).stats.range;
        assert!(tall.1 - tall.0 >= 4, "{tall:?}");
    }

    #[test]
    fn each_pattern_and_reading_gives_a_different_field() {
        let mut s = settings();
        let plates = bake(&s).target;
        s.pattern = Pattern::Ropes;
        let ropes = bake(&s).target;
        s.fissures = !s.fissures;
        let low = bake(&s).target;
        s.invert = true;
        let swapped = bake(&s).target;
        assert!(plates != ropes && ropes != low && low != swapped);
    }
}
