//! Ocean generation: the wave shape, the block build made from it, its
//! colouring, and pictures of the result.
//!
//! This is the development version of the ocean method. The sea is the sum
//! of many waves drawn from a measured wave spectrum (`waves`); near land
//! they slow, bend, find shelter and break (`shore`); foam lies where waves
//! break and where they were breaking a moment before (`build`). The
//! method's internal numbers live in `tuning`, so it can be adjusted
//! without rebuilding the program.

pub mod baked;
pub mod blocks;
pub mod build;
pub mod circ;
pub mod colour;
pub mod estimate;
pub mod flow;
/// Lava is on hold: only the development build has it.
#[cfg(feature = "dev")]
pub mod lava;
pub mod look;
pub mod noise;
pub mod pattern;
pub mod render;
pub mod settings;
pub mod scene;
pub mod shore;
pub mod survey;
pub mod terrain;
pub mod tuning;
pub mod waves;
pub mod zone;

pub use blocks::{BakeSource, Blocks, Columns, Land as LandBlocks, Stack};
pub use build::{bake, bake_with};
pub use baked::{Baked, Stats, DRY};
pub use look::{Dither, Look};
pub use settings::{OceanSettings, Seabed};
pub use scene::{BuildPlacement, Land, SceneSetup, SCENE_FILE};
pub use survey::{read_shore, Reading, ShoreKind, Survey};
#[cfg(feature = "dev")]
pub use terrain::{cliff_scene, coast_scene};
pub use terrain::{Ground, Mouth, Outcome, Placed, Pond, PondEnd, Spill, Stream, Terrain};
pub use flow::Flow;
pub use shore::Shore;
pub use tuning::{Tuning, TUNING_BACKUP, TUNING_FILE, TUNING_VERSION};
pub use waves::{Near, WaveField, WaveSettings};
pub use zone::{Zone, ZoneKind, Zones};

#[cfg(test)]
mod tests {
    use super::*;
    use of_core::params::ParamValues;
    use of_formats::catalogue;
    use of_formats::export::{self, AirMode, ExportOptions, Layer, Selection};
    use of_formats::model::Format;
    use of_formats::Progress;
    use crate::tuning::WaveTuning;
    use std::collections::BTreeMap;

    fn settings(width: i32, length: i32) -> OceanSettings {
        let mut s = OceanSettings::from_params(&ParamValues::default());
        (s.width, s.length) = (width, length);
        s
    }

    fn blocks(b: &Baked) -> BTreeMap<(usize, [i32; 3]), String> {
        let mut all = BTreeMap::new();
        let layers = b.layers();
        for (i, layer) in Layer::ALL.iter().enumerate() {
            layers.layer(*layer).for_each(|p, id| {
                all.insert((i, p), layers.palette.get(id).name.clone());
            });
        }
        all
    }

    #[test]
    fn defaults_come_from_the_registry() {
        let s = settings(64, 64);
        assert_eq!(s.placement_y, 62);
        assert_eq!(s.waves.height, 8.0);
        assert_eq!(s.waves.peak_distance, 48.0);
        assert!(!s.filled);
        assert_eq!(s.thickness, 2);
        assert_eq!(s.foundation, "light_blue");
        assert!(s.colour_glass);
    }

    #[test]
    fn waves_match_the_wave_height_and_rest_at_zero() {
        for (irregularity, sharpness) in [(0.0, 0.0), (0.5, 0.4), (1.0, 1.0)] {
            let mut w = settings(16, 16).waves;
            (w.irregularity, w.crest_sharpness) = (irregularity, sharpness);
            let field = WaveField::new(&w);
            // A wide stretch of sea, so that many separate waves are counted.
            let (mut lo, mut hi, mut sum, mut squares) = (f64::MAX, f64::MIN, 0.0, 0.0);
            for z in 0..400 {
                for x in 0..400 {
                    let h = field.height(x as f64 * 7.0, z as f64 * 7.0);
                    (lo, hi, sum, squares) = (lo.min(h), hi.max(h), sum + h, squares + h * h);
                }
            }
            let mean = sum / 160_000.0;
            assert!(lo >= field.lowest() - 1e-9 && hi <= field.highest() + 1e-9);
            assert!(mean.abs() < 0.35, "mean level {mean} is not near zero");
            // The larger waves are as tall as asked, the rare tallest taller,
            // and no wave is a wall of water.
            let larger = 5.0 * (squares / 160_000.0 - mean * mean).sqrt();
            assert!((larger - w.height).abs() < 0.15 * w.height, "the larger waves are {larger} tall, not {}", w.height);
            assert!(hi - lo > w.height && hi - lo < 2.2 * w.height, "tallest wave {} against the height {}", hi - lo, w.height);
        }
    }

    #[test]
    fn same_settings_give_the_same_build_and_the_seed_changes_it() {
        let cat = catalogue::default();
        let s = settings(48, 48);
        let a = bake("a", &s, None, cat).unwrap();
        assert_eq!(blocks(&a), blocks(&bake("a", &s, None, cat).unwrap()));
        let mut other = s.clone();
        other.waves.seed = 2;
        assert_ne!(a.water_top, bake("a", &other, None, cat).unwrap().water_top);
    }

    #[test]
    fn a_smaller_area_is_a_cut_out_of_the_larger_one() {
        let cat = catalogue::default();
        let whole = bake("whole", &settings(64, 64), None, cat).unwrap();
        let mut s = settings(24, 20);
        s.origin = [16, 24];
        let part = bake("part", &s, None, cat).unwrap();
        for z in 0..20 {
            for x in 0..24 {
                let here = part.water_top[(z * 24 + x) as usize] - part.sea_level;
                let there = whole.water_top[((z + 24) * 64 + x + 16) as usize] - whole.sea_level;
                assert_eq!(here, there, "column {x},{z}");
            }
        }
        // Either way the water rests at the same world height.
        assert_eq!(part.export_y + part.sea_level, 62);
        assert_eq!(whole.export_y + whole.sea_level, 62);
    }

    #[test]
    fn the_sea_does_not_repeat_and_irregularity_shortens_the_crests() {
        let mut w = settings(16, 16).waves;
        // How alike the water is at two places a fixed step apart.
        let alike = |field: &WaveField, dx: f64, dz: f64| -> f64 {
            let (mut both, mut one, mut other) = (0.0, 0.0, 0.0);
            for j in 0..120 {
                for i in 0..120 {
                    let (x, z) = (i as f64 * 5.3, j as f64 * 5.3);
                    let (a, b) = (field.height(x, z), field.height(x + dx, z + dz));
                    (both, one, other) = (both + a * b, one + a * a, other + b * b);
                }
            }
            both / (one * other).sqrt()
        };
        w.direction = 0.0;
        let length = w.peak_distance;
        // One wavelength on, the sea looks much the same; many wavelengths
        // on it is a different sea, however far the step.
        w.irregularity = 0.5;
        let field = WaveField::new(&w);
        assert!(alike(&field, length, 0.0) > 0.3);
        for step in [9.0, 17.0, 40.0, 173.0] {
            assert!(alike(&field, step * length, 0.0).abs() < 0.3, "the sea repeats after {step} wavelengths");
        }
        // Along a crest, a regular swell stays alike far longer than a confused sea.
        // The crests of the sea wander on their own, so that measure needs the
        // spectrum on its own: this is about the fan of directions, not the bends.
        let mut straight = WaveTuning::default();
        straight.wander = 0.0;
        w.irregularity = 0.0;
        let regular = alike(&WaveField::with(&w, &straight), 0.0, 2.0 * length);
        w.irregularity = 1.0;
        let confused = alike(&WaveField::with(&w, &straight), 0.0, 2.0 * length);
        assert!(regular > 0.45 && confused < 0.25, "regular {regular}, confused {confused}");
    }

