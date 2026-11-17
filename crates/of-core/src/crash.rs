//! Crash log. If the app hits an internal error it cannot recover from, the
//! details are appended to `crash.log` in the data folder so the problem can
//! be reported and diagnosed without a debugger.

use std::io::Write;
use std::path::{Path, PathBuf};

pub const CRASH_FILE: &str = "crash.log";

pub fn crash_log_path(data_dir: &Path) -> PathBuf {
    data_dir.join(CRASH_FILE)
}

/// Install a panic hook that records the error, then runs the previous hook.
/// `context` is free text such as the selected calculation device.
pub fn install(data_dir: &Path, context: impl Fn() -> String + Send + Sync + 'static) {
    let path = crash_log_path(data_dir);
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let thread = std::thread::current();
        let entry = format!(
            "---- Ocean Foundry {} ({}) ----\nunix time: {}\nthread: {}\ncontext: {}\n{}\nbacktrace:\n{}\n\n",
            crate::VERSION,
            crate::MILESTONE,
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0),
            thread.name().unwrap_or("unnamed"),
            context(),
            info,
            std::backtrace::Backtrace::force_capture(),
        );
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
            let _ = f.write_all(entry.as_bytes());
        }
        previous(info);
    }));
}
