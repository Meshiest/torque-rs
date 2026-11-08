//! Rendering from plugins: passes at named stages of the engine's renderer's frame, batches of many still copies of a mesh,
//! and particle emitter kinds beside TGE's. A plugin adds them while the engine is built, through [`EngineRender::render`]:
//!
//! ```
//! use torque_app::render::{EmitterDraws, EmitterFrame, EmitterKind, EngineRender, PassCx, Stage};
//! use torque_engine::{EngineBuilder, Plugin};
//!
//! #[derive(Default)]
//! struct Outline;
//!
//! impl torque_app::render::HdPass for Outline {
//!     fn draw(&mut self, cx: &mut PassCx<'_>) {
//!         // Begin a render pass in `cx.encoder` into `cx.ldr`, within `cx.viewport` ...
//!         let _ = cx;
//!     }
//! }
//!
//! #[derive(Default)]
//! struct Sparks;
//!
//! impl EmitterKind for Sparks {
//!     fn advance(&mut self, _seconds: f32) {}
//!     fn draw(&mut self, _frame: &EmitterFrame, _out: &mut EmitterDraws<'_, '_>) {}
//! }
//!
//! #[derive(Clone, Copy, Debug, Default, torque_engine::PluginId)]
//! #[plugin(id = "example.looks")]
//! struct Looks;
//!
//! impl Plugin for Looks {
//!     fn build(&self, engine: &mut EngineBuilder) {
//!         engine
//!             .render()
//!             .pass(Stage::Overlay, Outline::default)
//!             .emitter_kind("sparks", Sparks::default);
//!     }
//! }
//! ```
//!
//! **Passes** ([`RenderRegistry::pass`]): each client's world view makes its own from the factory when it attaches to the client
//! and runs it at its [`Stage`] of every frame the engine's renderer draws (another renderer on the slot does not run them),
//! after the passes added before it at that stage. What each stage hands a pass is in [`torque_render_hd::passes`]. A
//! stage with no pass costs a frame nothing but a check.
//!
//! **Static batches** ([`RenderRegistry::static_batch`]): a [`StaticBatchSource`] hands a [`StaticBatch`], one mesh placed by
//! many transforms. The view cuts it into [`BATCH_CHUNK`]-unit chunks by where each copy stands and puts each chunk into the
//! renderer's static geometry as a cell of its own: kept on the GPU, culled on the GPU in runs of its triangles, casting into
//! the cached shadow cascades, so a frame costs nothing for it on the CPU. The copies are laid out once per change (a new
//! [`StaticBatchSource::generation`]), not drawn as GPU instances: every copy's triangles are held in memory (about 36 bytes a
//! vertex on the CPU and as much again on the GPU). So a batch lays out at most [`MAX_BATCH_TRIANGLES`] triangles and a view
//! at most [`MAX_VIEW_BATCH_TRIANGLES`] over all its batches (less if the game sets a smaller budget,
//! [`WorldView::set_static_batch_budget`](crate::view::WorldView::set_static_batch_budget)); the copies past either are left out, in order, with a warning on
//! the client's console. [`WorldView::set_static_batch`](crate::view::WorldView::set_static_batch) does the same for a batch
//! the game holds itself.
//!
//! **Emitter kinds** ([`RenderRegistry::emitter_kind`]): an [`EmitterKind`] runs its own particles and draws them each frame
//! into the frame's particles, beside TGE's emitters, and they are drawn as TGE's particles are: sorted with the frame's
//! translucent draws by their distance, faded where they meet a surface, with no fog and no lighting of their own (the draws
//! are `lighting: false`, `fog: false`, as TGE's); the view's particle light rule lights the alpha-blended ones by the sun, the
//! sky and the lights, as it lights TGE's smoke, and leaves the additive ones unlit. A call draws at most
//! [`MAX_SPRITES_PER_CALL`] sprites. Each client's view makes one of each kind; a plugin reaches its kind's state through what
//! its factory shares (an `Rc` the factory captures).

use std::rc::Rc;
use std::sync::Arc;

use torque_engine::{Engine, EngineBuilder};
use torque_render_scene::math::Mat;
use torque_render_scene::textures::TextureSet;
use torque_render_scene::{Blend, Cull, Draw, Filter, Sampler, TexEnv, Texture, Vertex};

pub use torque_render_hd::passes::{HdPass, PassCx, PassScene, ShadowCascade, Stage};

