//! The GPU particle pool: the particles an emitter handed over (`torque_render_scene::particles::Emitter::hand_over`, through
//! [`HdScene::sprites`](torque_render_scene::hd::HdScene::sprites)) stepped, aged and drawn on the GPU, with
//! [`HdFeatures::gpu_particles`] on ([`ParticlePool::Auto`]: at Medium and up).
//!
//! - **The pool**: [`sprite_capacity`] particles of the quality (64 bytes each: `gpu_particle_types.wgsl`'s `PoolParticle`), a
//!   free slot all zero, held to [`HdWorld::set_gpu_particle_cap`] (by default the CPU path's `MAX_PARTICLES`: turning the
//!   pool on raises no count or overdraw until a fill budget bounds them; a spawn past the free slots is dropped and
//!   counted, as TGE drops a particle past its cap). Made the first frame the feature is on, made again when the quality changes (its particles dropped),
//!   dropped when the feature goes off.
//! - **A frame** (`gpu_particles.wgsl`, one compute pass before the frame's other passes, only when the scene handed over a new
//!   frame or the render origin moved): the step and the deaths (`cs_update`, TGE's step and key walk), the spawns into the
//!   free slots of the last frame's list (`cs_spawn`), the fill governor (`cs_fill`, `cs_governor`), then the lists: each slot's
//!   key (its look's bucket at its age, or free),
//!   its rank among its workgroup's slots of that key by bit masks, the counts' prefix sums, the scatter. A bucket's
//!   particles are in slot order and the free slots after the last bucket's, so the same frames make the same pictures byte
//!   for byte (no atomic append decides an order).
//! - **The draws**: one [`DrawRef`] a bucket that may hold a live particle ([`GpuSprites::live_buckets`]), its `sprites` the
//!   bucket. They go through the translucent list as the CPU path's particles do (`particle: true`: weighted by the WBOIT, never
//!   the front layer; the additive ones in the late pass after the TAA; the blended fast thin ones, a bucket's `late`, in the
//!   TAA frame's over pass after it), each one `draw_indirect` of the bucket's args with the
//!   forward module's `vs_gpu_sprite` (`gpu_sprite.wgsl`) and the stage's own fragment entry: lighting, soft particles, fog
//!   and the late pass are the forward pass's code. Group 1 is the texture group with the pool's particles, list, looks and
//!   fill scales for the vertex stage (read-only: a vertex stage gets no writable storage).
//! - **Looks beyond TGE's**: a flipbook's frames are one atlas texture (`torque_render_scene::hd::atlas`, built when its record
//!   is made) and its bucket the atlas's: the vertex stage finds the frame from the particle's age and the next frame's place
//!   (`VertexOut::uv2`), the fragment mixes them by `pool.x` when the look asks (`flipbookBlend`). `softDistance` is the soft
//!   particle's distance (`pool.z`), `lit` decides the material's lit bit, `sizeByVelocity` stretches the sprite along its motion
//!   and makes a blended bucket late. A lit sprite's sun visibility is found once in the vertex stage (`pool.y`; the frame
//!   group's shadow bindings are visible to it), where the CPU path's lit sprites still find it at each pixel.
//! - **The fill governor**: `cs_fill` sums the screen area every live sprite covers (clipped to the screen, from the main
//!   camera in the `Sim` uniform: workgroup partial sums in a fixed tree, so the same frame sums the same), `cs_governor` adds
//!   the partials and, past the quality's budget ([`fill_budget`], in frames of the screen), writes the scale the sprites' sizes
//!   draw at (the root of budget over total, so their area meets the budget) and the alpha scale (its root, not below 0.3);
//!   the vertex stage applies both. It reads the same frame's positions: no frame of lag. [`HdWorld::gpu_particle_counts`]
//!   reads the last frame's total and scales back.
//!
//! # Maintaining
//!
//! - Module map: this file (the pool's buffers, its compute pass, its pipelines and groups, the frame's hooks: `prepare_sprites`,
//!   `encode_sprites`, `sprite_draws`, `ensure_sprites`, `draw_sprites`); `shaders/gpu_particle_types.wgsl` (the records, shared);
//!   `shaders/gpu_particles.wgsl` (the compute pass); `shaders/gpu_sprite.wgsl` (the vertex entry); `torque_render_scene::hd::sprites`
//!   (the record table, the buckets, the hand-over), `torque_render_scene::hd::atlas` (the flipbook atlases) and
//!   `torque_render_scene::particles` (`hand_over`, `SpriteLook`, `GpuSpawn`).
//! - Invariants: the records' and particles' byte layouts are `SpriteRecord::write` and `GpuSpawn::write` (the scene's tests check
//!   their sizes, `every_shader_permutation_is_valid_wgsl` the shaders); `Sim::keys` is the scene's bucket count plus one (the free
//!   slots' key, last) and at most `MAX_KEYS` (65: 64 buckets); a bucket's draw is its 16 bytes of `args`; the free slots' place
//!   and count and the spawns dropped (the last pass's, all passes') follow at `FREE_AT`; `SIM_BYTES` is the `Sim` uniform's size
//!   (`prepare` writes it in `gpu_particles.wgsl`'s field order); the compute group binds eight storage buffers, the most a
//!   WebGPU device promises a stage, so a new buffer must share one of these (`sprites_supported` checks the limit);
//!   `keys` holds the fill governor's partial sums between `cs_fill` and `cs_governor`, then `cs_count`'s keys. The two-run test (`tests/gpu_particles.rs`) holds the pictures byte-identical; its CPU-path
//!   comparison holds the step and the corners to the CPU path's.
//! - The wind is `Sim::wind` (the frame's `GpuSprites::wind`, the Sky's): the step carries a particle by `windCoefficient` times it
//!   on top of its own velocity (`p += (v + wind * coef) dt`, as `Emitter::step` does). A particle's ejection direction rides in
//!   its flags above the two low bits (`pool_orient_dir`, `orient_bits`), and a look's orient mode is the third word of
//!   its record (`SpriteRecord::write`): `vs_gpu_sprite` turns the square along the direction (mode 1) or the velocity (mode 2).
//! - Extension points: the step's new terms (turbulence,
//!   collision: `cs_update`, which can bind more after `args`), a look's new fields (`PoolLook` and `SpriteRecord::write`
//!   together), a per-emitter budget (`cs_spawn` takes the free slots in request order), a sort (a pass after `cs_scatter` over a
//!   bucket's list), trails (another vertex entry over the same lists), the governor's rule (`cs_governor`: the scales it writes
//!   are all the vertex stage reads).
//! - Checks: `cargo test -p torque-render-hd --test gpu_particles` (the pictures, the atlas blend, the fill governor's budget and
//!   time, the late bucket, a sprite over bare sky), `cargo test -p torque-render-scene --lib -- sprites atlas`,
//!   `cargo run -p torque-render-hd --release --example particles-profile` (under the GPU bench lock; `--scale 64` with
//!   `--count 10000` is 512-pixel sprites and prints the governor's line).

