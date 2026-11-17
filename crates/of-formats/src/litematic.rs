//! Litematica `.litematic` files.
//!
//! Layout: a gzip NBT file with `Version`, `MinecraftDataVersion`,
//! `Metadata` and `Regions`. Each region has a `Position` corner and a
//! `Size` that may be negative on any axis, a palette, and block indices
//! packed tightly in the order y, z, x from the region's smallest corner.

use crate::model::{BlockEntity, BlockState, Build, Format, Meta, Note, Pos, Problem, Region};
use crate::nbt::{self, Compound, Tag};
use crate::pack::{bits_for, tight_get, tight_len, TightStream};
use crate::{damaged, now_millis, xyz, xyz_tag, Progress, Rows};
use std::collections::HashSet;

const ROOT_KNOWN: [&str; 4] = ["Version", "MinecraftDataVersion", "Metadata", "Regions"];
const META_KNOWN: [&str; 9] =
    ["Name", "Author", "Description", "RegionCount", "TotalVolume", "TotalBlocks", "TimeCreated", "TimeModified", "EnclosingSize"];
const REGION_KNOWN: [&str; 6] = ["Position", "Size", "BlockStatePalette", "BlockStates", "TileEntities", "Entities"];

pub fn looks_like(root: &Compound) -> bool {
    root.contains("Regions") && (root.contains("Version") || root.contains("MinecraftDataVersion"))
}

pub fn read(root: &Compound, progress: &Progress) -> Result<(Build, Vec<Note>), Problem> {
    let mut build = Build::new();
    build.source = Some(Format::Litematic);
    build.source_version = root.i32("Version");
    build.data_version = root.i32("MinecraftDataVersion");
    build.extra = root.without(&ROOT_KNOWN);
    if let Some(meta) = root.compound("Metadata") {
        build.meta = Meta {
            name: meta.str("Name").unwrap_or("").to_string(),
            author: meta.str("Author").unwrap_or("").to_string(),
            description: meta.str("Description").unwrap_or("").to_string(),
            created: meta.get("TimeCreated").and_then(Tag::as_i64),
            modified: meta.get("TimeModified").and_then(Tag::as_i64),
        };
        let extra = meta.without(&META_KNOWN);
        if !extra.0.is_empty() {
            build.extra.set("Metadata", Tag::Compound(extra));
        }
    }
    let regions = root.compound("Regions").ok_or_else(|| damaged("no Regions"))?;
    let total: u64 = regions
        .iter()
        .filter_map(|(_, r)| r.as_compound().and_then(|r| r.compound("Size")).and_then(xyz))
        .map(|s| s.iter().map(|d| d.unsigned_abs() as u64).product::<u64>())
        .sum();
    let mut done = 0u64;

    for (name, tag) in regions.iter() {
        let r = tag.as_compound().ok_or_else(|| damaged(format!("region '{name}' is not a compound")))?;
        let position = r.compound("Position").and_then(xyz).ok_or_else(|| damaged(format!("region '{name}' has no Position")))?;
        let size = r.compound("Size").and_then(xyz).ok_or_else(|| damaged(format!("region '{name}' has no Size")))?;
        let mut region = Region::new(name, [0; 3], [0; 3]);
        for a in 0..3 {
            region.dims[a] = size[a].checked_abs().ok_or_else(|| damaged("region size out of range"))?;
            region.flipped[a] = size[a] < 0;
            // A negative size means Position is the far corner on that axis.
            region.min[a] = if size[a] < 0 { position[a].wrapping_add(size[a] + 1) } else { position[a] };
        }
        let volume = region.volume();
        let mut cells = crate::model::Volume::new();

        let palette = r.list("BlockStatePalette").unwrap_or(&[]);
        let mut ids = Vec::with_capacity(palette.len());
        for entry in palette {
            let state = entry.as_compound().and_then(BlockState::from_nbt).ok_or_else(|| damaged("unreadable palette entry"))?;
            ids.push(build.palette.intern(state) + 1);
        }
        if volume > 0 {
            let longs = r.get("BlockStates").and_then(Tag::as_longs).ok_or_else(|| damaged(format!("region '{name}' has no BlockStates")))?;
            let bits = bits_for(ids.len(), 2);
            if longs.len() < tight_len(volume, bits) {
                return Err(damaged(format!("region '{name}' block data is too short")));
            }
            let [sx, sy, sz] = region.dims;
            let mut row = vec![0u32; sx as usize];
            let mut index = 0u64;
            for y in 0..sy {
                for z in 0..sz {
                    for cell in row.iter_mut() {
                        let v = tight_get(longs, bits, index) as usize;
                        *cell = *ids.get(v).ok_or_else(|| damaged(format!("region '{name}' uses a block outside its palette")))?;
                        index += 1;
                    }
                    cells.write_row([region.min[0], region.min[1] + y, region.min[2] + z], &row);
                }
                let world_y = region.min[1] + y;
                if world_y & 15 == 15 || y == sy - 1 {
                    cells.share_uniform(Some(world_y >> 4));
                }
                done += (sx as u64) * (sz as u64);
                progress.step(done as f32 / total.max(1) as f32)?;
            }
        }

        for te in r.list("TileEntities").unwrap_or(&[]) {
            let c = te.as_compound().ok_or_else(|| damaged("unreadable block entity"))?;
            let local = xyz(c).ok_or_else(|| damaged("block entity without a position"))?;
            region.block_entities.push(BlockEntity {
                pos: [region.min[0] + local[0], region.min[1] + local[1], region.min[2] + local[2]],
                id: c.str("id").map(str::to_string),
                data: c.without(&["x", "y", "z", "id"]),
            });
        }
        region.cells = std::sync::Arc::new(cells);
        region.entities = r.list("Entities").unwrap_or(&[]).to_vec();
        region.extra = r.without(&REGION_KNOWN);
        build.regions.push(region);
    }
    Ok((build, Vec::new()))
}

