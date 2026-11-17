//! Commands that bake an ocean and write pictures and build files of it.

use of_core::params::{self, Mode, ParamValues};
use of_formats::export::{self, AirMode, ExportOptions, Layer, Selection};
use of_formats::model::Format;
use of_formats::{catalogue, Progress};
use of_ocean::render::{self, Camera, Picture};
use of_ocean::{bake_with, BuildPlacement, Land, OceanSettings, Outcome, Pond, PondEnd, SceneSetup, ShoreKind, Spill, Stream, Survey, Tuning, Zone, ZoneKind};
use std::sync::Arc;
use std::fmt::Write as _;
use std::path::Path;

fn apply(values: &mut ParamValues, assignment: &str) -> Result<(), String> {
    let (id, text) = assignment.split_once('=').ok_or_else(|| format!("expected id=value, got '{assignment}'"))?;
    let def = params::find(id).filter(|d| d.mode == Mode::Ocean).ok_or_else(|| format!("unknown ocean setting '{id}'"))?;
    values.set(def, crate::parse_value(def, text)?);
    Ok(())
}

/// A river given as `stream=x1,z1,x2,z2[,width[,winding]]`: where it rises
/// and where it is headed, as shares of the plot (0 to 1).
fn parse_stream(text: &str) -> Result<Stream, String> {
    let bad = || format!("expected stream=x1,z1,x2,z2[,width[,winding[,delta]]] with numbers, got '{text}'");
    let n: Vec<f64> = text.split(',').map(|v| v.trim().parse::<f64>().map_err(|_| bad())).collect::<Result<_, _>>()?;
    if n.len() < 4 || n.len() > 7 {
        return Err(bad());
    }
    let standard = Stream::default();
    Ok(Stream {
        from: [n[0], n[1]],
        to: [n[2], n[3]],
        width: n.get(4).copied().unwrap_or(standard.width),
        winding: n.get(5).copied().unwrap_or(standard.winding),
        delta: n.get(6).copied().unwrap_or(standard.delta),
    })
}

fn blocks(n: i32) -> String {
    if n == 1 { "1 block".into() } else { format!("{n} blocks") }
}

fn columns(n: usize) -> String {
    if n == 1 { "1 column".into() } else { format!("{n} columns") }
}

/// A part of the plot given as `area=west,north,east,south` in blocks.
fn parse_area(text: &str) -> Result<[i32; 4], String> {
    let bad = || format!("expected area=west,north,east,south in blocks, got '{text}'");
    let n: Vec<i32> = text.split(',').map(|v| v.trim().parse::<i32>().map_err(|_| bad())).collect::<Result<_, _>>()?;
    let &[w, north, e, s] = n.as_slice() else { return Err(bad()) };
    Ok([w.min(e), north.min(s), w.max(e), north.max(s)])
}

/// Read the ripples' image as the app does: a normal map, or grey heights.
fn read_pattern(file: &str, flip: of_ocean::pattern::FlipY) -> Result<Arc<of_ocean::pattern::NormalMap>, String> {
    let image = image::open(file).map_err(|e| format!("{file}: {e}"))?.to_rgb8();
    let (w, h) = (image.width() as usize, image.height() as usize);
    let (map, _) = of_ocean::pattern::NormalMap::from_rgb(w, h, image.as_raw(), flip)?;
    Ok(Arc::new(map))
}

fn save_picture(path: &Path, picture: Picture) -> Result<(), String> {
    image::RgbaImage::from_raw(picture.width, picture.height, picture.rgba)
        .ok_or("the picture has the wrong size")?
        .save(path)
        .map_err(|e| format!("cannot write {}: {e}", path.display()))
}

