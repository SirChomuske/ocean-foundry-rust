//! The generator's internal numbers, gathered in one place so they can be
//! changed without rebuilding the program. The app keeps them in a text file
//! (`ocean-tuning.json`) and reads it again at every bake.
//!
//! These are not user settings: they are the knobs of the method itself,
//! kept adjustable while the method is still being developed.

use serde::{Deserialize, Serialize};

pub const TUNING_FILE: &str = "ocean-tuning.json";
/// Where the app puts a tuning file written for an earlier method.
pub const TUNING_BACKUP: &str = "ocean-tuning.previous.json";

/// Raised whenever the method changes so much that the numbers of an older
/// tuning file no longer mean the same thing.
///
/// 1: wave trains, whitecaps by height. 2: wave spectrum, breaking by
/// steepness with foam trails, waves bending and sheltering near land.
/// 3: colour led by the direction of the sun. 4: the water runs between a
/// chosen dark and light colour (no shade or thin-water colours worked out
/// from one water colour), and "Daylight" 0 is black. 5: the crests bend as
/// they travel (wander), the sea a shore throws back reaches a quarter of a
/// peak length and is spread out, the current runs faster where two shores
/// squeeze it and draws its foam out, and a fast current keeps more of the
/// foam it drags. 6: thick foam is cut into plates with cracks of open water
/// between them, so a wake reads as white lace rather than one sheet of paint.
/// 7: the wake keeps closer to what made it — its wedge fades at the sides
/// and the crests that fan out are lost sooner — and the water is thrown up
/// against a face the current strikes rather than only whitened on it.
/// 8: crests bend around land instead of bunching up where a passage narrows,
/// so a sea that came through a gap is the same length as the water beside it.
/// 9: the crest delay is also held to a gentle slope across the run, so the
/// edge of a shadow bends the crests instead of ruling a seam across the water.
/// 10: foam in a lee keeps its share of whitecaps with the wind waves that
/// are left there, and its threshold follows the spread of the sea that is
/// actually present, so a shaded sea still breaks and shows white.
/// 12: the wave directions bunch on a curve of "Irregularity" that keeps a
/// half-irregular sea more orderly than a straight line from calm to wild
/// does, so crests run in longer ridges instead of in patches.
pub const TUNING_VERSION: u32 = 12;

/// The open sea: a wave spectrum sampled as many small waves.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct WaveTuning {
    /// How many waves the sea is the sum of. More costs bake time.
    pub components: u32,
    /// How sharply the spectrum peaks at the main waves (the JONSWAP gamma):
    /// 1 is a fully developed sea, 3.3 the measured average, 7 very peaked.
    pub peak_enhancement: f64,
    /// Extra narrowing of the peak when "Irregularity" is 0, so that a
    /// regular swell is nearly one wavelength. No effect at irregularity 1.
    pub narrowing_at_calm: f64,
    /// How narrowly the wave directions are bunched (the "s" of cos^2s
    /// spreading) at irregularity 0 and at irregularity 1. Larger is
    /// narrower: 300 gives long straight crests, 4 a confused sea.
    pub spread_at_calm: f64,
    pub spread_at_wild: f64,
    /// How the wave directions bunch as "Irregularity" rises. 1 is a
    /// straight line from calm to wild; above 1 keeps the sea more orderly
    /// for longer, so a half-irregular sea still runs in long ridges.
    pub spread_curve: f64,
    /// The short waves that "Swell detail" adds start at this multiple of
    /// the main wave frequency (1.5 is a wavelength of about 0.44).
    pub tail_from: f64,
    /// Share of the sea's energy in those short waves at full "Swell detail".
    pub tail_energy: f64,
    /// Share of the components spent on them.
    pub tail_components: f64,
    /// Waves shorter than this many blocks cannot be built and are left out.
    pub shortest_length: f64,
    /// What "Wave height" measures, in standard deviations of the water
    /// level: 4 is the average of the highest third of the waves (the
    /// significant wave height of oceanography), 5.1 of the highest tenth.
    /// The rare tallest wave reaches about 7.5. Smaller makes a taller sea.
    pub height_in_deviations: f64,
    /// The colour of the water follows the sea with its short waves
    /// smoothed away, so it shows the swell rather than every wavelet.
    /// Waves shorter than about six times this share of the peak distance
    /// leave no trace in the colour. 0 colours every wavelet. The second
    /// number is the least smoothing, in blocks.
    pub tone_smoothing: f64,
    pub tone_smoothing_least: f64,
    /// How far full "Crest sharpness" moves the water toward the crests: 1
    /// is a true trochoid.
    pub chop_most: f64,
    /// How far the crests wander sideways from the straight lines a sum of
    /// plane waves would make, as a share of the peak wavelength. Waves in a
    /// row of parallel ridges read as a pattern; a real sea bends.
    pub wander: f64,
    /// Over how many peak wavelengths a bend comes and goes.
    pub wander_size: f64,
    /// Sharpening stops before the steepest crests fold over: 1 would let
    /// them become cusps.
    pub cusp_limit: f64,
}

