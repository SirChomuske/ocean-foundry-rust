//! Commands that work on build files.

use of_formats::catalogue;
use of_formats::export::{self, AirMode, ExportOptions, Selection};
use of_formats::model::{first_difference, Build, Format, Pos};
use of_formats::{nbt, Progress};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

fn be32(data: &[u8], at: usize) -> Result<usize, String> {
    data.get(at..at + 4).map(|b| u32::from_be_bytes(b.try_into().unwrap()) as usize).ok_or_else(|| "file ends early".to_string())
}

/// Print the tag tree of any NBT-based file.
pub fn nbt_dump(path: &str, depth: usize) -> Result<(), String> {
    let data = std::fs::read(path).map_err(|e| format!("cannot read {path}: {e}"))?;
    if data.starts_with(&[0x0A, 0xE5, 0xBB, 0x36]) {
        let header_len = be32(&data, 4)?;
        let header = data.get(8..8 + header_len).ok_or("file ends early")?;
        let (name, root, _) = nbt::read_raw(header).map_err(|e| e.to_string())?;
        println!("Axiom blueprint. Header (root name {name:?}):\n{}", nbt::outline(&root, depth));
        let at = 8 + header_len;
        let thumb_len = be32(&data, at)?;
        println!("Thumbnail: {thumb_len} bytes");
        let at = at + 4 + thumb_len;
        let body_len = be32(&data, at)?;
        let body = data.get(at + 4..at + 4 + body_len).ok_or("file ends early")?;
        println!("Block data: {body_len} bytes, {} bytes after it", data.len() - (at + 4 + body_len));
        let (name, root) = nbt::read(body).map_err(|e| e.to_string())?;
        println!("Block data (root name {name:?}):\n{}", nbt::outline(&root, depth));
        return Ok(());
    }
    let (name, root) = nbt::read(&data).map_err(|e| e.to_string())?;
    println!("Root name {name:?}:\n{}", nbt::outline(&root, depth));
    Ok(())
}

fn load(path: &Path) -> Result<of_formats::Imported, String> {
    let data = std::fs::read(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    of_formats::import(&data, &Progress::none()).map_err(|e| e.to_string())
}

fn format_name(f: Option<Format>) -> &'static str {
    match f {
        Some(Format::Litematic) => "Litematica",
        Some(Format::Sponge) => "Sponge schematic",
        Some(Format::Legacy) => "legacy schematic",
        Some(Format::Axiom) => "Axiom blueprint",
        None => "unknown",
    }
}

/// Describe a build file: kind, size, regions, blocks and anything unusual.
pub fn inspect(path: &str) -> Result<(), String> {
    let imported = load(Path::new(path))?;
    let b = &imported.build;
    println!(
        "{}{}{}",
        format_name(b.source),
        b.source_version.map(|v| format!(", file version {v}")).unwrap_or_default(),
        b.data_version.map(|v| format!(", Minecraft data version {v}")).unwrap_or_default()
    );
    if !b.meta.name.is_empty() || !b.meta.author.is_empty() {
        println!("Name: {:?}  Author: {:?}", b.meta.name, b.meta.author);
    }
    if let Some((lo, hi)) = b.bounds() {
        println!("Size: {} x {} x {}  (from {lo:?} to {hi:?})", hi[0] - lo[0] + 1, hi[1] - lo[1] + 1, hi[2] - lo[2] + 1);
    }
    println!("Regions: {}", b.regions.len());
    for r in b.regions.iter().take(12) {
        println!("  {:?} at {:?}, {} x {} x {}{}", r.name, r.min, r.dims[0], r.dims[1], r.dims[2], if r.flipped.contains(&true) { " (stored with negative size)" } else { "" });
    }
    if b.regions.len() > 12 {
        println!("  ... and {} more", b.regions.len() - 12);
    }
    let (blocks, air) = b.counts();
    println!("Blocks: {blocks}  Air: {air}  Block entities: {}  Entities: {}", b.block_entity_count(), b.entity_count());
    let histogram = b.histogram();
    println!("Block kinds: {}", histogram.len());
    for (state, n) in histogram.iter().take(15) {
        println!("  {n:>9}  {state}");
    }
    let unknown = catalogue::default().unknown_in(b);
    if !unknown.is_empty() {
        println!("Not in Minecraft {}: {}", catalogue::default().game_version, unknown.join(", "));
    }
    for note in &imported.notes {
        println!("Note: {note}");
    }
    Ok(())
}

