//! Pictures of a build, drawn on the processor with flat block colours and
//! the game's own face shading. Every pixel follows a straight line into
//! the blocks, so what you see is exactly what is there.

use of_formats::catalogue::Catalogue;
use of_formats::export::{BlockSource, Layer};

pub struct Picture {
    pub width: u32,
    pub height: u32,
    /// Four bytes per pixel: red, green, blue, opacity.
    pub rgba: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct Camera {
    /// Where the viewer stands relative to the build, as an (x, z) direction. (0, 1) is from the south.
    pub from: (f64, f64),
    /// Degrees the view looks down; 90 is a map seen from straight above, north up.
    pub pitch: f64,
    /// Pixels per block.
    pub scale: f64,
    /// Samples per pixel along each axis, for smooth edges.
    pub samples: u32,
    pub background: [u8; 3],
}

impl Camera {
    pub fn angled(from: (f64, f64), pitch: f64, scale: f64) -> Self {
        Self { from, pitch, scale, samples: 3, background: [244, 245, 247] }
    }
    pub fn map(scale: f64) -> Self {
        Self { from: (0.0, 1.0), pitch: 90.0, scale, samples: 1, background: [244, 245, 247] }
    }
}

#[derive(Clone, Copy)]
struct Paint {
    top: [f32; 3],
    side: [f32; 3],
    cover: f32,
}

/// A stretch of one block up a column: levels `lo..=hi` above the bottom of
/// the box, all of paint `id`.
#[derive(Clone, Copy)]
struct Run {
    lo: u16,
    hi: u16,
    id: u16,
}

/// The blocks to draw, as runs up each column of the box around them.
///
/// Why: a full grid of the box took 2 bytes for every cell, air included,
/// and refused anything above 600 million cells; a tall hull is mostly air,
/// and the sea is a few runs a column (stage 5 of the column bake plan).
struct Grid {
    min: [i32; 3],
    size: [usize; 3],
    /// Where each column's runs start in `runs`; columns go along X, then Z.
    starts: Vec<u32>,
    /// Lowest first within a column; 0 is never stored.
    runs: Vec<Run>,
    paints: Vec<Paint>,
    /// The highest block of each square patch of columns, coarsest first,
    /// so a line of sight above a patch crosses it in one jump.
    tops: Vec<Tops>,
}

/// One level of patch heights: patches of `1 << shift` columns a side.
struct Tops {
    shift: u32,
    /// Patches along X.
    across: usize,
    /// One above the highest block in each patch; 0 for an empty patch.
    top: Vec<u32>,
}

impl Tops {
    /// Patch heights of `side` columns from column heights of `size` columns.
    fn of(heights: &[u32], size: [usize; 2], shift: u32) -> Self {
        let (across, down) = (size[0].div_ceil(1 << shift), size[1].div_ceil(1 << shift));
        let mut top = vec![0u32; across * down];
        for z in 0..size[1] {
            for x in 0..size[0] {
                let t = &mut top[(z >> shift) * across + (x >> shift)];
                *t = (*t).max(heights[z * size[0] + x]);
            }
        }
        Self { shift, across, top }
    }
}

/// Most runs a scene may hold (6 bytes each), so a drawing never takes
/// more than about 1.2 GB.
const MOST_RUNS: u64 = 200_000_000;

impl Grid {
    fn new(source: &dyn BlockSource, layers: &[Layer], catalogue: &Catalogue) -> Result<Self, String> {
        let (min, max) = source.bounds(layers).ok_or("there is nothing to draw")?;
        let size: [usize; 3] = std::array::from_fn(|a| (max[a] - min[a] + 1) as usize);
        if size[1] > u16::MAX as usize {
            return Err("the build is too tall to draw".into());
        }
        let mut paints = vec![Paint { top: [0.0; 3], side: [0.0; 3], cover: 0.0 }];
        for state in source.palette().states() {
            let info = catalogue.blocks.get(&state.name);
            let top = info.and_then(|i| i.top).unwrap_or([255, 0, 255]);
            let side = info.and_then(|i| i.side).unwrap_or(top);
            paints.push(Paint {
                top: top.map(|c| c as f32),
                side: side.map(|c| c as f32),
                cover: info.and_then(|i| i.alpha).unwrap_or(255) as f32 / 255.0,
            });
        }

        // Rows of the box are read along X, a band of Z rows per thread; each
        // band gives its columns' runs in order.
        let [sx, sy, sz] = size;
        let total = std::sync::atomic::AtomicU64::new(0);
        let threads = std::thread::available_parallelism().map_or(4, |n| n.get()).min(sz).max(1);
        let band = sz.div_ceil(threads);
        let bands: Vec<Result<(Vec<u32>, Vec<Run>), String>> = std::thread::scope(|scope| {
            let workers: Vec<_> = (0..threads)
                .map(|t| {
                    let total = &total;
                    scope.spawn(move || {
                        let (mut counts, mut runs) = (Vec::new(), Vec::new());
                        let mut row = vec![0u32; sx];
                        let mut column: Vec<Vec<Run>> = vec![Vec::new(); sx];
                        for z in (t * band)..((t + 1) * band).min(sz) {
                            column.iter_mut().for_each(Vec::clear);
                            for y in 0..sy {
                                source.read_row(layers, [min[0], min[1] + y as i32, min[2] + z as i32], &mut row);
                                for (x, v) in row.iter().enumerate() {
                                    if *v == 0 {
                                        continue;
                                    }
                                    let (id, y) = (*v as u16, y as u16);
                                    match column[x].last_mut() {
                                        Some(r) if r.id == id && r.hi + 1 == y => r.hi = y,
                                        _ => column[x].push(Run { lo: y, hi: y, id }),
                                    }
                                }
                            }
                            let n: usize = column.iter().map(Vec::len).sum();
                            if total.fetch_add(n as u64, std::sync::atomic::Ordering::Relaxed) + n as u64 > MOST_RUNS {
                                return Err("the build is too large to draw".to_string());
                            }
                            if !of_core::memory::within_limit() || counts.try_reserve(sx).is_err() || runs.try_reserve(n).is_err() {
                                return Err(of_core::memory::refusal(None));
                            }
                            for c in &column {
                                counts.push(c.len() as u32);
                                runs.extend_from_slice(c);
                            }
                        }
                        Ok((counts, runs))
                    })
                })
                .collect();
            workers.into_iter().map(|w| w.join().expect("a drawing thread stopped unexpectedly")).collect()
        });
        // The two large lists are reserved without aborting: a scene that
        // does not fit is refused (stage 6 of the column bake plan).
        let (mut starts, mut runs) = (Vec::new(), Vec::new());
        if starts.try_reserve_exact(sx * sz + 1).is_err() || runs.try_reserve_exact(total.into_inner() as usize).is_err() {
            return Err(of_core::memory::refusal(None));
        }
        starts.push(0u32);
        for band in bands {
            let (counts, part) = band?;
            for n in counts {
                starts.push(starts.last().copied().unwrap_or(0) + n);
            }
            runs.extend_from_slice(&part);
        }
        // Patches of 64, 16 and 4 columns (stage 7: a line of sight across
        // a tall box mostly crosses air).
        let heights: Vec<u32> =
            (0..sx * sz).map(|c| if starts[c + 1] > starts[c] { runs[starts[c + 1] as usize - 1].hi as u32 + 1 } else { 0 }).collect();
        let tops = [6, 4, 2].iter().map(|&shift| Tops::of(&heights, [sx, sz], shift)).collect();
        Ok(Self { min, size, starts, runs, paints, tops })
    }

