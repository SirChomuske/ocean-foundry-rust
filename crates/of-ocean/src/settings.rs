//! The settings a bake reads: what the sea looks like and what it is made of.

use crate::look::{Dither, Look};
use crate::pattern::{self, Pattern};
use crate::waves::WaveSettings;
use crate::zone::Zone;
use of_core::params::{self, ParamValues, Value};
#[derive(Debug, Clone, PartialEq)]
pub struct OceanSettings {
    /// World X and Z of the plot's north-west corner.
    pub origin: [i32; 2],
    pub width: i32,
    pub length: i32,
    /// The Minecraft Y level the water rests at.
    pub placement_y: i32,
    pub waves: WaveSettings,
    /// Filled to the base rather than a hollow shell.
    pub filled: bool,
    pub thickness: i32,
    /// Dye colour of the foundation concrete.
    pub foundation: String,
    pub colour_glass: bool,
    /// The size of one block in metres. Sets how fast the waves are for
    /// their size, and so what a current does to them.
    pub metres_per_block: f64,
    /// A current through the sea: its speed in metres a second, and the
    /// way it runs in degrees as for the waves.
    pub flow_speed: f64,
    pub flow_direction: f64,
    pub look: Look,
    /// Blocks the coloured surface may not use.
    pub excluded: Vec<String>,
    /// Blocks only to be had in creative mode may be used too.
    pub creative_blocks: bool,
    /// Where the small ripples in the shading come from.
    pub pattern: Pattern,
    /// A made-up sea floor for open sea with no terrain.
    pub seabed: Option<Seabed>,
    /// Calm water and keep-dry areas.
    pub zones: Vec<Zone>,
}

/// A sea floor that is only imagined: it changes how deep the water looks
/// and where waves break, never the blocks under the water.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Seabed {
    /// Depth in blocks at the shallow edge.
    pub shallowest: f64,
    /// Blocks deeper for every block out from that edge.
    pub slope: f64,
    /// The way the floor falls away, degrees as for the waves.
    pub direction: f64,
}

impl OceanSettings {
    pub fn from_params(values: &ParamValues) -> Self {
        let get = |id: &str| params::find(id).map(|d| values.get(d));
        let num = |id: &str| match get(id) {
            Some(Value::Float(x)) => x,
            Some(Value::Int(i)) => i as f64,
            _ => 0.0,
        };
        let choice = |id: &str| match get(id) {
            Some(Value::Choice(c)) => c,
            _ => String::new(),
        };
        Self {
            origin: [0, 0],
            width: num("ocean.width") as i32,
            length: num("ocean.length") as i32,
            placement_y: num("ocean.placement_y") as i32,
            waves: WaveSettings {
                height: num("ocean.wave_height"),
                peak_distance: num("ocean.peak_distance"),
                direction: num("ocean.direction"),
                irregularity: num("ocean.irregularity"),
                swell_detail: num("ocean.swell_detail"),
                crest_sharpness: num("ocean.crest_sharpness"),
                seed: num("ocean.seed") as u32,
                moment: num("ocean.moment"),
                swell_height: num("ocean.swell_height"),
                swell_length: num("ocean.swell_length"),
                swell_direction: num("ocean.swell_direction"),
            },
            filled: choice("ocean.build_style") == "filled",
            thickness: (num("ocean.shell_thickness") as i32).max(1),
            foundation: choice("ocean.foundation_colour"),
            colour_glass: matches!(get("ocean.include_colour_glass"), Some(Value::Bool(true))),
            metres_per_block: num("ocean.metres_per_block").max(0.01),
            flow_speed: num("ocean.flow_speed"),
            flow_direction: num("ocean.flow_direction"),
            look: Look {
                dark: params::parse_colour(&choice("ocean.dark_colour")).unwrap_or(Look::default().dark),
                light: params::parse_colour(&choice("ocean.light_colour")).unwrap_or(Look::default().light),
                daylight: num("ocean.daylight"),
                lit_strength: num("ocean.lit_strength"),
                shading: num("ocean.shading_strength"),
                contrast: num("ocean.contrast"),
                midpoint: num("ocean.shade_midpoint"),
                shadow: num("ocean.shadow_strength"),
                shadow_softness: num("ocean.shadow_softness"),
                clarity: num("ocean.clarity"),
                reflection: num("ocean.reflection_strength"),
                reflection_auto: matches!(get("ocean.reflection_auto"), Some(Value::Bool(true))),
                reflection_colour: params::parse_colour(&choice("ocean.reflection_colour")).unwrap_or(Look::default().reflection_colour),
                reflection_softness: num("ocean.reflection_softness"),
                view_azimuth: num("ocean.view_bearing"),
                view_elevation: num("ocean.view_elevation"),
                sun_azimuth: num("ocean.light_bearing"),
                sun_elevation: num("ocean.light_elevation"),
                depth_influence: num("ocean.depth_influence"),
                depth_reach: num("ocean.depth_reach").max(0.5),
                ripple: num("ocean.ripple_strength"),
                foam: num("ocean.foam_amount"),
                streaks: num("ocean.foam_streaks"),
                impact_foam: num("ocean.impact_foam"),
                wake_foam: num("ocean.wake_foam"),
                wake_length: num("ocean.wake_length"),
                wake_scale: num("ocean.wake_scale"),
                spray: num("ocean.spray"),
                glass_layers: num("ocean.glass_layers") as u32,
                max_solids: num("ocean.max_solids") as usize,
                max_glass: num("ocean.max_glass") as usize,
                dither: num("ocean.dither_strength"),
                dither_mode: match choice("ocean.dither_mode").as_str() {
                    "off" => Dither::Off,
                    "noise" => Dither::Noise,
                    "diffusion" => Dither::Diffusion,
                    "bayer" => Dither::Bayer,
                    _ => Dither::Patches,
                },
                dither_grain: (num("ocean.dither_grain") as i32).max(1),
                dither_seed: num("ocean.dither_seed") as u32,
                foam_colour: params::parse_colour(&choice("ocean.foam_colour")).unwrap_or(Look::default().foam_colour),
            },
            excluded: Vec::new(),
            creative_blocks: matches!(get("ocean.creative_blocks"), Some(Value::Bool(true))),
            pattern: Pattern {
                source: match choice("ocean.pattern_source").as_str() {
                    "library" => pattern::Source::Library,
                    "image" => pattern::Source::Image,
                    "spectrum" => pattern::Source::Spectrum,
                    _ => pattern::Source::Wind,
                },
                name: choice("ocean.pattern_name"),
                spread: num("ocean.pattern_spread"),
                peak: num("ocean.pattern_peak").max(1.0),
                span: num("ocean.pattern_span").max(1.0),
                strength: num("ocean.pattern_strength"),
                rotation: num("ocean.pattern_rotation"),
                invert: matches!(get("ocean.pattern_invert"), Some(Value::Bool(true))),
                flip: match choice("ocean.pattern_flip_y").as_str() {
                    "opengl" => pattern::FlipY::Up,
                    "directx" => pattern::FlipY::Down,
                    _ => pattern::FlipY::Auto,
                },
                image: None,
            },
            seabed: matches!(get("ocean.seabed"), Some(Value::Bool(true))).then(|| Seabed {
                shallowest: num("ocean.seabed_shallowest").max(0.0),
                slope: num("ocean.seabed_slope").max(0.0),
                direction: num("ocean.seabed_direction"),
            }),
            zones: Vec::new(),
        }
    }
}