fn format_for(path: &Path) -> Result<Format, String> {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    Format::EXPORTABLE
        .into_iter()
        .find(|f| f.extension() == ext)
        .ok_or_else(|| format!("cannot tell the format from '{}': use .litematic, .schem or .bp", path.display()))
}

/// Re-save a build in the format the output name asks for.
pub fn convert(input: &str, output: &str) -> Result<(), String> {
    let imported = load(Path::new(input))?;
    let format = format_for(Path::new(output))?;
    let out = export::convert(&imported.build, format, catalogue::default(), &Progress::none()).map_err(|e| e.to_string())?;
    std::fs::write(output, &out.bytes).map_err(|e| format!("cannot write {output}: {e}"))?;
    println!("Wrote {output}: {} blocks, {} region(s), {} bytes", out.blocks, out.regions, out.bytes.len());
    for note in imported.notes.iter().chain(&out.notes) {
        println!("Note: {note}");
    }
    Ok(())
}

fn collect(path: &Path, out: &mut Vec<PathBuf>) {
    if path.is_dir() {
        let mut entries: Vec<PathBuf> = std::fs::read_dir(path).into_iter().flatten().flatten().map(|e| e.path()).collect();
        entries.sort();
        entries.iter().for_each(|p| collect(p, out));
    } else if matches!(path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).as_deref(), Some("litematic" | "schem" | "schematic" | "bp")) {
        out.push(path.to_path_buf());
    }
}

/// Every block that is not air, by position: what a player would see.
fn solid(build: &Build) -> HashMap<Pos, String> {
    let mut cells = HashMap::new();
    for r in &build.regions {
        r.for_each(|p, id| {
            let state = build.palette.get(id);
            if !state.is_air() {
                cells.insert(p, state.to_string());
            }
        });
    }
    cells
}

fn check_file(path: &Path) -> Result<String, String> {
    let original = load(path)?.build;
    let cat = catalogue::default();
    let source = original.source.ok_or("unknown kind")?;
    let mut done = Vec::new();
    // Same format: everything must survive, including how regions are laid out.
    if source != Format::Legacy {
        let (bytes, _) = of_formats::write(&original, source, cat.data_version, &Progress::none()).map_err(|e| format!("re-export: {e}"))?;
        let back = of_formats::import(&bytes, &Progress::none()).map_err(|e| format!("re-import: {e}"))?.build;
        if let Some(d) = first_difference(&original, &back, true) {
            return Err(format!("same-format round trip: {d}"));
        }
        if back.data_version != original.data_version || back.entity_count() != original.entity_count() {
            return Err("same-format round trip: data version or entities changed".into());
        }
        // A second pass must give the same bytes: nothing drifts.
        let (again, _) = of_formats::write(&back, source, cat.data_version, &Progress::none()).map_err(|e| format!("second export: {e}"))?;
        let stable = of_formats::import(&again, &Progress::none()).map_err(|e| format!("second import: {e}"))?.build;
        if let Some(d) = first_difference(&back, &stable, true) {
            return Err(format!("second round trip: {d}"));
        }
        done.push("same format");
    }
    // Other formats: every visible block and block entity must land in the same place.
    let want = solid(&original);
    for format in Format::EXPORTABLE.into_iter().filter(|f| Some(*f) != original.source) {
        let (bytes, _) = match of_formats::write(&original, format, cat.data_version, &Progress::none()) {
            Ok(v) => v,
            Err(e) if e.key == "err.too_large" => continue,
            Err(e) => return Err(format!("to {}: {e}", format.key())),
        };
        let back = of_formats::import(&bytes, &Progress::none()).map_err(|e| format!("from {}: {e}", format.key()))?.build;
        let got = solid(&back);
        if got.len() != want.len() {
            return Err(format!("to {}: {} blocks became {}", format.key(), want.len(), got.len()));
        }
        if let Some((p, s)) = want.iter().find(|(p, s)| got.get(*p) != Some(s)) {
            return Err(format!("to {}: {s} at {p:?} became {:?}", format.key(), got.get(p)));
        }
        if back.block_entity_count() != original.block_entity_count() {
            return Err(format!("to {}: {} block entities became {}", format.key(), original.block_entity_count(), back.block_entity_count()));
        }
        done.push(format.key());
    }
    Ok(format!(
        "{}, {} regions, {} blocks, {} block entities; checked {}",
        format_name(original.source),
        original.regions.len(),
        want.len(),
        original.block_entity_count(),
        done.join(", ")
    ))
}

