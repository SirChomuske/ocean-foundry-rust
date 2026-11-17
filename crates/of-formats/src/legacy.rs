//! Legacy `.schematic` files (MCEdit / old WorldEdit, Minecraft 1.12 and
//! earlier). Blocks are numeric ids with a 4-bit data value, translated to
//! modern block states by the table below. Import only.
//!
//! The table covers building blocks and the common shaped blocks. An id it
//! does not cover is imported as a clearly named placeholder
//! (`legacy:block_<id>_<data>`) and reported, never guessed at; exporting a
//! build that still contains placeholders is refused.

use crate::model::{BlockEntity, BlockState, Build, Format, Note, Problem, Region};
use crate::nbt::{Compound, Tag};
use crate::{damaged, xyz, Progress};
use std::collections::BTreeSet;

const COLOURS: [&str; 16] = [
    "white", "orange", "magenta", "light_blue", "yellow", "lime", "pink", "gray", "light_gray", "cyan", "purple", "blue", "brown",
    "green", "red", "black",
];
const WOODS: [&str; 6] = ["oak", "spruce", "birch", "jungle", "acacia", "dark_oak"];
const STONE_SLABS: [&str; 8] = ["smooth_stone", "sandstone", "petrified_oak", "cobblestone", "brick", "stone_brick", "nether_brick", "quartz"];

fn b(name: &str) -> Option<BlockState> {
    Some(BlockState::new(name))
}

fn pick(names: &[&str], data: u8) -> Option<BlockState> {
    names.get(data as usize).and_then(|n| b(n))
}

fn coloured(data: u8, suffix: &str) -> Option<BlockState> {
    b(&format!("{}_{suffix}", COLOURS[(data & 15) as usize]))
}

fn axis(name: &str, data: u8) -> Option<BlockState> {
    let axis = match data & 12 {
        0 => "y",
        4 => "x",
        8 => "z",
        _ => return None,
    };
    Some(BlockState::new(name).with("axis", axis))
}

fn stairs(name: &str, data: u8) -> Option<BlockState> {
    if data > 7 {
        return None;
    }
    let facing = ["east", "west", "south", "north"][(data & 3) as usize];
    let half = if data & 4 != 0 { "top" } else { "bottom" };
    Some(BlockState::new(name).with("facing", facing).with("half", half).with("shape", "straight").with("waterlogged", "false"))
}

fn slab(kind: &str, top: bool) -> Option<BlockState> {
    Some(BlockState::new(&format!("{kind}_slab")).with("type", if top { "top" } else { "bottom" }).with("waterlogged", "false"))
}

fn double_slab(kind: &str) -> Option<BlockState> {
    Some(BlockState::new(&format!("{kind}_slab")).with("type", "double").with("waterlogged", "false"))
}

fn log(woods: &[&str], data: u8) -> Option<BlockState> {
    let wood = woods.get((data & 3) as usize)?;
    match data >> 2 {
        0 => Some(BlockState::new(&format!("{wood}_log")).with("axis", "y")),
        1 => Some(BlockState::new(&format!("{wood}_log")).with("axis", "x")),
        2 => Some(BlockState::new(&format!("{wood}_log")).with("axis", "z")),
        _ => Some(BlockState::new(&format!("{wood}_wood")).with("axis", "y")),
    }
}

fn leaves(woods: &[&str], data: u8) -> Option<BlockState> {
    let wood = woods.get((data & 3) as usize)?;
    let persistent = if data & 4 != 0 { "true" } else { "false" };
    Some(BlockState::new(&format!("{wood}_leaves")).with("distance", "7").with("persistent", persistent).with("waterlogged", "false"))
}

fn fluid(name: &str, data: u8) -> Option<BlockState> {
    Some(BlockState::new(name).with("level", &data.to_string()))
}

fn horizontal(name: &str, data: u8) -> Option<BlockState> {
    let facing = ["south", "west", "north", "east"][(data & 3) as usize];
    Some(BlockState::new(name).with("facing", facing))
}

fn fence(name: &str) -> Option<BlockState> {
    let mut s = BlockState::new(name);
    for side in ["north", "east", "south", "west", "waterlogged"] {
        s = s.with(side, "false");
    }
    Some(s)
}