    #[test]
    fn a_gentle_swell_has_no_whitecaps_and_a_storm_has_many() {
        let cat = catalogue::default();
        let mut s = settings(128, 128);
        (s.waves.height, s.waves.peak_distance) = (3.0, 64.0);
        assert_eq!(bake("calm", &s, None, cat).unwrap().stats.white_columns, 0);
        (s.waves.height, s.waves.peak_distance, s.waves.irregularity) = (14.0, 56.0, 0.8);
        let storm = bake("storm", &s, None, cat).unwrap().stats;
        let white = storm.white_columns as f64 / storm.water_columns as f64;
        // A real gale whitens a few hundredths of the sea, never most of it.
        assert!(white > 0.01 && white < 0.12, "whitecaps cover {white}");
        // No foam at all when it is turned off.
        s.look.foam = 0.0;
        assert_eq!(bake("storm", &s, None, cat).unwrap().stats.white_columns, 0);
    }

    /// How much the water level varies over a part of a build.
    fn roughness(b: &Baked, xs: std::ops::Range<i32>, zs: std::ops::Range<i32>) -> f64 {
        let tops: Vec<f64> = zs
            .flat_map(|z| xs.clone().map(move |x| (x, z)))
            .map(|(x, z)| b.water_top[(z * b.width + x) as usize])
            .filter(|t| *t != DRY)
            .map(|t| t as f64)
            .collect();
        let mean = tops.iter().sum::<f64>() / tops.len() as f64;
        (tops.iter().map(|t| (t - mean).powi(2)).sum::<f64>() / tops.len() as f64).sqrt()
    }

    /// The bed of a keep-dry area is never given a sheet of water: beside a
    /// build brought in as the land there is no ground to lay one on (once
    /// laid at the "no ground" level, it made the build two billion blocks
    /// tall and filled the memory), and out at sea it would undo the area.
    #[cfg(feature = "dev")]
    #[test]
    fn keep_dry_areas_stay_dry_beside_a_build_and_out_at_sea() {
        use of_formats::model::{BlockState, Build, Region, Volume};
        use std::sync::Arc;
        let cat = catalogue::default();
        let dry = |at: [f64; 2], radius: f64| Zone { kind: ZoneKind::Dry, at, radius, strength: 0.0 };
        let empty_circle = |b: &Baked, at: [f64; 2], radius: f64| {
            let (cx, cz) = (at[0] * b.width as f64, at[1] * b.length as f64);
            for z in 0..b.length {
                for x in 0..b.width {
                    let d = ((x as f64 + 0.5 - cx).powi(2) + (z as f64 + 0.5 - cz).powi(2)).sqrt();
                    if d <= radius {
                        assert_eq!(b.water_top[(z * b.width + x) as usize], DRY, "water at {x},{z} in the keep-dry area");
                    }
                }
            }
        };

        // A stone pillar 8 x 12 x 8 brought in as the land, a third under water.
        let mut pillar = Build::new();
        let stone = pillar.palette.intern(BlockState::new("stone"));
        let mut cells = Volume::new();
        for y in 0..12 {
            for z in 0..8 {
                for x in 0..8 {
                    cells.set([x, y, z], stone);
                }
            }
        }
        let mut region = Region::new("pillar", [0, 0, 0], [8, 12, 8]);
        region.cells = Arc::new(cells);
        pillar.regions.push(region);
        let ground = Terrain::from_build(96, 96, &Arc::new(pillar), 0.3, [0, 0]).unwrap();
        let mut s = settings(96, 96);
        s.zones = vec![dry([0.2, 0.2], 6.0), dry([0.5, 0.5], 9.0)];
        let baked = bake("pillar", &s, Some(&ground), cat).unwrap();
        empty_circle(&baked, [0.2, 0.2], 6.0);
        assert!(baked.sea_level.abs() < 64, "the build reaches {} blocks below the water", baked.sea_level);
        assert!(baked.water_top.iter().any(|t| *t != DRY), "the sea is still there");

        // Coast: a keep-dry area out at sea has bare sea bed, not a sheet.
        let land = coast_scene(160, 112, 7, s.waves.direction);
        let mut s = settings(160, 112);
        s.zones = vec![dry([0.5, 0.85], 8.0)];
        empty_circle(&bake("coast", &s, Some(&land), cat).unwrap(), [0.5, 0.85], 8.0);

        // A height gone wrong is refused before anything is placed.
        assert!(build::tall_enough(-10, 200).is_ok());
        assert!(build::tall_enough(-2000, 2063).is_ok());
        let err = build::tall_enough(i32::MIN + 1, 5).unwrap_err();
        assert!(err.contains("blocks tall"), "{err}");
    }

    #[test]
    fn calm_water_is_smoother_and_a_keep_dry_area_has_no_water() {
        let cat = catalogue::default();
        let mut s = settings(128, 128);
        let open = bake("open", &s, None, cat).unwrap();
        s.zones = vec![
            Zone { kind: ZoneKind::Calm, at: [0.5, 0.5], radius: 40.0, strength: 1.0 },
            Zone { kind: ZoneKind::Dry, at: [0.2, 0.2], radius: 8.0, strength: 0.0 },
        ];
        let zoned = bake("zoned", &s, None, cat).unwrap();
        // The same square of sea, calmed; wide enough that the sea's own
        // bends cannot leave a patch sitting on a flat trough by luck.
        let (rough, calm) = (roughness(&open, 44..84, 44..84), roughness(&zoned, 44..84, 44..84));
        assert!(rough > 1.0 && calm < 0.2 * rough, "calm water {calm} against {rough}");
        // Beyond the zones the sea is as it was.
        for z in 110..128 {
            for x in 110..128 {
                let i = (z * 128 + x) as usize;
                assert_eq!(open.water_top[i], zoned.water_top[i]);
            }
        }
        // No water in the keep-dry circle, and water just outside it.
        let centre = (0.2 * 128.0, 0.2 * 128.0);
        for z in 0..128 {
            for x in 0..128 {
                let d = ((x as f64 + 0.5 - centre.0).powi(2) + (z as f64 + 0.5 - centre.1).powi(2)).sqrt();
                let top = zoned.water_top[(z * 128 + x) as usize];
                if d <= 8.0 {
                    assert_eq!(top, DRY, "water at {x},{z} in the keep-dry area");
                } else if d > 9.5 && d < 30.0 {
                    assert_ne!(top, DRY, "no water at {x},{z} outside the keep-dry area");
                }
            }
        }
        // A storm calmed all over has no whitecaps.
        (s.waves.height, s.waves.peak_distance, s.waves.irregularity) = (14.0, 56.0, 0.8);
        s.zones = vec![Zone { kind: ZoneKind::Calm, at: [0.5, 0.5], radius: 400.0, strength: 1.0 }];
        assert_eq!(bake("calmed", &s, None, cat).unwrap().stats.white_columns, 0);
        // Zones are kept with the scene.
        let mut scene = SceneSetup::of(Land::Open, 1);
        scene.zones = s.zones.clone();
        assert_eq!(SceneSetup::from_text(&scene.to_text()).unwrap().zones, s.zones);
        assert_eq!(Zone::parse(ZoneKind::Dry, "0.3,0.4,12").unwrap().radius, 12.0);
        assert!(Zone::parse(ZoneKind::Calm, "0.3").is_err());
    }