use torque_render_scene::hd::Quality;
use torque_render_scene::hd::sprites::{GpuSprites, MAX_BUCKETS, MAX_STEPS, SpriteRecord};
use torque_render_scene::particles::{GpuSpawn, MAX_CALL_MS};

use super::*;

/// The pool's particles at each quality (the most it can hold: [`HdWorld::set_gpu_particle_cap`] holds it lower).
pub fn sprite_capacity(quality: Quality) -> u32 {
    match quality {
        Quality::Low => 20_000,
        Quality::Medium => 100_000,
        Quality::High => 250_000,
        Quality::Ultra => 500_000,
    }
}

/// Whether the frames run the GPU particle pool ([`HdFeatures::gpu_particles`]).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ParticlePool {
    /// At [`Quality::Medium`] and up; Low draws its particles as the CPU path builds them.
    #[default]
    Auto,
    /// At every quality.
    On,
    /// At none: the CPU path's particles are all there is.
    Off,
}

impl ParticlePool {
    /// Whether the pool runs at `quality`.
    pub fn runs_at(self, quality: Quality) -> bool {
        match self {
            ParticlePool::Auto => quality != Quality::Low,
            ParticlePool::On => true,
            ParticlePool::Off => false,
        }
    }
}

/// The sprite area a frame may cover at `quality`, in frames of the screen (the fill governor's budget: past it the sprites
/// shrink and thin until their summed area is under it).
pub fn fill_budget(quality: Quality) -> f32 {
    match quality {
        Quality::Low => 2.0,
        Quality::Medium => 4.0,
        Quality::High => 8.0,
        Quality::Ultra => 16.0,
    }
}

