//! The parameter registry.
//!
//! Every user-facing setting is declared once here. The declaration drives the
//! control shown in the UI, its label and hover-help text (looked up by id in
//! the language tables), its saved value, and which pipeline stage it
//! invalidates when changed.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    Ocean,
    /// On hold: only in the development build (feature "dev").
    #[cfg(feature = "dev")]
    Lava,
}

impl Mode {
    #[cfg(feature = "dev")]
    pub const ALL: [Mode; 2] = [Mode::Ocean, Mode::Lava];
    #[cfg(not(feature = "dev"))]
    pub const ALL: [Mode; 1] = [Mode::Ocean];

    pub fn key(self) -> &'static str {
        match self {
            Mode::Ocean => "ocean",
            #[cfg(feature = "dev")]
            Mode::Lava => "lava",
        }
    }
}

/// Pipeline stages in dependency order. A parameter belongs to the earliest
/// stage whose result it changes; every later stage is then out of date too.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Stage {
    Scene = 0,
    Water = 1,
    Moment = 2,
    Geometry = 3,
    Appearance = 4,
    Blocks = 5,
}

impl Stage {
    pub const ALL: [Stage; 6] = [
        Stage::Scene,
        Stage::Water,
        Stage::Moment,
        Stage::Geometry,
        Stage::Appearance,
        Stage::Blocks,
    ];

