//! Turning a layered build into a file: which layers go in, how empty space
//! is handled, and the checks that refuse an export rather than let it
//! damage a world.

use crate::catalogue::Catalogue;
use crate::legacy;
use crate::model::{BlockEntity, BlockState, Build, Format, Note, Palette, Pos, Problem, Region, Volume};
use crate::{Progress, Rows};
use std::collections::HashMap;
use std::sync::Arc;

/// The parts a finished build is made of. Every layer uses the same
/// coordinates, so files exported separately line up when placed at the
/// same spot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Layer {
    /// Imported terrain, kept exactly.
    Terrain,
    /// The single-concrete base shape.
    Foundation,
    /// Opaque coloured blocks on the surface.
    Surface,
    Glass1,
    Glass2,
    Glass3,
}

impl Layer {
    pub const ALL: [Layer; 6] = [Layer::Terrain, Layer::Foundation, Layer::Surface, Layer::Glass1, Layer::Glass2, Layer::Glass3];
    fn index(self) -> usize {
        Self::ALL.iter().position(|l| *l == self).unwrap()
    }
}

#[derive(Debug, Clone, Default)]
pub struct LayeredBuild {
    pub name: String,
    pub author: String,
    pub palette: Palette,
    layers: [Volume; 6],
    /// Block entities belonging to the terrain layer.
    pub block_entities: Vec<BlockEntity>,
}

impl LayeredBuild {
    pub fn new(name: &str) -> Self {
        Self { name: name.to_string(), ..Self::default() }
    }
    pub fn set(&mut self, layer: Layer, pos: Pos, state: &BlockState) {
        let id = self.palette.intern(state.clone());
        self.layers[layer.index()].set(pos, id);
    }
    pub fn layer(&self, layer: Layer) -> &Volume {
        &self.layers[layer.index()]
    }
    pub fn layer_mut(&mut self, layer: Layer) -> &mut Volume {
        &mut self.layers[layer.index()]
    }
    /// Only the part of the build from `lo` to `hi` in X and Z, both
    /// included, at the same positions.
    pub fn cropped(&self, lo: [i32; 2], hi: [i32; 2]) -> Self {
        let inside = |p: Pos| p[0] >= lo[0] && p[0] <= hi[0] && p[2] >= lo[1] && p[2] <= hi[1];
        let mut out = Self { name: self.name.clone(), author: self.author.clone(), palette: self.palette.clone(), ..Self::default() };
        for (n, volume) in self.layers.iter().enumerate() {
            volume.for_each(|p, id| {
                if inside(p) {
                    out.layers[n].set(p, id);
                }
            });
        }
        out.block_entities = self.block_entities.iter().filter(|e| inside(e.pos)).cloned().collect();
        out
    }
    /// Positions claimed by more than one of the given layers. Exporting
    /// such a build would make one layer overwrite another (or the terrain).
    pub fn conflicts(&self, layers: &[Layer]) -> u64 {
        let mut seen = Volume::new();
        let mut clashes = 0u64;
        for layer in layers {
            self.layer(*layer).for_each(|p, _| {
                if seen.get(p).is_some() {
                    clashes += 1;
                } else {
                    seen.set(p, 0);
                }
            });
        }
        clashes
    }
}

/// Where an export reads its blocks from: the bake's own columns, or blocks
/// already laid out in layers. Every export streams from one of these.
pub trait BlockSource: Sync {
    fn name(&self) -> &str;
    fn author(&self) -> &str {
        ""
    }
    fn palette(&self) -> &Palette;
    /// Block entities of the terrain layer.
    fn block_entities(&self) -> &[BlockEntity] {
        &[]
    }
    /// Smallest box holding every block of these layers: (min, max), inclusive.
    fn bounds(&self, layers: &[Layer]) -> Option<(Pos, Pos)>;
    /// The cells of these layers along +X from `start`, as stored values
    /// (0 = none, otherwise palette id + 1). Where layers overlap, the one
    /// later in `layers` wins.
    fn read_row(&self, layers: &[Layer], start: Pos, out: &mut [u32]);
    /// Places held by more than one of these layers.
    fn conflicts(&self, layers: &[Layer]) -> u64;
}