    #[test]
    fn an_island_shelters_the_water_behind_it() {
        let cat = catalogue::default();
        let (w, l) = (200, 140);
        let mut s = settings(w, l);
        s.waves.direction = 0.0;
        // Deep water with one long island across the path of the waves.
        let mut height = vec![-40.0f32; (w * l) as usize];
        for z in 30..110 {
            for x in 80..96 {
                height[(z * w + x) as usize] = 5.0;
            }
        }
        let land = Terrain::from_heights(w, l, -10, height, vec![Ground::Rock; (w * l) as usize]);
        let b = bake("lee", &s, Some(&land), cat).unwrap();
        let (open, lee) = (roughness(&b, 20..70, 45..95), roughness(&b, 100..130, 55..85));
        assert!(open > 1.0, "the open sea is flat: {open}");
        // Half the height is still a shelter: a patch of a sea whose crests
        // wander never sits at exactly the same depth in the lee twice.
        assert!(lee < 0.5 * open, "no shelter: {lee} behind the island against {open} in front");
        // Far to the side of the island the sea is as rough as in front of it.
        let beside = roughness(&b, 100..150, 0..18);
        assert!(beside > 0.6 * open, "the sea beside the island is too calm: {beside} against {open}");
    }

    #[test]
    fn shelter_and_distance_have_no_seams_behind_a_corner() {
        let (w, l) = (240, 200);
        // Deep water with a square island; the waves run toward +X.
        let mut height = vec![-40.0f32; (w * l) as usize];
        for z in 60..100 {
            for x in 40..80 {
                height[(z * w + x) as usize] = 5.0;
            }
        }
        let land = Terrain::from_heights(w, l, -10, height, vec![Ground::Rock; (w * l) as usize]);

        // Distance from the shore is straight-line distance, the same in
        // every direction from the corner, so its slope turns smoothly.
        let survey = Survey::new(&land);
        for k in 0..=18 {
            let a = (k as f64 * 5.0).to_radians();
            let (x, z) = (79.5 + 30.0 * a.cos(), 99.5 + 30.0 * a.sin());
            let off = survey.off(x as i32, z as i32);
            let (cx, cz) = (x.floor() + 0.5, z.floor() + 0.5);
            let truth = (cx - cx.clamp(40.5, 79.5)).hypot(cz - cz.clamp(60.5, 99.5)) - 0.5;
            assert!((off - truth).abs() < 0.1, "{off} off the shore at {k} degrees where it is {truth}");
        }

        // Behind the island the shelter fades out smoothly past the corner,
        // with no step where one of the lines looked along clears the land.
        let mut s = settings(w, l);
        (s.waves.direction, s.waves.irregularity) = (0.0, 0.0);
        let field = WaveField::with(&s.waves, &Tuning::default().waves);
        let shore = Shore::new(&land, &field, s.waves.height, &Tuning::default().near_land, &Tuning::default().outflow, None, 1.0, &Progress::none(), (0.0, 1.0)).unwrap();
        for x in [120, 160, 200] {
            let along: Vec<(f64, f64)> = (60..200).map(|z| (shore.near(x, z).long, shore.near(x, z).short)).collect();
            let step = along.windows(2).map(|p| (p[1].0 - p[0].0).abs().max((p[1].1 - p[0].1).abs())).fold(0.0, f64::max);
            // The edge of the shelter is soft but can be steep close behind
            // the island; a seam left by one line would be a jump on its own.
            // A three block march trades a little smoothness for speed, so
            // the reading wobbles by a few hundredths from column to column.
            assert!(step < 0.1, "the shelter jumps by {step} between neighbours at x = {x}");
            // A soft shelter field is wider than a sharp one was, so the
            // test asks only that the water behind the island is clearly
            // calmer than open water: a fifth of the height gone at 20
            // blocks behind, and nearly all of it back clear of the island.
            assert!(along[20].0 + 0.15 < along[139].0 && along[139].0 > 0.95, "shelter {:?} behind and {:?} clear of the island", along[20], along[139]);
        }
    }

    #[test]
    fn waves_shrink_and_turn_toward_a_beach() {
        let cat = catalogue::default();
        let (w, l) = (160, 160);
        let mut s = settings(w, l);
        // Waves come in at an angle to a beach that shelves up toward +Z.
        s.waves.direction = 40.0;
        s.waves.irregularity = 0.0;
        // Spray lifts blocks of its own against the shore; this test is about
        // the swell of the waves alone.
        s.look.spray = 0.0;
        let depth_at = |z: i32| 30.0 - 30.0 * z as f32 / 130.0;
        let height: Vec<f32> = (0..w * l).map(|i| -depth_at(i / w)).collect();
        let land = Terrain::from_heights(w, l, -10, height, vec![Ground::Sand; (w * l) as usize]);
        let b = bake("beach", &s, Some(&land), cat).unwrap();
        let (deep, shallow) = (roughness(&b, 0..w, 5..40), roughness(&b, 0..w, 118..126));
        assert!(shallow < 0.5 * deep, "waves keep their height in the shallows: {shallow} against {deep}");

        // The crests lie along lines of equal delay. In deep water there is
        // none; in the shallows the delay grows toward the beach, which is
        // what turns the crests to face it.
        let field = WaveField::with(&s.waves, &Tuning::default().waves);
        let shore = Shore::new(&land, &field, s.waves.height, &Tuning::default().near_land, &Tuning::default().outflow, None, 1.0, &Progress::none(), (0.0, 1.0)).unwrap();
        assert!(shore.near(80, 10).lag.abs() < 1.0);
        assert!(shore.near(80, 120).lag > shore.near(80, 90).lag + 3.0);
        let lags = [shore.near(20, 120).lag, shore.near(80, 120).lag, shore.near(140, 120).lag, shore.near(80, 90).lag, shore.near(80, 10).lag];
        assert!((lags[0] - lags[2]).abs() < 0.2 * lags[1], "delays along the beach differ: {lags:?}");
        assert!(shore.near(80, 120).squeeze > 1.5 && shore.surf(80, 124) > 0.5 && shore.surf(80, 10) == 0.0);
    }

