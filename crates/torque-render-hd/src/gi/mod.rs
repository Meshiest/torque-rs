//! Global illumination at High and Ultra ([`Gi::enabled`], off by default): voxel-traced irradiance probes, in the manner of
//! Godot's SDFGI and HDDAGI and of DDGI with its rays marched through voxels instead of hardware rays.
//!
//! - **The voxel scene** (`clipmap.rs`, `voxels.rs`, `gi_voxel.wgsl`): levels of 128^3 voxels about the eye, the finest a
//!   quarter unit (a wall half a unit thick is two voxels), each level's voxels twice the last's, kept toroidally in tiles of
//!   8^3 that follow the eye a whole tile at a time. A tile is made on the GPU from the static triangles near it (`sources.rs`:
//!   the cells' opaque batches and the opaque planes; the moving shapes cast nothing): each voxel a triangle overlaps keeps the
//!   albedo, normal and emission of the one whose plane passes nearest its middle among those it lies on or in front of, and
//!   where that plane lies in it (a surface on a voxel boundary fills both sides; rays and light stop at the plane, not at the
//!   voxel's face). A ray goes on through a voxel whose plane crosses its inside while it stays in front of the plane (a door's
//!   jambs part-way into their voxels keep the opening's width), and through one whose plane lies on its side unless a triangle
//!   meets that voxel from well behind (where blocks meet: their hidden faces would let rays through the seams). A frame makes at most a
//!   budget of tiles with triangles, the nearest the eye first ([`GiTier::tile_budget`]), and clears the empty ones.
//! - **The voxels' light** (`light.rs`, `gi_light.wgsl`): each filled voxel's radiance is its albedo times the light on a face
//!   of its normal at its surface (the sun, the point, spot and area lights, with the forward pass's own `Light`, `cone_share`,
//!   `falloff` and `area.wgsl`, each shadowed by a march through the voxels) plus its emission (a glowing surface's voxels
//!   emit; its light is left out). A tile is lit the frame it is made, when a light in reach changes or the sun changes (under
//!   [`GiTier::relight_budget`]), and in turn (every tile within [`GiTier::relight_rounds`] frames). Its ambient is the probes'
//!   (the bounce: one more each frame, kept by the probes' hysteresis).
//! - **The probes** (`probes.rs`, `gi_probe.wgsl`, `gi_grid.wgsl`): cascades of 24 x 24 x 12 probes about the eye
//!   ([`GiTier::cascades`], spaced half a unit, 1, 2 and 4), kept toroidally, each half a spacing off its grid point (between
//!   the walls on the cascade's grid). A probe traced in a frame (a new or young one, every one in turn every
//!   [`GiTier::period`] frames, every one with [`Gi::converge`]) casts [`GiTier::rays`] rays of a spherical Fibonacci set turned by
//!   a rotation hashed from the frame number, marched through the voxels from the finest level (every cascade sees thin walls
//!   as thin): a front face gives its lit
//!   radiance, a back face (or a start inside a voxel) nothing and a short negative depth, a ray past the clipmap the sky's
//!   irradiance (`gi_sky.wgsl`). Its irradiance (6 x 6 octahedral texels) and depth moments (8 x 8) blend with a hysteresis of
//!   0.9 (0.5 after a change of a quarter); a probe behind its voxel's surface is switched off (its neighbors stand for it), one whose rays find
//!   mostly back faces moves out past the nearest and is switched off past half.
//! - **The shading** (`gi_lookup.wgsl`, `gi_forward.wgsl`): the forward pass's ambient (`shade_surf`'s `ambient_linear`) goes
//!   through the probes in the global illumination's own forward modules ([`crate::shaders::Permutation::GiForward`]: the one
//!   line `shaders::GI_HOOK` names, changed when the module is composed; the plain modules are as they were). The point is
//!   looked up a little off its surface and toward the eye; eight probes a cascade weighted by distance, by the normal (a probe
//!   behind the surface counts next to nothing) and by Chebyshev's bound on their depth moments; a cascade's outer two probes
//!   fade to the next, the last to the sky's irradiance. The probes take the light capture's bounce out of the sky's
//!   irradiance where they are drawn (the capture stays for the reflections). Moving shapes receive it and cast none.
//!
//! # Maintaining
//!
//! Module map: `mod.rs` the tiers and the frame's steps (`Gi::prepare`, `Gi::encode_voxels`, `Gi::encode_light`,
//! `Gi::encode_probes`) and the bindings the forward pass takes (`Gi::frame_bindings`: an empty state where there are no probes);
//! `sources.rs` the triangles, their bins and the triangles that changed; `clipmap.rs` the levels' places and the tiles waiting;
//! `voxels.rs` the GPU buffers and the voxel pass; `light.rs` which tiles a frame lights and the light pass; `probes.rs` the
//! probes' buffers, atlas and pass; `query.rs` the probes' ambient at given points (the checks'); `debug.rs` the voxel picture
//! ([`GiView`]); `tribox.rs` the CPU's exact triangle-box test. `HdWorld` binds the probes at its frame group's bindings 22 to
//! 24 and swaps its forward modules and pipelines for the global illumination's when a frame draws it (`set_gi_shading`).
//!
//! Flow of a frame (`HdWorld::draw`, after the irradiance and before the main view): the sources are brought to the scene's
//! cells and planes (a changed triangle marks its box), the levels move to the eye (the tiles they take in wait), the waiting
//! tiles nearest the eye are made, then the tiles due are lit, then the probes due are traced; the main view reads them.
//!
//! Rules, each held by a test in `tests.rs` or `light_tests.rs`:
//! - The voxels do not depend on the order the GPU ran in: a fixed set of cells made twice gives byte-identical buffers
//!   (`voxelizing_twice_gives_the_same_bytes`); each voxel's triangle is the one of the highest integer key.
//! - No voxel the CPU's exact triangle-box test fills is left empty (`no_voxel_the_cpu_fills_is_missed`, six meshes: boxes
//!   square and turned, slivers, a sphere, a slanted plane, tiny triangles).
//! - A tile slot read for a tile it does not hold is empty (its tag names the tile it holds): a level that moved never shows
//!   the far side's voxels.
//! - The voxels' light is the forward pass's diffuse light: each kind on a lit plane within 3 % of the CPU's reference
//!   (`each_light_kind_lights_the_voxels_as_the_forward_pass`; an area light's summed over its shape). The light pass takes the
//!   forward pass's light functions from its files (`shaders::excerpt`): a change there reaches both.
//! - The WGSL constants are the Rust ones (`the_shader_constants_are_the_clipmaps`, `the_probe_constants_are_the_grids`).
//! - The plain forward modules hold none of the global illumination and its own change one line
//!   (`the_forward_pass_changes_one_line`): with it off every picture is as it was.
//! - Measured by `gi-room` (not unit tests: they take the scenes and minutes): the probes against the path-traced reference
//!   on the two rooms' floors and walls seen from the door (`--compare`: each region's mean and the worst region; the
//!   reference shadows an area light by its center as the forward pass does), the leak through a shut door (room B under 2 %
//!   of room A), two runs' pictures the same (`--repeat`).
//!
//! Extension points: a new source kind is a `SourceKey` and a branch of `Sources::sync`; a light kind is a branch of
//! `voxel_light` in `gi_light.wgsl` (with the forward pass's function); a cost budget is a [`GiTier`] field.
//!
//! Checks: `cargo test -p torque-render-hd gi`; `gi-room --compare`, `--repeat` and `--shots --gi`; the passes' GPU times are the
//! `gi voxels`, `gi light` and `gi probes` marks of `gi-room --blocks 1000` and `level-profile --scene office --gi` (under the
//! bench lock).