/// The side of a static batch's chunks (units): the copies standing in one square of it are one cell.
pub const BATCH_CHUNK: f32 = 64.0;
/// The most copies a static batch lays out (the rest are left out).
pub const MAX_BATCH_COPIES: usize = 1 << 20;
/// The most triangles a static batch lays out (about 113 MB of vertices on the CPU): the copies past them are left out.
pub const MAX_BATCH_TRIANGLES: usize = 1 << 20;
/// The most triangles a view lays out over all its static batches.
pub const MAX_VIEW_BATCH_TRIANGLES: usize = 4 << 20;
/// The most sprites one [`EmitterDraws::sprites`] call draws (the rest are left out, with a warning once a view).
pub const MAX_SPRITES_PER_CALL: usize = 20_000;
/// The most triangles a chunk holds: one with more copies is cut again.
pub const MAX_CHUNK_TRIANGLES: usize = 256 * 1024;
/// The most passes, static batches and emitter kinds a client's view makes of each.
pub const MAX_ADDS: usize = 64;

/// A pass a plugin added: its stage and its factory.
#[derive(Clone)]
pub struct RenderPassEntry {
    stage: Stage,
    make: Rc<dyn Fn() -> Box<dyn HdPass>>,
}

impl RenderPassEntry {
    /// Its stage.
    pub fn stage(&self) -> Stage {
        self.stage
    }

    /// A new pass of it.
    pub fn make(&self) -> Box<dyn HdPass> {
        (self.make)()
    }
}

/// A static batch source a plugin added.
#[derive(Clone)]
pub struct StaticBatchEntry {
    make: Rc<dyn Fn() -> Box<dyn StaticBatchSource>>,
}

impl StaticBatchEntry {
    /// A new source of it.
    pub fn make(&self) -> Box<dyn StaticBatchSource> {
        (self.make)()
    }
}

/// An emitter kind a plugin added: its name and its factory.
#[derive(Clone)]
pub struct EmitterKindEntry {
    name: String,
    make: Rc<dyn Fn() -> Box<dyn EmitterKind>>,
}

impl EmitterKindEntry {
    /// Its name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// A new emitter of it.
    pub fn make(&self) -> Box<dyn EmitterKind> {
        (self.make)()
    }
}

/// The engine builder's call that opens the [`RenderRegistry`].
pub trait EngineRender {
    /// What plugins add to the engine's rendering.
    fn render(&mut self) -> RenderRegistry<'_>;
}

impl EngineRender for EngineBuilder {
    fn render(&mut self) -> RenderRegistry<'_> {
        RenderRegistry { engine: self }
    }
}

/// Adds passes, static batches and emitter kinds to an engine being built ([module docs](self)).
pub struct RenderRegistry<'a> {
    engine: &'a mut EngineBuilder,
}

impl RenderRegistry<'_> {
    /// Adds the pass `make` makes at `stage` of the engine's renderer's frame; each client's view makes its own.
    pub fn pass<P: HdPass>(&mut self, stage: Stage, make: impl Fn() -> P + 'static) -> &mut Self {
        self.engine.push(RenderPassEntry {
            stage,
            make: Rc::new(move || Box::new(make())),
        });
        self
    }

    /// Adds the static batch source `make` makes; each client's view makes its own and lays out its batch.
    pub fn static_batch<S: StaticBatchSource>(
        &mut self,
        make: impl Fn() -> S + 'static,
    ) -> &mut Self {
        self.engine.push(StaticBatchEntry {
            make: Rc::new(move || Box::new(make())),
        });
        self
    }

    /// Adds the emitter kind `make` makes under `name`; each client's view makes one. A name taken twice is the first's (the
    /// second is left out with a warning on the client's console).
    pub fn emitter_kind<K: EmitterKind>(
        &mut self,
        name: &str,
        make: impl Fn() -> K + 'static,
    ) -> &mut Self {
        self.engine.push(EmitterKindEntry {
            name: name.to_owned(),
            make: Rc::new(move || Box::new(make())),
        });
        self
    }
}

/// What the plugins of an engine added to its rendering, in the order they were added.
#[derive(Clone, Default)]
pub struct RenderAdds {
    pub passes: Vec<RenderPassEntry>,
    pub batches: Vec<StaticBatchEntry>,
    pub kinds: Vec<EmitterKindEntry>,
}

impl RenderAdds {
    /// What `engine`'s plugins added.
    pub fn of_engine(engine: &Engine) -> RenderAdds {
        RenderAdds {
            passes: engine.list::<RenderPassEntry>().to_vec(),
            batches: engine.list::<StaticBatchEntry>().to_vec(),
            kinds: engine.list::<EmitterKindEntry>().to_vec(),
        }
    }

