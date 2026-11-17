//! Background jobs: progress, cancellation, time estimates, and the rule that
//! a cancelled or failed job never replaces the last good result.

use std::any::Any;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver, TryRecvError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Why a job stopped without a result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobError {
    Cancelled,
    Failed(String),
}

impl From<String> for JobError {
    fn from(s: String) -> Self {
        JobError::Failed(s)
    }
}
impl From<&str> for JobError {
    fn from(s: &str) -> Self {
        JobError::Failed(s.to_string())
    }
}

#[derive(Debug)]
pub enum Outcome<T> {
    Done(T),
    Cancelled,
    Failed(String),
}

#[derive(Debug)]
struct Shared {
    /// Progress in 1/1_000_000ths, so it fits an atomic.
    progress_ppm: AtomicU32,
    cancel: AtomicBool,
    /// Language key describing the current step.
    step: Mutex<String>,
    /// Expected peak memory in bytes, 0 if the job has not said.
    est_bytes: AtomicU64,
    started: Instant,
}

/// Handed to the running job so it can report progress and notice cancellation.
#[derive(Debug, Clone)]
pub struct JobCtx {
    shared: Arc<Shared>,
}

impl JobCtx {
    /// Report progress (0..=1) and stop promptly if the user cancelled.
    /// Jobs call this often and propagate the error with `?`.
    pub fn progress(&self, fraction: f64) -> Result<(), JobError> {
        let ppm = (fraction.clamp(0.0, 1.0) * 1_000_000.0) as u32;
        self.shared.progress_ppm.store(ppm, Ordering::Relaxed);
        self.check()
    }

    pub fn check(&self) -> Result<(), JobError> {
        if self.shared.cancel.load(Ordering::Relaxed) {
            Err(JobError::Cancelled)
        } else {
            Ok(())
        }
    }

    pub fn is_cancelled(&self) -> bool {
        self.shared.cancel.load(Ordering::Relaxed)
    }

    pub fn step(&self, key: &str) {
        *self.shared.step.lock().unwrap() = key.to_string();
    }

    pub fn expect_memory(&self, bytes: u64) {
        self.shared.est_bytes.store(bytes, Ordering::Relaxed);
    }

    /// A context that is never cancelled and discards progress, for running
    /// job bodies directly from the command line or tests.
    pub fn detached() -> Self {
        Self { shared: Arc::new(Shared::new()) }
    }
}

impl Shared {
    fn new() -> Self {
        Self {
            progress_ppm: AtomicU32::new(0),
            cancel: AtomicBool::new(false),
            step: Mutex::new(String::new()),
            est_bytes: AtomicU64::new(0),
            started: Instant::now(),
        }
    }
}

/// A point-in-time view of a running job for the UI.
#[derive(Debug, Clone, PartialEq)]
pub struct Snapshot {
    pub fraction: f64,
    pub step: String,
    pub elapsed: Duration,
    /// Remaining time, once there is enough progress to estimate it.
    pub remaining: Option<Duration>,
    pub est_bytes: Option<u64>,
    pub cancelling: bool,
}

/// Estimate remaining time from progress so far. Returns `None` until the job
/// has run long enough for the estimate to mean something.
pub fn estimate_remaining(fraction: f64, elapsed: Duration) -> Option<Duration> {
    if fraction < 0.03 || elapsed < Duration::from_millis(400) || fraction >= 1.0 {
        return None;
    }
    Some(elapsed.mul_f64((1.0 - fraction) / fraction))
}

pub struct JobHandle<T> {
    name: String,
    shared: Arc<Shared>,
    rx: Receiver<Outcome<T>>,
}

impl<T> JobHandle<T> {
    /// Language key naming the job.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Ask the job to stop. It ends at its next progress report.
    pub fn cancel(&self) {
        self.shared.cancel.store(true, Ordering::Relaxed);
    }

    pub fn snapshot(&self) -> Snapshot {
        let fraction = self.shared.progress_ppm.load(Ordering::Relaxed) as f64 / 1_000_000.0;
        let elapsed = self.shared.started.elapsed();
        let bytes = self.shared.est_bytes.load(Ordering::Relaxed);
        Snapshot {
            fraction,
            step: self.shared.step.lock().unwrap().clone(),
            elapsed,
            remaining: estimate_remaining(fraction, elapsed),
            est_bytes: (bytes > 0).then_some(bytes),
            cancelling: self.shared.cancel.load(Ordering::Relaxed),
        }
    }

    /// Non-blocking: the outcome if the job has finished.
    pub fn poll(&self) -> Option<Outcome<T>> {
        match self.rx.try_recv() {
            Ok(o) => Some(o),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => Some(Outcome::Failed("the job stopped without reporting a result".into())),
        }
    }

    /// Block until the job ends. Used by the command line and tests.
    pub fn wait(self) -> Outcome<T> {
        self.rx.recv().unwrap_or_else(|_| Outcome::Failed("the job stopped without reporting a result".into()))
    }
}

fn panic_message(payload: Box<dyn Any + Send>) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "unknown internal error".to_string()
    }
}