/// Bake one ocean and write its pictures and build files into `dir`.
/// Returns a description of what was made.
fn produce(dir: &Path, name: &str, land: Land, assignments: &[&str]) -> Result<String, String> {
    let mut values = ParamValues::default();
    let mut tuning = Tuning::default();
    // The land's own running water, unless rivers are given.
    let mut streams: Option<Vec<Stream>> = None;
    let mut ponds: Vec<Pond> = Vec::new();
    let mut zones: Vec<Zone> = Vec::new();
    // A build file brought in as the land, and how it stands in the sea.
    let mut land = land;
    let mut placement = BuildPlacement::default();
    // A project file gives the settings, the left-out blocks and the land.
    let mut from_project: Option<SceneSetup> = None;
    let mut excluded: Vec<String> = Vec::new();
    let mut area: Option<[i32; 4]> = None;
    let mut pattern_image: Option<String> = None;
    // memory=report adds what each step and structure held (stage 1 of the column bake plan).
    let mut watch: Option<crate::mem::Watch> = None;
    for a in assignments {
        // The method's internal numbers can come from a tuning file.
        if let Some(file) = a.strip_prefix("tuning=") {
            let text = std::fs::read_to_string(file).map_err(|e| format!("cannot read {file}: {e}"))?;
            tuning = Tuning::from_text(&text).map_err(|e| format!("{file}: {e}"))?;
        } else if let Some(file) = a.strip_prefix("build=") {
            land = Land::Build;
            placement.file = file.to_string();
        } else if let Some(share) = a.strip_prefix("waterline=") {
            placement.waterline = share.parse::<f64>().map_err(|_| format!("expected waterline=share from 0 to 1, got '{share}'"))?;
        } else if let Some(by) = a.strip_prefix("offset=") {
            let bad = || format!("expected offset=east,south in blocks, got '{by}'");
            let (east, south) = by.split_once(',').ok_or_else(bad)?;
            placement.offset = [east.trim().parse().map_err(|_| bad())?, south.trim().parse().map_err(|_| bad())?];
        } else if let Some(file) = a.strip_prefix("project=") {
            let project = of_core::project::read(Path::new(file)).map_err(|e| format!("{file}: {e}"))?;
            if project.mode != Mode::Ocean {
                return Err(format!("{file} is not an ocean project"));
            }
            values = project.values;
            excluded = project.excluded;
            if let Some(text) = &project.scene {
                let mut scene = SceneSetup::from_text(text).map_err(|e| format!("{file}: {e}"))?;
                // An image carried in the project is unpacked beside the
                // output when the one it names is not on this computer.
                if let Some(carried) = &project.pattern_image {
                    if !Path::new(&scene.pattern_image).is_file() {
                        scene.pattern_image = carried.place(dir)?.to_string_lossy().into_owned();
                    }
                }
                land = scene.land;
                placement = scene.build.clone();
                streams = Some(scene.streams.clone());
                from_project = Some(scene);
            }
        } else if let Some(list) = a.strip_prefix("exclude=") {
            for name in list.split(',').map(str::trim).filter(|n| !n.is_empty()) {
                let name = if name.contains(':') { name.to_string() } else { format!("minecraft:{name}") };
                if !excluded.contains(&name) {
                    excluded.push(name);
                }
            }
        } else if let Some(text) = a.strip_prefix("area=") {
            area = Some(parse_area(text)?);
        } else if let Some(file) = a.strip_prefix("pattern_image=") {
            pattern_image = Some(file.to_string());
        } else if let Some(text) = a.strip_prefix("pond=") {
            ponds.push(Pond::parse(text).map_err(|e| format!("pond=: {e}"))?);
        } else if let Some(text) = a.strip_prefix("calm=") {
            zones.push(Zone::parse(ZoneKind::Calm, text).map_err(|e| format!("calm=: {e}"))?);
        } else if let Some(text) = a.strip_prefix("dry=") {
            zones.push(Zone::parse(ZoneKind::Dry, text).map_err(|e| format!("dry=: {e}"))?);
        } else if *a == "memory=report" {
            watch = Some(crate::mem::Watch::start());
        } else if *a == "streams=none" {
            streams = Some(Vec::new());
        } else if let Some(text) = a.strip_prefix("stream=") {
            streams.get_or_insert_with(Vec::new).push(parse_stream(text)?);
        } else {
            apply(&mut values, a)?;
        }
    }
    let mut settings = OceanSettings::from_params(&values);
    settings.excluded = excluded;
    if let Some(b) = area {
        let (w, l) = (settings.width as i32, settings.length as i32);
        if b[0] > w - 1 || b[2] < 0 || b[1] > l - 1 || b[3] < 0 {
            return Err(format!("area={},{},{},{} lies outside the plot ({w} x {l} blocks)", b[0], b[1], b[2], b[3]));
        }
        area = Some([b[0].max(0), b[1].max(0), b[2].min(w - 1), b[3].min(l - 1)]);
    }
    let cat = catalogue::default();
    let mut scene = match from_project {
        Some(scene) if scene.land == land => scene,
        _ => SceneSetup::of(land, settings.waves.seed),
    };
    if let Some(file) = pattern_image {
        scene.pattern_image = file;
    }
    if settings.pattern.source == of_ocean::pattern::Source::Image && !scene.pattern_image.is_empty() {
        settings.pattern.image = Some(read_pattern(&scene.pattern_image, settings.pattern.flip)?);
    }
    if let Some(streams) = streams {
        scene.streams = streams;
    }
    scene.ponds.extend(ponds);
    scene.zones.extend(zones);
    settings.zones = scene.zones.clone();
    scene.build = placement;
    let mut timings = String::new();
    if let Some(w) = watch.as_mut() {
        w.step("settings read");
    }
    let started = std::time::Instant::now();
    let brought = if land == Land::Build {
        let data = std::fs::read(&scene.build.file).map_err(|e| format!("cannot read {}: {e}", scene.build.file))?;
        // A .litematic is surveyed first, so a build too big to load and
        // bake under the memory limit is refused before it is loaded.
        if scene.build.file.to_ascii_lowercase().ends_with(".litematic") {
            let s = of_formats::survey::litematic(&data, &Progress::none()).map_err(|e| e.to_string())?;
            let after = s.held() + of_ocean::estimate::shape(s.cubes, &settings) + of_ocean::estimate::job_on_build(&settings);
            let whole = of_core::memory::fits(s.peak().max(after)).map_err(|needs| of_core::memory::refusal(Some(needs)))?;
            if let Some(w) = watch.as_mut() {
                w.step("survey the build file");
                w.note(&format!(
                    "survey: {} cubes ({} stored); loading peak {}, build {}; estimate for the whole job {}",
                    s.cubes,
                    s.stored,
                    crate::mem::mb(s.peak()),
                    crate::mem::mb(s.held()),
                    crate::mem::mb(whole)
                ));
            }
        }
        let build = of_formats::import(&data, &Progress::none()).map_err(|e| e.to_string())?.build;
        let _ = writeln!(timings, "  time: read the build file {:.1} s", started.elapsed().as_secs_f32());
        if let Some(w) = watch.as_mut() {
            w.step("read the build file");
            note_build(w, &build);
        }
        Some(Arc::new(build))
    } else {
        None
    };
    let started = std::time::Instant::now();
    let (ground, ends) = match scene.laid_with(settings.width, settings.length, settings.waves.direction, brought.as_ref())? {
        Some((ground, ends)) => (Some(ground), ends),
        None => (None, Vec::new()),
    };
    let ground_time = started.elapsed().as_secs_f32();
    if let Some(w) = watch.as_mut() {
        w.step("lay the land / read the build's shape");
    }
    // Refused before it starts when the estimate is over the memory limit.
    let more = of_ocean::estimate::job(&settings, ground.as_ref());
    let whole = of_core::memory::fits(more).map_err(|needs| of_core::memory::refusal(Some(needs)))?;
    if let Some(w) = watch.as_mut() {
        w.note(&format!(
            "estimate for the bake, pictures and files: {} (held {} + {}); limit {}",
            crate::mem::mb(whole),
            crate::mem::mb(whole - more),
            crate::mem::mb(more),
            crate::mem::mb(of_core::memory::limit())
        ));
    }
    let started = std::time::Instant::now();
    // With memory=report the bake's progress goes to stderr, each whole percent.
    let shown = std::sync::atomic::AtomicU32::new(u32::MAX);
    let report_bake = |f: f32| {
        let pc = (f * 100.0) as u32;
        if shown.swap(pc, std::sync::atomic::Ordering::Relaxed) != pc {
            eprintln!("  bake {pc}% at {:.1} s", started.elapsed().as_secs_f32());
        }
        true
    };
    let bake_progress = if watch.is_some() { Progress::new(&report_bake) } else { Progress::none() };
    let baked = bake_with(name, &settings, ground.as_ref(), cat, &tuning, &bake_progress)?;
    let bake_time = started.elapsed().as_secs_f32();
    if let Some(w) = watch.as_mut() {
        w.step("bake");
    }
    if let Some(w) = watch.as_mut() {
        note_baked(w, &baked);
    }
    if land == Land::Build {
        let _ = writeln!(timings, "  time: read the ground from the build {ground_time:.1} s");
    }
    std::fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    let started = std::time::Instant::now();

    let (w, l) = (settings.width as f64, settings.length as f64);
    let view = Camera::angled((-0.3, 1.0), 32.0, (1700.0 / (w + 0.6 * l)).clamp(2.0, 8.0));
    let map = Camera::map((1200.0 / w.max(l)).floor().clamp(2.0, 8.0));
    // A build too large to draw still gets its files; the report says why the pictures are missing.
    let mut not_drawn = None;
    // The scene is read from the bake's columns once, for both pictures.
    match render::Scene::new(&baked.source(None), &Layer::ALL, cat) {
        Ok(scene) => {
            if let Some(w) = watch.as_mut() {
                w.step("3D view scene from the columns");
                w.note(&format!("3D view scene: {}", crate::mem::mb(scene.bytes())));
            }
            for (file, camera) in [("view", &view), ("map", &map)] {
                match render::picture(&scene, camera) {
                    Ok(picture) => save_picture(&dir.join(format!("{name}-{file}.png")), picture)?,
                    Err(e) => {
                        not_drawn = Some(e);
                        break;
                    }
                }
            }
        }
        Err(e) => not_drawn = Some(e),
    }
    // The colours aimed for beside the colours the blocks gave.
    if let Some(colours) = baked.colour_picture((640 / settings.width.max(settings.length).max(1)).clamp(1, 6) as u32) {
        save_picture(&dir.join(format!("{name}-target.png")), colours)?;
    }

    let _ = writeln!(timings, "  time: pictures {:.1} s", started.elapsed().as_secs_f32());
    if let Some(w) = watch.as_mut() {
        w.step("pictures (3D view grid)");
    }
    let mut report = String::new();
    let stats = &baked.stats;
    let _ = writeln!(report, "{name}: {} x {} blocks, baked in {bake_time:.1} s", settings.width, settings.length);
    if let Some(e) = not_drawn {
        let _ = writeln!(report, "  {name}-view.png and {name}-map.png: not drawn, {e}");
    }
    if let Some(placed) = ground.as_ref().and_then(|g| g.placed.as_ref()) {
        let _ = writeln!(
            report,
            "  build brought in: {} x {} x {} blocks (width, height, length); the water rests {} blocks up it; {} blocks fell outside the plot",
            placed.size[0],
            placed.size[1],
            placed.size[2],
            placed.under,
            placed.cut
        );
    }
    // Ponds need land to stand on.
    if !scene.ponds.is_empty() && ground.is_none() {
        let _ = writeln!(report, "  ponds left out: open sea has no land to hold them");
    }
    // How the shore was read, and what became of each river.
    if let Some(ground) = &ground {
        let survey = Survey::new(ground);
        save_picture(&dir.join(format!("{name}-shore.png")), survey.picture(ground, (1200 / settings.width.max(settings.length).max(1)).clamp(2, 8) as u32))?;
        let (kinds, channel) = survey.tally();
        let all = kinds.iter().sum::<usize>().max(1) as f64;
        let share = |kind: ShoreKind| 100.0 * kinds[kind as usize] as f64 / all;
        let _ = writeln!(
            report,
            "  shore read from the ground: {:.0}% beach, {:.0}% rocky, {:.0}% cliff; {channel} columns of river channel",
            share(ShoreKind::Beach),
            share(ShoreKind::Rocky),
            share(ShoreKind::Cliff)
        );
        for (n, end) in ends.iter().enumerate() {
            let what = match end {
                Outcome::Dry => "has no land to run over".to_string(),
                Outcome::Inland => "ends on the land, short of the sea".to_string(),
                Outcome::Mouth(kind) => format!("runs into the sea through a plain mouth on a {} shore", kind.key()),
                Outcome::Delta(size) => format!("runs into the sea over a delta (size {size:.2})"),
                Outcome::Fall => "falls into the sea".to_string(),
            };
            let _ = writeln!(report, "  river {}: {what}", n + 1);
        }
        for (n, end) in ground.ponds().iter().enumerate() {
            let what = match end {
                PondEnd::Dry => "lies in the sea, not on the land".to_string(),
                PondEnd::Spills { depth: 0, .. } => "has no hollow to fill: the water runs away".to_string(),
                PondEnd::Held { depth, size } => format!("holds its water {} deep over {}", blocks(*depth), columns(*size)),
                PondEnd::Spills { depth, size, to } => format!("spills {} at {} deep over {}", match to {
                    Spill::Sea => "into the sea",
                    Spill::Edge => "off the edge of the plot",
                    Spill::River => "into a river",
                }, blocks(*depth), columns(*size)),
            };
            let _ = writeln!(report, "  pond {}: {what}", n + 1);
        }
    }
    if !scene.zones.is_empty() {
        let calm = scene.zones.iter().filter(|z| z.kind == ZoneKind::Calm).count();
        let dry = scene.zones.len() - calm;
        let _ = writeln!(report, "  zones: {calm} calm water, {dry} keep-dry");
    }
    if !assignments.is_empty() {
        let _ = writeln!(report, "  settings changed from the defaults: {}", assignments.join("  "));
    }
    let _ = writeln!(
        report,
        "  water rests at Y {}; surface from {} below to {} above it; paste the files with their lowest layer at Y {}",
        settings.placement_y, -stats.range.0, stats.range.1, baked.export_y
    );
    let _ = writeln!(
        report,
        "  blocks: terrain {}, foundation {}, coloured surface {}, glass {}",
        stats.blocks[0],
        stats.blocks[1],
        stats.blocks[2],
        stats.blocks[3] + stats.blocks[4] + stats.blocks[5]
    );
    let _ = writeln!(report, "  white water: {:.1}% of the sea", 100.0 * stats.white_columns as f64 / stats.water_columns.max(1) as f64);
    let used: Vec<String> = stats.used.iter().map(|(n, c)| format!("{} {c}", n.trim_start_matches("minecraft:"))).collect();
    let _ = writeln!(report, "  block types ({}): {}", used.len(), used.join(", "));

    if let Some(w) = watch.as_mut() {
        w.step("report and shore picture");
    }
    let source = baked.source(area);
    if let Some(b) = area {
        let _ = writeln!(report, "  files hold only blocks {}..{} east and {}..{} south", b[0], b[2], b[1], b[3]);
    }
    let mut selections = vec![(Selection::Complete, "")];
    if land != Land::Open {
        selections.push((Selection::WaterOnly, "-water-only"));
    }
    for (selection, suffix) in selections {
        for format in [Format::Litematic, Format::Sponge, Format::Axiom] {
            let options =
                ExportOptions { format, selection, air: AirMode::Compact, include_colour_glass: settings.colour_glass };
            let started = std::time::Instant::now();
            let out = match export::export(&source, &options, cat, &Progress::none()) {
                Ok(out) => out,
                Err(e) => return Err(e.to_string()),
            };
            if let Some(w) = watch.as_mut() {
                w.step(&format!("export {name}{suffix}.{}", format.extension()));
                w.note(&format!("{name}{suffix}.{}: file {}", format.extension(), crate::mem::mb(out.bytes.len() as u64)));
            }
            // Read it back, so a file that would not open is never handed over.
            let back = of_formats::import(&out.bytes, &Progress::none()).map_err(|e| e.to_string())?.build;
            if back.counts().0 != out.blocks {
                return Err(format!("{name}{suffix}.{} did not read back as written", format.extension()));
            }
            drop(back);
            if let Some(w) = watch.as_mut() {
                w.step(&format!("read back {suffix}.{} (CLI check only)", format.extension()));
            }
            let path = dir.join(format!("{name}{suffix}.{}", format.extension()));
            std::fs::write(&path, &out.bytes).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
            let _ = writeln!(
                report,
                "  {name}{suffix}.{}: {} blocks, {:.0} KB, written in {:.1} s",
                format.extension(),
                out.blocks,
                out.bytes.len() as f64 / 1e3,
                started.elapsed().as_secs_f32()
            );
        }
    }
    report.push_str(&timings);
    if let Some(w) = watch {
        report.push_str(&w.finish());
    }
    Ok(report)
}