impl BlockSource for LayeredBuild {
    fn name(&self) -> &str {
        &self.name
    }
    fn author(&self) -> &str {
        &self.author
    }
    fn palette(&self) -> &Palette {
        &self.palette
    }
    fn block_entities(&self) -> &[BlockEntity] {
        &self.block_entities
    }
    fn bounds(&self, layers: &[Layer]) -> Option<(Pos, Pos)> {
        let mut b: Option<(Pos, Pos)> = None;
        for (lo, hi) in layers.iter().filter_map(|l| self.layer(*l).bounds()) {
            let (b_lo, b_hi) = b.get_or_insert((lo, hi));
            for a in 0..3 {
                b_lo[a] = b_lo[a].min(lo[a]);
                b_hi[a] = b_hi[a].max(hi[a]);
            }
        }
        b
    }
    fn read_row(&self, layers: &[Layer], start: Pos, out: &mut [u32]) {
        out.fill(0);
        let mut part = [0u32; 64];
        for (n, piece) in out.chunks_mut(64).enumerate() {
            let at = [start[0] + 64 * n as i32, start[1], start[2]];
            for layer in layers {
                self.layer(*layer).read_row(at, &mut part[..piece.len()]);
                for (o, v) in piece.iter_mut().zip(&part) {
                    if *v != 0 {
                        *o = *v;
                    }
                }
            }
        }
    }
    fn conflicts(&self, layers: &[Layer]) -> u64 {
        LayeredBuild::conflicts(self, layers)
    }
}

/// What to put in the exported file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Selection {
    Complete,
    WaterOnly,
    Foundation,
    Surface,
    AllGlass,
    Glass1,
    Glass2,
    Glass3,
}

impl Selection {
    pub const ALL: [Selection; 8] = [
        Selection::Complete,
        Selection::WaterOnly,
        Selection::Foundation,
        Selection::Surface,
        Selection::AllGlass,
        Selection::Glass1,
        Selection::Glass2,
        Selection::Glass3,
    ];
    pub fn key(self) -> &'static str {
        match self {
            Selection::Complete => "complete",
            Selection::WaterOnly => "water_only",
            Selection::Foundation => "foundation",
            Selection::Surface => "surface",
            Selection::AllGlass => "all_glass",
            Selection::Glass1 => "glass_1",
            Selection::Glass2 => "glass_2",
            Selection::Glass3 => "glass_3",
        }
    }
    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|s| s.key() == key)
    }
    /// The layers this choice exports. With "Include colour and glass" off
    /// the build is the foundation alone, so the colour and glass choices
    /// have nothing to export and are refused.
    pub fn layers(self, include_colour_glass: bool) -> Result<Vec<Layer>, Problem> {
        use Layer::*;
        let colour: &[Layer] = if include_colour_glass { &[Surface, Glass1, Glass2, Glass3] } else { &[] };
        let chosen: Vec<Layer> = match self {
            Selection::Complete => [&[Terrain, Foundation][..], colour].concat(),
            Selection::WaterOnly => [&[Foundation][..], colour].concat(),
            Selection::Foundation => vec![Foundation],
            _ if !include_colour_glass => return Err(Problem::new("err.colour_off")),
            Selection::Surface => vec![Surface],
            Selection::AllGlass => vec![Glass1, Glass2, Glass3],
            Selection::Glass1 => vec![Glass1],
            Selection::Glass2 => vec![Glass2],
            Selection::Glass3 => vec![Glass3],
        };
        Ok(chosen)
    }
}

/// How a Litematica export treats positions with no block.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AirMode {
    /// Only occupied positions are written, as a set of boxes. Nothing in
    /// the world is replaced by air, whatever Litematica's settings are.
    OmitAir,
    /// One box around everything. Empty positions inside it are written as
    /// air, so Litematica must be told to ignore air when pasting.
    Compact,
}

impl AirMode {
    pub fn key(self) -> &'static str {
        match self {
            AirMode::OmitAir => "omit_air",
            AirMode::Compact => "compact",
        }
    }
}

