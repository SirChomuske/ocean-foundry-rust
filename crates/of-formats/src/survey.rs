//! What loading a `.litematic` file will hold, found by reading through it
//! without keeping it.
//!
//! Why: stage 7 of the column bake plan. The statue's estimate is shown to
//! the user before anything runs, and loading the build is the largest
//! part of it. Loading keeps every 16-block cube of each region's box
//! (cubes of one value throughout share a copy, `Volume::share_uniform`),
//! so the cost depends on how the blocks fall into cubes, not just on how
//! many there are. This reads the file twice, unpacking as it goes and
//! holding only a small piece at a time: once for each region's corner,
//! size and palette, and once to walk its block data cube by cube the way
//! the reader fills them.

use crate::model::{BlockState, Palette};
use crate::nbt::{Compound, List, Tag};
use crate::pack::bits_for;
use crate::{Problem, Progress};
use flate2::read::GzDecoder;
use std::collections::HashSet;
use std::io::{BufReader, Read};

/// Bytes of one stored cube: 16 × 16 × 16 cells of 4 bytes.
const CUBE_BYTES: u64 = 16 * 16 * 16 * 4;
/// The map entry and pointer each cube takes, stored or shared.
const CUBE_ENTRY: u64 = 32;

/// How a `.litematic` file falls into the loader's cubes.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Survey {
    pub regions: usize,
    /// Cells in all the regions' boxes, air included.
    pub cells: u64,
    /// Cells that hold a block other than air.
    pub blocks: u64,
    /// Cubes the loader keeps, and how many of them are stored separately.
    pub cubes: u64,
    pub stored: u64,
    /// Bytes of the unpacked file, and of its block data alone.
    pub unpacked: u64,
    pub block_data: u64,
}

impl Survey {
    /// What the loaded build holds afterwards.
    pub fn held(&self) -> u64 {
        self.stored * CUBE_BYTES + self.cubes * CUBE_ENTRY
    }
    /// The highest point while loading: the unpacked file and its tags are
    /// held together, then the tags while the cubes are filled.
    pub fn peak(&self) -> u64 {
        (2 * self.unpacked).max(self.block_data + self.held())
    }
}

/// One region as the first pass finds it.
#[derive(Default)]
struct Plan {
    position: Option<[i32; 3]>,
    size: Option<[i32; 3]>,
    palette: Vec<BlockState>,
}