/// Bytes a sparse block store holds: each stored 16-block cube of 4-byte
/// cells (cubes of one value share a copy), plus the map entry and pointer
/// for every cube (about 32 bytes).
fn volume_bytes(v: &of_formats::model::Volume) -> (usize, u64) {
    let (cubes, stored) = v.cube_counts();
    (cubes, stored as u64 * (16 * 16 * 16 * 4) + cubes as u64 * 32)
}

/// The loaded build's own store, and what reading its shape lists from it.
fn note_build(w: &mut crate::mem::Watch, build: &of_formats::model::Build) {
    let air: Vec<bool> = build.palette.states().iter().map(of_formats::model::BlockState::is_air).collect();
    let (mut chunks, mut bytes, mut filled) = (0, 0, 0u64);
    for region in &build.regions {
        let (c, b) = volume_bytes(&region.cells);
        chunks += c;
        bytes += b;
        for (_, cube) in region.cells.chunks() {
            filled += cube.iter().filter(|v| **v != 0 && !air[(**v - 1) as usize]).count() as u64;
        }
    }
    let stored: usize = build.regions.iter().map(|r| r.cells.cube_counts().1).sum();
    w.note(&format!("loaded build: {} regions, {chunks} cubes ({stored} stored), {}; {filled} filled cells", build.regions.len(), crate::mem::mb(bytes)));
    // terrain.rs from_build walks the cubes column by column; it keeps one
    // entry (about 24 B) per cube that holds a block, not a list of blocks.
    w.note(&format!("shape reading: cube index of about {}", crate::mem::mb(chunks as u64 * 24)));
}