    /// The paint at a cell of the box; 0 where there is none.
    fn at(&self, x: usize, y: usize, z: usize) -> u16 {
        let c = z * self.size[0] + x;
        let y = y as u16;
        // Runs go up the column without overlapping: find the first that
        // ends at or above y. A binary search, because a detailed build can
        // have hundreds of runs in a column.
        let runs = &self.runs[self.starts[c] as usize..self.starts[c + 1] as usize];
        match runs.get(runs.partition_point(|r| r.hi < y)) {
            Some(r) if r.lo <= y => r.id,
            _ => 0,
        }
    }
}

type V3 = [f64; 3];

fn dot(a: V3, b: V3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// The way the eye looks, and the right and up of the picture, for a viewer
/// standing in the (x, z) direction `from` and looking down by `pitch` degrees.
pub fn basis(from: (f64, f64), pitch: f64) -> (V3, V3, V3) {
    let pitch = pitch.clamp(5.0, 90.0).to_radians();
    let n = from.0.hypot(from.1).max(1e-9);
    let (ax, az) = (from.0 / n, from.1 / n);
    let f = [-ax * pitch.cos(), -pitch.sin(), -az * pitch.cos()];
    let r = [az, 0.0, -ax];
    let u = [r[1] * f[2] - r[2] * f[1], r[2] * f[0] - r[0] * f[2], r[0] * f[1] - r[1] * f[0]];
    (f, r, u)
}

/// A build made ready for drawing. Making it takes a moment for a large
/// build; drawing it from any side afterwards is quick.
pub struct Scene {
    grid: Grid,
}

/// A view of a scene for the app's 3D view: any side, any zoom, any picture size.
#[derive(Debug, Clone, PartialEq)]
pub struct View {
    /// Where the viewer stands, in degrees: 0 is south of the build looking north, 90 is east of it.
    pub yaw: f64,
    /// Degrees the view looks down; 90 is straight down.
    pub pitch: f64,
    /// Picture pixels per block.
    pub scale: f64,
    /// The point of the build at the middle of the picture.
    pub centre: [f64; 3],
    pub width: u32,
    pub height: u32,
    /// Samples per pixel along each axis, for smooth edges.
    pub samples: u32,
    /// A cutaway: blocks at or east of this X are left out, so the inside
    /// of the build shows. `None` shows everything.
    pub cut: Option<f64>,
}

impl View {
    pub fn basis(&self) -> (V3, V3, V3) {
        let yaw = self.yaw.to_radians();
        basis((yaw.sin(), yaw.cos()), self.pitch)
    }
}

struct Frame {
    forward: V3,
    right: V3,
    up: V3,
    /// Picture-plane position of the top left corner of the picture.
    left: f64,
    top: f64,
    scale: f64,
    width: u32,
    height: u32,
    samples: u32,
    /// Colour behind the build; `None` leaves it see-through.
    background: Option<[u8; 3]>,
    /// Blocks at or east of this X are left out.
    cut: Option<f64>,
}

impl Scene {
    pub fn new(source: &dyn BlockSource, layers: &[Layer], catalogue: &Catalogue) -> Result<Self, String> {
        if source.palette().len() >= u16::MAX as usize {
            return Err("the build has too many kinds of block to draw".into());
        }
        Ok(Self { grid: Grid::new(source, layers, catalogue)? })
    }

    /// Memory the scene holds, in bytes.
    pub fn bytes(&self) -> u64 {
        let tops: usize = self.grid.tops.iter().map(|t| t.top.len() * 4).sum();
        (self.grid.starts.len() * 4 + self.grid.runs.len() * std::mem::size_of::<Run>() + self.grid.paints.len() * std::mem::size_of::<Paint>() + tops)
            as u64
    }

    /// Lowest and highest corner of the box around every block.
    pub fn bounds(&self) -> (V3, V3) {
        let lo: V3 = self.grid.min.map(|v| v as f64);
        (lo, std::array::from_fn(|a| lo[a] + self.grid.size[a] as f64))
    }

    /// Draw one view. Where there are no blocks the picture is see-through.
    pub fn draw(&self, view: &View) -> Picture {
        let (forward, right, up) = view.basis();
        let scale = view.scale.max(1e-6);
        let (width, height) = (view.width.max(1), view.height.max(1));
        self.trace(&Frame {
            forward,
            right,
            up,
            left: dot(view.centre, right) - width as f64 / (2.0 * scale),
            top: dot(view.centre, up) + height as f64 / (2.0 * scale),
            scale,
            width,
            height,
            samples: view.samples.clamp(1, 4),
            background: None,
            cut: view.cut,
        })
    }

    fn trace(&self, frame: &Frame) -> Picture {
        let grid = &self.grid;
        let (lo, mut hi) = self.bounds();
        // A cutaway shortens the box of blocks from the east.
        let mut size = grid.size;
        if let Some(cut) = frame.cut {
            size[0] = ((cut - lo[0]).ceil().max(1.0) as usize).min(grid.size[0]);
            hi[0] = lo[0] + size[0] as f64;
        }
        let (forward, right, up) = (frame.forward, frame.right, frame.up);

        // Colour gathered along one line of sight, and how much of what lies behind still shows.
        let trace = |sr: f64, su: f64| -> ([f32; 3], f32) {
            let miss = ([0f32; 3], 1f32);
            let origin: V3 = std::array::from_fn(|a| right[a] * sr + up[a] * su);
            // Where the line enters and leaves the box of blocks.
            let (mut t_in, mut t_out, mut face) = (f64::MIN, f64::MAX, 1usize);
            for a in 0..3 {
                if forward[a].abs() < 1e-12 {
                    if origin[a] <= lo[a] || origin[a] >= hi[a] {
                        return miss;
                    }
                    continue;
                }
                let (t1, t2) = ((lo[a] - origin[a]) / forward[a], (hi[a] - origin[a]) / forward[a]);
                let (near, far) = (t1.min(t2), t1.max(t2));
                if near > t_in {
                    t_in = near;
                    face = a;
                }
                t_out = t_out.min(far);
            }
            if t_in >= t_out {
                return miss;
            }
            let start: V3 = std::array::from_fn(|a| origin[a] + forward[a] * (t_in + 1e-6));
            let mut cell: [i64; 3] = std::array::from_fn(|a| ((start[a] - lo[a]).floor() as i64).clamp(0, size[a] as i64 - 1));
            let step: [i64; 3] = std::array::from_fn(|a| if forward[a] > 0.0 { 1 } else { -1 });
            let delta: V3 = std::array::from_fn(|a| if forward[a].abs() < 1e-12 { f64::MAX } else { 1.0 / forward[a].abs() });
            let mut next: V3 = std::array::from_fn(|a| {
                if forward[a].abs() < 1e-12 {
                    return f64::MAX;
                }
                let edge = lo[a] + (cell[a] + if forward[a] > 0.0 { 1 } else { 0 }) as f64;
                (edge - origin[a]) / forward[a]
            });
            let (mut colour, mut clear, mut last) = ([0f32; 3], 1f32, 0u16);
            // Where along the line the current cell was entered.
            let mut t_now = t_in;
            'cells: loop {
                // Above every block of the patch around this cell: leave the
                // patch in one jump, sideways or down to its highest block.
                // The view always looks down, so the line cannot climb back.
                if forward[1] < 0.0 {
                    for tops in &grid.tops {
                        let tile = [cell[0] as usize >> tops.shift, cell[2] as usize >> tops.shift];
                        let top = tops.top[tile[1] * tops.across + tile[0]] as i64;
                        if cell[1] < top {
                            continue;
                        }
                        let side = 1i64 << tops.shift;
                        let (mut t_jump, mut axis) = ((lo[1] + top as f64 - origin[1]) / forward[1], 1usize);
                        for (a, t) in [(0usize, tile[0]), (2, tile[1])] {
                            if forward[a].abs() < 1e-12 {
                                continue;
                            }
                            let edge = if forward[a] > 0.0 { ((t as i64 + 1) * side).min(size[a] as i64) } else { t as i64 * side };
                            let t_edge = (lo[a] + edge as f64 - origin[a]) / forward[a];
                            if t_edge < t_jump {
                                (t_jump, axis) = (t_edge, a);
                            }
                        }
                        let t = t_jump.max(t_now);
                        // The crossed axis moves exactly one cell over its
                        // edge; the others are where the line is at that point.
                        for a in 0..3 {
                            cell[a] = if a == axis {
                                match a {
                                    1 => top - 1,
                                    _ if forward[a] > 0.0 => ((tile[a / 2] as i64 + 1) * side).min(size[a] as i64),
                                    _ => tile[a / 2] as i64 * side - 1,
                                }
                            } else {
                                ((origin[a] + forward[a] * t - lo[a]).floor() as i64).clamp(0, size[a] as i64 - 1)
                            };
                        }
                        if cell[axis] < 0 || cell[axis] >= size[axis] as i64 {
                            break 'cells;
                        }
                        for a in 0..3 {
                            if forward[a].abs() >= 1e-12 {
                                let edge = lo[a] + (cell[a] + if forward[a] > 0.0 { 1 } else { 0 }) as f64;
                                next[a] = (edge - origin[a]) / forward[a];
                            }
                        }
                        (face, last, t_now) = (axis, 0, t);
                        continue 'cells;
                    }
                }
                let id = grid.at(cell[0] as usize, cell[1] as usize, cell[2] as usize);
                // The game draws no face between two blocks of the same glass.
                if id != 0 && id != last {
                    let paint = grid.paints[id as usize];
                    // Face shading as in the game: top full, north and south 0.8, east and west 0.6.
                    let (shade, tone) = match face {
                        1 => (1.0, paint.top),
                        2 => (0.8, paint.side),
                        _ => (0.6, paint.side),
                    };
                    for c in 0..3 {
                        colour[c] += clear * paint.cover * tone[c] * shade;
                    }
                    clear *= 1.0 - paint.cover;
                    if clear < 0.004 {
                        return (colour, 0.0);
                    }
                }
                last = id;
                face = if next[0] <= next[1] && next[0] <= next[2] {
                    0
                } else if next[1] <= next[2] {
                    1
                } else {
                    2
                };
                t_now = next[face];
                cell[face] += step[face];
                next[face] += delta[face];
                if cell[face] < 0 || cell[face] >= size[face] as i64 {
                    break;
                }
            }
            (colour, clear)
        };

        let (width, height, samples) = (frame.width, frame.height, frame.samples);
        let rows: Vec<u32> = (0..height).collect();
        let threads = std::thread::available_parallelism().map_or(4, |n| n.get()).min(rows.len().max(1));
        let chunk = rows.len().div_ceil(threads).max(1);
        let trace = &trace;
        let mut rgba = Vec::with_capacity((width * height * 4) as usize);
        std::thread::scope(|scope| {
            let workers: Vec<_> = rows
                .chunks(chunk)
                .map(|part| {
                    scope.spawn(move || {
                        let mut out = Vec::with_capacity(part.len() * width as usize * 4);
                        for &py in part {
                            for px in 0..width {
                                let (mut sum, mut clear) = ([0f32; 3], 0f32);
                                for sy in 0..samples {
                                    for sx in 0..samples {
                                        let fx = px as f64 + (sx as f64 + 0.5) / samples as f64;
                                        let fy = py as f64 + (sy as f64 + 0.5) / samples as f64;
                                        let (c, through) = trace(frame.left + fx / frame.scale, frame.top - fy / frame.scale);
                                        for i in 0..3 {
                                            sum[i] += c[i];
                                        }
                                        clear += through;
                                    }
                                }
                                let n = (samples * samples) as f32;
                                let clear = clear / n;
                                let pixel = match frame.background {
                                    Some(back) => {
                                        let c: [u8; 3] = std::array::from_fn(|i| (sum[i] / n + clear * back[i] as f32 + 0.5) as u8);
                                        [c[0], c[1], c[2], 255]
                                    }
                                    None if clear >= 0.999 => [0, 0, 0, 0],
                                    None => {
                                        let cover = 1.0 - clear;
                                        let c: [u8; 3] = std::array::from_fn(|i| (sum[i] / n / cover + 0.5).min(255.0) as u8);
                                        [c[0], c[1], c[2], (cover * 255.0 + 0.5) as u8]
                                    }
                                };
                                out.extend_from_slice(&pixel);
                            }
                        }
                        out
                    })
                })
                .collect();
            for worker in workers {
                rgba.extend(worker.join().expect("a drawing thread stopped unexpectedly"));
            }
        });
        Picture { width, height, rgba }
    }
}

/// Draw the chosen layers of a build, with the picture sized to fit all of it.
pub fn render(source: &dyn BlockSource, layers: &[Layer], catalogue: &Catalogue, camera: &Camera) -> Result<Picture, String> {
    picture(&Scene::new(source, layers, catalogue)?, camera)
}

/// Draw a scene already made, with the picture sized to fit all of it.
pub fn picture(scene: &Scene, camera: &Camera) -> Result<Picture, String> {
    let (lo, hi) = scene.bounds();
    let (forward, right, up) =
        if camera.pitch >= 89.9 { ([0.0, -1.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, -1.0]) } else { basis(camera.from, camera.pitch) };
    let (mut r0, mut r1, mut u0, mut u1) = (f64::MAX, f64::MIN, f64::MAX, f64::MIN);
    for corner in 0..8 {
        let c: V3 = std::array::from_fn(|a| if corner >> a & 1 == 0 { lo[a] } else { hi[a] });
        r0 = r0.min(dot(c, right));
        r1 = r1.max(dot(c, right));
        u0 = u0.min(dot(c, up));
        u1 = u1.max(dot(c, up));
    }
    let margin = 12.0;
    let width = ((r1 - r0) * camera.scale + 2.0 * margin).ceil() as u32;
    let height = ((u1 - u0) * camera.scale + 2.0 * margin).ceil() as u32;
    if width as u64 * height as u64 > 64_000_000 {
        return Err("the picture would be too large; use a smaller scale".into());
    }
    Ok(scene.trace(&Frame {
        forward,
        right,
        up,
        left: r0 - margin / camera.scale,
        top: u1 + margin / camera.scale,
        scale: camera.scale,
        width,
        height,
        samples: camera.samples.clamp(1, 4),
        background: Some(camera.background),
        cut: None,
    }))
}
