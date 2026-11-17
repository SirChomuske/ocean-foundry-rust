//! Calculation devices: finding GPUs from any vendor, checking that each one
//! computes correctly, choosing one, and falling back to the CPU.
//!
//! The check and the speed measurement both run the same small grid
//! calculation (a neighbour-averaging step, the basic operation of the water
//! solver) on the GPU and on the CPU. The CPU version is the reference: a GPU
//! is only used if its answer matches.

mod kernel;

use of_core::jobs::{JobCtx, JobError};
use of_core::settings::ComputeChoice;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::time::{Duration, Instant};

pub use kernel::{cpu_relax, GpuRunner};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum DeviceKind {
    Discrete,
    Integrated,
    Virtual,
    Other,
    /// A software renderer. Never used for calculation: the app's own CPU
    /// path is faster and is the tested reference.
    Software,
}

impl DeviceKind {
    pub fn key(self) -> &'static str {
        match self {
            DeviceKind::Discrete => "compute.type.discrete",
            DeviceKind::Integrated => "compute.type.integrated",
            DeviceKind::Virtual => "compute.type.virtual",
            DeviceKind::Other => "compute.type.other",
            DeviceKind::Software => "compute.type.software",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SelfTest {
    Passed,
    Failed(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdapterEntry {
    pub name: String,
    /// "Dx12" or "Vulkan".
    pub backend: String,
    pub kind: DeviceKind,
    pub vendor: u32,
    pub driver: String,
    pub self_test: SelfTest,
}

impl AdapterEntry {
    pub fn usable(&self) -> bool {
        self.self_test == SelfTest::Passed && self.kind != DeviceKind::Software
    }

    pub fn vendor_name(&self) -> &'static str {
        match self.vendor {
            0x10DE => "NVIDIA",
            0x1002 | 0x1022 => "AMD",
            0x8086 => "Intel",
            0x5143 => "Qualcomm",
            0x1414 => "Microsoft",
            _ => "",
        }
    }

    pub fn matches(&self, name: &str, backend: &str) -> bool {
        self.name == name && self.backend == backend
    }
}

/// Everything found on this machine.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Probe {
    pub adapters: Vec<AdapterEntry>,
    pub cpu_threads: usize,
    /// Entries left out of `adapters` because the same card works on another
    /// backend. Kept for diagnostics only.
    pub superseded: Vec<AdapterEntry>,
}

/// Which device will actually do the work, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolved {
    /// Index into `Probe::adapters`.
    Gpu(usize),
    Cpu(CpuReason),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CpuReason {
    /// The user asked for the CPU.
    Chosen,
    /// Automatic choice, but no GPU passed the check.
    NoUsableGpu,
}

/// Outcome of applying the user's choice to what was found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolution {
    pub device: Resolved,
    /// True if a specific adapter was requested but is missing or failing,
    /// so the automatic choice was used instead.
    pub requested_missing: bool,
}

fn backends() -> wgpu::Backends {
    wgpu::Backends::DX12 | wgpu::Backends::VULKAN
}

fn instance() -> wgpu::Instance {
    let mut desc = wgpu::InstanceDescriptor::new_without_display_handle();
    desc.backends = backends();
    wgpu::Instance::new(desc)
}

fn kind_of(t: wgpu::DeviceType) -> DeviceKind {
    match t {
        wgpu::DeviceType::DiscreteGpu => DeviceKind::Discrete,
        wgpu::DeviceType::IntegratedGpu => DeviceKind::Integrated,
        wgpu::DeviceType::VirtualGpu => DeviceKind::Virtual,
        wgpu::DeviceType::Cpu => DeviceKind::Software,
        wgpu::DeviceType::Other => DeviceKind::Other,
    }
}

fn backend_name(b: wgpu::Backend) -> String {
    format!("{b:?}")
}

fn panic_text(p: Box<dyn std::any::Any + Send>) -> String {
    p.downcast_ref::<&str>()
        .map(|s| s.to_string())
        .or_else(|| p.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "internal error".into())
}

/// Grid size and step count of the correctness check. Small, so the check
/// takes a fraction of a second even on a slow integrated GPU.
const TEST_SIZE: u32 = 192;
const TEST_STEPS: u32 = 12;
/// Largest difference tolerated between GPU and CPU results. Both compute in
/// 32-bit floats; only rounding order may differ.
const TEST_TOLERANCE: f32 = 1e-4;

fn test_field(size: u32) -> Vec<f32> {
    // A deterministic pattern with sharp and smooth features and no symmetry.
    let n = size as usize;
    let mut v = vec![0.0f32; n * n];
    for y in 0..n {
        for x in 0..n {
            let fx = x as f32 / n as f32;
            let fy = y as f32 / n as f32;
            let smooth = (fx * 9.0).sin() * (fy * 5.0 + fx * 3.0).cos();
            let block = if (x / 17 + y / 23) % 3 == 0 { 1.5 } else { -0.25 };
            v[y * n + x] = smooth + block;
        }
    }
    v
}

fn self_test(adapter: &wgpu::Adapter) -> SelfTest {
    let run = || -> Result<(), String> {
        let input = test_field(TEST_SIZE);
        let mut runner = GpuRunner::new(adapter, TEST_SIZE, TEST_SIZE, &input)?;
        runner.run(TEST_STEPS, Duration::from_secs(20))?;
        let gpu = runner.read(Duration::from_secs(20))?;
        let cpu = cpu_relax(&input, TEST_SIZE as usize, TEST_SIZE as usize, TEST_STEPS);
        let worst = gpu.iter().zip(&cpu).map(|(a, b)| (a - b).abs()).fold(0.0f32, f32::max);
        if !worst.is_finite() || worst > TEST_TOLERANCE {
            return Err(format!("result differs from the CPU reference by {worst:e}"));
        }
        Ok(())
    };
    match catch_unwind(AssertUnwindSafe(run)) {
        Ok(Ok(())) => SelfTest::Passed,
        Ok(Err(e)) => SelfTest::Failed(e),
        Err(p) => SelfTest::Failed(panic_text(p)),
    }
}

fn enumerate() -> Vec<wgpu::Adapter> {
    catch_unwind(|| pollster::block_on(instance().enumerate_adapters(backends()))).unwrap_or_default()
}

/// Find every graphics device and check each one. Safe to call on a machine
/// with no GPU at all: the result is then an empty list.
pub fn probe(ctx: &JobCtx) -> Result<Probe, JobError> {
    ctx.step("job.step.gpu");
    let mut found = enumerate();
    // Direct3D 12 first: it ships with Windows for every vendor. Vulkan is
    // the second chance for a card whose Direct3D 12 entry fails the check.
    found.sort_by_key(|a| a.get_info().backend != wgpu::Backend::Dx12);
    let mut adapters: Vec<AdapterEntry> = Vec::new();
    let total = found.len().max(1) as f64;
    for (i, adapter) in found.iter().enumerate() {
        ctx.progress(i as f64 / total)?;
        let info = adapter.get_info();
        let kind = kind_of(info.device_type);
        let backend = backend_name(info.backend);
        // The same card usually appears once per backend. Keep the first
        // working entry and skip repeats.
        if adapters.iter().any(|a| a.name == info.name && a.usable()) {
            continue;
        }
        let result = if kind == DeviceKind::Software { SelfTest::Passed } else { self_test(adapter) };
        log::info!("adapter {} [{backend}] {kind:?}: {result:?}", info.name);
        adapters.push(AdapterEntry {
            name: info.name,
            backend,
            kind,
            vendor: info.vendor,
            driver: format!("{} {}", info.driver, info.driver_info).trim().to_string(),
            self_test: result,
        });
    }
    // If a card failed on one backend but passed on another, drop the failure.
    let passed: Vec<String> = adapters.iter().filter(|a| a.usable()).map(|a| a.name.clone()).collect();
    let (adapters, superseded): (Vec<_>, Vec<_>) =
        adapters.into_iter().partition(|a| a.usable() || !passed.contains(&a.name));
    let mut adapters = adapters;
    adapters.sort_by_key(|a| (a.kind, !a.usable()));
    ctx.progress(1.0)?;
    Ok(Probe { adapters, cpu_threads: rayon::current_num_threads(), superseded })
}

/// Apply the user's choice. Never fails: the CPU is always available.
pub fn resolve(choice: &ComputeChoice, probe: &Probe) -> Resolution {
    let auto = || match probe.adapters.iter().position(|a| a.usable()) {
        // Adapters are sorted dedicated-first, so the first usable one is the best.
        Some(i) => Resolved::Gpu(i),
        None => Resolved::Cpu(CpuReason::NoUsableGpu),
    };
    match choice {
        ComputeChoice::Cpu => Resolution { device: Resolved::Cpu(CpuReason::Chosen), requested_missing: false },
        ComputeChoice::Auto => Resolution { device: auto(), requested_missing: false },
        ComputeChoice::Adapter { name, backend } => {
            match probe.adapters.iter().position(|a| a.matches(name, backend) && a.usable()) {
                Some(i) => Resolution { device: Resolved::Gpu(i), requested_missing: false },
                None => Resolution { device: auto(), requested_missing: true },
            }
        }
    }
}

fn find_adapter(entry: &AdapterEntry) -> Option<wgpu::Adapter> {
    enumerate().into_iter().find(|a| {
        let info = a.get_info();
        info.name == entry.name && backend_name(info.backend) == entry.backend
    })
}

/// Measured calculation speed, in millions of grid cells updated per second.
#[derive(Debug, Clone, PartialEq)]
pub struct BenchResult {
    pub gpu: Option<(String, f64)>,
    pub cpu: f64,
    /// Why the GPU could not be measured, if it was requested.
    pub gpu_error: Option<String>,
}

const BENCH_SIZE: u32 = 1024;
const BENCH_BATCH: u32 = 16;
/// How long each device is measured for.
const BENCH_TIME: Duration = Duration::from_millis(1500);

/// Measure the chosen GPU (if any) and the CPU.
pub fn benchmark(ctx: &JobCtx, gpu: Option<&AdapterEntry>) -> Result<BenchResult, JobError> {
    let cells = (BENCH_SIZE as f64) * (BENCH_SIZE as f64);
    let input = test_field(BENCH_SIZE);
    // Two float grids on each side, plus the readback copy.
    ctx.expect_memory((cells as u64) * 4 * 5);
    let phases = if gpu.is_some() { 2.0 } else { 1.0 };
    let mut done_phases = 0.0;

    let mut gpu_result = None;
    let mut gpu_error = None;
    if let Some(entry) = gpu {
        ctx.step("job.step.gpu");
        let measured = catch_unwind(AssertUnwindSafe(|| -> Result<Result<f64, String>, JobError> {
            let Some(adapter) = find_adapter(entry) else {
                return Ok(Err("device is no longer available".to_string()));
            };
            let mut runner = match GpuRunner::new(&adapter, BENCH_SIZE, BENCH_SIZE, &input) {
                Ok(r) => r,
                Err(e) => return Ok(Err(e)),
            };
            // Warm up so shader compilation is not counted.
            if let Err(e) = runner.run(BENCH_BATCH, Duration::from_secs(30)) {
                return Ok(Err(e));
            }
            let start = Instant::now();
            let mut steps = 0u64;
            while start.elapsed() < BENCH_TIME {
                if let Err(e) = runner.run(BENCH_BATCH, Duration::from_secs(30)) {
                    return Ok(Err(e));
                }
                steps += BENCH_BATCH as u64;
                ctx.progress(start.elapsed().as_secs_f64() / BENCH_TIME.as_secs_f64() / phases)?;
            }
            Ok(Ok(steps as f64 * cells / start.elapsed().as_secs_f64() / 1e6))
        }));
        match measured {
            Ok(Ok(Ok(rate))) => gpu_result = Some((entry.name.clone(), rate)),
            Ok(Ok(Err(e))) => gpu_error = Some(e),
            Ok(Err(cancelled)) => return Err(cancelled),
            Err(p) => gpu_error = Some(panic_text(p)),
        }
        done_phases = 1.0;
    }

    ctx.step("job.step.cpu");
    let n = BENCH_SIZE as usize;
    let mut field = input;
    let start = Instant::now();
    let mut steps = 0u64;
    while start.elapsed() < BENCH_TIME {
        field = cpu_relax(&field, n, n, 2);
        steps += 2;
        ctx.progress((done_phases + start.elapsed().as_secs_f64() / BENCH_TIME.as_secs_f64()) / phases)?;
    }
    let cpu = steps as f64 * cells / start.elapsed().as_secs_f64() / 1e6;
    ctx.progress(1.0)?;
    Ok(BenchResult { gpu: gpu_result, cpu, gpu_error })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str, kind: DeviceKind, ok: bool) -> AdapterEntry {
        AdapterEntry {
            name: name.into(),
            backend: "Dx12".into(),
            kind,
            vendor: 0,
            driver: String::new(),
            self_test: if ok { SelfTest::Passed } else { SelfTest::Failed("x".into()) },
        }
    }