/// The bake's per-column maps and the blocks it keeps as columns.
fn note_baked(w: &mut crate::mem::Watch, baked: &of_ocean::build::Baked) {
    let columns = baked.water_top.len() as u64;
    let per_column = baked.water_top.len() * 4
        + baked.water_bottom.len() * 4
        + baked.target.len() * 3
        + baked.matched.len() * 3
        + baked.maps.depth.len() * 4
        + baked.maps.foam.len()
        + baked.maps.sheen.len();
    let water = baked.stats.water_columns;
    w.note(&format!("per-column maps kept with the bake: {} for {columns} columns ({water} with water)", crate::mem::mb(per_column as u64)));
    w.note(&format!("water as columns (plan, 12 B each): about {}", crate::mem::mb(water * 12)));
    if let of_ocean::Blocks::Columns(c) = &baked.blocks {
        let land = match &c.land {
            of_ocean::LandBlocks::Ground { starts, runs } => starts.len() * 4 + runs.len() * 12,
            _ => 0,
        };
        let bytes = c.ground.len() * 4 + c.stack.len() * 4 + land + baked.water_top.len() * 8;
        w.note(&format!("the bake's blocks as columns (top, bottom, ground, stack, land runs): {}", crate::mem::mb(bytes as u64)));
    }
    if let Some((lo, hi)) = of_formats::export::BlockSource::bounds(&baked.source(None), &Layer::ALL) {
        let cells: u64 = (0..3).map(|a| (hi[a] - lo[a] + 1) as u64).product();
        w.note(&format!("box around every block: {cells} cells"));
    }
}

