//! The upscaler's seam: what brings High's and Ultra's internal frame up to the output's size when the upscaling setting
//! ([`torque_render_scene::hd::post::Upscaling`]) asks for one. [`Upscale`] is the method, [`UpscaleInputs`] what a frame
//! hands it; [`TemporalUpscaler`] is the engine's own. With the setting off no upscaler is made and the frame resolves through
//! the TAA as it always has.

use torque_render_core::wgpu;
use torque_render_scene::hd::post::Upscaler;

use crate::post::{MOTION_FORMAT, SHOWN_FORMAT, depth, filtering, frame_uniform, module, texture};
use crate::shaders::Permutation;
use crate::{colour, full_frame, tv};

/// The frames the temporal upscaler's history holds from which Ultra's sharpen is at its full strength: a moving pixel's whole
/// history, so only one that has just started (a disocclusion: this frame's sample alone) is sharpened less.
pub const SHARPEN_FULL_FRAMES: f32 = 5.0;
/// The temporal upscaler's moments: the sum of each pixel's luma over the frames it has been still, up to the jitter's period
/// (in two half floats), those frames, and its 3 x 3 neighborhood's mean luma (the next frame's change test).
pub const MOMENTS_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
/// The temporal upscaler's ring of the last frames' luma (a layer a frame, [`RING_FRAMES`] of them): the sample of 8 frames
/// ago keeps a still pixel's running mean over the jitter's period exact, and against this frame's sample of the same phase it
/// is the light's change at the pixel, free of the jitter's noise.
pub const RING_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::R16Float;
/// The ring's layers: the jitter's 8 phases and one more, so the layer written is never the one read.
pub const RING_FRAMES: u32 = 9;
/// The frame index's period in the frame uniform (`temporal.z`) while an upscaler runs: exact in a float and a multiple of the
/// jitter's phases and the ring's layers, so both keep their place across the wrap.
pub const UPSCALED_FRAME_PERIOD: u64 = 1024 * RING_FRAMES as u64;

/// What an upscaler reads and writes in a frame. The first fields are what FSR 2/3 and DLSS take; the rest is what the engine
/// has beside them, for a method that reads it (the temporal upscaler does). Every texture of the internal size is drawn with
/// this frame's jitter.
pub struct UpscaleInputs<'a> {
    /// This frame's colour at the internal size: linear HDR before the exposure ([`crate::HDR_FORMAT`]).
    pub color: &'a wgpu::TextureView,
    /// The prepass's depth at the internal size ([`crate::DEPTH_FORMAT`], 1 near and 0 far).
    pub depth: &'a wgpu::TextureView,
    /// The prepass's motion at the internal size ([`MOTION_FORMAT`]): xy a pixel's motion in uv (this frame's place less
    /// the last's), z its eye distance last frame, w 1 plus its object's id (0 where nothing was drawn).
    pub motion: &'a wgpu::TextureView,
    /// This frame's sub-pixel jitter in internal pixels (x right, y down; `torque_render_scene::hd::post::jitter`).
    pub jitter: [f32; 2],
    /// The exposure's state (`exposure.wgsl`'s `ExposureState`, a storage buffer): the scale the tonemap applies after.
    pub exposure: &'a wgpu::Buffer,
    /// How much each pixel's history must give way to this frame (internal size, 0 to 1 in r): None, the renderer marks no
    /// reactive surfaces yet.
    pub reactive: Option<&'a wgpu::TextureView>,
    /// The history starts again this frame (the first frame, a cut, new targets): the frame uniform's `temporal.w` is 0.
    pub reset: bool,
    pub internal_size: [u32; 2],
    pub output_size: [u32; 2],
    /// This frame's output at the output's size ([`SHOWN_FORMAT`]): the tonemap's source. Its alpha is the frames the history
    /// holds at the pixel (Ultra's sharpen grows with it up to [`SHARPEN_FULL_FRAMES`]; a method that keeps no count writes
    /// at least that).
    pub output: &'a wgpu::TextureView,
    /// Last frame's output, which the method may keep as its history.
    pub output_before: &'a wgpu::TextureView,
    /// Last frame's prepass depth and motion (internal size).
    pub depth_before: &'a wgpu::TextureView,
    pub motion_before: &'a wgpu::TextureView,
    /// The sun's visibility mask of this frame and the last (internal size; a change is a shadow's edge passing).
    pub sun_mask: &'a wgpu::TextureView,
    pub sun_mask_before: &'a wgpu::TextureView,
    /// The frame uniform (`common.wgsl`'s `Frame`: the cameras, the jitter, the frame's index, whether the history is valid).
    pub frame: &'a wgpu::Buffer,
    /// Frames since the history started (the jitter's phase is it modulo 8).
    pub frame_index: u64,
    /// Which of two copies of a history is written this frame (the other is last frame's).
    pub parity: usize,
    /// Where the pass writes its GPU time (the `anti-aliasing` mark).
    pub timestamps: Option<wgpu::RenderPassTimestampWrites<'a>>,
}