pub(crate) mod clipmap;
mod debug;
pub(crate) mod light;
pub(crate) mod probes;
mod query;
pub(crate) mod sources;
#[cfg(test)]
mod tribox;
pub(crate) mod voxels;

use torque_render_core::wgpu;
use torque_render_scene::hd::cells::StaticGeometry;
use torque_render_scene::hd::{HdCamera, HdScene, Quality};

pub use debug::GiView;

use clipmap::Clipmap;
pub(crate) use light::GiLights;
use light::{Bounce, LightGpu, Relight};
use probes::ProbeGpu;
use sources::Sources;
use voxels::VoxelGpu;

/// A picture in linear light: its width, height and pixels (top row first).
pub type GiImage = (u32, u32, Vec<[f32; 4]>);

/// What a quality's global illumination costs: its voxel levels, the tiles with triangles a frame makes at most, the tiles a
/// change of light lights again a frame at most (past the ones made), the frames in which every tile is lit in turn, its probe
/// cascades, each probe's rays and the frames between a probe's turns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GiTier {
    pub levels: u32,
    pub tile_budget: usize,
    pub relight_budget: usize,
    pub relight_rounds: u32,
    pub cascades: u32,
    pub rays: u32,
    pub period: u32,
}

impl GiTier {
    pub const HIGH: GiTier = GiTier {
        levels: 3,
        tile_budget: 48,
        relight_budget: 128,
        relight_rounds: 32,
        cascades: 3,
        rays: 64,
        period: 8,
    };
    pub const ULTRA: GiTier = GiTier {
        levels: 4,
        tile_budget: 64,
        relight_budget: 192,
        relight_rounds: 32,
        cascades: 4,
        rays: 128,
        period: 4,
    };

