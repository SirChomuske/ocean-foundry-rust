//! The program's own memory use, for `memory=report`: what it holds now,
//! and the highest it reached during each step of a bake.
//!
//! Why: the column bake plan (stage 1) needs real peaks per step and per
//! structure to compare every later stage against. A watcher thread reads
//! the committed memory every few milliseconds, because a step's peak can
//! come and go between two of the steps' own reports.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

pub use of_core::memory::committed;

/// Watches the committed memory and keeps the highest value since the last `step`.
pub struct Watch {
    high: Arc<AtomicU64>,
    stop: Arc<AtomicBool>,
    lines: String,
    started: std::time::Instant,
}

const MB: f64 = 1024.0 * 1024.0;

pub fn mb(bytes: u64) -> String {
    format!("{:.1} MB", bytes as f64 / MB)
}

impl Watch {
    pub fn start() -> Self {
        let high = Arc::new(AtomicU64::new(committed()));
        let stop = Arc::new(AtomicBool::new(false));
        let (h, s) = (high.clone(), stop.clone());
        std::thread::spawn(move || {
            while !s.load(Ordering::Relaxed) {
                h.fetch_max(committed(), Ordering::Relaxed);
                std::thread::sleep(Duration::from_millis(5));
            }
        });
        Self { high, stop, lines: String::new(), started: std::time::Instant::now() }
    }

    /// Close the step just done: note what is held now and its highest point,
    /// then start watching the next step from the present value.
    pub fn step(&mut self, name: &str) {
        let now = committed();
        let high = self.high.fetch_max(now, Ordering::Relaxed).max(now);
        let line = format!(
            "  memory: {name:<34} peak {:>10}  after {:>10}  at {:.1} s\n",
            mb(high),
            mb(now),
            self.started.elapsed().as_secs_f32()
        );
        // Also live on stderr, so a long run shows where it is.
        eprint!("{line}");
        self.lines.push_str(&line);
        self.high.store(committed(), Ordering::Relaxed);
    }

    /// A line about the size of one structure.
    pub fn note(&mut self, text: &str) {
        self.lines.push_str("  memory size: ");
        self.lines.push_str(text);
        self.lines.push('\n');
    }

    pub fn finish(self) -> String {
        self.stop.store(true, Ordering::Relaxed);
        self.lines.clone()
    }
}