#[derive(Debug, Clone)]
pub struct ExportOptions {
    pub format: Format,
    pub selection: Selection,
    pub air: AirMode,
    pub include_colour_glass: bool,
}

pub struct Exported {
    pub bytes: Vec<u8>,
    pub notes: Vec<Note>,
    pub regions: usize,
    /// Blocks written, not counting air.
    pub blocks: u64,
}

/// Above this many boxes the export carries a warning. Litematica lists and
/// draws every region separately; the figure is a cautious guess that has
/// not been measured in game.
pub const COMFORTABLE_REGIONS: usize = 2_000;

/// Cover every cell for which `solid` is true with non-overlapping boxes,
/// merging neighbours greedily (along x, then z, then y). Returns inclusive
/// (min, max) corners.
///
/// Which axis is grown first changes the count a good deal on wavy
/// surfaces, so every order is tried (in parallel) and the fewest boxes win.
pub fn boxes(cells: &Volume, solid: &(dyn Fn(u32) -> bool + Sync), progress: &Progress) -> Result<Vec<(Pos, Pos)>, Problem> {
    let mut todo = Occupancy::default();
    cells.for_each(|p, id| {
        if solid(id) {
            todo.set(p);
        }
    });
    boxes_of(&todo, progress)
}

/// Which cells are taken, one bit each, in 16-block cubes. The box search
/// works on this rather than on the blocks themselves.
///
/// Why: the search tries six orders at once, each on its own copy; copies of
/// a 4-byte-per-cell store took far more memory than the export itself.
#[derive(Clone, Default)]
struct Occupancy {
    cubes: HashMap<Pos, [u64; 64]>,
}

impl Occupancy {
    fn split(p: Pos) -> (Pos, usize) {
        ([p[0] >> 4, p[1] >> 4, p[2] >> 4], (((p[1] & 15) as usize) << 8) | (((p[2] & 15) as usize) << 4) | (p[0] & 15) as usize)
    }
    fn set(&mut self, p: Pos) {
        let (c, i) = Self::split(p);
        self.cubes.entry(c).or_insert([0; 64])[i >> 6] |= 1 << (i & 63);
    }
    fn get(&self, p: Pos) -> bool {
        let (c, i) = Self::split(p);
        self.cubes.get(&c).is_some_and(|bits| bits[i >> 6] & (1 << (i & 63)) != 0)
    }
    fn clear(&mut self, p: Pos) {
        let (c, i) = Self::split(p);
        if let Some(bits) = self.cubes.get_mut(&c) {
            bits[i >> 6] &= !(1 << (i & 63));
        }
    }
}

fn boxes_of(todo: &Occupancy, progress: &Progress) -> Result<Vec<(Pos, Pos)>, Problem> {
    const ORDERS: [[usize; 3]; 6] = [[0, 2, 1], [2, 0, 1], [1, 0, 2], [1, 2, 0], [0, 1, 2], [2, 1, 0]];
    let results: Vec<Result<Vec<(Pos, Pos)>, Problem>> = std::thread::scope(|scope| {
        let handles: Vec<_> = ORDERS
            .iter()
            .enumerate()
            .map(|(n, order)| scope.spawn(move || boxes_in_order(todo.clone(), *order, if n == 0 { progress } else { &Progress(None) })))
            .collect();
        handles.into_iter().map(|h| h.join().expect("box merging does not panic")).collect()
    });
    let mut best: Option<Vec<(Pos, Pos)>> = None;
    for result in results {
        let found = result?;
        if best.as_ref().is_none_or(|b| found.len() < b.len()) {
            best = Some(found);
        }
    }
    Ok(best.unwrap_or_default())
}

