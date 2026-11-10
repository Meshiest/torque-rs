//! Render passes from an extension: each view of the world makes an instance of the pass and asks it for triangles at its
//! stage of every frame; the engine draws them (flat colored, alpha blended, depth tested in the world's stages).

use torque_plugin_abi::{
    TORQUE_SPACE_CLIP, TORQUE_SPACE_WORLD, TORQUE_STAGE_OPAQUE, TORQUE_STAGE_OVERLAY,
    TORQUE_STAGE_TRANSLUCENT, TorqueApi, TorqueDraw,
};

/// One vertex: a position and a linear RGBA color.
pub use torque_plugin_abi::TorqueVertex as Vertex;

/// Where in the frame a pass draws.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Stage {
    /// After the opaque pass: into the HDR frame, depth tested and written.
    Opaque,
    /// After the translucent passes: into the HDR frame, depth tested and written.
    Translucent,
    /// After the tonemap: over the finished picture, with no depth.
    Overlay,
}

impl Stage {
    /// The stage's code in the ABI (`TORQUE_STAGE_`).
    pub const fn code(self) -> u32 {
        match self {
            Stage::Opaque => TORQUE_STAGE_OPAQUE,
            Stage::Translucent => TORQUE_STAGE_TRANSLUCENT,
            Stage::Overlay => TORQUE_STAGE_OVERLAY,
        }
    }

    /// The stage of a code, or `None`.
    pub const fn of_code(code: u32) -> Option<Stage> {
        match code {
            TORQUE_STAGE_OPAQUE => Some(Stage::Opaque),
            TORQUE_STAGE_TRANSLUCENT => Some(Stage::Translucent),
            TORQUE_STAGE_OVERLAY => Some(Stage::Overlay),
            _ => None,
        }
    }
}

/// What space a draw's vertices are in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Space {
    /// World units (z up), seen through the frame's camera.
    World,
    /// Clip space: x and y from -1 to 1 across the viewport, y up.
    Clip,
}

impl Space {
    pub(crate) const fn code(self) -> u32 {
        match self {
            Space::World => TORQUE_SPACE_WORLD,
            Space::Clip => TORQUE_SPACE_CLIP,
        }
    }
}

/// The frame a pass draws in.
#[derive(Clone, Copy, Debug)]
pub struct PassFrame {
    /// The stage.
    pub stage: Stage,
    /// The size of the stage's target, in pixels.
    pub size: [u32; 2],
    /// Where the world is in the target: left, top, width, height.
    pub viewport: [u32; 4],
    /// The camera's view and projection, world to clip, column major.
    pub view_proj: [f32; 16],
    /// The camera's position.
    pub eye: [f32; 3],
    /// The scene's effects time, in seconds.
    pub time: f32,
}

/// Where a pass's triangles go for one frame.
pub struct Triangles<'a> {
    pub(crate) api: &'a TorqueApi,
    pub(crate) draw: TorqueDraw,
}

impl Triangles<'_> {
    /// Draws `vertices` (three a triangle) in `space`: false when the engine refused them (a count that is not a
    /// multiple of 3, or past the frame's limit).
    pub fn draw(&mut self, space: Space, vertices: &[Vertex]) -> bool {
        crate::ffi::draw_triangles(self.api, self.draw, space.code(), vertices)
    }

    /// Draws a quad of one color: its four corners in order around it.
    pub fn quad(&mut self, space: Space, corners: [[f32; 3]; 4], color: [f32; 4]) -> bool {
        let vertices = [0, 1, 2, 0, 2, 3].map(|i| Vertex {
            position: corners[i],
            color,
        });
        self.draw(space, &vertices)
    }
}

/// A render pass of an extension: each view of the world makes one ([`RenderRegistry::pass`]) and calls `draw` at its
/// stage of every frame. A closure taking the frame and the triangles is one.
pub trait Pass: Send + 'static {
    /// Draws one frame.
    fn draw(&mut self, frame: &PassFrame, out: &mut Triangles<'_>);
}

impl<F: FnMut(&PassFrame, &mut Triangles<'_>) + Send + 'static> Pass for F {
    fn draw(&mut self, frame: &PassFrame, out: &mut Triangles<'_>) {
        self(frame, out)
    }
}

/// The render passes an extension declares, from [`Registrar::render`](crate::Registrar::render).
pub struct RenderRegistry<'r, 'a> {
    pub(crate) registrar: &'r mut crate::Registrar<'a>,
}

impl RenderRegistry<'_, '_> {
    /// Declares a pass at `stage`: each view of the world makes one with `make`.
    pub fn pass<P: Pass>(
        &mut self,
        stage: Stage,
        make: impl Fn() -> P + Send + Sync + 'static,
    ) -> &mut Self {
        self.registrar
            .pass(stage, Box::new(move || Box::new(make()) as Box<dyn Pass>));
        self
    }
}

/// Makes a pass instance.
pub(crate) type MakePass = Box<dyn Fn() -> Box<dyn Pass> + Send + Sync>;
