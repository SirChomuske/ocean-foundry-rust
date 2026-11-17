//! The in-memory form every file format is read into and written from.
//!
//! A cell is in one of three states: a block, air (which is just a block
//! state the game treats as empty), or *absent* (the file says nothing about
//! that position). Keeping "absent" separate from air is what lets an export
//! avoid erasing terrain it was never meant to touch.

use crate::nbt::{Compound, Tag};
use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

// ------------------------------------------------------------- block state

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct BlockState {
    pub name: String,
    /// Sorted by property name, so equal states compare equal.
    pub props: Vec<(String, String)>,
}

impl BlockState {
    pub fn new(name: &str) -> Self {
        let name = if name.contains(':') { name.to_string() } else { format!("minecraft:{name}") };
        Self { name, props: Vec::new() }
    }
    pub fn with(mut self, key: &str, value: &str) -> Self {
        self.props.retain(|(k, _)| k != key);
        self.props.push((key.to_string(), value.to_string()));
        self.props.sort();
        self
    }
    pub fn air() -> Self {
        Self::new("minecraft:air")
    }
    pub fn is_air(&self) -> bool {
        matches!(self.name.as_str(), "minecraft:air" | "minecraft:cave_air" | "minecraft:void_air")
    }
    pub fn prop(&self, key: &str) -> Option<&str> {
        self.props.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
    }
    /// Parse the text form, e.g. `minecraft:oak_log[axis=y]`.
    pub fn parse(text: &str) -> Option<Self> {
        let text = text.trim();
        let (name, rest) = match text.find('[') {
            Some(i) => (&text[..i], Some(text[i + 1..].strip_suffix(']')?)),
            None => (text, None),
        };
        if name.is_empty() || name.contains(|c: char| c.is_whitespace() || c == ']' || c == '=') {
            return None;
        }
        let mut state = Self::new(name);
        if let Some(rest) = rest {
            for pair in rest.split(',').filter(|p| !p.trim().is_empty()) {
                let (k, v) = pair.split_once('=')?;
                state.props.push((k.trim().to_string(), v.trim().to_string()));
            }
            state.props.sort();
        }
        Some(state)
    }
    /// The `{Name, Properties}` form used inside NBT palettes.
    pub fn from_nbt(c: &Compound) -> Option<Self> {
        let mut state = Self::new(c.str("Name")?);
        if let Some(props) = c.compound("Properties") {
            for (k, v) in props.iter() {
                let value = match v {
                    Tag::String(s) => s.clone(),
                    other => other.as_i64()?.to_string(),
                };
                state.props.push((k.to_string(), value));
            }
            state.props.sort();
        }
        Some(state)
    }
    pub fn to_nbt(&self) -> Tag {
        let mut c = Compound::new().with("Name", Tag::string(&self.name));
        if !self.props.is_empty() {
            let props = self.props.iter().map(|(k, v)| (k.clone(), Tag::string(v))).collect();
            c.set("Properties", Tag::Compound(Compound(props)));
        }
        Tag::Compound(c)
    }
}

impl fmt::Display for BlockState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.name)?;
        if !self.props.is_empty() {
            f.write_str("[")?;
            for (i, (k, v)) in self.props.iter().enumerate() {
                if i > 0 {
                    f.write_str(",")?;
                }
                write!(f, "{k}={v}")?;
            }
            f.write_str("]")?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Default)]
pub struct Palette {
    states: Vec<BlockState>,
    index: HashMap<BlockState, u32>,
}

impl Palette {
    pub fn intern(&mut self, state: BlockState) -> u32 {
        if let Some(&id) = self.index.get(&state) {
            return id;
        }
        let id = self.states.len() as u32;
        self.index.insert(state.clone(), id);
        self.states.push(state);
        id
    }
    pub fn id_of(&self, state: &BlockState) -> Option<u32> {
        self.index.get(state).copied()
    }
    pub fn get(&self, id: u32) -> &BlockState {
        &self.states[id as usize]
    }
    pub fn len(&self) -> usize {
        self.states.len()
    }
    pub fn is_empty(&self) -> bool {
        self.states.is_empty()
    }
    pub fn states(&self) -> &[BlockState] {
        &self.states
    }
}

// ------------------------------------------------------------ sparse store

pub type Pos = [i32; 3];

const CHUNK_CELLS: usize = 4096;

type Cube = [u32; CHUNK_CELLS];