/// `of-cli ocean <folder> <name> [coast|cliffs] [build=FILE] [waterline=SHARE] [offset=EAST,SOUTH] [tuning=FILE]
/// [project=FILE] [exclude=BLOCK,...] [area=W,N,E,S] [pattern_image=FILE] [id=value ...]`
pub fn run(args: &[String]) -> Result<(), String> {
    let (folder, name) = (&args[0], &args[1]);
    let scene = match args.get(2).map(String::as_str) {
        #[cfg(feature = "dev")]
        Some("coast") => Land::Coast,
        #[cfg(feature = "dev")]
        Some("cliffs") => Land::Cliffs,
        _ => Land::Open,
    };
    let assignments: Vec<&str> = args[if scene == Land::Open { 2 } else { 3 }..].iter().map(String::as_str).collect();
    print!("{}", produce(Path::new(folder), name, scene, &assignments)?);
    Ok(())
}

/// `of-cli project <file.ofproject> <folder> [id=value ...]`: bake a saved
/// project with its own land, settings and left-out blocks.
pub fn run_project(file: &str, folder: &str, extra: &[String]) -> Result<(), String> {
    let name = Path::new(file).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "project".into());
    let first = format!("project={file}");
    let assignments: Vec<&str> = std::iter::once(first.as_str()).chain(extra.iter().map(String::as_str)).collect();
    print!("{}", produce(Path::new(folder), &name, Land::Open, &assignments)?);
    Ok(())
}

