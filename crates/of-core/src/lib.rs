//! Shared foundations for Ocean Foundry: parameters, localisation, settings,
//! stage fingerprints, background jobs, data paths and crash logging.
//!
//! Nothing in this crate depends on the UI or on a GPU, so all of it can be
//! driven headless from `of-cli` and from tests.

pub mod crash;
pub mod golden;
pub mod i18n;
pub mod jobs;
pub mod memory;
pub mod params;
pub mod paths;
pub mod project;
pub mod settings;
pub mod stage;

/// App version, stamped into settings, crash logs and the window title.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
/// Milestone label shown next to the version until the first full release.
pub const MILESTONE: &str = "M2 development";