/// The fill governor's last frame: the screen area (pixels) the pool's live sprites cover at full size, the budget they were
/// held to (0: none), and the scale their sizes were drawn at (1 under the budget).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct FillCounts {
    pub requested: f32,
    pub budget: f32,
    pub size_scale: f32,
    pub alpha_scale: f32,
}

/// The particles the last frame's lists hold in each bucket of the scene's table (in its order), the free slots, the
/// spawns the pool dropped so far for want of a free slot, and the fill governor's last frame ([`HdWorld::gpu_particle_counts`]).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SpriteCounts {
    pub buckets: Vec<u32>,
    pub free: u32,
    pub dropped: u64,
    pub fill: FillCounts,
}

impl SpriteCounts {
    /// The live particles.
    pub fn live(&self) -> u32 {
        self.buckets.iter().sum()
    }
}

/// `gpu_particles.wgsl`'s workgroup size, keys, scan block and the free slots' place in `args`.
const GROUP: u32 = 256;
const MAX_KEYS: u32 = MAX_BUCKETS as u32 + 1;
const SCAN_BLOCK: u32 = 1024;
const FREE_AT: u32 = MAX_KEYS * 4;
/// The words after `FREE_AT`: the free slots' place and count, the spawns dropped by the last pass and by all of them.
const ARGS_TAIL: u32 = 4;
/// The `Sim` uniform's bytes.
const SIM_BYTES: u64 = 288;
/// The compute pass's entries, in dispatch order.
const PASSES: [&str; 8] = [
    "cs_update",
    "cs_spawn",
    "cs_fill",
    "cs_governor",
    "cs_count",
    "cs_scan_blocks",
    "cs_scan_top",
    "cs_scatter",
];

/// What the frame's compute pass runs: its spawns and keys.
#[derive(Debug, Clone, Copy)]
struct Run {
    spawns: u32,
    keys: u32,
}

/// The pool on the GPU.
pub(crate) struct GpuParticles {
    capacity: u32,
    groups: u32,
    particles: wgpu::Buffer,
    spawns: wgpu::Buffer,
    looks: wgpu::Buffer,
    keys: wgpu::Buffer,
    scan: wgpu::Buffer,
    instances: wgpu::Buffer,
    args: wgpu::Buffer,
    /// The fill governor's result (`gpu_particles.wgsl`'s `fill`).
    fill: wgpu::Buffer,
    sim: wgpu::Buffer,
    sim_layout: wgpu::BindGroupLayout,
    sim_group: wgpu::BindGroup,
    passes: Vec<wgpu::ComputePipeline>,
    sprite_layout: wgpu::BindGroupLayout,
    pipeline_layout: wgpu::PipelineLayout,
    modules: HashMap<(bool, bool), wgpu::ShaderModule>,
    pipelines: HashMap<PipelineKey, wgpu::RenderPipeline>,
    sprite_groups: HashMap<(TexRef, SamplerKey), wgpu::BindGroup>,
    /// The scene's frame and record table last taken, the origin's shifts applied, the last run's keys.
    generation: Option<u64>,
    records_generation: Option<u64>,
    offset: [f64; 3],
    keys_run: u32,
    /// The spawns past the pool's capacity, left out before the pass.
    dropped: u64,
    run: Option<Run>,
    bytes: Vec<u8>,
}