impl Default for WaveTuning {
    fn default() -> Self {
        Self {
            components: 64,
            peak_enhancement: 3.3,
            narrowing_at_calm: 8.0,
            spread_at_calm: 300.0,
            spread_at_wild: 4.0,
            spread_curve: 1.35,
            tail_from: 1.5,
            tail_energy: 0.2,
            tail_components: 0.35,
            shortest_length: 6.0,
            height_in_deviations: 5.0,
            tone_smoothing: 0.06,
            tone_smoothing_least: 1.5,
            chop_most: 1.0,
            wander: 0.35,
            wander_size: 1.6,
            cusp_limit: 0.8,
        }
    }
}

/// What land does to the waves (only with terrain).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct NearLandTuning {
    /// The tallest waves the water can carry, as a share of its depth.
    /// Measured values run from 0.6 to 0.8; taller waves break.
    pub breaker_ratio: f64,
    /// Share of their height waves keep at the waterline.
    pub height_at_shore: f64,
    /// Blocks over which the depth is averaged before the waves feel it.
    pub depth_smoothing: u32,
    /// Waves are slowed as if the water were never shallower than this many blocks.
    pub least_depth: f64,
    /// The most the crests can bunch up: 3 lets them come three times closer together.
    pub slowest: f64,
    /// The most the crest delay may change from block to block along the run
    /// of the waves: 0.12 shortens a wave by about a seventh and no more, so
    /// the sea keeps its own length through a narrow passage.
    pub arrival_slope: f64,
    /// The most the crest delay may change from block to block across the run.
    /// Sideways the delay only tilts the crests instead of shortening them, so
    /// a larger slope is tolerable; 0.35 still spreads the edge of a shadow
    /// over a few blocks per block of delay rather than ruling a seam.
    pub arrival_side: f64,
    /// Shelter is found by looking upwind along this many lines, fanned out
    /// by `shelter_fan` degrees to each side, up to `shelter_reach` main wavelengths.
    pub shelter_rays: u32,
    pub shelter_fan: f64,
    pub shelter_reach: f64,
    /// Share of the wave energy that reaches around corners whatever the
    /// spread of the sea, as waves bend into the shelter of land.
    pub bend_share: f64,
    /// Share of their height the long waves keep in complete shelter.
    pub shelter_floor: f64,
    /// Short waves grow back behind land over about this many blocks.
    pub chop_regrow: f64,
}

impl Default for NearLandTuning {
    fn default() -> Self {
        Self {
            breaker_ratio: 0.78,
            height_at_shore: 0.08,
            depth_smoothing: 2,
            least_depth: 0.6,
            slowest: 3.0,
            arrival_slope: 0.12,
            arrival_side: 0.35,
            shelter_rays: 10,
            shelter_fan: 60.0,
            shelter_reach: 12.0,
            bend_share: 0.15,
            shelter_floor: 0.12,
            chop_regrow: 30.0,
        }
    }
}

/// Ripples too small to build, which exist only as shading.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RippleTuning {
    /// Slope the ripples add at full "Ripple strength".
    pub slope: f64,
    /// Their size in blocks, along and across.
    pub size: [f64; 2],
    /// How much of that slope they also have sideways, along the crests.
    pub sideways: f64,
    /// How much of that slope a library or image pattern gives at strength 1.
    pub pattern_share: f64,
    /// With a made-up sea floor, waves break in water shallower than this
    /// many times their height.
    pub seabed_break_depth: f64,
}

impl Default for RippleTuning {
    fn default() -> Self {
        Self { slope: 0.24, size: [2.2, 6.5], sideways: 0.35, pattern_share: 0.4, seabed_break_depth: 1.3 }
    }
}

/// White water where the sea meets land.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ShoreFoamTuning {
    /// How many blocks out the foam reaches, and how strong it is, on sand
    /// and on rock, where the shore faces the full sea.
    pub sand_reach: f64,
    pub sand_strength: f64,
    pub rock_reach: f64,
    pub rock_strength: f64,
    /// The same at the foot of a cliff. Which shore is which is read from
    /// the shape of the ground; a shore between two kinds gets a mixture.
    pub cliff_reach: f64,
    pub cliff_strength: f64,
    /// How much the height of the arriving waves widens or narrows the
    /// foam: 0 not at all, 0.4 from 0.6 times in a calm to 1.4 times in a storm.
    pub height_sway: f64,
    /// Share of the shore foam left on the banks of a river channel.
    pub channel_share: f64,
    /// Share of that foam a completely sheltered shore keeps.
    pub sheltered: f64,
    /// Size in blocks of the patches it breaks into.
    pub patch_size: f64,
    /// White water where a waterfall lands in the sea: how many blocks it
    /// reaches for each block the water falls, the most it reaches, and
    /// how white it is at the foot of the fall.
    pub fall_reach: f64,
    pub fall_reach_most: f64,
    pub fall_strength: f64,
}

