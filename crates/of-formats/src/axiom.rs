//! Axiom blueprint `.bp` files.
//!
//! Layout (confirmed against blueprints saved by Axiom itself):
//!   4 bytes   magic 0A E5 BB 36
//!   int + NBT header (uncompressed): Name, Author, Tags, Version,
//!             BlockCount, ContainsAir, thumbnail camera angles
//!   int + PNG thumbnail
//!   int + gzip NBT: DataVersion, BlockRegion, BlockEntities, Entities
//! `BlockRegion` is a list of 16-block cubes, each with its cube position
//! and a palette plus indices packed the way the game packs chunk sections.
//! `minecraft:structure_void` marks a position the blueprint leaves alone.

use crate::model::{BlockEntity, BlockState, Build, Format, Meta, Note, Pos, Problem, Region, Volume};
use crate::nbt::{self, Compound, Tag};
use crate::pack::{bits_for, padded_get, padded_len, padded_pack};
use crate::{damaged, xyz, Progress, Rows};
use std::io::Write;

pub const MAGIC: [u8; 4] = [0x0A, 0xE5, 0xBB, 0x36];
const VOID: &str = "minecraft:structure_void";
const CUBE: usize = 4096;

pub fn looks_like(data: &[u8]) -> bool {
    data.starts_with(&MAGIC)
}

fn section<'a>(data: &'a [u8], at: &mut usize) -> Result<&'a [u8], Problem> {
    let len = data.get(*at..*at + 4).map(|b| u32::from_be_bytes(b.try_into().unwrap()) as usize).ok_or_else(|| damaged("file ends early"))?;
    let body = data.get(*at + 4..*at + 4 + len).ok_or_else(|| damaged("file ends early"))?;
    *at += 4 + len;
    Ok(body)
}

pub fn read(data: &[u8], progress: &Progress) -> Result<(Build, Vec<Note>), Problem> {
    let mut at = 4;
    let (_, header, _) = nbt::read_raw(section(data, &mut at)?).map_err(|e| damaged(e.to_string()))?;
    let thumbnail = section(data, &mut at)?;
    let (_, body) = nbt::read(section(data, &mut at)?).map_err(|e| {
        if e.0 == nbt::OVER_LIMIT {
            Problem::new(crate::MEMORY).arg("limit", of_core::memory::mb(of_core::memory::limit()))
        } else {
            damaged(e.to_string())
        }
    })?;

    let mut build = Build::new();
    build.source = Some(Format::Axiom);
    build.source_version = header.get("Version").and_then(Tag::as_i32);
    build.data_version = body.i32("DataVersion");
    build.meta = Meta {
        name: header.str("Name").unwrap_or("").to_string(),
        author: header.str("Author").unwrap_or("").to_string(),
        ..Meta::default()
    };
    build.extra = body.without(&["DataVersion", "BlockRegion", "BlockEntities", "Entities"]);
    build.extra.set("Header", Tag::Compound(header));
    build.extra.set("Thumbnail", Tag::ByteArray(thumbnail.to_vec()));

    let mut region = Region::new("", [0; 3], [0; 3]);
    let mut volume = Volume::new();
    let cubes = body.list("BlockRegion").unwrap_or(&[]);
    for (n, cube) in cubes.iter().enumerate() {
        let c = cube.as_compound().ok_or_else(|| damaged("unreadable block cube"))?;
        let (cx, cy, cz) = match (c.i32("X"), c.i32("Y"), c.i32("Z")) {
            (Some(x), Some(y), Some(z)) => (x, y, z),
            _ => return Err(damaged("block cube without a position")),
        };
        let states = c.compound("BlockStates").ok_or_else(|| damaged("block cube without BlockStates"))?;
        let palette = states.list("palette").ok_or_else(|| damaged("block cube without a palette"))?;
        // 0 marks structure void, which stays absent.
        let mut ids = Vec::with_capacity(palette.len());
        for entry in palette {
            let state = entry.as_compound().and_then(BlockState::from_nbt).ok_or_else(|| damaged("unreadable palette entry"))?;
            ids.push(if state.name == VOID { 0 } else { build.palette.intern(state) + 1 });
        }
        if ids.is_empty() {
            return Err(damaged("block cube with an empty palette"));
        }
        let mut cells = [ids[0]; CUBE];
        if let Some(longs) = states.get("data").and_then(Tag::as_longs) {
            let mut bits = bits_for(ids.len(), 4);
            if longs.len() != padded_len(CUBE, bits) {
                bits = (bits..=32).find(|b| padded_len(CUBE, *b) == longs.len()).ok_or_else(|| damaged("block cube data has an unexpected length"))?;
            }
            for (i, cell) in cells.iter_mut().enumerate() {
                *cell = *ids.get(padded_get(longs, bits, i) as usize).ok_or_else(|| damaged("block outside the palette"))?;
            }
        } else if ids.len() > 1 {
            return Err(damaged("block cube without data"));
        }
        // The game's cell order inside a cube is y, z, x, the same as rows here.
        for (row_index, row) in cells.chunks_exact(16).enumerate() {
            let (y, z) = ((row_index >> 4) as i32, (row_index & 15) as i32);
            volume.write_row([cx << 4, (cy << 4) + y, (cz << 4) + z], row);
        }
        progress.step((n + 1) as f32 / cubes.len() as f32)?;
    }
    volume.share_uniform(None);
    if let Some((lo, hi)) = volume.bounds() {
        region.min = lo;
        region.dims = [hi[0] - lo[0] + 1, hi[1] - lo[1] + 1, hi[2] - lo[2] + 1];
    }
    for be in body.list("BlockEntities").unwrap_or(&[]) {
        let c = be.as_compound().ok_or_else(|| damaged("unreadable block entity"))?;
        region.block_entities.push(BlockEntity {
            pos: xyz(c).ok_or_else(|| damaged("block entity without a position"))?,
            id: c.str("id").map(str::to_string),
            data: c.without(&["x", "y", "z", "id"]),
        });
    }
    region.cells = std::sync::Arc::new(volume);
    region.entities = body.list("Entities").unwrap_or(&[]).to_vec();
    build.regions.push(region);
    Ok((build, Vec::new()))
}

