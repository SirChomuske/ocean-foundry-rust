//! Cross-format tests on hand-made files: the cases the real-file corpus
//! does not happen to contain.

use crate::export::{self, AirMode, ExportOptions, Selection};
use crate::model::{first_difference, BlockEntity, BlockState, Build, Format, Region, Volume};
use crate::nbt::{self, Compound, Tag};
use crate::{catalogue, import, legacy, write, Progress};
use std::sync::Arc;

fn chest_entity(pos: [i32; 3]) -> BlockEntity {
    let item = Compound::new().with("id", Tag::string("minecraft:prismarine")).with("count", Tag::Int(7)).with("Slot", Tag::Byte(3));
    BlockEntity { pos, id: Some("minecraft:chest".into()), data: Compound::new().with("Items", Tag::list(vec![Tag::Compound(item)])) }
}

/// Three regions: one ordinary, one stored with negative sizes far from the
/// origin at negative coordinates, one overlapping the first.
fn awkward_build() -> Build {
    let mut b = Build::new();
    b.data_version = Some(4790);
    b.meta.name = "Awkward".into();
    b.meta.author = "tester".into();
    b.meta.description = "three regions".into();
    let stone = b.palette.intern(BlockState::new("stone"));
    let air = b.palette.intern(BlockState::air());
    let stairs = b.palette.intern(BlockState::parse("minecraft:oak_stairs[facing=west,half=top,shape=inner_left,waterlogged=true]").unwrap());
    let chest = b.palette.intern(BlockState::parse("minecraft:chest[facing=north,type=single,waterlogged=false]").unwrap());
    let fill = |region: &mut Region, pick: &dyn Fn([i32; 3]) -> u32| {
        let mut v = Volume::new();
        let hi = region.max();
        for y in region.min[1]..=hi[1] {
            for z in region.min[2]..=hi[2] {
                for x in region.min[0]..=hi[0] {
                    v.set([x, y, z], pick([x, y, z]));
                }
            }
        }
        region.cells = Arc::new(v);
    };

    let mut first = Region::new("Main", [0, 0, 0], [5, 3, 4]);
    fill(&mut first, &|p| if p[1] == 0 { stone } else if p == [2, 1, 2] { chest } else { air });
    first.block_entities.push(chest_entity([2, 1, 2]));
    first.entities.push(Tag::Compound(Compound::new().with("id", Tag::string("minecraft:armor_stand"))));
    first.extra.set("SomethingNew", Tag::Long(42));

    let mut second = Region::new("Far corner", [-40, -70, -19], [9, 1, 7]);
    second.flipped = [true, false, true];
    fill(&mut second, &|p| if (p[0] + p[2]) % 2 == 0 { stairs } else { stone });

    let mut third = Region::new("Overlap", [3, 1, 1], [20, 17, 18]);
    fill(&mut third, &|p| if p[0] % 3 == 0 && p[1] > 5 { stairs } else { air });

    b.regions = vec![first, second, third];
    b.source = Some(Format::Litematic);
    b.extra.set("FutureTag", Tag::string("kept"));
    b
}

#[test]
fn litematica_keeps_regions_negative_sizes_entities_and_unknown_tags() {
    let original = awkward_build();
    let (bytes, notes) = write(&original, Format::Litematic, 4790, &Progress::none()).unwrap();
    assert!(notes.is_empty(), "{notes:?}");
    let back = import(&bytes, &Progress::none()).unwrap().build;
    assert_eq!(first_difference(&original, &back, true), None);
    assert_eq!(back.regions[1].flipped, [true, false, true]);
    assert_eq!(back.entity_count(), 1);
    assert_eq!((back.meta.name.as_str(), back.meta.author.as_str(), back.meta.description.as_str()), ("Awkward", "tester", "three regions"));
    assert_eq!(back.extra.str("FutureTag"), Some("kept"));
    assert_eq!(back.regions[0].extra.get("SomethingNew"), Some(&Tag::Long(42)));

    // The file itself states the far corner and negative sizes, as Litematica does.
    let (_, root) = nbt::read(&bytes).unwrap();
    assert_eq!(root.i32("Version"), Some(7));
    assert_eq!(root.i32("MinecraftDataVersion"), Some(4790));
    let far = root.compound("Regions").unwrap().compound("Far corner").unwrap();
    assert_eq!(crate::xyz(far.compound("Size").unwrap()), Some([-9, 1, -7]));
    assert_eq!(crate::xyz(far.compound("Position").unwrap()), Some([-32, -70, -13]));
    let meta = root.compound("Metadata").unwrap();
    assert_eq!(meta.i32("RegionCount"), Some(3));
    assert_eq!(crate::xyz(meta.compound("EnclosingSize").unwrap()), Some([63, 88, 38]));
    assert_eq!(meta.i32("TotalVolume"), Some(5 * 3 * 4 + 9 * 7 + 20 * 17 * 18));
}