/// Translate a numeric block and its data value to a modern block state.
pub fn legacy_state(id: u16, data: u8) -> Option<BlockState> {
    let plain = |name: &str| if data == 0 { b(name) } else { None };
    match id {
        0 => b("air"),
        1 => pick(&["stone", "granite", "polished_granite", "diorite", "polished_diorite", "andesite", "polished_andesite"], data),
        2 => Some(BlockState::new("grass_block").with("snowy", "false")),
        3 => match data {
            0 => b("dirt"),
            1 => b("coarse_dirt"),
            2 => Some(BlockState::new("podzol").with("snowy", "false")),
            _ => None,
        },
        4 => plain("cobblestone"),
        5 => WOODS.get(data as usize).and_then(|w| b(&format!("{w}_planks"))),
        7 => b("bedrock"),
        8 | 9 => fluid("water", data),
        10 | 11 => fluid("lava", data),
        12 => pick(&["sand", "red_sand"], data),
        13 => plain("gravel"),
        14 => plain("gold_ore"),
        15 => plain("iron_ore"),
        16 => plain("coal_ore"),
        17 => log(&WOODS[..4], data),
        18 => leaves(&WOODS[..4], data),
        19 => pick(&["sponge", "wet_sponge"], data),
        20 => plain("glass"),
        21 => plain("lapis_ore"),
        22 => plain("lapis_block"),
        24 => pick(&["sandstone", "chiseled_sandstone", "cut_sandstone"], data),
        30 => plain("cobweb"),
        31 => pick(&["dead_bush", "short_grass", "fern"], data),
        32 => plain("dead_bush"),
        35 => coloured(data, "wool"),
        37 => plain("dandelion"),
        38 => pick(
            &["poppy", "blue_orchid", "allium", "azure_bluet", "red_tulip", "orange_tulip", "white_tulip", "pink_tulip", "oxeye_daisy"],
            data,
        ),
        39 => plain("brown_mushroom"),
        40 => plain("red_mushroom"),
        41 => plain("gold_block"),
        42 => plain("iron_block"),
        43 => match data {
            0..=7 => double_slab(STONE_SLABS[data as usize]),
            8 => b("smooth_stone"),
            9 => b("smooth_sandstone"),
            15 => b("smooth_quartz"),
            _ => None,
        },
        44 => slab(STONE_SLABS[(data & 7) as usize], data & 8 != 0),
        45 => plain("bricks"),
        46 => Some(BlockState::new("tnt").with("unstable", "false")),
        47 => plain("bookshelf"),
        48 => plain("mossy_cobblestone"),
        49 => plain("obsidian"),
        50 => match data {
            1 => Some(BlockState::new("wall_torch").with("facing", "east")),
            2 => Some(BlockState::new("wall_torch").with("facing", "west")),
            3 => Some(BlockState::new("wall_torch").with("facing", "south")),
            4 => Some(BlockState::new("wall_torch").with("facing", "north")),
            0 | 5 => b("torch"),
            _ => None,
        },
        53 => stairs("oak_stairs", data),
        56 => plain("diamond_ore"),
        57 => plain("diamond_block"),
        58 => plain("crafting_table"),
        60 => Some(BlockState::new("farmland").with("moisture", &(data & 7).to_string())),
        65 => {
            let facing = match data {
                2 => "north",
                3 => "south",
                4 => "west",
                5 => "east",
                _ => return None,
            };
            Some(BlockState::new("ladder").with("facing", facing).with("waterlogged", "false"))
        }
        67 => stairs("cobblestone_stairs", data),
        73 | 74 => Some(BlockState::new("redstone_ore").with("lit", if id == 74 { "true" } else { "false" })),
        78 => Some(BlockState::new("snow").with("layers", &((data & 7) + 1).to_string())),
        79 => plain("ice"),
        80 => plain("snow_block"),
        82 => plain("clay"),
        85 => fence("oak_fence"),
        86 => horizontal("carved_pumpkin", data),
        87 => plain("netherrack"),
        88 => plain("soul_sand"),
        89 => plain("glowstone"),
        91 => horizontal("jack_o_lantern", data),
        95 => coloured(data, "stained_glass"),
        98 => pick(&["stone_bricks", "mossy_stone_bricks", "cracked_stone_bricks", "chiseled_stone_bricks"], data),
        101 => fence("iron_bars"),
        102 => fence("glass_pane"),
        103 => plain("melon"),
        108 => stairs("brick_stairs", data),
        109 => stairs("stone_brick_stairs", data),
        110 => Some(BlockState::new("mycelium").with("snowy", "false")),
        112 => plain("nether_bricks"),
        113 => fence("nether_brick_fence"),
        114 => stairs("nether_brick_stairs", data),
        121 => plain("end_stone"),
        125 => WOODS.get(data as usize).and_then(|w| double_slab(w)),
        126 => WOODS.get((data & 7) as usize).and_then(|w| slab(w, data & 8 != 0)),
        128 => stairs("sandstone_stairs", data),
        129 => plain("emerald_ore"),
        133 => plain("emerald_block"),
        134 => stairs("spruce_stairs", data),
        135 => stairs("birch_stairs", data),
        136 => stairs("jungle_stairs", data),
        152 => plain("redstone_block"),
        153 => plain("nether_quartz_ore"),
        155 => match data {
            0 => b("quartz_block"),
            1 => b("chiseled_quartz_block"),
            2 => Some(BlockState::new("quartz_pillar").with("axis", "y")),
            3 => Some(BlockState::new("quartz_pillar").with("axis", "x")),
            4 => Some(BlockState::new("quartz_pillar").with("axis", "z")),
            _ => None,
        },
        156 => stairs("quartz_stairs", data),
        159 => coloured(data, "terracotta"),
        160 => coloured(data, "stained_glass_pane").map(|s| ["north", "east", "south", "west", "waterlogged"].iter().fold(s, |s, k| s.with(k, "false"))),
        161 => leaves(&WOODS[4..], data),
        162 => log(&WOODS[4..], data),
        163 => stairs("acacia_stairs", data),
        164 => stairs("dark_oak_stairs", data),
        165 => plain("slime_block"),
        166 => plain("barrier"),
        168 => pick(&["prismarine", "prismarine_bricks", "dark_prismarine"], data),
        169 => plain("sea_lantern"),
        170 => axis("hay_block", data),
        171 => coloured(data, "carpet"),
        172 => plain("terracotta"),
        173 => plain("coal_block"),
        174 => plain("packed_ice"),
        179 => pick(&["red_sandstone", "chiseled_red_sandstone", "cut_red_sandstone"], data),
        180 => stairs("red_sandstone_stairs", data),
        181 => match data {
            0 => double_slab("red_sandstone"),
            8 => b("smooth_red_sandstone"),
            _ => None,
        },
        182 => slab("red_sandstone", data & 8 != 0),
        188 => fence("spruce_fence"),
        189 => fence("birch_fence"),
        190 => fence("jungle_fence"),
        191 => fence("dark_oak_fence"),
        192 => fence("acacia_fence"),
        198 => {
            let facing = ["down", "up", "north", "south", "west", "east"].get(data as usize)?;
            Some(BlockState::new("end_rod").with("facing", facing))
        }
        201 => plain("purpur_block"),
        202 => axis("purpur_pillar", data),
        203 => stairs("purpur_stairs", data),
        204 => double_slab("purpur"),
        205 => slab("purpur", data & 8 != 0),
        206 => plain("end_stone_bricks"),
        208 => plain("dirt_path"),
        213 => plain("magma_block"),
        214 => plain("nether_wart_block"),
        215 => plain("red_nether_bricks"),
        216 => axis("bone_block", data),
        235..=250 => horizontal(&format!("{}_glazed_terracotta", COLOURS[(id - 235) as usize]), data),
        251 => coloured(data, "concrete"),
        252 => coloured(data, "concrete_powder"),
        _ => None,
    }
}