impl Default for ShoreFoamTuning {
    fn default() -> Self {
        Self {
            sand_reach: 1.6,
            sand_strength: 0.9,
            rock_reach: 3.5,
            rock_strength: 1.0,
            cliff_reach: 2.5,
            cliff_strength: 1.0,
            height_sway: 0.4,
            channel_share: 0.25,
            sheltered: 0.35,
            patch_size: 3.0,
            fall_reach: 0.6,
            fall_reach_most: 16.0,
            fall_strength: 1.0,
        }
    }
}

/// Waves thrown back by the shore (only with terrain). In front of a shore
/// that throws waves back, the sea is the waves arriving and the same waves
/// leaving, crossing each other.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ReflectionTuning {
    /// Share of a wave's height thrown back by a cliff, by a rocky shore
    /// and by a beach. A wall throws back nearly all of it, a gentle beach
    /// next to nothing: there the wave breaks and is spent.
    pub cliff: f64,
    pub rocky: f64,
    pub beach: f64,
    /// How far out the returning waves are still seen, in lengths of the
    /// main wave: at this distance about a third is left.
    pub reach: f64,
    /// How many blocks wide a square the wave thrown back by a shore is
    /// spread over before it is added. A shore far off sends its wave home
    /// with crests closer together than a block: the column of sea behind
    /// the shore it is read from turns over many times between one column
    /// of water and the next, and drawn as it stands that is a crease of
    /// light lying across the waves. 0 adds it as it is.
    pub soften: f64,
}

impl Default for ReflectionTuning {
    fn default() -> Self {
        Self { cliff: 0.55, rocky: 0.2, beach: 0.0, reach: 0.25, soften: 3.0 }
    }
}

/// A river running out into the sea (only with terrain that has one).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct OutflowTuning {
    /// How fast the water leaves a river mouth, as a share of the speed of
    /// waves `speed_for_length` blocks long. Longer waves run faster and
    /// feel the same river less. A wave running against a current is
    /// stopped once the current reaches a quarter of the wave's speed.
    /// At 0 rivers do nothing to the waves.
    pub speed: f64,
    pub speed_for_length: f64,
    /// How quickly the outflow widens: blocks to each side for every block
    /// it travels.
    pub spread: f64,
    /// It fades away over this many widths of its mouth.
    pub reach: f64,
    /// The most a wave grows running against it. Left to themselves the
    /// sums grow without limit; real waves have broken long before.
    pub gain_most: f64,
    /// Share of that speed the water keeps where it fell into the sea
    /// instead of running into it.
    pub fall_share: f64,
    /// How much the river's water lightens the sea it runs into, toward
    /// the lit colour.
    pub plume_tint: f64,
}

impl Default for OutflowTuning {
    fn default() -> Self {
        Self { speed: 0.2, speed_for_length: 40.0, spread: 0.3, reach: 5.0, gain_most: 1.7, fall_share: 0.4, plume_tint: 0.3 }
    }
}

