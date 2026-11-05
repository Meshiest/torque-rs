//! The enhanced frame's data the renderer reads, whatever builds it: the sun, the sky and its irradiance, the fog, the planes,
//! the static geometry (a [`StaticGeometry`] provider's cells), the moving objects, the lights, the coronas and flare, the
//! particles and one texture table. What fills it from a world is the world's own.

use crate::hd::cells::StaticGeometry;
use crate::hd::{DynamicObject, HdDraw, TextureTable, clouds, material, sh, shadow, sky};
use crate::source::Eye;
use crate::textures::TextureSet;
use crate::{Blend, Draw};

/// The field of view a frame starts with (degrees).
pub const DEFAULT_FOV: u8 = 90;

/// A point light: world position, linear colour times brightness, the light's radius (the falloff is windowed to it).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HdLight {
    pub position: [f32; 3],
    pub color: [f32; 3],
    pub radius: f32,
    /// The shape the light is attached to (its ghost: a player's held light, with its mounted images), left out of the light's
    /// own shadow casters: the light sits on or in it, and its body would cast a wedge of dark from beside the source.
    pub carrier: Option<u32>,
}

/// A spot light (Torque3D's `SpotLight`): a point light whose light is cut to a cone about `direction`, whole inside the `inner`
/// half angle, none past the `outer` one, a smooth step between (radians). One that casts shadows takes a point light's shadow
/// slot, of which only the cube faces its cone meets are drawn ([`crate::hd::shadow::cone_meets_face`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HdSpot {
    pub light: HdLight,
    /// Where the cone points (unit).
    pub direction: [f32; 3],
    pub inner: f32,
    pub outer: f32,
    pub shadows: bool,
}

impl HdSpot {
    /// The cone as the GPU's light takes it: the direction times the reciprocal of the soft edge's width in cosines, and the
    /// outer half angle's cosine (a point light's is all zeros).
    pub fn cone(&self) -> [f32; 4] {
        let outer = self.outer.clamp(1e-3, std::f32::consts::PI - 1e-3);
        let inner = self.inner.clamp(0.0, outer);
        let width = (inner.cos() - outer.cos()).max(1e-3);
        let len = self.direction.iter().map(|v| v * v).sum::<f32>().sqrt();
        let d = if len > 1e-6 {
            self.direction.map(|v| v / len)
        } else {
            [0.0, 0.0, -1.0]
        };
        [d[0] / width, d[1] / width, d[2] / width, outer.cos()]
    }

    /// The cone's share of the light at `p` (1 inside the inner half angle, 0 past the outer, smooth between): what the forward
    /// pass takes.
    pub fn share(&self, p: [f32; 3]) -> f32 {
        cone_share(self.cone(), self.light.position, p)
    }
}

/// [`HdSpot::share`] of a cone as [`HdSpot::cone`] packs it, for a light at `at`.
pub fn cone_share(cone: [f32; 4], at: [f32; 3], p: [f32; 3]) -> f32 {
    let k = (cone[0] * cone[0] + cone[1] * cone[1] + cone[2] * cone[2]).sqrt();
    if k == 0.0 {
        return 1.0;
    }
    let to = [p[0] - at[0], p[1] - at[1], p[2] - at[2]];
    let d = (to[0] * to[0] + to[1] * to[1] + to[2] * to[2]).sqrt();
    if d < 1e-6 {
        return 1.0;
    }
    let c = (to[0] * cone[0] + to[1] * cone[1] + to[2] * cone[2]) / (d * k);
    let x = ((c - cone[3]) * k).clamp(0.0, 1.0);
    x * x * (3.0 - 2.0 * x)
}

/// The sun as the fixed-function frame lights with it (the sun light's colours, gamma-space 0..1), and the Sun's shadow colour.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HdSun {
    pub towards: [f32; 3],
    pub diffuse: [f32; 3],
    pub ambient: [f32; 3],
    pub shadow: [f32; 3],
}

/// The sky-matched distance fog: the horizon ban's colour (gamma 0..1), from `start` (the Sky's fog distance) to `end` (the visible
/// distance, where everything is fog: the far plane's cut is hidden).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HdFog {
    pub color: [f32; 3],
    pub start: f32,
    pub end: f32,
    /// TGE's linear fog from `start` to `end`, without the height fog; else the enhanced fog (the square of the linear share,
    /// the height fog over it).
    pub linear: bool,
}