fn boxes_in_order(mut todo: Occupancy, order: [usize; 3], progress: &Progress) -> Result<Vec<(Pos, Pos)>, Problem> {
    let [a, b, c] = order;
    let mut keys: Vec<Pos> = todo.cubes.keys().copied().collect();
    keys.sort_by_key(|k| (k[1], k[2], k[0]));
    let mut out = Vec::new();
    for (n, key) in keys.iter().enumerate() {
        for i in 0..4096usize {
            let p = [(key[0] << 4) + (i & 15) as i32, (key[1] << 4) + (i >> 8) as i32, (key[2] << 4) + ((i >> 4) & 15) as i32];
            if !todo.get(p) {
                continue;
            }
            // A run along the first axis, widened along the second while
            // every cell of the run is there, then along the third while
            // the whole slab is.
            let mut hi = p;
            let mut probe = p;
            probe[a] += 1;
            while todo.get(probe) {
                hi[a] = probe[a];
                probe[a] += 1;
            }
            let run_full = |todo: &Occupancy, at_b: i32, at_c: i32| {
                let mut q = p;
                q[b] = at_b;
                q[c] = at_c;
                (p[a]..=hi[a]).all(|v| {
                    q[a] = v;
                    todo.get(q)
                })
            };
            let mut end_b = p[b];
            while run_full(&todo, end_b + 1, p[c]) {
                end_b += 1;
            }
            let mut end_c = p[c];
            while (p[b]..=end_b).all(|v| run_full(&todo, v, end_c + 1)) {
                end_c += 1;
            }
            hi[b] = end_b;
            hi[c] = end_c;
            for y in p[1]..=hi[1] {
                for z in p[2]..=hi[2] {
                    for x in p[0]..=hi[0] {
                        todo.clear([x, y, z]);
                    }
                }
            }
            out.push((p, hi));
        }
        progress.step((n + 1) as f32 / keys.len() as f32)?;
    }
    Ok(out)
}

/// Checks shared by every export. Refuses rather than writes a file that
/// could not be placed as intended. A file holds positions relative to the
/// build only, so where it is pasted, and whether that fits the world's
/// height, is left to whoever pastes it.
pub fn check(build: &Build, catalogue: &Catalogue, strict_names: bool) -> Result<(), Problem> {
    let used = build.histogram();
    check_blocks(used.iter().map(|(s, _)| s), catalogue, strict_names)
}

/// Refuse placeholders, and with `strict_names` any block the target version lacks.
fn check_blocks<'a>(used: impl Iterator<Item = &'a BlockState>, catalogue: &Catalogue, strict_names: bool) -> Result<(), Problem> {
    let mut unknown: Vec<String> = used.filter(|s| legacy::is_placeholder(s) || (strict_names && !catalogue.knows(s))).map(|s| s.name.clone()).collect();
    unknown.sort();
    unknown.dedup();
    if !unknown.is_empty() {
        let more = if unknown.len() > 8 { ", …" } else { "" };
        let list = unknown.iter().take(8).cloned().collect::<Vec<_>>().join(", ");
        return Err(Problem::new("err.unknown_blocks")
            .arg("count", unknown.len())
            .arg("list", format!("{list}{more}"))
            .arg("version", &catalogue.game_version));
    }
    Ok(())
}

/// The chosen layers of a source as rows, as the writers read them.
struct Selected<'a> {
    source: &'a dyn BlockSource,
    layers: &'a [Layer],
}

impl Rows for Selected<'_> {
    fn read_row(&self, start: Pos, out: &mut [u32]) {
        self.source.read_row(self.layers, start, out)
    }
}