/// Import every build file under the given paths, export it, import the
/// result and compare. Nothing is written to disk.
pub fn roundtrip(paths: &[String]) -> Result<(), String> {
    let mut files = Vec::new();
    paths.iter().for_each(|p| collect(Path::new(p), &mut files));
    if files.is_empty() {
        return Err("no .litematic, .schem, .schematic or .bp files found".into());
    }
    let mut failed = 0;
    for file in &files {
        let name = file.file_name().and_then(|n| n.to_str()).unwrap_or("?");
        match check_file(file) {
            Ok(summary) => println!("ok    {name}: {summary}"),
            Err(e) => {
                failed += 1;
                println!("FAIL  {name}: {e}");
            }
        }
    }
    println!("{} of {} files round-tripped without any difference", files.len() - failed, files.len());
    if failed == 0 { Ok(()) } else { Err(format!("{failed} file(s) differed")) }
}

/// Export a large wavy water surface every way and report time, size and
/// region count: the worst case for "Omit air".
pub fn stress(size: i32) -> Result<(), String> {
    use of_formats::export::{Layer, LayeredBuild};
    use of_formats::model::BlockState;
    let started = std::time::Instant::now();
    let mut b = LayeredBuild::new("stress");
    let (foundation, glass) = (BlockState::new("blue_concrete"), BlockState::new("light_blue_stained_glass"));
    for z in 0..size {
        for x in 0..size {
            let (fx, fz) = (x as f32, z as f32);
            let h = 6.0 + 2.5 * (fx * 0.11 + fz * 0.03).sin() + 1.5 * (fz * 0.19 - fx * 0.05).sin() + 0.8 * (fx * 0.47 + fz * 0.41).sin();
            let h = h.round() as i32;
            // A shell three blocks thick, as a baked surface would be, with glass on top.
            for y in (h - 3).max(0)..h {
                b.set(Layer::Foundation, [x, y, z], &foundation);
            }
            b.set(Layer::Glass1, [x, h, z], &glass);
        }
    }
    println!("{size} x {size} surface built in {:.1} s", started.elapsed().as_secs_f32());
    let cat = catalogue::default();
    for (format, air, label) in [
        (Format::Litematic, AirMode::OmitAir, "Litematica, omit air"),
        (Format::Litematic, AirMode::Compact, "Litematica, compact"),
        (Format::Sponge, AirMode::Compact, "Sponge schematic"),
        (Format::Axiom, AirMode::Compact, "Axiom blueprint"),
    ] {
        let started = std::time::Instant::now();
        let options = ExportOptions { format, selection: Selection::WaterOnly, air, include_colour_glass: true };
        let out = export::export(&b, &options, cat, &Progress::none()).map_err(|e| e.to_string())?;
        let written = started.elapsed().as_secs_f32();
        let started = std::time::Instant::now();
        let back = of_formats::import(&out.bytes, &Progress::none()).map_err(|e| e.to_string())?.build;
        println!(
            "{label:<22} {:>9} blocks {:>7} region(s) {:>7.1} MB  write {written:.1} s, read back {:.1} s{}",
            out.blocks,
            out.regions,
            out.bytes.len() as f64 / 1e6,
            started.elapsed().as_secs_f32(),
            if back.counts().0 == out.blocks { "" } else { "  MISMATCH" }
        );
    }
    Ok(())
}

/// Write the sample build in every format and selection, for checking
/// placement in game.
pub fn sample(out_dir: &str) -> Result<(), String> {
    let source = export::sample_build();
    let cat = catalogue::default();
    let mut written = 0;
    let variants = [
        (Format::Litematic, AirMode::OmitAir, "litematica-omit-air"),
        (Format::Litematic, AirMode::Compact, "litematica-compact"),
        (Format::Sponge, AirMode::Compact, "worldedit"),
        (Format::Axiom, AirMode::Compact, "axiom"),
    ];
    for (format, air, folder) in variants {
        let dir = Path::new(out_dir).join(folder);
        std::fs::create_dir_all(&dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
        for selection in Selection::ALL {
            let options = ExportOptions { format, selection, air, include_colour_glass: true };
            let out = export::export(&source, &options, cat, &Progress::none()).map_err(|e| e.to_string())?;
            let path = dir.join(format!("sample-{}.{}", selection.key().replace('_', "-"), format.extension()));
            std::fs::write(&path, &out.bytes).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
            println!("{:<58} {:>5} blocks, {:>4} region(s)", path.display(), out.blocks, out.regions);
            written += 1;
        }
    }
    println!("Wrote {written} files for Minecraft {} (data version {})", cat.game_version, cat.data_version);
    Ok(())
}
