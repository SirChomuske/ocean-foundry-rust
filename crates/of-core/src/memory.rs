//! How much memory the program may use, and what it uses now.
//!
//! Why: an unbounded run once filled the PC's memory (the column bake plan,
//! stage 6). Every long job checks the process's committed memory against
//! one limit as it reports progress, and is refused up front when its
//! estimate is over the limit. The limit comes from `--max-memory`, or is
//! 40% of the memory free when the program starts.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub const MB: u64 = 1024 * 1024;

/// Problem key of a job stopped or refused for memory; the message is made
/// with `refusal` so it can carry the sizes.
pub const OVER_LIMIT: &str = "err.memory";

#[cfg(windows)]
mod sys {
    #[repr(C)]
    #[derive(Default)]
    struct Counters {
        cb: u32,
        page_fault_count: u32,
        peak_working_set: usize,
        working_set: usize,
        quota_peak_paged_pool: usize,
        quota_paged_pool: usize,
        quota_peak_non_paged_pool: usize,
        quota_non_paged_pool: usize,
        pagefile: usize,
        peak_pagefile: usize,
        private: usize,
    }
    #[repr(C)]
    #[derive(Default)]
    struct Status {
        length: u32,
        load: u32,
        total_phys: u64,
        avail_phys: u64,
        total_page_file: u64,
        avail_page_file: u64,
        total_virtual: u64,
        avail_virtual: u64,
        avail_extended_virtual: u64,
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetCurrentProcess() -> isize;
        fn K32GetProcessMemoryInfo(process: isize, counters: *mut Counters, size: u32) -> i32;
        fn GlobalMemoryStatusEx(status: *mut Status) -> i32;
    }
    /// Committed (private) bytes of this process, the figure the capped runner caps.
    pub fn committed() -> u64 {
        let mut c = Counters { cb: std::mem::size_of::<Counters>() as u32, ..Default::default() };
        // SAFETY: the structure is the documented PROCESS_MEMORY_COUNTERS_EX layout and its size is passed.
        let ok = unsafe { K32GetProcessMemoryInfo(GetCurrentProcess(), &mut c, c.cb) };
        if ok != 0 { c.private as u64 } else { 0 }
    }
    /// Physical memory free now.
    pub fn free() -> Option<u64> {
        let mut s = Status { length: std::mem::size_of::<Status>() as u32, ..Default::default() };
        // SAFETY: the structure is the documented MEMORYSTATUSEX layout with its length set.
        let ok = unsafe { GlobalMemoryStatusEx(&mut s) };
        (ok != 0).then_some(s.avail_phys)
    }
}

#[cfg(not(windows))]
mod sys {
    pub fn committed() -> u64 {
        0
    }
    pub fn free() -> Option<u64> {
        None
    }
}

pub use sys::{committed, free};

/// The limit in bytes; 0 until it is first asked for or set.
static LIMIT: AtomicU64 = AtomicU64::new(0);
/// When the committed memory was last read by `check`.
static LAST: Mutex<Option<Instant>> = Mutex::new(None);

/// 40% of the memory free now, and never under 256 MB; 4 GB where the free
/// memory cannot be read.
pub fn default_limit() -> u64 {
    free().map_or(4096 * MB, |f| (f / 5 * 2).max(256 * MB))
}

/// The limit every job keeps under.
pub fn limit() -> u64 {
    match LIMIT.load(Ordering::Relaxed) {
        0 => {
            let l = default_limit();
            let _ = LIMIT.compare_exchange(0, l, Ordering::Relaxed, Ordering::Relaxed);
            LIMIT.load(Ordering::Relaxed)
        }
        l => l,
    }
}

/// Set the limit (from `--max-memory`, in MB).
pub fn set_limit_mb(mb: u64) {
    LIMIT.store(mb.max(1) * MB, Ordering::Relaxed);
}

/// Read `--max-memory <MB>` or `--max-memory=<MB>` from a command line,
/// set the limit from it and return the arguments without it.
pub fn take_flag(args: Vec<String>) -> Result<Vec<String>, String> {
    let mut rest = Vec::with_capacity(args.len());
    let mut it = args.into_iter();
    while let Some(a) = it.next() {
        let value = if a == "--max-memory" {
            Some(it.next().ok_or("--max-memory expects a size in MB")?)
        } else {
            a.strip_prefix("--max-memory=").map(str::to_string)
        };
        match value {
            Some(v) => set_limit_mb(v.trim().parse::<u64>().map_err(|_| format!("--max-memory expects a size in MB, got '{v}'"))?),
            None => rest.push(a),
        }
    }
    Ok(rest)
}

/// Whether the process is still under the limit. Reads the memory at most
/// every 5 ms, so a job may call it as often as it reports progress.
pub fn within_limit() -> bool {
    {
        let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
        let now = Instant::now();
        if last.is_some_and(|t| now.duration_since(t) < Duration::from_millis(5)) {
            return true;
        }
        *last = Some(now);
    }
    committed() <= limit()
}

/// Whether a step that adds about `more` bytes to what is held now fits.
/// Err gives the whole estimate.
pub fn fits(more: u64) -> Result<u64, u64> {
    let whole = committed().saturating_add(more);
    if whole <= limit() { Ok(whole) } else { Err(whole) }
}

/// `vec![value; n]` that is refused with `OVER_LIMIT`, not aborted, when
/// the memory cannot be had or it would take the process over the limit.
pub fn filled<T: Clone>(n: usize, value: T) -> Result<Vec<T>, String> {
    let mut v = Vec::new();
    if fits((n as u64).saturating_mul(std::mem::size_of::<T>() as u64)).is_err() || v.try_reserve_exact(n).is_err() {
        return Err(OVER_LIMIT.to_string());
    }
    v.resize(n, value);
    Ok(v)
}

/// Whole MB, for messages.
pub fn mb(bytes: u64) -> String {
    format!("{} MB", bytes.div_ceil(MB))
}

/// The plain message for a job refused or stopped for memory: what it
/// needs (when known) and the limit.
pub fn refusal(needs: Option<u64>) -> String {
    match needs {
        Some(n) => format!("this needs about {}, more than the memory limit of {} (--max-memory)", mb(n), mb(limit())),
        None => format!("it went over the memory limit of {} (--max-memory) and was stopped", mb(limit())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_flag_sets_the_limit_and_is_taken_out() {
        let args = ["ocean", "--max-memory", "300", "a", "--max-memory=200", "b"].map(String::from).to_vec();
        assert_eq!(take_flag(args).unwrap(), ["ocean", "a", "b"]);
        assert_eq!(limit(), 200 * MB);
        assert!(take_flag(vec!["--max-memory".into()]).is_err());
        assert!(take_flag(vec!["--max-memory=lots".into()]).is_err());
    }
}