/// The sprites' group 1: the texture and sampler (the fragment's), the particles, the list, the looks and the fill governor's
/// scales (the vertex stage's).
fn sprite_entries() -> Vec<wgpu::BindGroupLayoutEntry> {
    let mut e = texture_entries();
    for b in 2..6 {
        e.push(crate::storage(b, true, wgpu::ShaderStages::VERTEX));
    }
    e
}

/// The compute pass's group: the uniform, then the pool's buffers (`gpu_particles.wgsl`'s bindings).
fn sim_entries() -> Vec<wgpu::BindGroupLayoutEntry> {
    let c = wgpu::ShaderStages::COMPUTE;
    let mut e = vec![crate::uniform(0, c)];
    for (b, read_only) in [
        (1, false),
        (2, true),
        (3, true),
        (4, false),
        (5, false),
        (6, false),
        (7, false),
        (8, false),
    ] {
        e.push(crate::storage(b, read_only, c));
    }
    e
}

/// Whether `device` runs the pool: a backend that executes indirect draws (as [`crate::arena::DrawWay`] reads it, and WebGPU's), and
/// room for the sprites' and the compute pass's bindings.
pub(super) fn sprites_supported(device: &wgpu::Device) -> bool {
    let indirect = matches!(
        device.adapter_info().backend,
        wgpu::Backend::Dx12
            | wgpu::Backend::Vulkan
            | wgpu::Backend::Metal
            | wgpu::Backend::BrowserWebGpu
    );
    let groups = [
        frame_entries(),
        sprite_entries(),
        crate::sky::group_entries(),
        reflect::layout_entries(),
    ];
    let groups: Vec<&[wgpu::BindGroupLayoutEntry]> = groups.iter().map(Vec::as_slice).collect();
    let needs = crate::limits::PipelineNeeds {
        name: "particle sprites",
        vertex: crate::limits::count(&groups, wgpu::ShaderStages::VERTEX),
        fragment: crate::limits::count(&groups, wgpu::ShaderStages::FRAGMENT),
        compute: crate::limits::StageCounts::default(),
    };
    let limits = device.limits();
    let compute = crate::limits::count(&[&sim_entries()], wgpu::ShaderStages::COMPUTE);
    indirect
        && crate::limits::check_needs(&[needs], &limits).is_ok()
        && limits.max_storage_buffers_per_shader_stage >= compute.storage_buffers
}

fn buffer(
    device: &wgpu::Device,
    label: &str,
    bytes: u64,
    usage: wgpu::BufferUsages,
) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size: bytes.max(16),
        usage: usage | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

