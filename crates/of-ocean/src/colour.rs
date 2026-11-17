//! Colour space and block matching. Colours are compared in Oklab, where
//! equal distances look about equally different to the eye.

use of_formats::catalogue::Catalogue;
use std::collections::HashMap;

fn to_linear(c: u8) -> f32 {
    let c = c as f32 / 255.0;
    if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}

fn from_linear(c: f32) -> u8 {
    let c = c.clamp(0.0, 1.0);
    let s = if c <= 0.003_130_8 { c * 12.92 } else { 1.055 * c.powf(1.0 / 2.4) - 0.055 };
    (s * 255.0 + 0.5) as u8
}

pub fn to_oklab(rgb: [u8; 3]) -> [f32; 3] {
    let (r, g, b) = (to_linear(rgb[0]), to_linear(rgb[1]), to_linear(rgb[2]));
    let l = (0.412_221_47 * r + 0.536_332_55 * g + 0.051_445_995 * b).cbrt();
    let m = (0.211_903_5 * r + 0.680_699_5 * g + 0.107_396_96 * b).cbrt();
    let s = (0.088_302_46 * r + 0.281_718_85 * g + 0.629_978_7 * b).cbrt();
    [
        0.210_454_26 * l + 0.793_617_8 * m - 0.004_072_047 * s,
        1.977_998_5 * l - 2.428_592_2 * m + 0.450_593_7 * s,
        0.025_904_037 * l + 0.782_771_77 * m - 0.808_675_77 * s,
    ]
}

pub fn from_oklab(c: [f32; 3]) -> [u8; 3] {
    let l = (c[0] + 0.396_337_78 * c[1] + 0.215_803_76 * c[2]).powi(3);
    let m = (c[0] - 0.105_561_346 * c[1] - 0.063_854_17 * c[2]).powi(3);
    let s = (c[0] - 0.089_484_18 * c[1] - 1.291_485_5 * c[2]).powi(3);
    [
        from_linear(4.076_741_7 * l - 3.307_711_6 * m + 0.230_969_94 * s),
        from_linear(-1.268_438 * l + 2.609_757_4 * m - 0.341_319_38 * s),
        from_linear(-0.004_196_086_3 * l - 0.703_418_6 * m + 1.707_614_7 * s),
    ]
}

pub fn mix(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t]
}

pub struct Solid {
    pub name: String,
    pub rgb: [u8; 3],
}

pub struct Glass {
    pub name: String,
    pub rgb: [u8; 3],
}

/// A solid block, optionally seen through stained glass.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pair {
    pub solid: u16,
    /// The glass colour of every layer over it, lowest first; `NO_GLASS`
    /// where there is none.
    pub glass: [u16; 3],
    /// How the stack looks from above.
    pub lab: [f32; 3],
    /// How colourful that is.
    chroma: f32,
    /// Added to the match error; favours smooth-textured blocks.
    penalty: f32,
}

pub const NO_GLASS: u16 = u16::MAX;

/// What counts when a stack is matched to a colour.
#[derive(Debug, Clone, Copy)]
pub struct Care {
    /// How much worse a wrong hue is than a tone that is only too dark or
    /// too light.
    pub hue: f32,
    /// How much worse again a stack is that has less colour than was asked
    /// for: grey blocks under glass look washed out.
    pub washed: f32,
    /// Both count in full only for a colour with at least the second of
    /// these as its strength of colour (OKLab chroma), and not at all for
    /// one with less than the first: water near grey has no hue to keep,
    /// and there the nearest colour is simply the nearest.
    pub from: [f32; 2],
}

/// Every stack the build may use, and the means to pick among them.
pub struct Matcher {
    pub solids: Vec<Solid>,
    pub glasses: Vec<Glass>,
    /// How uneven the face of each solid is.
    variation: Vec<f32>,
    /// How much of what lies under it each glass colour hides.
    cover: Vec<f32>,
    layers: usize,
    /// Whether the layers of a stack may differ in colour when the stacks
    /// are chosen.
    mixed: bool,
    /// How much it counts against a stack that its glass is of another
    /// colour than the stack looks from above.
    apart: f32,
}