#[test]
fn file_revision_follows_the_game_version() {
    assert_eq!(crate::litematic::file_version_for(4790), 7);
    assert_eq!(crate::litematic::file_version_for(3837), 7);
    assert_eq!(crate::litematic::file_version_for(3700), 6);
    assert_eq!(crate::litematic::file_version_for(2586), 5);
}

#[test]
fn several_regions_become_one_box_in_single_box_formats() {
    let original = awkward_build();
    for format in [Format::Sponge, Format::Axiom] {
        let (bytes, notes) = write(&original, format, 4790, &Progress::none()).unwrap();
        assert!(notes.iter().any(|n| n.key == "note.entities_dropped"), "{format:?} {notes:?}");
        let back = import(&bytes, &Progress::none()).unwrap().build;
        // Every block that is not air sits where it was; later regions win overlaps.
        let hi = original.bounds().unwrap();
        let mut seen = 0;
        for y in hi.0[1]..=hi.1[1] {
            for z in hi.0[2]..=hi.1[2] {
                for x in hi.0[0]..=hi.1[0] {
                    let want = original.get([x, y, z]).filter(|s| !s.is_air());
                    let got = back.get([x, y, z]).filter(|s| !s.is_air());
                    assert_eq!(want, got, "{format:?} at {:?}", [x, y, z]);
                    seen += want.is_some() as u32;
                }
            }
        }
        assert!(seen > 100);
        assert_eq!(back.block_entity_count(), 1);
        let be = back.regions.iter().flat_map(|r| &r.block_entities).next().unwrap();
        assert_eq!(be.pos, [2, 1, 2]);
        assert_eq!(be.id.as_deref(), Some("minecraft:chest"));
        assert!(be.data.same_content(&chest_entity([2, 1, 2]).data));
    }
}

fn old_sponge(version: i32, we_offset: bool) -> Vec<u8> {
    // 3 wide (x), 2 high (y), 2 long (z); index = x + z*W + y*W*L.
    let palette = Compound::new()
        .with("minecraft:air", Tag::Int(0))
        .with("minecraft:stone", Tag::Int(1))
        .with("minecraft:oak_log[axis=z]", Tag::Int(2));
    let data: Vec<u8> = vec![1, 1, 1, 1, 1, 1, 0, 2, 0, 0, 0, 2];
    let entity = Compound::new().with("Pos", Tag::IntArray(vec![1, 1, 0])).with("Id", Tag::string("minecraft:sign")).with("Colour", Tag::string("black"));
    let mut root = Compound::new()
        .with("Version", Tag::Int(version))
        .with("Width", Tag::Short(3))
        .with("Height", Tag::Short(2))
        .with("Length", Tag::Short(2))
        .with("PaletteMax", Tag::Int(3))
        .with("Palette", Tag::Compound(palette))
        .with("BlockData", Tag::ByteArray(data))
        .with("Offset", Tag::IntArray(vec![100, 64, -200]))
        .with(if version == 1 { "TileEntities" } else { "BlockEntities" }, Tag::list(vec![Tag::Compound(entity)]));
    if version >= 2 {
        root.set("DataVersion", Tag::Int(2586));
    }
    if we_offset {
        root.set("Metadata", Tag::Compound(Compound::new().with("WEOffsetX", Tag::Int(-1)).with("WEOffsetY", Tag::Int(-3)).with("WEOffsetZ", Tag::Int(5))));
    }
    nbt::write_gzip("Schematic", &root)
}