impl GpuParticles {
    fn new(
        device: &wgpu::Device,
        cache: Option<&wgpu::PipelineCache>,
        capacity: u32,
        layouts: [&wgpu::BindGroupLayout; 3],
    ) -> GpuParticles {
        let storage = wgpu::BufferUsages::STORAGE;
        let groups = capacity.div_ceil(GROUP);
        let scan_len = MAX_KEYS * groups;
        let particles = buffer(
            device,
            "hd particle pool",
            u64::from(capacity) * GpuSpawn::BYTES as u64,
            storage,
        );
        let keys = buffer(device, "hd particle keys", u64::from(capacity) * 4, storage);
        let scan = buffer(
            device,
            "hd particle scan",
            u64::from(scan_len + scan_len.div_ceil(SCAN_BLOCK)) * 4,
            storage,
        );
        // At first every slot is free, in order.
        let mut list = Vec::with_capacity(capacity as usize * 4);
        push_u32(&mut list, &(0..capacity).collect::<Vec<u32>>());
        let instances = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("hd particle list"),
            contents: &list,
            usage: storage | wgpu::BufferUsages::COPY_DST,
        });
        let mut words = vec![0u32; FREE_AT as usize + ARGS_TAIL as usize];
        words[FREE_AT as usize + 1] = capacity;
        let mut arg_bytes = Vec::new();
        push_u32(&mut arg_bytes, &words);
        let args = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("hd particle draws"),
            contents: &arg_bytes,
            usage: storage | wgpu::BufferUsages::INDIRECT | wgpu::BufferUsages::COPY_SRC,
        });
        let fill = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("hd particle fill"),
            // Before the first frame's pass: no scale.
            contents: &[1.0f32, 1.0, 0.0, 0.0].map(f32::to_le_bytes).concat(),
            usage: storage | wgpu::BufferUsages::COPY_SRC,
        });
        let spawns = buffer(
            device,
            "hd particle spawns",
            64 * GpuSpawn::BYTES as u64,
            storage,
        );
        let looks = buffer(
            device,
            "hd particle looks",
            16 * SpriteRecord::BYTES as u64,
            storage,
        );
        let sim = buffer(
            device,
            "hd particle frame",
            SIM_BYTES,
            wgpu::BufferUsages::UNIFORM,
        );
        let sim_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("hd particle frame"),
            entries: &sim_entries(),
        });
        let module = crate::module(device, Permutation::Particles);
        let passes = PASSES
            .iter()
            .map(|e| crate::compute_filter(device, cache, "hd particles", &sim_layout, &module, e))
            .collect();
        let sprite_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("hd particle sprites"),
            entries: &sprite_entries(),
        });
        let [frame, sky, refl] = layouts;
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("hd particle sprites"),
            bind_group_layouts: &[Some(frame), Some(&sprite_layout), Some(sky), Some(refl)],
            immediate_size: 0,
        });
        GpuParticles {
            capacity,
            groups,
            sim_group: sim_group(
                device,
                &sim_layout,
                [
                    &sim, &particles, &spawns, &looks, &keys, &scan, &instances, &args, &fill,
                ],
            ),
            particles,
            spawns,
            looks,
            keys,
            scan,
            instances,
            args,
            fill,
            sim,
            sim_layout,
            passes,
            sprite_layout,
            pipeline_layout,
            modules: HashMap::new(),
            pipelines: HashMap::new(),
            sprite_groups: HashMap::new(),
            generation: None,
            records_generation: None,
            offset: [0.0; 3],
            keys_run: 1,
            dropped: 0,
            run: None,
            bytes: Vec::new(),
        }
    }

    /// The compute group made again (a buffer it binds grew), and the sprites' groups let go (made again as drawn).
    fn regroup(&mut self, device: &wgpu::Device) {
        self.sim_group = sim_group(
            device,
            &self.sim_layout,
            [
                &self.sim,
                &self.particles,
                &self.spawns,
                &self.looks,
                &self.keys,
                &self.scan,
                &self.instances,
                &self.args,
                &self.fill,
            ],
        );
        self.sprite_groups.clear();
    }

    /// `buffer` with room for `bytes` (made again, twice as big as asked, when it has not: then true).
    fn room(device: &wgpu::Device, buffer: &mut wgpu::Buffer, bytes: usize, label: &str) -> bool {
        if buffer.size() >= bytes as u64 {
            return false;
        }
        *buffer = self::buffer(device, label, bytes as u64 * 2, wgpu::BufferUsages::STORAGE);
        true
    }

    /// Takes the scene's hand-over: the record table when it changed, and (when the frame is new or the origin moved) the
    /// frame's spawns and its `Sim` (the frame's [`Run`]).
    fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        s: &GpuSprites,
        camera: &HdCamera,
        budget: f32,
    ) {
        self.run = None;
        let mut grown = false;
        if self.records_generation != Some(s.records_generation) {
            self.records_generation = Some(s.records_generation);
            self.bytes.clear();
            for r in &s.records {
                r.write(&mut self.bytes);
            }
            grown |= Self::room(
                device,
                &mut self.looks,
                self.bytes.len(),
                "hd particle looks",
            );
            if !self.bytes.is_empty() {
                queue.write_buffer(&self.looks, 0, &self.bytes);
            }
        }
        let new = self.generation != Some(s.generation);
        let shift = [0, 1, 2].map(|k| (s.offset[k] - self.offset[k]) as f32);
        if !new && shift == [0.0; 3] {
            if grown {
                self.regroup(device);
            }
            return;
        }
        self.generation = Some(s.generation);
        self.offset = s.offset;
        if new {
            self.dropped += s.spawns.len().saturating_sub(self.capacity as usize) as u64;
        }
        let spawns: &[GpuSpawn] = if new {
            &s.spawns[..s.spawns.len().min(self.capacity as usize)]
        } else {
            &[]
        };
        self.bytes.clear();
        for sp in spawns {
            sp.write(&mut self.bytes);
        }
        grown |= Self::room(
            device,
            &mut self.spawns,
            self.bytes.len(),
            "hd particle spawns",
        );
        if grown {
            self.regroup(device);
        }
        if !self.bytes.is_empty() {
            queue.write_buffer(&self.spawns, 0, &self.bytes);
        }
        let keys = (s.buckets.len() as u32 + 1).min(MAX_KEYS);
        let scan_len = keys * self.groups;
        // Each step as the CPU's `advance` takes it (at most `MAX_CALL_MS`).
        let mut steps = [0u32; MAX_STEPS];
        let mut step_count = 0;
        if new {
            for (to, &ms) in steps.iter_mut().zip(&s.steps) {
                *to = ms.min(MAX_CALL_MS);
                step_count += 1;
            }
        }
        let mut sim = Vec::with_capacity(SIM_BYTES as usize);
        push_u32(
            &mut sim,
            &[self.capacity, spawns.len() as u32, step_count, self.groups],
        );
        push_u32(
            &mut sim,
            &[
                keys,
                scan_len,
                MAX_KEYS * self.groups,
                u32::from(new && s.clear),
            ],
        );
        push_f32(&mut sim, &[shift[0], shift[1], shift[2], 0.0]);
        push_f32(&mut sim, &[s.wind[0], s.wind[1], s.wind[2], 0.0]);
        push_u32(&mut sim, &steps);
        push_f32(&mut sim, &camera.view_proj);
        let (w, h) = (camera.width as f32, camera.height as f32);
        push_f32(&mut sim, &[camera.proj[0], camera.proj[5], w, h]);
        push_f32(&mut sim, &[budget * w * h, 0.0, 0.0, 0.0]);
        queue.write_buffer(&self.sim, 0, &sim);
        self.keys_run = keys;
        self.run = Some(Run {
            spawns: spawns.len() as u32,
            keys,
        });
    }

    /// The frame's compute pass (none when the frame runs none): true when encoded.
    fn encode(&self, encoder: &mut wgpu::CommandEncoder) -> bool {
        let Some(run) = self.run else {
            return false;
        };
        crate::count_pass();
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("hd particles"),
            timestamp_writes: None,
        });
        pass.set_bind_group(0, &self.sim_group, &[]);
        let sizes = [
            self.groups,
            run.spawns.div_ceil(GROUP),
            self.groups,
            1,
            self.groups,
            (run.keys * self.groups).div_ceil(SCAN_BLOCK),
            1,
            self.groups,
        ];
        for (p, n) in self.passes.iter().zip(sizes) {
            if n > 0 {
                pass.set_pipeline(p);
                pass.dispatch_workgroups(n, 1, 1);
            }
        }
        true
    }

    fn ensure_pipeline(
        &mut self,
        device: &wgpu::Device,
        cache: Option<&wgpu::PipelineCache>,
        key: PipelineKey,
    ) {
        if self.pipelines.contains_key(&key) {
            return;
        }
        let module = self
            .modules
            .entry((key.clustered, key.front_layer))
            .or_insert_with(|| {
                crate::module(
                    device,
                    Permutation::Sprite {
                        clustered: key.clustered,
                        front_layer: key.front_layer,
                    },
                )
            });
        let p = make_pipeline(
            device,
            cache,
            &self.pipeline_layout,
            module,
            key,
            Vertices::Sprites,
        );
        self.pipelines.insert(key, p);
    }

    fn ensure_group(
        &mut self,
        device: &wgpu::Device,
        key: (TexRef, SamplerKey),
        view: &wgpu::TextureView,
        sampler: &wgpu::Sampler,
    ) {
        if self.sprite_groups.contains_key(&key) {
            return;
        }
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("hd particle sprites"),
            layout: &self.sprite_layout,
            entries: &[
                crate::tv(0, view),
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(sampler),
                },
                whole(2, &self.particles),
                whole(3, &self.instances),
                whole(4, &self.looks),
                whole(5, &self.fill),
            ],
        });
        self.sprite_groups.insert(key, group);
    }
}