    fn check_shape(b: &Baked, thickness: i32) {
        let (w, l) = (b.width, b.length);
        let at = |x: i32, z: i32| (z * w + x) as usize;
        let all = blocks(b);
        let (foundation, surface, glass1) = (1, 2, 3);
        assert_eq!(b.layers().conflicts(&Layer::ALL), 0, "layers overlap");
        for z in 0..l {
            for x in 0..w {
                let (top, bottom) = (b.water_top[at(x, z)], b.water_bottom[at(x, z)]);
                if top == DRY {
                    continue;
                }
                assert!(top - bottom + 1 >= thickness || b.layers().layer(Layer::Terrain).get([x, bottom - 1, z]).is_some());
                // One unbroken run of water blocks, coloured on top, glass above.
                for y in bottom..=top {
                    assert!(all.contains_key(&(foundation, [x, y, z])) || all.contains_key(&(surface, [x, y, z])), "gap at {x},{y},{z}");
                }
                assert!(all.contains_key(&(surface, [x, top, z])), "uncoloured top at {x},{z}");
                assert!(all.contains_key(&(glass1, [x, top + 1, z])), "no glass at {x},{z}");
                let mut lowest_side = top;
                for (dx, dz) in [(-1, 0), (1, 0), (0, -1), (0, 1), (-1, -1), (1, -1), (-1, 1), (1, 1)] {
                    let (nx, nz) = (x + dx, z + dz);
                    if nx < 0 || nz < 0 || nx >= w || nz >= l || b.water_top[at(nx, nz)] == DRY {
                        continue;
                    }
                    // No hole between neighbours, even diagonally.
                    assert!(bottom <= b.water_top[at(nx, nz)], "hole between {x},{z} and {nx},{nz}");
                    if dx == 0 || dz == 0 {
                        lowest_side = lowest_side.min(b.water_top[at(nx, nz)]);
                    }
                }
                // Coating only where a face shows: never under the shell.
                for y in bottom..top {
                    if all.contains_key(&(surface, [x, y, z])) {
                        assert!(y > lowest_side, "hidden coating at {x},{y},{z}");
                    }
                }
            }
        }
    }

    #[test]
    fn shell_is_closed_and_coated_only_where_it_shows() {
        let cat = catalogue::default();
        let mut s = settings(72, 72);
        (s.waves.height, s.waves.peak_distance, s.waves.crest_sharpness) = (20.0, 24.0, 1.0);
        for thickness in [1, 3] {
            s.thickness = thickness;
            check_shape(&bake("steep", &s, None, cat).unwrap(), thickness);
        }
    }

    #[test]
    fn filled_build_reaches_one_flat_base() {
        let cat = catalogue::default();
        let mut s = settings(40, 40);
        s.filled = true;
        let b = bake("filled", &s, None, cat).unwrap();
        assert!(b.water_bottom.iter().all(|y| *y == 0));
        check_shape(&b, 1);
    }

    #[test]
    fn without_colour_and_glass_only_foundation_concrete_is_used() {
        let cat = catalogue::default();
        let mut s = settings(40, 40);
        s.colour_glass = false;
        s.foundation = "cyan".into();
        let b = bake("plain", &s, None, cat).unwrap();
        assert_eq!(b.stats.used.len(), 1);
        assert_eq!(b.stats.used[0].0, "minecraft:cyan_concrete");
        assert!(b.stats.blocks[1] > 0 && b.stats.blocks[2..].iter().all(|n| *n == 0));
    }

    #[test]
    fn light_colour_changes_nothing_when_fully_overcast() {
        let cat = catalogue::default();
        let mut s = settings(48, 48);
        // Reflections have a colour of their own here, so that only the
        // sun could bring out the light colour.
        s.look.reflection_auto = false;
        s.look.lit_strength = 0.0;
        let a = bake("overcast", &s, None, cat).unwrap();
        s.look.light = [255, 200, 40];
        assert_eq!(blocks(&a), blocks(&bake("overcast", &s, None, cat).unwrap()));
        // With sun, the same change does alter the blocks.
        s.look.lit_strength = 0.85;
        assert_ne!(blocks(&a), blocks(&bake("overcast", &s, None, cat).unwrap()));
    }

    #[test]
    fn black_can_be_chosen_and_the_sea_is_then_black() {
        let cat = catalogue::default();
        let mut s = settings(48, 48);
        // No white water, no reflections: only the two colours are left.
        (s.look.foam, s.look.reflection) = (0.0, 0.0);
        // The brightest part of any colour the columns aim for.
        let brightest = |b: &Baked| b.target.iter().flat_map(|c| c.iter()).copied().max().unwrap();
        // Both colours black: every column aims for black.
        (s.look.dark, s.look.light) = ([0, 0, 0], [0, 0, 0]);
        let b = bake("black", &s, None, cat).unwrap();
        assert_eq!(brightest(&b), 0, "the target must be black everywhere");
        assert!(b.stats.used.iter().any(|(n, _)| n.contains("black")), "{:?}", b.stats.used);
        // A black dark colour under a light one: the sea runs between them.
        s.look.light = [104, 199, 233];
        let b = bake("black", &s, None, cat).unwrap();
        assert!(brightest(&b) > 60);
        // No daylight at all is black as well, whatever the colours.
        s.look.dark = [17, 102, 176];
        s.look.daylight = 0.0;
        let b = bake("black", &s, None, cat).unwrap();
        assert_eq!(brightest(&b), 0);
    }

    #[test]
    fn block_limits_are_respected() {
        let cat = catalogue::default();
        let mut s = settings(64, 64);
        (s.look.max_solids, s.look.max_glass) = (3, 1);
        let b = bake("few", &s, None, cat).unwrap();
        let glass = b.stats.used.iter().filter(|(n, _)| n.ends_with("stained_glass")).count();
        // Three surface solids plus the foundation concrete.
        assert!(glass == 1 && b.stats.used.len() - glass <= 4, "{:?}", b.stats.used);
    }

    #[cfg(feature = "dev")]
    #[test]
    fn coast_scene_has_every_shore_and_water_never_replaces_ground() {
        let cat = catalogue::default();
        let mut s = settings(160, 112);
        s.waves.direction = 200.0;
        let land = coast_scene(160, 112, 7, s.waves.direction);
        let b = bake("coast", &s, Some(&land), cat).unwrap();
        assert_eq!(b.layers().conflicts(&Layer::ALL), 0);
        for name in ["minecraft:yellow_concrete", "minecraft:lime_concrete", "minecraft:light_gray_concrete"] {
            assert!(b.stats.used.iter().any(|(n, c)| n == name && *c > 200), "{name} is missing");
        }
        let wet = b.stats.water_columns as f64 / (160.0 * 112.0);
        assert!(wet > 0.45 && wet < 0.85, "water covers {wet}");
        // Dry land stands above the water everywhere it is dry.
        let dry = b.water_top.iter().filter(|t| **t == DRY).count();
        assert!(dry > 2000);

        let options = ExportOptions {
            format: Format::Litematic,
            selection: Selection::Complete,
            air: AirMode::Compact,
            include_colour_glass: true,
        };
        let out = export::export(&b.source(None), &options, cat, &Progress::none()).unwrap();
        let back = of_formats::import(&out.bytes, &Progress::none()).unwrap().build;
        assert_eq!(back.counts().0, b.stats.blocks.iter().sum::<u64>());
    }

    /// How many columns of the ground are of one kind.
    fn count(land: &Terrain, kind: Ground) -> usize {
        (0..land.length).flat_map(|z| (0..land.width).map(move |x| (x, z))).filter(|(x, z)| land.kind(*x, *z) == kind).count()
    }

