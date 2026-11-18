//! Headless command line for Ocean Foundry. Everything the app does below
//! the UI can be driven from here, which is how it is tested and diagnosed.

mod catalogue_tool;
mod files;
mod mem;
mod ocean;

use of_core::i18n::{self, Lang};
use of_core::jobs::{coarse_bytes, spawn, JobCtx, Outcome};
use of_core::params::{self, Kind, Mode, ParamValues, Stage, Value};
use of_core::settings::ComputeChoice;
use of_core::stage::{fingerprint, freshness, BakeStamp};
use of_gpu::{CpuReason, Resolved};
use std::process::ExitCode;

#[cfg(feature = "dev")]
const USAGE: &str = "\
Ocean Foundry command line

USAGE:
  of-cli [--max-memory MB] <command> ...
                                     Every command keeps under a memory limit: MB, or 40% of
                                     the memory free when it starts. A job estimated over it is
                                     refused; one that goes over it is stopped
  of-cli version                     Show the version
  of-cli devices [--cpu]             List calculation devices and the one that would be used
  of-cli bench [--cpu]               Measure calculation speed
  of-cli params [ocean|lava]         List parameters, ranges and defaults
  of-cli strings                     Check the language tables
  of-cli fingerprint <mode> [id=value ...]
                                     Show stage fingerprints, and which stages a change invalidates

BUILD FILES (.litematic, .schem, .schematic, .bp):
  of-cli inspect <file>              Describe a build file
  of-cli convert <in> <out>          Re-save in the format of the output name
  of-cli roundtrip <file|folder>...  Import, export and re-import; report any difference
  of-cli sample <folder>             Write the sample build in every format and selection
  of-cli nbt <file> [depth]          Show a file's raw tag tree

OCEAN (development version):
  of-cli ocean <folder> <name> [coast|cliffs] [build=FILE] [waterline=SHARE] [offset=EAST,SOUTH]
               [tuning=FILE] [stream=...] [streams=none] [id=value ...]
                                     Bake an ocean; write pictures and build files. 'coast' adds
                                     the demonstration coast, 'cliffs' the cliff coast with its
                                     arch, sea stacks and waterfall. tuning=FILE reads the
                                     method's internal numbers from a tuning file.
                                     stream=x1,z1,x2,z2[,width[,winding[,delta]]] places a river
                                     (positions as shares of the plot); streams=none drops the
                                     land's own. With land, <name>-shore.png shows how the shore
                                     was read. Settings are ocean.* ids (see params).
                                     project=FILE starts from a saved .ofproject (later
                                     settings override it); exclude=BLOCK,... leaves blocks out;
                                     area=W,N,E,S saves only that part of the plot;
                                     pattern_image=FILE gives the ripples' image;
                                     memory=report adds the memory each step and
                                     structure held
  of-cli project <file.ofproject> <folder> [id=value ...]
                                     Bake a saved project exactly as the app would
  of-cli lava <folder> <name> [tuning=FILE] [id=value ...]
                                     Bake a field of lava; write pictures and build files.
                                     Settings are lava.* ids (see params)
  of-cli testruns <folder>           Write the ocean test runs: every scene in a calm, a
                                     moderate and a stormy sea

DEVELOPMENT:
  of-cli stress [size]               Time every export on a large wavy surface (default 512)
  of-cli estimate <build.litematic> [ocean.* settings]
                                     Say how much memory loading the build and baking an
                                     ocean around it would take, without loading it
  of-cli testbuild <file> [height]   Write a hollow test figure (16 to 1536 tall, default 96)
                                     to try a build as the land
  of-cli catalogue <client.jar> <out.json> [min-y max-y]
                                     Build a block catalogue from a Minecraft install
";

/// The package has no lava and no demonstration lands.
#[cfg(not(feature = "dev"))]
const USAGE: &str = "\
Ocean Foundry command line

