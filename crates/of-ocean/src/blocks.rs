//! The blocks of a bake, worked out from its columns when they are needed.
//!
//! A bake keeps, for every column of the plot, where its water starts and
//! ends (in `Baked`), the ground beside it and which stack of blocks it
//! takes; and, for the land, a link to the build brought in or short runs of
//! ground blocks. Every block of the result follows from those.
//!
//! Why: the six 3D layers the bake used to fill took 30 to 40 times the
//! memory of the water they described (stage 1 of the column bake plan).
//! `Baked::layers` still makes them, for the exporters and the 3D view,
//! until those read the columns themselves.

use crate::build::{Baked, DRY};
use of_formats::export::{BlockSource, Layer, LayeredBuild};
use of_formats::model::{BlockEntity, Build, Palette, Pos};
use std::borrow::Cow;
use std::sync::Arc;

/// The blocks one water column is coloured with, as palette ids: the solid
/// for its surface, and the glass laid over it, layer by layer.
#[derive(Debug, Clone, Copy)]
pub struct Stack {
    pub solid: u32,
    pub glass: [Option<u32>; 3],
}

/// The land the water meets.
#[derive(Debug, Clone)]
pub enum Land {
    None,
    /// Ground laid by the app (test scenes): for each column, from
    /// `starts[i]` to `starts[i + 1]`, runs of one block over levels
    /// `from..=to` (inside the build), with its palette id.
    Ground { starts: Vec<u32>, runs: Vec<(i32, i32, u32)> },
    /// A build brought in from a file, copied block for block. `ids` gives
    /// each of its palette entries an id here, or none for air; `shift` moves
    /// a block of it to its column and its level relative to the resting water.
    Placed { build: Arc<Build>, ids: Vec<Option<u32>>, shift: [i32; 3], reworked: Reworked },
}

/// Columns of a brought-in build that rivers and ponds changed. In each,
/// the build's blocks from a level up are left out, cut away by the water
/// or under it, and the water's own blocks are put in.
#[derive(Debug, Clone, Default)]
pub struct Reworked {
    /// Column, the lowest level whose build block is left out, and the
    /// water's runs in `runs` (from, to).
    pub cols: Vec<(u32, i32, u32, u32)>,
    /// Runs of one block over levels `from..=to`, with its palette id.
    pub runs: Vec<(i32, i32, u32)>,
}

impl Reworked {
    fn find(&self, column: u32) -> Option<&(u32, i32, u32, u32)> {
        self.cols.binary_search_by_key(&column, |c| c.0).ok().map(|k| &self.cols[k])
    }
    /// Whether the build's block at a column and level stays.
    pub fn keeps(&self, column: u32, level: i32) -> bool {
        self.cols.is_empty() || self.find(column).is_none_or(|c| level < c.1)
    }
    /// The water's runs in a column.
    fn runs_at(&self, column: u32) -> &[(i32, i32, u32)] {
        self.find(column).map_or(&[], |c| &self.runs[c.2 as usize..c.3 as usize])
    }
}

/// Everything besides the water's top and bottom that the blocks follow from.
#[derive(Debug, Clone)]
pub struct Columns {
    pub name: String,
    pub palette: Palette,
    pub foundation: u32,
    /// Per column, the highest ground block, inside the build; `DRY` where there is none.
    pub ground: Vec<i32>,
    /// Per column, its stack in `stacks`; `u32::MAX` for plain foundation blocks.
    pub stack: Vec<u32>,
    pub stacks: Vec<Stack>,
    pub land: Land,
}

/// What a bake's blocks are kept as.
#[derive(Debug, Clone)]
pub enum Blocks {
    Columns(Columns),
    /// Already laid out in layers (lava, which is on hold, still bakes this way).
    Layers(LayeredBuild),
}

const GLASS: [Layer; 3] = [Layer::Glass1, Layer::Glass2, Layer::Glass3];

impl Baked {
    /// The palette the block ids refer to.
    pub fn palette(&self) -> &Palette {
        match &self.blocks {
            Blocks::Columns(c) => &c.palette,
            Blocks::Layers(b) => &b.palette,
        }
    }