/// The file revision Litematica itself writes for a given game data version.
pub fn file_version_for(data_version: i32) -> i32 {
    // Revision 7 arrived with the item-component change in 1.20.5 (data version 3837).
    if data_version >= 3837 {
        7
    } else if data_version >= 2975 {
        6
    } else {
        5
    }
}

/// Write a Litematica file region by region, straight into the compressor.
/// With `rows` set, every region's cells are read from it (see `write_from`).
/// A first pass over the cells finds each region's palette and the block
/// count the header states; the second writes the packed cells as it reads them.
pub fn write(build: &Build, rows: Option<&dyn Rows>, fallback_data_version: i32, progress: &Progress) -> Result<(Vec<u8>, Vec<Note>), Problem> {
    let mut notes = Vec::new();
    let same_format = build.source == Some(Format::Litematic);
    let data_version = build.data_version.unwrap_or(fallback_data_version);
    let air = BlockState::air();
    let air_id = build.palette.id_of(&air);
    let total: u64 = build.regions.iter().map(Region::volume).sum();
    let is_air: Vec<bool> = build.palette.states().iter().map(BlockState::is_air).collect();
    let step = |done: u64, half: u64| progress.step((done as f32 / total.max(1) as f32 + half as f32) / 2.0);

    // Pass 1: each region's palette (air first, as Litematica expects, then
    // states in order of first use) and the blocks that are not air.
    let (mut done, mut total_blocks) = (0u64, 0u64);
    let mut local: Vec<u32> = vec![u32::MAX; build.palette.len()];
    let mut used: Vec<Vec<u32>> = Vec::with_capacity(build.regions.len());
    for region in &build.regions {
        let [sx, sy, sz] = region.dims;
        if region.volume() > i32::MAX as u64 * 16 {
            return Err(Problem::new("err.too_large").arg("detail", format!("{sx} x {sy} x {sz}")));
        }
        let cells: &dyn Rows = match rows {
            Some(r) => r,
            None => &*region.cells,
        };
        let mut ids = Vec::new();
        let mut row = vec![0u32; sx as usize];
        for y in 0..sy {
            for z in 0..sz {
                cells.read_row([region.min[0], region.min[1] + y, region.min[2] + z], &mut row);
                for v in row.iter().filter(|v| **v != 0) {
                    let id = *v - 1;
                    if !is_air[id as usize] {
                        total_blocks += 1;
                    }
                    if local[id as usize] == u32::MAX && Some(id) != air_id {
                        local[id as usize] = 0;
                        ids.push(id);
                    }
                }
            }
            done += (sx as u64) * (sz as u64);
            step(done, 0)?;
        }
        for id in &ids {
            local[*id as usize] = u32::MAX;
        }
        used.push(ids);
    }

    // The root, all but the regions, which follow it.
    let now = now_millis();
    let enclosing = build.bounds().map(|(lo, hi)| [hi[0] - lo[0] + 1, hi[1] - lo[1] + 1, hi[2] - lo[2] + 1]).unwrap_or([0; 3]);
    let mut meta = Compound::new()
        .with("Name", Tag::string(if build.meta.name.is_empty() { "Unnamed" } else { &build.meta.name }))
        .with("Author", Tag::string(&build.meta.author))
        .with("Description", Tag::string(&build.meta.description))
        .with("RegionCount", Tag::Int(build.regions.len() as i32))
        .with("TotalVolume", Tag::Int(total.min(i32::MAX as u64) as i32))
        .with("TotalBlocks", Tag::Int(total_blocks.min(i32::MAX as u64) as i32))
        .with("TimeCreated", Tag::Long(build.meta.created.unwrap_or(now)))
        .with("TimeModified", Tag::Long(build.meta.modified.unwrap_or(now)))
        .with("EnclosingSize", xyz_tag(enclosing));
    let mut root = Compound::new()
        .with("MinecraftDataVersion", Tag::Int(data_version))
        .with("Version", Tag::Int(if same_format { build.source_version.unwrap_or_else(|| file_version_for(data_version)) } else { file_version_for(data_version) }));
    if same_format {
        for (k, v) in build.extra.iter() {
            match (k, v) {
                ("Metadata", Tag::Compound(extra)) => extra.iter().for_each(|(k, v)| meta.set(k, v.clone())),
                _ => root.set(k, v.clone()),
            }
        }
    }
    if !root.contains("SubVersion") {
        root.set("SubVersion", Tag::Int(1));
    }
    root.set("Metadata", Tag::Compound(meta));
    root.remove("Regions");
    let mut out = nbt::Stream::new("");
    for (k, v) in root.iter() {
        out.entry(k, v);
    }

    // Pass 2: the regions, each region's cells packed as they are read.
    out.begin_compound("Regions");
    let mut used_names = HashSet::new();
    done = 0;
    for (region, ids) in build.regions.iter().zip(&used) {
        let [sx, sy, sz] = region.dims;
        let cells: &dyn Rows = match rows {
            Some(r) => r,
            None => &*region.cells,
        };
        if let Some(id) = air_id {
            local[id as usize] = 0;
        }
        for (k, id) in ids.iter().enumerate() {
            local[*id as usize] = k as u32 + 1;
        }
        let states: Vec<Tag> = std::iter::once(&air).chain(ids.iter().map(|id| build.palette.get(*id))).map(|s| s.to_nbt()).collect();
        let bits = bits_for(states.len(), 2);

        let (mut position, mut size) = (region.min, region.dims);
        for a in 0..3 {
            if region.flipped[a] {
                position[a] = region.min[a] + region.dims[a] - 1;
                size[a] = -region.dims[a];
            }
        }
        let tiles = region
            .block_entities
            .iter()
            .map(|be| {
                let mut c = be.data.clone();
                if let Some(id) = &be.id {
                    c.set("id", Tag::string(id));
                }
                c.set("x", Tag::Int(be.pos[0] - region.min[0]));
                c.set("y", Tag::Int(be.pos[1] - region.min[1]));
                c.set("z", Tag::Int(be.pos[2] - region.min[2]));
                Tag::Compound(c)
            })
            .collect();
        let entities = if same_format {
            region.entities.clone()
        } else {
            if !region.entities.is_empty() {
                notes.push(Note::new("note.entities_dropped").arg("count", region.entities.len()));
            }
            Vec::new()
        };
        // The packed cells stand in this tree as an empty array; they are
        // written in its place as they are read.
        let mut tree = Compound::new()
            .with("Position", xyz_tag(position))
            .with("Size", xyz_tag(size))
            .with("BlockStatePalette", Tag::list(states))
            .with("BlockStates", Tag::LongArray(Vec::new()))
            .with("TileEntities", Tag::List(nbt::List { elem: 10, items: tiles }))
            .with("Entities", Tag::List(nbt::List { elem: 10, items: entities }));
        if same_format {
            for (k, v) in region.extra.iter() {
                tree.set(k, v.clone());
            }
        }
        for key in ["PendingBlockTicks", "PendingFluidTicks"] {
            if !tree.contains(key) {
                tree.set(key, Tag::List(nbt::List { elem: 10, items: vec![] }));
            }
        }
        // Region names are the keys of a compound, so they must be unique.
        let mut name = if region.name.is_empty() { "Unnamed".to_string() } else { region.name.clone() };
        let mut n = 2;
        while !used_names.insert(name.clone()) {
            name = format!("{} {n}", region.name);
            n += 1;
        }
        out.begin_compound(&name);
        for (k, v) in tree.iter() {
            if k != "BlockStates" {
                out.entry(k, v);
                continue;
            }
            out.begin_long_array("BlockStates", tight_len(region.volume(), bits));
            let mut packer = TightStream::new(bits);
            let mut row = vec![0u32; sx as usize];
            for y in 0..sy {
                for z in 0..sz {
                    cells.read_row([region.min[0], region.min[1] + y, region.min[2] + z], &mut row);
                    for v in &row {
                        let value = if *v == 0 { 0 } else { local[(*v - 1) as usize] };
                        packer.push(value, &mut |w| out.long(w));
                    }
                }
                done += (sx as u64) * (sz as u64);
                step(done, 1)?;
            }
            packer.finish(&mut |w| out.long(w));
        }
        out.end();
        for id in ids {
            local[*id as usize] = u32::MAX;
        }
        if let Some(id) = air_id {
            local[id as usize] = u32::MAX;
        }
    }
    out.end();
    out.end();
    Ok((out.finish(), notes))
}

/// Used by tests and the sample generator: positions of a region's corners
/// as Litematica would state them.
pub fn stated_corner(region: &Region) -> (Pos, Pos) {
    let (mut position, mut size) = (region.min, region.dims);
    for a in 0..3 {
        if region.flipped[a] {
            position[a] = region.min[a] + region.dims[a] - 1;
            size[a] = -region.dims[a];
        }
    }
    (position, size)
}