/// A current running past land (only with land, and with "Current speed"
/// above 0).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct FlowTuning {
    /// Share of the current that is lost right behind land.
    pub slack_most: f64,
    /// How quickly the slack water mixes with the current at its sides
    /// (0 to 0.33). More closes the slack water sooner.
    pub slack_spread: f64,
    /// The current is back about this many widths of the land behind it.
    pub slack_widths: f64,
    /// How far in front of land the water is piled up, in blocks.
    pub pile_reach: f64,
    /// How high it is piled, and how far it is drawn down behind the land,
    /// as shares of the head of the current (speed squared over twice gravity).
    pub pile_height: f64,
    pub lee_drop: f64,
    /// Height of the standing waves behind land, as a share of that head.
    pub wave_height: f64,
    /// Height of the crests that fan out to the sides, against the crests
    /// that lie across the current.
    pub divergent: f64,
    /// The standing waves are lost in the sea over about this many
    /// distances between their crests.
    pub wave_reach: f64,
    /// How gradually the wake fades at the sides of the wedge it keeps to,
    /// as a share of the wedge's half width. Wide melts the wake into the
    /// sea; narrow ends it on a line.
    pub wedge_soft: f64,
    /// The crests that fan out to the sides are lost over about this many
    /// distances between crests. They lie longer across the sea than the
    /// crests across the current, so they fade sooner.
    pub fan_reach: f64,
    /// The shore is gathered into at most this many places that each start
    /// a wake. More follows the shape of the land more closely and costs
    /// bake time.
    pub sources: u32,
    /// Standing waves shorter than the first number of blocks are left
    /// out; from the second they have their full height.
    pub shortest: [f64; 2],
    /// The water is never lifted or lowered by more than this many heads.
    pub lift_most: f64,
    /// How much of their height short waves lose in slack water.
    pub calm: f64,
    /// How much white water the current leaves where it strikes land and
    /// in the slack water behind it.
    pub foam: f64,
    /// How many seconds foam lasts. It is carried along meanwhile, so
    /// it reaches as far as the current runs in this time.
    pub foam_life: f64,
    /// Length and width of the streaks that foam is drawn into, in blocks.
    pub foam_streak: [f64; 2],
    /// A current slower than this many metres a second leaves no foam; at
    /// three times this it leaves all of it.
    pub foam_from: f64,
    /// How much the white water at a struck shore is raised by the speed of
    /// the current that strikes it, on top of what the shore gives on its
    /// own. Was 0.35 before it was found to flood a fast current's shores.
    pub impact_bonus: f64,
    /// How much white water a fast current is rid of at the faces it
    /// strikes, as it reaches three times `foam_from`. A fast current already
    /// breaks the sea white all around it, so those faces must not read as a
    /// sheet painted on top of that. The trail it drags keeps its own cap:
    /// that white wash is how a fast current reads.
    pub fast_cap: f64,
    /// How high the water is thrown up the face of what it strikes, in
    /// blocks, at the head of the current. It climbs the face and comes away
    /// in spouts above it, so a wall is met by water rather than only painted
    /// white, and the height falls off as the current slows. 0 leaves the
    /// surface flat as before.
    pub splash_height: f64,
    /// How much faster the current runs where it is squeezed between two
    /// shores, as a share of its open speed. The same water through a
    /// smaller gap goes faster; 0 leaves the current even everywhere.
    pub squeeze: f64,
    /// How far to the side the room is measured, in blocks. Shores closer
    /// together than twice this can squeeze the current between them.
    pub squeeze_reach: f64,
    /// How far foam is drawn out by a squeezed current, against how much
    /// faster that current runs. 0 leaves the foam where it was made.
    pub foam_stretch: f64,
    /// How much of the foam a wake carries is stirred sideways into the water
    /// beside it. Water behind an obstacle mixes out into the stream, so the
    /// wake widens downwind.
    pub mix: f64,
    /// How many blocks to each side that stirring reaches.
    pub mix_spread: f64,
    /// How high the water stands where the land squeezes the stream, as a
    /// share of the head the current has. A narrow way between two shores
    /// runs the water faster and lifts it a little.
    pub squeeze_lift: f64,
    /// How much the current is stopped where it runs into a face that leaves
    /// it no way on: the water piles up there and goes aside instead. 0 lets
    /// it run on as if the face were not there.
    pub stagnation: f64,
}

impl Default for FlowTuning {
    fn default() -> Self {
        Self {
            slack_most: 0.85,
            slack_spread: 0.2,
            slack_widths: 8.0,
            pile_reach: 10.0,
            pile_height: 1.0,
            lee_drop: 0.5,
            wave_height: 0.9,
            divergent: 0.8,
            wave_reach: 2.2,
            wedge_soft: 0.7,
            fan_reach: 0.9,
            sources: 48,
            shortest: [10.0, 24.0],
            lift_most: 2.0,
            calm: 0.6,
            foam: 0.9,
            foam_life: 14.0,
            foam_streak: [22.0, 3.0],
            foam_from: 1.0,
            impact_bonus: 0.2,
            fast_cap: 0.15,
            splash_height: 6.0,
            squeeze: 1.0,
            squeeze_reach: 8.0,
                foam_stretch: 0.6,
            mix: 0.5,
            mix_spread: 3.0,
            squeeze_lift: 0.5,
            stagnation: 0.5,
        }
    }
}

/// Whitecaps: where waves break, and the foam they leave behind.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct WhitecapTuning {
    /// A crest breaks where the water is this steep. For one plain wave the
    /// steepness is half its height times 2 pi over its length; the
    /// theoretical limit is 0.44 and waves at sea break from about 0.3.
    /// Seas built from blocks are usually far steeper than real ones, so the
    /// standard value is above that.
    pub breaking_steepness: f64,
    /// However steep the sea, no more than this share of it breaks at one
    /// moment: at sea, breaking is what keeps waves from getting steeper.
    /// With the foam left behind, about four times as much shows white.
    pub most_breaking: f64,
    /// Range of steepness over which breaking sets in.
    pub softness: f64,
    /// How much the short waves riding on the sea count toward that
    /// steepness. At 1 every ripple breaks; at 0 only the main waves do.
    pub short_wave_share: f64,
    /// The share of the open sea's whitecaps a completely shaded lee keeps.
    /// A lee is a smaller sea, not a still one, so it is never quite white-
    /// less; this is how much cover even the emptiest lee holds on to.
    pub lee_foam_least: f64,
    /// How fast that cover falls as the slope of the sea that is left drops
    /// below the open sea's. At 1.76 it follows the wind sea's cover curve
    /// (W ∝ H^1.76) as though the sea were that much smaller; 0 would keep
    /// the open sea's cover everywhere, higher clears the lee sooner.
    pub lee_foam_power: f64,
    /// How far "Foam amount" moves that threshold: at 0.15, full foam breaks
    /// waves 15% less steep and no foam needs them 15% steeper.
    pub foam_amount_sway: f64,
    /// How long foam lasts, in main wave periods. Measured foam fades over 2
    /// to 5 seconds, a third to a half of the period of a breaking sea.
    pub trail_time: f64,
    /// How many past moments are looked at to find that foam. More is
    /// smoother and costs bake time.
    pub trail_samples: u32,
    /// How white a passing breaker leaves the water behind it.
    pub trail_strength: f64,
    /// Foam left behind is never a solid sheet: at most this share of its
    /// blocks turn white, so it stays streaked however thick it is.
    pub trail_most: f64,
    /// How much more easily waves break where the water is too shallow for
    /// them: 0.7 lets a wave 70% less steep break in the surf.
    pub surf_ease: f64,
}

