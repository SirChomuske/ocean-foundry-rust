//! Development tool: build a block catalogue from a Minecraft client jar.
//!
//! Reads the block list, the properties its block models mention, and the
//! textures, and measures each block's colour. The result is checked into
//! `assets/catalogue/` and compiled into the app; the app itself never
//! reads a game install.

use of_formats::catalogue::{is_gravity, tier_of, BlockInfo, Catalogue};
use serde_json::Value;
use std::collections::{BTreeMap, HashMap};
use std::io::Read;

struct Jar {
    blockstates: BTreeMap<String, Value>,
    models: HashMap<String, Value>,
    textures: HashMap<String, Vec<u8>>,
    version: Value,
}

fn load(path: &str) -> Result<Jar, String> {
    let file = std::fs::File::open(path).map_err(|e| format!("cannot open {path}: {e}"))?;
    let mut zip = zip::ZipArchive::new(file).map_err(|e| format!("{path} is not a jar: {e}"))?;
    let mut jar = Jar { blockstates: BTreeMap::new(), models: HashMap::new(), textures: HashMap::new(), version: Value::Null };
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i).map_err(|e| e.to_string())?;
        let name = entry.name().map_err(|e| e.to_string())?.to_string();
        let json = |entry: &mut dyn Read| -> Result<Value, String> {
            let mut text = String::new();
            entry.read_to_string(&mut text).map_err(|e| format!("{name}: {e}"))?;
            serde_json::from_str(&text).map_err(|e| format!("{name}: {e}"))
        };
        if name == "version.json" {
            jar.version = json(&mut entry)?;
        } else if let Some(stem) = name.strip_prefix("assets/minecraft/blockstates/").and_then(|n| n.strip_suffix(".json")) {
            jar.blockstates.insert(stem.to_string(), json(&mut entry)?);
        } else if let Some(stem) = name.strip_prefix("assets/minecraft/models/").and_then(|n| n.strip_suffix(".json")) {
            jar.models.insert(stem.to_string(), json(&mut entry)?);
        } else if let Some(stem) = name.strip_prefix("assets/minecraft/textures/").and_then(|n| n.strip_suffix(".png")) {
            let mut bytes = Vec::new();
            entry.read_to_end(&mut bytes).map_err(|e| format!("{name}: {e}"))?;
            jar.textures.insert(stem.to_string(), bytes);
        }
    }
    Ok(jar)
}

fn short(id: &str) -> &str {
    id.strip_prefix("minecraft:").unwrap_or(id)
}

/// Record `key=value` pairs from a variant key such as "facing=east,half=top".
fn note_pairs(props: &mut BTreeMap<String, Vec<String>>, text: &str) {
    for pair in text.split(',').filter(|p| !p.is_empty()) {
        if let Some((k, v)) = pair.split_once('=') {
            note(props, k, v);
        }
    }
}

fn note(props: &mut BTreeMap<String, Vec<String>>, key: &str, values: &str) {
    let seen = props.entry(key.to_string()).or_default();
    for v in values.split('|') {
        let v = v.trim_start_matches('!');
        if !seen.iter().any(|s| s == v) {
            seen.push(v.to_string());
        }
    }
}

fn note_when(props: &mut BTreeMap<String, Vec<String>>, when: &Value) {
    let Some(map) = when.as_object() else { return };
    for (k, v) in map {
        match (k.as_str(), v) {
            ("OR" | "AND", Value::Array(items)) => items.iter().for_each(|w| note_when(props, w)),
            (_, Value::String(s)) => note(props, k, s),
            (_, Value::Bool(b)) => note(props, k, &b.to_string()),
            (_, Value::Number(n)) => note(props, k, &n.to_string()),
            _ => {}
        }
    }
}

fn first_model(v: &Value) -> Option<&str> {
    match v {
        Value::Array(items) => items.first().and_then(first_model),
        other => other.get("model").and_then(Value::as_str),
    }
}

struct Resolved {
    textures: HashMap<String, String>,
    elements: Vec<Value>,
}