/// A bind group entry of all of `buffer`.
fn whole(binding: u32, buffer: &wgpu::Buffer) -> wgpu::BindGroupEntry<'_> {
    wgpu::BindGroupEntry {
        binding,
        resource: buffer.as_entire_binding(),
    }
}

fn sim_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    buffers: [&wgpu::Buffer; 9],
) -> wgpu::BindGroup {
    let entries: Vec<wgpu::BindGroupEntry> = buffers
        .iter()
        .enumerate()
        .map(|(b, buffer)| whole(b as u32, buffer))
        .collect();
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("hd particle frame"),
        layout,
        entries: &entries,
    })
}

impl<E: HdExtension> HdWorld<E> {
    /// Whether the frames run the GPU particle pool at the quality of the scene last drawn ([`HdFeatures::gpu_particles`]) on a
    /// device that runs it (indirect draws executed, room for its bindings). A caller hands its emitters' particles over only
    /// then; else they draw as before.
    pub fn gpu_particles_on(&self) -> bool {
        self.gpu_particles_at(self.pool_quality)
    }

    /// [`HdWorld::gpu_particles_on`] at `quality`.
    pub fn gpu_particles_at(&self, quality: Quality) -> bool {
        self.features.gpu_particles.runs_at(quality) && self.sprites_supported
    }