    /// Every block of the bake with its layer, place and palette id. A place
    /// can come twice only in the terrain, where regions of a brought-in
    /// build overlap; the later one is the one kept.
    pub fn for_each_block(&self, mut f: impl FnMut(Layer, Pos, u32)) {
        let c = match &self.blocks {
            Blocks::Columns(c) => c,
            Blocks::Layers(b) => {
                for layer in Layer::ALL {
                    b.layer(layer).for_each(|p, id| f(layer, p, id));
                }
                return;
            }
        };
        let (w, l) = (self.width, self.length);
        let inside = |x: i32, z: i32| x >= 0 && z >= 0 && x < w && z < l;
        let at = |x: i32, z: i32| (z * w + x) as usize;
        match &c.land {
            Land::None => {}
            Land::Ground { starts, runs } => {
                for i in 0..(w * l) as usize {
                    let (x, z) = (i as i32 % w, i as i32 / w);
                    for &(from, to, id) in &runs[starts[i] as usize..starts[i + 1] as usize] {
                        for y in from..=to {
                            f(Layer::Terrain, [x, y, z], id);
                        }
                    }
                }
            }
            Land::Placed { build, ids, shift, reworked } => {
                for region in &build.regions {
                    for (corner, chunk) in region.cells.chunks() {
                        for (i, v) in chunk.iter().enumerate() {
                            let Some(id) = v.checked_sub(1).and_then(|v| ids[v as usize]) else { continue };
                            let p = [(corner[0] << 4) + (i & 15) as i32, (corner[1] << 4) + (i >> 8) as i32, (corner[2] << 4) + ((i >> 4) & 15) as i32];
                            let (x, level, z) = (p[0] + shift[0], p[1] + shift[1], p[2] + shift[2]);
                            let y = level + self.sea_level;
                            if region.contains(p) && inside(x, z) && reworked.keeps(at(x, z) as u32, y) {
                                f(Layer::Terrain, [x, y, z], id);
                            }
                        }
                    }
                }
                for &(column, _, from, to) in &reworked.cols {
                    let (x, z) = (column as i32 % w, column as i32 / w);
                    for &(lo, hi, id) in &reworked.runs[from as usize..to as usize] {
                        for y in lo..=hi {
                            f(Layer::Terrain, [x, y, z], id);
                        }
                    }
                }
            }
        }
        for z in 0..l {
            for x in 0..w {
                let i = at(x, z);
                let top = self.water_top[i];
                if top == DRY {
                    continue;
                }
                let open_from = self.open_from(c, x, z);
                let stack = c.stacks.get(c.stack[i] as usize);
                for y in self.water_bottom[i]..=top {
                    match stack {
                        Some(s) if y >= open_from => f(Layer::Surface, [x, y, z], s.solid),
                        _ => f(Layer::Foundation, [x, y, z], c.foundation),
                    }
                }
                if let Some(s) = stack {
                    for (k, glass) in s.glass.iter().enumerate() {
                        if let Some(id) = glass {
                            f(GLASS[k], [x, top + 1 + k as i32, z], *id);
                        }
                    }
                }
            }
        }
    }

    /// The lowest level of a water column that takes the coloured surface:
    /// the coating covers the top and any side that stands above a
    /// neighbour. The plot edge counts as level, so there is no skirt.
    fn open_from(&self, c: &Columns, x: i32, z: i32) -> i32 {
        let (w, l) = (self.width, self.length);
        let mut open_from = self.water_top[(z * w + x) as usize];
        for (dx, dz) in [(-1, 0), (1, 0), (0, -1), (0, 1)] {
            let (nx, nz) = (x + dx, z + dz);
            if nx >= 0 && nz >= 0 && nx < w && nz < l {
                let j = (nz * w + nx) as usize;
                open_from = open_from.min(self.water_top[j].max(c.ground[j]).saturating_add(1));
            }
        }
        open_from
    }