#[test]
fn older_sponge_revisions_are_read_and_saved_as_the_current_one() {
    for (version, we_offset, origin) in [(1, false, [100, 64, -200]), (2, true, [-1, -3, 5]), (2, false, [100, 64, -200])] {
        let imported = import(&old_sponge(version, we_offset), &Progress::none()).unwrap().build;
        assert_eq!((imported.source, imported.source_version), (Some(Format::Sponge), Some(version)));
        assert_eq!(imported.data_version, (version >= 2).then_some(2586));
        let at = |x: i32, y: i32, z: i32| imported.get([origin[0] + x, origin[1] + y, origin[2] + z]).unwrap().to_string();
        assert_eq!(at(0, 0, 0), "minecraft:stone");
        assert_eq!(at(1, 1, 0), "minecraft:oak_log[axis=z]");
        assert_eq!(at(2, 1, 1), "minecraft:oak_log[axis=z]");
        assert_eq!(at(0, 1, 0), "minecraft:air");
        let be = &imported.regions[0].block_entities[0];
        assert_eq!(be.pos, [origin[0] + 1, origin[1] + 1, origin[2]]);
        assert_eq!(be.data.str("Colour"), Some("black"));

        let (bytes, notes) = write(&imported, Format::Sponge, 4790, &Progress::none()).unwrap();
        assert_eq!(notes.iter().any(|n| n.key == "note.no_data_version"), version == 1);
        let (_, root) = nbt::read(&bytes).unwrap();
        let s = root.compound("Schematic").unwrap();
        assert_eq!(s.i32("Version"), Some(3));
        assert_eq!(s.i32("DataVersion"), Some(if version == 1 { 4790 } else { 2586 }));
        let back = import(&bytes, &Progress::none()).unwrap().build;
        assert_eq!(first_difference(&imported, &back, true), None);
    }
}

#[test]
fn legacy_files_are_translated_and_unknown_numbers_are_never_guessed() {
    // 2 x 1 x 3: stone, red wool, oak planks, glass, then two ids with no translation.
    let cells = [(1, 0), (35, 14), (5, 0), (20, 0), (1, 0), (35, 0)];
    let imported = import(&legacy::make_test_file([2, 1, 3], &cells), &Progress::none()).unwrap();
    assert!(imported.notes.is_empty(), "{:?}", imported.notes);
    let b = &imported.build;
    assert_eq!(b.source, Some(Format::Legacy));
    // The file's WorldEdit offset is (-1, 0, 2); cells run x first, then z.
    assert_eq!(b.get([-1, 0, 2]).unwrap().name, "minecraft:stone");
    assert_eq!(b.get([0, 0, 2]).unwrap().name, "minecraft:red_wool");
    assert_eq!(b.get([-1, 0, 3]).unwrap().name, "minecraft:oak_planks");
    assert_eq!(b.get([0, 0, 3]).unwrap().name, "minecraft:glass");
    assert_eq!(b.get([0, 0, 4]).unwrap().name, "minecraft:white_wool");
    let cat = catalogue::default();
    for format in Format::EXPORTABLE {
        let out = export::convert(b, format, cat, &Progress::none()).unwrap();
        let back = import(&out.bytes, &Progress::none()).unwrap().build;
        assert_eq!(first_difference(b, &back, false), None, "{format:?}");
    }
    assert_eq!(write(b, Format::Legacy, 4790, &Progress::none()).err().unwrap().key, "err.legacy_export");

    // An id above 255 (via AddBlocks) that has no translation.
    let odd = import(&legacy::make_test_file([2, 1, 1], &[(1, 0), (3000, 9)]), &Progress::none()).unwrap();
    let note = odd.notes.iter().find(|n| n.key == "note.legacy_unmapped").expect("unmapped ids are reported");
    assert!(note.args.iter().any(|(k, v)| *k == "list" && v.contains("3000:9")));
    assert!(legacy::is_placeholder(odd.build.get([0, 0, 2]).unwrap()));
    for format in Format::EXPORTABLE {
        assert_eq!(export::convert(&odd.build, format, cat, &Progress::none()).err().unwrap().key, "err.unknown_blocks");
    }
}