pub fn placeholder(id: u16, data: u8) -> BlockState {
    BlockState { name: format!("legacy:block_{id}_{data}"), props: Vec::new() }
}

pub fn is_placeholder(state: &BlockState) -> bool {
    state.name.starts_with("legacy:")
}

pub fn looks_like(root: &Compound) -> bool {
    root.contains("Blocks") && root.contains("Data") && root.contains("Width") && !root.contains("Palette")
}

pub fn read(root: &Compound, progress: &Progress) -> Result<(Build, Vec<Note>), Problem> {
    let dim = |key: &str| root.get(key).and_then(Tag::as_i64).map(|v| (v as u16) as i32).ok_or_else(|| damaged(format!("no {key}")));
    let dims = [dim("Width")?, dim("Height")?, dim("Length")?];
    let blocks = root.get("Blocks").and_then(Tag::as_bytes).ok_or_else(|| damaged("no Blocks"))?;
    let data = root.get("Data").and_then(Tag::as_bytes).ok_or_else(|| damaged("no Data"))?;
    let add = root.get("AddBlocks").and_then(Tag::as_bytes);
    let cells = dims.iter().map(|d| *d as usize).product::<usize>();
    if blocks.len() < cells || data.len() < cells {
        return Err(damaged("block data is too short"));
    }
    // WorldEdit stores where the box sits relative to the paste point.
    let offset = match (root.i32("WEOffsetX"), root.i32("WEOffsetY"), root.i32("WEOffsetZ")) {
        (Some(x), Some(y), Some(z)) => [x, y, z],
        _ => [0; 3],
    };

    let mut build = Build::new();
    build.source = Some(Format::Legacy);
    build.extra = root.without(&["Width", "Height", "Length", "Blocks", "Data", "AddBlocks", "TileEntities", "Entities"]);
    let mut region = Region::new("", offset, dims);
    let mut cells = crate::model::Volume::new();
    // One table entry per (id, data), filled in on first use.
    let mut table: Vec<u32> = vec![0; 4096 * 16];
    let mut unmapped: BTreeSet<(u16, u8)> = BTreeSet::new();
    let [sx, sy, sz] = dims;
    let mut row = vec![0u32; sx as usize];
    let mut i = 0usize;
    for y in 0..sy {
        for z in 0..sz {
            for cell in row.iter_mut() {
                let mut id = blocks[i] as u16;
                if let Some(nibbles) = add.and_then(|a| a.get(i >> 1)) {
                    id |= (if i & 1 == 0 { nibbles & 0x0F } else { nibbles >> 4 } as u16) << 8;
                }
                let d = data[i] & 15;
                let key = (id as usize) * 16 + d as usize;
                if table[key] == 0 {
                    let state = legacy_state(id, d).unwrap_or_else(|| {
                        unmapped.insert((id, d));
                        placeholder(id, d)
                    });
                    table[key] = build.palette.intern(state) + 1;
                }
                *cell = table[key];
                i += 1;
            }
            cells.write_row([offset[0], offset[1] + y, offset[2] + z], &row);
        }
        let world_y = offset[1] + y;
        if world_y & 15 == 15 || y == sy - 1 {
            cells.share_uniform(Some(world_y >> 4));
        }
        progress.step((y + 1) as f32 / sy.max(1) as f32)?;
    }

    for te in root.list("TileEntities").unwrap_or(&[]) {
        let c = te.as_compound().ok_or_else(|| damaged("unreadable block entity"))?;
        let local = xyz(c).ok_or_else(|| damaged("block entity without a position"))?;
        region.block_entities.push(BlockEntity {
            pos: [offset[0] + local[0], offset[1] + local[1], offset[2] + local[2]],
            id: c.str("id").map(str::to_string),
            data: c.without(&["x", "y", "z", "id"]),
        });
    }
    region.cells = std::sync::Arc::new(cells);
    region.entities = root.list("Entities").unwrap_or(&[]).to_vec();

    let mut notes = Vec::new();
    if !unmapped.is_empty() {
        let list: Vec<String> = unmapped.iter().take(12).map(|(id, d)| format!("{id}:{d}")).collect();
        let more = if unmapped.len() > 12 { ", …" } else { "" };
        notes.push(Note::new("note.legacy_unmapped").arg("count", unmapped.len()).arg("list", format!("{}{more}", list.join(", "))));
    }
    if !region.block_entities.is_empty() {
        notes.push(Note::new("note.legacy_block_entities").arg("count", region.block_entities.len()));
    }
    build.regions.push(region);
    Ok((build, notes))
}