    /// The bake as an export reads it, kept to `area` (lowest X, lowest Z,
    /// highest X, highest Z, all included) when one is given.
    pub fn source(&self, area: Option<[i32; 4]>) -> BakeSource<'_> {
        let (w, l) = (self.width, self.length);
        let area = match area {
            Some(b) => [b[0].max(0), b[1].max(0), b[2].min(w - 1), b[3].min(l - 1)],
            None => [0, 0, w - 1, l - 1],
        };
        match &self.blocks {
            Blocks::Layers(b) => {
                let layered = if area == [0, 0, w - 1, l - 1] { Cow::Borrowed(b) } else { Cow::Owned(b.cropped([area[0], area[1]], [area[2], area[3]])) };
                BakeSource { baked: self, area, open_from: Vec::new(), layered: Some(layered), entities: Vec::new() }
            }
            Blocks::Columns(c) => {
                let mut open_from = vec![0; (w.max(0) * l.max(0)) as usize];
                for z in 0..l {
                    for x in 0..w {
                        if self.water_top[(z * w + x) as usize] != DRY {
                            open_from[(z * w + x) as usize] = self.open_from(c, x, z);
                        }
                    }
                }
                let entities = self.block_entities(c, area);
                BakeSource { baked: self, area, open_from, layered: None, entities }
            }
        }
    }

    /// The brought-in build's block entities (chest contents, sign text and
    /// the like) at their places in the plot, for the blocks that are kept.
    /// Where regions overlap, the later one's block wins, as in `for_each_block`.
    fn block_entities(&self, c: &Columns, area: [i32; 4]) -> Vec<BlockEntity> {
        let Land::Placed { build, ids, shift, reworked } = &c.land else { return Vec::new() };
        let mut out = Vec::new();
        for (k, region) in build.regions.iter().enumerate() {
            for be in &region.block_entities {
                let p = be.pos;
                let kept = region.contains(p) && region.cells.get(p).is_some_and(|v| ids[v as usize].is_some());
                let covered = build.regions[k + 1..].iter().any(|r| r.contains(p) && r.cells.get(p).is_some());
                let (x, z) = (p[0] + shift[0], p[2] + shift[2]);
                let y = p[1] + shift[1] + self.sea_level;
                let column = (z * self.width + x) as u32;
                if kept && !covered && x >= area[0] && z >= area[1] && x <= area[2] && z <= area[3] && reworked.keeps(column, y) {
                    out.push(BlockEntity { pos: [x, y, z], ..be.clone() });
                }
            }
        }
        out
    }

    /// The blocks laid out in six 3D layers, as the 3D view takes them. Made
    /// on each call from the columns; it takes far more memory than they
    /// do, so hold it only while it is used. Exports read the columns
    /// themselves (`Baked::source`).
    pub fn layers(&self) -> Cow<'_, LayeredBuild> {
        match &self.blocks {
            Blocks::Layers(b) => Cow::Borrowed(b),
            Blocks::Columns(c) => {
                let mut build = LayeredBuild::new(&c.name);
                build.palette = c.palette.clone();
                self.for_each_block(|layer, p, id| {
                    // Where the build already stands, the sea keeps out: its
                    // blocks hold their place and the water is laid around them.
                    if WATER.contains(&layer) && build.layer(Layer::Terrain).get(p).is_some() {
                        return;
                    }
                    build.layer_mut(layer).set(p, id);
                });
                Cow::Owned(build)
            }
        }
    }
}

/// The bake as an export reads it: each row of blocks worked out from the
/// columns as the writer asks for it, kept to an area of the plot.
///
/// Why: the export used to copy the chosen layers into one more 3D store,
/// on top of the six layers; the columns already say where every block is.
pub struct BakeSource<'a> {
    baked: &'a Baked,
    /// Lowest X, lowest Z, highest X, highest Z, all included; inside the plot.
    area: [i32; 4],
    /// Per water column, its lowest level that takes the coloured surface.
    open_from: Vec<i32>,
    /// A bake already laid out in layers (lava), kept to the area.
    layered: Option<Cow<'a, LayeredBuild>>,
    /// The brought-in build's block entities inside the area, at plot positions.
    entities: Vec<BlockEntity>,
}

const WATER: [Layer; 5] = [Layer::Foundation, Layer::Surface, Layer::Glass1, Layer::Glass2, Layer::Glass3];

