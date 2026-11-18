# Ocean Foundry: hand-off (2026-11-17)

For whoever picks up the work next, human or Claude: the rules in force, how to work here, the current state and what is left.

## 1. Rules (from the user; each with its reason)

- **Every test run goes through the capped runner.** This covers cargo test, the CLI, the app and the regression check.
  *Why:* on 2026-11-16 an uncapped run filled the PC's memory and forced a restart.
  ```
  pwsh -NoProfile -File scripts\capped-run.ps1 [-CapMB 2048] [-TimeoutS 300] [-Name label] -- <program> [args...]
  ```
  - It uses a Windows Job Object with a hard cap. It also stops when the PC's free RAM falls below 3 GB or its free commit below 6 GB.
  - It writes logs to `test-logs\runs\<stamp>-<name>\` and `test-logs\runs.csv`, and failures to `test-logs\crashes\`.
  - Exit codes: 120 cap, 121 PC memory low, 122 crash, 123 refused, 124 timeout.
  - Typical peaks: `cargo test --workspace --release` about 460 MB; the regression check about 80 MB.
- **No run estimated above 8 GB, and no statue ("dog") run, without the user's yes in chat.** Only then pass `-UserApproved`.
  *Why:* the user's explicit rule after the crash.
- **When a value read from a file or a marker becomes a size, check its range before allocating.**
  *Why:* this caused both 2026-11-16 bugs. One was the `DRY` marker used in arithmetic in the `build.rs` refill. The other was a `.schem` palette index that sized a table at 8 GB.
- **Dev builds only:** `scripts\dev-build.ps1`. Never run `package.ps1`. Never modify `dist\OceanFoundry-dev\OceanFoundry-data`; use a copy or a fresh data dir.
  *Why:* the user is still trying the method and sending feedback; that folder holds their settings.
- **Default output stays unchanged:** `scripts/regression/run.sh` must give 41/41.
  *Why:* new features must not quietly change the accepted seas.
- **Tuning numbers** go in `ocean-tuning.json` (the `Tuning` structs in `crates/of-ocean/src/tuning.rs`) and are documented in `assets\TUNING.txt`.
  *Why:* the method must stay adjustable without a rebuild.
- **Every control gets a tooltip in EN and zh-CN** (`<key>.help`). No paragraphs in the UI.
  *Why:* the user wants a clean UI with help on hover.
- **Colour rules:**
  - the sun's direction leads;
  - no grain;
  - darkness is never tied to wave size;
  - black only by the user's choice;
  - change in small steps.
  *Why:* the user's verdicts on earlier looks. The current look is accepted.
- **A brought-in build is shape only.** Only whether a cell holds a block matters. Generated land is for testing only. Exports carry no air.
  *Why:* "we are making oceans for the build not making a model editor for the build."
- **On hold until the user says otherwise:**
  - lava mode;
  - wake "V shape" polishing;
  - storm foam (`ocean.foam_streaks` stays 0);
  - the statue run.
- **Minecraft Java 26.1.2 only. Ignore `ocean-foundry-v2`.** In-game placement must be confirmed by the user.
  *Why:* the user's target, and Minecraft can't be run from here.

## 2. Environment

- cargo is not on PATH. In bash: `export PATH="$USERPROFILE/.cargo/bin:$PATH"`.
- Build (release only): `RUSTFLAGS="-C target-feature=+crt-static" cargo build --release -p of-app -p of-cli`
- Tests: `cargo test --workspace --release`. That is 117 tests, run under the cap.
- Regression check: `pwsh -NoProfile -File scripts/capped-run.ps1 -CapMB 4096 -Name regression -- bash scripts/regression/run.sh`. Add `update` after `run.sh` only once the user has accepted a new look.
- Python: use `python -X utf8`, **without** `-I`, for scripts that import Pillow. `-I` hides the user's site-packages.
- Headless app:
  ```
  target/release/of-app.exe --data-dir <dir> --mode ocean --lang en|zh-CN --appearance light|dark --size WxH [--land key] [--bake] --screenshot out.png
  ```
  The full option list is in `scripts/README-DEV.txt`.
- Strings check: `of-cli strings` (595 EN / 595 zh-CN at hand-off).
- Memory measurements (column plan, stage 1): `bash scripts/measure/run.sh [quick|figures]`. Results per stage are kept in `test-logs/measure/table-stageN.md` and `target/measure-stageN/` (N = 1–7). Each case is its own capped run, and a series stops at its first cap. The table goes to `test-logs/measure/table.md`.
  - `of-cli ocean ... memory=report` adds each step's peak and each structure's size.
  - `of-cli testbuild <file> <height>` writes a hollow test figure up to 1,536 tall, a stand-in for the statue.
- Golden registry (`tests/golden`): `OF_UPDATE_GOLDEN=1 cargo test -p of-core`.
- Dev build, in PowerShell: `& .\scripts\dev-build.ps1 -Test`

## 3. State at hand-off

**Verified** (under the cap):
- 117/117 tests pass;
- regression 41/41;
- strings complete;
- dev build refreshed.

**Testing:** the tester is the Sonnet session "Fable project build completion" (`local_8d53d435-010c-4a2d-985d-2728fe5d581a`). It has been paused since the crash; ask the user before restarting it. Its rules:
- no source edits;
- no cargo in the workspace;
- test a copy of the dev build;
- the capped runner for every run.

## 4. Open work

1. **Column bake plan** (https://claude.ai/artifact/XqauVCSRpNTR135uHEQLL7). Stages 1 (measure), 2 (cubes of one value share a copy; shape read column by column), 3, 4, 5, 6 and 7 are done. The statue itself was run on 2026-11-17 (item 2).
   - Stage 7: `of_formats::survey::litematic(bytes)` reads a .litematic twice as a stream (corners, sizes and palettes, then the block data cube by cube, as the reader fills `Volume`), holding a few MB. It gives the cubes, the stored cubes, the unpacked size and the loading peak (`max(2 × unpacked, block data + build)`). `of-cli estimate <build> [ocean.*]` prints it with `estimate::shape(cubes, settings)` (24 B per cube + 17 B per column of ground) and `estimate::job_on_build`. The CLI's `ocean` surveys a .litematic before loading it, and refuses over the limit (the 1,417 hull at `--max-memory 1000`: refused at 9 MB after 6 s). The app does not survey yet; its estimate comes after loading, as in stage 6. The ramp: hull 420 on 512 → 141 MB, 860 on 1,024 → 597 MB, 1,417 on 2,048 (a 5.8 G-cell box) → 1,909 MB whole run, 704 s (436 s of it the pictures). On 2,048 the bake adds 890 MB whatever the box (1.7, 3.5 and 5.8 G cells); the rest is the loaded build, which follows its blocks. Survey estimates are within 1.4% of the job peak on every ramp case. Tables: `test-logs/measure/table-stage7.md` (`scripts/measure/ramp.py`, `estimate.py`); runs in `target/measure-stage7/` (the 1,417 hull is `figure-1417.litematic` there).
   - Stage 6: one memory limit per process (`of_core::memory`): `--max-memory MB` on the CLI and the app (taken out by `take_flag` before other arguments), else 40% of free RAM, at least 256 MB. `Progress::step` refuses with `of_formats::MEMORY` (`err.memory`) once committed memory is over the limit (read at most every 5 ms), so every job that reports progress is guarded. Gunzip reads in 4 MB pieces with `try_reserve`; `render::Grid::new` and the bake's per-column maps (`memory::filled`) use `try_reserve`; the app thins textures over the card's `max_texture_side`; `GpuRunner::new` refuses buffers over the device's limits. Before a bake, `of_ocean::estimate::job` (bytes per column for open sea, demo land, a build; from the stage 5 peaks) plus what is held is checked with `memory::fits`; over the limit, the CLI and app refuse with the size and limit (`file.err.memory_estimate`). The estimate is within about 11% of the job's peak on every stage 1 case and within 5% on sizes it was not fitted to (`test-logs/measure/estimate-stage6.md`). Not estimated, only guarded: loading a build, and the CLI's read-back checks (up to +93% over the job on small figures). `tests/memory_limit.rs`: a limit 4 MB over what is held refuses a 2048² plot and a 400 MB gzip bomb, using under 16 MB. At `--max-memory 200` the 860 figure was stopped at 216 MB while loading (the 5 ms throttle). Baselines: `test-logs/measure/table-stage6.md`, `target/measure-stage6/`.
   - Stage 5: the 3D view reads the same `BlockSource`. `render::Scene::new(&dyn BlockSource, layers, catalogue)` builds a `Grid` of runs up each column (`starts` per column, `runs` of `{lo, hi, id}`), read row by row in parallel bands of Z. `render::picture(&scene, camera)` draws it; `render::render` does both. `MOST_RUNS` (200 million runs, about 1.2 GB) replaces the old 600-million-cell limit; a box taller than 65,535 is refused. The app preview, CLI pictures and lava all use `baked.source(None)`, so nothing but tests calls `Baked::layers()` now. Pictures are byte-identical to stage 4; app screenshots match (`target/shots/stage5/`). Picture peak on the 1024 storm sea 558 → 91 MB; 420 figure 1,428 → 195 MB; the 860 figure is now drawn (scene 24 MB, run 598 MB). Baselines: `test-logs/measure/table-stage5.md`, `target/measure-stage5/`. The view of a brought-in build keeps real block colours; that choice is still the user's.
   - Stage 4: exports read from `of_formats::export::BlockSource` (`name`, `palette`, `bounds`, `read_row`, `conflicts`). It is implemented for `LayeredBuild` and for `of_ocean::BakeSource`, which comes from `Baked::source(area)` and works rows out from the columns. `export::export(&dyn BlockSource, ..)` counts in one pass, finds Omit-air boxes from a bit per cell, and calls `of_formats::write_from(build, Some(rows), ..)`. The build there only gives the region layout. The writers stream through `nbt::Stream` and `pack::TightStream`. The CLI drops the layers after the pictures; the app's save never makes them. Omit air now defaults to on in the ocean Save panel (`of-app/src/ocean.rs`). The `--export` automated launch still takes it from `--files-omit-air`, and the Files converter is unchanged. The complete-build export is kept. The 3D view and lava moved off `Baked::layers()` in stage 5. Each export adds 1–6 MB over the bake at 1024. .bp files match stage 3 only once unzipped. Baselines: `test-logs/measure/table-stage4.md`, `target/measure-stage4/`. The user still has to confirm placement in Minecraft.
   - Stage 3: `Baked.blocks` (`of-ocean/src/blocks.rs`) keeps columns: water top/bottom, ground, stack id, and the land as a link to the loaded build or ground runs. `Baked::layers()` lays out the six layers on demand, for the exporters and the 3D view (the app's preview lays them out each time it makes a scene). `Baked::for_each_block` drives both. The kept bake for the 1024 storm sea is 16 MB, against 427 MB of layers. Peaks are unchanged until stages 4–5 stop taking the layers. Baselines: `test-logs/measure/table-stage3.md` and `target/measure-stage3/`; stage 2's are in `target/measure-stage2/` (each case has a `report.txt`). Left from stage 2: region-edge cubes mix air and absent and can't be shared. `roundtrip` on a large figure runs out of memory in `first_difference` (a hash map of every cell). It makes memory scale with the water, not the bounding box. One decision is still the user's: the 3D view of a brought-in build, real block colours or a neutral shape (stage 5). Stage 4 settled the other two by default: the complete-build export is kept, and Omit air is the default. Either can be undone if the user says so.
2. **Statue run: done 2026-11-17, on the user's yes, under a 4 GB cap.** Results: `test-logs/measure/statue-stage7.md`. The file is kept in `target/statue/statue.litematic`.
   - **CLI:** 579 s, peak 1,905 MB (estimate 1,819). Bake 40 s; pictures 412 s.
   - **App:** 50 s to the finished 3D view, peak 2.3 GB. A still view draws in 3.7 s.
   - **Files:** the water-only files are 4–5 MB. The complete files are refused, because the statue is taller than the world.
   - **Fixes this run needed:**
     - `BakeSource::terrain_row` reads only the regions whose box holds the row. Before, the statue's 1,989 regions made the 3D scene take over an hour.
     - `memory=report` prints live progress.
     - The app prints 3D view timings to stderr.
     - The automated screenshot waits up to 30 min for a bake.
   - **Faster 3D drawing:** `render::Grid` keeps patch heights (`Tops`, patches of 64, 16 and 4 columns), so a line of sight jumps across the air above a patch.
     - Still view in the app: 3.7 s to 0.71 s.
     - CLI pictures: 412 s to 75 s; the whole CLI run takes 242 s.
     - Output is identical.
   - Any rerun still needs the user's yes.
   - **After the statue (same day):**
     - Block entities (chests, signs and the like) of a brought-in build are kept in the complete files of all three formats; water-only files have none. Blocks covered by a later region drop their entity. Test: `block_entities_of_a_build_reach_the_complete_files`.
     - The land card ("Land, rivers and waterfalls") is now the first card in the ocean panel, so a build is brought in before anything else is set (user's request).
     - **Dev build vs package (user's request, 2026-11-17):** lava (on hold) and the Coast and Cliffs lands are compiled into the dev build only. They are not just hidden: the package's programs have no code for them.
       - Feature `dev` on `of-core` (Mode::Lava, the lava.* settings, their strings), `of-ocean` (the `lava` module, `Land::Coast`/`Cliffs`, `terrain/presets.rs`), `of-cli` and `of-app`. `of-core/build.rs` writes the language tables and leaves out lava/coast/cliffs strings without `dev`.
       - `dev-build.ps1` builds and tests with `--features of-app/dev,of-cli/dev`; `package.ps1` is unchanged and builds without them.
       - Regression uses coast and cliffs, so it needs the dev form in `target/release`. `run.sh` now refuses a package build. After `package.ps1`, run `dev-build.ps1` (or build with the features) before the regression.
       - Tests: 118 in the dev form, 106 in the package form; regression 41/41 on the dev form.
       - A package app with saved lava/coast/cliffs settings sets the unreadable file aside and starts fresh (the existing settings-file behaviour).
     - **Release package made 2026-11-17, on the user's yes** (the never-package rule still stands otherwise): `dist/OceanFoundry-0.1.0-win64.zip`, 25.4 MB (remade the same day after the height change below). Its sample test runs are open sea only (calm, moderate, storm). The app shows no Ocean/Lava switch and offers Open sea and Your build.
     - **No world-height refusal on export (2026-11-17, user's request):** a file holds positions relative to the build only, so export no longer checks the build against Minecraft's -64..319. `err.height`, `check_height` and `ExportOptions.placement_y` are gone; Placement Y only sets the paste hint ("paste with the lowest layer at Y …").
     - **Rivers and ponds over an imported build (2026-11-17, user's request, delta excluded):** the scene card shows rivers (width and winding, no delta slider, no land-shape number) and ponds for Your build. `SceneSetup::laid_with` lays them with delta 0 and notes the changed columns in `Placed::reworked`; the bake (`blocks::Reworked`) cuts the build away above the new ground and under the water in those columns only, and adds the water blocks. Falls stop at the build's inside. Test: `rivers_and_ponds_run_over_a_build`. In the release package (remade 22:27, 25.4 MB).
     - Dev build 2026-11-17 21:33.
     - **Radiating streaks fixed (2026-11-18, user's request):** straight lines fanning out from every corner of land or of a shallow shelf, and broad X-shaped creases.
       - Fans: the shelter step (`shore.rs`, `Shore::new`) looked along 15 lines, each blocked or not, and took the shallowest water along the line alone. Now each line stands for a wedge as wide as the gap to the next: land passing through part of it blocks that share, read from a signed distance field (`side_at`), and the shallows are averaged over the wedge's width through a summed-area table (`shoal_at`). No new settings.
       - Creases: `survey.rs` `distances()` was a chamfer distance (octagons), whose slope jumps on the diagonals from corners; the shore reflection follows that slope. It now keeps the nearest marked column and measures straight-line distance to it (two sweeps each way). Also used for shore kinds, `off` and channels.
       - Ruled out by test bakes: the Kelvin wake (~4% change), shadows, ripples, crest sharpness, swell detail, the refraction delay.
       - Test: `shelter_and_distance_have_no_seams_behind_a_corner`. Tests 120 dev form. Regression: 17 open-sea pictures same, the 24 coast/cliffs/bright pictures changed (~2% of pixels, near shores); the user accepted them and the baseline was updated 2026-11-18 02:20. Regression 41/41.
       - **Speed (same day, user saw 5 min → 20 min):** the soft rays read about a dozen values a step everywhere. Now a step farther from land and from any sudden change in depth (`clear`, a distance field) than its wedge is wide reads one value, and a line stops once under 0.5% gets through. 1536×1280 test bake: 107 s before the streak fix, 267 s with it, 66 s now. Pictures within 0.03% of the slow version; regression: 23 pictures differ from the new baseline by at most 0.014% of pixels, accepted by the user, baseline updated 2026-11-18 02:40. Regression 41/41.
       - Pictures: `target/shots/streaks/fix-full.png`, `fix-plain.png`, `regression-before-after.png`. Dev build 2026-11-18 02:17. Package not remade.
3. **Not yet checked by the tester** (from the previous session):
   - ripple library and ripple image patterns;
   - the imagined sea floor;
   - creative-only blocks;
   - the Surface/Glass layers;
   - the Depth/Foam/Normals/Reflection views;
   - the cutaway;
   - export of part of the plot.
   Also check, in EN and zh-CN, that the 9-segment view row and the 5-segment layer row fit small windows.
4. **Clean-up found but not done** (list only; wait for the user's go):
   - **Unused pub functions:**
     - `of-core/src/jobs.rs:70 is_cancelled`
     - `of-core/src/settings.rs:117 bake_for`
     - `of-formats/src/litematic.rs:269 stated_corner`
     - `of-formats/src/model.rs:39 prop`
   - **Duplicates:**
     - the chunk-cell decode (`build.rs` / `terrain.rs`);
     - the palette remap (sponge / litematic / axiom);
     - the bounds-over-layers loop (`render.rs`, twice).
   - **Files over 800 lines:**
     - `of-app/src/ocean.rs` (split into viewport / export / scene / tuning);
     - `build.rs` (split `bake_with` together with plan stage 3);
     - `terrain.rs` (test scenes to `test_scenes.rs`);
     - `app.rs` (top bar / panel / status);
     - `lib.rs` (tests to `tests.rs`).
5. **Scope still missing:**
   - painted zones and a full colour solver;
   - standalone pond or lake modes;
   - JPEG and photo-texture synthesis for ripple images;
   - a keep-dry brush;
   - the export area in the CLI, and dragging it on the map;
   - river deltas and waterfalls as their own boxes in a brought-in build.