/// `of-cli lava <folder> <name> [tuning=FILE] [id=value ...]`: bake a field
/// of lava and write its pictures and build files (development build only).
#[cfg(feature = "dev")]
pub fn run_lava(args: &[String]) -> Result<(), String> {
    let (dir, name) = (Path::new(&args[0]), &args[1]);
    let mut values = ParamValues::default();
    let mut tuning = Tuning::default();
    for a in &args[2..] {
        if let Some(file) = a.strip_prefix("tuning=") {
            let text = std::fs::read_to_string(file).map_err(|e| format!("cannot read {file}: {e}"))?;
            tuning = Tuning::from_text(&text).map_err(|e| format!("{file}: {e}"))?;
        } else {
            let (id, text) = a.split_once('=').ok_or_else(|| format!("expected id=value, got '{a}'"))?;
            let def = params::find(id).filter(|d| d.mode == Mode::Lava).ok_or_else(|| format!("unknown lava setting '{id}'"))?;
            values.set(def, crate::parse_value(def, text)?);
        }
    }
    let settings = of_ocean::lava::LavaSettings::from_params(&values);
    let cat = catalogue::default();
    let started = std::time::Instant::now();
    let baked = of_ocean::lava::bake_lava(name, &settings, cat, &tuning, &Progress::none())?;
    let bake_time = started.elapsed().as_secs_f32();
    std::fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    let (w, l) = (settings.width as f64, settings.length as f64);
    let view = Camera::angled((-0.3, 1.0), 32.0, (1700.0 / (w + 0.6 * l)).clamp(2.0, 8.0));
    save_picture(&dir.join(format!("{name}-view.png")), render::render(&baked.source(None), &Layer::ALL, cat, &view)?)?;
    let map = Camera::map((1200.0 / w.max(l)).floor().clamp(2.0, 8.0));
    save_picture(&dir.join(format!("{name}-map.png")), render::render(&baked.source(None), &Layer::ALL, cat, &map)?)?;
    if let Some(colours) = baked.colour_picture((640 / settings.width.max(settings.length).max(1)).clamp(1, 6) as u32) {
        save_picture(&dir.join(format!("{name}-target.png")), colours)?;
    }
    let stats = &baked.stats;
    println!("{name}: {} x {} blocks of lava, baked in {bake_time:.1} s", settings.width, settings.length);
    println!("  paste the files with their lowest layer at Y {}", baked.export_y);
    println!("  blocks: foundation {}, coloured surface {}, glass {}", stats.blocks[1], stats.blocks[2], stats.blocks[3] + stats.blocks[4] + stats.blocks[5]);
    let used: Vec<String> = stats.used.iter().map(|(n, c)| format!("{} {c}", n.trim_start_matches("minecraft:"))).collect();
    println!("  block types ({}): {}", used.len(), used.join(", "));
    for format in [Format::Litematic, Format::Sponge, Format::Axiom] {
        let options = ExportOptions { format, selection: Selection::Complete, air: AirMode::Compact, include_colour_glass: true };
        let out = export::export(&baked.source(None), &options, cat, &Progress::none()).map_err(|e| e.to_string())?;
        let back = of_formats::import(&out.bytes, &Progress::none()).map_err(|e| e.to_string())?.build;
        if back.counts().0 != out.blocks {
            return Err(format!("{name}.{} did not read back as written", format.extension()));
        }
        let path = dir.join(format!("{name}.{}", format.extension()));
        std::fs::write(&path, &out.bytes).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
        println!("  {name}.{}: {} blocks, {:.0} KB", format.extension(), out.blocks, out.bytes.len() as f64 / 1e3);
    }
    Ok(())
}