/// A sparse grid of palette ids in 16-block cubes. Cells never written are
/// absent. Coordinates may be negative.
///
/// Cubes that hold one value throughout (all air, say) can share a single
/// stored copy (`share_uniform`); writing to a shared cube copies it first.
/// Why: a build file stores the air of its whole box, and a tall build's box
/// is mostly air; one copy per cube made a 1.4 M block figure take 1.8 GB.
#[derive(Debug, Clone, Default)]
pub struct Volume {
    chunks: HashMap<Pos, Arc<Cube>>,
    /// One shared cube per value, for `share_uniform`.
    uniform: HashMap<u32, Arc<Cube>>,
}

fn empty_cube() -> Arc<Cube> {
    Arc::new([0; CHUNK_CELLS])
}

fn split(p: Pos) -> (Pos, usize) {
    ([p[0] >> 4, p[1] >> 4, p[2] >> 4], (((p[1] & 15) as usize) << 8) | (((p[2] & 15) as usize) << 4) | (p[0] & 15) as usize)
}

fn join(chunk: Pos, i: usize) -> Pos {
    [(chunk[0] << 4) + (i & 15) as i32, (chunk[1] << 4) + (i >> 8) as i32, (chunk[2] << 4) + ((i >> 4) & 15) as i32]
}

impl Volume {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn get(&self, p: Pos) -> Option<u32> {
        let (c, i) = split(p);
        self.chunks.get(&c).and_then(|chunk| chunk[i].checked_sub(1))
    }
    pub fn set(&mut self, p: Pos, id: u32) {
        let (c, i) = split(p);
        Arc::make_mut(self.chunks.entry(c).or_insert_with(empty_cube))[i] = id + 1;
    }
    /// Make a cell absent again.
    pub fn clear(&mut self, p: Pos) {
        let (c, i) = split(p);
        if let Some(chunk) = self.chunks.get_mut(&c) {
            if chunk[i] != 0 {
                Arc::make_mut(chunk)[i] = 0;
            }
        }
    }
    /// Fill `out` with a run of cells along +X, as stored values
    /// (0 = absent, otherwise id + 1). Much faster than repeated `get`.
    pub fn read_row(&self, start: Pos, out: &mut [u32]) {
        let mut x = start[0];
        let mut done = 0;
        while done < out.len() {
            let (c, i) = split([x, start[1], start[2]]);
            let run = (16 - (x & 15) as usize).min(out.len() - done);
            match self.chunks.get(&c) {
                Some(chunk) => out[done..done + run].copy_from_slice(&chunk[i..i + run]),
                None => out[done..done + run].fill(0),
            }
            done += run;
            x += run as i32;
        }
    }
    /// Write a run of cells along +X from stored values (0 = leave the cell alone).
    pub fn write_row(&mut self, start: Pos, values: &[u32]) {
        let mut x = start[0];
        let mut done = 0;
        while done < values.len() {
            let (c, i) = split([x, start[1], start[2]]);
            let run = (16 - (x & 15) as usize).min(values.len() - done);
            let src = &values[done..done + run];
            if src.iter().any(|v| *v != 0) {
                let chunk = Arc::make_mut(self.chunks.entry(c).or_insert_with(empty_cube));
                for (d, s) in chunk[i..i + run].iter_mut().zip(src) {
                    if *s != 0 {
                        *d = *s;
                    }
                }
            }
            done += run;
            x += run as i32;
        }
    }
    pub fn for_each(&self, mut f: impl FnMut(Pos, u32)) {
        for (c, chunk) in &self.chunks {
            for (i, v) in chunk.iter().enumerate() {
                if *v != 0 {
                    f(join(*c, i), v - 1);
                }
            }
        }
    }
    /// Raw access to stored 16-block cubes: (cube position, values as in `read_row`).
    pub fn chunks(&self) -> impl Iterator<Item = (Pos, &[u32; CHUNK_CELLS])> {
        self.chunks.iter().map(|(c, v)| (*c, &**v))
    }
    /// Let every cube that holds one value throughout share one stored copy
    /// of it. `cube_y` limits this to one layer of cubes (a reader calls it
    /// as each layer is finished); `None` does all of them. Nothing a reader
    /// sees changes, only how much memory it takes.
    pub fn share_uniform(&mut self, cube_y: Option<i32>) {
        let uniform = &mut self.uniform;
        for (pos, chunk) in self.chunks.iter_mut() {
            if cube_y.is_some_and(|y| pos[1] != y) || Arc::strong_count(chunk) > 1 {
                continue;
            }
            let first = chunk[0];
            if chunk.iter().all(|v| *v == first) {
                *chunk = uniform.entry(first).or_insert_with(|| Arc::new([first; CHUNK_CELLS])).clone();
            }
        }
    }
    /// Cubes held, and how many of them are stored separately (not shared).
    pub fn cube_counts(&self) -> (usize, usize) {
        let mut seen = std::collections::HashSet::new();
        let stored = self.chunks.values().filter(|c| seen.insert(Arc::as_ptr(c))).count();
        (self.chunks.len(), stored)
    }
    pub fn count(&self) -> u64 {
        self.chunks.values().map(|c| c.iter().filter(|v| **v != 0).count() as u64).sum()
    }
    pub fn is_empty(&self) -> bool {
        self.chunks.values().all(|c| c.iter().all(|v| *v == 0))
    }
    /// Smallest box holding every present cell: (min corner, max corner), inclusive.
    pub fn bounds(&self) -> Option<(Pos, Pos)> {
        let mut b: Option<(Pos, Pos)> = None;
        self.for_each(|p, _| {
            let (lo, hi) = b.get_or_insert((p, p));
            for a in 0..3 {
                lo[a] = lo[a].min(p[a]);
                hi[a] = hi[a].max(p[a]);
            }
        });
        b
    }
    /// Copy every present cell of `other` into this volume.
    pub fn overlay(&mut self, other: &Volume) {
        for (c, src) in &other.chunks {
            if src.iter().all(|v| *v == 0) {
                continue;
            }
            let dst = Arc::make_mut(self.chunks.entry(*c).or_insert_with(empty_cube));
            for (d, s) in dst.iter_mut().zip(src.iter()) {
                if *s != 0 {
                    *d = *s;
                }
            }
        }
    }
}

