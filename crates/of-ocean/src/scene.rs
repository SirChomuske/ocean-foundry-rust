//! What the sea is baked around: a piece of land, and the rivers, streams
//! and waterfalls laid over it.

#[cfg(feature = "dev")]
use crate::terrain::{cliff_scene, coast_scene};
use crate::terrain::{Outcome, Pond, Stream, Terrain};
use crate::zone::Zone;
use of_formats::model::Build;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// The file in the data folder that keeps the scene between runs.
pub const SCENE_FILE: &str = "ocean-scene.json";

/// The land in the scene.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Land {
    /// Nothing but water.
    #[default]
    Open,
    /// A beach that turns into a cliff sea front, and an island
    /// (development build only).
    #[cfg(feature = "dev")]
    Coast,
    /// A cliff coast with a rock arch, sea stacks, a bay and a cove
    /// (development build only).
    #[cfg(feature = "dev")]
    Cliffs,
    /// A build brought in from a file, standing in the sea.
    Build,
}

impl Land {
    #[cfg(feature = "dev")]
    pub const ALL: [Land; 4] = [Land::Open, Land::Coast, Land::Cliffs, Land::Build];
    #[cfg(not(feature = "dev"))]
    pub const ALL: [Land; 2] = [Land::Open, Land::Build];

    pub fn key(self) -> &'static str {
        match self {
            Land::Open => "open",
            #[cfg(feature = "dev")]
            Land::Coast => "coast",
            #[cfg(feature = "dev")]
            Land::Cliffs => "cliffs",
            Land::Build => "build",
        }
    }

    /// The running water this land is shown with until it is changed.
    pub fn streams(self) -> Vec<Stream> {
        match self {
            Land::Open | Land::Build => Vec::new(),
            // A river that crosses the beach to the sea.
            #[cfg(feature = "dev")]
            Land::Coast => vec![Stream { from: [0.13, 0.0], to: [0.21, 0.36], width: 7.0, winding: 0.7, delta: 0.6 }],
            // A stream over the cliff top that falls into the cove.
            #[cfg(feature = "dev")]
            Land::Cliffs => vec![Stream { from: [0.895, 0.0], to: [0.878, 0.3], width: 5.0, winding: 0.6, delta: 0.5 }],
        }
    }
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SceneSetup {
    pub land: Land,
    /// Varies the shape of the land.
    pub seed: u32,
    pub streams: Vec<Stream>,
    /// Still water in hollows of the land, laid after the rivers.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub ponds: Vec<Pond>,
    /// Calm water and keep-dry areas, on any land.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub zones: Vec<Zone>,
    /// Where the build of `Land::Build` comes from and how it stands.
    pub build: BuildPlacement,
    /// An image whose normal map (or grey heights) gives the small ripples,
    /// when the pattern source is an image.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub pattern_image: String,
}

/// A build file brought in as land, and how it stands in the sea.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct BuildPlacement {
    /// The file the build is read from.
    pub file: String,
    /// How far up the build the water rests, as a share of its height:
    /// 0 stands it on the water, 1 sinks it to its top.
    pub waterline: f64,
    /// Blocks the build is moved east and south from the middle of the plot.
    pub offset: [i32; 2],
}

impl Default for BuildPlacement {
    fn default() -> Self {
        Self { file: String::new(), waterline: 0.2, offset: [0, 0] }
    }
}

impl SceneSetup {
    /// A land with the running water it is first shown with.
    pub fn of(land: Land, seed: u32) -> Self {
        Self { land, seed, streams: land.streams(), ponds: Vec::new(), zones: Vec::new(), build: BuildPlacement::default(), pattern_image: String::new() }
    }

    /// The land without its running water.
    #[cfg_attr(not(feature = "dev"), allow(unused_variables))]
    pub fn bare(&self, width: i32, length: i32, wave_direction: f64) -> Option<Terrain> {
        match self.land {
            Land::Open | Land::Build => None,
            #[cfg(feature = "dev")]
            Land::Coast => Some(coast_scene(width, length, self.seed, wave_direction)),
            #[cfg(feature = "dev")]
            Land::Cliffs => Some(cliff_scene(width, length, self.seed)),
        }
    }

    /// The ground with its running water and how each stream ended, where
    /// the land may be a build brought in from a file: `brought` is that
    /// build, already read. Open sea gives none.
    pub fn laid_with(
        &self,
        width: i32,
        length: i32,
        wave_direction: f64,
        brought: Option<&Arc<Build>>,
    ) -> Result<Option<(Terrain, Vec<Outcome>)>, String> {
        if self.land != Land::Build {
            return Ok(self.laid(width, length, wave_direction));
        }
        let build = brought.ok_or("no build file has been chosen for the land")?;
        let mut ground = Terrain::from_build(width, length, build, self.build.waterline, self.build.offset)?;
        // Rivers and ponds cut into the build where they run; a river lays
        // no delta over it, which would heap sand on the build.
        let before = ground.shape();
        let ends = self
            .streams
            .iter()
            .enumerate()
            .map(|(i, stream)| ground.add_stream(&Stream { delta: 0.0, ..stream.clone() }, self.stream_seed(i)))
            .collect();
        for pond in &self.ponds {
            ground.add_pond(pond);
        }
        ground.note_reworked(&before);
        Ok(Some((ground, ends)))
    }

    /// The ground for a plot of this size, or none for open sea.
    pub fn terrain(&self, width: i32, length: i32, wave_direction: f64) -> Option<Terrain> {
        self.laid(width, length, wave_direction).map(|(ground, _)| ground)
    }

    /// The same, with how each of the streams ended.
    pub fn laid(&self, width: i32, length: i32, wave_direction: f64) -> Option<(Terrain, Vec<Outcome>)> {
        let mut ground = self.bare(width, length, wave_direction)?;
        let ends = self.streams.iter().enumerate().map(|(i, stream)| ground.add_stream(stream, self.stream_seed(i))).collect();
        for pond in &self.ponds {
            ground.add_pond(pond);
        }
        Some((ground, ends))
    }

    fn stream_seed(&self, index: usize) -> u32 {
        self.seed ^ (index as u32 + 1).wrapping_mul(0x9E37)
    }

    /// The line one of the streams follows on a plot of this size, in
    /// blocks from the north-west corner.
    pub fn course(&self, index: usize, width: i32, length: i32) -> Vec<(f64, f64)> {
        self.streams.get(index).map(|s| s.course(width, length, self.stream_seed(index))).unwrap_or_default()
    }

    pub fn to_text(&self) -> String {
        serde_json::to_string_pretty(self).expect("a scene always serialises")
    }

    pub fn from_text(text: &str) -> Result<Self, String> {
        serde_json::from_str(text).map_err(|e| e.to_string())
    }
}