/// Blocks that are full, plain cubes but would look wrong as water.
const UNSUITABLE: [&str; 9] = [
    "sculk",
    "netherite_block",
    "crying_obsidian",
    "nether_wart_block",
    "warped_wart_block",
    "jukebox",
    "smithing_table",
    "coral_block",
    "nylium",
];

/// With two glass layers the stacks are drawn from this many glass colours,
/// the most useful for the scene; with three, from this many.
const SHORTLIST: [usize; 2] = [8, 5];
/// The largest table of every colour with every stack. Scenes that would
/// need a larger one have near-identical colours merged first.
const TABLE_MOST: usize = 24_000_000;

/// Every look that `layers` layers drawn from these glass colours can have.
/// Layers of one colour count once, and are laid at the bottom.
fn looks(layers: usize, colours: &[u16], mixed: bool) -> Vec<[u16; 3]> {
    if layers == 0 || colours.is_empty() {
        return vec![[NO_GLASS; 3]];
    }
    let mut found = Vec::new();
    for &a in colours {
        let mut one = [NO_GLASS; 3];
        one[..layers].fill(a);
        found.push(one);
        for &b in colours.iter().filter(|b| mixed && layers >= 2 && **b != a) {
            let mut two = one;
            two[layers - 1] = b;
            found.push(two);
            for &c in colours.iter().filter(|c| layers >= 3 && **c != b) {
                found.push([a, b, c]);
            }
        }
    }
    found
}

impl Matcher {
    /// `glass_layers` of stained glass lie over each solid; 0 means none.
    /// With `mixed` the layers of a stack may be chosen in different
    /// colours; without, a stack has glass of one colour.
    /// The same choice of blocks without the ones named.
    pub fn without(mut self, names: &[String]) -> Self {
        if names.is_empty() {
            return self;
        }
        let keep: Vec<bool> = self.solids.iter().map(|s| !names.contains(&s.name)).collect();
        let mut at = keep.iter();
        self.solids.retain(|_| *at.next().unwrap());
        let mut at = keep.iter();
        self.variation.retain(|_| *at.next().unwrap());
        let keep: Vec<bool> = self.glasses.iter().map(|g| !names.contains(&g.name)).collect();
        let mut at = keep.iter();
        self.glasses.retain(|_| *at.next().unwrap());
        let mut at = keep.iter();
        self.cover.retain(|_| *at.next().unwrap());
        self
    }

    /// A solid may be as uneven as `texture_most`, as seen through the glass.
    /// `apart` is how much it counts against a stack that its glass is of
    /// another colour than the stack looks from above.
    pub fn new(catalogue: &Catalogue, glass_layers: u32, texture_most: f32, mixed: bool, apart: f32) -> Self {
        Self::with_tier(catalogue, glass_layers, texture_most, mixed, apart, true)
    }

    /// As `new`, but with `survival_only` false the blocks only to be had in
    /// creative mode may be used too.
    pub fn with_tier(catalogue: &Catalogue, glass_layers: u32, texture_most: f32, mixed: bool, apart: f32, survival_only: bool) -> Self {
        let layers = glass_layers.min(3) as usize;
        let (mut glasses, mut cover) = (Vec::new(), Vec::new());
        if layers > 0 {
            for name in catalogue.stained_glass() {
                let info = &catalogue.blocks[&name];
                let (Some(rgb), Some(alpha)) = (info.top, info.alpha) else { continue };
                cover.push(alpha as f32 / 255.0);
                glasses.push(Glass { name, rgb });
            }
        }
        // Glass evens out the texture of what lies under it.
        let shows = cover.iter().fold(1f32, |least, c| least.min(1.0 - c));
        let (mut solids, mut variation) = (Vec::new(), Vec::new());
        for (name, info) in catalogue.solid_colour_blocks(survival_only, false) {
            let (Some(top), uneven) = (info.top, info.variation.unwrap_or(0.0)) else { continue };
            let side = info.side.unwrap_or(top);
            let same_all_round = (0..3).map(|i| (top[i] as i32 - side[i] as i32).abs()).sum::<i32>() <= 24;
            // Blocks with properties can face several ways; plain ones cannot be placed wrongly.
            if !info.props.is_empty() || uneven * shows > texture_most || !same_all_round || UNSUITABLE.iter().any(|u| name.contains(u)) {
                continue;
            }
            solids.push(Solid { name: name.to_string(), rgb: top });
            variation.push(uneven);
        }
        Self { solids, glasses, variation, cover, layers, mixed, apart: apart.max(0.0) }
    }