/// Write an Axiom blueprint, its block data straight into the compressor
/// one 16-block cube at a time. With `rows` set, the cells are read from it
/// (see `write_from`). A first pass finds the cubes that hold anything and
/// the counts the header states; the second writes each cube as it reads it.
pub fn write(build: &Build, rows: Option<&dyn Rows>, fallback_data_version: i32, progress: &Progress) -> Result<(Vec<u8>, Vec<Note>), Problem> {
    let mut notes = Vec::new();
    let same = build.source == Some(Format::Axiom);
    let merged;
    // Where the cells come from, and the cubes to look at, in file order.
    let (cells, mut cubes): (&dyn Rows, Vec<Pos>) = match rows {
        Some(rows) => {
            let mut all = Vec::new();
            if let Some((lo, hi)) = build.bounds() {
                for y in lo[1] >> 4..=hi[1] >> 4 {
                    for z in lo[2] >> 4..=hi[2] >> 4 {
                        for x in lo[0] >> 4..=hi[0] >> 4 {
                            all.push([x, y, z]);
                        }
                    }
                }
            }
            (rows, all)
        }
        None => {
            let shared = build.regions.windows(2).all(|w| std::sync::Arc::ptr_eq(&w[0].cells, &w[1].cells));
            let volume: &Volume = if shared && !build.regions.is_empty() {
                &build.regions[0].cells
            } else {
                let mut v = Volume::new();
                build.regions.iter().for_each(|r| v.overlay(&r.cells));
                merged = v;
                &merged
            };
            let all = volume.chunks().filter(|(_, c)| c.iter().any(|v| *v != 0)).map(|(p, _)| p).collect();
            (volume, all)
        }
    };
    cubes.sort_by_key(|p| (p[1], p[2], p[0]));
    let fill = |pos: Pos, out: &mut [u32; CUBE]| {
        for (n, row) in out.chunks_mut(16).enumerate() {
            cells.read_row([pos[0] << 4, (pos[1] << 4) + (n >> 4) as i32, (pos[2] << 4) + (n & 15) as i32], row);
        }
    };
    let is_air: Vec<bool> = build.palette.states().iter().map(BlockState::is_air).collect();
    let void = BlockState::new(VOID).to_nbt();
    let step = |n: usize, half: usize| progress.step(((n + 1) as f32 / cubes.len().max(1) as f32 + half as f32) / 2.0);

    // Pass 1: the cubes that hold anything, the blocks and whether there is air.
    let (mut block_count, mut contains_air) = (0u64, false);
    let mut stored = [0u32; CUBE];
    let mut kept = Vec::with_capacity(cubes.len());
    for (n, pos) in cubes.iter().enumerate() {
        fill(*pos, &mut stored);
        if stored.iter().all(|v| *v == 0) {
            continue;
        }
        for v in stored.iter().filter(|v| **v != 0) {
            if is_air[(*v - 1) as usize] {
                contains_air = true;
            } else {
                block_count += 1;
            }
        }
        kept.push(*pos);
        if n % 64 == 63 {
            step(n, 0)?;
        }
    }
    let cubes = kept;

    let block_entities: Vec<Tag> = build
        .regions
        .iter()
        .flat_map(|r| &r.block_entities)
        .map(|be| {
            let mut c = be.data.clone();
            c.set("x", Tag::Int(be.pos[0]));
            c.set("y", Tag::Int(be.pos[1]));
            c.set("z", Tag::Int(be.pos[2]));
            if let Some(id) = &be.id {
                c.set("id", Tag::string(id));
            }
            Tag::Compound(c)
        })
        .collect();
    let entity_count = build.entity_count();
    let entities = if same {
        build.regions.iter().flat_map(|r| r.entities.iter().cloned()).collect()
    } else {
        if entity_count > 0 {
            notes.push(Note::new("note.entities_dropped").arg("count", entity_count));
        }
        Vec::new()
    };
    // The cubes stand in this tree as an empty list; they are written in
    // its place as they are read.
    let mut body = Compound::new()
        .with("DataVersion", Tag::Int(build.data_version.unwrap_or(fallback_data_version)))
        .with("BlockRegion", Tag::List(nbt::List { elem: 10, items: Vec::new() }))
        .with("BlockEntities", Tag::List(nbt::List { elem: 10, items: block_entities }))
        .with("Entities", Tag::List(nbt::List { elem: 10, items: entities }));

    let mut header = Compound::new()
        .with("ThumbnailYaw", Tag::Float(135.0))
        .with("ThumbnailPitch", Tag::Float(30.0))
        .with("LockedThumbnail", Tag::Byte(0))
        .with("Version", Tag::Int(2))
        .with("Tags", Tag::List(nbt::List { elem: 8, items: vec![] }))
        .with("Name", Tag::string(if build.meta.name.is_empty() { "Ocean Foundry" } else { &build.meta.name }))
        .with("Author", Tag::string(&build.meta.author));
    let mut thumbnail = None;
    if same {
        for (k, v) in build.extra.iter() {
            match (k, v) {
                ("Header", Tag::Compound(h)) => header = h.clone(),
                ("Thumbnail", Tag::ByteArray(png)) => thumbnail = Some(png.clone()),
                _ => body.set(k, v.clone()),
            }
        }
    }
    header.set("BlockCount", Tag::Int(block_count.min(i32::MAX as u64) as i32));
    header.set("ContainsAir", Tag::Byte(contains_air as i8));
    let thumbnail = thumbnail.unwrap_or_else(placeholder_thumbnail);

    // Pass 2: the body, each cube written as it is read again.
    let mut out = nbt::Stream::new("");
    let mut local: Vec<u32> = vec![u32::MAX; build.palette.len()];
    for (k, v) in body.iter() {
        if k != "BlockRegion" {
            out.entry(k, v);
            continue;
        }
        out.begin_list("BlockRegion", 10, cubes.len());
        for (n, pos) in cubes.iter().enumerate() {
            fill(*pos, &mut stored);
            local.fill(u32::MAX);
            let mut palette: Vec<Tag> = Vec::new();
            let mut void_index = u32::MAX;
            let mut indices = [0u32; CUBE];
            for (i, v) in stored.iter().enumerate() {
                indices[i] = if *v == 0 {
                    if void_index == u32::MAX {
                        void_index = palette.len() as u32;
                        palette.push(void.clone());
                    }
                    void_index
                } else {
                    let id = (*v - 1) as usize;
                    if local[id] == u32::MAX {
                        local[id] = palette.len() as u32;
                        palette.push(build.palette.get(id as u32).to_nbt());
                    }
                    local[id]
                };
            }
            let mut states = Compound::new();
            if palette.len() > 1 {
                states.set("data", Tag::LongArray(padded_pack(&indices, bits_for(palette.len(), 4))));
            }
            states.set("palette", Tag::list(palette));
            out.item(&Tag::Compound(
                Compound::new().with("BlockStates", Tag::Compound(states)).with("X", Tag::Int(pos[0])).with("Y", Tag::Int(pos[1])).with("Z", Tag::Int(pos[2])),
            ));
            if n % 64 == 63 || n + 1 == cubes.len() {
                step(n, 1)?;
            }
        }
    }
    out.end();

    let header = nbt::write_raw("", &header);
    let body = out.finish();
    let mut file = Vec::with_capacity(16 + header.len() + thumbnail.len() + body.len());
    file.extend_from_slice(&MAGIC);
    for part in [&header, &thumbnail, &body] {
        file.extend_from_slice(&(part.len() as u32).to_be_bytes());
        file.extend_from_slice(part);
    }
    Ok((file, notes))
}

