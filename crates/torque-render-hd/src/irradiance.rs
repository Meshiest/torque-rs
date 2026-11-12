//! The diffuse ambient's irradiance on the GPU (`irradiance.wgsl`): the environment cubes (the sky cube and the light capture)
//! projected on L2 spherical harmonics and convolved with the clamped cosine, as the ambient's radiance the forward pass reads
//! (group 0's binding 20: [`IrradiancePipe::output`]).
//!
//! A frame projects only the cube faces it drew (each face's sums stay in a buffer until the face is drawn again: the sky cube's
//! six when the scene's draws change, the capture's one in turn), then resolves the sums into two sets of coefficients: the
//! main view's (the capture's, where it bounces: [`IrradianceParams::bounce`]) and the secondary views': with the bounce the
//! sky the capture sees through the scene's gaps (a closed room takes no sky light for its bounce, and the capture's faces are
//! not lit by the capture, so a change of light settles once each face is drawn again), else the sky cube's. Each projects
//! [`LEVEL`] of the cubes' chains (64 px a face): the L2 harmonics keep nothing finer, and a capture texel that mixes the scene
//! and the sky is classed by its alpha (a distance: the scene) at a level fine enough that the mixed ones are few.

use torque_render_core::wgpu;
use torque_render_scene::hd::reflect::CAPTURE_SIZE;

use crate::push_f32;
use crate::shaders::Permutation;

/// The cubes' level projected (64 x 64 a face of the 256 px chains).
pub const LEVEL: u32 = 2;
/// The output: 18 vec4 (0-8 the main view's coefficients, 9-17 the secondary views'; rgb the ambient's radiance; the first's w
/// says the lower half is the sky as drawn). The secondary views are drawn before the projection: they read the last frame's.
pub const OUTPUT_BYTES: u64 = 18 * 16;
const PARAMS_BYTES: u64 = 5 * 16;
/// The chunks of 256 texels a face's level is projected in (one workgroup each).
const CHUNKS: u32 = 16;
/// Twelve faces' chunks' sums of 28 vec4 each.
const SUMS_BYTES: u64 = 12 * CHUNKS as u64 * 28 * 16;

/// How the sky's drawn radiance becomes light.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SkyLight {
    /// Times this scale (the physical sky's: `torque_render_scene::hd::sh::sky_light_scale`).
    Scaled(f32),
    /// Its luminance alone, scaled so the horizontal normals take this ambient on average (linear): the scene's ambient color,
    /// the sky's shape.
    Ambient([f32; 3]),
}

/// What a resolve makes of the faces' sums.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct IrradianceParams {
    pub sky: SkyLight,
    /// The main view takes the capture's sums (else the sky cube's).
    pub bounce: bool,
    /// The sky below the horizon is a ground of this albedo, lit by the sun and the sky above it (else the sky as it is drawn).
    pub ground: Option<[f32; 3]>,
    /// The sun's light (linear) on a face toward it, and towards the sun's z.
    pub sun: [f32; 3],
    pub sun_z: f32,
}

impl IrradianceParams {
    /// The uniform's bytes (`irradiance.wgsl`'s `Params`), projecting the faces of `faces` (bits 0-5 the sky cube's, 6-11 the
    /// capture's).
    fn bytes(&self, faces: u32) -> Vec<u8> {
        let mut b = Vec::with_capacity(PARAMS_BYTES as usize);
        let (scale, ambient) = match self.sky {
            SkyLight::Scaled(s) => (s.max(1e-9), [0.0; 3]),
            SkyLight::Ambient(a) => (0.0, a),
        };
        let flag = |on: bool| if on { 1.0 } else { 0.0 };
        push_f32(
            &mut b,
            &[scale, flag(self.bounce), flag(self.ground.is_some()), 0.0],
        );
        push_f32(&mut b, &[ambient[0], ambient[1], ambient[2], 0.0]);
        push_f32(&mut b, &[self.sun[0], self.sun[1], self.sun[2], self.sun_z]);
        let g = self.ground.unwrap_or([0.0; 3]);
        push_f32(&mut b, &[g[0], g[1], g[2], 0.0]);
        crate::push_u32(&mut b, &[faces, 0, 0, 0]);
        b
    }
}

/// The projection's pipelines, buffers and group (over the two cubes' [`LEVEL`] views, fixed for the renderer's life).
pub struct IrradiancePipe {
    faces: wgpu::ComputePipeline,
    resolve: wgpu::ComputePipeline,
    params: wgpu::Buffer,
    /// The faces' sums (kept between frames: a face is projected again only when it is drawn again).
    _sums: wgpu::Buffer,
    /// The coefficients the forward pass reads (uniform) and the resolve writes (storage).
    pub output: wgpu::Buffer,
    group: wgpu::BindGroup,
    /// The parameters of the last resolve: a frame that draws no face and keeps them encodes nothing.
    last: Option<IrradianceParams>,
    /// The faces projected so far (bits as [`IrradiancePipe::encode`]'s).
    projected: u32,
}