/// One export: one format, one file. The blocks are read from `source` as
/// each piece of the file is written; nothing is copied out of it first.
/// The regions only give the layout: one box around everything, or with
/// Omit air one box per run of blocks, found from a bit per cell.
pub fn export(source: &dyn BlockSource, options: &ExportOptions, catalogue: &Catalogue, progress: &Progress) -> Result<Exported, Problem> {
    let layers = options.selection.layers(options.include_colour_glass)?;
    let clashes = source.conflicts(&layers);
    if clashes > 0 {
        return Err(Problem::new("err.overwrite").arg("count", clashes));
    }
    let omit_air = options.format == Format::Litematic && options.air == AirMode::OmitAir;
    let palette = source.palette();
    let is_air: Vec<bool> = palette.states().iter().map(BlockState::is_air).collect();
    let (lo, hi) = source.bounds(&layers).ok_or_else(|| Problem::new("err.empty"))?;

    // One pass over the box: which blocks are used, how many are not air,
    // the box around those, and (to omit air) where they are.
    let rows = Selected { source, layers: &layers };
    let mut used = vec![false; palette.len()];
    let mut blocks = 0u64;
    let mut solid: Option<(Pos, Pos)> = None;
    let mut taken = Occupancy::default();
    let mut row = vec![0u32; (hi[0] - lo[0] + 1) as usize];
    for y in lo[1]..=hi[1] {
        for z in lo[2]..=hi[2] {
            rows.read_row([lo[0], y, z], &mut row);
            for (dx, v) in row.iter().enumerate() {
                if *v == 0 {
                    continue;
                }
                let id = (*v - 1) as usize;
                used[id] = true;
                if is_air[id] {
                    continue;
                }
                blocks += 1;
                let p = [lo[0] + dx as i32, y, z];
                let (s_lo, s_hi) = solid.get_or_insert((p, p));
                for a in 0..3 {
                    s_lo[a] = s_lo[a].min(p[a]);
                    s_hi[a] = s_hi[a].max(p[a]);
                }
                if omit_air {
                    taken.set(p);
                }
            }
        }
        progress.step((y - lo[1] + 1) as f32 / (hi[1] - lo[1] + 1) as f32)?;
    }
    if omit_air && solid.is_none() {
        return Err(Problem::new("err.empty"));
    }
    check_blocks(used.iter().enumerate().filter(|(_, u)| **u).map(|(id, _)| palette.get(id as u32)), catalogue, true)?;

    let found: Vec<(Pos, Pos)> = if omit_air { boxes_of(&taken, progress)? } else { vec![(lo, hi)] };
    drop(taken);
    let mut build = Build::new();
    build.data_version = Some(catalogue.data_version);
    build.meta.name = source.name().to_string();
    build.meta.author = source.author().to_string();
    build.palette = palette.clone();
    build.regions = box_regions(source.name(), &found, Arc::new(Volume::new()));
    if layers.contains(&Layer::Terrain) {
        place_block_entities(&mut build.regions, source.block_entities().iter());
    }

    let mut notes = Vec::new();
    match (options.format, options.air) {
        (Format::Litematic, AirMode::OmitAir) => {
            notes.push(Note::new("note.omit_air").arg("count", found.len()));
            if found.len() > COMFORTABLE_REGIONS {
                notes.push(Note::new("note.many_regions").arg("count", found.len()).arg("limit", COMFORTABLE_REGIONS));
            }
        }
        (Format::Litematic, AirMode::Compact) => notes.push(Note::new("note.compact")),
        (Format::Sponge, _) => notes.push(Note::new("note.sponge_air")),
        (Format::Axiom, _) => notes.push(Note::new("note.axiom_air")),
        (Format::Legacy, _) => {}
    }
    let (bytes, more) = crate::write_from(&build, Some(&rows), options.format, catalogue.data_version, progress)?;
    notes.extend(more);
    Ok(Exported { bytes, notes, regions: build.regions.len(), blocks })
}

/// Re-save an imported build in another (or the same) format, keeping its
/// layout. The file's own data version is kept so the game upgrades old
/// blocks itself; only placeholders that have no real block are refused.
pub fn convert(build: &Build, format: Format, catalogue: &Catalogue, progress: &Progress) -> Result<Exported, Problem> {
    check(build, catalogue, false)?;
    let (bytes, mut notes) = crate::write(build, format, catalogue.data_version, progress)?;
    let (blocks, air) = build.counts();
    match format {
        Format::Litematic if air > 0 => notes.push(Note::new("note.compact")),
        Format::Sponge => notes.push(Note::new("note.sponge_air")),
        Format::Axiom => notes.push(Note::new("note.axiom_air")),
        _ => {}
    }
    let regions = if format == Format::Litematic { build.regions.len() } else { 1 };
    Ok(Exported { bytes, notes, regions, blocks })
}