/// Survey a `.litematic` file (gzip or not). Checks the memory limit and
/// can be cancelled through `progress`, once per layer of a region.
pub fn litematic(data: &[u8], progress: &Progress) -> Result<Survey, Problem> {
    let damaged = |e: String| crate::damaged(e);
    let mut plans = Vec::new();
    let mut s = Survey::default();
    let unpacked = walk(data, &mut |scan, region, key, id| {
        if region >= plans.len() {
            plans.resize_with(region + 1, Plan::default);
        }
        let plan = &mut plans[region];
        match (key, id) {
            ("Position", 10) | ("Size", 10) => {
                let tag = scan.tag(id, 0)?;
                let c = tag.as_compound().ok_or("not a compound")?;
                let xyz = [c.i32("x"), c.i32("y"), c.i32("z")];
                let v = if let [Some(x), Some(y), Some(z)] = xyz { Some([x, y, z]) } else { None };
                if key == "Position" { plan.position = v } else { plan.size = v }
            }
            ("BlockStatePalette", 9) => {
                let tag = scan.tag(id, 0)?;
                for entry in tag.as_list().unwrap_or(&[]) {
                    plan.palette.push(entry.as_compound().and_then(BlockState::from_nbt).ok_or("unreadable palette entry")?);
                }
            }
            ("BlockStates", 12) => {
                let n = scan.i32()?.max(0) as u64;
                s.block_data += n * 8;
                scan.skip_bytes(n * 8)?;
            }
            _ => scan.skip(id, 0)?,
        }
        Ok(())
    })
    .map_err(damaged)?;
    s.unpacked = unpacked;
    s.regions = plans.len();

    // The whole build's palette, as the reader makes it: the same state in
    // two regions is one id.
    let mut palette = Palette::default();
    let ids: Vec<Vec<u32>> = plans.iter().map(|p| p.palette.iter().map(|b| palette.intern(b.clone()) + 1).collect()).collect();
    let air: Vec<bool> = std::iter::once(true).chain(palette.states().iter().map(BlockState::is_air)).collect();
    let total: u64 = plans.iter().filter_map(|p| p.size).map(|d| d.iter().map(|v| v.unsigned_abs() as u64).product::<u64>()).sum();
    let mut done = 0u64;
    let mut failed: Option<Problem> = None;

    let read = walk(data, &mut |scan, region, key, id| {
        if key != "BlockStates" || id != 12 {
            return scan.skip(id, 0);
        }
        let n = scan.i32()?.max(0) as u64;
        let plan = &plans[region];
        let (Some(position), Some(size)) = (plan.position, plan.size) else { return scan.skip_bytes(n * 8) };
        let mut min = [0; 3];
        let mut dims = [0i64; 3];
        for a in 0..3 {
            dims[a] = (size[a] as i64).abs();
            min[a] = if size[a] < 0 { position[a] as i64 + size[a] as i64 + 1 } else { position[a] as i64 };
        }
        let volume = (dims[0] * dims[1] * dims[2]) as u64;
        s.cells += volume;
        if volume == 0 {
            return scan.skip_bytes(n * 8);
        }
        let ids = &ids[region];
        let bits = bits_for(ids.len(), 2);
        let mask = (1u64 << bits) - 1;
        let mut left = n;
        let (mut acc, mut have) = (0u128, 0u32);

        // Cubes of one layer (16 rows of y) of this region, left to right.
        let (cx0, cx1) = (min[0] >> 4, (min[0] + dims[0] - 1) >> 4);
        let (cz0, cz1) = (min[2] >> 4, (min[2] + dims[2] - 1) >> 4);
        let (ncx, ncz) = ((cx1 - cx0 + 1) as usize, (cz1 - cz0 + 1) as usize);
        let whole = |c: i64, lo: i64, len: i64| c << 4 >= lo && (c << 4) + 15 < lo + len;
        let full_x: Vec<bool> = (cx0..=cx1).map(|c| whole(c, min[0], dims[0])).collect();
        let full_z: Vec<bool> = (cz0..=cz1).map(|c| whole(c, min[2], dims[2])).collect();
        const MIXED: u32 = u32::MAX;
        let mut layer = vec![0u32; ncx * ncz];
        let mut shared: HashSet<u32> = HashSet::new();
        let mut row_cube = Vec::with_capacity(dims[0] as usize);
        for x in 0..dims[0] {
            row_cube.push((((min[0] + x) >> 4) - cx0) as usize);
        }

        for y in 0..dims[1] {
            for z in 0..dims[2] {
                let base = (((min[2] + z) >> 4) - cz0) as usize * ncx;
                for cube in &row_cube {
                    if have < bits {
                        if left == 0 {
                            return Err("block data is too short".into());
                        }
                        acc |= (scan.i64()? as u64 as u128) << have;
                        have += 64;
                        left -= 1;
                    }
                    let v = (acc as u64 & mask) as usize;
                    acc >>= bits;
                    have -= bits;
                    let id = *ids.get(v).ok_or("a block outside its palette")?;
                    if !air[id as usize] {
                        s.blocks += 1;
                    }
                    let cell = &mut layer[base + cube];
                    if *cell == 0 {
                        *cell = id;
                    } else if *cell != id {
                        *cell = MIXED;
                    }
                }
            }
            let world_y = min[1] + y;
            if world_y & 15 == 15 || y == dims[1] - 1 {
                let full_y = whole(world_y >> 4, min[1], dims[1]);
                for (i, cell) in layer.iter_mut().enumerate() {
                    s.cubes += 1;
                    if full_y && full_x[i % ncx] && full_z[i / ncx] && *cell != MIXED {
                        shared.insert(*cell);
                    } else {
                        s.stored += 1;
                    }
                    *cell = 0;
                }
            }
            done += (dims[0] * dims[2]) as u64;
            if let Err(p) = progress.step(done as f32 / total.max(1) as f32) {
                failed = Some(p);
                return Err(String::new());
            }
        }
        s.stored += shared.len() as u64;
        scan.skip_bytes(left * 8)
    });
    read.map_err(|e| failed.take().unwrap_or_else(|| damaged(e)))?;
    Ok(s)
}

