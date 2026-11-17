//! Sponge schematic `.schem` files, revisions 1 to 3 (WorldEdit, FAWE).
//!
//! Revisions 1 and 2 keep everything at the top level of a root tag named
//! "Schematic". Revision 3 nests it one level down and groups blocks under
//! `Blocks`. Cells are stored in the order y, z, x as variable-length
//! palette indices; every cell in the box is present.

use crate::model::{BlockEntity, BlockState, Build, Format, Meta, Note, Problem, Region};
use crate::nbt::{self, Compound, Tag};
use crate::pack::{varint_push, varint_read};
use crate::{damaged, now_millis, Progress, Rows};

pub fn looks_like(name: &str, root: &Compound) -> bool {
    let inner = root.compound("Schematic").unwrap_or(root);
    (name == "Schematic" || root.contains("Schematic")) && inner.contains("Version") && (inner.contains("Palette") || inner.contains("Blocks"))
}

fn dimension(c: &Compound, key: &str) -> Result<i32, Problem> {
    // Stored as a signed 16-bit number but meant as unsigned.
    c.get(key).and_then(Tag::as_i64).map(|v| (v as u16) as i32).ok_or_else(|| damaged(format!("no {key}")))
}

fn ints3(tag: Option<&Tag>) -> Option<[i32; 3]> {
    let v = tag?.as_ints()?;
    (v.len() == 3).then(|| [v[0], v[1], v[2]])
}

pub fn read(root: &Compound, progress: &Progress) -> Result<(Build, Vec<Note>), Problem> {
    let s = root.compound("Schematic").unwrap_or(root);
    let version = s.i32("Version").ok_or_else(|| damaged("no Version"))?;
    if !(1..=3).contains(&version) {
        return Err(Problem::new("err.unsupported_version").arg("detail", format!("Sponge schematic {version}")));
    }
    let mut build = Build::new();
    build.source = Some(Format::Sponge);
    build.source_version = Some(version);
    build.data_version = s.i32("DataVersion");

    let dims = [dimension(s, "Width")?, dimension(s, "Height")?, dimension(s, "Length")?];
    let stated_offset = ints3(s.get("Offset")).unwrap_or([0; 3]);
    let meta = s.compound("Metadata");
    // Where the box sits relative to the point it is pasted at. Revision 3
    // states this directly; WorldEdit's revision 2 files keep it in metadata
    // and use Offset for the original world position instead.
    let paste_offset = if version >= 3 {
        stated_offset
    } else {
        match meta.and_then(|m| Some([m.i32("WEOffsetX")?, m.i32("WEOffsetY")?, m.i32("WEOffsetZ")?])) {
            Some(o) => o,
            None => stated_offset,
        }
    };
    if let Some(m) = meta {
        build.meta = Meta {
            name: m.str("Name").unwrap_or("").to_string(),
            author: m.str("Author").unwrap_or("").to_string(),
            description: String::new(),
            created: m.get("Date").and_then(Tag::as_i64),
            modified: None,
        };
    }

    let blocks = if version >= 3 { s.compound("Blocks").ok_or_else(|| damaged("no Blocks"))? } else { s };
    let palette = blocks.compound("Palette").ok_or_else(|| damaged("no Palette"))?;
    let data = blocks.get(if version >= 3 { "Data" } else { "BlockData" }).and_then(Tag::as_bytes).ok_or_else(|| damaged("no block data"))?;
    let mut ids: Vec<u32> = Vec::new();
    for (text, index) in palette.iter() {
        // The index sizes the lookup table, so one read from a damaged file
        // is refused when no real palette could reach it (Minecraft has
        // fewer than 2^15 block states); an index near 2^31 once asked for 8 GB.
        let index = index.as_i32().filter(|i| (0..1 << 20).contains(i)).ok_or_else(|| damaged("bad palette index"))? as usize;
        let state = BlockState::parse(text).ok_or_else(|| damaged(format!("unreadable block '{text}'")))?;
        if ids.len() <= index {
            ids.resize(index + 1, 0);
        }
        ids[index] = build.palette.intern(state) + 1;
    }

    let mut region = Region::new("", paste_offset, dims);
    let mut cells = crate::model::Volume::new();
    let [sx, sy, sz] = dims;
    let mut row = vec![0u32; sx as usize];
    let mut at = 0usize;
    for y in 0..sy {
        for z in 0..sz {
            for cell in row.iter_mut() {
                let (v, next) = varint_read(data, at).ok_or_else(|| damaged("block data is too short"))?;
                at = next;
                *cell = ids.get(v as usize).copied().filter(|id| *id != 0).ok_or_else(|| damaged("block outside the palette"))?;
            }
            cells.write_row([paste_offset[0], paste_offset[1] + y, paste_offset[2] + z], &row);
        }
        let world_y = paste_offset[1] + y;
        if world_y & 15 == 15 || y == sy - 1 {
            cells.share_uniform(Some(world_y >> 4));
        }
        progress.step((y + 1) as f32 / sy.max(1) as f32)?;
    }

    let list_key = match version {
        1 => "TileEntities",
        _ => "BlockEntities",
    };
    for be in blocks.list(list_key).unwrap_or(&[]) {
        let c = be.as_compound().ok_or_else(|| damaged("unreadable block entity"))?;
        let local = ints3(c.get("Pos")).ok_or_else(|| damaged("block entity without a position"))?;
        let id = c.str("Id").map(str::to_string);
        let data = if version >= 3 {
            c.compound("Data").map(|d| d.without(&["id", "x", "y", "z"])).unwrap_or_default()
        } else {
            c.without(&["Pos", "Id"])
        };
        region.block_entities.push(BlockEntity {
            pos: [paste_offset[0] + local[0], paste_offset[1] + local[1], paste_offset[2] + local[2]],
            id,
            data,
        });
    }
    region.cells = std::sync::Arc::new(cells);
    region.entities = s.list("Entities").unwrap_or(&[]).to_vec();

    // Everything else (biomes, metadata details) is kept for a same-format export.
    let mut known = vec!["Version", "DataVersion", "Width", "Height", "Length", "Offset", "Entities"];
    if version >= 3 {
        known.push("Blocks");
    } else {
        known.extend(["Palette", "PaletteMax", "BlockData", "TileEntities", "BlockEntities"]);
    }
    build.extra = s.without(&known);
    build.regions.push(region);
    Ok((build, Vec::new()))
}