/// The same blocks laid out as boxes that contain no air and no empty
/// space, for a Litematica file that cannot remove anything when pasted.
/// Where the build's own regions overlap, later regions win.
pub fn omit_air(build: &Build, progress: &Progress) -> Result<(Build, Vec<Note>), Problem> {
    let mut cells = Volume::new();
    for region in &build.regions {
        region.for_each(|p, id| cells.set(p, id));
    }
    let is_air: Vec<bool> = build.palette.states().iter().map(BlockState::is_air).collect();
    let found = boxes(&cells, &|id| !is_air[id as usize], progress)?;
    if found.is_empty() {
        return Err(Problem::new("err.empty"));
    }
    let mut out = Build::new();
    out.data_version = build.data_version;
    out.meta = build.meta.clone();
    out.palette = build.palette.clone();
    let name = if build.meta.name.is_empty() { "Region" } else { &build.meta.name };
    out.regions = box_regions(name, &found, Arc::new(cells));
    place_block_entities(&mut out.regions, build.regions.iter().flat_map(|r| &r.block_entities));
    let mut notes = vec![Note::new("note.omit_air").arg("count", found.len())];
    if found.len() > COMFORTABLE_REGIONS {
        notes.push(Note::new("note.many_regions").arg("count", found.len()).arg("limit", COMFORTABLE_REGIONS));
    }
    let entities = build.entity_count();
    if entities > 0 {
        notes.push(Note::new("note.entities_dropped").arg("count", entities));
    }
    Ok((out, notes))
}

fn box_regions(name: &str, found: &[(Pos, Pos)], cells: Arc<Volume>) -> Vec<Region> {
    let width = found.len().to_string().len();
    found
        .iter()
        .enumerate()
        .map(|(n, (lo, hi))| {
            let name = if found.len() == 1 { name.to_string() } else { format!("{name} {:0width$}", n + 1) };
            let mut region = Region::new(&name, *lo, [hi[0] - lo[0] + 1, hi[1] - lo[1] + 1, hi[2] - lo[2] + 1]);
            region.cells = cells.clone();
            region
        })
        .collect()
}

/// Give each block entity to the region whose box holds it. Regions are
/// indexed by position first, so this stays fast with very many boxes.
fn place_block_entities<'a>(regions: &mut [Region], entities: impl Iterator<Item = &'a BlockEntity>) {
    let entities: Vec<&BlockEntity> = entities.collect();
    if entities.is_empty() {
        return;
    }
    if regions.len() == 1 {
        regions[0].block_entities.extend(entities.into_iter().filter(|be| regions[0].contains(be.pos)).cloned().collect::<Vec<_>>());
        return;
    }
    let mut wanted: std::collections::HashMap<Pos, Vec<&BlockEntity>> = std::collections::HashMap::new();
    for be in entities {
        wanted.entry(be.pos).or_default().push(be);
    }
    for region in regions.iter_mut() {
        // Small boxes are looked up cell by cell; big ones test every entity.
        if region.volume() <= wanted.len() as u64 {
            let hi = region.max();
            for y in region.min[1]..=hi[1] {
                for z in region.min[2]..=hi[2] {
                    for x in region.min[0]..=hi[0] {
                        if let Some(list) = wanted.get(&[x, y, z]) {
                            region.block_entities.extend(list.iter().map(|be| (*be).clone()));
                        }
                    }
                }
            }
        } else {
            for (pos, list) in &wanted {
                if region.contains(*pos) {
                    region.block_entities.extend(list.iter().map(|be| (*be).clone()));
                }
            }
        }
    }
}