    pub fn key(self) -> &'static str {
        match self {
            Stage::Scene => "scene",
            Stage::Water => "water",
            Stage::Moment => "moment",
            Stage::Geometry => "geometry",
            Stage::Appearance => "appearance",
            Stage::Blocks => "blocks",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Kind {
    Float { min: f64, max: f64, default: f64, decimals: u8 },
    Int { min: i64, max: i64, default: i64 },
    /// A reproducible random seed with a randomise button.
    Seed { default: i64 },
    /// One of a fixed list; option labels come from `param.<id>.opt.<option>`,
    /// or from `shared_options` when set (for example `colour.<option>`).
    Choice { options: &'static [&'static str], default: &'static str, shared_options: Option<&'static str> },
    Bool { default: bool },
    /// A colour, kept as text in the form `#rrggbb`.
    Colour { default: &'static str },
}

/// Read `#rrggbb` (the `#` is optional, letters in either case).
pub fn parse_colour(text: &str) -> Option<[u8; 3]> {
    let hex = text.trim().trim_start_matches('#');
    if hex.len() != 6 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let part = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).ok();
    Some([part(0)?, part(2)?, part(4)?])
}

pub fn colour_text(rgb: [u8; 3]) -> String {
    format!("#{:02x}{:02x}{:02x}", rgb[0], rgb[1], rgb[2])
}

#[derive(Debug, Clone, Copy)]
pub struct ParamDef {
    /// Stable id, also the key in saved settings. Prefixed by mode.
    pub id: &'static str,
    pub mode: Mode,
    /// Section the control appears under; label is `group.<group>`.
    pub group: &'static str,
    pub stage: Stage,
    /// Advanced parameters live in the collapsed advanced panel.
    pub advanced: bool,
    pub kind: Kind,
}

impl ParamDef {
    pub fn label_key(&self) -> String {
        format!("param.{}.label", self.id)
    }
    pub fn help_key(&self) -> String {
        format!("param.{}.help", self.id)
    }
    pub fn option_key(&self, option: &str) -> String {
        match self.kind {
            Kind::Choice { shared_options: Some(prefix), .. } => format!("{prefix}.{option}"),
            _ => format!("param.{}.opt.{}", self.id, option),
        }
    }
    pub fn default_value(&self) -> Value {
        match self.kind {
            Kind::Float { default, .. } => Value::Float(default),
            Kind::Int { default, .. } | Kind::Seed { default } => Value::Int(default),
            Kind::Choice { default, .. } => Value::Choice(default.to_string()),
            Kind::Bool { default } => Value::Bool(default),
            Kind::Colour { default } => Value::Choice(default.to_string()),
        }
    }
    /// Clamp or reject a value so it is always valid for this parameter.
    pub fn sanitise(&self, v: &Value) -> Value {
        match (self.kind, v) {
            (Kind::Float { min, max, .. }, Value::Float(x)) if x.is_finite() => Value::Float(x.clamp(min, max)),
            (Kind::Float { min, max, .. }, Value::Int(x)) => Value::Float((*x as f64).clamp(min, max)),
            (Kind::Int { min, max, .. }, Value::Int(x)) => Value::Int((*x).clamp(min, max)),
            (Kind::Seed { .. }, Value::Int(x)) => Value::Int((*x).clamp(0, SEED_MAX)),
            (Kind::Choice { options, .. }, Value::Choice(s)) if options.contains(&s.as_str()) => v.clone(),
            (Kind::Bool { .. }, Value::Bool(_)) => v.clone(),
            (Kind::Colour { .. }, Value::Choice(s)) => match parse_colour(s) {
                Some(rgb) => Value::Choice(colour_text(rgb)),
                None => self.default_value(),
            },
            _ => self.default_value(),
        }
    }
}

/// Seeds are kept within 32 bits so they fit every generator and every UI box.
pub const SEED_MAX: i64 = u32::MAX as i64;

/// The 16 Minecraft dye colours, in the game's own order.
pub const DYE_COLOURS: [&str; 16] = [
    "white", "orange", "magenta", "light_blue", "yellow", "lime", "pink", "gray", "light_gray", "cyan", "purple",
    "blue", "brown", "green", "red", "black",
];

const fn float(min: f64, max: f64, default: f64, decimals: u8) -> Kind {
    Kind::Float { min, max, default, decimals }
}
const fn int(min: i64, max: i64, default: i64) -> Kind {
    Kind::Int { min, max, default }
}
const fn unit(default: f64) -> Kind {
    Kind::Float { min: 0.0, max: 1.0, default, decimals: 2 }
}

/// Build height limits of the supported Minecraft version (Java 26.1.2).
pub const WORLD_MIN_Y: i64 = -64;
pub const WORLD_MAX_Y: i64 = 319;
/// Largest scene edge the app is designed for.
pub const MAX_EXTENT: i64 = 2048;

macro_rules! p {
    ($id:literal, $mode:ident, $group:literal, $stage:ident, $adv:literal, $kind:expr) => {
        ParamDef { id: $id, mode: Mode::$mode, group: $group, stage: Stage::$stage, advanced: $adv, kind: $kind }
    };
}

/// All parameters known to this build, in display order.
///
/// M0 declares the placement and foundation-shape parameters for both modes.
/// Later milestones append to this list; ids never change once released.
pub static REGISTRY: &[ParamDef] = &[
    // ---- Ocean: area ----
    p!("ocean.width", Ocean, "area", Scene, false, int(16, MAX_EXTENT, 256)),
    p!("ocean.length", Ocean, "area", Scene, false, int(16, MAX_EXTENT, 256)),
    p!("ocean.placement_y", Ocean, "area", Scene, false, int(WORLD_MIN_Y, WORLD_MAX_Y, 62)),
    // ---- Ocean: wave shape (foundation) ----
    p!("ocean.wave_height", Ocean, "shape", Water, false, float(0.0, 64.0, 8.0, 1)),
    p!("ocean.peak_distance", Ocean, "shape", Water, false, float(4.0, 512.0, 48.0, 0)),
    p!("ocean.direction", Ocean, "shape", Water, false, float(0.0, 360.0, 0.0, 0)),
    p!("ocean.irregularity", Ocean, "shape", Water, false, float(0.0, 1.0, 0.5, 2)),
    p!("ocean.swell_detail", Ocean, "shape", Water, false, float(0.0, 1.0, 0.4, 2)),
    p!("ocean.crest_sharpness", Ocean, "shape", Water, false, float(0.0, 1.0, 0.4, 2)),
    p!("ocean.moment", Ocean, "shape", Moment, false, float(0.0, 60.0, 0.0, 1)),
    p!("ocean.seed", Ocean, "shape", Water, false, Kind::Seed { default: 1 }),
    // ---- Ocean: construction ----
    p!(
        "ocean.build_style",
        Ocean,
        "construction",
        Geometry,
        false,
        Kind::Choice { options: &["shell", "filled"], default: "shell", shared_options: None }
    ),
    p!("ocean.shell_thickness", Ocean, "construction", Geometry, false, int(1, 16, 2)),
    p!(
        "ocean.foundation_colour",
        Ocean,
        "construction",
        Blocks,
        false,
        Kind::Choice { options: &DYE_COLOURS, default: "light_blue", shared_options: Some("colour") }
    ),
    p!("ocean.include_colour_glass", Ocean, "construction", Geometry, false, Kind::Bool { default: true }),
    // ---- Ocean: water colour (section 1.8 of the plan) ----
    // The water runs between two colours: the dark one where little light
    // reaches it, the light one where the light catches it. Any colour may
    // be chosen for either, black included.
    p!("ocean.dark_colour", Ocean, "colour", Appearance, false, Kind::Colour { default: "#1166b0" }),
    p!("ocean.light_colour", Ocean, "colour", Appearance, false, Kind::Colour { default: "#68c7e9" }),
    p!("ocean.daylight", Ocean, "colour", Appearance, false, unit(1.0)),
    p!("ocean.lit_strength", Ocean, "colour", Appearance, false, unit(0.85)),
    p!("ocean.depth_influence", Ocean, "colour", Appearance, false, unit(1.0)),
    p!("ocean.clarity", Ocean, "colour", Appearance, false, unit(0.5)),
    p!("ocean.depth_reach", Ocean, "colour", Appearance, false, float(1.0, 32.0, 7.0, 0)),
    p!("ocean.foam_amount", Ocean, "colour", Appearance, false, unit(0.5)),
    p!("ocean.foam_streaks", Ocean, "colour", Appearance, false, unit(0.0)),
    p!("ocean.foam_colour", Ocean, "colour", Appearance, false, Kind::Colour { default: "#eef4f8" }),
    // ---- Ocean: light ----
    p!("ocean.shading_strength", Ocean, "light", Appearance, false, unit(0.7)),
    p!("ocean.contrast", Ocean, "light", Appearance, false, float(0.0, 6.0, 1.6, 1)),
    p!("ocean.shade_midpoint", Ocean, "light", Appearance, false, unit(0.5)),
    p!("ocean.light_bearing", Ocean, "light", Appearance, false, float(0.0, 360.0, 45.0, 0)),
    p!("ocean.light_elevation", Ocean, "light", Appearance, false, float(1.0, 90.0, 38.0, 0)),
    p!("ocean.ripple_strength", Ocean, "light", Appearance, false, unit(0.5)),
    p!("ocean.shadow_strength", Ocean, "light", Appearance, false, unit(0.7)),
    p!("ocean.shadow_softness", Ocean, "light", Appearance, false, float(0.0, 12.0, 2.0, 1)),
    // ---- Ocean: where the small ripples in the shading come from ----
    p!(
        "ocean.pattern_source",
        Ocean,
        "ripples",
        Appearance,
        false,
        Kind::Choice { options: &["wind", "library", "spectrum", "image"], default: "wind", shared_options: None }
    ),
    p!(
        "ocean.pattern_name",
        Ocean,
        "ripples",
        Appearance,
        false,
        Kind::Choice { options: &["broken_crests", "fine_chop", "long_ripples", "cross_sea", "calm_swirls"], default: "broken_crests", shared_options: None }
    ),
    p!("ocean.pattern_spread", Ocean, "ripples", Appearance, false, float(0.0, 180.0, 40.0, 0)),
    p!("ocean.pattern_peak", Ocean, "ripples", Appearance, false, float(1.0, 10.0, 3.3, 1)),
    p!("ocean.pattern_span", Ocean, "ripples", Appearance, false, float(4.0, 1024.0, 64.0, 0)),
    p!("ocean.pattern_strength", Ocean, "ripples", Appearance, false, float(0.0, 3.0, 1.0, 2)),
    p!("ocean.pattern_rotation", Ocean, "ripples", Appearance, false, float(0.0, 360.0, 0.0, 0)),
    p!("ocean.pattern_invert", Ocean, "ripples", Appearance, false, Kind::Bool { default: false }),
    p!(
        "ocean.pattern_flip_y",
        Ocean,
        "ripples",
        Appearance,
        false,
        Kind::Choice { options: &["auto", "opengl", "directx"], default: "auto", shared_options: None }
    ),
    // ---- Ocean: a made-up sea floor for open sea ----
    p!("ocean.seabed", Ocean, "seabed", Water, false, Kind::Bool { default: false }),
    p!("ocean.seabed_shallowest", Ocean, "seabed", Water, false, float(0.0, 64.0, 2.0, 1)),
    p!("ocean.seabed_slope", Ocean, "seabed", Water, false, float(0.0, 2.0, 0.1, 2)),
    p!("ocean.seabed_direction", Ocean, "seabed", Water, false, float(0.0, 360.0, 0.0, 0)),
    // ---- Ocean: light thrown back by the surface, a layer of its own ----
    p!("ocean.reflection_strength", Ocean, "reflection", Appearance, false, unit(0.5)),
    p!("ocean.reflection_auto", Ocean, "reflection", Appearance, false, Kind::Bool { default: true }),
    p!("ocean.reflection_colour", Ocean, "reflection", Appearance, false, Kind::Colour { default: "#d9ecf5" }),
    p!("ocean.reflection_softness", Ocean, "reflection", Appearance, false, unit(0.15)),
    p!("ocean.view_bearing", Ocean, "reflection", Appearance, false, float(0.0, 360.0, 225.0, 0)),
    p!("ocean.view_elevation", Ocean, "reflection", Appearance, false, float(1.0, 90.0, 35.0, 0)),
    // ---- Ocean: block choice ----
    p!("ocean.glass_layers", Ocean, "blocks", Geometry, false, int(0, 3, 3)),
    p!("ocean.max_solids", Ocean, "blocks", Blocks, false, int(1, 32, 8)),
    p!("ocean.max_glass", Ocean, "blocks", Blocks, false, int(1, 16, 4)),
    p!(
        "ocean.dither_mode",
        Ocean,
        "blocks",
        Blocks,
        false,
        Kind::Choice { options: &["off", "patches", "bayer", "noise", "diffusion"], default: "patches", shared_options: None }
    ),
    p!("ocean.dither_strength", Ocean, "blocks", Blocks, false, unit(0.6)),
    p!("ocean.dither_grain", Ocean, "blocks", Blocks, false, int(1, 4, 1)),
    p!("ocean.dither_seed", Ocean, "blocks", Blocks, false, Kind::Seed { default: 1 }),
    p!("ocean.creative_blocks", Ocean, "blocks", Blocks, false, Kind::Bool { default: false }),
    // ---- Ocean: a swell from far away, under the wind sea ----
    p!("ocean.swell_height", Ocean, "swell", Water, false, float(0.0, 32.0, 0.0, 1)),
    p!("ocean.swell_length", Ocean, "swell", Water, false, float(16.0, 1024.0, 160.0, 0)),
    p!("ocean.swell_direction", Ocean, "swell", Water, false, float(0.0, 360.0, 30.0, 0)),
    // ---- Ocean: advanced ----
    p!("ocean.metres_per_block", Ocean, "physical", Scene, false, float(0.25, 4.0, 1.0, 2)),
    p!("ocean.flow_speed", Ocean, "physical", Scene, false, float(0.0, 10.0, 0.0, 1)),
    p!("ocean.flow_direction", Ocean, "physical", Scene, false, float(0.0, 360.0, 0.0, 0)),
    // ---- Lava: area ----
    #[cfg(feature = "dev")]
    p!("lava.width", Lava, "area", Scene, false, int(16, MAX_EXTENT, 128)),
    #[cfg(feature = "dev")]
    p!("lava.length", Lava, "area", Scene, false, int(16, MAX_EXTENT, 128)),
    #[cfg(feature = "dev")]
    p!("lava.placement_y", Lava, "area", Scene, false, int(WORLD_MIN_Y, WORLD_MAX_Y, 32)),
    // ---- Lava: pattern ----
    #[cfg(feature = "dev")]
    p!(
        "lava.pattern",
        Lava,
        "lava_pattern",
        Water,
        false,
        Kind::Choice { options: &["plates", "ropes", "pools"], default: "plates", shared_options: None }
    ),
    #[cfg(feature = "dev")]
    p!("lava.map_scale", Lava, "lava_pattern", Water, false, float(4.0, 512.0, 64.0, 0)),
    #[cfg(feature = "dev")]
    p!("lava.rotation", Lava, "lava_pattern", Water, false, float(0.0, 360.0, 0.0, 0)),
    #[cfg(feature = "dev")]
    p!("lava.seed", Lava, "lava_pattern", Water, false, Kind::Seed { default: 1 }),
    #[cfg(feature = "dev")]
    p!("lava.flow_distortion", Lava, "lava_pattern", Water, false, float(0.0, 1.0, 0.3, 2)),
    #[cfg(feature = "dev")]
    p!("lava.invert", Lava, "lava_pattern", Water, false, Kind::Bool { default: false }),
    // ---- Lava: surface ----
    #[cfg(feature = "dev")]
    p!("lava.crust_colour", Lava, "lava_surface", Appearance, false, Kind::Colour { default: "#2b2422" }),
    #[cfg(feature = "dev")]
    p!("lava.molten_colour", Lava, "lava_surface", Appearance, false, Kind::Colour { default: "#e8501a" }),
    #[cfg(feature = "dev")]
    p!("lava.hot_colour", Lava, "lava_surface", Appearance, false, Kind::Colour { default: "#ffd24a" }),
    #[cfg(feature = "dev")]
    p!("lava.crust_coverage", Lava, "lava_surface", Appearance, false, float(0.0, 1.0, 0.7, 2)),
    #[cfg(feature = "dev")]
    p!("lava.glow", Lava, "lava_surface", Appearance, false, float(0.0, 1.0, 0.6, 2)),
    #[cfg(feature = "dev")]
    p!(
        "lava.interpretation",
        Lava,
        "lava_surface",
        Appearance,
        false,
        Kind::Choice { options: &["slope", "fissures"], default: "fissures", shared_options: None }
    ),
    // ---- Lava: construction and block choice ----
    #[cfg(feature = "dev")]
    p!("lava.relief", Lava, "construction", Geometry, false, int(0, 8, 2)),
    #[cfg(feature = "dev")]
    p!(
        "lava.foundation_colour",
        Lava,
        "construction",
        Blocks,
        false,
        Kind::Choice { options: &DYE_COLOURS, default: "black", shared_options: Some("colour") }
    ),
    #[cfg(feature = "dev")]
    p!("lava.glass_layers", Lava, "blocks", Geometry, false, int(0, 3, 0)),
    #[cfg(feature = "dev")]
    p!("lava.max_solids", Lava, "blocks", Blocks, false, int(1, 32, 10)),
    #[cfg(feature = "dev")]
    p!("lava.max_glass", Lava, "blocks", Blocks, false, int(1, 16, 4)),
    #[cfg(feature = "dev")]
    p!("lava.dither_strength", Lava, "blocks", Blocks, false, unit(0.6)),
];

pub fn find(id: &str) -> Option<&'static ParamDef> {
    REGISTRY.iter().find(|p| p.id == id)
}

