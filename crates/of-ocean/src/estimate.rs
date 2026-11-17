//! How much memory an ocean job adds over what the process holds when it
//! starts: the bake, its 3D view and pictures, and saving its files.
//!
//! Why: stage 6 of the column bake plan. A job whose estimate is over the
//! memory limit is refused before it starts, instead of running until the
//! guard in `Progress::step` stops it part way.
//!
//! The job grows with the plot, not with the build or the box around it:
//! the loaded build and its shape are already held when the estimate is
//! made (the caller counts them in through the process's memory), the bake
//! keeps a few numbers per column, and the 3D view keeps runs up each
//! column. The bytes per column below are the highest each step reached in
//! the stage 5 measurements (`test-logs/measure/table-stage5.md`), rounded
//! up a little.

use crate::build::OceanSettings;
use crate::terrain::Terrain;

const MB: u64 = 1024 * 1024;

/// Bytes per column one kind of land takes at each step.
struct Rates {
    /// Highest point of the bake over what was held before it.
    bake: u64,
    /// What the bake keeps afterwards.
    kept: u64,
    /// Highest point of the 3D view and pictures over what the bake keeps.
    pictures: u64,
    /// Highest point of the shore picture and the report over the same.
    shore: u64,
}

/// Open sea: waves alone.
const OPEN: Rates = Rates { bake: 160, kept: 32, pictures: 62, shore: 0 };
/// The demonstration coasts: land, rivers, a current and the shore read.
const LAND: Rates = Rates { bake: 250, kept: 60, pictures: 62, shore: 75 };
/// A build brought in as the land.
const BUILD: Rates = Rates { bake: 225, kept: 34, pictures: 75, shore: 50 };

/// The pictures' own buffers, whatever the plot: about 1,700 by 1,000
/// pixels with their depths, and the map.
const PICTURE_BUFFERS: u64 = 10 * MB;

/// Bytes the bake of `settings` on `land` adds over what is held now, at
/// the highest point of the bake, the pictures or saving.
pub fn job(settings: &OceanSettings, land: Option<&Terrain>) -> u64 {
    let rates = match land {
        None => &OPEN,
        Some(t) if t.placed.is_some() => &BUILD,
        Some(_) => &LAND,
    };
    with_rates(settings, rates)
}

/// `job` for a build brought in as the land, before it is loaded.
pub fn job_on_build(settings: &OceanSettings) -> u64 {
    with_rates(settings, &BUILD)
}

/// What laying a loaded build down as the land adds: an entry for each
/// of its cubes while its shape is read (`terrain.rs`, `from_build`), and
/// the ground kept for each column of the plot (about 17 B, measured in
/// stage 7: 69 MB on 2048 x 2048, 17 MB on 1024 x 1024).
pub fn shape(cubes: u64, settings: &OceanSettings) -> u64 {
    cubes * 24 + 17 * settings.width.max(1) as u64 * settings.length.max(1) as u64
}

fn with_rates(settings: &OceanSettings, rates: &Rates) -> u64 {
    let columns = settings.width.max(1) as u64 * settings.length.max(1) as u64;
    let bake = rates.bake * columns;
    let after = rates.kept * columns + (rates.pictures * columns + PICTURE_BUFFERS).max(rates.shore * columns);
    bake.max(after)
}

// The test reads the cliff coast, a development-build land.
#[cfg(all(test, feature = "dev"))]
mod tests {
    use super::*;
    use of_core::params::ParamValues;

    #[test]
    fn the_estimate_grows_with_the_plot_and_the_land() {
        let mut values = ParamValues::default();
        let size = |values: &mut ParamValues, n: i64| {
            for id in ["ocean.width", "ocean.length"] {
                values.set(of_core::params::find(id).unwrap(), of_core::params::Value::Int(n));
            }
        };
        size(&mut values, 1024);
        let big = OceanSettings::from_params(&values);
        size(&mut values, 256);
        let small = OceanSettings::from_params(&values);
        let open = job(&big, None);
        assert!(open > 150 * MB && open < 200 * MB, "{open}");
        assert!(job(&small, None) < 25 * MB);
        let land = crate::terrain::cliff_scene(256, 256, 3);
        assert!(job(&small, Some(&land)) > job(&small, None));
    }
}