fn resolve(jar: &Jar, model: &str) -> Resolved {
    let mut out = Resolved { textures: HashMap::new(), elements: Vec::new() };
    let mut at = Some(short(model).to_string());
    let mut guard = 0;
    while let Some(name) = at.take() {
        guard += 1;
        let Some(m) = jar.models.get(&name).filter(|_| guard < 32) else { break };
        if let Some(t) = m.get("textures").and_then(Value::as_object) {
            for (k, v) in t {
                // Either a plain name or, in newer versions, an object naming a sprite.
                if let Some(v) = v.as_str().or_else(|| v.get("sprite").and_then(Value::as_str)) {
                    // The model nearest the block wins.
                    out.textures.entry(k.clone()).or_insert_with(|| v.to_string());
                }
            }
        }
        if out.elements.is_empty() {
            if let Some(e) = m.get("elements").and_then(Value::as_array) {
                out.elements = e.clone();
            }
        }
        at = m.get("parent").and_then(Value::as_str).map(|p| short(p).to_string());
    }
    out
}

impl Resolved {
    fn texture(&self, reference: &str) -> Option<String> {
        let mut r = reference.to_string();
        for _ in 0..16 {
            match r.strip_prefix('#') {
                Some(key) => r = self.textures.get(key)?.clone(),
                None => return Some(short(&r).to_string()),
            }
        }
        None
    }
}

fn corner(e: &Value, key: &str) -> [f64; 3] {
    let a = e.get(key).and_then(Value::as_array);
    let at = |i: usize| a.and_then(|a| a.get(i)).and_then(Value::as_f64).unwrap_or(0.0);
    [at(0), at(1), at(2)]
}

struct Measured {
    rgb: [u8; 3],
    alpha: u8,
    variation: f32,
}

fn to_linear(c: u8) -> f64 {
    let c = c as f64 / 255.0;
    if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}

fn to_srgb(c: f64) -> u8 {
    let c = if c <= 0.0031308 { c * 12.92 } else { 1.055 * c.powf(1.0 / 2.4) - 0.055 };
    (c * 255.0).round().clamp(0.0, 255.0) as u8
}

/// Average a texture in linear light, weighting by opacity. Animated
/// textures are tall strips of square frames; only the first is used.
fn measure(png: &[u8]) -> Option<Measured> {
    let img = image::load_from_memory_with_format(png, image::ImageFormat::Png).ok()?.to_rgba8();
    let (w, h) = (img.width(), img.height().min(img.width()));
    let (mut sum, mut weight, mut alpha_sum) = ([0f64; 3], 0f64, 0f64);
    let mut lumas = Vec::new();
    for y in 0..h {
        for x in 0..w {
            let p = img.get_pixel(x, y).0;
            let a = p[3] as f64 / 255.0;
            alpha_sum += a;
            if a > 0.0 {
                let lin = [to_linear(p[0]), to_linear(p[1]), to_linear(p[2])];
                for c in 0..3 {
                    sum[c] += lin[c] * a;
                }
                weight += a;
                lumas.push(0.2126 * lin[0] + 0.7152 * lin[1] + 0.0722 * lin[2]);
            }
        }
    }
    if weight == 0.0 {
        return None;
    }
    let mean = lumas.iter().sum::<f64>() / lumas.len() as f64;
    let variance = lumas.iter().map(|l| (l - mean).powi(2)).sum::<f64>() / lumas.len() as f64;
    Some(Measured {
        rgb: [to_srgb(sum[0] / weight), to_srgb(sum[1] / weight), to_srgb(sum[2] / weight)],
        alpha: (alpha_sum / (w * h) as f64 * 255.0).round() as u8,
        variation: ((variance.sqrt() * 1000.0).round() / 1000.0) as f32,
    })
}