/// The enhanced renderer's frame data, the static geometry from the provider `G`.
#[derive(Debug, Clone, PartialEq)]
pub struct HdScene<G> {
    /// The eye the eye-dependent parts (sky, planes, coronas, flares) were built for.
    pub eye: Option<Eye>,
    pub fov: u8,
    pub visible_distance: f32,
    pub fog_distance: f32,
    pub sun: HdSun,
    /// The sky's irradiance shape as L2 SH ([`sh`]: 9 RGB coefficients, 1 on average over the horizontal normals).
    pub sky_sh: [[f32; 3]; 9],
    pub fog: HdFog,
    /// The farthest the sun's cascades reach (units; [`shadow::SHADOW_DISTANCE`], or the visible distance of a scene whose rooms
    /// run past it: past the cascades the floor and walls read sunlit).
    pub shadow_distance: f32,
    /// The fixed-function frame's clear (the fog colour quad), gamma 0..1.
    pub clear: [f32; 3],
    /// The sky in TGE's order (box, the remote flare, clouds, bans): drawn first, unlit, no depth.
    pub sky: Vec<HdDraw>,
    /// The eye the sky's draws were built around (a whole sync's): the renderer moves them by the eye's way from it each frame, so
    /// the sky stays at the eye between syncs (it rode with the world and snapped back at each sync).
    pub sky_eye: [f32; 3],
    /// Which sky the enhanced renderer draws, and whether the last sync drew its own ([`sky::SkyChoice::atmosphere`]): then `sky`
    /// holds only the server's flat cloud layers and there is no flare.
    pub sky_choice: sky::SkyChoice,
    pub atmosphere: bool,
    /// The sun the server's box paints near the Sun's direction ([`sky::painted_sun`]; None with our own sky or a box without one):
    /// the disk, the flares and the sky's glow about the sun sit on it ([`HdScene::sky_sun`]); the light keeps the Sun's direction.
    pub painted_sun: Option<sky::PaintedSun>,
    /// The sun the physical sky is made for when the light comes from elsewhere (the moon at night, [`crate::hd::scatter`]);
    /// None: the Sun's direction.
    pub atmosphere_sun: Option<[f32; 3]>,
    /// The last search's key: the box's textures and the Sun's direction (bits) it was made for.
    pub(crate) painted_key: Option<(Vec<usize>, [u32; 3])>,
    /// Whether a sync looks for a painted sun at all (off: the disk, the flares and the glow stay on the Sun's direction; the
    /// checks' before pictures).
    pub follow_painted_sun: bool,
    /// Opaque planes (the ground) and blended ones (water).
    pub planes: Vec<HdDraw>,
    pub water: Vec<HdDraw>,
    pub cells: G,
    pub objects: Vec<DynamicObject>,
    /// The objects as the last frame drew them, kept by a whole sync for the control sync of the same frame (the shapes it makes
    /// again move from where that frame had them, not from where the sync just put them).
    pub drawn_objects: Option<Vec<DynamicObject>>,
    /// The coronas, each at its eye distance.
    pub coronas: Vec<(f32, HdDraw)>,
    /// The sun's own flare, drawn last.
    pub flare: Option<HdDraw>,
    /// The particles of the frame (the images' state emitters, the explosions), each at its eye distance, drawn among the
    /// translucent draws back to front; set every frame, not by a sync.
    pub particles: Vec<(f32, HdDraw)>,
    /// Counts the particle sets (the GPU uploads them again when it changes).
    pub particles_generation: u64,
    /// Whether the last sync put off static geometry textures still being decoded (a live window's build decodes new ones on a
    /// decoder thread).
    pub textures_pending: bool,
    pub lights: Vec<HdLight>,
    /// The spot lights (their changes counted by `lights_generation` too).
    pub spots: Vec<HdSpot>,
    /// How many of `lights`, at its end, cast no shadows (glowing surfaces' and shaded lamps' lights).
    pub glow_lights: usize,
    /// Counts the changes of `lights` (a control sync moves a light attached to the control object; the GPU uploads them again
    /// when it changed).
    pub lights_generation: u64,
    pub textures: TextureTable,
    pub notes: Vec<String>,
    /// Counts the syncs (the GPU side compares it).
    pub generation: u64,
    /// How many of them were control syncs (the control object's only): `generation` less this counts the changes to everything
    /// else (the sky's, the planes', the other shapes' draws).
    pub control_generation: u64,
    /// What the server's Sky gives the volumetric clouds (None without a Sky: no clouds).
    pub clouds: Option<clouds::CloudSky>,
    /// The frame size of the last sync (a control sync's coronas are placed for it).
    pub size: (u32, u32),
    /// Control syncs keep each moving shape's vertices in its meshes' own space and the GPU places them
    /// ([`NodePlace`](crate::hd::NodePlace)); off: the CPU places every vertex (the reference the checks compare with).
    pub gpu_nodes: bool,
}