/// Read a value for this parameter from text, as typed after `id=` on a
/// command line.
pub fn parse_value(def: &ParamDef, text: &str) -> Result<Value, String> {
    let bad = || format!("'{text}' is not a valid value for {}", def.id);
    Ok(match def.kind {
        Kind::Float { .. } => Value::Float(text.parse().map_err(|_| bad())?),
        Kind::Int { .. } | Kind::Seed { .. } => Value::Int(text.parse().map_err(|_| bad())?),
        Kind::Bool { .. } => Value::Bool(text.parse().map_err(|_| bad())?),
        Kind::Colour { .. } => Value::Choice(colour_text(parse_colour(text).ok_or_else(bad)?)),
        Kind::Choice { options, .. } => {
            if !options.contains(&text) {
                return Err(bad());
            }
            Value::Choice(text.to_string())
        }
    })
}

/// Whether a control makes sense with the current settings. Irrelevant
/// controls are hidden; their saved values are kept for when they return.
pub fn is_relevant(def: &ParamDef, values: &ParamValues) -> bool {
    let choice_is = |id: &str, want: &str| values.get_id(id).and_then(|v| v.as_choice().map(|c| c == want)).unwrap_or(false);
    let on = |id: &str| values.get_id(id).and_then(|v| v.as_bool()).unwrap_or(false);
    let positive = |id: &str| values.get_id(id).and_then(|v| v.as_f64()).unwrap_or(0.0) > 0.0;
    match def.id {
        "ocean.shell_thickness" => choice_is("ocean.build_style", "shell"),
        // A current that does not run has no direction.
        "ocean.flow_direction" => positive("ocean.flow_speed"),
        "ocean.swell_length" | "ocean.swell_direction" => positive("ocean.swell_height"),
        #[cfg(feature = "dev")]
        "lava.max_glass" => positive("lava.glass_layers"),
        "ocean.seabed_shallowest" | "ocean.seabed_slope" | "ocean.seabed_direction" => on("ocean.seabed"),
        _ if def.mode == Mode::Ocean && matches!(def.group, "colour" | "light" | "reflection" | "blocks" | "ripples") => {
            // Nothing here does anything without the coloured surface, and the
            // sunlight settings do nothing under a fully overcast sky.
            on("ocean.include_colour_glass")
                && match def.id {
                    // Without sun the whole sea has the dark colour; the light
                    // one then shows only in reflections that take it.
                    "ocean.light_colour" => {
                        positive("ocean.lit_strength") || (positive("ocean.reflection_strength") && on("ocean.reflection_auto"))
                    }
                    // Wave shading and ripple shading stay: under cloud they
                    // still shade the waves by the light of the sky.
                    "ocean.depth_influence" | "ocean.contrast" | "ocean.shade_midpoint" | "ocean.light_bearing"
                    | "ocean.light_elevation" | "ocean.shadow_strength" => positive("ocean.lit_strength"),
                    "ocean.shadow_softness" => positive("ocean.lit_strength") && positive("ocean.shadow_strength"),
                    "ocean.depth_reach" => (positive("ocean.lit_strength") && positive("ocean.depth_influence")) || positive("ocean.clarity"),
                    "ocean.reflection_auto" | "ocean.view_elevation" => positive("ocean.reflection_strength"),
                    "ocean.reflection_colour" => positive("ocean.reflection_strength") && !on("ocean.reflection_auto"),
                    // The glint is the sun's; without sun there is none to spread.
                    "ocean.reflection_softness" => positive("ocean.reflection_strength") && positive("ocean.lit_strength"),
                    // Seen from straight above there is no side to look from.
                    "ocean.view_bearing" => {
                        positive("ocean.reflection_strength") && values.get_id("ocean.view_elevation").and_then(|v| v.as_f64()).unwrap_or(90.0) < 90.0
                    }
                    "ocean.max_glass" => positive("ocean.glass_layers"),
                    "ocean.dither_strength" => !choice_is("ocean.dither_mode", "off"),
                    "ocean.dither_grain" => choice_is("ocean.dither_mode", "bayer") || choice_is("ocean.dither_mode", "noise"),
                    "ocean.dither_seed" => choice_is("ocean.dither_mode", "noise"),
                    "ocean.foam_colour" => positive("ocean.foam_amount") || positive("ocean.foam_streaks"),
                    "ocean.pattern_source" => positive("ocean.ripple_strength"),
                    "ocean.pattern_name" => positive("ocean.ripple_strength") && choice_is("ocean.pattern_source", "library"),
                    "ocean.pattern_flip_y" => positive("ocean.ripple_strength") && choice_is("ocean.pattern_source", "image"),
                    "ocean.pattern_spread" | "ocean.pattern_peak" => {
                        positive("ocean.ripple_strength") && choice_is("ocean.pattern_source", "spectrum")
                    }
                    "ocean.pattern_span" | "ocean.pattern_strength" | "ocean.pattern_rotation" | "ocean.pattern_invert" => {
                        positive("ocean.ripple_strength") && !choice_is("ocean.pattern_source", "wind")
                    }
                    _ => true,
                }
        }
        _ => true,
    }
}