    /// The tier of a quality (None: no global illumination there).
    pub fn of(quality: Quality) -> Option<GiTier> {
        match quality {
            Quality::High => Some(GiTier::HIGH),
            Quality::Ultra => Some(GiTier::ULTRA),
            Quality::Low | Quality::Medium => None,
        }
    }
}

/// What a frame gives the global illumination: the scene, the main view's eye, the quality's tier (None: none) and the
/// lights.
pub(crate) struct GiFrame<'a, G> {
    pub(crate) scene: &'a HdScene<G>,
    pub(crate) eye: [f32; 3],
    pub(crate) tier: Option<GiTier>,
    pub(crate) lights: &'a GiLights<'a>,
}

/// The global illumination's state: its sources, the clipmap's places and its GPU buffers (made with the first frame that
/// draws it, let go when a frame does not).
pub struct Gi {
    tier: Option<GiTier>,
    sources: Sources,
    clipmap: Clipmap,
    voxels: Option<VoxelGpu>,
    light: Option<LightGpu>,
    relight: Relight,
    probes: Option<ProbeGpu>,
    /// What the forward pass and the light pass bind where there are no probes: an empty state (no cascades), a texel of
    /// atlas, and the atlas's sampler.
    empty_state: wgpu::Buffer,
    empty_atlas: wgpu::TextureView,
    sampler: wgpu::Sampler,
    debug: Option<debug::DebugPipe>,
    query: Option<query::QueryPipe>,
    frame: u32,
    /// Drawn where the quality has it (High and Ultra); off by default: the frame draws as before.
    pub enabled: bool,
    /// Every waiting tile made each frame, whatever the budget (the checks and the references; off by default).
    pub converge: bool,
    /// The tiles the last frame made (with triangles, and empty) and lit.
    pub made: (usize, usize),
    pub lit: usize,
}

