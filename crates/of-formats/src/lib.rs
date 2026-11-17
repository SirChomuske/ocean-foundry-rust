//! Blocks and build files: NBT, the block model, and import/export of
//! Litematica, Sponge schematic, legacy schematic and Axiom blueprint files.

pub mod axiom;
pub mod catalogue;
pub mod export;
pub mod legacy;
pub mod litematic;
pub mod model;
pub mod nbt;
pub mod pack;
pub mod sponge;
pub mod survey;
#[cfg(test)]
mod tests;

use model::{Build, Format, Note, Pos, Problem};
use nbt::{Compound, Tag};

/// Progress reporting and cancellation for long reads and writes. The
/// callback receives a fraction from 0 to 1 and returns false to stop.
pub struct Progress<'a>(Option<&'a (dyn Fn(f32) -> bool + Sync)>);

impl<'a> Progress<'a> {
    pub fn none() -> Self {
        Self(None)
    }
    pub fn new(f: &'a (dyn Fn(f32) -> bool + Sync)) -> Self {
        Self(Some(f))
    }
    /// Report how far a job has got. Also stops it, with `MEMORY`, once the
    /// process is over the memory limit (`of_core::memory`), so no read,
    /// bake or write grows past it between two reports.
    pub fn step(&self, fraction: f32) -> Result<(), Problem> {
        if !of_core::memory::within_limit() {
            return Err(Problem::new(MEMORY).arg("limit", of_core::memory::mb(of_core::memory::limit())));
        }
        match self.0 {
            Some(f) if !f(fraction.clamp(0.0, 1.0)) => Err(Problem::new(CANCELLED)),
            _ => Ok(()),
        }
    }
}

/// Problem key used when the user cancels.
pub const CANCELLED: &str = "err.cancelled";

/// Problem key used when a job goes over the memory limit.
pub const MEMORY: &str = of_core::memory::OVER_LIMIT;

pub(crate) fn damaged(detail: impl Into<String>) -> Problem {
    Problem::new("err.damaged").arg("detail", detail.into())
}

pub(crate) fn xyz(c: &Compound) -> Option<Pos> {
    Some([c.i32("x")?, c.i32("y")?, c.i32("z")?])
}

pub(crate) fn xyz_tag(p: Pos) -> Tag {
    Tag::Compound(Compound::new().with("x", Tag::Int(p[0])).with("y", Tag::Int(p[1])).with("z", Tag::Int(p[2])))
}

pub(crate) fn now_millis() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

pub struct Imported {
    pub build: Build,
    pub notes: Vec<Note>,
}

/// Read a build file of any supported kind. The kind is recognised from the
/// file's contents, not its name.
pub fn import(data: &[u8], progress: &Progress) -> Result<Imported, Problem> {
    let (build, notes) = if axiom::looks_like(data) {
        axiom::read(data, progress)?
    } else {
        let (name, root) = nbt::read(data).map_err(|e| {
            if e.0 == nbt::OVER_LIMIT {
                Problem::new(MEMORY).arg("limit", of_core::memory::mb(of_core::memory::limit()))
            } else {
                Problem::new("err.not_a_build").arg("detail", e.to_string())
            }
        })?;
        if litematic::looks_like(&root) {
            litematic::read(&root, progress)?
        } else if sponge::looks_like(&name, &root) {
            sponge::read(&root, progress)?
        } else if legacy::looks_like(&root) {
            legacy::read(&root, progress)?
        } else {
            return Err(Problem::new("err.unknown_kind"));
        }
    };
    Ok(Imported { build, notes })
}

/// Where a writer reads cells from, a row along +X at a time: each cell as
/// a stored value (0 = absent, otherwise palette id + 1), as
/// `Volume::read_row` gives them.
pub trait Rows: Sync {
    fn read_row(&self, start: Pos, out: &mut [u32]);
}

impl Rows for model::Volume {
    fn read_row(&self, start: Pos, out: &mut [u32]) {
        model::Volume::read_row(self, start, out)
    }
}

/// Write a build in the given format exactly as it is laid out (regions and
/// all), reading each region's cells from the region itself.
pub fn write(build: &Build, format: Format, fallback_data_version: i32, progress: &Progress) -> Result<(Vec<u8>, Vec<Note>), Problem> {
    write_from(build, None, format, fallback_data_version, progress)
}

/// As `write`, but with `rows` set the cells of every region are read from
/// it instead, as each piece of the file is written; the regions then only
/// give the layout. An export lays out its regions this way over the bake's
/// own blocks (`export::export`), so the blocks are never copied.
pub fn write_from(build: &Build, rows: Option<&dyn Rows>, format: Format, fallback_data_version: i32, progress: &Progress) -> Result<(Vec<u8>, Vec<Note>), Problem> {
    match format {
        Format::Litematic => litematic::write(build, rows, fallback_data_version, progress),
        Format::Sponge => sponge::write(build, rows, fallback_data_version, progress),
        Format::Axiom => axiom::write(build, rows, fallback_data_version, progress),
        Format::Legacy => Err(Problem::new("err.legacy_export")),
    }
}