/// Parameters for one mode, in display order.
pub fn for_mode(mode: Mode) -> impl Iterator<Item = &'static ParamDef> {
    REGISTRY.iter().filter(move |p| p.mode == mode)
}

/// Group names for one mode in first-appearance order, split by advanced flag.
pub fn groups(mode: Mode, advanced: bool) -> Vec<&'static str> {
    let mut out: Vec<&'static str> = Vec::new();
    for p in for_mode(mode).filter(|p| p.advanced == advanced) {
        if !out.contains(&p.group) {
            out.push(p.group);
        }
    }
    out
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Value {
    Bool(bool),
    Int(i64),
    Float(f64),
    Choice(String),
}

impl Value {
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Value::Float(x) => Some(*x),
            Value::Int(x) => Some(*x as f64),
            _ => None,
        }
    }
    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Value::Int(x) => Some(*x),
            _ => None,
        }
    }
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Value::Bool(x) => Some(*x),
            _ => None,
        }
    }
    pub fn as_choice(&self) -> Option<&str> {
        match self {
            Value::Choice(s) => Some(s),
            _ => None,
        }
    }

    /// Text form used for fingerprints. Floats are written by exact bit
    /// pattern so that equal settings always hash equally.
    pub fn canonical(&self) -> String {
        match self {
            Value::Bool(b) => format!("b:{b}"),
            Value::Int(i) => format!("i:{i}"),
            // Normalise -0.0 to 0.0 so the two never count as a change.
            Value::Float(f) => format!("f:{:016x}", (if *f == 0.0 { 0.0f64 } else { *f }).to_bits()),
            Value::Choice(s) => format!("c:{s}"),
        }
    }
}

