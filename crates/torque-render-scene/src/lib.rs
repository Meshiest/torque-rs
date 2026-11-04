//! A frame as plain data: the camera's matrices, the sun as GL light 0, the textures with their mip chains, and a list of
//! [`Draw`]s, each a triangle list with the fixed-function state it is drawn with (lighting, texture environment, sampler,
//! culling, depth, blending), so a renderer only has to reproduce OpenGL 1.x's per-vertex lighting, texture environments and
//! rasterisation; the enhanced renderer's data types ([`hd`]), the scene's source ([`source`]), what a frame reads of the
//! client's world of ghosts ([`world`]), the environment ([`env`](mod@env)), the sun's flare ([`flare`]), the lights' coronas ([`lights`]), the decals on surfaces ([`decals`]) and
//! their lines of sight ([`sight`]), the camera ([`camera`]) and TGE's matrix math ([`math`]). No GPU type appears here, and every type is `Send + Sync`.
//!
//! Files come through [`Assets`] (the caller's file reader).

pub mod camera;
pub mod decals;
pub mod env;
pub mod flare;
pub mod hd;
pub mod images;
pub mod lamps;
pub mod lights;
pub mod map;
pub mod math;
pub mod mounts;
pub mod moving;
pub mod par;
pub mod particles;
pub mod player;
pub mod shapecache;
pub mod shapes;
pub mod sight;
pub mod sky;
pub mod source;
pub mod textures;
pub mod tge;
pub mod wheels;
pub mod world;

#[cfg(test)]
mod tests;

/// The caller's file reader: the bytes of `path` (such as `data/shapes/rock.png`, matched ignoring case as TGE's
/// resource manager does), `None` when there is no such file.
pub trait Assets {
    fn read(&self, path: &str) -> Option<Vec<u8>>;
}

/// The frame's default size.
pub const DEFAULT_SIZE: (u32, u32) = (800, 600);

/// A vertex as a fixed-function vertex buffer holds one (position, normal, RGBA8 colour, UV).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Vertex {
    /// World space.
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub color: [u8; 4],
    pub uv: [f32; 2],
}

/// `GL_TEXTURE_ENV_MODE`, or no texture at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TexEnv {
    /// No texture bound: the fragment's colour.
    None,
    /// `GL_DECAL`: colour x (1 - texture alpha) + texture x texture alpha; the fragment's alpha.
    Decal,
    /// `GL_MODULATE`: colour x texture.
    Modulate,
    /// `GL_REPLACE`: the texture.
    Replace,
}

/// A texture filter (`GL_NEAREST` / `GL_LINEAR`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Filter {
    Nearest,
    Linear,
}

/// How a draw samples its texture: wrap (`GL_REPEAT`, else `GL_CLAMP`, which NVIDIA's GL treats as clamp to edge), the
/// magnification filter, and the minification filter as its two halves (`GL_NEAREST_MIPMAP_LINEAR` is `Nearest` within a level,
/// `Linear` between levels).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sampler {
    pub repeat: bool,
    pub mag: Filter,
    pub min: Filter,
    pub mip: Filter,
    /// Whether the levels below the first are sampled (a `NoMipMap` material's texture is drawn from level 0 only).
    pub mipmaps: bool,
}

/// Blending (`glBlendFunc`), when enabled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Blend {
    Opaque,
    /// `SRC_ALPHA, ONE_MINUS_SRC_ALPHA`.
    Alpha,
    /// `SRC_ALPHA, ONE` (additive).
    Additive,
    /// `ONE, ONE` (an fxLight's corona with `BlendMode` 2).
    One,
    /// `DST_COLOR, ONE_MINUS_SRC_ALPHA` (multiplies the destination).
    Multiply,
}

/// A point light as TGE's `LightManager::installGLLight` sets a GL light: world position (w = 1), the diffuse colour (zero
/// ambient), and the constant, linear and quadratic attenuation.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PointLight {
    pub position: [f32; 3],
    pub diffuse: [f32; 3],
    pub attenuation: [f32; 3],
}

/// Face culling.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Cull {
    None,
    /// Cull back faces, the front being clockwise.
    BackCw,
    /// Cull back faces, the front being counter-clockwise.
    BackCcw,
}

/// One draw call: triangles (three vertices each, a quad split as GL splits `GL_QUADS`: 0 1 2, 0 2 3) and their state.
#[derive(Debug, Clone, PartialEq)]
pub struct Draw {
    /// What it is, for the notes and the per-pass debugging images.
    pub label: String,
    pub vertices: Vec<Vertex>,
    /// GL lighting (`GL_LIGHTING` with `GL_COLOR_MATERIAL`: the vertex colour is the ambient and diffuse material).
    pub lighting: bool,
    pub tex_env: TexEnv,
    /// An index into [`Scene::textures`] and how it is sampled (`None` with [`TexEnv::None`]).
    pub texture: Option<(usize, Sampler)>,
    pub blend: Blend,
    pub cull: Cull,
    pub depth_test: bool,
    pub depth_write: bool,
    /// Whether linear distance fog applies ([`Scene::fog`]).
    pub fog: bool,
    /// The point lights installed beside the sun (GL lights 1 on).
    pub lights: Vec<PointLight>,
    /// The vertices' identity across builds (a live window's static chunks): the renderer keeps the
    /// buffers it made for a key and uses them again for a draw with the same key and no vertices of its own. None: none kept.
    pub vertex_key: Option<u64>,
}

/// A texture as TGE uploads it: RGBA8 levels from the full size down to 1 x 1 (the CPU mip chain of `extrudeMipLevels`).
#[derive(Debug, Clone, PartialEq)]
pub struct Texture {
    /// The path it was loaded from.
    pub path: String,
    pub width: u32,
    pub height: u32,
    /// Level 0 first; each level's RGBA8 bytes, rows top to bottom. Shared: the decoded-texture cache and every frame
    /// that draws the file hold one copy.
    pub levels: std::sync::Arc<Vec<Vec<u8>>>,
}

/// The sun as GL light 0: a directional light with no specular term in effect (the materials' specular is GL's default black).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sun {
    /// The unit vector towards the sun, world space (the light's `GL_POSITION` with w = 0).
    pub towards: [f32; 3],
    pub diffuse: [f32; 3],
    pub ambient: [f32; 3],
}

/// Linear distance fog (when a draw has it): the colour reaches `color` at `end` from nothing at `start` (eye distance).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Fog {
    pub color: [f32; 3],
    pub start: f32,
    pub end: f32,
}

/// The camera: the matrices and what they were made from.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Camera {
    /// World position of the eye.
    pub eye: [f32; 3],
    /// World to GL clip space (GL's depth range: z in [-w, w]), column-major as GL takes it.
    pub view_proj: [f32; 16],
    /// Horizontal field of view (radians), the near and far planes.
    pub fov: f32,
    pub near: f32,
    pub far: f32,
}

/// A frame.
#[derive(Debug, Clone, PartialEq)]
pub struct Scene {
    pub width: u32,
    pub height: u32,
    pub camera: Camera,
    /// The colour the frame starts from (`glClear`), RGB 0..1.
    pub clear: [f32; 3],
    pub sun: Sun,
    pub fog: Option<Fog>,
    pub textures: Vec<Texture>,
    pub draws: Vec<Draw>,
    /// What the build could not draw or had to guess, one line each.
    pub notes: Vec<String>,
}
