//! Turn the wave shape into blocks: a concrete foundation, a coloured
//! coating on the faces that show from above, and stained glass on top.

use crate::blocks::{Blocks, Columns, Land, Reworked, Stack};
use crate::colour::{self, Care, Matcher, NO_GLASS};
use crate::flow::Flow;
use crate::noise::{cell_edge, fbm, hash, lerp, smoothstep, value};
use crate::shore::Shore;
use crate::survey::Survey;
use crate::terrain::{Ground, Terrain};
use crate::tuning::Tuning;
use crate::waves::{Near, WaveField};
use crate::zone::Zones;
use of_formats::catalogue::Catalogue;
use of_formats::export::Layer;
use of_formats::model::{BlockState, Palette};
use of_formats::Progress;
use std::collections::{BTreeMap, HashMap};

use crate::look::*;
use crate::settings::*;
pub use crate::baked::{Baked, Maps, Stats, DRY, MOST_HEIGHT};
pub(crate) use crate::baked::{height_reached, tall_enough};
const BAYER: [u8; 16] = [0, 8, 2, 10, 12, 4, 14, 6, 3, 11, 1, 9, 15, 7, 13, 5];

/// Fill `out` one row at a time on every processor core, reporting progress
/// between bands of rows. `fill` gets the row number and that row's cells.
pub(crate) fn fill_rows<T: Send>(
    out: &mut [T],
    row_len: usize,
    progress: &Progress,
    span: (f32, f32),
    fill: &(dyn Fn(usize, &mut [T]) + Sync),
) -> Result<(), String> {
    let rows = out.len() / row_len.max(1);
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get());
    let band = (rows / 24).max(threads).max(1);
    let mut done = 0;
    for part in out.chunks_mut(band * row_len) {
        let part_rows = part.len() / row_len;
        let each = part_rows.div_ceil(threads).max(1);
        std::thread::scope(|scope| {
            for (i, rows) in part.chunks_mut(each * row_len).enumerate() {
                let first = done + i * each;
                scope.spawn(move || {
                    for (r, row) in rows.chunks_mut(row_len).enumerate() {
                        fill(first + r, row);
                    }
                });
            }
        });
        done += part_rows;
        step(progress, span.0 + (span.1 - span.0) * done as f32 / rows as f32)?;
    }
    Ok(())
}

fn step(progress: &Progress, fraction: f32) -> Result<(), String> {
    progress.step(fraction).map_err(|p| p.key.to_string())
}

/// How many standard deviations above the mean a share `part` of a normal
/// distribution lies (Abramowitz and Stegun 26.2.23).
fn tail_point(part: f64) -> f64 {
    let t = (-2.0 * part.clamp(1e-12, 0.5).ln()).sqrt();
    t - (2.515_517 + 0.802_853 * t + 0.010_328 * t * t) / (1.0 + 1.432_788 * t + 0.189_269 * t * t + 0.001_308 * t * t * t)
}

/// Bake with the standard tuning and no progress reports.
pub fn bake(name: &str, s: &OceanSettings, terrain: Option<&Terrain>, catalogue: &Catalogue) -> Result<Baked, String> {
    bake_with(name, s, terrain, catalogue, &Tuning::default(), &Progress::none())
}