fn describe(jar: &Jar, name: &str, state: &Value) -> BlockInfo {
    let mut props = BTreeMap::new();
    let mut model = None;
    if let Some(variants) = state.get("variants").and_then(Value::as_object) {
        for (key, v) in variants {
            note_pairs(&mut props, key);
            model = model.or_else(|| first_model(v));
        }
    }
    if let Some(parts) = state.get("multipart").and_then(Value::as_array) {
        for part in parts {
            if let Some(when) = part.get("when") {
                note_when(&mut props, when);
            }
            model = model.or_else(|| part.get("apply").and_then(first_model));
        }
    }
    for values in props.values_mut() {
        values.sort();
    }
    let id = format!("minecraft:{name}");
    let mut info = BlockInfo {
        props,
        top: None,
        side: None,
        alpha: None,
        variation: None,
        full_cube: false,
        tinted: false,
        gravity: is_gravity(&id),
        tier: tier_of(&id),
    };
    let Some(model) = model else { return info };
    let resolved = resolve(jar, model);
    let whole = resolved.elements.iter().find(|e| corner(e, "from") == [0.0; 3] && corner(e, "to") == [16.0; 3]);
    // A full cube is exactly one whole-cell box (overlays on the same box,
    // as on grass, are allowed).
    info.full_cube = whole.is_some() && resolved.elements.iter().all(|e| corner(e, "from") == [0.0; 3] && corner(e, "to") == [16.0; 3]);
    // Measure the whole-cell box if there is one, else the tallest part.
    let part = whole.or_else(|| {
        resolved.elements.iter().max_by(|a, b| corner(a, "to")[1].partial_cmp(&corner(b, "to")[1]).unwrap_or(std::cmp::Ordering::Equal))
    });
    let Some(part) = part else { return info };
    let face = |side: &str| -> Option<(Measured, bool)> {
        let f = part.get("faces")?.get(side)?;
        let texture = resolved.texture(f.get("texture")?.as_str()?)?;
        Some((measure(jar.textures.get(&texture)?)?, f.get("tintindex").is_some()))
    };
    if let Some((m, tinted)) = face("up") {
        info.top = Some(m.rgb);
        info.alpha = Some(m.alpha);
        info.variation = Some(m.variation);
        info.tinted |= tinted;
    }
    if let Some((m, tinted)) = face("north") {
        info.side = Some(m.rgb);
        info.tinted |= tinted;
    }
    info
}

pub fn build(jar_path: &str, out_path: &str, min_y: i32, max_y: i32) -> Result<(), String> {
    let jar = load(jar_path)?;
    let game_version = jar.version.get("id").and_then(Value::as_str).ok_or("the jar has no version.json id")?.to_string();
    let data_version = jar.version.get("world_version").and_then(Value::as_i64).ok_or("the jar has no world_version")? as i32;
    let mut blocks = BTreeMap::new();
    for (name, state) in &jar.blockstates {
        blocks.insert(format!("minecraft:{name}"), describe(&jar, name, state));
    }
    let catalogue = Catalogue { game_version, data_version, min_y, max_y, blocks };

    // One block per line, so a new version's file can be compared by eye.
    let mut text = format!(
        "{{\n\"game_version\": {},\n\"data_version\": {},\n\"min_y\": {},\n\"max_y\": {},\n\"blocks\": {{\n",
        serde_json::to_string(&catalogue.game_version).unwrap(),
        catalogue.data_version,
        min_y,
        max_y
    );
    let lines: Vec<String> = catalogue
        .blocks
        .iter()
        .map(|(name, info)| format!("{}: {}", serde_json::to_string(name).unwrap(), serde_json::to_string(info).unwrap()))
        .collect();
    text.push_str(&lines.join(",\n"));
    text.push_str("\n}\n}\n");
    std::fs::write(out_path, text).map_err(|e| format!("cannot write {out_path}: {e}"))?;

    let count = |f: &dyn Fn(&BlockInfo) -> bool| catalogue.blocks.values().filter(|b| f(b)).count();
    println!("Minecraft {} (data version {}), heights {} to {}", catalogue.game_version, catalogue.data_version, min_y, max_y);
    println!("{} blocks, {} full cubes, {} with a measured colour", catalogue.blocks.len(), count(&|b| b.full_cube), count(&|b| b.top.is_some()));
    println!(
        "{} survival, {} creative-only, {} technical, {} falling, {} biome-tinted",
        count(&|b| b.tier == of_formats::catalogue::Tier::Survival),
        count(&|b| b.tier == of_formats::catalogue::Tier::Creative),
        count(&|b| b.tier == of_formats::catalogue::Tier::Technical),
        count(&|b| b.gravity),
        count(&|b| b.tinted)
    );
    println!("{} usable as solid colour in survival without falling", catalogue.solid_colour_blocks(true, false).len());
    println!("Wrote {out_path}");
    Ok(())
}