USAGE:
  of-cli [--max-memory MB] <command> ...
                                     Every command keeps under a memory limit: MB, or 40% of
                                     the memory free when it starts. A job estimated over it is
                                     refused; one that goes over it is stopped
  of-cli version                     Show the version
  of-cli devices [--cpu]             List calculation devices and the one that would be used
  of-cli bench [--cpu]               Measure calculation speed
  of-cli params                      List parameters, ranges and defaults
  of-cli strings                     Check the language tables
  of-cli fingerprint <mode> [id=value ...]
                                     Show stage fingerprints, and which stages a change invalidates

BUILD FILES (.litematic, .schem, .schematic, .bp):
  of-cli inspect <file>              Describe a build file
  of-cli convert <in> <out>          Re-save in the format of the output name
  of-cli roundtrip <file|folder>...  Import, export and re-import; report any difference
  of-cli sample <folder>             Write the sample build in every format and selection
  of-cli nbt <file> [depth]          Show a file's raw tag tree

OCEAN (development version):
  of-cli ocean <folder> <name> [build=FILE] [waterline=SHARE] [offset=EAST,SOUTH]
               [tuning=FILE] [stream=...] [streams=none] [id=value ...]
                                     Bake an ocean; write pictures and build files.
                                     tuning=FILE reads the method's internal numbers
                                     from a tuning file.
                                     stream=x1,z1,x2,z2[,width[,winding[,delta]]] places a river
                                     (positions as shares of the plot); streams=none drops the
                                     land's own. With land, <name>-shore.png shows how the shore
                                     was read. Settings are ocean.* ids (see params).
                                     project=FILE starts from a saved .ofproject (later
                                     settings override it); exclude=BLOCK,... leaves blocks out;
                                     area=W,N,E,S saves only that part of the plot;
                                     pattern_image=FILE gives the ripples' image;
                                     memory=report adds the memory each step and
                                     structure held
  of-cli project <file.ofproject> <folder> [id=value ...]
                                     Bake a saved project exactly as the app would
  of-cli testruns <folder>           Write the ocean test runs: open sea in a calm, a
                                     moderate and a stormy sea

DEVELOPMENT:
  of-cli stress [size]               Time every export on a large wavy surface (default 512)
  of-cli estimate <build.litematic> [ocean.* settings]
                                     Say how much memory loading the build and baking an
                                     ocean around it would take, without loading it
  of-cli testbuild <file> [height]   Write a hollow test figure (16 to 1536 tall, default 96)
                                     to try a build as the land
  of-cli catalogue <client.jar> <out.json> [min-y max-y]
                                     Build a block catalogue from a Minecraft install
";