/// Build a legacy file, for tests.
#[cfg(test)]
pub fn make_test_file(dims: [i32; 3], cells: &[(u16, u8)]) -> Vec<u8> {
    let blocks: Vec<u8> = cells.iter().map(|(id, _)| *id as u8).collect();
    let data: Vec<u8> = cells.iter().map(|(_, d)| *d).collect();
    let mut add = vec![0u8; cells.len().div_ceil(2)];
    for (i, (id, _)) in cells.iter().enumerate() {
        let high = (id >> 8) as u8;
        add[i >> 1] |= if i & 1 == 0 { high } else { high << 4 };
    }
    let root = Compound::new()
        .with("Width", Tag::Short(dims[0] as i16))
        .with("Height", Tag::Short(dims[1] as i16))
        .with("Length", Tag::Short(dims[2] as i16))
        .with("Materials", Tag::string("Alpha"))
        .with("Blocks", Tag::ByteArray(blocks))
        .with("AddBlocks", Tag::ByteArray(add))
        .with("Data", Tag::ByteArray(data))
        .with("WEOffsetX", Tag::Int(-1))
        .with("WEOffsetY", Tag::Int(0))
        .with("WEOffsetZ", Tag::Int(2))
        .with("TileEntities", Tag::List(crate::nbt::List { elem: 10, items: vec![] }));
    crate::nbt::write_gzip("Schematic", &root)
}
