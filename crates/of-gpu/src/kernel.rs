//! The reference grid calculation, on CPU and GPU.
//!
//! One step replaces every cell with a weighted average of itself and its
//! four neighbours. Cells on the edge treat the missing neighbour as a copy of
//! themselves, which keeps the grid's total unchanged. Both versions use
//! 32-bit floats and the same order of operations.

use rayon::prelude::*;
use std::borrow::Cow;
use std::time::Duration;

/// CPU reference: `steps` relaxation steps over a `w` by `h` grid.
pub fn cpu_relax(input: &[f32], w: usize, h: usize, steps: u32) -> Vec<f32> {
    assert_eq!(input.len(), w * h);
    let mut a = input.to_vec();
    let mut b = vec![0.0f32; w * h];
    for _ in 0..steps {
        let src = &a;
        b.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
            let up = if y > 0 { y - 1 } else { y };
            let down = if y + 1 < h { y + 1 } else { y };
            for x in 0..w {
                let left = if x > 0 { x - 1 } else { x };
                let right = if x + 1 < w { x + 1 } else { x };
                let c = src[y * w + x];
                let sum = src[y * w + left] + src[y * w + right] + src[up * w + x] + src[down * w + x];
                row[x] = c * 0.5 + sum * 0.125;
            }
        });
        std::mem::swap(&mut a, &mut b);
    }
    a
}

const SHADER: &str = r#"
struct Dims { w: u32, h: u32, pad0: u32, pad1: u32 }
@group(0) @binding(0) var<uniform> dims: Dims;
@group(0) @binding(1) var<storage, read> src: array<f32>;
@group(0) @binding(2) var<storage, read_write> dst: array<f32>;

@compute @workgroup_size(8, 8)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    if (id.x >= dims.w || id.y >= dims.h) { return; }
    let x = id.x;
    let y = id.y;
    let left = select(x, x - 1u, x > 0u);
    let right = select(x, x + 1u, x + 1u < dims.w);
    let up = select(y, y - 1u, y > 0u);
    let down = select(y, y + 1u, y + 1u < dims.h);
    let c = src[y * dims.w + x];
    let sum = src[y * dims.w + left] + src[y * dims.w + right] + src[up * dims.w + x] + src[down * dims.w + x];
    dst[y * dims.w + x] = c * 0.5 + sum * 0.125;
}
"#;

/// Runs the relaxation on one GPU adapter.
pub struct GpuRunner {
    device: wgpu::Device,
    queue: wgpu::Queue,
    pipeline: wgpu::ComputePipeline,
    /// `[a -> b, b -> a]`
    bind: [wgpu::BindGroup; 2],
    buffers: [wgpu::Buffer; 2],
    w: u32,
    h: u32,
    /// Index of the buffer holding the current state.
    current: usize,
}

fn wait(device: &wgpu::Device, timeout: Duration) -> Result<(), String> {
    device
        .poll(wgpu::PollType::Wait { submission_index: None, timeout: Some(timeout) })
        .map(|_| ())
        .map_err(|e| format!("the device did not finish in time ({e})"))
}

impl GpuRunner {
    pub fn new(adapter: &wgpu::Adapter, w: u32, h: u32, input: &[f32]) -> Result<Self, String> {
        assert_eq!(input.len(), (w * h) as usize);
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("ocean-foundry-compute"),
            ..Default::default()
        }))
        .map_err(|e| format!("could not open the device ({e})"))?;

        let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("relax"),
            source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(SHADER)),
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("relax"),
            layout: None,
            module: &module,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: None,
        });
        let size = (input.len() * 4) as u64;
        // A buffer over the card's limits is refused here, not left to the
        // driver (stage 6 of the column bake plan).
        let limits = device.limits();
        let most = limits.max_buffer_size.min(limits.max_storage_buffer_binding_size as u64);
        if size > most {
            return Err(format!("the grid needs a {size}-byte buffer; this device takes at most {most}"));
        }
        let make = |label| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        };
        let buffers = [make("grid-a"), make("grid-b")];
        let dims = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("dims"),
            size: 16,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(&dims, 0, bytemuck::cast_slice(&[w, h, 0u32, 0u32]));
        queue.write_buffer(&buffers[0], 0, bytemuck::cast_slice(input));
        let layout = pipeline.get_bind_group_layout(0);
        let group = |src: &wgpu::Buffer, dst: &wgpu::Buffer| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: None,
                layout: &layout,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: dims.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 1, resource: src.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 2, resource: dst.as_entire_binding() },
                ],
            })
        };
        let bind = [group(&buffers[0], &buffers[1]), group(&buffers[1], &buffers[0])];
        if let Some(e) = pollster::block_on(scope.pop()) {
            return Err(format!("the device rejected the calculation program ({e})"));
        }
        Ok(Self { device, queue, pipeline, bind, buffers, w, h, current: 0 })
    }

    /// Run `steps` steps and wait for the device to finish them.
    pub fn run(&mut self, steps: u32, timeout: Duration) -> Result<(), String> {
        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        for _ in 0..steps {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor { label: None, timestamp_writes: None });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &self.bind[self.current], &[]);
            pass.dispatch_workgroups(self.w.div_ceil(8), self.h.div_ceil(8), 1);
            drop(pass);
            self.current ^= 1;
        }
        self.queue.submit([encoder.finish()]);
        wait(&self.device, timeout)
    }

    /// Copy the current state back to main memory.
    pub fn read(&self, timeout: Duration) -> Result<Vec<f32>, String> {
        let size = (self.w as u64) * (self.h as u64) * 4;
        let staging = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("readback"),
            size,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        encoder.copy_buffer_to_buffer(&self.buffers[self.current], 0, &staging, 0, size);
        self.queue.submit([encoder.finish()]);
        let (tx, rx) = std::sync::mpsc::channel();
        staging.slice(..).map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        wait(&self.device, timeout)?;
        rx.recv()
            .map_err(|_| "the device did not return the result".to_string())?
            .map_err(|e| format!("could not read the result back ({e})"))?;
        let view = staging.slice(..).get_mapped_range().map_err(|e| format!("could not read the result back ({e})"))?;
        let out = bytemuck::cast_slice::<u8, f32>(&view).to_vec();
        drop(view);
        staging.unmap();
        Ok(out)
    }
}