    #[cfg(feature = "dev")]
    #[test]
    fn rivers_run_to_the_sea_and_fall_where_the_ground_drops() {
        let cat = catalogue::default();
        let mut s = settings(320, 224);
        s.waves.direction = 300.0;
        s.waves.seed = 5;

        // The cliff coast: an arch, and a stream that falls into the cove.
        let scene = SceneSetup::of(Land::Cliffs, 5);
        let bare = scene.bare(320, 224, 300.0).unwrap();
        assert_eq!(count(&bare, Ground::Stream) + count(&bare, Ground::Fall), 0, "running water is never part of the land");
        assert!((0..224).any(|z| (0..320).any(|x| bare.roof(x, z).is_some())), "the arch is missing");
        let land = scene.terrain(320, 224, 300.0).unwrap();
        assert!(count(&land, Ground::Stream) > 100 && count(&land, Ground::Fall) > 4);
        let dry = bake("cliffs", &s, Some(&bare), cat).unwrap();
        let wet = bake("cliffs", &s, Some(&land), cat).unwrap();
        assert_eq!(wet.layers().conflicts(&Layer::ALL), 0);
        assert!(
            wet.stats.white_columns > dry.stats.white_columns + 40,
            "the fall whitens the sea: {} against {}",
            wet.stats.white_columns,
            dry.stats.white_columns
        );

        // The same stream moved to where the ground is different: over the
        // cliffs in the west it falls too; down the valley to the beach it
        // only runs.
        let moved = |from: [f64; 2], to: [f64; 2]| {
            let scene = SceneSetup { streams: vec![Stream { from, to, ..Stream::default() }], ..scene.clone() };
            scene.terrain(320, 224, 300.0).unwrap()
        };
        let west = moved([0.2, 0.0], [0.22, 0.45]);
        assert!(count(&west, Ground::Stream) > 100 && count(&west, Ground::Fall) > 4);
        let valley = moved([0.6, 0.0], [0.62, 0.4]);
        assert!(count(&valley, Ground::Stream) > 100);
        assert_eq!(count(&valley, Ground::Fall), 0);

        // On the other coast a river crosses the beach without a fall.
        let coast = SceneSetup::of(Land::Coast, 7).terrain(320, 224, 200.0).unwrap();
        assert!(count(&coast, Ground::Stream) > 150);
        assert_eq!(count(&coast, Ground::Fall), 0);
        let b = bake("coast", &s, Some(&coast), cat).unwrap();
        assert_eq!(b.layers().conflicts(&Layer::ALL), 0);

        // A scene survives being written down, and open sea has no ground.
        assert_eq!(SceneSetup::from_text(&scene.to_text()).unwrap(), scene);
        assert!(SceneSetup::of(Land::Open, 1).terrain(64, 64, 0.0).is_none());
    }

    #[cfg(feature = "dev")]
    #[test]
    fn ponds_fill_their_hollow_or_spill_over_its_lowest_rim() {
        // Land 10 blocks up with a pit 4 up in the middle of the north half,
        // a notch 7 up from the pit to the west edge, and sea in the south.
        let (w, l): (i32, i32) = (64, 64);
        let mut height = vec![10.0f32; (w * l) as usize];
        for z in 0..l {
            for x in 0..w {
                let i = (z * w + x) as usize;
                if z >= 40 {
                    height[i] = -3.0;
                } else if (x - 32).pow(2) + (z - 16).pow(2) <= 36 {
                    height[i] = 4.0;
                } else if z == 16 && x < 32 {
                    height[i] = 7.0;
                }
            }
        }
        let land = || Terrain::from_heights(w, l, -10, height.clone(), vec![Ground::Grass; (w * l) as usize]);
        let pond = |at: [f64; 2], depth: f64| {
            let mut t = land();
            let end = t.add_pond(&Pond { at, depth, dig: 0.0 });
            (t, end)
        };
        // Anywhere in the pit, as deep as asked.
        let (t, end) = pond([0.52, 0.27], 2.0);
        let PondEnd::Held { depth: 2, size } = end else { panic!("{end:?}") };
        assert!(size > 80 && size == count(&t, Ground::Pond), "{size}");
        assert_eq!(t.block_at(32, 16, 6), Ground::Pond);
        assert_eq!(t.block_at(32, 16, 4), Ground::Rock, "the bed stays ground");
        assert_eq!(t.top(32, 16), Some(6));
        // Deeper than the pit: out over the notch, off the edge of the plot.
        let (t, end) = pond([0.5, 0.25], 10.0);
        assert!(matches!(end, PondEnd::Spills { depth: 3, to: Spill::Edge, .. }), "{end:?}");
        assert_eq!(t.top(32, 16), Some(7));
        assert_eq!(t.kind(20, 16), Ground::Grass, "the notch is not flooded");
        // In the sea, and on flat ground with no hollow.
        assert_eq!(pond([0.5, 0.8], 3.0).1, PondEnd::Dry);
        let (t, end) = pond([0.1, 0.5], 3.0);
        assert!(matches!(end, PondEnd::Spills { depth: 0, size: 0, .. }), "{end:?}");
        assert_eq!(count(&t, Ground::Pond), 0);
        // There, a dug basin holds it.
        let mut t = land();
        let end = t.add_pond(&Pond { at: [0.1, 0.5], depth: 3.0, dig: 5.0 });
        assert!(matches!(end, PondEnd::Held { depth: 3, size } if size > 30), "{end:?}");

        // A scene with ponds lays them and is written down with them; one
        // without has no ponds in its text, as before.
        let mut scene = SceneSetup::of(Land::Coast, 7);
        assert!(!scene.to_text().contains("ponds"));
        scene.ponds.push(Pond { at: [0.5, 0.1], depth: 4.0, dig: 8.0 });
        assert_eq!(SceneSetup::from_text(&scene.to_text()).unwrap(), scene);
        assert_eq!(scene.terrain(320, 224, 200.0).unwrap().ponds().len(), 1);
    }

    /// A plot whose north is land sloping down to a beach and into the sea,
    /// with `drop` blocks of cliff at the shore.
    fn shelving(w: i32, l: i32, drop: f32) -> Terrain {
        let height: Vec<f32> = (0..w * l)
            .map(|i| {
                let z = (i / w) as f32;
                let slope = 8.0 - 0.1 * z;
                if slope > 0.0 { slope + drop } else { slope * 3.0 - drop }
            })
            .collect();
        Terrain::from_heights(w, l, -10, height, vec![Ground::Sand; (w * l) as usize])
    }