    /// A solid under the glass of `glass`, lowest layer first.
    ///
    /// The game blends glass over what is behind it in plain sRGB. It does
    /// not draw the faces between two glass blocks of one colour, so layers
    /// of the same colour look like a single one from above; only a change
    /// of colour from one layer to the next adds to what is seen.
    fn pair(&self, solid: usize, glass: [u16; 3]) -> Pair {
        let mut seen: [f32; 3] = std::array::from_fn(|i| self.solids[solid].rgb[i] as f32);
        let (mut shows, mut last) = (1f32, NO_GLASS);
        for g in glass.into_iter().filter(|g| *g != NO_GLASS) {
            if g != last {
                let cover = self.cover[g as usize];
                seen = std::array::from_fn(|i| seen[i] * (1.0 - cover) + self.glasses[g as usize].rgb[i] as f32 * cover);
                shows *= 1.0 - cover;
            }
            last = g;
        }
        let lab = to_oklab(seen.map(|c| (c + 0.5) as u8));
        // From above, glass over a solid of another colour mixes with it
        // into one colour. From the side the glass shows more of its own:
        // blue glass over green looks teal on the map and blue along every
        // step in the waves. The further the glass is from the look of
        // the stack, the more that counts against the stack.
        let mut apart = 0f32;
        for g in glass.into_iter().filter(|g| *g != NO_GLASS) {
            let own = to_oklab(self.glasses[g as usize].rgb);
            apart = apart.max((own[1] - lab[1]).powi(2) + (own[2] - lab[2]).powi(2));
        }
        Pair { solid: solid as u16, glass, lab, chroma: lab[1].hypot(lab[2]), penalty: (self.variation[solid] * shows * 0.25).powi(2) + self.apart * apart }
    }

    /// How far a stack is from a colour. The three ways of being off do not
    /// look equally wrong. A stack of another hue (water gone purple or
    /// green) is the worst. One with less colour than asked looks washed
    /// out. One with more colour than asked is the least wrong: it counts
    /// like a tone as far off in lightness. All this holds for water with
    /// colour in it. A colour near grey has no hue to keep: there every
    /// way of being off counts alike, and the nearest stack is the best.
    fn cost(target: &[f32; 3], p: &Pair, care: &Care) -> f32 {
        let d = [target[0] - p.lab[0], target[1] - p.lab[1], target[2] - p.lab[2]];
        let asked = target[1].hypot(target[2]);
        let more = p.chroma - asked;
        let off_hue = (d[1] * d[1] + d[2] * d[2] - more * more).max(0.0);
        let coloured = ((asked - care.from[0]) / (care.from[1] - care.from[0]).max(1e-6)).clamp(0.0, 1.0);
        let coloured = coloured * coloured * (3.0 - 2.0 * coloured);
        let hue = 1.0 + (care.hue - 1.0) * coloured;
        let strength = if more < 0.0 { hue + care.washed * coloured } else { 1.0 };
        d[0] * d[0] + hue * off_hue + strength * more * more + p.penalty
    }