/// `of-cli estimate <build> [settings]`: the memory that loading a
/// `.litematic` build and baking an ocean around it would take, read from
/// the file without loading it (column bake plan, stage 7: the statue's
/// estimate is shown before it runs).
pub fn estimate(file: &str, args: &[String]) -> Result<(), String> {
    use of_core::memory::{committed, limit, mb};
    let mut values = ParamValues::default();
    for a in args {
        apply(&mut values, a)?;
    }
    let settings = OceanSettings::from_params(&values);
    let started = std::time::Instant::now();
    let data = std::fs::read(file).map_err(|e| format!("cannot read {file}: {e}"))?;
    let held_now = committed();
    let s = of_formats::survey::litematic(&data, &Progress::none()).map_err(|e| e.to_string())?;
    let job = of_ocean::estimate::job_on_build(&settings);
    let shape = of_ocean::estimate::shape(s.cubes, &settings);
    let after = s.held() + shape + job;
    let whole = held_now + s.peak().max(after);
    println!("{file}: {} regions, {} cells in their boxes, {} blocks", s.regions, s.cells, s.blocks);
    println!("  cubes: {} ({} stored, the rest shared)", s.cubes, s.stored);
    println!("  loading: peak about {}, then the build holds {}", mb(s.peak()), mb(s.held()));
    println!("  laying it down as the land: {}", mb(shape));
    println!("  bake, pictures and files on {} x {}: {}", settings.width, settings.length, mb(job));
    println!("  whole run: about {} (held now {} + the larger of loading and baking; the CLI's read-back checks come on top)", mb(whole), mb(held_now));
    println!("  memory limit: {}; {}", mb(limit()), if whole <= limit() { "it fits" } else { "it would be refused" });
    println!("  read in {:.1} s", started.elapsed().as_secs_f32());
    Ok(())
}

/// `of-cli testbuild <file> [height]`: write a small hollow figure to try
/// "a build as land" with: a body on four legs with a head that hangs out
/// over the water, as a shell one block thick. Up to 1,536 tall, so it can
/// stand in for a statue-sized build when measuring memory (the statue is
/// 1,417 tall); about 0.6 x 1.15 times its height across.
pub fn test_build(path: &str, height: i32) -> Result<(), String> {
    use of_formats::model::{BlockState, Build, Region};
    let s = height.clamp(16, 1536) as f64;
    let inside = |x: f64, y: f64, z: f64| -> bool {
        let ball = |cx: f64, cy: f64, cz: f64, rx: f64, ry: f64, rz: f64| ((x - cx) / rx).powi(2) + ((y - cy) / ry).powi(2) + ((z - cz) / rz).powi(2) <= 1.0;
        let leg = |cx: f64, cz: f64| y >= 0.0 && y <= 0.45 * s && (x - cx).hypot(z - cz) <= 0.07 * s;
        ball(0.0, 0.52 * s, 0.0, 0.26 * s, 0.2 * s, 0.42 * s)
            || ball(0.0, 0.8 * s, 0.5 * s, 0.17 * s, 0.17 * s, 0.17 * s)
            || leg(-0.15 * s, -0.28 * s)
            || leg(0.15 * s, -0.28 * s)
            || leg(-0.15 * s, 0.28 * s)
            || leg(0.15 * s, 0.28 * s)
    };
    let (half_x, from_z, to_z, top) = ((0.3 * s) as i32, -(0.45 * s) as i32, (0.7 * s) as i32, s as i32);
    let mut build = Build::new();
    let id = build.palette.intern(BlockState::new("minecraft:white_concrete"));
    let mut region = Region::new("figure", [-half_x, 0, from_z], [2 * half_x + 1, top + 1, to_z - from_z + 1]);
    let mut cells = of_formats::model::Volume::new();
    for y in 0..=top {
        for z in from_z..=to_z {
            for x in -half_x..=half_x {
                let at = |dx: i32, dy: i32, dz: i32| inside((x + dx) as f64, (y + dy) as f64, (z + dz) as f64);
                // Only the skin: blocks with open air on some side.
                if at(0, 0, 0) && !(at(1, 0, 0) && at(-1, 0, 0) && at(0, 1, 0) && at(0, -1, 0) && at(0, 0, 1) && at(0, 0, -1)) {
                    cells.set([x, y, z], id);
                }
            }
        }
    }
    let blocks = cells.count();
    region.cells = Arc::new(cells);
    build.regions.push(region);
    build.meta.name = "figure".into();
    let (bytes, _) = of_formats::write(&build, Format::Litematic, catalogue::default().data_version, &Progress::none()).map_err(|e| e.to_string())?;
    std::fs::write(path, bytes).map_err(|e| format!("cannot write {path}: {e}"))?;
    println!("Wrote {path}: a hollow figure {top} blocks tall, {blocks} blocks");
    Ok(())
}