#[test]
fn axiom_keeps_empty_space_empty_across_cube_edges() {
    let mut b = Build::new();
    b.data_version = Some(4790);
    let glass = b.palette.intern(BlockState::new("cyan_stained_glass"));
    let air = b.palette.intern(BlockState::air());
    let chest = b.palette.intern(BlockState::parse("minecraft:chest[facing=east]").unwrap());
    let mut v = Volume::new();
    // A diagonal line through negative and positive cubes, with gaps.
    for i in -40..40 {
        if i % 5 != 0 {
            v.set([i, i / 2, -i], glass);
        }
    }
    v.set([-17, 3, 30], air);
    v.set([15, -1, -16], chest);
    let (lo, hi) = v.bounds().unwrap();
    let mut region = Region::new("line", lo, [hi[0] - lo[0] + 1, hi[1] - lo[1] + 1, hi[2] - lo[2] + 1]);
    region.cells = Arc::new(v);
    region.block_entities.push(chest_entity([15, -1, -16]));
    b.regions.push(region);

    let (bytes, _) = write(&b, Format::Axiom, 4790, &Progress::none()).unwrap();
    let back = import(&bytes, &Progress::none()).unwrap().build;
    assert_eq!(first_difference(&b, &back, false), None);
    assert_eq!(back.counts(), (64 + 1, 1));
    assert_eq!(back.get([0, 0, 0]), None, "a gap was filled in");
    assert_eq!(back.get([-17, 3, 30]).map(BlockState::is_air), Some(true), "deliberate air was lost");
    // Written again from the imported build, nothing changes.
    let (again, _) = write(&back, Format::Axiom, 4790, &Progress::none()).unwrap();
    assert_eq!(first_difference(&back, &import(&again, &Progress::none()).unwrap().build, true), None);
}

#[test]
fn damaged_files_are_refused_without_crashing() {
    let source = export::sample_build();
    let cat = catalogue::default();
    let mut seed = 0x2545F4914F6CDD1Du64;
    let mut next = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    let mut refused = 0;
    let mut files: Vec<Vec<u8>> = Format::EXPORTABLE
        .into_iter()
        .map(|format| {
            let options = ExportOptions { format, selection: Selection::Complete, air: AirMode::OmitAir, include_colour_glass: true };
            export::export(&source, &options, cat, &Progress::none()).unwrap().bytes
        })
        .collect();
    files.push(old_sponge(2, true));
    files.push(legacy::make_test_file([2, 1, 3], &[(1, 0), (35, 14), (5, 0), (20, 0), (1, 0), (35, 0)]));
    for file in &files {
        // Work on the uncompressed tags so damage reaches the readers
        // instead of being caught by the compression checksum.
        let plain = nbt::gunzip_if_needed(file).unwrap_or_else(|_| file.clone());
        for round in 0..400 {
            let mut bad = plain.clone();
            if round % 4 == 0 {
                bad.truncate(next() as usize % bad.len());
            } else {
                for _ in 0..1 + round % 3 {
                    let at = next() as usize % bad.len();
                    bad[at] = next() as u8;
                }
            }
            refused += import(&bad, &Progress::none()).is_err() as u32;
        }
    }
    assert!(refused > 200, "only {refused} damaged files were refused");
    assert_eq!(import(b"not a build at all", &Progress::none()).err().unwrap().key, "err.not_a_build");
    let unrelated = nbt::write_gzip("", &Compound::new().with("Data", Tag::Int(1)));
    assert_eq!(import(&unrelated, &Progress::none()).err().unwrap().key, "err.unknown_kind");
}

#[test]
fn the_sample_is_made_of_blocks_the_catalogue_knows() {
    let source = export::sample_build();
    let cat = catalogue::default();
    for state in source.palette.states() {
        assert!(cat.knows(state), "{state}");
    }
    assert_eq!(source.conflicts(&export::Layer::ALL), 0);
}