    /// The most particles the pool holds, below its quality's [`sprite_capacity`] (by default the CPU path's
    /// `MAX_PARTICLES`: the pool holds no more than the CPU path draws until a fill budget bounds its overdraw); a change makes
    /// the pool again at the next frame (its particles dropped).
    pub fn set_gpu_particle_cap(&mut self, cap: u32) {
        self.sprite_cap = cap;
    }

    /// The particles in the pool's lists after the last frame's pass, by the scene's buckets, and its free slots (None without a
    /// pool; waits for the GPU).
    pub fn gpu_particle_counts(&self) -> Result<Option<SpriteCounts>, HdError> {
        let Some(pool) = &self.sprites else {
            return Ok(None);
        };
        let bytes = self.read_buffer(&pool.args, u64::from(FREE_AT + ARGS_TAIL) * 4)?;
        let word = |i: u32| {
            let at = i as usize * 4;
            u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
        };
        let fill = self.read_buffer(&pool.fill, 16)?;
        let scale = |i: usize| {
            f32::from_le_bytes([
                fill[4 * i],
                fill[4 * i + 1],
                fill[4 * i + 2],
                fill[4 * i + 3],
            ])
        };
        Ok(Some(SpriteCounts {
            buckets: (0..pool.keys_run - 1).map(|b| word(b * 4) / 6).collect(),
            free: word(FREE_AT + 1),
            dropped: u64::from(word(FREE_AT + 3)) + pool.dropped,
            fill: FillCounts {
                requested: scale(2),
                budget: scale(3),
                size_scale: scale(0),
                alpha_scale: scale(1),
            },
        }))
    }