impl<G> HdScene<G> {
    /// An empty frame drawing the cells of `cells`.
    pub fn new(cells: G) -> HdScene<G> {
        HdScene {
            eye: None,
            fov: DEFAULT_FOV,
            visible_distance: 500.0,
            fog_distance: 250.0,
            sun: HdSun {
                towards: [0.0, 0.707, 0.707],
                diffuse: [0.0; 3],
                ambient: [0.0; 3],
                shadow: [0.0; 3],
            },
            sky_sh: sh::uniform(),
            fog: HdFog {
                color: [1.0; 3],
                start: 250.0,
                end: 500.0,
                linear: false,
            },
            clear: [1.0; 3],
            shadow_distance: shadow::SHADOW_DISTANCE,
            sky: Vec::new(),
            sky_eye: [0.0; 3],
            sky_choice: sky::SkyChoice::Auto,
            atmosphere: false,
            painted_sun: None,
            atmosphere_sun: None,
            painted_key: None,
            follow_painted_sun: true,
            planes: Vec::new(),
            water: Vec::new(),
            cells,
            objects: Vec::new(),
            drawn_objects: None,
            gpu_nodes: true,
            coronas: Vec::new(),
            flare: None,
            particles: Vec::new(),
            particles_generation: 0,
            textures_pending: false,
            lights: Vec::new(),
            spots: Vec::new(),
            glow_lights: 0,
            lights_generation: 0,
            textures: TextureTable::default(),
            notes: Vec::new(),
            generation: 0,
            control_generation: 0,
            clouds: None,
            size: (1, 1),
        }
    }

    /// The frame's particles ([`particles`](crate::particles)' draws, each at its eye distance, their textures indices into
    /// `textures`): unlit sprites that fade into what is behind them; the additive ones also emitters, into the range the bloom
    /// picks up (as the coronas). Replaces the last frame's and counts a new set.
    pub fn set_particles(&mut self, draws: &[(f32, Draw)], textures: TextureSet) {
        self.set_particles_and_decals(draws, &[], textures);
    }

    /// [`HdScene::set_particles`] with the frame's decals ([`decals`](crate::decals)' draws, each at its eye distance, their
    /// textures indices into the same `textures`): unlit translucent squares on the surfaces, drawn as the particles are but
    /// not faded into the surface they lie on ([`material::DECAL`]); a [`Blend::Modulate2x`] one in its own pass over the
    /// opaque frame instead.
    pub fn set_particles_and_decals(
        &mut self,
        particles: &[(f32, Draw)],
        decals: &[(f32, Draw)],
        textures: TextureSet,
    ) {
        let local: Vec<usize> = textures
            .into_textures()
            .into_iter()
            .map(|t| self.textures.intern(t))
            .collect();
        let sprites = particles.iter().map(|(dist, d)| {
            let hd =
                HdDraw::from_fixed(d, material::KIND_SKY, &local).with_material(material::PARTICLE);
            let hd = if d.blend == Blend::Additive {
                hd.with_material(material::EMITTER)
            } else {
                hd
            };
            (*dist, hd)
        });
        let marks = decals.iter().map(|(dist, d)| {
            let hd = HdDraw::from_fixed(d, material::KIND_SKY, &local)
                .with_material(material::PARTICLE | material::DECAL);
            (*dist, hd)
        });
        self.particles = sprites.chain(marks).collect();
        self.particles_generation = self.particles_generation.wrapping_add(1);
    }

    /// Towards the sun as the sky shows it: the sun the server's box paints ([`HdScene::painted_sun`]), else the physical sky's
    /// ([`HdScene::atmosphere_towards`]).
    pub fn sky_sun(&self) -> [f32; 3] {
        self.painted_sun
            .and_then(|p| p.drawn())
            .unwrap_or(self.atmosphere_towards())
    }

    /// Towards the sun the physical sky is made for: [`HdScene::atmosphere_sun`], else the Sun's direction.
    pub fn atmosphere_towards(&self) -> [f32; 3] {
        self.atmosphere_sun.unwrap_or(self.sun.towards)
    }
}

impl<G: StaticGeometry> HdScene<G> {
    /// Whether changed cells or their textures wait for a later build.
    pub fn pending(&self) -> bool {
        self.cells.pending() || self.textures_pending
    }
}

#[cfg(test)]
#[path = "scene_tests.rs"]
mod tests;
