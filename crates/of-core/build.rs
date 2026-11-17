//! The language tables built into the program. The development build
//! (feature "dev") takes them as they are; the package leaves out the
//! strings of lava (on hold) and of the coast and cliffs lands, which the
//! package does not have.

use std::path::Path;

fn main() {
    let out = std::env::var("OUT_DIR").expect("OUT_DIR is set by cargo");
    let dev = std::env::var_os("CARGO_FEATURE_DEV").is_some();
    for name in ["en.json", "zh-CN.json"] {
        let from = Path::new("../../assets/lang").join(name);
        println!("cargo:rerun-if-changed={}", from.display());
        let text = std::fs::read_to_string(&from).unwrap_or_else(|e| panic!("cannot read {}: {e}", from.display()));
        let kept = if dev {
            text
        } else {
            let table: serde_json::Map<String, serde_json::Value> =
                serde_json::from_str(&text).unwrap_or_else(|e| panic!("language table {name} is invalid: {e}"));
            let table: serde_json::Map<String, serde_json::Value> = table.into_iter().filter(|(k, _)| !dev_only(k)).collect();
            serde_json::to_string_pretty(&table).expect("a table writes back")
        };
        std::fs::write(Path::new(&out).join(name), kept).expect("OUT_DIR is writable");
    }
}

/// Strings only the development build uses.
fn dev_only(key: &str) -> bool {
    key.split(['.', '_']).any(|part| part == "lava") || key.starts_with("scene.land.coast") || key.starts_with("scene.land.cliffs")
}