impl Default for WhitecapTuning {
    fn default() -> Self {
        Self {
            breaking_steepness: 0.5,
            most_breaking: 0.02,
            softness: 0.06,
            short_wave_share: 0.4,
            lee_foam_least: 0.1,
            lee_foam_power: 1.76,
            foam_amount_sway: 0.15,
            trail_time: 0.3,
            trail_samples: 24,
            trail_strength: 1.5,
            trail_most: 0.6,
            surf_ease: 0.45,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ColourTuning {
    /// Colour of white water, as red, green, blue.
    pub foam_white: [u8; 3],
    /// How strongly "Foam amount" turns water white, and the most white allowed.
    pub foam_gain: f64,
    pub foam_most: f64,
    /// Extra say white water gets when the block types are chosen.
    pub foam_weight: f64,
    /// Where a column lies between the dark and the light colour. The
    /// direction of the light leads: `slope_light` is how strongly a face
    /// turned to or from the sun moves the colour. The thickness of the
    /// water supports it: crests are thin and light comes through them,
    /// troughs are deep, and `crest_light` is how strongly the height of
    /// the water within the wave moves the colour.
    pub crest_light: f64,
    pub slope_light: f64,
    /// The height above the middle of the swell at which a crest counts as
    /// a full one: this many times the usual rise and fall of the sea in
    /// hand, and never less than this many blocks, so that a nearly flat
    /// sea is nearly one colour.
    pub crest_span: [f64; 2],
    /// Level water in full sun lies half way between the "Dark colour" and
    /// the "Light colour" (at "Level water" one half). The darkest face
    /// falls to this share of that, toward the dark colour; the lightest
    /// rises this far beyond it, toward the light colour.
    pub tone_reach: [f64; 2],
    /// Two bright colours leave a pale, flat sea. Where the "Dark colour"
    /// is itself bright, faces turned from the sun go deeper than it,
    /// darker and with more colour: by this share at the most. It sets in
    /// between the two lightnesses of `bright_from` (0 black, 1 white), so
    /// a dark colour that is dark is never touched. 0: never deeper.
    pub bright_depth: f64,
    pub bright_from: [f64; 2],
    /// Reflections take the light colour made paler: this far from it
    /// toward white. 0: reflections are the light colour itself.
    pub reflection_white: f64,
    /// How much of the colour left after that the reflections keep. 1: all
    /// of it. 0: reflections are a grey as pale as `reflection_white` makes.
    pub reflection_tint: f64,
    /// Wind does not ruffle the sea evenly. `patch_tone` is how much
    /// darker and lighter that leaves broad patches of it; `patch_size`
    /// their size in blocks, along the wind and across it.
    pub patch_tone: f64,
    pub patch_size: [f64; 2],
    /// Without direct sun the light comes from the whole sky. Crests stand
    /// open to it and troughs and steep faces see less of it: `sky_shade`
    /// is how much lighter and darker that makes them at full "Wave
    /// shading", and `sky_lift` how quickly a crest gains with its height.
    pub sky_shade: f64,
    pub sky_lift: f64,
    /// How far the sea floor's colour can replace the water's at the
    /// waterline, at full "Water clarity".
    pub bed_tint: f64,
    /// How dark the sea is at "Daylight" 0, as a share of its lightness in
    /// full day. At 0 the sea is black there; raise it to keep some light
    /// however low "Daylight" is set.
    pub darkest: f64,
    /// How much of the fall in "Daylight" the white water and the
    /// reflections share. 0: they stay as bright in the dark. 1: they
    /// darken as much as the water.
    pub white_dims: f64,
    /// How far from the mirroring slope a glint still shows, as a
    /// difference in slope, at "Reflection softness" 0 and 1.
    pub glint_spread: [f64; 2],
    /// How much brighter the sky is than the light coming up out of the
    /// water. Water mirrors little of the sky where one looks down into it
    /// and much of it on slopes seen at a low angle; the brighter the sky
    /// against the water, the more those slopes take the reflection colour.
    pub sky_ratio: f64,
    /// Strength of the glint of the sun, and of that sheen of the sky, in
    /// the reflections.
    pub glint_share: f64,
    pub sheen_share: f64,
    /// Slopes turned toward the onlooker mirror less sky than level water
    /// and show the deep colour: this is the most they darken and deepen.
    pub sheen_deepen: f64,
    /// When blocks are chosen, how much worse a wrong hue is than a tone
    /// that is only too dark or too light. 1: both count alike.
    pub hue_care: f64,
    /// How much worse again a stack of blocks is that has less colour
    /// than the water was given: grey blocks under glass look washed out.
    /// 0: only the nearness of the colour counts.
    pub washed_care: f64,
    /// `hue_care` and `washed_care` count in full only for water with at
    /// least the second of these as its strength of colour (OKLab chroma;
    /// 0 is grey, a clear sea blue about 0.1), and not at all below the
    /// first. Grey storm water has no hue to keep: there the nearest
    /// blocks are simply the nearest.
    pub care_from: [f64; 2],
    /// The most uneven a block's face may be to be used, as it shows
    /// through the glass over it (which evens it out). Raise it to let in
    /// more textured blocks; lower it for the flattest blocks only.
    pub texture_most: f64,
    /// How much it counts against a stack of blocks that its glass is of
    /// another colour than the stack looks from above. Blue glass over
    /// green blocks looks teal on the map, and blue along every step in
    /// the waves when seen from the side. 0: only the look from above
    /// counts.
    pub glass_apart: f64,
    /// A further solid block or glass colour is taken, within the limits
    /// set in the app, only while it brings the blocks this much closer to
    /// the colours aimed for, as a share of what is still off.
    pub block_gain: f64,
    /// The steepness of sea the shading is laid out for (the typical slope
    /// of its swell). In a steeper sea the shading is eased by as much, so
    /// that it does not turn into patches of darkest and lightest. 0: never eased.
    pub shade_slope: f64,
    /// Foam is built as a scatter of white blocks: thin foam turns the few
    /// blocks white whose own threshold lies below it. Thresholds run from
    /// `foam_grain` to 1; `foam_edge` is how gradually a block turns white.
    pub foam_grain: f64,
    pub foam_edge: f64,
    /// How strongly thick foam is cut into plates by cracks of open water
    /// between them: 0 leaves one sheet of white, 1 cuts through to the sea.
    /// Thin foam at the edges is left as it is.
    pub foam_net: f64,
    /// The size of one plate along the current and across it, in blocks.
    pub foam_net_size: [f64; 2],
    /// Share of those thresholds that follows streaks drawn out along the
    /// waves' path rather than changing from block to block, and the length
    /// and width of the streaks in blocks.
    pub foam_streaks: f64,
    pub foam_streak_size: [f64; 2],
    /// How much the churned water around foam is lightened toward the lit colour.
    pub aerated_tint: f64,
    /// Dithering mixes the two block tones a colour lies between, in a
    /// fine fixed pattern, in the proportion that gives the colour at a
    /// distance. `dither_reach` is how much of the step between the two is
    /// mixed across at full "Dithering"; steps in lightness larger than
    /// `dither_most` are not mixed across in full.
    pub dither_reach: f64,
    pub dither_most: f64,
    /// A pattern that changes from block to block reads as grain. This is
    /// the share of the mixing done instead in patches a few blocks across,
    /// drawn out along the crests; and their length and width in blocks.
    /// 0: the fine pattern only.
    pub dither_patches: f64,
    pub dither_patch_size: [f64; 2],
    /// 0: a block whose colour none of the four beside it share takes the
    /// nearest colour among theirs, so no single blocks are left standing
    /// out. 1: lone blocks are kept. Foam is a scatter and is never touched.
    pub lone_blocks: f64,
    /// With two or three layers of glass, the layers are blended along
    /// every border between two looks: the top layer keeps a smooth line,
    /// each layer under it changes colour along a line of its own, and the
    /// lowest are dithered, so one hard border becomes several faint ones.
    /// Away from a border all layers of a column have one colour. 1: on.
    /// 0: off; the layers of a stack may instead differ in colour anywhere,
    /// which gives more looks to choose from but leaves even water blotchy.
    pub layer_blend: f64,
    /// How far from a border, in blocks, the layers are blended.
    pub layer_reach: f64,
    /// How far, in blocks, the colours are softened before the middle and
    /// the top layer of glass are chosen, so their lines run smooth.
    pub layer_smooth: [f64; 2],
    /// How much the lowest, the middle and the top layer of glass are
    /// dithered, as a share of "Dithering": at 1 or more in all, a colour
    /// between two looks is mixed from both in full proportion. The glass
    /// above softens the pattern, so the lowest layer can take the most.
    pub layer_dither: [f64; 3],
}

impl Default for ColourTuning {
    fn default() -> Self {
        Self {
            foam_white: [238, 244, 248],
            crest_light: 0.3,
            crest_span: [2.5, 1.5],
            slope_light: 3.2,
            tone_reach: [0.3, 0.5],
            bright_depth: 0.3,
            bright_from: [0.56, 0.78],
            reflection_white: 0.68,
            reflection_tint: 0.73,
            patch_tone: 0.12,
            patch_size: [70.0, 30.0],
            sheen_share: 1.0,
            shade_slope: 0.28,
            hue_care: 2.5,
            washed_care: 4.0,
            care_from: [0.03, 0.06],
            texture_most: 0.045,
            glass_apart: 0.04,
            block_gain: 0.003,
            sky_shade: 0.25,
            sky_lift: 1.5,
            bed_tint: 0.6,
            white_dims: 0.5,
            glint_spread: [0.04, 0.3],
            sky_ratio: 12.0,
            glint_share: 0.7,
            sheen_deepen: 0.25,
            darkest: 0.0,
            dither_reach: 1.0,
            dither_most: 0.05,
            dither_patches: 1.0,
            dither_patch_size: [9.0, 3.0],
            lone_blocks: 0.0,
            layer_blend: 1.0,
            layer_reach: 3.0,
            layer_smooth: [1.0, 2.0],
            layer_dither: [1.6, 0.8, 0.0],
            foam_gain: 2.4,
            foam_most: 0.92,
            foam_weight: 4.0,
            foam_grain: 0.2,
            foam_edge: 0.1,
            foam_net: 0.85,
            foam_net_size: [13.0, 7.0],
            foam_streaks: 0.8,
            foam_streak_size: [7.0, 1.6],
            aerated_tint: 0.4,
        }
    }
}

/// Foam left lying on the water: the lace behind a breaker, and the streaks
/// the wind draws out of it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SpentFoamTuning {
    /// How much of the sea is white in all, by the height of the waves in
    /// metres: `cover_per_height` times the height to the power
    /// `cover_power`, never above `cover_most`. The standard numbers are the
    /// whitecap cover measured against wind speed (Monahan and
    /// O'Muircheartaigh 1980), for the wind that raises a sea of that
    /// height: 1% at 2.5 m, 6% at 7 m, 17% at 13 m. "Foam amount" 0.5 and
    /// "Storm foam" 0.5 give this cover; both at 1 give four times the
    /// foam left behind. "Storm foam" 0, the standard, turns all of this off.
    pub cover_per_height: f64,
    pub cover_power: f64,
    pub cover_most: f64,
    /// Share of that cover that is crests breaking at the moment. The rest
    /// is foam they left. A large sea breaks this much even where its waves
    /// are not steep enough by `whitecaps.breaking_steepness`.
    pub breaking_share: f64,
    /// Waves less steep than this never break, however large the sea.
    pub steepness_floor: f64,
    /// How long the lace lasts and how long the streaks last, in main wave
    /// periods.
    pub life: [f64; 2],
    /// Past moments looked at for each. More is smoother and costs bake time.
    pub samples: u32,
    /// The foam left is worked out every this many blocks and filled in
    /// between.
    pub step: u32,
    /// Share of the foam left that lies as lace; the rest is streaks.
    pub lace_share: f64,
    /// Size of the meshes of the lace, along the waves' path and across it,
    /// in blocks; how wide its threads are (0 to 1); and how thin foam may
    /// be and still show as threads (0 to 1).
    pub lace_size: [f64; 2],
    pub lace_width: f64,
    pub lace_from: f64,
    /// Length and width of the streaks in blocks, how gradually they fill
    /// in as the foam thickens (0 to 1), and how thin foam may be and still
    /// show.
    pub streak_size: [f64; 2],
    pub streak_width: f64,
    pub streak_from: f64,
    /// How far along the wind the foam is drawn out into streaks, in blocks.
    pub streak_drawn: f64,
    /// How far the streaks waver from side to side, in blocks.
    pub streak_waver: f64,
    /// How evenly the streaks lie over the sea: 0 only where waves broke
    /// lately, 1 the same everywhere.
    pub streak_even: f64,
    /// The most white foam left behind can be, as a share of `colour.foam_most`.
    pub most: f64,
    /// How gradually a block turns white.
    pub edge: f64,
    /// The foam left is never multiplied up by more than this to reach the
    /// cover, so a sea that hardly breaks stays clean.
    pub gain_most: f64,
}

impl Default for SpentFoamTuning {
    fn default() -> Self {
        Self {
            cover_per_height: 0.0021,
            cover_power: 1.7,
            cover_most: 0.5,
            breaking_share: 0.12,
            steepness_floor: 0.35,
            life: [1.2, 3.0],
            samples: 24,
            step: 4,
            lace_share: 0.5,
            lace_size: [13.0, 10.0],
            lace_width: 0.5,
            lace_from: 0.12,
            streak_size: [52.0, 3.2],
            streak_width: 0.7,
            streak_from: 0.12,
            streak_drawn: 40.0,
            streak_waver: 5.0,
            streak_even: 0.5,
            most: 0.9,
            edge: 0.06,
            gain_most: 40.0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Tuning {
    #[serde(rename = "_about")]
    pub about: String,
    /// Which method the numbers were written for; see `TUNING_VERSION`.
    #[serde(rename = "_version")]
    pub version: u32,
    pub waves: WaveTuning,
    pub near_land: NearLandTuning,
    pub reflection: ReflectionTuning,
    pub ripples: RippleTuning,
    pub shore_foam: ShoreFoamTuning,
    pub outflow: OutflowTuning,
    pub flow: FlowTuning,
    pub whitecaps: WhitecapTuning,
    pub spent_foam: SpentFoamTuning,
    pub colour: ColourTuning,
}

const ABOUT: &str = "Internal numbers of the Ocean Foundry ocean generator (development build). \
Change a number, save this file, then press Bake. Delete the file to get the defaults back. \
TUNING.txt beside the program says what each number does. Keep the _version line.";

impl Default for Tuning {
    fn default() -> Self {
        Self {
            about: ABOUT.to_string(),
            version: TUNING_VERSION,
            waves: WaveTuning::default(),
            near_land: NearLandTuning::default(),
            reflection: ReflectionTuning::default(),
            ripples: RippleTuning::default(),
            shore_foam: ShoreFoamTuning::default(),
            outflow: OutflowTuning::default(),
            flow: FlowTuning::default(),
            whitecaps: WhitecapTuning::default(),
            spent_foam: SpentFoamTuning::default(),
            colour: ColourTuning::default(),
        }
    }
}

impl Tuning {
    pub fn to_text(&self) -> String {
        serde_json::to_string_pretty(self).expect("tuning is plain data") + "\n"
    }

    /// Read tuning text. The error says what is wrong and on which line.
    pub fn from_text(text: &str) -> Result<Self, String> {
        let tuning: Tuning = serde_json::from_str(text).map_err(|e| e.to_string())?;
        let numbers_ok = serde_json::to_value(&tuning).map(|v| all_finite(&v)).unwrap_or(false);
        if !numbers_ok {
            return Err("every number must be finite".into());
        }
        Ok(tuning)
    }

    /// Which method a tuning file was written for. A file that does not say
    /// is from before versions were recorded. Text that cannot be read at
    /// all counts as current, so that reading it reports the mistake.
    pub fn version_of(text: &str) -> u32 {
        match serde_json::from_str::<serde_json::Value>(text) {
            Ok(serde_json::Value::Object(o)) => o.get("_version").and_then(|v| v.as_u64()).map_or(1, |v| v as u32),
            _ => TUNING_VERSION,
        }
    }
}

fn all_finite(v: &serde_json::Value) -> bool {
    match v {
        serde_json::Value::Number(n) => n.as_f64().is_some_and(f64::is_finite),
        serde_json::Value::Array(a) => a.iter().all(all_finite),
        serde_json::Value::Object(o) => o.values().all(all_finite),
        _ => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_written_file_reads_back_the_same() {
        let t = Tuning::default();
        assert_eq!(Tuning::from_text(&t.to_text()).unwrap(), t);
        assert_eq!(Tuning::version_of(&t.to_text()), TUNING_VERSION);
    }

    #[test]
    fn a_partial_file_keeps_the_other_defaults() {
        let t = Tuning::from_text(r#"{ "whitecaps": { "trail_time": 0.9 } }"#).unwrap();
        assert_eq!(t.whitecaps.trail_time, 0.9);
        assert_eq!(t.whitecaps.softness, WhitecapTuning::default().softness);
        assert_eq!(t.waves, WaveTuning::default());
    }

    #[test]
    fn mistakes_are_refused_with_a_reason() {
        assert!(Tuning::from_text(r#"{ "whitecaps": { "trailtime": 11 } }"#).unwrap_err().contains("trailtime"));
        assert!(Tuning::from_text(r#"{ "waves": { "components": -3 } }"#).is_err());
        assert!(Tuning::from_text("not json").is_err());
    }

    #[test]
    fn a_file_from_an_earlier_method_is_recognised() {
        // The first development build wrote wave trains and no version.
        let old = r#"{ "_about": "x", "waves": { "trains": [], "height_match": 1.0 } }"#;
        assert_eq!(Tuning::version_of(old), 1);
        assert!(Tuning::from_text(old).is_err());
        assert_eq!(Tuning::version_of(r#"{ "_version": 2 }"#), 2);
        assert_eq!(Tuning::version_of("not json"), TUNING_VERSION);
    }
}