impl BakeSource<'_> {
    fn columns(&self) -> &Columns {
        match &self.baked.blocks {
            Blocks::Columns(c) => c,
            Blocks::Layers(_) => unreachable!("a bake in layers is read through `layered`"),
        }
    }
    fn inside(&self, x: i32, z: i32) -> bool {
        x >= self.area[0] && z >= self.area[1] && x <= self.area[2] && z <= self.area[3]
    }
    /// The water block of column `i` at level `y`, with its layer.
    fn water(&self, c: &Columns, i: usize, y: i32) -> Option<(Layer, u32)> {
        let top = self.baked.water_top[i];
        if top == DRY {
            return None;
        }
        let stack = c.stacks.get(c.stack[i] as usize);
        if y >= self.baked.water_bottom[i] && y <= top {
            return Some(match stack {
                Some(s) if y >= self.open_from[i] => (Layer::Surface, s.solid),
                _ => (Layer::Foundation, c.foundation),
            });
        }
        let k = y.checked_sub(top)?.checked_sub(1)?;
        if (0..3).contains(&k) {
            return stack.and_then(|s| s.glass[k as usize]).map(|id| (GLASS[k as usize], id));
        }
        None
    }
    /// Terrain along +X from `x0` at level `y`, row `z`, into `out` (stored
    /// values). Where regions of a brought-in build overlap, the later wins,
    /// as in `for_each_block`.
    fn terrain_row(&self, c: &Columns, x0: i32, y: i32, z: i32, out: &mut [u32]) {
        let w = self.baked.width;
        match &c.land {
            Land::None => {}
            Land::Ground { starts, runs } => {
                for (k, o) in out.iter_mut().enumerate() {
                    let i = (z * w + x0 + k as i32) as usize;
                    for &(from, to, id) in &runs[starts[i] as usize..starts[i + 1] as usize] {
                        if y >= from && y <= to {
                            *o = id + 1;
                        }
                    }
                }
            }
            Land::Placed { build, ids, shift, reworked } => {
                let q = [x0 - shift[0], y - self.baked.sea_level - shift[1], z - shift[2]];
                let mut part = [0u32; 64];
                for region in &build.regions {
                    // Only the part of the row inside this region's box is read.
                    // Why: the statue has 1,989 regions, and reading the whole
                    // row from each one made its 3D view take an hour.
                    let (lo, hi) = (region.min, [0, 1, 2].map(|a| region.min[a] + region.dims[a]));
                    if q[1] < lo[1] || q[1] >= hi[1] || q[2] < lo[2] || q[2] >= hi[2] {
                        continue;
                    }
                    let from = (lo[0] - q[0]).clamp(0, out.len() as i32) as usize;
                    let to = (hi[0] - q[0]).clamp(from as i32, out.len() as i32) as usize;
                    for (n, piece) in out[from..to].chunks_mut(64).enumerate() {
                        let start = [q[0] + (from + 64 * n) as i32, q[1], q[2]];
                        region.cells.read_row(start, &mut part[..piece.len()]);
                        for (o, v) in piece.iter_mut().zip(&part) {
                            if *v == 0 {
                                continue;
                            }
                            if let Some(id) = ids[*v as usize - 1] {
                                *o = id + 1;
                            }
                        }
                    }
                }
                // Columns the rivers and ponds changed: the build cut away
                // above their level, and their water.
                if !reworked.cols.is_empty() {
                    for (k, o) in out.iter_mut().enumerate() {
                        let column = (z * w + x0 + k as i32) as u32;
                        if !reworked.keeps(column, y) {
                            *o = 0;
                        }
                        for &(from, to, id) in reworked.runs_at(column) {
                            if y >= from && y <= to {
                                *o = id + 1;
                            }
                        }
                    }
                }
            }
        }
    }
}

impl BlockSource for BakeSource<'_> {
    fn name(&self) -> &str {
        match &self.layered {
            Some(b) => &b.name,
            None => &self.columns().name,
        }
    }
    fn palette(&self) -> &Palette {
        self.baked.palette()
    }
    fn block_entities(&self) -> &[BlockEntity] {
        match &self.layered {
            Some(b) => &b.block_entities,
            None => &self.entities,
        }
    }
    fn bounds(&self, layers: &[Layer]) -> Option<(Pos, Pos)> {
        if let Some(b) = &self.layered {
            return b.bounds(layers);
        }
        let mut b: Option<(Pos, Pos)> = None;
        self.baked.for_each_block(|layer, p, _| {
            if layers.contains(&layer) && self.inside(p[0], p[2]) {
                let (lo, hi) = b.get_or_insert((p, p));
                for a in 0..3 {
                    lo[a] = lo[a].min(p[a]);
                    hi[a] = hi[a].max(p[a]);
                }
            }
        });
        b
    }
    fn read_row(&self, layers: &[Layer], start: Pos, out: &mut [u32]) {
        if let Some(b) = &self.layered {
            return b.read_row(layers, start, out);
        }
        out.fill(0);
        let [x0, y, z] = start;
        if z < self.area[1] || z > self.area[3] {
            return;
        }
        // The part of the row inside the area.
        let from = (self.area[0] - x0).clamp(0, out.len() as i32) as usize;
        let to = (self.area[2] + 1 - x0).clamp(from as i32, out.len() as i32) as usize;
        if from == to {
            return;
        }
        let c = self.columns();
        let (lo, part) = (x0 + from as i32, &mut out[from..to]);
        if layers.contains(&Layer::Terrain) {
            self.terrain_row(c, lo, y, z, part);
        }
        if WATER.iter().any(|l| layers.contains(l)) {
            let row = (z * self.baked.width + lo) as usize;
            for (k, o) in part.iter_mut().enumerate() {
                // A block the build already stands there keeps its place.
                if *o != 0 {
                    continue;
                }
                if let Some((layer, id)) = self.water(c, row + k, y) {
                    if layers.contains(&layer) {
                        *o = id + 1;
                    }
                }
            }
        }
    }
    fn conflicts(&self, layers: &[Layer]) -> u64 {
        if let Some(b) = &self.layered {
            return b.conflicts(layers);
        }
        // The water layers never share a place with each other, and where the
        // build already stands the water is not laid at all, so nothing here
        // is written twice. A block the build brought in stays as it is.
        let _ = layers;
        0
    }
}