fn main() -> ExitCode {
    let args = match of_core::memory::take_flag(std::env::args().skip(1).collect()) {
        Ok(args) => args,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::from(2);
        }
    };
    let result = match args.first().map(String::as_str) {
        Some("version") => {
            println!("Ocean Foundry {} ({})", of_core::VERSION, of_core::MILESTONE);
            Ok(())
        }
        Some("devices") => devices(args.iter().any(|a| a == "--cpu")),
        Some("bench") => bench(args.iter().any(|a| a == "--cpu")),
        Some("params") => list_params(args.get(1).map(String::as_str)),
        Some("strings") => strings(),
        Some("fingerprint") => fingerprints(&args[1..]),
        Some("nbt") if args.len() >= 2 => {
            files::nbt_dump(&args[1], args.get(2).and_then(|d| d.parse().ok()).unwrap_or(3))
        }
        Some("inspect") if args.len() == 2 => files::inspect(&args[1]),
        Some("convert") if args.len() == 3 => files::convert(&args[1], &args[2]),
        Some("roundtrip") if args.len() >= 2 => files::roundtrip(&args[1..]),
        Some("sample") if args.len() == 2 => files::sample(&args[1]),
        Some("ocean") if args.len() >= 3 => ocean::run(&args[1..]),
        #[cfg(feature = "dev")]
        Some("lava") if args.len() >= 3 => ocean::run_lava(&args[1..]),
        Some("project") if args.len() >= 3 => ocean::run_project(&args[1], &args[2], &args[3..]),
        Some("testruns") if args.len() == 2 => ocean::test_runs(&args[1]),
        Some("estimate") if args.len() >= 2 => ocean::estimate(&args[1], &args[2..]),
        Some("testbuild") if args.len() >= 2 => ocean::test_build(&args[1], args.get(2).and_then(|v| v.parse().ok()).unwrap_or(96)),
        #[cfg(feature = "dev")]
        Some("pair") if args.len() >= 2 => ocean::pair_build(&args[1], args.get(2).and_then(|v| v.parse().ok()).unwrap_or(40), args.get(3).and_then(|v| v.parse().ok()).unwrap_or(120)),
        Some("stress") => files::stress(args.get(1).and_then(|v| v.parse().ok()).unwrap_or(512)),
        Some("catalogue") if args.len() >= 3 => {
            let height = |i: usize, default: i32| args.get(i).and_then(|v| v.parse().ok()).unwrap_or(default);
            catalogue_tool::build(&args[1], &args[2], height(3, -64), height(4, 319))
        }
        _ => {
            print!("{USAGE}");
            return if args.is_empty() { ExitCode::SUCCESS } else { ExitCode::from(2) };
        }
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            // A job stopped by the memory guard says so plainly.
            let e = if e.starts_with(of_core::memory::OVER_LIMIT) { of_core::memory::refusal(None) } else { e };
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

fn choice(cpu_only: bool) -> ComputeChoice {
    if cpu_only {
        ComputeChoice::Cpu
    } else {
        ComputeChoice::Auto
    }
}

fn devices(cpu_only: bool) -> Result<(), String> {
    let probe = of_gpu::probe(&JobCtx::detached()).map_err(|e| format!("{e:?}"))?;
    if probe.adapters.is_empty() {
        println!("No graphics devices found.");
    }
    for a in probe.adapters.iter().chain(&probe.superseded) {
        let check = match &a.self_test {
            of_gpu::SelfTest::Passed if a.kind == of_gpu::DeviceKind::Software => "not used for calculation".to_string(),
            of_gpu::SelfTest::Passed => "passed check".to_string(),
            of_gpu::SelfTest::Failed(e) => format!("FAILED check: {e}"),
        };
        println!("{:<44} {:<7} {:<10} {:<9} {}", a.name, a.backend, format!("{:?}", a.kind), a.vendor_name(), check);
    }
    println!("CPU threads: {}", probe.cpu_threads);
    let resolution = of_gpu::resolve(&choice(cpu_only), &probe);
    match resolution.device {
        Resolved::Gpu(i) => println!("Would use: {} [{}]", probe.adapters[i].name, probe.adapters[i].backend),
        Resolved::Cpu(CpuReason::Chosen) => println!("Would use: CPU (as requested)"),
        Resolved::Cpu(CpuReason::NoUsableGpu) => println!("Would use: CPU (no graphics device passed the check)"),
    }
    Ok(())
}

fn bench(cpu_only: bool) -> Result<(), String> {
    let probe = of_gpu::probe(&JobCtx::detached()).map_err(|e| format!("{e:?}"))?;
    let gpu = match of_gpu::resolve(&choice(cpu_only), &probe).device {
        Resolved::Gpu(i) => Some(probe.adapters[i].clone()),
        Resolved::Cpu(_) => None,
    };
    // Run through the real job system so the command line exercises the same
    // path as the app, including progress reporting.
    let handle = spawn("job.benchmark", || {}, move |ctx| of_gpu::benchmark(ctx, gpu.as_ref()));
    loop {
        if let Some(outcome) = handle.poll() {
            eprintln!();
            return match outcome {
                Outcome::Done(r) => {
                    if let Some((name, rate)) = &r.gpu {
                        println!("{name}: {rate:.0} million cells per second");
                    }
                    if let Some(e) = &r.gpu_error {
                        println!("GPU could not be measured: {e}");
                    }
                    println!("CPU ({} threads): {:.0} million cells per second", probe.cpu_threads, r.cpu);
                    Ok(())
                }
                Outcome::Cancelled => Err("cancelled".into()),
                Outcome::Failed(e) => Err(e),
            };
        }
        let s = handle.snapshot();
        let memory = s.est_bytes.map(coarse_bytes).unwrap_or_default();
        eprint!("\r{:>3.0}%  {:<12} {memory}   ", s.fraction * 100.0, s.step);
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}

fn parse_mode(s: Option<&str>) -> Result<Option<Mode>, String> {
    match s {
        None => Ok(None),
        Some("ocean") => Ok(Some(Mode::Ocean)),
        #[cfg(feature = "dev")]
        Some("lava") => Ok(Some(Mode::Lava)),
        #[cfg(feature = "dev")]
        Some(other) => Err(format!("unknown mode '{other}' (use ocean or lava)")),
        #[cfg(not(feature = "dev"))]
        Some(other) => Err(format!("unknown mode '{other}' (use ocean)")),
    }
}

fn list_params(mode: Option<&str>) -> Result<(), String> {
    let mode = parse_mode(mode)?;
    let en = i18n::I18n::new(Lang::En);
    for p in params::REGISTRY.iter().filter(|p| mode.is_none_or(|m| p.mode == m)) {
        let range = match p.kind {
            Kind::Float { min, max, default, .. } => format!("{min} to {max}, default {default}"),
            Kind::Int { min, max, default } => format!("{min} to {max}, default {default}"),
            Kind::Seed { default } => format!("seed, default {default}"),
            Kind::Choice { options, default, .. } => format!("one of {}, default {default}", options.join("/")),
            Kind::Bool { default } => format!("on/off, default {}", if default { "on" } else { "off" }),
            Kind::Colour { default } => format!("colour as #rrggbb, default {default}"),
        };
        println!(
            "{:<28} {:<26} {:<11}{} {range}",
            p.id,
            en.t(&p.label_key()),
            p.stage.key(),
            if p.advanced { " advanced" } else { "" }
        );
    }
    Ok(())
}

fn strings() -> Result<(), String> {
    let en = i18n::load_table(Lang::En);
    let mut problems = Vec::new();
    for lang in Lang::ALL {
        let table = i18n::load_table(lang);
        for k in i18n::registry_keys() {
            if !table.contains_key(&k) {
                problems.push(format!("{}: missing {k}", lang.code()));
            }
        }
        for k in en.keys().filter(|k| !table.contains_key(*k)) {
            problems.push(format!("{}: missing {k}", lang.code()));
        }
        for k in table.keys().filter(|k| !en.contains_key(*k)) {
            problems.push(format!("{}: has {k}, which English lacks", lang.code()));
        }
        println!("{}: {} strings", lang.code(), table.len());
    }
    if problems.is_empty() {
        println!("Language tables are complete.");
        Ok(())
    } else {
        Err(problems.join("\n"))
    }
}

fn parse_value(def: &params::ParamDef, text: &str) -> Result<Value, String> {
    params::parse_value(def, text)
}

fn fingerprints(args: &[String]) -> Result<(), String> {
    let mode = parse_mode(args.first().map(String::as_str))?.ok_or(if cfg!(feature = "dev") { "give a mode: ocean or lava" } else { "give a mode: ocean" })?;
    let defaults = ParamValues::default();
    let baked = BakeStamp::capture(&defaults, mode);
    let mut values = defaults.clone();
    for assignment in &args[1..] {
        let (id, text) = assignment.split_once('=').ok_or_else(|| format!("expected id=value, got '{assignment}'"))?;
        let def = params::find(id).ok_or_else(|| format!("unknown parameter '{id}'"))?;
        values.set(def, parse_value(def, text)?);
    }
    for stage in Stage::ALL {
        let now = fingerprint(&values, mode, stage);
        let changed = now != baked.stages[stage as usize];
        println!("{:<11} {now}{}", stage.key(), if changed { "  (differs from defaults)" } else { "" });
    }
    println!("Against a bake made with default settings: {:?}", freshness(&values, mode, Some(&baked)));
    Ok(())
}