    #[test]
    fn a_river_builds_a_delta_on_a_beach_and_none_under_a_fall() {
        let (w, l) = (200, 200);
        let dry = |land: &Terrain| (0..l).flat_map(|z| (0..w).map(move |x| (x, z))).filter(|(x, z)| *z >= 82 && land.height(*x, *z) > 0.05 && land.kind(*x, *z) != Ground::Fall).count();
        let river = |delta: f64, drop: f32, seed: u32| {
            let mut land = shelving(w, l, drop);
            land.add_stream(&Stream { from: [0.5, 0.0], to: [0.5, 0.7], width: 6.0, winding: 0.3, delta }, seed);
            land
        };
        assert_eq!(dry(&shelving(w, l, 0.0)), 0);

        // No delta asked for: one plain mouth and the shore where it was.
        let plain = river(0.0, 0.0, 3);
        assert_eq!(plain.mouths().len(), 1);
        assert!(!plain.mouths()[0].fell && plain.mouths()[0].toward.1 > 0.7);
        assert_eq!(dry(&plain), 0);

        // A delta: the channel has divided and sand stands where sea was.
        let small = river(0.3, 0.0, 3);
        let broad = river(1.0, 0.0, 3);
        assert_eq!(small.mouths().len(), 2);
        assert!(broad.mouths().len() >= 4, "{} mouths", broad.mouths().len());
        assert!(dry(&small) > 40 && dry(&broad) > 3 * dry(&small), "{} and {}", dry(&small), dry(&broad));
        for mouth in broad.mouths() {
            assert!(!mouth.fell && mouth.at.1 > 80.0 && mouth.toward.1 > 0.0, "{mouth:?}");
            // Every channel ends in the sea, not shut in by its own sand.
            assert!(broad.height(mouth.at.0 as i32, mouth.at.1 as i32) <= 0.0);
        }
        // Its sand lies unevenly, not as a shape mirrored about the river.
        let lean = |land: &Terrain| {
            let side = |from: i32, to: i32| (82..l).flat_map(|z| (from..to).map(move |x| (x, z))).filter(|(x, z)| land.height(*x, *z) > 0.05).count() as f64;
            side(0, 100) / side(100, w).max(1.0)
        };
        assert!((lean(&broad) - 1.0).abs() > 0.05, "the delta is symmetric: {}", lean(&broad));
        // Another seed lays another delta.
        let other = river(1.0, 0.0, 4);
        assert!((0..l).any(|z| (0..w).any(|x| (other.height(x, z) >= 0.0) != (broad.height(x, z) >= 0.0))));

        // Over a cliff it falls, and there is no delta whatever was asked.
        let fall = river(1.0, 9.0, 3);
        assert_eq!(fall.mouths().len(), 1);
        assert!(fall.mouths()[0].fell);
        assert_eq!(dry(&fall), 0);
        assert!(count(&fall, Ground::Fall) > 4);
    }

    #[test]
    fn waves_steepen_against_a_river_running_out() {
        let (w, l) = (200, 200);
        let mut s = settings(w, l);
        // A low swell running straight in toward the beach in the north.
        (s.waves.direction, s.waves.height, s.waves.irregularity) = (270.0, 2.0, 0.0);
        let mut land = shelving(w, l, 0.0);
        land.add_stream(&Stream { from: [0.5, 0.0], to: [0.5, 0.7], width: 8.0, winding: 0.0, delta: 0.0 }, 3);
        let mouth = land.mouths()[0];
        let field = WaveField::with(&s.waves, &Tuning::default().waves);
        let tuning = Tuning::default();
        let shore = |outflow: &tuning::OutflowTuning| Shore::new(&land, &field, s.waves.height, &tuning.near_land, outflow, None, 1.0, &Progress::none(), (0.0, 1.0)).unwrap();
        let (with, without) = (shore(&tuning.outflow), shore(&tuning::OutflowTuning { speed: 0.0, ..tuning.outflow.clone() }));
        let (x, z) = (mouth.at.0 as i32, mouth.at.1 as i32);
        // In the river's path the long waves stand taller and closer, the
        // short ones are held back, and the river's water shows.
        let (a, b) = (with.near(x, z + 16), without.near(x, z + 16));
        assert!(a.long > 1.1 * b.long, "{} against {}", a.long, b.long);
        assert!(a.squeeze > b.squeeze);
        assert!(with.plume(x, z + 6) > 0.5 && without.plume(x, z + 6) == 0.0);
        // Well to the side and far out nothing has changed.
        for (x, z) in [(x + 80, z + 16), (x, z + 112)] {
            assert!((with.near(x, z).long - without.near(x, z).long).abs() < 0.03);
            assert!(with.plume(x, z) < 0.05);
        }
    }

    /// A plot with land in the north and three shores side by side: a low
    /// beach in the west with a narrow inlet running up into it, a cliff
    /// in the middle and low rock in the east.
    fn three_shores() -> Terrain {
        let (w, l) = (320, 200);
        let (mut height, mut kind) = (Vec::new(), Vec::new());
        for z in 0..l {
            for x in 0..w {
                // Blocks in from the shore; negative out at sea.
                let inland = (80 - z) as f32;
                let (h, k) = if x < 120 {
                    if (20..30).contains(&x) && (10..80).contains(&z) {
                        (-2.0, Ground::Sand)
                    } else if inland > 0.0 {
                        (0.1 * inland, Ground::Sand)
                    } else {
                        (-0.6 + 0.15 * inland, Ground::Sand)
                    }
                } else if x < 220 {
                    (if inland > 0.0 { 12.0 } else { -8.0 }, Ground::Rock)
                } else if inland > 0.0 {
                    ((0.6 + 0.3 * inland).min(6.0), Ground::Rock)
                } else {
                    (-1.0 + 0.4 * inland, Ground::Rock)
                };
                height.push(h);
                kind.push(k);
            }
        }
        Terrain::from_heights(w, l, -12, height, kind)
    }

    #[cfg(feature = "dev")]
    #[test]
    fn the_shore_is_read_from_the_shape_of_the_ground() {
        let land = three_shores();
        let survey = Survey::new(&land);
        // Each stretch of shore is told from its shape, and so is the sea in front of it.
        for (x, kind) in [(80, ShoreKind::Beach), (170, ShoreKind::Cliff), (270, ShoreKind::Rocky)] {
            assert_eq!(survey.shore(x, 79), Some(kind), "the shore at {x}");
            assert_eq!(survey.nearest(x, 90).map(|n| n.0), Some(kind), "the sea at {x}");
            assert_eq!(read_shore(&land, (x as f64 + 0.5, 79.5), (0.0, 1.0)).kind, kind);
        }
        assert_eq!(survey.shore(80, 120), None);
        let (cliff, rocky) = survey.character(80, 86);
        assert!(cliff < 0.02 && rocky < 0.02, "beach: {cliff} {rocky}");
        assert!(survey.character(170, 86).0 > 0.95 && survey.character(270, 86).1 > 0.95);
        let (kinds, channel) = survey.tally();
        assert!(kinds.iter().all(|n| *n > 60), "{kinds:?}");

        // There is room for a delta on the beach, little on rock, none under a cliff.
        let room = |x: i32| read_shore(&land, (x as f64 + 0.5, 79.5), (0.0, 1.0)).delta_room;
        assert!(room(80) > 0.8 && room(270) <= 0.3 && room(170) == 0.0, "{} {} {}", room(80), room(270), room(170));

        // The inlet is a river channel; the open sea is not.
        assert!(survey.channel(25, 30) && survey.channel(25, 60));
        assert!(!survey.channel(25, 110) && !survey.channel(170, 100) && !survey.channel(80, 40));
        assert!(channel > 300 && channel < 1200, "{channel} columns of channel");
        // Narrow water that lies open to the sea is no river: the passage
        // under the arch of the cliff coast, between its pier and the headland.
        for seed in [5, 7, 31] {
            let (_, under_arch) = Survey::new(&cliff_scene(320, 224, seed)).tally();
            assert_eq!(under_arch, 0, "land shape {seed}");
        }

        // In front of the cliff a column's image lies straight behind the shore.
        let ((mx, mz), firm) = survey.mirror(170, 90).unwrap();
        assert!(firm > 0.9 && mx.abs() < 1.0 && (mz + 2.0 * survey.off(170, 90)).abs() < 1.5, "{mx} {mz} {firm}");
        assert!((survey.off(170, 90) - 10.5).abs() < 0.6 && survey.off(170, 60) < 0.0);

        // A river ends as the shore it meets lets it.
        let river = |x: f64, delta: f64| three_shores().add_stream(&Stream { from: [x / 320.0, 0.0], to: [x / 320.0, 0.7], width: 6.0, winding: 0.0, delta }, 3);
        assert!(matches!(river(80.0, 0.8), Outcome::Delta(size) if size > 0.6), "{:?}", river(80.0, 0.8));
        assert_eq!(river(80.0, 0.0), Outcome::Mouth(ShoreKind::Beach));
        assert_eq!(river(270.0, 0.3), Outcome::Mouth(ShoreKind::Rocky));
        assert!(matches!(river(270.0, 1.0), Outcome::Delta(size) if size <= 0.3), "{:?}", river(270.0, 1.0));
        assert_eq!(river(170.0, 1.0), Outcome::Fall);
    }