/// Write a set of test runs that show the generator at work: every scene
/// in a calm, a moderate and a stormy sea.
pub fn test_runs(folder: &str) -> Result<(), String> {
    #[cfg_attr(not(feature = "dev"), allow(unused_mut))]
    let mut scenes: Vec<(&str, Land, &str, &[&str])> = vec![("open", Land::Open, "Open ocean, nothing but water.", &["ocean.width=160", "ocean.length=160"])];
    // The demonstration lands are in the development build only.
    #[cfg(feature = "dev")]
    scenes.extend::<[(&str, Land, &str, &[&str]); 2]>([
        (
            "coast",
            Land::Coast,
            "A coast with a beach that turns into a cliff sea front, an island with a beach and rocky shores, and open sea. \
             The waves come in from the east.",
            &["ocean.width=320", "ocean.length=224", "ocean.direction=200", "ocean.seed=7"],
        ),
        (
            "cliffs",
            Land::Cliffs,
            "A cliff coast: a headland ending in a rock arch with a needle of rock beyond it, large rocks standing alone in the sea, \
             a bay with a shingle beach, and a stream that falls from the cliff top into a cove. The waves come in from the south-west.",
            &["ocean.width=320", "ocean.length=224", "ocean.direction=300", "ocean.seed=5"],
        ),
    ]);
    let seas: [(&str, &str, &[&str]); 3] = [
        (
            "calm",
            "A calm day: a low, long, regular swell.",
            &["ocean.wave_height=2.5", "ocean.peak_distance=56", "ocean.irregularity=0.2", "ocean.swell_detail=0.2", "ocean.crest_sharpness=0.15"],
        ),
        ("moderate", "A moderate sea.", &["ocean.wave_height=7", "ocean.peak_distance=44", "ocean.irregularity=0.6", "ocean.swell_detail=0.5"]),
        (
            "storm",
            "A storm: a tall, confused sea with sharp crests. The waves alone do not darken the water: this run also sets the \
             colour settings to grey-green water colours, less daylight, hardly any sun and grey reflections.",
            &[
                "ocean.wave_height=13",
                "ocean.peak_distance=120",
                "ocean.irregularity=0.85",
                "ocean.swell_detail=0.6",
                "ocean.crest_sharpness=0.8",
                "ocean.dark_colour=#467177",
                "ocean.light_colour=#b0cfd3",
                "ocean.daylight=0.78",
                "ocean.lit_strength=0.15",
                "ocean.reflection_auto=false",
                "ocean.reflection_colour=#b4bcc0",
                "ocean.foundation_colour=gray",
                "ocean.foam_amount=0.9",
            ],
        ),
    ];
    let cat = catalogue::default();
    #[cfg(not(feature = "dev"))]
    let mut readme = format!(
        "Ocean Foundry {} - ocean test runs for Minecraft Java {}\n\n\
         Open sea in a calm, a moderate and a stormy sea. Each folder holds one run: a\n\
         picture seen from the south (-view.png), a map with north up (-map.png), the colours the\n\
         water was meant to have beside the colours its blocks gave (-target.png), and the build as\n\
         a Litematica file (.litematic), a WorldEdit schematic (.schem) and an Axiom blueprint (.bp).\n\n\
         The pictures use the flat colours measured from the game's blocks, with the game's own\n\
         face shading. The sea is built from a measured wave spectrum. This is a development\n\
         version of the method and is expected to change.\n\n",
        of_core::VERSION,
        cat.game_version
    );
    #[cfg(feature = "dev")]
    let mut readme = format!(
        "Ocean Foundry {} - ocean test runs for Minecraft Java {}\n\n\
         Three scenes, each in a calm, a moderate and a stormy sea. Each folder holds one run: a\n\
         picture seen from the south (-view.png), a map with north up (-map.png), the colours the\n\
         water was meant to have beside the colours its blocks gave (-target.png), and the build as\n\
         a Litematica file (.litematic), a WorldEdit schematic (.schem) and an Axiom blueprint (.bp).\n\
         Scenes with land also come as water only, without the land.\n\n\
         Yellow concrete is sand, lime concrete is grass, light gray concrete is rock. The stream\n\
         and the waterfall of the cliff scene are part of its test terrain (light blue and white\n\
         concrete); what the sea does where the fall strikes it is the generator's work.\n\n\
         The pictures use the flat colours measured from the game's blocks, with the game's own\n\
         face shading. The sea is built from a measured wave spectrum; near land the waves slow,\n\
         turn toward the shore, find shelter behind land and break in shallow water. This is a\n\
         development version of the method and is expected to change.\n\n",
        of_core::VERSION,
        cat.game_version
    );
    for (scene_name, scene, place, placed) in scenes {
        for (sea_name, weather, sea) in seas {
            let name = format!("{scene_name}-{sea_name}");
            let assignments: Vec<&str> = placed.iter().chain(sea.iter()).copied().collect();
            let report = produce(&Path::new(folder).join(&name), &name, scene, &assignments)?;
            print!("{report}");
            let _ = write!(readme, "{place} {weather}\n{report}\n");
        }
    }
    let path = Path::new(folder).join("README.txt");
    std::fs::write(&path, readme.replace('\n', "\r\n")).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    println!("Wrote {}", path.display());
    Ok(())
}