/// A cube's level as the 2D array the projection loads from.
pub fn level_view(texture: &wgpu::Texture, level: u32) -> wgpu::TextureView {
    texture.create_view(&wgpu::TextureViewDescriptor {
        label: Some("hd irradiance level"),
        dimension: Some(wgpu::TextureViewDimension::D2Array),
        base_mip_level: level,
        mip_level_count: Some(1),
        ..wgpu::TextureViewDescriptor::default()
    })
}

impl IrradiancePipe {
    /// Over `sky` and `capture`: one level of each cube as a 2D array of six layers ([`level_view`]), both of one size.
    pub fn new(
        device: &wgpu::Device,
        cache: Option<&wgpu::PipelineCache>,
        sky: &wgpu::TextureView,
        capture: &wgpu::TextureView,
    ) -> IrradiancePipe {
        let texture = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: false },
                view_dimension: wgpu::TextureViewDimension::D2Array,
                multisampled: false,
            },
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("hd irradiance"),
            entries: &[
                crate::uniform(0, wgpu::ShaderStages::COMPUTE),
                texture(1),
                texture(2),
                crate::storage(3, false, wgpu::ShaderStages::COMPUTE),
                crate::storage(4, false, wgpu::ShaderStages::COMPUTE),
            ],
        });
        let module = crate::module(device, Permutation::Irradiance);
        let faces = crate::compute_filter(
            device,
            cache,
            "hd irradiance faces",
            &layout,
            &module,
            "cs_faces",
        );
        let resolve = crate::compute_filter(
            device,
            cache,
            "hd irradiance resolve",
            &layout,
            &module,
            "cs_resolve",
        );
        let buffer = |label, size, usage| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size,
                usage,
                mapped_at_creation: false,
            })
        };
        let params = buffer(
            "hd irradiance params",
            PARAMS_BYTES,
            wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        );
        let sums = buffer(
            "hd irradiance sums",
            SUMS_BYTES,
            wgpu::BufferUsages::STORAGE,
        );
        let output = buffer(
            "hd irradiance",
            OUTPUT_BYTES,
            wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::UNIFORM
                | wgpu::BufferUsages::COPY_SRC
                | wgpu::BufferUsages::COPY_DST,
        );
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("hd irradiance"),
            layout: &layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: params.as_entire_binding(),
                },
                crate::tv(1, sky),
                crate::tv(2, capture),
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: sums.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: output.as_entire_binding(),
                },
            ],
        });
        IrradiancePipe {
            faces,
            resolve,
            params,
            _sums: sums,
            output,
            group,
            last: None,
            projected: 0,
        }
    }

    /// Forgets every projection and clears the coefficients, as a new pipe has them: the next frame projects every face again, and
    /// the secondary views drawn before it read a zero ambient, not the last picture's.
    pub fn reset(&mut self, queue: &wgpu::Queue) {
        self.last = None;
        self.projected = 0;
        queue.write_buffer(&self.output, 0, &[0; OUTPUT_BYTES as usize]);
    }

    /// Projects the faces `faces` names (bits 0-5 the sky cube's, 6-11 the capture's: the faces drawn before this in
    /// `encoder`), and those of the cubes the resolve reads that were never projected (the sky cube's always, the capture's with
    /// [`IrradianceParams::bounce`]: as they stand), then resolves the sums under `params`; nothing when no face is to be
    /// projected and the parameters are the last resolve's. A change of the sky's kind (the ground, the physical sky) projects
    /// every face again, whatever `faces` names: the kept sums class the sky below the horizon by the kind they were made under.
    pub fn encode(
        &mut self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        params: &IrradianceParams,
        faces: u32,
    ) {
        // The sums class the sky below the horizon by the ground: a change of the sky's kind (the physical sky drawn or not)
        // projects every face again.
        let kind =
            |p: &IrradianceParams| (p.ground.is_some(), matches!(p.sky, SkyLight::Scaled(_)));
        if self.last.as_ref().is_some_and(|l| kind(l) != kind(params)) {
            self.projected = 0;
        }
        let read = if params.bounce { 0xfff } else { 0x3f };
        let faces = (faces | !self.projected) & read;
        if faces == 0 && self.last.as_ref() == Some(params) {
            return;
        }
        self.projected |= faces;
        queue.write_buffer(&self.params, 0, &params.bytes(faces));
        self.last = Some(*params);
        crate::count_pass();
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("hd irradiance"),
            timestamp_writes: None,
        });
        pass.set_bind_group(0, &self.group, &[]);
        if faces != 0 {
            pass.set_pipeline(&self.faces);
            pass.dispatch_workgroups(6, if faces >> 6 != 0 { 2 } else { 1 }, CHUNKS);
        }
        pass.set_pipeline(&self.resolve);
        pass.dispatch_workgroups(1, 1, 1);
    }
}

/// The size of the cubes' level projected.
pub const LEVEL_SIZE: u32 = CAPTURE_SIZE >> LEVEL;
// A face's level fits its chunks (`irradiance.wgsl`'s CHUNKS of 256 texels).
const _: () = assert!(LEVEL_SIZE * LEVEL_SIZE <= CHUNKS * 256);

#[cfg(test)]
#[path = "irradiance_tests.rs"]
mod tests;
