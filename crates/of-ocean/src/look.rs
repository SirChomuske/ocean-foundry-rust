//! How the water is coloured and dithered.

/// How the water is coloured. See section 1.8 of the plan.
#[derive(Debug, Clone, PartialEq)]
pub struct Look {
    /// The water where little light reaches it: deep troughs, faces turned
    /// from the sun, shadow, the whole sea under cloud. Any colour, black included.
    pub dark: [u8; 3],
    /// The water where the light catches it. Every column lies somewhere
    /// between the two.
    pub light: [u8; 3],
    /// How much light there is altogether: 1 is full day, at 0 the sea is black.
    pub daylight: f64,
    /// How much sun there is: 0 is fully overcast and leaves the dark colour everywhere.
    pub lit_strength: f64,
    /// How directional the sunlight is: 0 tints every face evenly, 1 lets faces turned away fall toward the dark colour.
    pub shading: f64,
    /// How quickly a slope goes from shaded to fully lit.
    pub contrast: f64,
    /// Where level water in full sun sits between the dark and the light colour: 0.5 is half way.
    pub midpoint: f64,
    /// How dark the shadows are that land and tall waves cast on the water.
    pub shadow: f64,
    /// How far the edges of those shadows are blurred, in blocks.
    pub shadow_softness: f64,
    /// How much of the sea floor shows through shallow water.
    pub clarity: f64,
    /// Light thrown back by the surface itself, laid over the water's colour.
    pub reflection: f64,
    /// Reflections take a paler tint of the light colour rather than `reflection_colour`.
    pub reflection_auto: bool,
    pub reflection_colour: [u8; 3],
    /// 0: a small sharp glint. 1: a broad soft sheen.
    pub reflection_softness: f64,
    /// Where the onlooker stands for whom the reflections are worked out, as for the sun.
    pub view_azimuth: f64,
    pub view_elevation: f64,
    /// Where the sun stands, degrees. 0 is +X, 90 is +Z.
    pub sun_azimuth: f64,
    /// Height of the sun above the horizon, degrees.
    pub sun_elevation: f64,
    /// Extra sunlit tint in shallow water: 1 doubles it at the waterline, never past the lit colour.
    pub depth_influence: f64,
    /// Depth in blocks at which that extra tint has faded out.
    pub depth_reach: f64,
    /// Strength of the small ripples, which exist only as shading.
    pub ripple: f64,
    /// Amount of white water at shores and on the tallest crests. 0 removes it.
    pub foam: f64,
    /// Amount of foam left lying on the water: lace and wind streaks.
    pub streaks: f64,
    /// How much white water a current leaves where it strikes land, 0 to 1.
    pub impact_foam: f64,
    /// How high a breaking wave throws its white water as it meets the shore
    pub spray: f64,
    /// How much foam a current drags away downstream from land, 0 to 1.
    pub wake_foam: f64,
    /// How far downstream that foam is drawn out, as a multiple of its life.
    pub wake_length: f64,
    /// How big the trailing foam reads, both how far it is drawn out and
    /// how wide it fans out. Multiplies `wake_length`.
    pub wake_scale: f64,
    /// Stained glass layers over the water, 0 to 3.
    pub glass_layers: u32,
    /// Most solid block types the coloured surface may use.
    pub max_solids: usize,
    /// Most stained glass colours it may use.
    pub max_glass: usize,
    /// Strength of the dithering that blends neighbouring block colours.
    pub dither: f64,
    /// How the two colours of a blend are laid out, how many blocks across
    /// one cell of that layout is, and the seed of the random layout.
    pub dither_mode: Dither,
    pub dither_grain: i32,
    pub dither_seed: u32,
    /// The colour of white water.
    pub foam_colour: [u8; 3],
}

/// How dithering lays out the two block colours of a blend.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dither {
    /// No blending: clean bands.
    Off,
    /// Patches a few blocks across, drawn out along the crests.
    Patches,
    /// A fixed fine pattern.
    Bayer,
    /// A random scatter.
    Noise,
    /// Each block passes what it could not show on to the next
    /// (Floyd-Steinberg).
    Diffusion,
}

impl Default for Look {
    fn default() -> Self {
        Self {
            dark: [17, 102, 176],
            light: [104, 199, 233],
            daylight: 1.0,
            lit_strength: 0.85,
            shading: 0.7,
            contrast: 1.6,
            midpoint: 0.5,
            shadow: 0.7,
            shadow_softness: 2.0,
            clarity: 0.5,
            reflection: 0.5,
            reflection_auto: true,
            reflection_colour: [217, 236, 245],
            reflection_softness: 0.15,
            view_azimuth: 225.0,
            view_elevation: 35.0,
            sun_azimuth: 45.0,
            sun_elevation: 38.0,
            depth_influence: 1.0,
            depth_reach: 7.0,
            ripple: 0.5,
            foam: 0.5,
            streaks: 0.0,
            impact_foam: 0.5,
            wake_foam: 0.5,
            wake_length: 1.0,
            wake_scale: 1.0,
            spray: 0.5,
            glass_layers: 3,
            max_solids: 8,
            max_glass: 4,
            dither: 0.6,
            dither_mode: Dither::Patches,
            dither_grain: 1,
            dither_seed: 1,
            foam_colour: [238, 244, 248],
        }
    }
}