/// An upscaling method. A new one is a new [`Upscaler`] setting, its own type implementing this, and an arm in [`make`].
pub trait Upscale: wgpu::WasmNotSend + 'static {
    /// Encodes the frame's upscale into `encoder`: reads `inputs`, writes `inputs.output` whole.
    fn encode(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        inputs: UpscaleInputs<'_>,
    );

    /// The history the frame of `parity` wrote when it is not that frame's output, for the checks' readbacks.
    fn history(&self, _parity: usize) -> Option<&wgpu::Texture> {
        None
    }
}

/// The method `kind` asks for (None: no upscaler, [`Upscaler::Off`]).
pub fn make(
    kind: Upscaler,
    device: &wgpu::Device,
    cache: Option<&wgpu::PipelineCache>,
    precision: HistoryPrecision,
) -> Option<Box<dyn Upscale>> {
    match kind {
        Upscaler::Off => None,
        Upscaler::Temporal => Some(Box::new(TemporalUpscaler::new(device, cache, precision))),
    }
}

/// The precision of the temporal upscaler's history: half floats (the renderer's), or 32-bit ones (the reference a check
/// compares the half floats' running mean with).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum HistoryPrecision {
    #[default]
    Half,
    Full,
}

impl HistoryPrecision {
    pub const ALL: [HistoryPrecision; 2] = [HistoryPrecision::Half, HistoryPrecision::Full];
}

/// The views a parity's groups bind, other than the upscaler's own: a change makes the groups again.
#[derive(PartialEq)]
struct GroupKey {
    views: [wgpu::TextureView; 8],
    frame: wgpu::Buffer,
}

/// The upscaler's own targets at the output's size.
struct Own {
    size: [u32; 2],
    /// The 32-bit history (with [`HistoryPrecision::Full`]; a half-float history is the output itself).
    history: Option<[wgpu::Texture; 2]>,
    history_views: Option<[wgpu::TextureView; 2]>,
    moments_views: [wgpu::TextureView; 2],
    ring_views: Vec<wgpu::TextureView>,
}

/// The engine's temporal upscaler (`tsr_gather.wgsl`, `tsr.wgsl`): the TAA's reprojection and tests with a history that holds
/// longer where the image flickers, a still pixel's luma the exact mean of its own samples over the jitter's period, and the
/// light's change from the ring's sample of the same phase. One pass writes the output (with a half-float history, the history
/// itself), the moments and the ring's layer.
pub struct TemporalUpscaler {
    precision: HistoryPrecision,
    layout: wgpu::BindGroupLayout,
    pipeline: wgpu::RenderPipeline,
    linear: wgpu::Sampler,
    own: Option<Own>,
    /// Each parity's groups (one a ring slot: the layer written and the one of 8 frames ago), and what they bind.
    groups: [Option<(GroupKey, Vec<wgpu::BindGroup>)>; 2],
}

