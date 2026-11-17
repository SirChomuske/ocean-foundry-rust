//! Ocean Foundry desktop app.

// No console window in release builds; debug builds keep it for log output.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod files;
mod ocean;
mod preview;
mod theme;
mod widgets;

use eframe::egui;
use of_core::i18n::Lang;
use of_core::params::Mode;
use std::path::PathBuf;

/// Set when a file the command line asked for could not be saved, so the
/// program ends with a failure code.
pub static SAVE_FAILED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Options for automated runs. Normal use passes none of these.
#[derive(Debug, Default, Clone)]
pub struct Launch {
    /// Save a picture of the window to this file, then exit.
    pub screenshot: Option<PathBuf>,
    /// Use this folder for settings instead of the usual one.
    pub data_dir: Option<PathBuf>,
    pub lang: Option<Lang>,
    pub mode: Option<Mode>,
    /// Light, dark or automatic appearance.
    pub appearance: Option<of_core::settings::Appearance>,
    /// Show hover help for this parameter id as if the pointer were on it.
    pub hover: Option<String>,
    /// Open the calculation-device window.
    pub show_devices: bool,
    /// Switch the calculation device to "CPU only" (and save that choice).
    pub cpu: bool,
    pub size: Option<(f32, f32)>,
    /// Open the build-files window on this file.
    pub files: Option<PathBuf>,
    /// Format chosen in the build-files window (litematic, sponge or axiom).
    pub files_format: Option<of_formats::model::Format>,
    pub files_omit_air: bool,
    /// Save the baked ocean to this file with the export window's options.
    pub export_to: Option<PathBuf>,
    /// Scroll the control panel down by this many points.
    pub scroll: Option<f32>,
    /// Bake the ocean as soon as the window opens.
    pub bake: bool,
    /// Open the export window (it uses --files-format and --files-omit-air).
    pub export: bool,
    /// Bake the ocean around this land (open, coast or cliffs).
    pub land: Option<of_ocean::Land>,
    /// Bake the ocean around this build file, with the water resting this
    /// share of the way up it.
    pub build: Option<String>,
    pub waterline: Option<f64>,
    /// Open the viewport on this view (build, wanted, blocks, difference
    /// or height).
    pub show: Option<String>,
    /// Show only these layers of the build.
    pub layers: Option<preview::Layers>,
    /// Cut the build open this share of the way across.
    pub cut_open: Option<f64>,
    /// Settings to change before anything else, as parameter and value.
    pub set: Vec<(&'static of_core::params::ParamDef, of_core::params::Value)>,
    /// Open this project file, as the Project menu's Open does.
    pub open_project: Option<PathBuf>,
    /// Open the saved profile of this name.
    pub load_profile: Option<String>,
    /// Go back to this past bake (0 is the newest).
    pub restore_history: Option<usize>,
    /// Once there is a build, save the project to this file.
    pub save_project: Option<PathBuf>,
    /// Once there is a build, keep it as a profile of this name.
    pub save_profile: Option<String>,
    /// Leave these blocks out of the bake.
    pub exclude: Vec<String>,
    /// Bring every left-out block back.
    pub restore_blocks: bool,
    /// Take the ripples from this image.
    pub pattern_image: Option<String>,
    /// Ponds added to the land, each as `x,z[,depth]`.
    pub ponds: Vec<of_ocean::Pond>,
    /// Calm-water and keep-dry areas, each as `x,z[,radius[,strength]]`.
    pub zones: Vec<of_ocean::Zone>,
    /// What the export window saves.
    pub export_selection: Option<of_formats::export::Selection>,
    /// Save only this part of the plot: west, north, east and south edges.
    pub export_area: Option<[i32; 4]>,
}

fn parse_args() -> Result<Launch, String> {
    let mut launch = Launch::default();
    // --max-memory MB sets the limit every job keeps under (of_core::memory).
    let mut args = of_core::memory::take_flag(std::env::args().skip(1).collect())?.into_iter();
    while let Some(arg) = args.next() {
        let mut value = |name: &str| args.next().ok_or_else(|| format!("{name} needs a value"));
        match arg.as_str() {
            "--screenshot" => launch.screenshot = Some(PathBuf::from(value("--screenshot")?)),
            "--data-dir" => launch.data_dir = Some(PathBuf::from(value("--data-dir")?)),
            "--lang" => {
                let v = value("--lang")?;
                launch.lang = Some(Lang::from_code(&v).ok_or_else(|| format!("unknown language '{v}'"))?);
            }
            "--mode" => {
                launch.mode = Some(match value("--mode")?.as_str() {
                    "ocean" => Mode::Ocean,
                    #[cfg(feature = "dev")]
                    "lava" => Mode::Lava,
                    other => return Err(format!("unknown mode '{other}'")),
                });
            }
            "--appearance" => {
                let v = value("--appearance")?;
                launch.appearance = Some(of_core::settings::Appearance::from_key(&v).ok_or_else(|| format!("unknown appearance '{v}'"))?);
            }
            "--hover" => launch.hover = Some(value("--hover")?),
            "--devices" => launch.show_devices = true,
            "--files" => launch.files = Some(PathBuf::from(value("--files")?)),
            "--files-format" => {
                let v = value("--files-format")?;
                launch.files_format = Some(of_formats::model::Format::from_key(&v).ok_or_else(|| format!("unknown format '{v}'"))?);
            }
            "--files-omit-air" => launch.files_omit_air = true,
            "--scroll" => launch.scroll = Some(value("--scroll")?.parse().map_err(|_| "bad scroll distance")?),
            "--bake" => launch.bake = true,
            "--land" => {
                let v = value("--land")?;
                launch.land = Some(of_ocean::Land::ALL.into_iter().find(|l| l.key() == v).ok_or_else(|| format!("unknown land '{v}'"))?);
            }
            "--build" => launch.build = Some(value("--build")?),
            "--waterline" => launch.waterline = Some(value("--waterline")?.parse().map_err(|_| "bad waterline share")?),
            "--show" => launch.show = Some(value("--show")?),
            "--layers" => {
                let v = value("--layers")?;
                launch.layers = Some(
                    preview::Layers::ALL.into_iter().find(|l| l.key().rsplit('.').next() == Some(v.as_str())).ok_or_else(|| format!("unknown layers '{v}'"))?,
                );
            }
            "--cut-open" => launch.cut_open = Some(value("--cut-open")?.parse().map_err(|_| "--cut-open expects a share from 0 to 1")?),
            "--set" => {
                let v = value("--set")?;
                let (id, text) = v.split_once('=').ok_or_else(|| format!("--set expects id=value, got '{v}'"))?;
                let def = of_core::params::find(id).ok_or_else(|| format!("unknown setting '{id}'"))?;
                launch.set.push((def, of_core::params::parse_value(def, text)?));
            }
            "--open-project" => launch.open_project = Some(PathBuf::from(value("--open-project")?)),
            "--load-profile" => launch.load_profile = Some(value("--load-profile")?),
            "--restore-history" => launch.restore_history = Some(value("--restore-history")?.parse().map_err(|_| "bad history number")?),
            "--save-project" => launch.save_project = Some(PathBuf::from(value("--save-project")?)),
            "--save-profile" => launch.save_profile = Some(value("--save-profile")?),
            "--exclude" => {
                let v = value("--exclude")?;
                launch.exclude.push(if v.contains(':') { v } else { format!("minecraft:{v}") });
            }
            "--restore-blocks" => launch.restore_blocks = true,
            "--pattern-image" => launch.pattern_image = Some(value("--pattern-image")?),
            "--pond" => launch.ponds.push(of_ocean::Pond::parse(&value("--pond")?)?),
            "--calm" => launch.zones.push(of_ocean::Zone::parse(of_ocean::ZoneKind::Calm, &value("--calm")?)?),
            "--dry" => launch.zones.push(of_ocean::Zone::parse(of_ocean::ZoneKind::Dry, &value("--dry")?)?),
            "--export-selection" => {
                let v = value("--export-selection")?;
                launch.export_selection = Some(of_formats::export::Selection::from_key(&v).ok_or_else(|| format!("unknown selection '{v}'"))?);
            }
            "--export-area" => {
                let v = value("--export-area")?;
                let n: Vec<i32> = v.split(',').map(|x| x.trim().parse::<i32>()).collect::<Result<_, _>>().map_err(|_| "--export-area expects W,N,E,S")?;
                let &[w, north, e, s] = n.as_slice() else { return Err("--export-area expects W,N,E,S".into()) };
                launch.export_area = Some([w.min(e), north.min(s), w.max(e), north.max(s)]);
            }
            "--export" => launch.export = true,
            "--export-to" => {
                launch.export = true;
                launch.export_to = Some(PathBuf::from(value("--export-to")?));
            }
            "--cpu" => launch.cpu = true,
            "--size" => {
                let v = value("--size")?;
                let (w, h) = v.split_once('x').ok_or("--size expects WIDTHxHEIGHT")?;
                launch.size = Some((w.parse().map_err(|_| "bad width")?, h.parse().map_err(|_| "bad height")?));
            }
            other => return Err(format!("unknown option '{other}'")),
        }
    }
    Ok(launch)
}

fn main() -> std::process::ExitCode {
    let launch = match parse_args() {
        Ok(l) => l,
        Err(e) => {
            eprintln!("error: {e}");
            return std::process::ExitCode::from(2);
        }
    };
    let data = match of_core::paths::data_dir(launch.data_dir.as_deref()) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("error: no writable folder for settings: {e}");
            return std::process::ExitCode::FAILURE;
        }
    };
    of_core::crash::install(&data.path, || format!("app; args {:?}", std::env::args().skip(1).collect::<Vec<_>>()));

    let (w, h) = launch.size.unwrap_or((1280.0, 800.0));
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(format!("Ocean Foundry {} ({})", of_core::VERSION, of_core::MILESTONE))
            .with_inner_size([w, h])
            .with_min_inner_size([960.0, 600.0]),
        renderer: eframe::Renderer::Wgpu,
        ..Default::default()
    };
    let result = eframe::run_native(
        "Ocean Foundry",
        options,
        Box::new(move |cc| Ok(Box::new(app::OceanFoundryApp::new(cc, data, launch)))),
    );
    match result {
        Ok(()) if SAVE_FAILED.load(std::sync::atomic::Ordering::Relaxed) => std::process::ExitCode::FAILURE,
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            std::process::ExitCode::FAILURE
        }
    }
}
