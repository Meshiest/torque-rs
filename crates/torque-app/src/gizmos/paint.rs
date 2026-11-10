//! What a [`WorldView`](crate::view::WorldView) does with a client's [`Gizmos`] each frame: one batched draw for the lines, the
//! labels as glyph quads of the GUI's fonts, and the shapes' ageing. Nothing is made, and nothing sent to the GPU, until a frame
//! has something to draw.

use std::rc::Rc;

use torque_gui::{DrawCache, GuiDraws, TextLook};
use torque_render_core::{Gpu, GuiPass, wgpu};
use torque_render_hd::gizmos::{GizmoFrame, GizmoLines};
use torque_render_scene::hd::HdCamera;
use web_time::Instant;

use super::state::{Gizmos, Label};

/// The longest a frame ages the shapes by when it follows the clock (seconds): a frame after a stall is not a long time.
const MAX_FRAME_SECONDS: f32 = 1.0;

/// The view's side of one client's [`Gizmos`].
pub struct GizmoPainter {
    gizmos: Rc<Gizmos>,
    lines: Option<GizmoLines>,
    text: Option<TextPass>,
    last: Instant,
    /// The time every frame ages the shapes by (None: the clock's).
    step: Option<f32>,
}

/// The labels' pass: the GUI's glyph quads over the world, with the fonts it finds.
struct TextPass {
    gui: GuiPass,
    cache: DrawCache,
}

impl GizmoPainter {
    /// A painter for `gizmos`.
    pub fn new(gizmos: Rc<Gizmos>) -> GizmoPainter {
        GizmoPainter {
            gizmos,
            lines: None,
            text: None,
            last: Instant::now(),
            step: None,
        }
    }

    /// Whether nothing was ever drawn: no pipeline, buffer or font was made.
    pub fn is_idle(&self) -> bool {
        self.lines.is_none() && self.text.is_none()
    }

    /// The shapes this painter draws.
    pub fn gizmos(&self) -> &Rc<Gizmos> {
        &self.gizmos
    }

    /// Every frame ages the shapes by `seconds` (None: by the time the clock says passed since the last frame). The offscreen
    /// runners set the frame's step so a picture does not depend on how long the machine took.
    pub fn set_frame_seconds(&mut self, seconds: Option<f32>) {
        self.step = seconds;
    }

    /// Draws the shapes over the world just drawn into `rect` (left, top, width, height) of `target`, seen through `camera`, and ages
    /// them by the frame's time. `depth` is the renderer's depth of the frame (None: nothing is hidden behind the scene).
    pub fn paint(
        &mut self,
        gpu: &Gpu,
        target: &wgpu::TextureView,
        rect: [u32; 4],
        camera: &HdCamera,
        depth: Option<&wgpu::TextureView>,
    ) {
        let now = Instant::now();
        let seconds = self
            .step
            .unwrap_or_else(|| (now - self.last).as_secs_f32().min(MAX_FRAME_SECONDS));
        self.last = now;
        if self.gizmos.is_empty() {
            return;
        }
        let GizmoPainter {
            gizmos,
            lines,
            text,
            ..
        } = self;
        let settings = *gizmos.settings();
        gizmos.with_drawn(|segments, labels| {
            if !segments.is_empty() {
                let frame = GizmoFrame {
                    target,
                    rect,
                    camera,
                    depth,
                    width: settings.line_width,
                };
                lines
                    .get_or_insert_with(|| GizmoLines::new(gpu))
                    .draw(gpu, &frame, segments);
            }
            if !labels.is_empty() {
                let pass = text.get_or_insert_with(|| TextPass {
                    gui: GuiPass::new(gpu, None),
                    cache: crate::draw_cache(),
                });
                pass.draw(gpu, target, rect, camera, labels, &settings);
            }
        });
        gizmos.end_frame(seconds);
    }
}

impl TextPass {
    fn draw(
        &mut self,
        gpu: &Gpu,
        target: &wgpu::TextureView,
        rect: [u32; 4],
        camera: &HdCamera,
        labels: &[Label],
        settings: &super::GizmoSettings,
    ) {
        let texture = target.texture();
        let mut draws = GuiDraws {
            size: (texture.width(), texture.height()),
            ..GuiDraws::default()
        };
        let [x, y, w, h] = rect;
        let clip = [x as i32, y as i32, w as i32, h as i32];
        for label in labels {
            let Some((px, py)) = project(camera, label.position, rect) else {
                continue;
            };
            let face = settings.font_face;
            let size = settings.font_size;
            let Some((width, height)) = self.cache.text_size(face, size, label.text.as_bytes())
            else {
                continue;
            };
            let at = (px as i32 - width / 2, py as i32 - height - 2);
            let outline = [0, 0, 0, label.color[3]];
            self.cache.push_text(
                &mut draws,
                clip,
                at,
                label.text.as_bytes(),
                &TextLook {
                    face,
                    size,
                    color: label.color,
                    outline: Some(outline),
                },
            );
        }
        if draws.quads() > 0 {
            self.gui.draw(gpu, &draws, target, None);
        }
    }
}

/// The pixel of the target a world point lands on through `camera`, with the world drawn into `rect` (None: behind the eye or
/// well outside the rectangle).
pub fn project(camera: &HdCamera, point: [f32; 3], rect: [u32; 4]) -> Option<(f32, f32)> {
    let m = &camera.view_proj;
    let row = |i: usize| m[i] * point[0] + m[4 + i] * point[1] + m[8 + i] * point[2] + m[12 + i];
    let w = row(3);
    if w <= camera.near {
        return None;
    }
    let (nx, ny) = (row(0) / w, row(1) / w);
    if nx.abs() > 1.25 || ny.abs() > 1.25 {
        return None;
    }
    let [x, y, width, height] = rect.map(|v| v as f32);
    Some((x + (nx * 0.5 + 0.5) * width, y + (0.5 - ny * 0.5) * height))
}