// ---------------------------------------------------------------- the build

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Format {
    Litematic,
    Sponge,
    Legacy,
    Axiom,
}

impl Format {
    pub const EXPORTABLE: [Format; 3] = [Format::Litematic, Format::Sponge, Format::Axiom];
    pub fn key(self) -> &'static str {
        match self {
            Format::Litematic => "litematic",
            Format::Sponge => "sponge",
            Format::Legacy => "legacy",
            Format::Axiom => "axiom",
        }
    }
    pub fn extension(self) -> &'static str {
        match self {
            Format::Litematic => "litematic",
            Format::Sponge => "schem",
            Format::Legacy => "schematic",
            Format::Axiom => "bp",
        }
    }
    pub fn from_key(key: &str) -> Option<Self> {
        [Format::Litematic, Format::Sponge, Format::Legacy, Format::Axiom].into_iter().find(|f| f.key() == key)
    }
}

/// A block's extra data (chest contents, sign text and so on), kept whole.
#[derive(Debug, Clone, PartialEq)]
pub struct BlockEntity {
    pub pos: Pos,
    pub id: Option<String>,
    /// Everything except the position and id.
    pub data: Compound,
}

/// A box of cells. Litematica files may hold several; other formats hold one.
#[derive(Debug, Clone)]
pub struct Region {
    pub name: String,
    /// Smallest corner of the box, in the build's coordinates.
    pub min: Pos,
    /// Box size along each axis, always positive.
    pub dims: Pos,
    /// Litematica allows a box to be described from its far corner with a
    /// negative size. This records which axes were, so the file can be
    /// written back the same way.
    pub flipped: [bool; 3],
    /// Cells in the build's coordinates (not relative to `min`).
    pub cells: Arc<Volume>,
    pub block_entities: Vec<BlockEntity>,
    /// Entities exactly as the source format stored them.
    pub entities: Vec<Tag>,
    /// Region tags this program does not interpret, carried through untouched.
    pub extra: Compound,
}