    /// Whether nothing was added.
    pub fn is_empty(&self) -> bool {
        self.passes.is_empty() && self.batches.is_empty() && self.kinds.is_empty()
    }

    /// The adds a client's view makes: at most [`MAX_ADDS`] of each, an emitter kind's name once (the first's). `warn` is told of
    /// each one left out.
    pub fn kept(mut self, mut warn: impl FnMut(String)) -> RenderAdds {
        for (what, n) in [
            ("render passes", self.passes.len()),
            ("static batches", self.batches.len()),
            ("emitter kinds", self.kinds.len()),
        ] {
            if n > MAX_ADDS {
                warn(format!("{} {what} added: the first {MAX_ADDS} are kept", n));
            }
        }
        self.passes.truncate(MAX_ADDS);
        self.batches.truncate(MAX_ADDS);
        let mut kinds: Vec<EmitterKindEntry> = Vec::new();
        for k in self.kinds {
            if kinds.iter().any(|e| e.name == k.name) {
                warn(format!(
                    "a second emitter kind named {:?}: the first is kept",
                    k.name
                ));
            } else if kinds.len() < MAX_ADDS {
                kinds.push(k);
            }
        }
        self.kinds = kinds;
        self
    }
}

/// One mesh standing still in many places: its draws in the mesh's own space (textures indices into `textures`), each copy
/// placed by one of `transforms` (rotation, uniform scale and translation). Each draw is drawn as
/// [`WorldView::set_static_mesh`](crate::view::WorldView::set_static_mesh) draws a static mesh's.
#[derive(Debug, Clone, Default)]
pub struct StaticBatch {
    pub draws: Vec<Draw>,
    pub textures: Vec<Texture>,
    pub transforms: Vec<Mat>,
}

/// How a static batch is laid out within a budget of triangles ([`StaticBatch::layout`]).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BatchLayout {
    /// Each chunk's copies (indices into the batch's `transforms`).
    pub chunks: Vec<Vec<usize>>,
    /// The triangles laid out.
    pub triangles: usize,
    /// The copies left out for the budget (or past [`MAX_BATCH_COPIES`]).
    pub left_out: usize,
}

impl StaticBatch {
    /// The triangles of one copy (the mesh's).
    pub fn mesh_triangles(&self) -> usize {
        self.draws.iter().map(|d| d.vertices.len() / 3).sum()
    }

    /// [`StaticBatch::layout`] within [`MAX_BATCH_TRIANGLES`]: the chunks alone.
    pub fn chunks(&self) -> Vec<Vec<usize>> {
        self.layout(MAX_BATCH_TRIANGLES).chunks
    }

    /// The batch's copies cut into chunks: the chunks in the order of their squares, a chunk of more than
    /// [`MAX_CHUNK_TRIANGLES`] cut again in order. The copies are taken in order while their triangles fit in `budget` (and
    /// [`MAX_BATCH_TRIANGLES`]); the rest, and those past [`MAX_BATCH_COPIES`], are counted left out. A copy placed at a point
    /// that is not finite is skipped.
    pub fn layout(&self, budget: usize) -> BatchLayout {
        let triangles = self.mesh_triangles();
        let per_chunk = (MAX_CHUNK_TRIANGLES / triangles.max(1)).max(1);
        let fit = budget.min(MAX_BATCH_TRIANGLES) / triangles.max(1);
        let mut squares: std::collections::BTreeMap<(i64, i64, i64), Vec<usize>> =
            Default::default();
        let mut kept = 0usize;
        let mut left_out = self.transforms.len().saturating_sub(MAX_BATCH_COPIES);
        for (i, t) in self.transforms.iter().enumerate().take(MAX_BATCH_COPIES) {
            let at = t.column(3);
            if at.iter().any(|c| !c.is_finite()) {
                continue;
            }
            if kept >= fit {
                left_out += 1;
                continue;
            }
            kept += 1;
            let square = at.map(|c| (c / BATCH_CHUNK).floor() as i64);
            squares
                .entry((square[0], square[1], square[2]))
                .or_default()
                .push(i);
        }
        let chunks = squares
            .into_values()
            .flat_map(|copies| {
                copies
                    .chunks(per_chunk)
                    .map(<[usize]>::to_vec)
                    .collect::<Vec<_>>()
            })
            .collect();
        BatchLayout {
            chunks,
            triangles: kept * triangles,
            left_out,
        }
    }