    /// The stack among `stacks` nearest a colour.
    pub fn nearest(stacks: &[Pair], target: &[f32; 3], care: &Care) -> usize {
        let mut found = (0, f32::INFINITY);
        for (i, p) in stacks.iter().enumerate() {
            let cost = Self::cost(target, p, care);
            if cost < found.1 {
                found = (i, cost);
            }
        }
        found.0
    }

    /// The stack nearest a colour among those of `stacks` that `keep` lets
    /// through (by their place in `stacks`), and how far off it is. `None`
    /// when it lets none through.
    pub fn nearest_where(stacks: &[Pair], target: &[f32; 3], care: &Care, keep: impl Fn(usize) -> bool) -> Option<(usize, f32)> {
        let mut found: Option<(usize, f32)> = None;
        for (i, p) in stacks.iter().enumerate().filter(|(i, _)| keep(*i)) {
            let cost = Self::cost(target, p, care);
            if found.is_none_or(|(_, least)| cost < least) {
                found = Some((i, cost));
            }
        }
        found
    }

    /// Every way the solids and glass colours of `chosen` can be laid, each
    /// layer with a colour of its own. Many look alike from above (layers
    /// of one colour show as one); of those, the ones that change colour
    /// lowest down come first, so the upper layers stay in one piece where
    /// they can.
    pub fn arrangements(&self, chosen: &[Pair]) -> Vec<Pair> {
        let (mut solids, mut colours): (Vec<u16>, Vec<u16>) = (Vec::new(), Vec::new());
        for p in chosen {
            if !solids.contains(&p.solid) {
                solids.push(p.solid);
            }
            for g in p.glass.into_iter().filter(|g| *g != NO_GLASS) {
                if !colours.contains(&g) {
                    colours.push(g);
                }
            }
        }
        colours.sort();
        let mut glass: Vec<[u16; 3]> = vec![[NO_GLASS; 3]];
        for k in 0..self.layers {
            glass = glass
                .iter()
                .flat_map(|laid| {
                    colours.iter().map(move |g| {
                        let mut more = *laid;
                        more[k] = *g;
                        more
                    })
                })
                .collect();
        }
        // A change of colour counts for more the higher it lies.
        let high = |laid: &[u16; 3]| (1..self.layers).filter(|k| laid[*k] != laid[*k - 1]).map(|k| 1u32 << k).sum::<u32>();
        glass.sort_by_key(high);
        solids.iter().flat_map(|s| glass.iter().map(move |laid| self.pair(*s as usize, *laid))).collect()
    }

    /// The glass colours worth considering for these colours: those in the
    /// stacks that suit the most wanted colours best.
    fn shortlist(&self, targets: &[([f32; 3], f32)], care: &Care) -> Vec<u16> {
        let all: Vec<u16> = (0..self.glasses.len() as u16).collect();
        let keep = if self.layers >= 3 { SHORTLIST[1] } else { SHORTLIST[0] };
        if !self.mixed || self.layers < 2 || all.len() <= keep {
            return all;
        }
        // Two layers show what a colour is worth beside another.
        let two = looks(2, &all, true);
        let pairs: Vec<Pair> = (0..self.solids.len()).flat_map(|s| two.iter().map(move |look| self.pair(s, *look))).collect();
        let mut heaviest: Vec<&([f32; 3], f32)> = targets.iter().collect();
        heaviest.sort_by(|a, b| b.1.total_cmp(&a.1));
        let mut worth = vec![0f64; all.len()];
        for (colour, weight) in heaviest.into_iter().take(256) {
            let best = pairs[Self::nearest(&pairs, colour, care)];
            let mut counted = NO_GLASS;
            for g in best.glass.into_iter().filter(|g| *g != NO_GLASS) {
                if g != counted {
                    worth[g as usize] += *weight as f64;
                }
                counted = g;
            }
        }
        let mut order = all;
        order.sort_by(|a, b| worth[*b as usize].total_cmp(&worth[*a as usize]).then(a.cmp(b)));
        order.truncate(keep);
        order.sort();
        order
    }