    #[test]
    fn a_cliff_throws_waves_back_and_a_beach_does_not() {
        let cat = catalogue::default();
        let land = three_shores();
        let mut s = settings(320, 200);
        // A swell running straight in toward the shore in the north.
        (s.waves.direction, s.waves.height, s.waves.peak_distance, s.waves.irregularity) = (270.0, 4.0, 40.0, 0.3);
        let with = bake("thrown", &s, Some(&land), cat).unwrap();
        let mut still = Tuning::default();
        (still.reflection.cliff, still.reflection.rocky) = (0.0, 0.0);
        let without = bake_with("thrown", &s, Some(&land), cat, &still, &Progress::none()).unwrap();
        let changed = |xs: std::ops::Range<i32>, zs: std::ops::Range<i32>| {
            zs.flat_map(|z| xs.clone().map(move |x| (z * 320 + x) as usize)).filter(|i| with.water_top[*i] != without.water_top[*i]).count()
        };
        // The sea in front of the cliff is the waves coming and going; in
        // front of the beach, and far out, it is as it was.
        let (cliff, rocky, beach) = (changed(140..200, 80..104), changed(250..290, 84..100), changed(60..100, 84..100));
        assert!(cliff > 60, "{cliff} columns changed before the cliff");
        assert!(rocky < cliff, "{rocky} before the rocks, {cliff} before the cliff");
        assert_eq!(beach, 0);
        assert_eq!(changed(0..320, 192..200), 0);
        assert_eq!(with.layers().conflicts(&Layer::ALL), 0);
    }

    #[test]
    fn pictures_show_the_build() {
        let cat = catalogue::default();
        let b = bake("picture", &settings(32, 32), None, cat).unwrap();
        let layers = Layer::ALL;
        let map = render::render(&b.source(None), &layers, cat, &render::Camera::map(2.0)).unwrap();
        assert_eq!((map.width, map.height), (32 * 2 + 24, 32 * 2 + 24));
        // The middle of the map is water, not background.
        let mid = ((map.height / 2 * map.width + map.width / 2) * 4) as usize;
        assert!(map.rgba[mid + 2] > map.rgba[mid], "expected blue, got {:?}", &map.rgba[mid..mid + 3]);
        let view = render::render(&b.source(None), &layers, cat, &render::Camera::angled((-0.3, 1.0), 32.0, 3.0)).unwrap();
        assert!(view.width > 100 && view.height > 60);
        assert_eq!(view.rgba.len(), (view.width * view.height * 4) as usize);
    }

    /// What a file holds once read back: every block that is not air, by place.
    #[cfg_attr(not(feature = "dev"), allow(dead_code))]
    fn read_back(bytes: &[u8]) -> BTreeMap<[i32; 3], String> {
        let build = of_formats::import(bytes, &Progress::none()).unwrap().build;
        let mut all = BTreeMap::new();
        for region in &build.regions {
            region.cells.for_each(|p, id| {
                let state = build.palette.get(id);
                if region.contains(p) && !state.is_air() {
                    all.insert(p, state.to_string());
                }
            });
        }
        all
    }