/// Bake an ocean. A cancelled bake returns `of_formats::CANCELLED` as its error.
pub fn bake_with(
    name: &str,
    s: &OceanSettings,
    terrain: Option<&Terrain>,
    catalogue: &Catalogue,
    tuning: &Tuning,
    progress: &Progress,
) -> Result<Baked, String> {
    let (w, l) = (s.width, s.length);
    if w < 1 || l < 1 {
        return Err("the area must be at least one block each way".into());
    }
    if let Some(t) = terrain {
        if t.width != w || t.length != l {
            return Err(format!("the terrain is {} x {} but the area is {w} x {l}", t.width, t.length));
        }
    }
    let field = WaveField::with(&s.waves, &tuning.waves);
    let zones = Zones::new(&s.zones, w, l);
    let cells = (w * l) as usize;
    let at = |x: i32, z: i32| (z * w + x) as usize;
    let inside = |x: i32, z: i32| x >= 0 && z >= 0 && x < w && z < l;
    let world = |x: i32, z: i32| ((s.origin[0] + x) as f64 + 0.5, (s.origin[1] + z) as f64 + 0.5);

    // Near land the waves slow, bend, find shelter and break.
    // A current through the sea, and what the land does to it.
    let flow = match terrain {
        Some(t) => Flow::new(t, s.flow_speed, s.flow_direction, s.metres_per_block, &tuning.flow, s.look.wake_length, s.look.wake_scale, progress, (0.0, 0.04))?,
        None => None,
    };
    let flow = flow.as_ref();
    let shore = match terrain {
        Some(t) => {
            Some(Shore::new(t, &field, s.waves.height, &tuning.near_land, &tuning.outflow, flow, s.metres_per_block, progress, (0.04, 0.15))?)
        }
        None => None,
    };
    let shore = shore.as_ref();
    // Which shore is beach, which rocky and which cliff, read from the
    // shape of the ground: waves do different things on each.
    let survey = terrain.map(Survey::new);
    let survey = survey.as_ref();
    let echo = &tuning.reflection;
    let echo_reach = echo.reach.max(0.01) * field.peak_length();
    // The height of the water and the same with its short waves smoothed away.
    let surface_at = |x: i32, z: i32| -> (f64, f64, f64, f64) {
        let (wx, wz) = world(x, z);
        let near = shore.map_or(Near::OPEN, |s| s.near(x, z));
        let water = field.sample(wx, wz, &near);
        let (mut height, mut smooth) = (water.height, water.smooth);
        // What the shores throw back, kept apart until it can be spread over
        // the blocks the water has room to carry it in.
        let (mut echo_height, mut echo_smooth) = (0.0, 0.0);
        // A steep shore throws waves back. The wave that returns from a
        // shore is the arriving wave as it would be at the same distance
        // behind the shore, so that is where it is looked up.
        if let Some((sv, (way, firm))) = survey.and_then(|sv| sv.mirror(x, z).map(|m| (sv, m))) {
            let (cliff, rocky) = sv.character(x, z);
            let share = cliff * echo.cliff + rocky * echo.rocky + (1.0 - cliff - rocky).max(0.0) * echo.beach;
            let thrown = share.clamp(0.0, 1.0) * firm * (-sv.off(x, z).max(0.0) / echo_reach).exp();
            if thrown > 0.02 {
                let back = field.sample(wx + way.0, wz + way.1, &near);
                echo_height = thrown * back.height;
                echo_smooth = thrown * back.smooth;
                height += echo_height;
                smooth += echo_smooth;
            }
        }
        // Water piled up and drawn down by a current, and the standing
        // waves it leaves behind land.
        if let Some(f) = flow {
            height += f.lift(x, z);
            smooth += f.lift(x, z);
        }
        // Calm water keeps only part of its waves.
        if !zones.is_empty() {
            let left = zones.calm(x, z);
            height *= left;
            smooth *= left;
            echo_height *= left;
            echo_smooth *= left;
        }
        (height, smooth, echo_height, echo_smooth)
    };
    // Worked out once for every column and a one-block rim, which the
    // slopes at the edge of the plot need.
    let rim = (w + 2) as usize;
    let mut heights = of_core::memory::filled(rim * (l + 2) as usize, (0f64, 0f64, 0f64, 0f64))?;
    fill_rows(&mut heights, rim, progress, (0.15, 0.45), &|row, cells| {
        for (i, cell) in cells.iter_mut().enumerate() {
            *cell = surface_at(i as i32 - 1, row as i32 - 1);
        }
    })?;
    // A wave thrown back from a far shore comes home with crests closer
    // together than a block: the column of sea it is looked up behind moves
    // by blocks between one column and the next, and the wave it finds
    // there turns over many times over that distance. Drawn as it is, that
    // is a crease of light lying across the waves wherever the two happen
    // to come out of step. What the water can carry of it is the stretched
    // out shape, so it is spread over a few blocks before it is added.
    {
        let spread = tuning.reflection.soften.max(0.0).round() as i32;
        if spread > 0 {
            let (wsize, lsize) = (rim as i32, (l + 2) as i32);
            let mut across = vec![(0f64, 0f64); heights.len()];
            for z in 0..lsize {
                for x in 0..wsize {
                    let (mut sh, mut ss, mut n) = (0f64, 0f64, 0f64);
                    for d in -spread..=spread {
                        let j = (z * wsize + (x + d).clamp(0, wsize - 1)) as usize;
                        sh += heights[j].2;
                        ss += heights[j].3;
                        n += 1.0;
                    }
                    across[(z * wsize + x) as usize] = (sh / n, ss / n);
                }
            }
            for z in 0..lsize {
                for x in 0..wsize {
                    let (mut sh, mut ss, mut n) = (0f64, 0f64, 0f64);
                    for d in -spread..=spread {
                        let j = ((z + d).clamp(0, lsize - 1) * wsize + x) as usize;
                        sh += across[j].0;
                        ss += across[j].1;
                        n += 1.0;
                    }
                    let k = (z * wsize + x) as usize;
                    heights[k].0 += sh / n - heights[k].2;
                    heights[k].1 += ss / n - heights[k].3;
                }
            }
        }
    }
    let surface = |x: i32, z: i32| heights[(z + 1) as usize * rim + (x + 1) as usize].0;
    let smooth = |x: i32, z: i32| heights[(z + 1) as usize * rim + (x + 1) as usize].1;


    // ---- Shape: where the water stands, relative to the resting level.
    let mut top = vec![DRY; cells];
    let mut ground = vec![DRY; cells];
    let mut spray = vec![0f32; cells];
    for z in 0..l {
        for x in 0..w {
            let i = at(x, z);
            let mut level = surface(x, z).round() as i32;
            // A wave breaking on the shore throws up a crest taller than
            // the swell it came from: the white water leaps as it meets
            // the rock, instead of only lying on the surface.
            let mut splash = 0.0f64;
            if let Some(sh) = shore {
                let hit = sh.surf(x, z) as f64;
                if hit > 0.02 && s.look.spray > 0.0 && surface(x, z) > 0.0 {
                    let (wx, wz) = world(x, z);
                    let grain = 0.5 + 0.5 * crate::noise::fbm(wx / 5.0, wz / 5.0, s.waves.seed ^ 0x5EA1, 2);
                    splash = (s.look.spray.clamp(0.0, 1.0) * hit * grain * 2.0).clamp(0.0, 1.0);
                    level += (splash * 2.5).round() as i32;
                }
            }
            spray[i] = splash as f32;
            // Under an arch the water and its glass stay clear of the rock.
            if let Some((lowest, _)) = terrain.and_then(|t| t.roof(x, z)) {
                level = level.min(lowest - 4);
            }
            ground[i] = terrain.and_then(|t| t.top(x, z)).unwrap_or(DRY);
            if level > ground[i] && !zones.dry(x, z) {
                top[i] = level;
            }
        }
    }
    // Water stranded on the land by a passing crest is not part of the sea:
    // keep the largest body and anything that runs off the edge of the plot.
    if terrain.is_some() {
        let mut body = vec![0u32; cells];
        let mut bodies: Vec<(usize, bool)> = vec![(0, false)];
        for start in 0..cells {
            if top[start] == DRY || body[start] != 0 {
                continue;
            }
            let id = bodies.len() as u32;
            let (mut size, mut at_edge, mut queue) = (0, false, vec![start]);
            body[start] = id;
            while let Some(i) = queue.pop() {
                size += 1;
                let (x, z) = (i as i32 % w, i as i32 / w);
                at_edge |= x == 0 || z == 0 || x == w - 1 || z == l - 1;
                for (dx, dz) in [(-1, 0), (1, 0), (0, -1), (0, 1)] {
                    if inside(x + dx, z + dz) && top[at(x + dx, z + dz)] != DRY && body[at(x + dx, z + dz)] == 0 {
                        body[at(x + dx, z + dz)] = id;
                        queue.push(at(x + dx, z + dz));
                    }
                }
            }
            bodies.push((size, at_edge));
        }
        let largest = bodies.iter().map(|b| b.0).max().unwrap_or(0);
        for i in 0..cells {
            let (size, at_edge) = bodies[body[i] as usize];
            if top[i] != DRY && size < largest && !at_edge {
                top[i] = DRY;
            }
        }
    }
    // In the shallows a trough can dip below the sea bed and leave a patch
    // of it bare with water all round. The sea does not part like that: bed
    // that nowhere joins ground above the resting level keeps a sheet of water.
    // The sheet lies on the bed, so a column with no ground under it (beside
    // a build brought in as the land) has nothing to hold one, and a
    // keep-dry area stays dry.
    if terrain.is_some() {
        let mut seen = vec![false; cells];
        for start in 0..cells {
            if top[start] != DRY || seen[start] {
                continue;
            }
            let (mut patch, mut queue, mut land) = (Vec::new(), vec![start], false);
            seen[start] = true;
            while let Some(i) = queue.pop() {
                patch.push(i);
                land |= ground[i] >= 0;
                let (x, z) = (i as i32 % w, i as i32 / w);
                for (dx, dz) in [(-1, 0), (1, 0), (0, -1), (0, 1)] {
                    if inside(x + dx, z + dz) && top[at(x + dx, z + dz)] == DRY && !seen[at(x + dx, z + dz)] {
                        seen[at(x + dx, z + dz)] = true;
                        queue.push(at(x + dx, z + dz));
                    }
                }
            }
            if !land {
                for i in patch {
                    let (x, z) = (i as i32 % w, i as i32 / w);
                    if ground[i] != DRY && !zones.dry(x, z) {
                        top[i] = ground[i] + 1;
                    }
                }
            }
        }
    }
    const AROUND: [(i32, i32); 8] = [(-1, 0), (1, 0), (0, -1), (0, 1), (-1, -1), (1, -1), (-1, 1), (1, 1)];
    let wave_base = field.lowest().floor() as i32 - s.thickness;
    let base = terrain.map_or(wave_base, |t| t.floor.min(wave_base));
    let mut bottom = vec![DRY; cells];
    let mut lowest = terrain.filter(|_| ground.iter().any(|g| *g != DRY)).map_or(i32::MAX, |t| t.floor);
    for z in 0..l {
        for x in 0..w {
            let i = at(x, z);
            if top[i] == DRY {
                continue;
            }
            // Reach down to the lowest neighbour, so the shell has no holes
            // on steep faces, then add the thickness below that.
            let mut low = top[i];
            for (dx, dz) in AROUND {
                if inside(x + dx, z + dz) && top[at(x + dx, z + dz)] != DRY {
                    low = low.min(top[at(x + dx, z + dz)]);
                }
            }
            let mut b = if s.filled { base } else { low.saturating_sub(s.thickness - 1) };
            if ground[i] != DRY {
                b = b.max(ground[i].saturating_add(1));
            }
            bottom[i] = b.min(top[i]);
            lowest = lowest.min(bottom[i]);
        }
    }
    if lowest == i32::MAX {
        return Err("there is nothing to build: no water and no terrain".into());
    }
    // Before anything is placed, the build must fit in a Minecraft world.
    // A height gone wrong would otherwise fill memory one block at a time.
    let highest = height_reached(&top, &ground, terrain);
    tall_enough(lowest, highest)?;
    let sea_level = -lowest;

    let mut palette = Palette::default();
    let mut stats = Stats::default();
    let layer_index = |layer: Layer| Layer::ALL.iter().position(|l| *l == layer).unwrap();

    // ---- Terrain passes through untouched; water only ever goes where it is not.
    // Only how to make its blocks is kept (see `blocks.rs`), not the blocks.
    let land = if let Some(placed) = terrain.and_then(|t| t.placed.as_ref()) {
        // A build brought in from a file is copied block for block.
        let ids: Vec<Option<u32>> = placed.build.palette.states().iter().map(|state| (!state.is_air()).then(|| palette.intern(state.clone()))).collect();
        // Whether the build has a block at a column and level.
        let built = |x: i32, z: i32, level: i32| {
            let p = [x - placed.shift[0], level - placed.shift[1], z - placed.shift[2]];
            placed.build.regions.iter().any(|r| r.contains(p) && r.cells.get(p).is_some_and(|v| v != 0 && ids[v as usize - 1].is_some()))
        };
        // Where rivers and ponds changed a column, the build is cut away
        // above the ground they left, and under their water; the water is
        // put in. Elsewhere the build stands as it is.
        let t = terrain.expect("a placed build comes with its ground");
        let mut reworked = Reworked::default();
        let mut ids_of: [Option<u32>; Ground::ALL.len()] = [None; Ground::ALL.len()];
        for &column in &placed.reworked {
            let i = column as usize;
            let (x, z) = (i as i32 % w, i as i32 / w);
            let top = ground[i];
            let from = reworked.runs.len() as u32;
            let mut clear = if top == DRY { i32::MIN } else { top + 1 };
            if top != DRY {
                let mut level = top;
                while level >= t.floor {
                    let kind = t.block_at(x, z, level);
                    if !matches!(kind, Ground::Stream | Ground::Fall | Ground::Pond) {
                        break;
                    }
                    // Falling water runs over the build's face and through
                    // the air in front of it, never down through its inside.
                    if kind == Ground::Fall && level < top - 1 && built(x, z, level) {
                        break;
                    }
                    let slot = Ground::ALL.iter().position(|k| *k == kind).unwrap();
                    let id = *ids_of[slot].get_or_insert_with(|| palette.intern(BlockState::new(t.name_of(kind))));
                    let y = level + sea_level;
                    let own = reworked.runs.len() as u32 > from;
                    match reworked.runs.last_mut() {
                        Some(run) if own && run.0 == y + 1 && run.2 == id => run.0 = y,
                        _ => reworked.runs.push((y, y, id)),
                    }
                    clear = level;
                    level -= 1;
                }
            }
            let clear = if clear == i32::MIN { i32::MIN } else { clear + sea_level };
            reworked.cols.push((column, clear, from, reworked.runs.len() as u32));
        }
        Land::Placed { build: placed.build.clone(), ids, shift: placed.shift, reworked }
    } else if let Some(t) = terrain {
        // Only the kinds of ground that occur are added to the block list.
        let mut ids: [Option<u32>; Ground::ALL.len()] = [None; Ground::ALL.len()];
        let mut starts: Vec<u32> = Vec::with_capacity(cells + 1);
        let mut runs: Vec<(i32, i32, u32)> = Vec::new();
        for z in 0..l {
            for x in 0..w {
                let first = runs.len();
                starts.push(first as u32);
                let mut place = |level: i32, ground: Ground| {
                    let slot = Ground::ALL.iter().position(|k| *k == ground).unwrap();
                    let id = *ids[slot].get_or_insert_with(|| palette.intern(BlockState::new(t.name_of(ground))));
                    let y = level + sea_level;
                    let own = runs.len() > first;
                    match runs.last_mut() {
                        Some(run) if own && run.2 == id && run.1 + 1 == y => run.1 = y,
                        _ => runs.push((y, y, id)),
                    }
                };
                if ground[at(x, z)] != DRY {
                    for level in t.floor..=ground[at(x, z)] {
                        place(level, t.block_at(x, z, level));
                    }
                }
                // Rock that spans over the water.
                if let Some((from, to)) = t.roof(x, z) {
                    for level in from.max(ground[at(x, z)].saturating_add(1))..=to {
                        place(level, Ground::Rock);
                    }
                }
            }
        }
        starts.push(runs.len() as u32);
        Land::Ground { starts, runs }
    } else {
        Land::None
    };

    // ---- Appearance: a target colour for every water column.
    let look = &s.look;
    let glass_layers = if s.colour_glass { look.glass_layers.min(3) } else { 0 };
    let mut pair_of: Vec<u32> = vec![u32::MAX; cells];
    let mut pairs = Vec::new();
    let mut maps = Maps::default();
    let (mut target, mut matched): (Vec<[u8; 3]>, Vec<[u8; 3]>) = (Vec::new(), Vec::new());
    // With two or three layers of glass and `layer_blend` on, the stacks are
    // chosen with glass of one colour and the layers blended along the
    // borders afterwards; with it off, a stack may mix colours outright.
    let blend = glass_layers >= 2 && tuning.colour.layer_blend >= 0.5;
    let matcher =
        Matcher::with_tier(catalogue, glass_layers, tuning.colour.texture_most as f32, !blend, tuning.colour.glass_apart as f32, !s.creative_blocks).without(&s.excluded);
    if s.colour_glass {
        if matcher.solids.is_empty() {
            return Err("the block catalogue has no blocks suitable for a coloured surface".into());
        }
        let (az, el) = (look.sun_azimuth.to_radians(), look.sun_elevation.to_radians());
        let sun = [el.cos() * az.cos(), el.sin(), el.cos() * az.sin()];
        let paint = &tuning.colour;
        // The onlooker for whom the reflections are worked out.
        let (az, el) = (look.view_azimuth.to_radians(), look.view_elevation.to_radians());
        let view = [el.cos() * az.cos(), el.sin(), el.cos() * az.sin()];
        // Water has little colour of its own: what is seen is daylight that
        // went in and was scattered back out. With less light the same water
        // is darker, whatever its colour.
        let scaled = |c: [f32; 3], by: f64| [c[0] * by as f32, c[1] * by as f32, c[2] * by as f32];
        let dim = lerp(paint.darkest.clamp(0.0, 1.0), 1.0, look.daylight.clamp(0.0, 1.0));
        let white_dim = 1.0 - paint.white_dims.clamp(0.0, 1.0) * (1.0 - dim);
        // The water runs between two chosen colours: the dark one where
        // little light reaches it, the light one where the light catches it.
        // Either may be any colour at all, black included.
        let (dark_lab, light_lab) = (scaled(colour::to_oklab(look.dark), dim), scaled(colour::to_oklab(look.light), dim));
        // The tone of a column runs from 0, the dark colour, to 2, the
        // light colour; the blocks pass from the one to the other.
        let toned = move |tone: f64| colour::mix(dark_lab, light_lab, (0.5 * tone).clamp(0.0, 1.0) as f32);
        // Two bright colours leave little between them to shape the waves
        // with, and the sea looks pale and flat. Where the dark colour is
        // itself bright, faces turned from the sun go deeper than it:
        // darker, and with more colour.
        let deeper = paint.bright_depth.clamp(0.0, 0.9) * smoothstep(paint.bright_from[0], paint.bright_from[1].max(paint.bright_from[0] + 1e-6), colour::to_oklab(look.dark)[0] as f64);
        let foam_lab = scaled(colour::to_oklab(look.foam_colour), white_dim);
        // What the surface mirrors: the light colour made paler, or a
        // colour of its own.
        let mirrored = if look.reflection_auto {
            // Paler, and with less colour than paleness alone would
            // leave: a sky is greyer than the water under it.
            let pale = colour::mix(colour::to_oklab(look.light), colour::to_oklab([255; 3]), paint.reflection_white.clamp(0.0, 1.0) as f32);
            let kept = paint.reflection_tint.clamp(0.0, 1.0) as f32;
            [pale[0], pale[1] * kept, pale[2] * kept]
        } else {
            colour::to_oklab(look.reflection_colour)
        };
        let reflection_lab = scaled(mirrored, white_dim);
        // The sea floor, where shallow water lets it show.
        let beds: [Option<[f32; 3]>; Ground::ALL.len()] = match terrain {
            Some(t) => Ground::ALL.map(|g| {
                if matches!(g, Ground::Stream | Ground::Fall | Ground::Pond) {
                    return None;
                }
                catalogue.blocks.get(&BlockState::new(t.name_of(g)).name).and_then(|b| b.top).map(colour::to_oklab)
            }),
            None => [None; Ground::ALL.len()],
        };
        // The slope that mirrors the sun to the onlooker, and how far from
        // it a glint still shows.
        let half = [sun[0] + view[0], sun[1] + view[1], sun[2] + view[2]];
        let glint_at = if half[1] > 0.05 { Some((-half[0] / half[1], -half[2] / half[1])) } else { None };
        let glint_spread = lerp(paint.glint_spread[0], paint.glint_spread[1], look.reflection_softness.clamp(0.0, 1.0)).max(0.005);
        // How much of what is seen on a face is mirrored sky rather than
        // light from within the water, by the angle it is seen at.
        let sky_ratio = paint.sky_ratio.max(0.0);
        let sky_share = move |cos: f64| {
            let mirrored = 0.02 + 0.98 * (1.0 - cos).powi(5);
            mirrored * sky_ratio / (mirrored * sky_ratio + 1.0 - mirrored)
        };
        let level_sheen = sky_share(view[1].clamp(0.0, 1.0));
        // The tone of level water, of the deepest trough and of the thinnest crest.
        let level = 2.0 * look.midpoint.clamp(0.0, 1.0);
        let (lowest_tone, highest_tone) = (level * paint.tone_reach[0].clamp(0.0, 1.0), (level + paint.tone_reach[1].max(0.0)).min(2.0));
        let patch_size = [paint.patch_size[0].max(1.0), paint.patch_size[1].max(1.0)];
        let seed = s.waves.seed;
        let (ripples, edge, caps) = (&tuning.ripples, &tuning.shore_foam, &tuning.whitecaps);
        let ripple_size = [ripples.size[0].max(0.5), ripples.size[1].max(0.5)];
        // A crest breaks where the water is steeper than this. "Foam amount"
        // moves the threshold as well as the whiteness.
        let soft = caps.softness.max(1e-6);
        let ride = caps.short_wave_share.clamp(0.0, 1.0);
        // A sea steeper than any real one still breaks only in part.
        let deviation = field.steepness_deviation(ride);
        let most = tail_point(caps.most_breaking.clamp(1e-6, 0.5)) * deviation;
        // The taller the waves, the more of the sea is white: the wind that
        // raised them is still blowing on them. A large sea breaks for its
        // share of that cover even where its waves are not steep enough by
        // the rule, though never waves that are not steep at all.
        let spent = &tuning.spent_foam;
        let cover = (spent.cover_per_height.max(0.0) * (s.waves.height * s.metres_per_block).max(0.0).powf(spent.cover_power)).min(spent.cover_most.clamp(0.0, 1.0));
        let windy = if look.streaks > 0.0 { (tail_point((cover * spent.breaking_share * look.streaks.min(1.0) * 2.0).clamp(1e-6, 0.5)) * deviation).max(spent.steepness_floor) } else { f64::MAX };
        let breaks_at = caps.breaking_steepness.max(0.01).min(windy).max(most) * lerp(1.0 + caps.foam_amount_sway, 1.0 - caps.foam_amount_sway, look.foam.clamp(0.0, 1.0));
        let heading = field.heading();
        // Ripples from the library or an image, in place of the wind's.
        let pattern = s.pattern.ready(seed, heading);
        let pattern_share = ripples.pattern_share.max(0.0);
        // A made-up sea floor, only where there is no real one. It falls
        // away from the edge of the plot that is shallowest.
        let seabed = s.seabed.filter(|_| terrain.is_none());
        let bed_depth = seabed.map(|b| {
            let (sn, cs) = b.direction.to_radians().sin_cos();
            let along = move |(wx, wz): (f64, f64)| wx * cs + wz * sn;
            let start = [world(0, 0), world(w - 1, 0), world(0, l - 1), world(w - 1, l - 1)].map(along).into_iter().fold(f64::MAX, f64::min);
            move |wx: f64, wz: f64| b.shallowest + b.slope * (along((wx, wz)) - start).max(0.0)
        });
        let break_depth = ripples.seabed_break_depth.max(0.01) * s.waves.height.max(0.5);
        let streak =[paint.foam_streak_size[0].max(0.5), paint.foam_streak_size[1].max(0.5)];
        // Foam fades over `trail_time`; older than three times that it is gone.
        let fade = caps.trail_time.max(1e-3);
        let moments = caps.trail_samples.min(96) as usize;
        // A sea too gentle to break anywhere is not searched for foam.
        let can_break = look.foam > 0.0 && (terrain.is_some() || seabed.is_some() || field.steepest() >= 0.8 * (breaks_at - soft));
        let reach = (edge.sand_reach.max(edge.rock_reach).max(edge.cliff_reach) * (1.0 + edge.height_sway.max(0.0))).ceil().clamp(1.0, 8.0) as i32;
        let top = &top;
        // ---- Foam left lying on the water. A breaker leaves a sheet of
        // foam that thins into lace and is gone after a wave or two; the
        // wind draws what is left out into long streaks. Both are found by
        // asking the sea where it was breaking before, as for the trail, but
        // farther back, and so only every few blocks.
        let every = spent.step.clamp(1, 16) as i32;
        let (cw, cl) = (((w + every - 1) / every + 1) as usize, ((l + every - 1) / every + 1) as usize);
        // Per place: breaking now, the lace, the streaks, and whether it is water.
        let mut left = vec![[0f32; 4]; cw * cl];
        let left_share = (cover * 4.0 * look.foam.clamp(0.0, 1.0) * look.streaks.clamp(0.0, 1.0)).min(spent.cover_most.clamp(0.0, 1.0));
        let lace_size = [spent.lace_size[0].max(1.0), spent.lace_size[1].max(1.0)];
        let streak_size = [spent.streak_size[0].max(1.0), spent.streak_size[1].max(1.0)];
        let (lace_from, streak_from) = (spent.lace_from.clamp(0.0, 0.95), spent.streak_from.clamp(0.0, 0.95));
        let (lace_width, streak_width) = (spent.lace_width.max(0.01), spent.streak_width.max(0.01));
        // How much foam it takes to turn a block white: little on the
        // threads of the lace and on the lines of the streaks, a full sheet
        // between them.
        // The lace is a mesh of threads around open cells, with a finer
        // mesh inside it that only thicker foam fills; neither is regular.
        let lace_needs = move |along: f64, across: f64| {
            let (u, v) = (along / lace_size[0], across / lace_size[1]);
            let (u, v) = (u + 0.35 * value(1.7 * u, 1.7 * v, seed ^ 0x3ACE), v + 0.35 * value(1.7 * u, 1.7 * v, seed ^ 0x4ACE));
            let broad = cell_edge(u, v, seed ^ 0x1ACE);
            let fine = cell_edge(2.2 * u, 2.2 * v, seed ^ 0x2ACE);
            lerp(lace_from, 1.0, (broad.min(0.2 + 1.2 * fine) / lace_width).min(1.0))
        };
        // The streaks are slivers drawn out along the wind, each tapering
        // to nothing, and they waver a little from side to side.
        let streak_needs = move |along: f64, across: f64| {
            let across = across + spent.streak_waver * value(along / (1.5 * streak_size[0]), across / (6.0 * streak_size[1]), seed ^ 0x58EA);
            let sliver = fbm(along / streak_size[0], across / streak_size[1], seed ^ 0x57EA, 2);
            lerp(streak_from, 1.0, ((0.5 - sliver) / streak_width).clamp(0.0, 1.0))
        };
        let (mut lace_gain, mut streak_gain) = (0.0f64, 0.0f64);
        if can_break && left_share > 0.0 {
            let lives = [spent.life[0].max(1e-3), spent.life[1].max(1e-3)];
            let moments = spent.samples.clamp(1, 96);
            // Moments are taken thickest where the foam is freshest.
            let span = 1.0 - (-3.0f64).exp();
            fill_rows(&mut left, cw, &Progress::none(), (0.0, 0.0), &|row, out| {
                let z = (row as i32 * every).min(l - 1);
                for (n, cell) in out.iter_mut().enumerate() {
                    let x = (n as i32 * every).min(w - 1);
                    if top[at(x, z)] == DRY {
                        continue;
                    }
                    let (wx, wz) = world(x, z);
                    let arriving = shore.map_or(Near::OPEN, |s| s.near(x, z));
                    let from = breaks_at * (1.0 - caps.surf_ease.clamp(0.0, 0.95) * shore.map_or(0.0, |s| s.surf(x, z)));
                    let breaking = |ago: f64| smoothstep(from - soft, from + soft, field.steepness(wx, wz, ago, &arriving, ride));
                    let stagger = hash(s.origin[0] + x, s.origin[1] + z, seed ^ 0x5BE7) as f64 / (u32::MAX as f64 + 1.0);
                    let left = if zones.is_empty() { 1.0 } else { zones.calm(x, z) };
                    cell[0] = (breaking(0.0) * left) as f32;
                    for (k, life) in lives.iter().enumerate() {
                        let mut sum = 0.0;
                        for m in 0..moments {
                            let part = (m as f64 + stagger) / moments as f64;
                            sum += breaking(-life * (1.0 - part * span).ln());
                        }
                        cell[1 + k] = (sum / moments as f64 * left) as f32;
                    }
                    cell[3] = 1.0;
                }
            })?;
            // The lace is evened out a little; the streaks are drawn out
            // along the wind.
            let found = left.clone();
            let place = |x: f64, z: f64, k: usize| -> f64 {
                let (gx, gz) = (x.clamp(0.0, cw as f64 - 1.0), z.clamp(0.0, cl as f64 - 1.0));
                let (x0, z0) = ((gx.floor() as usize).min(cw - 2), (gz.floor() as usize).min(cl - 2));
                let (tx, tz) = (gx - x0 as f64, gz - z0 as f64);
                let (mut sum, mut all) = (0.0, 0.0);
                for (dx, dz, weight) in [(0, 0, (1.0 - tx) * (1.0 - tz)), (1, 0, tx * (1.0 - tz)), (0, 1, (1.0 - tx) * tz), (1, 1, tx * tz)] {
                    let c = found[(z0 + dz) * cw + x0 + dx];
                    sum += weight * (c[k] * c[3]) as f64;
                    all += weight * c[3] as f64;
                }
                if all > 1e-6 { sum / all } else { 0.0 }
            };
            let drawn = spent.streak_drawn.max(0.0) / every as f64;
            for gz in 0..cl {
                for gx in 0..cw {
                    let cell = &mut left[gz * cw + gx];
                    if cell[3] == 0.0 {
                        continue;
                    }
                    let (fx, fz) = (gx as f64, gz as f64);
                    let around = place(fx - 1.0, fz, 1) + place(fx + 1.0, fz, 1) + place(fx, fz - 1.0, 1) + place(fx, fz + 1.0, 1);
                    cell[1] = (0.5 * found[gz * cw + gx][1] as f64 + 0.125 * around) as f32;
                    let (mut sum, mut all) = (0.0, 0.0);
                    for n in -4..=4 {
                        let off = n as f64 / 4.0;
                        let weight = 1.0 - 0.8 * off.abs();
                        sum += weight * place(fx + heading.0 * drawn * off, fz + heading.1 * drawn * off, 2);
                        all += weight;
                    }
                    cell[2] = (sum / all) as f32;
                }
            }
            // Streaks gather foam from far around, so they lie more evenly
            // over the sea than the lace does.
            let wet = left.iter().filter(|c| c[3] > 0.0).count().max(1) as f64;
            let mean = left.iter().map(|c| (c[2] * c[3]) as f64).sum::<f64>() / wet;
            let even = spent.streak_even.clamp(0.0, 1.0);
            for cell in left.iter_mut() {
                cell[2] = lerp(cell[2] as f64, mean, even) as f32;
            }
            // The foam left is scaled so that it covers what remains of the
            // share of the sea that is white once the breaking crests have
            // had theirs.
            let wet: Vec<&[f32; 4]> = left.iter().filter(|c| c[3] > 0.0).collect();
            let active = wet.iter().map(|c| c[0] as f64).sum::<f64>() / wet.len().max(1) as f64;
            let to_cover = (left_share - active).max(0.0);
            let solve = |k: usize, needs: &dyn Fn(f64, f64) -> f64, share: f64| -> f64 {
                let mut needed: Vec<f32> = (0..96 * 96).map(|i| needs((i % 96) as f64 * 1.37, (i / 96) as f64 * 1.37) as f32).collect();
                needed.sort_by(|a, b| a.total_cmp(b));
                let covered = |gain: f64| wet.iter().map(|c| needed.partition_point(|t| (*t as f64) <= c[k] as f64 * gain)).sum::<usize>() as f64 / (wet.len().max(1) * needed.len()) as f64;
                let most = spent.gain_most.max(0.0);
                if share <= 0.0 || covered(most) <= share {
                    return if share > 0.0 { most } else { 0.0 };
                }
                let (mut low, mut high) = (0.0, most);
                for _ in 0..24 {
                    let mid = 0.5 * (low + high);
                    if covered(mid) < share {
                        low = mid
                    } else {
                        high = mid
                    }
                }
                0.5 * (low + high)
            };
            let lace_share = spent.lace_share.clamp(0.0, 1.0);
            lace_gain = solve(1, &lace_needs, to_cover * lace_share);
            streak_gain = solve(2, &streak_needs, to_cover * (1.0 - lace_share));
        }
        let left = &left;
        // The foam left at a column, filled in between the places worked out.
        let left_at = move |x: i32, z: i32, k: usize| -> f64 {
            let (gx, gz) = (x as f64 / every as f64, z as f64 / every as f64);
            let (x0, z0) = ((gx.floor() as usize).min(cw - 2), (gz.floor() as usize).min(cl - 2));
            let (tx, tz) = ((gx - x0 as f64).min(1.0), (gz - z0 as f64).min(1.0));
            let (mut sum, mut all) = (0.0, 0.0);
            for (dx, dz, weight) in [(0, 0, (1.0 - tx) * (1.0 - tz)), (1, 0, tx * (1.0 - tz)), (0, 1, (1.0 - tx) * tz), (1, 1, tx * tz)] {
                let c = left[(z0 + dz) * cw + x0 + dx];
                sum += weight * (c[k] * c[3]) as f64;
                all += weight * c[3] as f64;
            }
            if all > 1e-6 { sum / all } else { 0.0 }
        };
        // Falling water churns the sea white where it lands. Any ground
        // marked as a fall that stands beside the sea counts; the farther
        // the water drops, the wider the white water around its foot.
        let mut churned = vec![0f32; cells];
        if let Some(t) = terrain.filter(|_| look.foam > 0.0) {
            for z in 0..l {
                for x in 0..w {
                    if top[at(x, z)] != DRY || t.kind(x, z) != Ground::Fall {
                        continue;
                    }
                    let beside = [(-1, 0), (1, 0), (0, -1), (0, 1)]
                        .iter()
                        .filter(|(dx, dz)| inside(x + dx, z + dz))
                        .map(|(dx, dz)| top[at(x + dx, z + dz)])
                        .filter(|level| *level != DRY)
                        .min();
                    let Some(water) = beside else { continue };
                    let drop = (ground[at(x, z)] - water) as f64;
                    let reach = (edge.fall_reach.max(0.0) * drop).min(edge.fall_reach_most.max(0.0));
                    if reach < 1.0 {
                        continue;
                    }
                    let r = reach.ceil() as i32;
                    for nz in -r..=r {
                        for nx in -r..=r {
                            let away = ((nx * nx + nz * nz) as f64).sqrt() / reach;
                            if away < 1.0 && inside(x + nx, z + nz) {
                                let cell = &mut churned[at(x + nx, z + nz)];
                                *cell = cell.max((1.0 - away * away) as f32);
                            }
                        }
                    }
                }
            }
        }
        let churned = &churned;
        // Where the sun does not reach the water: behind land, under an
        // arch, behind a wave taller than the sun is high. Each column
        // looks toward the sun and is in shadow if anything stands in the way.
        let mut sunlit = vec![1f32; cells];
        let level_sun = (sun[0] * sun[0] + sun[2] * sun[2]).sqrt();
        if look.lit_strength > 0.0 && look.shadow > 0.0 && level_sun > 1e-6 {
            let (toward_x, toward_z, rise) = (sun[0] / level_sun, sun[2] / level_sun, sun[1] / level_sun);
            let ground = &ground;
            let roof = |x: i32, z: i32| terrain.and_then(|t| t.roof(x, z));
            let mut tallest = DRY;
            for z in 0..l {
                for x in 0..w {
                    tallest = tallest.max(top[at(x, z)]).max(ground[at(x, z)]).max(roof(x, z).map_or(DRY, |r| r.1));
                }
            }
            fill_rows(&mut sunlit, w as usize, progress, (0.45, 0.5), &|row, out| {
                let z = row as i32;
                for x in 0..w {
                    if top[at(x, z)] == DRY {
                        continue;
                    }
                    // Half a block clear of the water, so a level sea does
                    // not shade itself.
                    let from = top[at(x, z)] as f64 + 0.5;
                    let far = ((tallest as f64 - from) / rise).min(600.0);
                    let mut away = 1.0;
                    while away <= far {
                        let (px, pz) = ((x as f64 + 0.5 + toward_x * away).floor() as i32, (z as f64 + 0.5 + toward_z * away).floor() as i32);
                        if !inside(px, pz) {
                            break;
                        }
                        let ray = from + away * rise;
                        let solid = top[at(px, pz)].max(ground[at(px, pz)]);
                        let spanned = roof(px, pz).is_some_and(|(low, high)| ray > low as f64 - 1.0 && ray < high as f64);
                        if (solid != DRY && solid as f64 > ray) || spanned {
                            out[x as usize] = 0.0;
                            break;
                        }
                        away += 1.0;
                    }
                }
            })?;
            // Soft edges: every column takes the average of the water around it.
            let soft = look.shadow_softness.clamp(0.0, 12.0);
            if soft >= 0.5 {
                let reach = soft.ceil() as i32;
                let weights: Vec<f32> = (-reach..=reach).map(|k| (-0.5 * (k as f64 / (0.5 * soft)).powi(2)).exp() as f32).collect();
                for (ax, az) in [(1, 0), (0, 1)] {
                    let before = sunlit.clone();
                    for z in 0..l {
                        for x in 0..w {
                            if top[at(x, z)] == DRY {
                                continue;
                            }
                            let (mut sum, mut all) = (0f32, 0f32);
                            for (n, k) in (-reach..=reach).enumerate() {
                                let (px, pz) = (x + k * ax, z + k * az);
                                if inside(px, pz) && top[at(px, pz)] != DRY {
                                    sum += weights[n] * before[at(px, pz)];
                                    all += weights[n];
                                }
                            }
                            sunlit[at(x, z)] = sum / all;
                        }
                    }
                }
            }
        }
        let sunlit = &sunlit;
        // How steep the swell is, taken over the whole sea. The shading is
        // laid out for a sea of ordinary steepness; in a steeper one every
        // slope would be as dark or as light as the colours go, and the sea
        // a hard pattern of the two. There the shading is eased to suit.
        let (mut sum, mut wet, mut heights_sum, mut heights_squared) = (0.0f64, 0usize, 0.0f64, 0.0f64);
        for z in 0..l {
            for x in 0..w {
                if top[at(x, z)] != DRY {
                    let (dx, dz) = ((smooth(x + 1, z) - smooth(x - 1, z)) * 0.5, (smooth(x, z + 1) - smooth(x, z - 1)) * 0.5);
                    sum += dx * dx + dz * dz;
                    heights_sum += smooth(x, z);
                    heights_squared += smooth(x, z).powi(2);
                    wet += 1;
                }
            }
        }
        let steepness = (sum / wet.max(1) as f64).sqrt();
        // How far the swell of this sea rises and falls about its middle,
        // so that its crests and troughs span the tones whatever its size.
        // A sea of very small waves keeps a small span.
        let middle = heights_sum / wet.max(1) as f64;
        let rise = (heights_squared / wet.max(1) as f64 - middle * middle).max(0.0).sqrt();
        let span = (paint.crest_span[0].max(0.1) * rise).max(paint.crest_span[1].max(0.1));
        let ease = if paint.shade_slope > 0.0 && steepness > paint.shade_slope { paint.shade_slope / steepness } else { 1.0 };
        // The colour each column aims for, rounded so that near-identical
        // colours share one entry, and how much say it has in the block choice.
        // Also, per column, its depth, white water and mirrored sky, for the maps.
        let mut wanted: Vec<([i16; 3], f32, bool, [f32; 3])> = of_core::memory::filled(cells, ([0; 3], -1.0, false, [0.0; 3]))?;
        fill_rows(&mut wanted, w as usize, progress, (0.5, 0.85), &|row, out| {
            let z = row as i32;
            for x in 0..w {
                let i = at(x, z);
                if top[i] == DRY {
                    continue;
                }
                let (wx, wz) = world(x, z);
                let here = surface(x, z);
                // Slope of the swell, plus ripples too small to build. The
                // ripples are small waves of the same wind: short the way
                // the waves run, longer along the crests, and they tilt the
                // water mostly that way.
                let (along, across) = (wx * heading.0 + wz * heading.1, wz * heading.0 - wx * heading.1);
                let mut dx = (smooth(x + 1, z) - smooth(x - 1, z)) * 0.5;
                let mut dz = (smooth(x, z + 1) - smooth(x, z - 1)) * 0.5;
                let ripple = look.ripple * ripples.slope;
                if let Some(p) = &pattern {
                    let (px, pz) = p.slope(wx, wz);
                    dx += ripple * pattern_share * px;
                    dz += ripple * pattern_share * pz;
                } else {
                    let onward = ripple * fbm(along / ripple_size[0], across / ripple_size[1], seed ^ 0xA11, 2);
                    let sideways = ripple * ripples.sideways.max(0.0) * value(along / ripple_size[1], across / ripple_size[0], seed ^ 0xB22);
                    dx += onward * heading.0 - sideways * heading.1;
                    dz += onward * heading.1 + sideways * heading.0;
                }
                // Depth of the made-up sea floor under this column.
                let imagined = bed_depth.as_ref().map(|d| (d(wx, wz) + here).max(0.0));
                let len = (dx * dx + 1.0 + dz * dz).sqrt();
                let toward_sun = (-dx * sun[0] + sun[1] - dz * sun[2]) / len;
                // The direction of the light leads: a face turned to the sun
                // takes in more light and is lighter, one turned away is
                // darker. The height within the wave supports it: light comes
                // through the thin water of a crest, a trough is deep.
                let turned = paint.slope_light * ease * (toward_sun - sun[1]);
                let lift = paint.crest_light * (smooth(x, z) - middle) / span;
                // The darkest and the lightest are approached, not struck.
                let swing = look.shading * (look.contrast * (lift + turned)).tanh();
                // Broad patches the wind has ruffled more, or less.
                let ruffled = paint.patch_tone * fbm(along / patch_size[0], across / patch_size[1], seed ^ 0x9A7, 3);
                let tone = level + if swing > 0.0 { swing * (highest_tone - level) } else { swing * (level - lowest_tone) } + ruffled;
                // In shadow the direct sun is gone and only skylight is left.
                let sunshine = 1.0 - look.shadow.clamp(0.0, 1.0) * (1.0 - sunlit[i] as f64);
                let direct = look.lit_strength * sunshine;
                let mut tint = direct * tone.clamp(0.0, 2.0);
                let mut bed: Option<([f32; 3], f64)> = None;
                let (mut foam, mut trail) = (0.0f64, 0.0f64);
                if can_break {
                    // Where the water is steep enough a crest is breaking.
                    // Foam is also left where one was breaking a moment
                    // ago: the sea is asked how steep it was here at a
                    // handful of past moments, and older foam counts less.
                    // In water too shallow for the waves they break more
                    // readily, so the surf shows as lines of breakers.
                    let arriving = shore.map_or(Near::OPEN, |s| s.near(x, z));
                    let surf = match imagined {
                        Some(d) => smoothstep(break_depth, 0.3 * break_depth, d),
                        None => shore.map_or(0.0, |s| s.surf(x, z)),
                    };
                    let from = breaks_at * (1.0 - caps.surf_ease.clamp(0.0, 0.95) * surf);
                    let breaking = |ago: f64| smoothstep(from - soft, from + soft, field.steepness(wx, wz, ago, &arriving, ride));
                    foam = breaking(0.0);
                    if moments > 0 {
                        let stagger = hash(s.origin[0] + x, s.origin[1] + z, seed ^ 0xF0A3) as f64 / (u32::MAX as f64 + 1.0);
                        let mut left = 0.0;
                        for n in 0..moments {
                            let ago = (n as f64 + stagger) / moments as f64 * 3.0;
                            left += breaking(ago * fade) * (-ago).exp();
                        }
                        trail = left * 3.0 / moments as f64 * caps.trail_strength;
                    }
                    // Calm water breaks less.
                    if !zones.is_empty() {
                        let left = zones.calm(x, z);
                        (foam, trail) = (foam * left, trail * left);
                    }
                }
                if let Some(depth) = imagined {
                    let shallow = (1.0 - depth / look.depth_reach).clamp(0.0, 1.0);
                    tint = (tint * (1.0 + look.depth_influence * shallow)).min(2.0);
                }
                let depth_here = match (imagined, terrain) {
                    (Some(d), _) => d,
                    (None, Some(t)) => (here - t.height(x, z) as f64).max(0.0),
                    (None, None) => f64::INFINITY,
                };
                if let Some(t) = terrain {
                    let depth = (here - t.height(x, z) as f64).max(0.0);
                    let shallow = (1.0 - depth / look.depth_reach).clamp(0.0, 1.0);
                    tint = (tint * (1.0 + look.depth_influence * shallow)).min(2.0);
                    if look.clarity > 0.0 {
                        let kind = Ground::ALL.iter().position(|k| *k == t.kind(x, z)).unwrap();
                        bed = beds[kind].map(|colour| (colour, shallow * shallow));
                    }
                    // White water where the sea meets land: a thin line
                    // where waves run up a beach, a broad broken band where
                    // they strike a rocky shore, a solid one at the foot of
                    // a cliff; wider under tall waves, and little of any on
                    // a sheltered shore or up a river.
                    let mut nearest: Option<(f64, Ground)> = None;
                    for nz in -reach..=reach {
                        for nx in -reach..=reach {
                            let (px, pz) = (x + nx, z + nz);
                            if inside(px, pz) && top[at(px, pz)] == DRY {
                                let dist = ((nx * nx + nz * nz) as f64).sqrt();
                                if nearest.is_none_or(|(d, _)| dist < d) {
                                    nearest = Some((dist, t.kind(px, pz)));
                                }
                            }
                        }
                    }
                    if let (Some((dist, _)), Some(sv)) = (nearest, survey) {
                        let open = shore.map_or(1.0, |s| s.open(x, z));
                        let mut struck = lerp(edge.sheltered.clamp(0.0, 1.0), 1.0, open);
                        if sv.channel(x, z) {
                            struck *= edge.channel_share.clamp(0.0, 1.0);
                        }
                        let (cliff, rocky) = sv.character(x, z);
                        let beach = (1.0 - cliff - rocky).max(0.0);
                        let tall = s.waves.height * shore.map_or(1.0, |s| s.near(x, z).long);
                        let sway = edge.height_sway.clamp(0.0, 1.0);
                        let swell = lerp(1.0 - sway, 1.0 + sway, smoothstep(1.0, 9.0, tall));
                        let reach = (beach * edge.sand_reach + (rocky * edge.rock_reach + cliff * edge.cliff_reach) * (0.4 + 0.6 * open)) * swell;
                        let strength = beach * edge.sand_strength + rocky * edge.rock_strength + cliff * edge.cliff_strength;
                        let size = edge.patch_size.max(0.5);
                        // Foam is most broken up among rocks.
                        let whole = 0.35 + 0.25 * (1.0 - rocky);
                        let patch = whole + (1.0 - whole) * (0.5 + 0.5 * fbm(wx / size, wz / size, seed ^ 0xC33, 2));
                        let mut shore_foam = strength * struck * (1.0 - (dist - 1.0) / reach.max(0.01)).clamp(0.0, 1.0) * patch;
                        if let Some(f) = flow {
                            // Waves raised by the current break harder on the
                            // shore they meet, and "Collision white water"
                            // trims the white water there.
                            let fast = smoothstep(tuning.flow.foam_from, 3.0 * tuning.flow.foam_from, f.speed);
                            let push = (f.strike(x, z) * 2.0).clamp(0.0, 1.0);
                            shore_foam *= (1.0 + tuning.flow.impact_bonus.max(0.0) * fast * push) * lerp(1.0, look.impact_foam.clamp(0.0, 1.0) * 2.0, push);
                        }
                        foam = foam.max(shore_foam);
                    }
                    // A river's water shows paler where it runs out into the sea.
                    if let Some(s) = shore {
                        tint = (tint + direct * tuning.outflow.plume_tint.max(0.0) * s.plume(x, z)).min(2.0);
                    }
                    // White water at the foot of a waterfall.
                    if churned[i] > 0.0 {
                        let size = edge.patch_size.max(0.5);
                        let patch = 0.55 + 0.45 * (0.5 + 0.5 * fbm(wx / size, wz / size, seed ^ 0xD44, 2));
                        foam = foam.max(edge.fall_strength.clamp(0.0, 1.0) * churned[i] as f64 * patch);
                    }
                }
                // White water where a current strikes land, and the foam it
                // drags away downstream. The struck face may go solid white;
                // the trail may not, or it reads as a sheet pasted on the sea.
                // A fast current already breaks the sea white all around it,
                // so the faces it strikes are capped lower as it speeds up.
                // The trail keeps its own cap: that white wash is how a fast
                // current reads behind what the water runs past.
                let fast = flow.map_or(0.0, |f| smoothstep(tuning.flow.foam_from, 3.0 * tuning.flow.foam_from, f.speed));
                let fast_cap = (1.0 - tuning.flow.fast_cap.clamp(0.0, 1.0) * fast).clamp(0.0, 1.0);
                let mut carried = 0.0f64;
                let mut broken = 0.0f64;
                if let Some(f) = flow {
                    let size = tuning.flow.foam_streak;
                    let (down, aside) = (wx * f.toward.0 + wz * f.toward.1, wz * f.toward.0 - wx * f.toward.1);
                    let drawn = 0.5 + 0.5 * fbm(down / size[0].max(1.0), aside / size[1].max(1.0), seed ^ 0xF10A, 2);
                    let struck = f.strike(x, z);
                    if struck > 0.0 {
                        // A fast current already breaks the sea white all
                        // around it, so the faces it strikes are capped lower
                        // and left to the sea's own foam to finish.
                        let face = look.impact_foam.clamp(0.0, 1.0) * 2.0 * fast_cap;
                        foam = foam.max(struck * face * (0.5 + 0.5 * drawn));
                    }
                    let away = f.wake(x, z);
                    if away > 0.0 {
                        let tail = look.wake_foam.clamp(0.0, 1.0) * 2.0;
                        carried = away * tail * (0.15 + 0.85 * drawn);
                        // How the trail breaks up along the current.
                        broken = (0.5 + 0.7 * fbm(down / (0.35 * size[0]).max(1.0), aside / size[1].max(1.0), seed ^ 0x7A11, 2)).clamp(0.0, 1.0);
                    }
                }
                // A crest that is breaking now, a struck shore and the foot
                // of a fall can be solid white; foam left behind cannot.
                let shown = |foam: f64| (foam * look.foam * paint.foam_gain).clamp(0.0, 1.0);
                let most = caps.trail_most.clamp(0.0, 1.0);
                let amount = shown(foam).max(shown(trail).min(most));
                // Churned water around the foam is paler than the sea, as
                // far as there is sunlight to show it.
                tint = (tint + direct * paint.aerated_tint * amount).min(2.0);
                let mut lab = toned(tint);
                if deeper > 0.0 && swing < 0.0 {
                    let by = (deeper * direct * -swing) as f32;
                    lab = [lab[0] * (1.0 - by), lab[1] * (1.0 + by), lab[2] * (1.0 + by)];
                }
                // Under cloud and in shadow the light comes from the whole
                // sky: crests stand open to it, troughs and steep faces see
                // less of it. This keeps the shape of the waves readable
                // where there is no sun to pick it out.
                let open = (0.5 + 0.5 * (2.0 * paint.sky_lift * smooth(x, z) / s.waves.height.max(0.5)).tanh()) / (len * len);
                lab = scaled(lab, 1.0 + paint.sky_shade * look.shading * (1.0 - direct) * (2.0 * open - 1.0));
                // Shallow water is clear enough to show what lies under it,
                // as brightly as there is light to reach it.
                if let Some((colour, near)) = bed {
                    let seen = scaled(colour, dim * (0.55 + 0.45 * direct));
                    lab = colour::mix(lab, seen, (look.clarity * paint.bed_tint * near).clamp(0.0, 1.0) as f32);
                }
                // The surface also throws light back without it entering the
                // water: the glint of the sun on slopes that mirror it to the
                // onlooker, and the sheen of the sky on slopes seen at a low
                // angle. This lies over the water's colour as a layer of its own.
                let mut mirrored_share = 0.0f64;
                if look.reflection > 0.0 {
                    let glint = glint_at.map_or(0.0, |(gx, gz)| {
                        let off = ((dx - gx).powi(2) + (dz - gz).powi(2)) / (2.0 * glint_spread * glint_spread);
                        look.lit_strength * sunlit[i] as f64 * (-off).exp()
                    });
                    let seen = ((-dx * view[0] + view[1] - dz * view[2]) / len).clamp(0.0, 1.0);
                    // Against level water: a face turned away mirrors more
                    // sky and pales, a face turned toward the onlooker
                    // mirrors less and shows the deep colour.
                    let sheen = (sky_share(seen) - level_sheen) / (1.0 - level_sheen).max(1e-6);
                    let layer = look.reflection * (paint.glint_share.max(0.0) * glint + paint.sheen_share.max(0.0) * sheen);
                    mirrored_share = layer.clamp(0.0, 1.0);
                    lab = colour::mix(lab, reflection_lab, layer.clamp(-paint.sheen_deepen.max(0.0), 1.0) as f32);
                    // Deepening water that is already black leaves it black.
                    if lab[0] <= 0.0 {
                        lab = [0.0; 3];
                    }
                }
                // Foam is a scatter of white blocks, not a grey wash: each
                // column has its own threshold, so thin foam turns a few
                // blocks white and thick foam nearly all of them.
                let grain = hash(s.origin[0] + x, s.origin[1] + z, seed ^ 0x6D2B) as f64 / (u32::MAX as f64 + 1.0);
                let drawn = (0.5 + 0.7 * fbm(along / streak[0], across / streak[1], seed ^ 0xE55, 2)).clamp(0.0, 1.0);
                let threshold = lerp(paint.foam_grain.clamp(0.0, 1.0), 1.0, lerp(grain, drawn, paint.foam_streaks.clamp(0.0, 1.0)));
                let spread = paint.foam_edge.max(1e-6);
                let mut white = if amount > 0.0 { paint.foam_most.clamp(0.0, 1.0) * smoothstep(threshold - spread, threshold + spread, amount) } else { 0.0 };
                // Foam the current drags away is foam left behind: capped
                // below pure white, broken into strips along the current, and
                // gone once it thins under the threshold, so the sea recovers
                // downstream of land.
                if carried > 0.0 {
                    let need = lerp(paint.foam_grain.clamp(0.0, 1.0), 1.0, lerp(grain, broken, paint.foam_streaks.clamp(0.0, 1.0)));
                    let lying = smoothstep(need - spread, need + spread, carried);
                    // The trail keeps its own cap: what the current drags
                    // away is the white wash the sea is read by, and a fast
                    // current must still show it. Only the faces it strikes
                    // are trimmed as it speeds up (see `fast_cap` above).
                    let cap = spent.most.clamp(0.0, 1.0);
                    white = white.max(paint.foam_most.clamp(0.0, 1.0) * cap * lying);
                }
                // Foam left behind: lace where a crest broke a wave or two
                // ago, streaks where the wind has drawn it out.
                let soft_edge = spent.edge.max(1e-6);
                let mut lying = 0.0f64;
                if lace_gain > 0.0 {
                    let (has, needs) = (lace_gain * left_at(x, z, 1), lace_needs(along, across));
                    lying = smoothstep(needs - soft_edge, needs + soft_edge, has);
                }
                if streak_gain > 0.0 {
                    let (has, needs) = (streak_gain * left_at(x, z, 2), streak_needs(along, across));
                    lying = lying.max(smoothstep(needs - soft_edge, needs + soft_edge, has));
                }
                white = white.max(paint.foam_most.clamp(0.0, 1.0) * spent.most.clamp(0.0, 1.0) * lying);
                // Leaping spray is white all the way through.
                let leaping = spray[i] as f64;
                if leaping > 0.12 {
                    white = white.max(paint.foam_most.clamp(0.0, 1.0) * ((leaping - 0.12) / 0.88).powi(2));
                }
                lab = colour::mix(lab, foam_lab, white as f32);
                let key = [(lab[0] * 160.0).round() as i16, (lab[1] * 160.0).round() as i16, (lab[2] * 160.0).round() as i16];
                // White water is sparse but catches the eye, so it counts for
                // more than its area when the blocks are chosen.
                let look_of = [depth_here.min(f32::MAX as f64) as f32, white as f32, mirrored_share as f32];
                out[x as usize] = (key, 1.0 + (paint.foam_weight.max(0.0) * white) as f32, white >= 0.5, look_of);
            }
        })?;
        let mut targets: Vec<([f32; 3], f32)> = Vec::new();
        let mut index: HashMap<[i16; 3], u32> = HashMap::new();
        let mut target_of: Vec<u32> = vec![u32::MAX; cells];
        maps = Maps { depth: vec![f32::INFINITY; cells], foam: vec![0; cells], sheen: vec![0; cells] };
        for (i, (key, weight, white, seen)) in wanted.iter().enumerate() {
            if *weight < 0.0 {
                continue;
            }
            maps.depth[i] = seen[0];
            maps.foam[i] = (seen[1].clamp(0.0, 1.0) * 255.0).round() as u8;
            maps.sheen[i] = (seen[2].clamp(0.0, 1.0) * 255.0).round() as u8;
            let t = *index.entry(*key).or_insert_with(|| {
                targets.push(([key[0] as f32 / 160.0, key[1] as f32 / 160.0, key[2] as f32 / 160.0], 0.0));
                targets.len() as u32 - 1
            });
            targets[t as usize].1 += weight;
            target_of[i] = t;
            stats.white_columns += *white as u64;
        }
        step(progress, 0.87)?;
        let care = &Care { hue: paint.hue_care as f32, washed: paint.washed_care as f32, from: [paint.care_from[0] as f32, paint.care_from[1] as f32] };
        pairs = matcher.choose(&targets, look.max_solids.max(1), look.max_glass.max(1), care, paint.block_gain as f32);
        // The stack nearest each colour, and the steps in lightness between
        // the stacks in use.
        let own: Vec<usize> = targets.iter().map(|(colour, _)| Matcher::nearest(&pairs, colour, care)).collect();
        let mut tones: Vec<f32> = own.iter().map(|p| pairs[*p].lab[0]).collect();
        tones.sort_by(|a, b| a.total_cmp(b));
        tones.dedup_by(|a, b| (*a - *b).abs() < 1e-4);
        // Dithering: a colour between two tones is built as a fine fixed
        // pattern of both, in the proportion that gives the colour at a
        // distance, so the steps between block colours do not show as bands.
        let reach = if look.dither_mode == Dither::Off { 0.0 } else { (look.dither * paint.dither_reach).max(0.0) as f32 };
        let patch = [paint.dither_patch_size[0].max(0.5), paint.dither_patch_size[1].max(0.5)];
        // The fine layout of a blend: a fixed pattern or a random scatter,
        // in cells `dither_grain` blocks across.
        let grain = look.dither_grain.max(1);
        let fine_at = |bx: i32, bz: i32, salt: u32| -> f64 {
            let (cx, cz) = (bx.div_euclid(grain), bz.div_euclid(grain));
            match look.dither_mode {
                Dither::Noise => crate::noise::hash(cx, cz, look.dither_seed ^ salt.wrapping_mul(0x9E37_79B9)) as f64 / u32::MAX as f64,
                _ => BAYER[((cz + (salt >> 2) as i32).rem_euclid(4) * 4 + (cx + salt as i32).rem_euclid(4)) as usize] as f64 / 16.0,
            }
        };
        // Passing on what a block could not show: the share of the error
        // that goes to the block ahead and to the three in the row below.
        let diffuse = look.dither_mode == Dither::Diffusion && reach > 0.0;
        // How much of the layout is patches rather than the fine pattern.
        let patches = if look.dither_mode == Dither::Patches { paint.dither_patches.clamp(0.0, 1.0) } else { 0.0 };
        // A fine layout is a scatter by choice, so its lone blocks stay.
        let scatter = !matches!(look.dither_mode, Dither::Off | Dither::Patches);
        let (mut ahead, mut below) = (vec![0f32; w as usize + 2], vec![0f32; w as usize + 2]);
        for i in 0..cells {
            let column = i % w as usize;
            if diffuse && column == 0 {
                std::mem::swap(&mut ahead, &mut below);
                below.iter_mut().for_each(|v| *v = 0.0);
            }
            if target_of[i] == u32::MAX {
                continue;
            }
            if diffuse {
                let colour = targets[target_of[i] as usize].0;
                let wanted = colour[0] + ahead[column + 1];
                let chosen = Matcher::nearest(&pairs, &[wanted, colour[1], colour[2]], care);
                let most = paint.dither_most.max(0.0) as f32;
                let left = ((wanted - pairs[chosen].lab[0]) * look.dither.clamp(0.0, 1.0) as f32).clamp(-most, most);
                ahead[column + 2] += left * 7.0 / 16.0;
                below[column] += left * 3.0 / 16.0;
                below[column + 1] += left * 5.0 / 16.0;
                below[column + 2] += left / 16.0;
                pair_of[i] = chosen as u32;
                continue;
            }
            let colour = targets[target_of[i] as usize].0;
            let mut chosen = own[target_of[i] as usize];
            let above = tones.partition_point(|tone| *tone < colour[0]);
            if reach > 0.0 && above > 0 && above < tones.len() {
                let gap = (tones[above] - tones[above - 1]).min(paint.dither_most.max(0.0) as f32);
                let (bx, bz) = (s.origin[0] + i as i32 % w, s.origin[1] + i as i32 / w);
                // A block-by-block pattern reads as grain. Patches a few
                // blocks across, drawn out along the crests, read as water.
                let fine = fine_at(bx, bz, 0);
                let (along, across) = (bx as f64 * heading.0 + bz as f64 * heading.1, bz as f64 * heading.0 - bx as f64 * heading.1);
                let drawn = (0.5 + 0.9 * fbm(along / patch[0], across / patch[1], seed ^ 0xD17, 2)).clamp(0.0, 1.0);
                let pattern = lerp(fine, drawn, patches) as f32;
                let nudge = reach * gap * (pattern - 0.47);
                chosen = Matcher::nearest(&pairs, &[colour[0] + nudge, colour[1], colour[2]], care);
            }
            pair_of[i] = chosen as u32;
        }
        // A block whose colour none of the four beside it share is a speck.
        // It takes the colour, among theirs, nearest to what it was meant to
        // have. Foam is left alone: it is meant to be a scatter.
        if paint.lone_blocks < 0.5 && !scatter {
            for i in 0..cells {
                if target_of[i] == u32::MAX || wanted[i].2 {
                    continue;
                }
                let (x, z) = (i as i32 % w, i as i32 / w);
                let mut beside: [u32; 4] = [u32::MAX; 4];
                let mut alone = true;
                for (n, (dx, dz)) in AROUND[..4].iter().enumerate() {
                    if inside(x + dx, z + dz) {
                        let j = at(x + dx, z + dz);
                        if target_of[j] != u32::MAX && !wanted[j].2 {
                            beside[n] = pair_of[j];
                            alone &= pair_of[j] != pair_of[i];
                        }
                    }
                }
                if !alone {
                    continue;
                }
                let colour = targets[target_of[i] as usize].0;
                let far = |p: u32| {
                    let lab = pairs[p as usize].lab;
                    (lab[0] - colour[0]).powi(2) + (lab[1] - colour[1]).powi(2) + (lab[2] - colour[2]).powi(2)
                };
                if let Some(best) = beside.iter().filter(|p| **p != u32::MAX).min_by(|a, b| far(**a).total_cmp(&far(**b))) {
                    pair_of[i] = *best;
                }
            }
        }
        // With two or three layers of glass, a border between two looks is
        // blended through the layers. So far every column has one look: a
        // solid under glass of one colour. Within a few blocks of a border,
        // each layer now takes its colour in a pass of its own, the top one
        // first, from among the colours in use nearby. The top follows a
        // softened copy of the colours and is not dithered, so its line runs
        // smooth; each layer under it sets right what the ones above left
        // off, with more dithering the lower it lies, where the glass over
        // it softens the pattern. No two layers change colour along the same
        // line, so one hard border becomes several faint ones a little
        // apart. Away from a border nothing changes.
        let reach = paint.layer_reach.round().max(0.0) as i32;
        if blend && reach > 0 && !pairs.is_empty() {
            let n = glass_layers as usize;
            let plain = |i: usize| target_of[i] != u32::MAX && !wanted[i].2;
            let wanted_lab = |i: usize| targets[target_of[i] as usize].0;
            // The solids and the glass colours in use, each with a bit of its own.
            let (mut solids, mut colours): (Vec<u16>, Vec<u16>) = (Vec::new(), Vec::new());
            for p in &pairs {
                if !solids.contains(&p.solid) {
                    solids.push(p.solid);
                }
                if !colours.contains(&p.glass[0]) {
                    colours.push(p.glass[0]);
                }
            }
            let solid_bit = |solid: u16| 1u64 << solids.iter().position(|s| *s == solid).unwrap_or(0).min(63);
            let colour_bit = |glass: u16| if glass == NO_GLASS { 0 } else { 1u32 << colours.iter().position(|g| *g == glass).unwrap_or(0).min(31) };
            // Which of them the columns within reach of each column use.
            let mut near: Vec<(u64, u32)> = (0..cells)
                .map(|i| {
                    let p = if plain(i) { &pairs[pair_of[i] as usize] } else { return (0, 0) };
                    (solid_bit(p.solid), colour_bit(p.glass[0]))
                })
                .collect();
            for (dx, dz) in [(1, 0), (0, 1)] {
                let from = near.clone();
                for z in 0..l {
                    for x in 0..w {
                        let mut all = (0, 0);
                        for d in -reach..=reach {
                            if inside(x + dx * d, z + dz * d) {
                                let v = from[at(x + dx * d, z + dz * d)];
                                all = (all.0 | v.0, all.1 | v.1);
                            }
                        }
                        near[at(x, z)] = all;
                    }
                }
            }
            let zone: Vec<usize> = (0..cells).filter(|i| plain(*i) && (near[*i].0.count_ones() > 1 || near[*i].1.count_ones() > 1)).collect();
            // Every way the blocks in use can be stacked, and what each is made of.
            let full = matcher.arrangements(&pairs);
            let made: Vec<(u64, [u32; 3])> = full.iter().map(|p| (solid_bit(p.solid), p.glass.map(colour_bit))).collect();
            let place: HashMap<(u16, [u16; 3]), u32> = full.iter().enumerate().map(|(i, p)| ((p.solid, p.glass), i as u32)).collect();
            for i in 0..cells {
                if let Some(p) = pairs.get(pair_of[i] as usize) {
                    pair_of[i] = place[&(p.solid, p.glass)];
                }
            }
            let softened = |radius: f64| -> Vec<[f32; 3]> {
                let far = (radius * 2.0).ceil() as i32;
                let weights: Vec<f32> = (-far..=far).map(|d| (-0.5 * (d as f64 / radius).powi(2)).exp() as f32).collect();
                let pass = |from: &[[f32; 4]], (dx, dz): (i32, i32)| -> Vec<[f32; 4]> {
                    let mut out = vec![[0f32; 4]; cells];
                    for z in 0..l {
                        for x in 0..w {
                            let mut sum = [0f32; 4];
                            for (k, weight) in weights.iter().enumerate() {
                                let (nx, nz) = (x + dx * (k as i32 - far), z + dz * (k as i32 - far));
                                if inside(nx, nz) {
                                    let v = from[at(nx, nz)];
                                    for c in 0..4 {
                                        sum[c] += v[c] * weight;
                                    }
                                }
                            }
                            out[at(x, z)] = sum;
                        }
                    }
                    out
                };
                // Foam and dry land are left out, so neither bleeds into the water beside it.
                let first: Vec<[f32; 4]> = (0..cells)
                    .map(|i| {
                        let c = if plain(i) { wanted_lab(i) } else { return [0.0; 4] };
                        [c[0], c[1], c[2], 1.0]
                    })
                    .collect();
                let both = pass(&pass(&first, (1, 0)), (0, 1));
                both.iter().map(|v| if v[3] > 1e-6 { [v[0] / v[3], v[1] / v[3], v[2] / v[3]] } else { [0.0; 3] }).collect()
            };
            let patch = [paint.dither_patch_size[0].max(0.5), paint.dither_patch_size[1].max(0.5)];
            // Each layer has a pattern of its own, so their borders differ.
            let pattern = |i: usize, salt: u32| -> f32 {
                let (bx, bz) = (s.origin[0] + i as i32 % w, s.origin[1] + i as i32 / w);
                let fine = fine_at(bx, bz, salt);
                let (along, across) = (bx as f64 * heading.0 + bz as f64 * heading.1, bz as f64 * heading.0 - bx as f64 * heading.1);
                let drawn = (0.5 + 0.9 * fbm(along / patch[0], across / patch[1], seed ^ salt, 2)).clamp(0.0, 1.0);
                lerp(fine, drawn, patches) as f32
            };
            // The nearest stack to a colour among those made only of the
            // solids and, layer by layer, the glass colours allowed.
            let mut found: HashMap<([i16; 3], u64, [u32; 3]), Option<(usize, f32)>> = HashMap::new();
            let mut best = |colour: [f32; 3], solids: u64, glass: [u32; 3]| -> Option<(usize, f32)> {
                let key = colour.map(|c| (c * 160.0).round() as i16);
                *found.entry((key, solids, glass)).or_insert_with(|| {
                    Matcher::nearest_where(&full, &key.map(|c| c as f32 / 160.0), care, |at| made[at].0 & solids != 0 && (0..3).all(|k| made[at].1[k] == 0 || made[at].1[k] & glass[k] != 0))
                })
            };
            // Of the looks open to a layer, the nearest to a colour. Where the
            // colour lies between the nearest two, the second is taken in
            // patches, for as large a share as the colour is of the way
            // towards it: together they give the colour at a distance. A
            // colour one look gives outright is never mixed.
            let cap = paint.dither_most.max(1e-6) as f32;
            let pick = |options: &[(usize, f32)], colour: [f32; 3], pattern: f32, amount: f32| -> Option<usize> {
                let (mut nearest, mut next): (Option<(usize, f32)>, Option<(usize, f32)>) = (None, None);
                for o in options {
                    if nearest.is_none_or(|n| o.1 < n.1) {
                        (nearest, next) = (Some(*o), nearest);
                    } else if next.is_none_or(|n| o.1 < n.1) {
                        next = Some(*o);
                    }
                }
                let first = nearest?.0;
                let Some(second) = next.map(|o| o.0).filter(|_| amount > 0.0) else { return Some(first) };
                let (a, b) = (full[first].lab, full[second].lab);
                let step = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
                let length = step[0] * step[0] + step[1] * step[1] + step[2] * step[2];
                if length < 1e-8 {
                    return Some(first);
                }
                let share = ((colour[0] - a[0]) * step[0] + (colour[1] - a[1]) * step[1] + (colour[2] - a[2]) * step[2]) / length;
                // A step larger than `dither_most` is not mixed across in full.
                let narrow = (length.sqrt() / cap).max(1.0) / amount;
                Some(if 0.5 + (share - 0.5) * narrow > pattern { second } else { first })
            };
            let bits = |set: u64| (0..64).map(|b| 1u64 << b).filter(move |b| set & b != 0);
            let mut options: Vec<(usize, f32)> = Vec::new();
            // The glass colours a column may still take in each layer; one bit once a layer is laid.
            let mut open: Vec<[u32; 3]> = (0..cells).map(|i| std::array::from_fn(|k| if k < n { near[i].1 } else { 0 })).collect();
            for k in (0..n).rev() {
                let (soft, mix) = if k == n - 1 {
                    (paint.layer_smooth[1], paint.layer_dither[2])
                } else if k == 0 {
                    (0.0, paint.layer_dither[0])
                } else {
                    (paint.layer_smooth[0], paint.layer_dither[1])
                };
                let field = (soft > 0.0).then(|| softened(soft));
                let amount = if look.dither_mode == Dither::Off { 0.0 } else { (look.dither * mix).clamp(0.0, 1.0) as f32 };
                let aim = |i: usize| -> [f32; 3] { field.as_ref().map_or_else(|| wanted_lab(i), |f| f[i]) };
                for &i in &zone {
                    let colour = aim(i);
                    options.clear();
                    for g in bits(near[i].1 as u64) {
                        let mut tried = open[i];
                        tried[k] = g as u32;
                        options.extend(best(colour, near[i].0, tried));
                    }
                    if let Some(p) = pick(&options, colour, pattern(i, 0xA11 + 0x3D * k as u32), amount) {
                        open[i][k] = made[p].1[k];
                    }
                }
                // No single blocks left standing out in the upper layers.
                if k > 0 && paint.lone_blocks < 0.5 && !scatter {
                    for &i in &zone {
                        let (x, z) = (i as i32 % w, i as i32 / w);
                        let beside: Vec<u32> = AROUND[..4].iter().filter(|(dx, dz)| inside(x + dx, z + dz)).map(|(dx, dz)| at(x + dx, z + dz)).filter(|j| plain(*j)).map(|j| open[j][k]).collect();
                        if beside.is_empty() || beside.contains(&open[i][k]) {
                            continue;
                        }
                        let mut least = f32::INFINITY;
                        for g in beside {
                            let mut tried = open[i];
                            tried[k] = g;
                            if let Some((_, cost)) = best(aim(i), near[i].0, tried) {
                                if cost < least {
                                    (least, open[i][k]) = (cost, g);
                                }
                            }
                        }
                    }
                }
            }
            // The solid under the glass sets right what is left.
            let amount = if look.dither_mode == Dither::Off { 0.0 } else { (look.dither * paint.dither_reach).clamp(0.0, 1.0) as f32 };
            for &i in &zone {
                let colour = wanted_lab(i);
                options.clear();
                for solid in bits(near[i].0) {
                    options.extend(best(colour, solid, open[i]));
                }
                if let Some(p) = pick(&options, colour, pattern(i, 0xD17), amount) {
                    pair_of[i] = p as u32;
                }
            }
            if paint.lone_blocks < 0.5 && !scatter {
                for &i in &zone {
                    let (x, z) = (i as i32 % w, i as i32 / w);
                    let own = made[pair_of[i] as usize].0;
                    let beside: Vec<u64> = AROUND[..4].iter().filter(|(dx, dz)| inside(x + dx, z + dz)).map(|(dx, dz)| at(x + dx, z + dz)).filter(|j| plain(*j)).map(|j| made[pair_of[j] as usize].0).collect();
                    if beside.is_empty() || beside.contains(&own) {
                        continue;
                    }
                    let mut least = f32::INFINITY;
                    for solid in beside {
                        if let Some((p, cost)) = best(wanted_lab(i), solid, open[i]) {
                            if cost < least {
                                (least, pair_of[i]) = (cost, p as u32);
                            }
                        }
                    }
                }
            }
            pairs = full;
        }
        target = target_of.iter().map(|t| targets.get(*t as usize).map_or([0; 3], |t| colour::from_oklab(t.0))).collect();
        matched = pair_of.iter().map(|p| pairs.get(*p as usize).map_or([0; 3], |p| colour::from_oklab(p.lab))).collect();
        step(progress, 0.92)?;
    }

    // ---- Blocks: which stack each column takes. The blocks themselves
    // follow from the columns when they are needed (`Baked::layers`).
    let foundation = palette.intern(BlockState::new(&format!("{}_concrete", s.foundation)));
    if !catalogue.blocks.contains_key(&palette.get(foundation).name) {
        return Err(format!("'{}' is not a concrete colour", s.foundation));
    }
    let solid_ids: Vec<u32> = matcher.solids.iter().map(|b| palette.intern(BlockState::new(&b.name))).collect();
    let glass_ids: Vec<u32> = matcher.glasses.iter().map(|b| palette.intern(BlockState::new(&b.name))).collect();
    let stacks: Vec<Stack> = pairs
        .iter()
        .map(|p| Stack {
            solid: solid_ids[p.solid as usize],
            glass: std::array::from_fn(|k| (k < glass_layers as usize && p.glass[k] != NO_GLASS).then(|| glass_ids[p.glass[k] as usize])),
        })
        .collect();
    let mut range = (i32::MAX, i32::MIN);
    for t in top.iter().filter(|t| **t != DRY) {
        stats.water_columns += 1;
        range = (range.0.min(*t), range.1.max(*t));
    }
    stats.range = if stats.water_columns > 0 { range } else { (0, 0) };
    let local = |v: &[i32]| -> Vec<i32> { v.iter().map(|t| if *t == DRY { DRY } else { t + sea_level }).collect() };
    let columns = Columns { name: name.to_string(), palette, foundation, ground: local(&ground), stack: pair_of, stacks, land };
    let mut baked = Baked {
        blocks: Blocks::Columns(columns),
        sea_level,
        export_y: s.placement_y - sea_level,
        width: w,
        length: l,
        water_top: local(&top),
        water_bottom: local(&bottom),
        target,
        matched,
        maps,
        stats,
    };
    // What the blocks come to, counted without laying them out.
    let mut counts: HashMap<u32, u64> = HashMap::new();
    let mut blocks = [0u64; 6];
    baked.for_each_block(|layer, _, id| {
        *counts.entry(id).or_default() += 1;
        blocks[layer_index(layer)] += 1;
    });
    step(progress, 0.99)?;
    baked.stats.blocks = blocks;
    let mut used: BTreeMap<String, u64> = BTreeMap::new();
    for (id, n) in counts {
        *used.entry(baked.palette().get(id).name.clone()).or_default() += n;
    }
    baked.stats.used = used.into_iter().collect();
    baked.stats.used.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    Ok(baked)
}
