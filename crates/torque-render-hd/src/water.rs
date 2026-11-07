//! The water on the GPU: the planar reflection's target (the opaque scene drawn by the camera mirrored
//! in the water plane, cut at the plane, at half the internal frame's size at High and at its size at Ultra) and the water's
//! pipelines (`water.wgsl`'s `fs_water`: the plane drawn over the opaque frame's copy, not blended).

use torque_render_core::wgpu;
pub use torque_render_scene::hd::WaterZone;
use torque_render_scene::hd::{HdCamera, HdScene};

/// The planar reflection's colour and depth, and its view's frame uniform.
pub struct Planar {
    pub width: u32,
    pub height: u32,
    pub color: wgpu::Texture,
    pub view: wgpu::TextureView,
    pub depth: wgpu::TextureView,
    pub frame: wgpu::Buffer,
}

impl Planar {
    /// `frame_bytes`: the frame uniform's size (`common.wgsl`'s `Frame`, the extension's block included).
    pub fn new(device: &wgpu::Device, width: u32, height: u32, frame_bytes: u64) -> Planar {
        let tex = |label: &str, format, usage| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d {
                    width: width.max(1),
                    height: height.max(1),
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage,
                view_formats: &[],
            })
        };
        let color = tex(
            "hd planar",
            crate::HDR_FORMAT,
            wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC,
        );
        let view = color.create_view(&wgpu::TextureViewDescriptor::default());
        let depth = tex(
            "hd planar depth",
            wgpu::TextureFormat::Depth32Float,
            wgpu::TextureUsages::RENDER_ATTACHMENT,
        )
        .create_view(&wgpu::TextureViewDescriptor::default());
        let frame = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("hd planar frame"),
            size: frame_bytes,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Planar {
            width,
            height,
            color,
            view,
            depth,
            frame,
        }
    }

    pub fn bytes(&self) -> u64 {
        u64::from(self.width) * u64::from(self.height) * 12
    }
}

/// How far under water half the light is absorbed at a tint alpha of a half (units); the absorption scales with the tint's alpha.
pub const UNDERWATER_HALF_DISTANCE: f32 = 12.0;

/// The frame uniform's water terms (`common.wgsl`'s `Frame::water` and `Frame::under`).
pub struct FrameTerms {
    pub water: [f32; 4],
    pub under: [f32; 4],
}

/// The water's terms of a frame seen from `eye`: the water's height (the plane's, else the zone's top) and whether it is drawn as
/// water (`drawn`: the reflections are on and the scene has a water plane), whether the eye is in the water zone `zone`, the
/// absorption there and the zone's tint (the main view's only: `main`).
pub fn frame_terms<G>(
    scene: &HdScene<G>,
    zone: Option<WaterZone>,
    eye: [f32; 3],
    drawn: bool,
    main: bool,
) -> FrameTerms {
    let level = torque_render_scene::hd::reflect::water_level(scene);
    let zone = zone.filter(|z| main && z.holds(eye));
    let tint = zone.map_or([0.0; 4], |z| z.tint);
    let under = zone.is_some() && tint[3] > 0.0;
    let sigma = if under {
        std::f32::consts::LN_2 / UNDERWATER_HALF_DISTANCE * (tint[3] / 0.5)
    } else {
        0.0
    };
    let lin = tint.map(torque_render_scene::hd::srgb_to_linear);
    FrameTerms {
        water: [
            level.or(zone.map(|z| z.max[2])).unwrap_or(0.0),
            if drawn && level.is_some() { 1.0 } else { 0.0 },
            if under { 1.0 } else { 0.0 },
            sigma,
        ],
        under: [lin[0], lin[1], lin[2], if under { tint[3] } else { 0.0 }],
    }
}

/// The tonemap's water terms (`tonemap.wgsl`'s `Post::under` and `Post::waterline`) of the water zone `zone`: its tint (its gamma
/// colour and alpha, 0 where the eye is outside the zone's sides or there is no zone), and the near plane's height over the zone's
/// top and bottom as affine functions of the output's NDC (`a x + b y + c`): a pixel whose near-plane point is under the top and
/// over the bottom is under water (the waterline splits the view where the camera straddles it).
pub fn post_terms(zone: Option<WaterZone>, camera: &HdCamera) -> ([f32; 4], [f32; 4]) {
    let Some(z) = zone else {
        return ([0.0; 4], [0.0, 0.0, 1.0, 1.0]);
    };
    let e = camera.eye;
    let tint = z.tint;
    let inside = (0..2).all(|k| z.min[k] <= e[k] && e[k] < z.max[k]);
    if !inside || tint[3] <= 0.0 {
        return ([0.0; 4], [0.0, 0.0, 1.0, 1.0]);
    }
    let to_world = crate::invert(&camera.view);
    let [l, r, b, t] = camera.frustum;
    let height = |x: f32, y: f32| -> f32 {
        let p = [
            l + (x + 1.0) * 0.5 * (r - l),
            b + (y + 1.0) * 0.5 * (t - b),
            -camera.near,
            1.0,
        ];
        (0..4).map(|k| to_world[k * 4 + 2] * p[k]).sum::<f32>()
            / (0..4).map(|k| to_world[k * 4 + 3] * p[k]).sum::<f32>()
    };
    let h0 = height(0.0, 0.0);
    let a = height(1.0, 0.0) - h0;
    let bb = height(0.0, 1.0) - h0;
    (tint, [a, bb, h0 - z.max[2], h0 - z.min[2]])
}

#[cfg(test)]
#[path = "water_tests.rs"]
mod tests;