/// Current values of all parameters. Missing or invalid entries read as the
/// parameter's default, so old settings files keep working as the registry
/// grows.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ParamValues {
    map: BTreeMap<String, Value>,
}

impl ParamValues {
    pub fn get(&self, def: &ParamDef) -> Value {
        match self.map.get(def.id) {
            Some(v) => def.sanitise(v),
            None => def.default_value(),
        }
    }

    pub fn get_id(&self, id: &str) -> Option<Value> {
        find(id).map(|d| self.get(d))
    }

    /// Store a value. Returns true if the effective value changed.
    pub fn set(&mut self, def: &ParamDef, value: Value) -> bool {
        let new = def.sanitise(&value);
        if self.get(def) == new {
            return false;
        }
        self.map.insert(def.id.to_string(), new);
        true
    }

    pub fn reset(&mut self, def: &ParamDef) -> bool {
        let changed = self.get(def) != def.default_value();
        self.map.remove(def.id);
        changed
    }

    /// Drop entries this build does not know and clamp the rest.
    pub fn sanitise_all(&mut self) {
        self.map.retain(|k, _| find(k).is_some());
        for def in REGISTRY {
            if let Some(v) = self.map.get(def.id) {
                let clean = def.sanitise(v);
                self.map.insert(def.id.to_string(), clean);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_unique_and_prefixed_by_mode() {
        let mut seen = std::collections::HashSet::new();
        for p in REGISTRY {
            assert!(seen.insert(p.id), "duplicate id {}", p.id);
            assert!(p.id.starts_with(&format!("{}.", p.mode.key())), "{} lacks mode prefix", p.id);
        }
    }

    #[test]
    fn defaults_are_valid() {
        for p in REGISTRY {
            let d = p.default_value();
            assert_eq!(p.sanitise(&d), d, "default of {} is out of range", p.id);
        }
    }

    #[test]
    fn values_clamp_and_fall_back() {
        let w = find("ocean.width").unwrap();
        let mut v = ParamValues::default();
        assert!(v.set(w, Value::Int(999_999)));
        assert_eq!(v.get(w), Value::Int(MAX_EXTENT));
        assert!(!v.set(w, Value::Int(MAX_EXTENT)), "no change must report false");
        v.set(w, Value::Choice("nonsense".into()));
        assert_eq!(v.get(w), w.default_value());
    }

    #[test]
    fn colours_are_kept_as_clean_hex_text() {
        let c = find("ocean.dark_colour").unwrap();
        let mut v = ParamValues::default();
        assert_eq!(v.get(c), Value::Choice("#1166b0".into()));
        // Black is a colour like any other.
        assert!(v.set(c, Value::Choice("#000000".into())));
        assert_eq!(v.get(c), Value::Choice("#000000".into()));
        assert!(v.set(c, Value::Choice("FF8800".into())));
        assert_eq!(v.get(c), Value::Choice("#ff8800".into()));
        v.set(c, Value::Choice("not a colour".into()));
        assert_eq!(v.get(c), c.default_value());
        assert_eq!(parse_colour("#3E96cd"), Some([62, 150, 205]));
        assert_eq!(parse_colour("#12345"), None);
    }

    #[test]
    fn sunlight_settings_hide_when_they_can_do_nothing() {
        let mut v = ParamValues::default();
        let shown = |v: &ParamValues, id: &str| is_relevant(find(id).unwrap(), v);
        // Both water colours are there; reflections take the light colour
        // until they are given one of their own.
        assert!(shown(&v, "ocean.dark_colour") && shown(&v, "ocean.light_colour"));
        assert!(shown(&v, "ocean.contrast") && shown(&v, "ocean.shadow_softness") && shown(&v, "ocean.reflection_softness"));
        assert!(shown(&v, "ocean.reflection_auto") && !shown(&v, "ocean.reflection_colour"));
        v.set(find("ocean.lit_strength").unwrap(), Value::Float(0.0));
        assert!(!shown(&v, "ocean.contrast") && !shown(&v, "ocean.shadow_softness") && !shown(&v, "ocean.depth_influence"));
        assert!(!shown(&v, "ocean.reflection_softness"), "no sun, no glint to spread");
        // The sky still shades the waves and the ripples under cloud.
        assert!(shown(&v, "ocean.shading_strength") && shown(&v, "ocean.ripple_strength"));
        assert!(shown(&v, "ocean.dark_colour") && shown(&v, "ocean.lit_strength") && shown(&v, "ocean.foam_amount"));
        // The light colour still tints the reflections.
        assert!(shown(&v, "ocean.light_colour"));
        v.set(find("ocean.reflection_auto").unwrap(), Value::Bool(false));
        assert!(shown(&v, "ocean.reflection_colour") && !shown(&v, "ocean.light_colour"));
        assert!(shown(&v, "ocean.view_bearing") && shown(&v, "ocean.view_elevation"));
        v.set(find("ocean.view_elevation").unwrap(), Value::Float(90.0));
        assert!(!shown(&v, "ocean.view_bearing"));
        v.set(find("ocean.view_elevation").unwrap(), Value::Float(40.0));
        assert!(shown(&v, "ocean.view_bearing"));
        v.set(find("ocean.reflection_strength").unwrap(), Value::Float(0.0));
        assert!(!shown(&v, "ocean.view_bearing") && !shown(&v, "ocean.reflection_colour") && !shown(&v, "ocean.reflection_auto"));
        v.set(find("ocean.include_colour_glass").unwrap(), Value::Bool(false));
        assert!(!shown(&v, "ocean.dark_colour") && !shown(&v, "ocean.max_solids"));
    }

    #[test]
    fn a_setting_the_method_does_not_use_yet_is_not_offered() {
        let v = ParamValues::default();
        assert!(is_relevant(find("ocean.metres_per_block").unwrap(), &v));
        assert!(!is_relevant(find("ocean.flow_direction").unwrap(), &v));
    }

    #[cfg(feature = "dev")]
    #[test]
    fn modes_keep_separate_settings() {
        let mut v = ParamValues::default();
        v.set(find("ocean.seed").unwrap(), Value::Int(42));
        assert_eq!(v.get(find("lava.seed").unwrap()), Value::Int(1));
    }

    #[test]
    fn foundation_offers_all_sixteen_concretes() {
        match find("ocean.foundation_colour").unwrap().kind {
            Kind::Choice { options, .. } => assert_eq!(options.len(), 16),
            _ => panic!("wrong kind"),
        }
    }
}