impl Gi {
    pub(crate) fn new(device: &wgpu::Device) -> Gi {
        Gi {
            tier: None,
            sources: Sources::new(device),
            clipmap: Clipmap::new(0),
            voxels: None,
            light: None,
            relight: Relight::default(),
            probes: None,
            empty_state: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("hd gi no probes"),
                size: probes::HEADER_BYTES + 16,
                usage: wgpu::BufferUsages::STORAGE,
                mapped_at_creation: false,
            }),
            empty_atlas: device
                .create_texture(&wgpu::TextureDescriptor {
                    label: Some("hd gi no atlas"),
                    size: wgpu::Extent3d {
                        width: 1,
                        height: 1,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: probes::ATLAS_FORMAT,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                })
                .create_view(&wgpu::TextureViewDescriptor::default()),
            sampler: device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some("hd gi atlas"),
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                ..wgpu::SamplerDescriptor::default()
            }),
            debug: None,
            query: None,
            frame: 0,
            enabled: false,
            converge: false,
            made: (0, 0),
            lit: 0,
        }
    }

    /// Forgets the scene: every source and tile is made again from the next frame's (a new scene whose cells may share keys
    /// and generations with the last's).
    pub fn reset(&mut self) {
        self.sources.clear();
        self.clipmap.reset();
        self.relight.reset();
    }

    /// The tier the last frame drew with (None: none).
    pub fn tier(&self) -> Option<GiTier> {
        self.tier
    }

    /// The tiles still waiting to be made.
    pub fn pending(&self) -> usize {
        self.clipmap.pending()
    }

    /// The static triangles the voxels are made from.
    pub fn triangles(&self) -> usize {
        self.sources.triangles()
    }

    /// The GPU bytes the voxels and their triangles take.
    pub fn gpu_bytes(&self) -> u64 {
        self.voxels.as_ref().map_or(0, VoxelGpu::bytes)
            + self.probes.as_ref().map_or(0, ProbeGpu::bytes)
            + self.sources.buffer().size()
    }

    /// The frame's steps on the CPU: the sources brought to the frame's scene, the levels moved to its eye, the frame's tiles
    /// to make and to light written. False (and the GPU buffers let go) when its tier is None or the device holds no level
    /// of voxels (a tier is cut to the levels the device's limits hold, its cascades to its levels).
    pub(crate) fn prepare<G: StaticGeometry>(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        cache: Option<&wgpu::PipelineCache>,
        frame: GiFrame<'_, G>,
    ) -> bool {
        let GiFrame {
            scene,
            eye,
            tier,
            lights,
        } = frame;
        let tier = tier.and_then(|t| {
            let levels = voxels::levels_that_fit(device, t.levels);
            (levels > 0).then_some(GiTier {
                levels,
                cascades: t.cascades.min(levels),
                ..t
            })
        });
        let Some(tier) = tier else {
            if self.tier.take().is_some() {
                self.voxels = None;
                self.light = None;
                self.probes = None;
                self.clipmap = Clipmap::new(0);
                self.relight.reset();
            }
            return false;
        };
        if self
            .tier
            .is_none_or(|t| t.levels != tier.levels || t.cascades != tier.cascades)
            || self.voxels.is_none()
        {
            self.voxels = Some(VoxelGpu::new(device, cache, tier.levels));
            self.probes = Some(ProbeGpu::new(device, cache, tier.cascades));
            self.clipmap = Clipmap::new(tier.levels);
            self.relight.reset();
        }
        let light = self
            .light
            .get_or_insert_with(|| LightGpu::new(device, cache));
        self.tier = Some(tier);
        self.frame = self.frame.wrapping_add(1);
        self.sources.sync(
            device,
            queue,
            scene.cells.cells(),
            scene.cells.slots(),
            &scene.planes,
            scene.planes_generation,
            &scene.textures,
        );
        for b in self.sources.take_dirty() {
            self.clipmap.mark(&b);
        }
        self.clipmap.place(eye);
        let budget = (!self.converge).then_some(tier.tile_budget);
        let sources = &self.sources;
        let jobs = self.clipmap.take(eye, budget, |b| sources.triangles_in(b));
        let made: Vec<clipmap::TileKey> = jobs
            .iter()
            .filter(|j| !j.1.is_empty())
            .map(|j| j.0)
            .collect();
        self.made = (made.len(), jobs.len() - made.len());
        let lit = self
            .relight
            .take(&self.clipmap, &made, lights, eye, tier, self.converge);
        self.lit = lit.len();
        light.write(device, queue, &lit, lights, true);
        if let Some(p) = self.probes.as_mut() {
            p.write(
                queue,
                eye,
                probes::Trace {
                    rays: tier.rays,
                    period: tier.period,
                    frame: self.frame,
                    all: self.converge,
                    cascades: tier.cascades,
                },
            );
        }
        let Some(voxels) = self.voxels.as_mut() else {
            return false;
        };
        voxels.write_jobs(device, queue, &jobs);
        queue.write_buffer(
            &voxels.volume,
            0,
            &voxels::volume_bytes(
                &self.clipmap.origins,
                self.clipmap.levels,
                self.frame,
                jobs.len() as u32,
            ),
        );
        true
    }

    /// The voxel pass: the frame's tiles made.
    pub(crate) fn encode_voxels(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
    ) {
        if let Some(v) = self.voxels.as_mut() {
            v.encode(
                device,
                encoder,
                self.sources.buffer(),
                self.sources.buffer_generation,
            );
        }
    }

    /// The light pass: the frame's tiles lit (with the forward pass's `lights` buffer and the irradiance's coefficients `sky`).
    pub(crate) fn encode_light(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        lights: &wgpu::Buffer,
        sky: &wgpu::Buffer,
    ) {
        let (state, atlas) = match &self.probes {
            Some(p) => (&p.state, &p.atlas_view),
            None => (&self.empty_state, &self.empty_atlas),
        };
        let bounce = Bounce {
            state,
            atlas,
            sampler: &self.sampler,
            sky,
        };
        if let (Some(l), Some(v)) = (self.light.as_mut(), self.voxels.as_ref()) {
            l.encode(device, encoder, v, lights, &bounce);
        }
    }

    /// The probe pass: the frame's probes traced and blended (`sky`: the irradiance's coefficients, for the rays that leave the
    /// voxels).
    pub(crate) fn encode_probes(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        sky: &wgpu::Buffer,
    ) {
        if let (Some(p), Some(v)) = (self.probes.as_mut(), self.voxels.as_ref()) {
            p.encode(device, encoder, v, sky);
        }
    }

    /// What the forward pass binds of the global illumination (its frame group's bindings 22 to 24): the probes' atlas and
    /// state, and the atlas's sampler (an empty state where there are no probes: no cascade reaches anything).
    pub(crate) fn frame_bindings(&self) -> (&wgpu::TextureView, &wgpu::Buffer, &wgpu::Sampler) {
        match &self.probes {
            Some(p) => (&p.atlas_view, &p.state, &self.sampler),
            None => (&self.empty_atlas, &self.empty_state, &self.sampler),
        }
    }

    /// A picture of the voxels from `camera` (linear light; None before a frame drew them).
    pub(crate) fn debug_image(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        camera: &HdCamera,
        view: GiView,
    ) -> Result<Option<GiImage>, String> {
        let Some(voxels) = &self.voxels else {
            return Ok(None);
        };
        let pipe = self
            .debug
            .get_or_insert_with(|| debug::DebugPipe::new(device));
        pipe.draw(device, queue, voxels, camera, view).map(Some)
    }

    /// The ambient's radiance (linear) the probes give each point facing its normal as the last frame left them, black past
    /// them (the checks' comparison with a reference).
    pub(crate) fn query(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        points: &[([f32; 3], [f32; 3])],
    ) -> Result<Vec<[f32; 3]>, String> {
        let pipe = self
            .query
            .get_or_insert_with(|| query::QueryPipe::new(device));
        let bindings = match &self.probes {
            Some(p) => (&p.atlas_view, &p.state, &self.sampler),
            None => (&self.empty_atlas, &self.empty_state, &self.sampler),
        };
        pipe.run(device, queue, bindings, points)
    }

    /// The voxel buffers' bytes (tile infos, occupancy words, voxels) as they stand (the checks').
    #[cfg(test)]
    pub(crate) fn read_voxels(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
    ) -> Result<[Vec<u8>; 3], String> {
        let v = self.voxels.as_ref().ok_or("no voxels")?;
        Ok([
            read_buffer(device, queue, &v.tiles, v.tiles.size())?,
            read_buffer(device, queue, &v.occ, v.occ.size())?,
            read_buffer(device, queue, &v.voxels, v.voxels.size())?,
        ])
    }

    #[cfg(test)]
    pub(crate) fn clipmap(&self) -> &Clipmap {
        &self.clipmap
    }

    #[cfg(test)]
    pub(crate) fn sources(&self) -> &Sources {
        &self.sources
    }
}

/// `size` bytes of `buffer` (one that may be copied from), read back.
pub(crate) fn read_buffer(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    buffer: &wgpu::Buffer,
    size: u64,
) -> Result<Vec<u8>, String> {
    let staging = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("hd gi readback"),
        size,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    encoder.copy_buffer_to_buffer(buffer, 0, &staging, 0, size);
    queue.submit([encoder.finish()]);
    let slice = staging.slice(..);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |r| {
        let _ = tx.send(r);
    });
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .map_err(|e| e.to_string())?;
    rx.recv()
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())?;
    let data = slice
        .get_mapped_range()
        .map_err(|e| e.to_string())?
        .to_vec();
    staging.unmap();
    Ok(data)
}

#[cfg(test)]
mod light_tests;
#[cfg(test)]
mod tests;