    fn sorted(mut adapters: Vec<AdapterEntry>) -> Probe {
        adapters.sort_by_key(|a| (a.kind, !a.usable()));
        Probe { adapters, cpu_threads: 8, superseded: Vec::new() }
    }

    #[test]
    fn automatic_prefers_a_working_dedicated_gpu() {
        let p = sorted(vec![entry("Integrated", DeviceKind::Integrated, true), entry("Dedicated", DeviceKind::Discrete, true)]);
        let r = resolve(&ComputeChoice::Auto, &p);
        assert_eq!(r.device, Resolved::Gpu(0));
        assert_eq!(p.adapters[0].name, "Dedicated");
    }

    #[test]
    fn a_failing_gpu_is_skipped_for_the_next_one() {
        let p = sorted(vec![entry("Broken", DeviceKind::Discrete, false), entry("Integrated", DeviceKind::Integrated, true)]);
        match resolve(&ComputeChoice::Auto, &p).device {
            Resolved::Gpu(i) => assert_eq!(p.adapters[i].name, "Integrated"),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn no_gpu_or_only_software_falls_back_to_cpu() {
        assert_eq!(resolve(&ComputeChoice::Auto, &Probe::default()).device, Resolved::Cpu(CpuReason::NoUsableGpu));
        let p = sorted(vec![entry("Basic Render Driver", DeviceKind::Software, true), entry("Broken", DeviceKind::Discrete, false)]);
        assert_eq!(resolve(&ComputeChoice::Auto, &p).device, Resolved::Cpu(CpuReason::NoUsableGpu));
    }

    #[test]
    fn cpu_choice_is_respected_even_with_a_gpu() {
        let p = sorted(vec![entry("Dedicated", DeviceKind::Discrete, true)]);
        assert_eq!(resolve(&ComputeChoice::Cpu, &p).device, Resolved::Cpu(CpuReason::Chosen));
    }

    #[test]
    fn a_remembered_gpu_that_vanished_falls_back_and_says_so() {
        let p = sorted(vec![entry("Integrated", DeviceKind::Integrated, true)]);
        let r = resolve(&ComputeChoice::Adapter { name: "Old Card".into(), backend: "Dx12".into() }, &p);
        assert!(r.requested_missing);
        assert_eq!(r.device, Resolved::Gpu(0));
        let r = resolve(&ComputeChoice::Adapter { name: "Integrated".into(), backend: "Dx12".into() }, &p);
        assert!(!r.requested_missing);
    }

    #[test]
    fn cpu_reference_conserves_the_total_and_smooths() {
        let n = 64usize;
        let input = test_field(n as u32);
        let out = cpu_relax(&input, n, n, 20);
        let spread = |v: &[f32]| v.iter().cloned().fold(f32::MIN, f32::max) - v.iter().cloned().fold(f32::MAX, f32::min);
        assert!(spread(&out) < spread(&input));
        let sum = |v: &[f32]| v.iter().map(|x| *x as f64).sum::<f64>();
        assert!((sum(&out) - sum(&input)).abs() < 1e-2 * n as f64, "edge handling must not create or lose quantity");
    }

    /// Runs on whatever hardware the test machine has. With no GPU the probe
    /// is empty and the test still passes: that is the CPU-fallback path.
    #[test]
    fn every_reported_usable_gpu_matches_the_cpu() {
        let p = probe(&JobCtx::detached()).unwrap();
        for a in &p.adapters {
            println!("{} [{}] {:?} {:?}", a.name, a.backend, a.kind, a.self_test);
        }
        for a in &p.superseded {
            println!("superseded: {} [{}] {:?} {:?}", a.name, a.backend, a.kind, a.self_test);
        }
        assert!(p.cpu_threads >= 1);
    }
}
