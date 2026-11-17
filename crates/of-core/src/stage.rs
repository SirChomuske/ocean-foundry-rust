//! Stage fingerprints and the "out of date" rule.
//!
//! A stage's fingerprint is a hash of every parameter value that can change
//! that stage's result: its own parameters and those of all earlier stages,
//! for the active mode only. A bake records the fingerprints it was made
//! from. The displayed build is current only while those still match.

use crate::params::{self, Mode, ParamValues, Stage};
use serde::{Deserialize, Serialize};

/// Bumped whenever an algorithm change makes old results incomparable, so a
/// build baked by an older version is never reported as current.
pub const PIPELINE_REVISION: u32 = 1;

pub fn fingerprint(values: &ParamValues, mode: Mode, stage: Stage) -> String {
    let mut h = blake3::Hasher::new();
    h.update(format!("ocean-foundry/{PIPELINE_REVISION}/{}/{}\n", mode.key(), stage.key()).as_bytes());
    // Registry order is fixed, so the hash does not depend on map ordering.
    for def in params::for_mode(mode).filter(|d| d.stage <= stage) {
        h.update(def.id.as_bytes());
        h.update(b"=");
        h.update(values.get(def).canonical().as_bytes());
        h.update(b"\n");
    }
    h.finalize().to_hex()[..32].to_string()
}

/// Fingerprints of all stages at the moment a bake finished.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BakeStamp {
    pub mode: Mode,
    pub stages: Vec<String>,
}

impl BakeStamp {
    pub fn capture(values: &ParamValues, mode: Mode) -> Self {
        Self { mode, stages: Stage::ALL.iter().map(|s| fingerprint(values, mode, *s)).collect() }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Freshness {
    /// Nothing has been baked for this mode.
    NeverBaked,
    /// The baked build matches the current settings and may be exported.
    Current,
    /// Settings changed since the bake. Holds the earliest stage affected.
    Stale(Stage),
}

impl Freshness {
    pub fn can_export(self) -> bool {
        self == Freshness::Current
    }
}

pub fn freshness(values: &ParamValues, mode: Mode, last_bake: Option<&BakeStamp>) -> Freshness {
    let Some(stamp) = last_bake.filter(|s| s.mode == mode) else {
        return Freshness::NeverBaked;
    };
    for (i, stage) in Stage::ALL.iter().enumerate() {
        if stamp.stages.get(i).map(String::as_str) != Some(fingerprint(values, mode, *stage).as_str()) {
            return Freshness::Stale(*stage);
        }
    }
    Freshness::Current
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::params::{find, Value};

    #[test]
    fn unbaked_scene_cannot_export() {
        let v = ParamValues::default();
        assert_eq!(freshness(&v, Mode::Ocean, None), Freshness::NeverBaked);
        assert!(!Freshness::NeverBaked.can_export());
    }

    #[test]
    fn changing_an_input_marks_the_build_stale_from_its_stage() {
        let mut v = ParamValues::default();
        let stamp = BakeStamp::capture(&v, Mode::Ocean);
        assert_eq!(freshness(&v, Mode::Ocean, Some(&stamp)), Freshness::Current);

        // A block-choice change leaves every earlier stage reusable.
        v.set(find("ocean.foundation_colour").unwrap(), Value::Choice("blue".into()));
        assert_eq!(freshness(&v, Mode::Ocean, Some(&stamp)), Freshness::Stale(Stage::Blocks));

        // A wave-shape change invalidates the water result and everything after.
        v.set(find("ocean.wave_height").unwrap(), Value::Float(12.0));
        assert_eq!(freshness(&v, Mode::Ocean, Some(&stamp)), Freshness::Stale(Stage::Water));
        assert_eq!(fingerprint(&v, Mode::Ocean, Stage::Scene), stamp.stages[0], "scene must stay reusable");
    }

    #[test]
    fn restoring_the_value_makes_it_current_again() {
        let mut v = ParamValues::default();
        let stamp = BakeStamp::capture(&v, Mode::Ocean);
        let d = find("ocean.direction").unwrap();
        v.set(d, Value::Float(90.0));
        assert!(!freshness(&v, Mode::Ocean, Some(&stamp)).can_export());
        v.set(d, Value::Float(0.0));
        assert!(freshness(&v, Mode::Ocean, Some(&stamp)).can_export());
    }

    #[cfg(feature = "dev")]
    #[test]
    fn modes_do_not_invalidate_each_other() {
        let mut v = ParamValues::default();
        let stamp = BakeStamp::capture(&v, Mode::Ocean);
        v.set(find("lava.glow").unwrap(), Value::Float(1.0));
        assert_eq!(freshness(&v, Mode::Ocean, Some(&stamp)), Freshness::Current);
        // An ocean bake says nothing about lava.
        assert_eq!(freshness(&v, Mode::Lava, Some(&stamp)), Freshness::NeverBaked);
    }
}