/// A plain 64x64 picture, so the blueprint has something to show in Axiom's
/// browser until Axiom regenerates it.
fn placeholder_thumbnail() -> Vec<u8> {
    const N: u32 = 64;
    let mut raw = Vec::with_capacity((N * (N * 4 + 1)) as usize);
    for y in 0..N {
        raw.push(0); // no row filter
        for x in 0..N {
            let wave = ((x as f32 * 0.3).sin() * 3.0 + y as f32) as u32 % 16 < 8;
            let shade = if wave { [38, 132, 176, 255] } else { [30, 108, 150, 255] };
            raw.extend_from_slice(&shade);
        }
    }
    let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    z.write_all(&raw).expect("writing to memory cannot fail");
    let idat = z.finish().expect("writing to memory cannot fail");

    let mut png = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    let mut chunk = |kind: &[u8; 4], data: &[u8]| {
        png.extend_from_slice(&(data.len() as u32).to_be_bytes());
        let mut crc = flate2::Crc::new();
        crc.update(kind);
        crc.update(data);
        png.extend_from_slice(kind);
        png.extend_from_slice(data);
        png.extend_from_slice(&crc.sum().to_be_bytes());
    };
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&N.to_be_bytes());
    ihdr.extend_from_slice(&N.to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]); // 8 bits per channel, RGBA
    chunk(b"IHDR", &ihdr);
    chunk(b"IDAT", &idat);
    chunk(b"IEND", &[]);
    png
}