#[test]
fn omitting_air_keeps_every_block_and_writes_no_air() {
    let original = awkward_build();
    let (boxed, notes) = export::omit_air(&original, &Progress::none()).unwrap();
    assert!(notes.iter().any(|n| n.key == "note.omit_air"), "{notes:?}");
    assert!(notes.iter().any(|n| n.key == "note.entities_dropped"), "{notes:?}");

    // Through a real file and back, as Litematica would see it.
    let (bytes, _) = write(&boxed, Format::Litematic, 4790, &Progress::none()).unwrap();
    let back = import(&bytes, &Progress::none()).unwrap().build;
    assert_eq!(back.regions.len(), boxed.regions.len());
    assert!(back.regions.len() > 1);

    let mut written = 0u64;
    for region in &back.regions {
        region.for_each(|p, id| {
            assert!(!back.palette.states()[id as usize].is_air(), "air written at {p:?}");
            written += 1;
        });
    }
    let (lo, hi) = original.bounds().unwrap();
    let mut wanted = 0u64;
    for y in lo[1]..=hi[1] {
        for z in lo[2]..=hi[2] {
            for x in lo[0]..=hi[0] {
                let want = original.get([x, y, z]).filter(|s| !s.is_air());
                assert_eq!(want, back.get([x, y, z]), "at {:?}", [x, y, z]);
                wanted += want.is_some() as u64;
            }
        }
    }
    // Equal counts mean no box overlaps another and nothing was written twice.
    assert_eq!(written, wanted);

    assert_eq!(back.block_entity_count(), 1);
    let be = back.regions.iter().flat_map(|r| &r.block_entities).next().unwrap();
    assert_eq!(be.pos, [2, 1, 2]);
    assert!(be.data.same_content(&chest_entity([2, 1, 2]).data));

    // A build of nothing but air has nothing to write.
    let mut empty = Build::new();
    let air = empty.palette.intern(BlockState::air());
    let mut region = Region::new("Air", [0, 0, 0], [2, 2, 2]);
    let mut v = Volume::new();
    v.set([1, 1, 1], air);
    region.cells = Arc::new(v);
    empty.regions = vec![region];
    assert_eq!(export::omit_air(&empty, &Progress::none()).err().unwrap().key, "err.empty");
}

/// Every refusal and remark this crate (and the app's file window) can
/// produce has wording in both languages, with the same placeholders.
#[test]
fn every_message_has_wording_in_both_languages() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut sources = vec![root.join("crates/of-app/src/files.rs")];
    for entry in std::fs::read_dir(root.join("crates/of-formats/src")).unwrap() {
        sources.push(entry.unwrap().path());
    }
    let mut keys = std::collections::BTreeSet::new();
    for path in sources {
        let text = std::fs::read_to_string(&path).unwrap();
        for prefix in ["\"err.", "\"note."] {
            let mut rest = text.as_str();
            while let Some(at) = rest.find(prefix) {
                let tail = &rest[at + 1..];
                let end = tail.find('"').unwrap();
                let key = &tail[..end];
                if key.len() > prefix.len() - 1 && key.chars().all(|c| c.is_ascii_lowercase() || c == '_' || c == '.') {
                    keys.insert(key.to_string());
                }
                rest = &tail[end..];
            }
        }
    }
    assert!(keys.len() >= 20, "{keys:?}");
    let placeholders = |s: &str| -> std::collections::BTreeSet<String> {
        s.split('{').skip(1).filter_map(|part| part.split_once('}').map(|(name, _)| name.to_string())).collect()
    };
    let load = |name: &str| -> serde_json::Value {
        serde_json::from_str(&std::fs::read_to_string(root.join("assets/lang").join(name)).unwrap()).unwrap()
    };
    let (en, zh) = (load("en.json"), load("zh-CN.json"));
    let mut missing = Vec::new();
    for key in &keys {
        let full = format!("file.{key}");
        match (en.get(&full).and_then(|v| v.as_str()), zh.get(&full).and_then(|v| v.as_str())) {
            (Some(a), Some(b)) => assert_eq!(placeholders(a), placeholders(b), "{full}"),
            _ => missing.push(full),
        }
    }
    assert!(missing.is_empty(), "no wording for {missing:?}");
}