/// A small layered wave used to check placement in game and in tests:
/// a concrete foundation, an opaque coloured crest line and three glass
/// layers above it, 24 by 16 blocks.
pub fn sample_build() -> LayeredBuild {
    let mut b = LayeredBuild::new("Ocean Foundry sample");
    b.author = "Ocean Foundry".to_string();
    let foundation = BlockState::new("light_blue_concrete");
    let surface = BlockState::new("cyan_concrete");
    let crest = BlockState::new("white_concrete");
    let glass = [BlockState::new("light_blue_stained_glass"), BlockState::new("cyan_stained_glass"), BlockState::new("blue_stained_glass")];
    // A strip of shore standing in for imported terrain.
    let (stone, sandstone) = (BlockState::new("stone"), BlockState::new("sandstone"));
    for z in 0..16 {
        for x in -4..0 {
            for y in 0..(9 + x) {
                b.set(Layer::Terrain, [x, y, z], if y >= 7 + x { &sandstone } else { &stone });
            }
        }
    }
    for z in 0..16 {
        for x in 0..24 {
            let wave = ((x as f32 * 0.5).sin() * 2.0 + (z as f32 * 0.35).cos() * 1.5 + 4.5).round() as i32;
            for y in 0..wave {
                b.set(Layer::Foundation, [x, y, z], &foundation);
            }
            b.set(Layer::Surface, [x, wave, z], if wave >= 7 { &crest } else { &surface });
            // Glass fills the troughs up to a level top, one layer at a time.
            for (n, state) in glass.iter().enumerate() {
                let y = wave + 1 + n as i32;
                if y <= 7 {
                    let layer = [Layer::Glass1, Layer::Glass2, Layer::Glass3][n];
                    b.set(layer, [x, y, z], state);
                }
            }
        }
    }
    b
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalogue;
    use crate::model::first_difference;

    fn options(format: Format, selection: Selection, air: AirMode) -> ExportOptions {
        ExportOptions { format, selection, air, include_colour_glass: true }
    }

    fn expected(source: &LayeredBuild, layers: &[Layer], skip_air_fill: bool, bounds_fill: bool) -> std::collections::HashMap<Pos, BlockState> {
        let mut cells = std::collections::HashMap::new();
        let mut merged = Volume::new();
        for l in layers {
            merged.overlay(source.layer(*l));
        }
        merged.for_each(|p, id| {
            cells.insert(p, source.palette.get(id).clone());
        });
        if bounds_fill && !skip_air_fill {
            let (lo, hi) = merged.bounds().unwrap();
            for y in lo[1]..=hi[1] {
                for z in lo[2]..=hi[2] {
                    for x in lo[0]..=hi[0] {
                        cells.entry([x, y, z]).or_insert_with(BlockState::air);
                    }
                }
            }
        }
        cells
    }

    fn actual(build: &Build) -> std::collections::HashMap<Pos, BlockState> {
        let mut cells = std::collections::HashMap::new();
        for r in &build.regions {
            r.for_each(|p, id| {
                assert!(cells.insert(p, build.palette.get(id).clone()).is_none(), "regions overlap at {p:?}");
            });
        }
        cells
    }

    #[test]
    fn every_selection_in_every_format_lands_on_the_same_coordinates() {
        let source = sample_build();
        let cat = catalogue::default();
        for format in Format::EXPORTABLE {
            for selection in Selection::ALL {
                for air in [AirMode::OmitAir, AirMode::Compact] {
                    let out = export(&source, &options(format, selection, air), cat, &Progress::none())
                        .unwrap_or_else(|e| panic!("{format:?} {selection:?}: {e}"));
                    let back = crate::import(&out.bytes, &Progress::none()).unwrap().build;
                    assert_eq!(back.source, Some(format));
                    assert_eq!(back.data_version, Some(cat.data_version));
                    let layers = selection.layers(true).unwrap();
                    let (omit, fill) = match format {
                        Format::Litematic => (air == AirMode::OmitAir, true),
                        Format::Sponge => (false, true),
                        _ => (true, false),
                    };
                    let want = expected(&source, &layers, omit, fill);
                    let got = actual(&back);
                    assert_eq!(got.len(), want.len(), "{format:?} {selection:?} {air:?}");
                    for (p, s) in &want {
                        assert_eq!(got.get(p), Some(s), "{format:?} {selection:?} {air:?} at {p:?}");
                    }
                    assert_eq!(out.blocks, want.values().filter(|s| !s.is_air()).count() as u64);
                }
            }
        }
    }

    #[test]
    fn omit_air_writes_no_air_at_all_and_boxes_do_not_overlap() {
        let source = sample_build();
        let out = export(&source, &options(Format::Litematic, Selection::AllGlass, AirMode::OmitAir), catalogue::default(), &Progress::none()).unwrap();
        assert!(out.regions > 1);
        assert!(out.notes.iter().any(|n| n.key == "note.omit_air"));
        let back = crate::import(&out.bytes, &Progress::none()).unwrap().build;
        assert_eq!(back.regions.len(), out.regions);
        assert_eq!(back.counts().1, 0, "air cells were written");
        // `actual` asserts that no position is covered twice.
        let volume: u64 = back.regions.iter().map(Region::volume).sum();
        assert_eq!(volume, actual(&back).len() as u64);
        // Compact mode is a single box that does contain air.
        let compact = export(&source, &options(Format::Litematic, Selection::AllGlass, AirMode::Compact), catalogue::default(), &Progress::none()).unwrap();
        assert_eq!(compact.regions, 1);
        assert!(crate::import(&compact.bytes, &Progress::none()).unwrap().build.counts().1 > 0);
    }

    #[test]
    fn greedy_boxes_merge_a_solid_block_into_one() {
        let mut v = Volume::new();
        for y in 0..5 {
            for z in -20..20 {
                for x in -3..30 {
                    v.set([x, y, z], 0);
                }
            }
        }
        v.set([100, 0, 0], 0);
        let b = boxes(&v, &|_| true, &Progress::none()).unwrap();
        assert_eq!(b.len(), 2);
        assert!(b.contains(&([-3, 0, -20], [29, 4, 19])));
    }

    #[test]
    fn colour_and_glass_off_means_foundation_only() {
        let source = sample_build();
        let cat = catalogue::default();
        let mut o = options(Format::Litematic, Selection::WaterOnly, AirMode::OmitAir);
        o.include_colour_glass = false;
        let back = crate::import(&export(&source, &o, cat, &Progress::none()).unwrap().bytes, &Progress::none()).unwrap().build;
        let names: Vec<String> = back.histogram().into_iter().map(|(s, _)| s.name).collect();
        assert_eq!(names, vec!["minecraft:light_blue_concrete".to_string()]);
        o.selection = Selection::Glass2;
        assert_eq!(export(&source, &o, cat, &Progress::none()).err().unwrap().key, "err.colour_off");
    }

    #[test]
    fn exports_that_would_do_damage_are_refused() {
        let cat = catalogue::default();
        // Water placed where terrain already is.
        let mut source = sample_build();
        source.set(Layer::Terrain, [3, 1, 3], &BlockState::new("stone"));
        let err = export(&source, &options(Format::Sponge, Selection::Complete, AirMode::Compact), cat, &Progress::none()).err().unwrap();
        assert_eq!(err.key, "err.overwrite");
        // ...but exporting the water alone is still fine.
        assert!(export(&source, &options(Format::Sponge, Selection::WaterOnly, AirMode::Compact), cat, &Progress::none()).is_ok());


        // A block the target version does not have.
        let mut source = sample_build();
        source.set(Layer::Surface, [0, 30, 0], &BlockState::new("minecraft:not_a_real_block"));
        let err = export(&source, &options(Format::Litematic, Selection::Surface, AirMode::OmitAir), cat, &Progress::none()).err().unwrap();
        assert_eq!(err.key, "err.unknown_blocks");
    }

    #[test]
    fn cancelling_stops_an_export() {
        let stop = |_f: f32| false;
        let err = export(&sample_build(), &options(Format::Litematic, Selection::Complete, AirMode::OmitAir), catalogue::default(), &Progress::new(&stop)).err().unwrap();
        assert_eq!(err.key, crate::CANCELLED);
    }

    #[test]
    fn converting_between_formats_keeps_every_block() {
        let source = sample_build();
        let cat = catalogue::default();
        let first = export(&source, &options(Format::Litematic, Selection::Complete, AirMode::Compact), cat, &Progress::none()).unwrap();
        let a = crate::import(&first.bytes, &Progress::none()).unwrap().build;
        for format in Format::EXPORTABLE {
            let bytes = convert(&a, format, cat, &Progress::none()).unwrap().bytes;
            let b = crate::import(&bytes, &Progress::none()).unwrap().build;
            // A single full box survives any format; only Axiom drops nothing either
            // because the box has no absent cells.
            assert_eq!(first_difference(&a, &b, false), None, "{format:?}");
        }
    }
}