/// Read the file through once, calling `visit(scan, region, key, tag id)`
/// for every entry of every region; `visit` must read or skip the entry's
/// payload. Returns the unpacked size.
fn walk(data: &[u8], visit: &mut dyn FnMut(&mut Scan, usize, &str, u8) -> Result<(), String>) -> Result<u64, String> {
    let gz = data.len() >= 2 && data[0] == 0x1f && data[1] == 0x8b;
    let source: Box<dyn Read + '_> = if gz { Box::new(GzDecoder::new(data)) } else { Box::new(data) };
    let mut scan = Scan { r: BufReader::with_capacity(1 << 16, source), read: 0 };
    if scan.u8()? != 10 {
        return Err("not an NBT file".into());
    }
    scan.string()?;
    let mut region = 0;
    loop {
        let id = scan.u8()?;
        if id == 0 {
            break;
        }
        let name = scan.string()?;
        if name != "Regions" || id != 10 {
            scan.skip(id, 0)?;
            continue;
        }
        loop {
            let id = scan.u8()?;
            if id == 0 {
                break;
            }
            scan.string()?;
            if id != 10 {
                scan.skip(id, 0)?;
                continue;
            }
            loop {
                let id = scan.u8()?;
                if id == 0 {
                    break;
                }
                let key = scan.string()?;
                visit(&mut scan, region, &key, id)?;
            }
            region += 1;
        }
    }
    Ok(scan.read)
}

/// A forward-only NBT reader over a stream.
struct Scan<'a> {
    r: BufReader<Box<dyn Read + 'a>>,
    read: u64,
}

const MAX_DEPTH: u32 = 512;

