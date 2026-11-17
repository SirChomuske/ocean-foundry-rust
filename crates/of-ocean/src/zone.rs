//! Zones: round areas of the plot whose water is treated differently.
//! Calm water has smaller waves and less foam, easing back to the open sea
//! at its edge; a keep-dry area has no water at all.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ZoneKind {
    /// Smaller waves and less foam, as in a harbour.
    #[default]
    Calm,
    /// No water, so something can be built there.
    Dry,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Zone {
    pub kind: ZoneKind,
    /// The middle, as shares of the plot's width (west to east) and length
    /// (north to south), each 0 to 1.
    pub at: [f64; 2],
    /// How far it reaches from the middle, in blocks.
    pub radius: f64,
    /// For calm water, how much of the waves is taken away in the middle:
    /// 0 leaves them, 1 leaves the water flat.
    pub strength: f64,
}

impl Default for Zone {
    fn default() -> Self {
        Self { kind: ZoneKind::Calm, at: [0.5, 0.5], radius: 16.0, strength: 0.8 }
    }
}

impl Zone {
    /// A zone of this kind from `x,z[,radius[,strength]]`.
    pub fn parse(kind: ZoneKind, text: &str) -> Result<Zone, String> {
        let bad = || format!("expected x,z[,radius[,strength]] with numbers, got '{text}'");
        let n: Vec<f64> = text.split(',').map(|v| v.trim().parse::<f64>().map_err(|_| bad())).collect::<Result<_, _>>()?;
        if n.len() < 2 || n.len() > 4 {
            return Err(bad());
        }
        let standard = Zone::default();
        Ok(Zone {
            kind,
            at: [n[0], n[1]],
            radius: n.get(2).copied().unwrap_or(standard.radius),
            strength: n.get(3).copied().unwrap_or(standard.strength),
        })
    }

    /// How far block `(x, z)` of a `width` x `length` plot is from the
    /// middle, in blocks.
    fn distance(&self, x: i32, z: i32, width: i32, length: i32) -> f64 {
        let (cx, cz) = (self.at[0] * width as f64, self.at[1] * length as f64);
        ((x as f64 + 0.5 - cx).powi(2) + (z as f64 + 0.5 - cz).powi(2)).sqrt()
    }
}

/// The zones of a plot, asked about one block at a time.
#[derive(Debug, Clone, Copy)]
pub struct Zones<'a> {
    zones: &'a [Zone],
    width: i32,
    length: i32,
}

impl<'a> Zones<'a> {
    pub fn new(zones: &'a [Zone], width: i32, length: i32) -> Self {
        Self { zones, width, length }
    }

    pub fn is_empty(&self) -> bool {
        self.zones.is_empty()
    }

    /// How much of the waves is left at a block: 1 in the open sea, less in
    /// calm water. The calm is full over the inner half of a zone and
    /// eases out to nothing at its edge.
    pub fn calm(&self, x: i32, z: i32) -> f64 {
        let mut left = 1.0;
        for zone in self.zones.iter().filter(|z| z.kind == ZoneKind::Calm) {
            let r = zone.radius.max(1.0);
            let d = zone.distance(x, z, self.width, self.length);
            if d < r {
                let t = ((r - d) / (0.5 * r)).clamp(0.0, 1.0);
                let reach = t * t * (3.0 - 2.0 * t);
                left *= 1.0 - zone.strength.clamp(0.0, 1.0) * reach;
            }
        }
        left
    }

    /// Whether a block is to be kept dry.
    pub fn dry(&self, x: i32, z: i32) -> bool {
        self.zones.iter().any(|zone| zone.kind == ZoneKind::Dry && zone.distance(x, z, self.width, self.length) <= zone.radius.max(0.5))
    }
}