    /// The draws of the copies `copies`, in the world: each of the mesh's draws once, holding every copy's triangles.
    pub fn chunk_draws(&self, copies: &[usize]) -> Vec<Draw> {
        self.draws
            .iter()
            .map(|d| {
                let mut vertices = Vec::with_capacity(d.vertices.len() * copies.len());
                for &c in copies {
                    let t = &self.transforms[c];
                    vertices.extend(d.vertices.iter().map(|v| {
                        let n = t.mul_v(v.normal);
                        let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
                        Vertex {
                            position: t.mul_p(v.position),
                            normal: if len > 1e-12 {
                                n.map(|k| k / len)
                            } else {
                                v.normal
                            },
                            ..*v
                        }
                    }));
                }
                Draw {
                    vertices,
                    vertex_key: None,
                    ..d.clone()
                }
            })
            .collect()
    }
}

/// A static batch a plugin keeps: the view lays out [`StaticBatchSource::batch`] again whenever
/// [`StaticBatchSource::generation`] changes (it asks once a frame).
pub trait StaticBatchSource: 'static {
    /// Counts the batch's changes.
    fn generation(&self) -> u64;

    /// The batch as it is now.
    fn batch(&mut self) -> StaticBatch;
}

/// What an emitter kind draws a frame for: the eye and the camera's right and up (a billboard's axes).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EmitterFrame {
    pub eye: [f32; 3],
    pub right: [f32; 3],
    pub up: [f32; 3],
}

/// One particle as [`EmitterDraws::sprites`] draws it: a square of `size` facing the camera at `position`, turned by `spin`
/// (radians), in `color` (times the texture).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sprite {
    pub position: [f32; 3],
    pub size: f32,
    pub spin: f32,
    pub color: [u8; 4],
}

/// Where an emitter kind's draws go: the frame's particles (each with its distance from the eye) and the textures they name.
pub struct EmitterDraws<'s, 'a> {
    pub textures: &'s mut TextureSet<'a>,
    pub draws: &'s mut Vec<(f32, Draw)>,
    /// The frame's eye.
    pub eye: [f32; 3],
    pub right: [f32; 3],
    pub up: [f32; 3],
    /// The most sprites a call drew that were left out this frame (past [`MAX_SPRITES_PER_CALL`]).
    over_cap: usize,
}

impl EmitterDraws<'_, '_> {
    /// Adds `draw` at `distance` from the eye (the translucent draws are sorted by it).
    pub fn push(&mut self, distance: f32, draw: Draw) {
        self.draws.push((distance, draw));
    }

    /// Adds `sprites` as one particle draw, as TGE's particles draw: textured with the file `texture` (none: plain squares;
    /// a texture that does not load: nothing is drawn), blended `blend` (TGE's are [`Blend::Additive`], or [`Blend::Alpha`] with
    /// `useInvAlpha`), no depth writes. Sorted at the sprites' mean point. The first [`MAX_SPRITES_PER_CALL`] are drawn (the
    /// view warns once of the rest). False: nothing was drawn.
    pub fn sprites(&mut self, sprites: &[Sprite], texture: Option<&str>, blend: Blend) -> bool {
        let texture = match texture {
            Some(name) => match self.textures.get_modulated(name) {
                Some(t) => Some(t),
                None => return false,
            },
            None => None,
        };
        self.sprites_with(sprites, texture, blend)
    }

    /// [`EmitterDraws::sprites`] with a texture the kind made itself.
    pub fn sprites_generated(
        &mut self,
        sprites: &[Sprite],
        texture: &Arc<Texture>,
        blend: Blend,
    ) -> bool {
        let Some(t) = self.textures.generated(texture) else {
            return false;
        };
        self.sprites_with(sprites, Some(t), blend)
    }