impl TemporalUpscaler {
    pub fn new(
        device: &wgpu::Device,
        cache: Option<&wgpu::PipelineCache>,
        precision: HistoryPrecision,
    ) -> TemporalUpscaler {
        let half = precision == HistoryPrecision::Half;
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("hd tsr"),
            entries: &[
                frame_uniform(0),
                texture(1, false),
                depth(2),
                depth(3),
                texture(4, false),
                texture(5, false),
                // A half-float history is read through the sampler; a 32-bit one with loads.
                texture(6, half),
                filtering(8),
                texture(9, true),
                texture(10, true),
                texture(11, false),
                texture(12, false),
            ],
        });
        let m = module(device, Permutation::Tsr(precision));
        let pipeline = full_frame(
            device,
            cache,
            "hd tsr",
            &layout,
            &m,
            "fs_taa",
            &Self::targets(precision),
        );
        let linear = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("hd tsr linear clamp"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..wgpu::SamplerDescriptor::default()
        });
        TemporalUpscaler {
            precision,
            layout,
            pipeline,
            linear,
            own: None,
            groups: [None, None],
        }
    }

    /// The pass's targets in order: the 32-bit history when kept apart, the output, the moments and the ring's layer.
    fn targets(precision: HistoryPrecision) -> Vec<wgpu::TextureFormat> {
        let mut t = Vec::with_capacity(4);
        if precision == HistoryPrecision::Full {
            t.push(wgpu::TextureFormat::Rgba32Float);
        }
        t.extend([SHOWN_FORMAT, MOMENTS_FORMAT, RING_FORMAT]);
        t
    }

    /// The upscaler's own targets at `size` (made again, and the groups with them, when the output's size changes).
    fn own(&mut self, device: &wgpu::Device, size: [u32; 2]) -> &Own {
        if self.own.as_ref().is_none_or(|o| o.size != size) {
            self.groups = [None, None];
            self.own = Some(self.make_own(device, size));
        }
        self.own.as_ref().expect("made above")
    }

    fn make_own(&self, device: &wgpu::Device, size: [u32; 2]) -> Own {
        let usage = wgpu::TextureUsages::RENDER_ATTACHMENT
            | wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::COPY_SRC;
        let tex = |label: &str, format, layers| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d {
                    width: size[0],
                    height: size[1],
                    depth_or_array_layers: layers,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage,
                view_formats: &[],
            })
        };
        let view = |t: &wgpu::Texture| t.create_view(&wgpu::TextureViewDescriptor::default());
        let history = (self.precision == HistoryPrecision::Full)
            .then(|| [0, 1].map(|_| tex("hd tsr history", wgpu::TextureFormat::Rgba32Float, 1)));
        let history_views = history.as_ref().map(|h| [view(&h[0]), view(&h[1])]);
        let moments_views = [0, 1].map(|_| view(&tex("hd tsr moments", MOMENTS_FORMAT, 1)));
        let ring = tex("hd tsr ring", RING_FORMAT, RING_FRAMES);
        let ring_views = (0..RING_FRAMES)
            .map(|layer| {
                ring.create_view(&wgpu::TextureViewDescriptor {
                    dimension: Some(wgpu::TextureViewDimension::D2),
                    base_array_layer: layer,
                    array_layer_count: Some(1),
                    ..wgpu::TextureViewDescriptor::default()
                })
            })
            .collect();
        Own {
            size,
            history,
            history_views,
            moments_views,
            ring_views,
        }
    }

    /// The groups of the frame's parity, made again when what they bind changed.
    fn groups(&mut self, device: &wgpu::Device, inputs: &UpscaleInputs<'_>) -> &[wgpu::BindGroup] {
        let p = inputs.parity;
        let key = GroupKey {
            views: [
                inputs.color,
                inputs.depth,
                inputs.depth_before,
                inputs.motion,
                inputs.motion_before,
                inputs.output_before,
                inputs.sun_mask,
                inputs.sun_mask_before,
            ]
            .map(Clone::clone),
            frame: inputs.frame.clone(),
        };
        if self.groups[p].as_ref().is_none_or(|(k, _)| *k != key) {
            let own = self.own.as_ref().expect("made before the groups");
            let history = own
                .history_views
                .as_ref()
                .map_or(inputs.output_before, |h| &h[1 - p]);
            let slots = RING_FRAMES as usize;
            let made = (0..slots)
                .map(|slot| {
                    device.create_bind_group(&wgpu::BindGroupDescriptor {
                        label: Some("hd tsr"),
                        layout: &self.layout,
                        entries: &[
                            wgpu::BindGroupEntry {
                                binding: 0,
                                resource: inputs.frame.as_entire_binding(),
                            },
                            tv(1, inputs.color),
                            tv(2, inputs.depth),
                            tv(3, inputs.depth_before),
                            tv(4, inputs.motion),
                            tv(5, inputs.motion_before),
                            tv(6, history),
                            wgpu::BindGroupEntry {
                                binding: 8,
                                resource: wgpu::BindingResource::Sampler(&self.linear),
                            },
                            tv(9, inputs.sun_mask),
                            tv(10, inputs.sun_mask_before),
                            tv(11, &own.moments_views[1 - p]),
                            tv(12, &own.ring_views[(slot + 1) % slots]),
                        ],
                    })
                })
                .collect();
            self.groups[p] = Some((key, made));
        }
        &self.groups[p].as_ref().expect("made above").1
    }
}

impl Upscale for TemporalUpscaler {
    fn encode(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        inputs: UpscaleInputs<'_>,
    ) {
        debug_assert_eq!(inputs.output.texture().format(), SHOWN_FORMAT);
        debug_assert_eq!(inputs.motion.texture().format(), MOTION_FORMAT);
        self.own(device, inputs.output_size);
        let slot = (inputs.frame_index % u64::from(RING_FRAMES)) as usize;
        let group = self.groups(device, &inputs)[slot].clone();
        let own = self.own.as_ref().expect("made above");
        let p = inputs.parity;
        let mut attachments = Vec::with_capacity(4);
        if let Some(h) = &own.history_views {
            attachments.push(Some(colour(&h[p])));
        }
        attachments.extend([
            Some(colour(inputs.output)),
            Some(colour(&own.moments_views[p])),
            Some(colour(&own.ring_views[slot])),
        ]);
        crate::count_pass();
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("hd tsr"),
            color_attachments: &attachments,
            depth_stencil_attachment: None,
            timestamp_writes: inputs.timestamps,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &group, &[]);
        pass.draw(0..3, 0..1);
        crate::count_draws(1);
    }

    fn history(&self, parity: usize) -> Option<&wgpu::Texture> {
        self.own.as_ref()?.history.as_ref().map(|h| &h[parity])
    }
}

#[cfg(test)]
#[path = "upscale_tests.rs"]
mod tests;