impl Scan<'_> {
    fn bytes<const N: usize>(&mut self) -> Result<[u8; N], String> {
        let mut b = [0u8; N];
        self.r.read_exact(&mut b).map_err(|e| format!("the file ends early or is damaged: {e}"))?;
        self.read += N as u64;
        Ok(b)
    }
    fn u8(&mut self) -> Result<u8, String> {
        Ok(self.bytes::<1>()?[0])
    }
    fn i32(&mut self) -> Result<i32, String> {
        Ok(i32::from_be_bytes(self.bytes()?))
    }
    fn i64(&mut self) -> Result<i64, String> {
        Ok(i64::from_be_bytes(self.bytes()?))
    }
    fn len(&mut self) -> Result<u64, String> {
        Ok(self.i32()?.max(0) as u64)
    }
    fn string(&mut self) -> Result<String, String> {
        let n = u16::from_be_bytes(self.bytes()?) as usize;
        let mut b = vec![0u8; n];
        self.r.read_exact(&mut b).map_err(|e| format!("the file ends early: {e}"))?;
        self.read += n as u64;
        Ok(String::from_utf8_lossy(&b).into_owned())
    }
    fn skip_bytes(&mut self, n: u64) -> Result<(), String> {
        let got = std::io::copy(&mut (&mut self.r).take(n), &mut std::io::sink()).map_err(|e| e.to_string())?;
        self.read += got;
        if got < n { Err("the file ends early".into()) } else { Ok(()) }
    }
    fn skip(&mut self, id: u8, depth: u32) -> Result<(), String> {
        if depth > MAX_DEPTH {
            return Err("tags are nested too deeply".into());
        }
        match id {
            1 => self.skip_bytes(1),
            2 => self.skip_bytes(2),
            3 | 5 => self.skip_bytes(4),
            4 | 6 => self.skip_bytes(8),
            7 => {
                let n = self.len()?;
                self.skip_bytes(n)
            }
            8 => {
                let n = u16::from_be_bytes(self.bytes()?) as u64;
                self.skip_bytes(n)
            }
            9 => {
                let elem = self.u8()?;
                let n = self.len()?;
                for _ in 0..n {
                    self.skip(elem, depth + 1)?;
                }
                Ok(())
            }
            10 => loop {
                let id = self.u8()?;
                if id == 0 {
                    return Ok(());
                }
                let n = u16::from_be_bytes(self.bytes()?) as u64;
                self.skip_bytes(n)?;
                self.skip(id, depth + 1)?;
            },
            11 => {
                let n = self.len()?;
                self.skip_bytes(n * 4)
            }
            12 => {
                let n = self.len()?;
                self.skip_bytes(n * 8)
            }
            other => Err(format!("unknown tag type {other}")),
        }
    }
    /// Read a small tag whole (a palette, a corner).
    fn tag(&mut self, id: u8, depth: u32) -> Result<Tag, String> {
        if depth > MAX_DEPTH {
            return Err("tags are nested too deeply".into());
        }
        Ok(match id {
            1 => Tag::Byte(self.u8()? as i8),
            2 => Tag::Short(i16::from_be_bytes(self.bytes()?)),
            3 => Tag::Int(self.i32()?),
            4 => Tag::Long(self.i64()?),
            5 => Tag::Float(f32::from_bits(self.i32()? as u32)),
            6 => Tag::Double(f64::from_bits(self.i64()? as u64)),
            8 => Tag::String(self.string()?),
            9 => {
                let elem = self.u8()?;
                let n = self.len()?;
                let mut items = Vec::new();
                for _ in 0..n {
                    items.push(self.tag(elem, depth + 1)?);
                }
                Tag::List(List { elem, items })
            }
            10 => {
                let mut entries = Vec::new();
                loop {
                    let id = self.u8()?;
                    if id == 0 {
                        break;
                    }
                    let name = self.string()?;
                    entries.push((name, self.tag(id, depth + 1)?));
                }
                Tag::Compound(Compound(entries))
            }
            7 | 11 | 12 => {
                self.skip(id, depth)?;
                Tag::List(List { elem: 0, items: Vec::new() })
            }
            other => return Err(format!("unknown tag type {other}")),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Build, Format, Region, Volume};
    use std::sync::Arc;

    /// The survey counts the same cubes as loading the file does.
    #[test]
    fn the_survey_matches_what_loading_holds() {
        let mut build = Build::new();
        let stone = build.palette.intern(BlockState::new("minecraft:stone"));
        let air = build.palette.intern(BlockState::air());
        // A region whose box starts off the cube grid, with a solid lump,
        // a shell and a lot of air; and a second, flipped region.
        let mut region = Region::new("a", [-21, 3, 5], [70, 50, 40]);
        let mut cells = Volume::new();
        for y in 3..53 {
            for z in 5..45 {
                for x in -21..49 {
                    let solid = y < 20 || (x == 30 && z % 3 == 0);
                    cells.set([x, y, z], if solid { stone } else { air });
                }
            }
        }
        region.cells = Arc::new(cells);
        build.regions.push(region);
        let mut second = Region::new("b", [100, 0, 0], [17, 33, 16]);
        second.flipped = [true, false, true];
        let mut cells = Volume::new();
        for y in 0..33 {
            for z in 0..16 {
                for x in 100..117 {
                    cells.set([x, y, z], if (x + y) % 5 == 0 { stone } else { air });
                }
            }
        }
        second.cells = Arc::new(cells);
        build.regions.push(second);
        let (bytes, _) = crate::write(&build, Format::Litematic, 4000, &Progress::none()).unwrap();

        let survey = litematic(&bytes, &Progress::none()).unwrap();
        let loaded = crate::import(&bytes, &Progress::none()).unwrap().build;
        let (mut cubes, mut stored, mut blocks) = (0, 0, 0);
        for r in &loaded.regions {
            let (c, s) = r.cells.cube_counts();
            cubes += c as u64;
            stored += s as u64;
            r.cells.for_each(|_, id| blocks += !loaded.palette.get(id).is_air() as u64);
        }
        assert_eq!((survey.regions, survey.cubes, survey.stored, survey.blocks), (2, cubes, stored, blocks));
        assert_eq!(survey.cells, 70 * 50 * 40 + 17 * 33 * 16);
        assert!(survey.unpacked > survey.block_data && survey.block_data > 0);
        assert!(stored < cubes, "some cubes are shared: {stored} of {cubes}");
    }
}