/// Write revision 3, the form current WorldEdit versions read and write.
/// A build with several regions is written as one box; cells no region
/// covers become air.
/// Write a Sponge schematic (version 3) straight into the compressor. With
/// `rows` set, the cells are read from it (see `write_from`). A first pass
/// finds the palette and how long the block data will be; the second writes
/// the block data as it reads the cells.
pub fn write(build: &Build, rows: Option<&dyn Rows>, fallback_data_version: i32, progress: &Progress) -> Result<(Vec<u8>, Vec<Note>), Problem> {
    let mut notes = Vec::new();
    let (lo, hi) = build.bounds().ok_or_else(|| Problem::new("err.empty"))?;
    let dims = [hi[0] - lo[0] + 1, hi[1] - lo[1] + 1, hi[2] - lo[2] + 1];
    if dims.iter().any(|d| *d > u16::MAX as i32) {
        return Err(Problem::new("err.too_large").arg("detail", format!("{} x {} x {}", dims[0], dims[1], dims[2])));
    }
    if build.regions.len() > 1 {
        notes.push(Note::new("note.regions_merged").arg("count", build.regions.len()));
    }
    let same = build.source == Some(Format::Sponge) && build.source_version == Some(3);
    if build.data_version.is_none() {
        notes.push(Note::new("note.no_data_version").arg("version", fallback_data_version));
    }

    // One row of the box, its regions merged (later regions win) unless
    // the cells come from one source.
    let [sx, sy, sz] = dims;
    let mut row = vec![0u32; sx as usize];
    let mut merged = vec![0u32; sx as usize];
    let mut read = |y: i32, z: i32, f: &mut dyn FnMut(&[u32])| {
        let start = [lo[0], lo[1] + y, lo[2] + z];
        if let Some(rows) = rows {
            rows.read_row(start, &mut row);
            f(&row);
        } else if build.regions.len() == 1 {
            build.regions[0].cells.read_row(start, &mut row);
            f(&row);
        } else {
            merged.fill(0);
            for region in &build.regions {
                region.cells.read_row(start, &mut row);
                for (m, v) in merged.iter_mut().zip(&row) {
                    if *v != 0 {
                        *m = *v;
                    }
                }
            }
            f(&merged);
        }
    };
    let step = |y: i32, half: i32| progress.step(((y + 1) as f32 / sy.max(1) as f32 + half as f32) / 2.0);

    // Pass 1: the palette, air first, then states in order of first use;
    // and the length of the block data.
    let air = BlockState::air();
    let mut local: Vec<u32> = vec![u32::MAX; build.palette.len()];
    let mut states: Vec<String> = vec![air.to_string()];
    if let Some(id) = build.palette.id_of(&air) {
        local[id as usize] = 0;
    }
    let mut length = 0usize;
    let mut scratch = Vec::with_capacity(5);
    for y in 0..sy {
        for z in 0..sz {
            read(y, z, &mut |cells| {
                for v in cells {
                    if *v == 0 {
                        length += 1;
                        continue;
                    }
                    let id = (*v - 1) as usize;
                    if local[id] == u32::MAX {
                        local[id] = states.len() as u32;
                        states.push(build.palette.get(id as u32).to_string());
                    }
                    scratch.clear();
                    varint_push(&mut scratch, local[id]);
                    length += scratch.len();
                }
            });
        }
        step(y, 0)?;
    }

    let palette = Compound(states.into_iter().enumerate().map(|(i, s)| (s, Tag::Int(i as i32))).collect());
    let block_entities: Vec<Tag> = build
        .regions
        .iter()
        .flat_map(|r| &r.block_entities)
        .map(|be| {
            let mut c = Compound::new().with("Pos", Tag::IntArray(vec![be.pos[0] - lo[0], be.pos[1] - lo[1], be.pos[2] - lo[2]]));
            if let Some(id) = &be.id {
                c.set("Id", Tag::string(id));
            }
            c.set("Data", Tag::Compound(be.data.clone()));
            Tag::Compound(c)
        })
        .collect();
    // The block data stands in this tree as an empty array; it is written
    // in its place as the cells are read.
    let blocks = Compound::new()
        .with("Palette", Tag::Compound(palette))
        .with("Data", Tag::ByteArray(Vec::new()))
        .with("BlockEntities", Tag::List(nbt::List { elem: 10, items: block_entities }));

    let mut s = Compound::new()
        .with("Version", Tag::Int(3))
        .with("DataVersion", Tag::Int(build.data_version.unwrap_or(fallback_data_version)))
        .with("Width", Tag::Short(sx as u16 as i16))
        .with("Height", Tag::Short(sy as u16 as i16))
        .with("Length", Tag::Short(sz as u16 as i16))
        .with("Offset", Tag::IntArray(lo.to_vec()))
        .with("Blocks", Tag::Compound(blocks));
    let entity_count = build.entity_count();
    if same {
        for (k, v) in build.extra.iter() {
            s.set(k, v.clone());
        }
        if entity_count > 0 {
            s.set("Entities", Tag::list(build.regions.iter().flat_map(|r| r.entities.iter().cloned()).collect()));
        }
    } else {
        if entity_count > 0 {
            notes.push(Note::new("note.entities_dropped").arg("count", entity_count));
        }
        if build.source == Some(Format::Sponge) && !build.extra.0.is_empty() {
            notes.push(Note::new("note.extras_dropped"));
        }
        let mut meta = Compound::new().with("Date", Tag::Long(build.meta.created.unwrap_or_else(now_millis)));
        if !build.meta.name.is_empty() {
            meta.set("Name", Tag::string(&build.meta.name));
        }
        if !build.meta.author.is_empty() {
            meta.set("Author", Tag::string(&build.meta.author));
        }
        s.set("Metadata", Tag::Compound(meta));
    }

    // Pass 2: the file, the block data written as the cells are read again.
    let mut out = nbt::Stream::new("");
    out.begin_compound("Schematic");
    for (k, v) in s.iter() {
        match (k, v) {
            ("Blocks", Tag::Compound(blocks)) => {
                out.begin_compound("Blocks");
                for (k, v) in blocks.iter() {
                    if k != "Data" {
                        out.entry(k, v);
                        continue;
                    }
                    out.begin_byte_array("Data", length);
                    let mut data = Vec::with_capacity(sx as usize * 2);
                    for y in 0..sy {
                        for z in 0..sz {
                            data.clear();
                            read(y, z, &mut |cells| {
                                for v in cells {
                                    if *v == 0 {
                                        data.push(0);
                                    } else {
                                        varint_push(&mut data, local[(*v - 1) as usize]);
                                    }
                                }
                            });
                            out.bytes(&data);
                        }
                        step(y, 1)?;
                    }
                }
                out.end();
            }
            _ => out.entry(k, v),
        }
    }
    out.end();
    out.end();
    Ok((out.finish(), notes))
}