/// Run `work` on a background thread. `on_update` is called whenever the job
/// finishes, so the UI can redraw (pass `|| {}` when nobody is watching).
/// A panic inside the job becomes `Outcome::Failed`; it never takes down the app.
pub fn spawn<T, F>(name: &str, on_finish: impl Fn() + Send + 'static, work: F) -> JobHandle<T>
where
    T: Send + 'static,
    F: FnOnce(&JobCtx) -> Result<T, JobError> + Send + 'static,
{
    let shared = Arc::new(Shared::new());
    let ctx = JobCtx { shared: shared.clone() };
    let (tx, rx) = channel();
    let thread_name = format!("job:{name}");
    let spawned = std::thread::Builder::new().name(thread_name).spawn(move || {
        let outcome = match catch_unwind(AssertUnwindSafe(|| work(&ctx))) {
            Ok(Ok(v)) => Outcome::Done(v),
            Ok(Err(JobError::Cancelled)) => Outcome::Cancelled,
            Ok(Err(JobError::Failed(e))) => Outcome::Failed(e),
            Err(payload) => Outcome::Failed(panic_message(payload)),
        };
        let _ = tx.send(outcome);
        on_finish();
    });
    if let Err(e) = spawned {
        // The sender was moved into the closure and dropped with it, so the
        // handle reports failure through the disconnected channel.
        log::error!("could not start job {name}: {e}");
    }
    JobHandle { name: name.to_string(), shared, rx }
}

/// Holds the most recent successful result of some stage. Only a finished job
/// may replace it; cancelling or failing leaves the previous value in place.
#[derive(Debug, Clone, PartialEq)]
pub struct LastGood<T> {
    value: Option<T>,
    /// Message from the most recent failure, cleared by the next success.
    pub last_failure: Option<String>,
}

impl<T> Default for LastGood<T> {
    fn default() -> Self {
        Self { value: None, last_failure: None }
    }
}

impl<T> LastGood<T> {
    pub fn get(&self) -> Option<&T> {
        self.value.as_ref()
    }

    /// Apply a job outcome. Returns true if the stored value was replaced.
    pub fn apply(&mut self, outcome: Outcome<T>) -> bool {
        match outcome {
            Outcome::Done(v) => {
                self.value = Some(v);
                self.last_failure = None;
                true
            }
            Outcome::Cancelled => false,
            Outcome::Failed(e) => {
                self.last_failure = Some(e);
                false
            }
        }
    }
}

/// Human-readable duration for estimates: "8 s", "3 min", "1 h 20 min".
/// The words are language keys resolved by the caller.
pub fn coarse_duration(d: Duration) -> (u64, &'static str, Option<u64>) {
    let s = d.as_secs_f64().round() as u64;
    if s < 90 {
        (s.max(1), "unit.second", None)
    } else if s < 90 * 60 {
        ((s + 30) / 60, "unit.minute", None)
    } else {
        (s / 3600, "unit.hour", Some((s % 3600) / 60))
    }
}

/// Human-readable byte count: "640 MB", "2.4 GB".
pub fn coarse_bytes(bytes: u64) -> String {
    const MB: f64 = 1024.0 * 1024.0;
    let mb = bytes as f64 / MB;
    if mb < 1.0 {
        "<1 MB".to_string()
    } else if mb < 1000.0 {
        format!("{mb:.0} MB")
    } else {
        format!("{:.1} GB", mb / 1024.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finished_job_returns_its_value_and_reports_progress() {
        let h = spawn("test", || {}, |ctx| {
            ctx.step("job.benchmark");
            ctx.expect_memory(64 << 20);
            ctx.progress(0.5)?;
            Ok(41 + 1)
        });
        match h.wait() {
            Outcome::Done(v) => assert_eq!(v, 42),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn cancel_stops_the_job_promptly() {
        let h = spawn("test", || {}, |ctx| -> Result<(), JobError> {
            loop {
                ctx.progress(0.1)?;
                std::thread::sleep(Duration::from_millis(2));
            }
        });
        std::thread::sleep(Duration::from_millis(20));
        h.cancel();
        assert!(h.snapshot().cancelling);
        let t = Instant::now();
        assert!(matches!(h.wait(), Outcome::Cancelled));
        assert!(t.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn panic_becomes_a_failure_not_a_crash() {
        let h = spawn::<(), _>("test", || {}, |_| panic!("boom"));
        match h.wait() {
            Outcome::Failed(e) => assert!(e.contains("boom")),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn cancelled_or_failed_work_keeps_the_last_good_result() {
        let mut slot = LastGood::default();
        assert!(slot.apply(Outcome::Done("first")));
        assert!(!slot.apply(Outcome::Cancelled));
        assert_eq!(slot.get(), Some(&"first"));
        assert!(!slot.apply(Outcome::Failed("disk full".into())));
        assert_eq!(slot.get(), Some(&"first"));
        assert_eq!(slot.last_failure.as_deref(), Some("disk full"));
        assert!(slot.apply(Outcome::Done("second")));
        assert_eq!(slot.get(), Some(&"second"));
        assert_eq!(slot.last_failure, None);
    }

    #[test]
    fn estimates_wait_for_enough_information() {
        assert_eq!(estimate_remaining(0.01, Duration::from_secs(5)), None);
        assert_eq!(estimate_remaining(0.5, Duration::from_millis(100)), None);
        assert_eq!(estimate_remaining(0.25, Duration::from_secs(10)), Some(Duration::from_secs(30)));
    }

    #[test]
    fn durations_and_sizes_read_naturally() {
        assert_eq!(coarse_duration(Duration::from_secs(8)), (8, "unit.second", None));
        assert_eq!(coarse_duration(Duration::from_secs(170)), (3, "unit.minute", None));
        assert_eq!(coarse_duration(Duration::from_secs(6000)), (1, "unit.hour", Some(40)));
        assert_eq!(coarse_bytes(640 << 20), "640 MB");
        assert_eq!(coarse_bytes(5 << 30), "5.0 GB");
    }
}