impl Region {
    pub fn new(name: &str, min: Pos, dims: Pos) -> Self {
        Self {
            name: name.to_string(),
            min,
            dims,
            flipped: [false; 3],
            cells: Arc::new(Volume::new()),
            block_entities: Vec::new(),
            entities: Vec::new(),
            extra: Compound::new(),
        }
    }
    pub fn max(&self) -> Pos {
        [self.min[0] + self.dims[0] - 1, self.min[1] + self.dims[1] - 1, self.min[2] + self.dims[2] - 1]
    }
    pub fn volume(&self) -> u64 {
        self.dims.iter().map(|d| *d as u64).product()
    }
    pub fn contains(&self, p: Pos) -> bool {
        (0..3).all(|a| p[a] >= self.min[a] && p[a] < self.min[a] + self.dims[a])
    }
    /// Visit every present cell inside this region's box. Regions may share
    /// one store, so the box (not the store) decides what belongs to a region.
    pub fn for_each(&self, mut f: impl FnMut(Pos, u32)) {
        let mut row = vec![0u32; self.dims[0].max(0) as usize];
        for y in self.min[1]..self.min[1] + self.dims[1] {
            for z in self.min[2]..self.min[2] + self.dims[2] {
                self.cells.read_row([self.min[0], y, z], &mut row);
                for (i, v) in row.iter().enumerate() {
                    if *v != 0 {
                        f([self.min[0] + i as i32, y, z], v - 1);
                    }
                }
            }
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct Meta {
    pub name: String,
    pub author: String,
    pub description: String,
    /// Milliseconds since 1970, when known.
    pub created: Option<i64>,
    pub modified: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct Build {
    pub source: Option<Format>,
    /// The source file's own format revision (Litematica "Version", Sponge "Version").
    pub source_version: Option<i32>,
    /// Minecraft data version the blocks were saved with, when the file says.
    pub data_version: Option<i32>,
    pub meta: Meta,
    pub palette: Palette,
    pub regions: Vec<Region>,
    /// Top-level tags this program does not interpret, carried through to a
    /// same-format export.
    pub extra: Compound,
}

impl Build {
    pub fn new() -> Self {
        Self {
            source: None,
            source_version: None,
            data_version: None,
            meta: Meta::default(),
            palette: Palette::default(),
            regions: Vec::new(),
            extra: Compound::new(),
        }
    }
    /// Smallest box holding every region: (min, max), inclusive.
    pub fn bounds(&self) -> Option<(Pos, Pos)> {
        let mut regions = self.regions.iter();
        let first = regions.next()?;
        let mut b = (first.min, first.max());
        for r in regions {
            let hi = r.max();
            for a in 0..3 {
                b.0[a] = b.0[a].min(r.min[a]);
                b.1[a] = b.1[a].max(hi[a]);
            }
        }
        Some(b)
    }
    /// The block at a position; later regions win where regions overlap.
    pub fn get(&self, p: Pos) -> Option<&BlockState> {
        self.regions.iter().rev().filter(|r| r.contains(p)).find_map(|r| r.cells.get(p)).map(|id| self.palette.get(id))
    }
    pub fn block_entity_count(&self) -> usize {
        self.regions.iter().map(|r| r.block_entities.len()).sum()
    }
    pub fn entity_count(&self) -> usize {
        self.regions.iter().map(|r| r.entities.len()).sum()
    }
    /// Count cells by kind: (blocks that are not air, air cells).
    pub fn counts(&self) -> (u64, u64) {
        let air: Vec<bool> = self.palette.states().iter().map(BlockState::is_air).collect();
        let (mut blocks, mut airs) = (0u64, 0u64);
        for r in &self.regions {
            r.for_each(|_, id| if air[id as usize] { airs += 1 } else { blocks += 1 });
        }
        (blocks, airs)
    }
    /// How many cells use each block state, most common first.
    pub fn histogram(&self) -> Vec<(BlockState, u64)> {
        let mut n = vec![0u64; self.palette.len()];
        for r in &self.regions {
            r.for_each(|_, id| n[id as usize] += 1);
        }
        let mut out: Vec<_> = n.into_iter().enumerate().filter(|(_, c)| *c > 0).map(|(i, c)| (self.palette.get(i as u32).clone(), c)).collect();
        out.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        out
    }
}

impl Default for Build {
    fn default() -> Self {
        Self::new()
    }
}

// ------------------------------------------------------ problems and notes

/// A message for the user, as a language-table key plus values to fill in.
/// Keys are `file.<key>` in the language tables.
#[derive(Debug, Clone, PartialEq)]
pub struct Note {
    pub key: &'static str,
    pub args: Vec<(&'static str, String)>,
}

impl Note {
    pub fn new(key: &'static str) -> Self {
        Self { key, args: Vec::new() }
    }
    pub fn arg(mut self, name: &'static str, value: impl ToString) -> Self {
        self.args.push((name, value.to_string()));
        self
    }
}

impl fmt::Display for Note {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.key)?;
        for (k, v) in &self.args {
            write!(f, " {k}={v}")?;
        }
        Ok(())
    }
}

/// A reason an import or export was refused.
pub type Problem = Note;

/// Compare two builds cell by cell, ignoring how they are split into regions
/// when `layout` is false. Returns a description of the first difference.
pub fn first_difference(a: &Build, b: &Build, layout: bool) -> Option<String> {
    if layout {
        if a.regions.len() != b.regions.len() {
            return Some(format!("region count {} vs {}", a.regions.len(), b.regions.len()));
        }
        for (x, y) in a.regions.iter().zip(&b.regions) {
            if (x.min, x.dims, x.flipped, &x.name) != (y.min, y.dims, y.flipped, &y.name) {
                return Some(format!("region '{}' {:?}+{:?} vs '{}' {:?}+{:?}", x.name, x.min, x.dims, y.name, y.min, y.dims));
            }
        }
    }
    fn flat(build: &Build) -> HashMap<Pos, &BlockState> {
        let mut cells: HashMap<Pos, &BlockState> = HashMap::new();
        for r in &build.regions {
            r.for_each(|p, id| {
                cells.insert(p, build.palette.get(id));
            });
        }
        cells
    }
    let (ca, cb) = (flat(a), flat(b));
    if ca.len() != cb.len() {
        return Some(format!("{} cells vs {}", ca.len(), cb.len()));
    }
    for (p, s) in &ca {
        match cb.get(p) {
            Some(t) if t == s => {}
            Some(t) => return Some(format!("at {p:?}: {s} vs {t}")),
            None => return Some(format!("at {p:?}: {s} vs nothing")),
        }
    }
    let entities = |build: &Build| {
        let mut list: Vec<BlockEntity> = build.regions.iter().flat_map(|r| r.block_entities.iter().cloned()).collect();
        list.sort_by_key(|e| e.pos);
        list
    };
    let (ea, eb) = (entities(a), entities(b));
    if ea.len() != eb.len() {
        return Some(format!("{} block entities vs {}", ea.len(), eb.len()));
    }
    for (x, y) in ea.iter().zip(&eb) {
        if x.pos != y.pos || x.id != y.id || !x.data.same_content(&y.data) {
            return Some(format!("block entity at {:?} differs", x.pos));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn block_state_text_round_trips_and_sorts_properties() {
        let s = BlockState::parse("minecraft:oak_stairs[half=top,facing=north]").unwrap();
        assert_eq!(s.to_string(), "minecraft:oak_stairs[facing=north,half=top]");
        assert_eq!(BlockState::parse("stone").unwrap().name, "minecraft:stone");
        assert_eq!(s, BlockState::from_nbt(s.to_nbt().as_compound().unwrap()).unwrap());
        assert!(BlockState::parse("bad[").is_none());
        assert!(BlockState::parse("").is_none());
        assert!(BlockState::new("cave_air").is_air());
    }

    #[test]
    fn absent_air_and_block_are_three_different_things() {
        let mut palette = Palette::default();
        let air = palette.intern(BlockState::air());
        let stone = palette.intern(BlockState::new("stone"));
        let mut v = Volume::new();
        v.set([-1, -70, 5], air);
        v.set([40, 3, -17], stone);
        assert_eq!(v.get([-1, -70, 5]), Some(air));
        assert_eq!(v.get([40, 3, -17]), Some(stone));
        assert_eq!(v.get([0, 0, 0]), None);
        assert_eq!(v.count(), 2);
        assert_eq!(v.bounds(), Some(([-1, -70, -17], [40, 3, 5])));
        v.clear([-1, -70, 5]);
        assert_eq!(v.get([-1, -70, 5]), None);
    }

    #[test]
    fn rows_read_across_cube_edges_match_single_reads() {
        let mut v = Volume::new();
        for x in -40..40 {
            if x % 3 != 0 {
                v.set([x, 7, -3], (x + 100) as u32);
            }
        }
        let mut row = vec![9; 90];
        v.read_row([-45, 7, -3], &mut row);
        for (i, stored) in row.iter().enumerate() {
            let expect = v.get([-45 + i as i32, 7, -3]).map(|id| id + 1).unwrap_or(0);
            assert_eq!(*stored, expect);
        }
    }

    #[test]
    fn shared_cubes_read_the_same_and_copy_before_a_write() {
        let mut v = Volume::new();
        // Two cubes of air (id 0), one mixed cube.
        v.write_row([0, 0, 0], &[1; 32]);
        for y in 0..16 {
            for z in 0..16 {
                v.write_row([0, y, z], &[1; 32]);
            }
        }
        v.set([40, 3, 3], 5);
        v.share_uniform(None);
        assert_eq!(v.cube_counts(), (3, 2));
        assert_eq!(v.count(), 2 * 4096 + 1);
        v.set([17, 2, 2], 7);
        assert_eq!(v.get([17, 2, 2]), Some(7));
        assert_eq!(v.get([1, 2, 2]), Some(0), "the other shared cube is untouched");
        assert_eq!(v.cube_counts(), (3, 3));
        v.clear([2, 2, 2]);
        assert_eq!(v.get([2, 2, 2]), None);
        assert_eq!(v.get([3, 2, 2]), Some(0));
    }
}