    /// Choose the stacks a build may use for its target colours (colour,
    /// how many places want it): at most `max_solids` solid blocks and
    /// `max_glass` glass colours, and every stack those make. Uniform
    /// scenes end up with fewer.
    ///
    /// Solids and glass are chosen together: a solid is only as good as the
    /// glass that will lie on it, so each is weighed with the others already
    /// chosen, never with glass the build will not have.
    pub fn choose(&self, targets: &[([f32; 3], f32)], max_solids: usize, max_glass: usize, care: &Care, least_gain: f32) -> Vec<Pair> {
        let ns = self.solids.len();
        if ns == 0 || targets.is_empty() {
            return Vec::new();
        }
        let care = &Care { hue: care.hue.max(0.0), washed: care.washed.max(0.0), from: care.from };
        let colours = self.shortlist(targets, care);
        let looks = looks(self.layers, &colours, self.mixed);
        let nl = looks.len();
        // Which glass colours each look needs, as one bit each.
        let needs: Vec<u32> = looks.iter().map(|look| look.iter().filter(|g| **g != NO_GLASS).fold(0u32, |bits, g| bits | 1 << g)).collect();
        let pairs: Vec<Pair> = (0..ns).flat_map(|s| looks.iter().map(move |look| self.pair(s, *look))).collect();
        // A scene with very many colours has near-identical ones merged, so
        // the table of every colour with every stack stays a sensible size.
        let mut merged: Vec<([f32; 3], f32)>;
        let mut targets = targets;
        let mut step = 80.0f32;
        while targets.len() * pairs.len() > TABLE_MOST && step >= 10.0 {
            let mut index: HashMap<[i16; 3], usize> = HashMap::new();
            let mut fewer: Vec<([f32; 3], f32)> = Vec::new();
            for (colour, weight) in targets {
                let key = colour.map(|c| (c * step).round() as i16);
                let at = *index.entry(key).or_insert_with(|| {
                    fewer.push(([0.0; 3], 0.0));
                    fewer.len() - 1
                });
                for i in 0..3 {
                    fewer[at].0[i] += colour[i] * weight;
                }
                fewer[at].1 += weight;
            }
            for (colour, weight) in &mut fewer {
                *colour = colour.map(|c| c / weight.max(1e-9));
            }
            merged = fewer;
            targets = &merged;
            step *= 0.5;
        }
        let (max_solids, max_glass) = (max_solids.clamp(1, ns), max_glass.clamp(1, colours.len().max(1)));
        let nt = targets.len();
        // The error of every target with every stack.
        let mut table = vec![0f32; nt * ns * nl];
        for (ti, (t, _)) in targets.iter().enumerate() {
            for (pi, p) in pairs.iter().enumerate() {
                table[ti * ns * nl + pi] = Self::cost(t, p, care);
            }
        }
        let at = |ti: usize, s: usize, k: usize| table[(ti * ns + s) * nl + k];
        let weight = |ti: usize| targets[ti].1 as f64;
        // The looks a set of glass colours can make, and those one more
        // colour adds to a set.
        let made = |glass: u32| -> Vec<usize> { (0..nl).filter(|k| needs[*k] & !glass == 0).collect() };
        let added = |glass: u32, more: u32| -> Vec<usize> { (0..nl).filter(|k| needs[*k] & !(glass | more) == 0 && needs[*k] & more != 0).collect() };
        // The error left with a set of solids and glass, target by target.
        let left = |solids: &[usize], glass: u32| -> Vec<f32> {
            let made = made(glass);
            (0..nt)
                .map(|ti| {
                    let mut best = f32::INFINITY;
                    for &s in solids {
                        for &k in &made {
                            best = best.min(at(ti, s, k));
                        }
                    }
                    best
                })
                .collect()
        };
        let sum = |current: &[f32]| -> f64 { (0..nt).map(|ti| weight(ti) * current[ti] as f64).sum() };
        // What a set would leave with one more solid, or one more glass.
        let with_solid = |current: &[f32], glass: u32, s: usize| -> f64 {
            let made = made(glass);
            (0..nt).map(|ti| weight(ti) * made.iter().fold(current[ti], |b, &k| b.min(at(ti, s, k))) as f64).sum()
        };
        let with_glass = |current: &[f32], solids: &[usize], glass: u32, more: u32| -> f64 {
            let added = added(glass, more);
            (0..nt)
                .map(|ti| {
                    let mut best = current[ti];
                    for &s in solids {
                        for &k in &added {
                            best = best.min(at(ti, s, k));
                        }
                    }
                    weight(ti) * best as f64
                })
                .sum()
        };
        let bits: Vec<u32> = colours.iter().map(|g| 1u32 << g).collect();

        // Start from the one stack that suits the whole scene best.
        let mut first = (0, 0, f64::INFINITY);
        for s in 0..ns {
            for k in (0..nl).filter(|k| needs[*k].count_ones() as usize <= max_glass) {
                let total: f64 = (0..nt).map(|ti| weight(ti) * at(ti, s, k) as f64).sum();
                if total < first.2 {
                    first = (s, k, total);
                }
            }
        }
        let (mut solids, mut glass) = (vec![first.0], needs[first.1]);
        let mut current = left(&solids, glass);
        // Then add the solid or the glass that helps the whole scene most,
        // one at a time. A rare tone still wins a place when nothing chosen
        // so far is near it: white water is rare, but nothing blue is like it.
        loop {
            let now = sum(&current);
            let mut pick: Option<(bool, usize, f64)> = None;
            if solids.len() < max_solids {
                for s in (0..ns).filter(|s| !solids.contains(s)) {
                    let total = with_solid(&current, glass, s);
                    if pick.is_none_or(|(_, _, b)| total < b) {
                        pick = Some((true, s, total));
                    }
                }
            }
            if (glass.count_ones() as usize) < max_glass {
                for (g, bit) in bits.iter().enumerate().filter(|(_, bit)| glass & **bit == 0) {
                    let total = with_glass(&current, &solids, glass, *bit);
                    if pick.is_none_or(|(_, _, b)| total < b) {
                        pick = Some((false, g, total));
                    }
                }
            }
            let Some((solid, which, total)) = pick else { break };
            if now - total <= least_gain.max(0.0) as f64 * now {
                break;
            }
            if solid {
                solids.push(which);
            } else {
                glass |= bits[which];
            }
            current = left(&solids, glass);
        }
        // Adding one at a time can settle on a block that a later one made
        // a poor choice. Try each chosen block against every other in its place.
        for _ in 0..3 {
            let mut changed = false;
            for i in 0..solids.len() {
                let mut rest = solids.clone();
                rest.remove(i);
                let without = if rest.is_empty() { vec![f32::INFINITY; nt] } else { left(&rest, glass) };
                let mut best = (solids[i], sum(&current));
                for s in (0..ns).filter(|s| !solids.contains(s)) {
                    let total = with_solid(&without, glass, s);
                    if total < best.1 * 0.995 {
                        best = (s, total);
                    }
                }
                if best.0 != solids[i] {
                    solids[i] = best.0;
                    current = left(&solids, glass);
                    changed = true;
                }
            }
            for bit in bits.iter().filter(|bit| glass & **bit != 0).copied().collect::<Vec<u32>>() {
                let rest = glass & !bit;
                let without = if rest == 0 { vec![f32::INFINITY; nt] } else { left(&solids, rest) };
                let mut best = (bit, sum(&current));
                for other in bits.iter().filter(|other| glass & **other == 0) {
                    let total = with_glass(&without, &solids, rest, *other);
                    if total < best.1 * 0.995 {
                        best = (*other, total);
                    }
                }
                if best.0 != bit {
                    glass = rest | best.0;
                    current = left(&solids, glass);
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }
        let made = made(glass);
        solids.iter().flat_map(|&s| made.iter().map(move |&k| (s, k))).map(|(s, k)| pairs[s * nl + k]).collect()
    }
}
