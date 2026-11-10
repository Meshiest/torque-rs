use torque_render_scene::source::Eye;

use super::*;

/// A camera 10 units in front of the origin looking along +y (Torque's forward), 64 x 64 pixels.
fn camera() -> HdCamera {
    let eye = Eye {
        position: [0.0, -10.0, 0.0],
        pitch: 0.0,
        yaw: 0.0,
        fov: None,
    };
    HdCamera::new(eye, 90.0_f32, 100.0, 64, 64)
}

fn gpu() -> Option<Gpu> {
    match Gpu::new() {
        Ok(g) => Some(g),
        Err(e) => {
            eprintln!("skipped: {e}");
            None
        }
    }
}

fn segment(a: [f32; 3], b: [f32; 3], color: [u8; 4], depth_test: bool) -> GizmoSegment {
    GizmoSegment {
        a,
        b,
        color,
        depth_test,
    }
}

/// A depth texture of 64 x 64 cleared to `value`, readable by shaders.
fn depth_of(gpu: &Gpu, value: f32) -> wgpu::TextureView {
    let texture = gpu.device().create_texture(&wgpu::TextureDescriptor {
        label: Some("test depth"),
        size: wgpu::Extent3d {
            width: 64,
            height: 64,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: crate::DEPTH_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    let mut encoder = gpu
        .device()
        .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
    drop(encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("test depth clear"),
        color_attachments: &[],
        depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
            view: &view,
            depth_ops: Some(wgpu::Operations {
                load: wgpu::LoadOp::Clear(value),
                store: wgpu::StoreOp::Store,
            }),
            stencil_ops: None,
        }),
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    }));
    gpu.queue().submit([encoder.finish()]);
    view
}

fn frame_of<'a>(
    target: &'a wgpu::TextureView,
    camera: &'a HdCamera,
    depth: Option<&'a wgpu::TextureView>,
    width: f32,
) -> GizmoFrame<'a> {
    GizmoFrame {
        target,
        rect: [0, 0, 64, 64],
        camera,
        depth,
        width,
    }
}

fn pixel(frame: &torque_render_core::Frame, x: u32, y: u32) -> [u8; 4] {
    let i = 4 * (y * frame.width + x) as usize;
    [
        frame.rgba[i],
        frame.rgba[i + 1],
        frame.rgba[i + 2],
        frame.rgba[i + 3],
    ]
}

#[test]
fn the_shader_is_valid_wgsl() {
    let module = naga::front::wgsl::parse_str(SHADER).expect("it parses");
    naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::default(),
    )
    .validate(&module)
    .expect("it validates");
}

#[test]
fn a_segment_is_written_in_the_layout_the_shader_reads() {
    let mut out = Vec::new();
    segment([1.0, 2.0, 3.0], [4.0, 5.0, 6.0], [7, 8, 9, 10], true).write(&mut out);
    assert_eq!(out.len(), SEGMENT_BYTES);
    assert_eq!(out[0..4], 1.0f32.to_le_bytes());
    assert_eq!(out[12..16], 1u32.to_le_bytes());
    assert_eq!(out[16..20], 4.0f32.to_le_bytes());
    assert_eq!(out[28..32], [7, 8, 9, 10]);
}

#[test]
fn a_line_is_drawn_where_the_camera_sees_it_and_an_empty_batch_draws_nothing() {
    let Some(gpu) = gpu() else { return };
    let target = gpu.offscreen(64, 64).unwrap();
    let mut lines = GizmoLines::new(&gpu);
    let camera = camera();
    assert_eq!(
        lines.draw(&gpu, &frame_of(&target.color, &camera, None, 3.0), &[]),
        0
    );
    // A horizontal line through the origin: the middle row of the picture.
    let line = segment([-4.0, 0.0, 0.0], [4.0, 0.0, 0.0], [255, 0, 0, 255], true);
    assert_eq!(
        lines.draw(&gpu, &frame_of(&target.color, &camera, None, 3.0), &[line]),
        1
    );
    let frame = gpu.read_back(&target).unwrap();
    let on = pixel(&frame, 32, 32);
    assert!(on[0] > 200 && on[1] < 40, "{on:?}");
    assert_eq!(pixel(&frame, 32, 10)[0], 0);
    assert_eq!(pixel(&frame, 2, 32)[0], 0, "past the line's end");
}

#[test]
fn a_tested_segment_is_hidden_behind_the_scene_and_an_untested_one_is_not() {
    let Some(gpu) = gpu() else { return };
    let camera = camera();
    // The scene is at the near plane (reversed depth 1) everywhere: it is nearer than any line.
    let depth = depth_of(&gpu, 1.0);
    let tested = segment([-4.0, 0.0, 0.0], [4.0, 0.0, 0.0], [0, 255, 0, 255], true);
    let free = segment([-4.0, 0.0, 1.0], [4.0, 0.0, 1.0], [0, 0, 255, 255], false);
    let target = gpu.offscreen(64, 64).unwrap();
    let mut lines = GizmoLines::new(&gpu);
    lines.draw(
        &gpu,
        &frame_of(&target.color, &camera, Some(&depth), 3.0),
        &[tested, free],
    );
    let frame = gpu.read_back(&target).unwrap();
    assert_eq!(pixel(&frame, 32, 32)[1], 0, "the tested line is hidden");
    let blues = (0..64).filter(|&y| pixel(&frame, 32, y)[2] > 200).count();
    assert!(blues > 0, "the untested line shows");
    // The scene far behind everything (reversed depth 0): the tested line shows.
    let depth = depth_of(&gpu, 0.0);
    let target = gpu.offscreen(64, 64).unwrap();
    lines.draw(
        &gpu,
        &frame_of(&target.color, &camera, Some(&depth), 3.0),
        &[tested],
    );
    let frame = gpu.read_back(&target).unwrap();
    assert!(pixel(&frame, 32, 32)[1] > 200);
}

#[test]
fn a_line_behind_the_camera_is_not_drawn_and_one_crossing_the_near_plane_is_cut() {
    let Some(gpu) = gpu() else { return };
    let camera = camera();
    let target = gpu.offscreen(64, 64).unwrap();
    let mut lines = GizmoLines::new(&gpu);
    let behind = segment([-4.0, -20.0, 0.0], [4.0, -20.0, 0.0], [255; 4], false);
    let crossing = segment(
        [0.0, -20.0, 2.0],
        [0.0, 30.0, 2.0],
        [255, 255, 0, 255],
        false,
    );
    lines.draw(
        &gpu,
        &frame_of(&target.color, &camera, None, 2.0),
        &[behind, crossing],
    );
    let frame = gpu.read_back(&target).unwrap();
    let lit = (0..64).filter(|&y| pixel(&frame, 32, y)[0] > 100).count();
    assert!(lit > 0, "the part in front is drawn");
    assert_eq!(pixel(&frame, 10, 32)[0], 0, "the line behind is not");
}