    /// The frame's hand-over into the pool (made or made again for the quality; dropped with the feature off), and the textures
    /// its looks name.
    pub(super) fn prepare_sprites(&mut self, scene: &E::Scene, camera: &HdCamera) {
        let quality = E::mode(scene).unwrap_or_else(E::fallback_mode).quality;
        self.pool_quality = quality;
        if !self.gpu_particles_on() {
            self.sprites = None;
            return;
        }
        let capacity = sprite_capacity(quality).min(self.sprite_cap).max(1);
        if self.sprites.as_ref().is_none_or(|p| p.capacity != capacity) {
            self.sprites = Some(GpuParticles::new(
                &self.device,
                self.pipeline_cache.as_ref(),
                capacity,
                [&self.frame_layout, &self.sky.layout, &self.refl.layout],
            ));
        }
        while self.textures.len() < scene.textures.textures.len() {
            let view = self.upload_texture(&scene.textures.textures[self.textures.len()]);
            self.textures.push(view);
        }
        if let Some(pool) = &mut self.sprites {
            pool.prepare(
                &self.device,
                &self.queue,
                &scene.sprites,
                camera,
                fill_budget(quality),
            );
        }
    }

    /// The frame's pool pass into `encoder`: true when encoded.
    pub(super) fn encode_sprites(&self, encoder: &mut wgpu::CommandEncoder) -> bool {
        self.sprites.as_ref().is_some_and(|p| p.encode(encoder))
    }

    /// The frame's draws of the pool: one a bucket that may hold a live particle.
    pub(super) fn sprite_draws<'a>(
        &'a self,
        scene: &'a E::Scene,
    ) -> impl Iterator<Item = DrawRef> + 'a {
        let buckets = self.sprites.as_ref().map(|_| scene.sprites.live_buckets());
        buckets.into_iter().flatten().map(|(i, b)| DrawRef {
            first: 0,
            count: 0,
            base_vertex: 0,
            tex: b.texture.map_or(TexRef::White, |(t, _)| TexRef::Table(t)),
            sampler: b
                .texture
                .map_or(SamplerKey::PLAIN, |(_, s)| SamplerKey::of(&s)),
            blend: b.blend,
            cull: Cull::None,
            depth_test: true,
            depth_write: false,
            motion: 0,
            particle: true,
            sprites: Some(i as u8),
            late: b.late,
            shadow_only: false,
            view_only: false,
            cutout: false,
        })
    }

    /// The pipelines a pool draw of `key` is drawn with (its own; with the weighted blend an alpha one's tail, an added one's
    /// late entry), and its group of `tex` and `s`.
    pub(super) fn ensure_sprites(
        &mut self,
        key: PipelineKey,
        wboit: bool,
        tex: TexRef,
        s: SamplerKey,
    ) {
        if self.sprites.is_none() {
            return;
        }
        self.sampler(s);
        let mut keys = vec![key];
        if wboit && key.blend == Blend::Alpha && key.front_layer {
            keys.push(PipelineKey {
                stage: Stage::Tail,
                ..key
            });
        }
        if key.blend != Blend::Alpha {
            keys.push(PipelineKey {
                stage: Stage::Particle,
                ..key
            });
        }
        let view = match tex {
            TexRef::White => &self.white,
            TexRef::Empty => &self.empty,
            TexRef::Table(i) => self.textures.get(i).unwrap_or(&self.white),
        };
        let Some(pool) = &mut self.sprites else {
            return;
        };
        for k in keys {
            pool.ensure_pipeline(&self.device, self.pipeline_cache.as_ref(), k);
        }
        pool.ensure_group(&self.device, (tex, s), view, &self.samplers[&s]);
    }

    /// Draws bucket `bucket` of the pool with `key`'s pipeline and the group of `group`: the draws made (0 or 1).
    pub(super) fn draw_sprites(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        key: PipelineKey,
        group: (TexRef, SamplerKey),
        bucket: u8,
    ) -> usize {
        let Some(pool) = &self.sprites else {
            return 0;
        };
        let (Some(pipeline), Some(g)) = (pool.pipelines.get(&key), pool.sprite_groups.get(&group))
        else {
            return 0;
        };
        pass.set_pipeline(pipeline);
        pass.set_bind_group(1, g, &[]);
        pass.draw_indirect(&pool.args, u64::from(bucket) * 16);
        crate::count_draws(1);
        1
    }
}