    /// Stage 4: the files read straight from the columns hold the same
    /// blocks at the same places as those written from the 3D layers, for
    /// every part, format and air setting, the whole plot or an area of it.
    #[cfg(feature = "dev")]
    #[test]
    fn files_read_from_the_columns_match_those_from_the_layers() {
        use of_formats::model::{BlockState, Build, Region, Volume};
        use std::sync::Arc;
        let cat = catalogue::default();
        // A stone pillar with a granite cap whose region overlaps it.
        let mut pillar = Build::new();
        let stone = pillar.palette.intern(BlockState::new("stone"));
        let granite = pillar.palette.intern(BlockState::new("granite"));
        let (mut body, mut cap) = (Volume::new(), Volume::new());
        for y in 0..12 {
            for z in 0..8 {
                for x in 0..8 {
                    body.set([x, y, z], stone);
                    if y >= 8 && (2..6).contains(&x) && (2..6).contains(&z) {
                        cap.set([x, y, z], granite);
                    }
                }
            }
        }
        let mut region = Region::new("body", [0, 0, 0], [8, 12, 8]);
        region.cells = Arc::new(body);
        pillar.regions.push(region);
        let mut region = Region::new("cap", [2, 8, 2], [4, 4, 4]);
        region.cells = Arc::new(cap);
        pillar.regions.push(region);

        let s = settings(72, 56);
        let scenes = [
            ("open", None),
            ("coast", Some(coast_scene(72, 56, 7, s.waves.direction))),
            ("cliffs", Some(cliff_scene(72, 56, 3))),
            ("pillar", Some(Terrain::from_build(72, 56, &Arc::new(pillar), 0.3, [20, 16]).unwrap())),
        ];
        for (name, land) in &scenes {
            let baked = bake(name, &s, land.as_ref(), cat).unwrap();
            let layers = baked.layers();
            for area in [None, Some([10, 5, 40, 30]), Some([-8, 20, 200, 23])] {
                let new = baked.source(area);
                let part = area.map(|b| layers.cropped([b[0], b[1]], [b[2], b[3]]));
                let old: &of_formats::export::LayeredBuild = part.as_ref().unwrap_or(&layers);
                for selection in Selection::ALL {
                    for format in [Format::Litematic, Format::Sponge, Format::Axiom] {
                        for air in [AirMode::Compact, AirMode::OmitAir] {
                            for glass in [true, false] {
                                let options = ExportOptions { format, selection, air, include_colour_glass: glass };
                                let what = format!("{name} {area:?} {selection:?} {format:?} {air:?} glass {glass}");
                                match (export::export(old, &options, cat, &Progress::none()), export::export(&new, &options, cat, &Progress::none())) {
                                    (Ok(a), Ok(b)) => {
                                        assert_eq!((a.blocks, a.regions, a.notes.len()), (b.blocks, b.regions, b.notes.len()), "{what}");
                                        assert!(read_back(&a.bytes) == read_back(&b.bytes), "{what}: the blocks differ");
                                    }
                                    (Err(a), Err(b)) => assert_eq!(a.key, b.key, "{what}"),
                                    (a, b) => panic!("{what}: {:?} against {:?}", a.err().map(|e| e.key), b.err().map(|e| e.key)),
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    /// A brought-in build's chest keeps its contents in the complete files
    /// (they were dropped before), sits on its own block, and the water-only
    /// files hold none. A chest under a later region's block is not kept.
    #[test]
    fn block_entities_of_a_build_reach_the_complete_files() {
        use of_formats::model::{BlockEntity, BlockState, Build, Region, Volume};
        use of_formats::nbt::{Compound, Tag};
        use std::sync::Arc;
        let cat = catalogue::default();
        let mut pillar = Build::new();
        let stone = pillar.palette.intern(BlockState::new("stone"));
        let chest = pillar.palette.intern(BlockState::new("chest"));
        let granite = pillar.palette.intern(BlockState::new("granite"));
        let (mut body, mut cap) = (Volume::new(), Volume::new());
        for y in 0..12 {
            for z in 0..8 {
                for x in 0..8 {
                    body.set([x, y, z], stone);
                }
            }
        }
        for y in 8..12 {
            for z in 2..6 {
                for x in 2..6 {
                    cap.set([x, y, z], granite);
                }
            }
        }
        body.set([0, 11, 0], chest);
        body.set([3, 9, 3], chest);
        let named = |pos, name: &str| BlockEntity {
            pos,
            id: Some("minecraft:chest".into()),
            data: Compound(vec![("CustomName".into(), Tag::String(name.into()))]),
        };
        let mut region = Region::new("body", [0, 0, 0], [8, 12, 8]);
        region.cells = Arc::new(body);
        region.block_entities = vec![named([0, 11, 0], "kept"), named([3, 9, 3], "covered")];
        pillar.regions.push(region);
        let mut region = Region::new("cap", [2, 8, 2], [4, 4, 4]);
        region.cells = Arc::new(cap);
        pillar.regions.push(region);

        let s = settings(64, 48);
        let ground = Terrain::from_build(64, 48, &Arc::new(pillar), 0.3, [20, 16]).unwrap();
        let baked = bake("pillar", &s, Some(&ground), cat).unwrap();
        for (format, selection, want) in [Format::Litematic, Format::Sponge, Format::Axiom]
            .into_iter()
            .flat_map(|f| [(f, Selection::Complete, 1), (f, Selection::WaterOnly, 0)])
        {
            let options = ExportOptions { format, selection, air: AirMode::Compact, include_colour_glass: true };
            let out = export::export(&baked.source(None), &options, cat, &Progress::none()).unwrap();
            let back = of_formats::import(&out.bytes, &Progress::none()).unwrap().build;
            let entities: Vec<_> = back.regions.iter().flat_map(|r| r.block_entities.iter()).collect();
            assert_eq!(entities.len(), want, "{format:?} {selection:?}");
            for be in entities {
                assert_eq!(be.data.str("CustomName"), Some("kept"), "{format:?}");
                let at = back.regions.iter().find_map(|r| r.cells.get(be.pos).filter(|_| r.contains(be.pos))).map(|id| back.palette.get(id).to_string());
                assert!(at.is_some_and(|b| b.contains("chest")), "{format:?}: the chest's data is not on the chest");
            }
        }
    }

    /// Rivers and ponds laid over a brought-in build: their water is built,
    /// the build is cut only in the columns they changed, a river lays no
    /// delta, and an export holds the same blocks as the bake.
    #[test]
    fn rivers_and_ponds_run_over_a_build() {
        use of_formats::model::{BlockState, Build, Region, Volume};
        use std::sync::Arc;
        let cat = catalogue::default();
        // A hill sloping down to the sea in the south.
        let mut hill = Build::new();
        let stone = hill.palette.intern(BlockState::new("stone"));
        let mut cells = Volume::new();
        for z in 0..60 {
            let top = 16 - z / 4;
            for y in 0..=top {
                for x in 0..96 {
                    cells.set([x, y, z], stone);
                }
            }
        }
        let mut region = Region::new("hill", [0, 0, 0], [96, 17, 60]);
        region.cells = Arc::new(cells);
        hill.regions.push(region);
        let hill = Arc::new(hill);

        let mut scene = SceneSetup::of(Land::Build, 3);
        scene.build.waterline = 0.25;
        scene.streams.push(Stream { from: [0.5, 0.02], to: [0.5, 0.95], width: 5.0, winding: 0.3, delta: 1.0 });
        scene.ponds.push(Pond { at: [0.2, 0.32], depth: 3.0, dig: 6.0 });
        let (ground, ends) = scene.laid_with(96, 96, 180.0, Some(&hill)).unwrap().unwrap();
        assert!(!matches!(ends[0], Outcome::Dry | Outcome::Delta(_)), "{:?}", ends[0]);
        assert!(matches!(ground.ponds()[0], PondEnd::Held { .. } | PondEnd::Spills { depth: 1.., .. }), "{:?}", ground.ponds()[0]);
        let reworked = ground.placed.as_ref().unwrap().reworked.clone();
        assert!(!reworked.is_empty());

        let baked = bake("hill", &settings(96, 96), Some(&ground), cat).unwrap();
        let name = |id: u32| baked.palette().get(id).to_string();
        let (mut stream, mut pond, mut stone_kept, mut stone_untouched) = (0, 0, 0, 0);
        let (ground_blocks, w) = (crate::terrain::TerrainBlocks::default(), 96);
        let changed: std::collections::HashSet<u32> = reworked.iter().copied().collect();
        baked.for_each_block(|layer, p, id| {
            if layer != Layer::Terrain {
                return;
            }
            let n = name(id);
            if n.contains(&ground_blocks.stream) {
                stream += 1;
            } else if n.contains(&ground_blocks.pond) {
                pond += 1;
            } else if n.contains("stone") {
                stone_kept += 1;
                if !changed.contains(&((p[2] * w + p[0]) as u32)) {
                    stone_untouched += 1;
                }
            }
        });
        assert!(stream > 50 && pond > 5, "stream {stream}, pond {pond}");
        // Columns no river or pond reached keep every block of the hill.
        let whole: i32 = (0..60).map(|z: i32| (16 - z / 4 + 1) * 96).sum();
        let placed = ground.placed.as_ref().unwrap();
        assert_eq!(placed.cut, 0);
        let shift = placed.shift;
        let cut_columns: i32 = reworked
            .iter()
            .map(|&c| ((c as i32) % w - shift[0], (c as i32) / w - shift[2]))
            .filter(|(x, z)| (0..96).contains(x) && (0..60).contains(z))
            .map(|(_, z)| 16 - z / 4 + 1)
            .sum();
        assert_eq!(stone_untouched, whole - cut_columns);
        assert!(stone_kept < whole);

        // The export reads the same blocks, row by row.
        let options = ExportOptions { format: Format::Sponge, selection: Selection::Complete, air: AirMode::Compact, include_colour_glass: true };
        let out = export::export(&baked.source(None), &options, cat, &Progress::none()).unwrap();
        let back = of_formats::import(&out.bytes, &Progress::none()).unwrap().build;
        let stone_name = BlockState::new("stone").to_string();
        let mut in_bake = BTreeMap::new();
        baked.for_each_block(|_, _, id| *in_bake.entry(name(id)).or_insert(0u64) += 1);
        let in_file: BTreeMap<String, u64> = back.histogram().into_iter().filter(|(b, _)| !b.is_air()).map(|(b, n)| (b.to_string(), n)).collect();
        assert_eq!(in_file.get(&stone_name), Some(&(stone_kept as u64)));
        assert_eq!(in_file, in_bake);
    }
}