    fn sprites_with(&mut self, sprites: &[Sprite], texture: Option<usize>, blend: Blend) -> bool {
        if sprites.len() > MAX_SPRITES_PER_CALL {
            self.over_cap = self.over_cap.max(sprites.len() - MAX_SPRITES_PER_CALL);
        }
        let sprites = &sprites[..sprites.len().min(MAX_SPRITES_PER_CALL)];
        let mut vertices = Vec::with_capacity(sprites.len() * 6);
        let mut center = [0.0f64; 3];
        let mut n = 0usize;
        for s in sprites {
            let (sin, cos) = s.spin.sin_cos();
            let half = s.size * 0.5;
            let corner = |x: f32, z: f32, uv: [f32; 2]| {
                let rx = x * cos - z * sin;
                let rz = x * sin + z * cos;
                Vertex {
                    position: [0, 1, 2]
                        .map(|k| s.position[k] + (self.right[k] * rx + self.up[k] * rz) * half),
                    normal: [0.0, 0.0, 1.0],
                    color: s.color,
                    uv,
                }
            };
            let v = [
                corner(-1.0, -1.0, [0.0, 1.0]),
                corner(1.0, -1.0, [1.0, 1.0]),
                corner(1.0, 1.0, [1.0, 0.0]),
                corner(-1.0, 1.0, [0.0, 0.0]),
            ];
            if v.iter().any(|x| x.position.iter().any(|c| !c.is_finite())) {
                continue;
            }
            vertices.extend([v[0], v[1], v[3], v[1], v[2], v[3]]);
            for (c, p) in center.iter_mut().zip(s.position) {
                *c += f64::from(p);
            }
            n += 1;
        }
        if n == 0 {
            return false;
        }
        let center = center.map(|c| (c / n as f64) as f32);
        let distance = (0..3)
            .map(|k| (center[k] - self.eye[k]).powi(2))
            .sum::<f32>()
            .sqrt();
        self.push(
            distance,
            Draw {
                label: "emitter kind".to_string(),
                vertices,
                lighting: false,
                tex_env: if texture.is_some() {
                    TexEnv::Modulate
                } else {
                    TexEnv::None
                },
                texture: texture.map(|t| {
                    (
                        t,
                        Sampler {
                            repeat: false,
                            mag: Filter::Linear,
                            min: Filter::Linear,
                            mip: Filter::Linear,
                            mipmaps: true,
                        },
                    )
                }),
                blend,
                cull: Cull::None,
                depth_test: true,
                depth_write: false,
                fog: false,
                lights: Vec::new(),
                vertex_key: None,
            },
        );
        true
    }
}

/// A kind of particle emitter beside TGE's: runs its own particles and draws them into each frame's particles.
pub trait EmitterKind: 'static {
    /// Runs the particles on by `seconds` (once a drawn frame, before [`EmitterKind::draw`]).
    fn advance(&mut self, seconds: f32);

    /// Draws the particles for `frame` into `out`.
    fn draw(&mut self, frame: &EmitterFrame, out: &mut EmitterDraws<'_, '_>);
}

/// The longest step an emitter kind is run on by in one frame (seconds): a stalled frame does not pour out particles.
pub const MAX_EMITTER_STEP: f32 = 0.25;

/// A view's emitter kinds and their clock.
#[derive(Default)]
pub(crate) struct EmitterKinds {
    pub kinds: Vec<(String, Box<dyn EmitterKind>)>,
    /// The step each frame runs them on by (None: the time since the last frame).
    pub fixed: Option<f32>,
    last: Option<web_time::Instant>,
    /// Whether a call drew past [`MAX_SPRITES_PER_CALL`] already (the view warns once).
    pub warned: bool,
}

impl EmitterKinds {
    /// Runs every kind on by this frame's step and draws them for `frame` into `draws`. Some: the first frame a kind drew past
    /// [`MAX_SPRITES_PER_CALL`] sprites in a call, the warning for the console.
    pub fn frame(
        &mut self,
        frame: &EmitterFrame,
        textures: &mut TextureSet<'_>,
        draws: &mut Vec<(f32, Draw)>,
    ) -> Option<String> {
        if self.kinds.is_empty() {
            return None;
        }
        let seconds = match self.fixed {
            Some(s) => s,
            None => {
                let now = web_time::Instant::now();
                let s = self
                    .last
                    .map_or(0.0, |l| now.duration_since(l).as_secs_f32());
                self.last = Some(now);
                s
            }
        };
        let seconds = if seconds.is_finite() {
            seconds.clamp(0.0, MAX_EMITTER_STEP)
        } else {
            0.0
        };
        let mut out = EmitterDraws {
            textures,
            draws,
            eye: frame.eye,
            right: frame.right,
            up: frame.up,
            over_cap: 0,
        };
        let mut warning = None;
        for (name, kind) in &mut self.kinds {
            kind.advance(seconds);
            kind.draw(frame, &mut out);
            if out.over_cap > 0 && !self.warned {
                self.warned = true;
                warning = Some(format!(
                    "emitter kind {name:?} drew {} sprites past {MAX_SPRITES_PER_CALL} in one call: they are left out",
                    out.over_cap
                ));
            }
            out.over_cap = 0;
        }
        warning
    }
}

#[cfg(test)]
#[path = "render_tests.rs"]
mod tests;
