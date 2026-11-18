//! What a bake leaves behind: the maps the exporter and the pictures read.

use crate::blocks::Blocks;
use crate::render::Picture;
use crate::terrain::Terrain;

#[derive(Debug, Clone, Default)]
pub struct Stats {
    pub water_columns: u64,
    /// Blocks in each layer, in `Layer::ALL` order.
    pub blocks: [u64; 6],
    /// Every block used and how many, most used first.
    pub used: Vec<(String, u64)>,
    /// Lowest and highest water surface, in blocks relative to the resting level.
    pub range: (i32, i32),
    /// Water columns built as white water.
    pub white_columns: u64,
}

pub struct Baked {
    /// The blocks, kept as columns (see `blocks.rs`); `layers()` lays them out.
    pub blocks: Blocks,
    /// Height inside the build of the resting water level.
    pub sea_level: i32,
    /// World Y at which the build's y = 0 belongs, so the water rests at the chosen level.
    pub export_y: i32,
    pub width: i32,
    pub length: i32,
    /// Per column, the height inside the build of the top water block; `DRY` where there is none.
    pub water_top: Vec<i32>,
    /// Per column, the lowest water block.
    pub water_bottom: Vec<i32>,
    /// Per column, the colour the water was meant to have, and the colour
    /// of the blocks chosen for it as they look from above. Empty when the
    /// build has no coloured surface.
    pub target: Vec<[u8; 3]>,
    pub matched: Vec<[u8; 3]>,
    /// What was worked out for each column on the way to its colour.
    pub maps: Maps,
    pub stats: Stats,
}

/// Per column, as `target`: empty when the build has no coloured surface.
#[derive(Debug, Clone, Default)]
pub struct Maps {
    /// Depth of the water over the ground or the made-up sea floor;
    /// infinite where there is neither.
    pub depth: Vec<f32>,
    /// White water, 0 to 255.
    pub foam: Vec<u8>,
    /// Share of the colour that is mirrored sky, 0 to 255.
    pub sheen: Vec<u8>,
}

impl Baked {
    /// The colour every column of water was meant to have (left) beside the
    /// colour of the blocks chosen for it (right), from above with north up.
    /// The difference between the two is what the blocks could not give.
    pub fn colour_picture(&self, scale: u32) -> Option<Picture> {
        if self.target.is_empty() {
            return None;
        }
        let (w, l, s) = (self.width as u32, self.length as u32, scale.max(1));
        let gap = 4 * s;
        let (width, height) = (2 * w * s + gap, l * s);
        let mut rgba = vec![255u8; (width * height * 4) as usize];
        for (half, colours) in [&self.target, &self.matched].into_iter().enumerate() {
            let left = half as u32 * (w * s + gap);
            for z in 0..l {
                for x in 0..w {
                    let i = (z * w + x) as usize;
                    let c = if self.water_top[i] == DRY { [203, 201, 195] } else { colours[i] };
                    for py in z * s..(z + 1) * s {
                        for px in left + x * s..left + (x + 1) * s {
                            let at = ((py * width + px) * 4) as usize;
                            rgba[at..at + 3].copy_from_slice(&c);
                        }
                    }
                }
            }
        }
        Some(Picture { width, height, rgba })
    }
}

pub const DRY: i32 = i32::MIN;

/// The tallest a build may stand, in blocks: the most a Minecraft world holds.
pub const MOST_HEIGHT: i64 = 4064;

/// The highest level anything reaches: the water and the glass over it, the
/// ground, rock spanning over the water, and a build brought in as the land.
pub(crate) fn height_reached(top: &[i32], ground: &[i32], terrain: Option<&Terrain>) -> i32 {
    let mut highest = i32::MIN;
    for (t, g) in top.iter().zip(ground) {
        if *t != DRY {
            highest = highest.max(t.saturating_add(3));
        }
        if *g != DRY {
            highest = highest.max(*g);
        }
    }
    if let Some(t) = terrain {
        for z in 0..t.length {
            for x in 0..t.width {
                if let Some((_, high)) = t.roof(x, z) {
                    highest = highest.max(high);
                }
            }
        }
        if let Some(p) = &t.placed {
            highest = highest.max(p.shift[1].saturating_add(p.size[1] - 1));
        }
    }
    highest
}

/// An error when a build reaching from `lowest` to `highest` is taller than
/// a Minecraft world, as it only is when a height has gone wrong. Checked
/// before any block is placed, so it fails at once and allocates nothing.
pub(crate) fn tall_enough(lowest: i32, highest: i32) -> Result<(), String> {
    let span = highest as i64 - lowest as i64 + 1;
    if span > MOST_HEIGHT {
        return Err(format!("the build would stand {span} blocks tall, more than a Minecraft world holds ({MOST_HEIGHT}); nothing was built"));
    }
    Ok(())
}
