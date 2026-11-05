//! The arena's own touches on the engine's ghosts ([`ArenaStyle`], a [`GhostStyle`]): the first-person pistol kicked by a shot
//! ([`Kick`]) and raised to its sights while the player aims ([`Sights`]: the `sightRear` and `sightFront` nodes on the eye's
//! line), a bolt drawn leaving the shooter's muzzle and curving onto its flight, growing out of it and turning about its flight
//! ([`BoltPath`]), the shot's flare at the muzzle ([`ShotFlare`]), and the interiors' look: the kit's tinted tiles drawn as the
//! cubes are, walls lighter the higher ([`ArenaStyle::interior_draw`]).

use std::collections::HashMap;

use torque::app::ghosts::{EffectsFrame, Flight, GhostStyle, HeldImage};
use torque::ghost_world::tge::TgeClientSet;
use torque::ghost_world::world::ClientWorld;
use torque_render_scene::images::{HeldMotion, ImageDb, node_named, sight_offset};
use torque_render_scene::math::Mat;
use torque_render_scene::shapes::load;
use torque_render_scene::textures::TextureSet;
use torque_render_scene::{Assets, Draw, Filter, Vertex};

/// A shot's kick of the image that fired it (none in TGE: its `fire` sequence and its owner's recoil sequence alone): how long it
/// takes to reach its height and how fast it settles after (seconds), how far it goes at its height in the first person (back,
/// units; up, radians) and in the hand (up, radians; 0 leaves the hand's kick to the owner's recoil sequence).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Kick {
    pub rise: f32,
    pub settle: f32,
    pub back: f32,
    pub up: f32,
    pub up_hand: f32,
}

impl Kick {
    /// How far into its kick the image is `t` seconds after a shot: 0 at rest, 1 at its height (a quick rise, then a settle).
    pub fn at(&self, t: f32) -> f32 {
        if !(0.0..1.0).contains(&t) {
            0.0
        } else if t < self.rise {
            t / self.rise
        } else {
            (-(t - self.rise) / self.settle).exp()
        }
    }
}

/// Aiming down the sights: how long an image takes to come up to its sights (seconds), how far ahead of the eye its rear sight is
/// held ([`sight_offset`]), and where an image without sights is held there (its place in the eye's frame).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sights {
    pub seconds: f32,
    pub rear: f32,
    pub fallback: [f32; 3],
}

impl Default for Sights {
    fn default() -> Sights {
        Sights {
            seconds: 0.18,
            rear: 0.36,
            fallback: [0.0, 0.42, -0.057],
        }
    }
}

/// The ease of the sights' blend: `t` (0 to 1 of [`Sights::seconds`]) smoothed at both ends.
pub fn aim_ease(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// How a projectile is drawn leaving its source's muzzle: curving from there onto its flight over its first `blend` units, its
/// size growing from `start_size` of its own to all of it over its first `grow` units, and turning about its flight at `spin`
/// radians a second. The defaults draw it on its flight from the first frame, whole and unturned.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BoltPath {
    pub blend: f32,
    pub start_size: f32,
    pub grow: f32,
    pub spin: f32,
}

impl Default for BoltPath {
    fn default() -> BoltPath {
        BoltPath {
            blend: 0.0,
            start_size: 1.0,
            grow: 0.0,
            spin: 0.0,
        }
    }
}

/// A shot's flare at the muzzle: a star facing the eye (`texture`, `size` units wide head on), shrinking and fading over
/// `seconds`.
#[derive(Debug, Clone, PartialEq)]
pub struct ShotFlare {
    pub texture: String,
    pub seconds: f32,
    pub size: f32,
}

/// A quad draw of `corners` (with `uv`s), added over what is behind it, `alpha` its strength.
fn sprite(label: &str, corners: [[f32; 3]; 4], texture: usize, alpha: f32) -> Draw {
    let uv = [[0.0, 1.0], [1.0, 1.0], [1.0, 0.0], [0.0, 0.0]];
    let a = (alpha.clamp(0.0, 1.0) * 255.0).round() as u8;
    let v = |i: usize| Vertex {
        position: corners[i],
        normal: [0.0, 0.0, 1.0],
        color: [255, 255, 255, a],
        uv: uv[i],
    };
    Draw {
        label: label.to_string(),
        vertices: vec![v(0), v(1), v(2), v(0), v(2), v(3)],
        lighting: false,
        tex_env: torque_render_scene::TexEnv::Modulate,
        texture: Some((
            texture,
            torque_render_scene::Sampler {
                repeat: false,
                mag: Filter::Linear,
                min: Filter::Linear,
                mip: Filter::Linear,
                mipmaps: true,
            },
        )),
        blend: torque_render_scene::Blend::Additive,
        cull: torque_render_scene::Cull::None,
        depth_test: true,
        depth_write: false,
        fog: false,
        lights: Vec::new(),
        vertex_key: None,
    }
}

/// A shot's flare ([`ShotFlare`]) at the muzzle `at` along `dir`, `t` (0 to 1) through its time, seen from `eye` with the
/// camera's `right` and `up`: the star facing the eye at the muzzle.
fn flare_draws(
    flare: &ShotFlare,
    at: [f32; 3],
    dir: [f32; 3],
    t: f32,
    eye: [f32; 3],
    (right, up): ([f32; 3], [f32; 3]),
    set: &mut TextureSet,
) -> Vec<(f32, Draw)> {
    let mut out = Vec::new();
    let fade = 1.0 - t.clamp(0.0, 1.0);
    let dist = (0..3).map(|k| (at[k] - eye[k]).powi(2)).sum::<f32>().sqrt();
    if let Some(tex) = set.get_modulated(&flare.texture) {
        let h = 0.5 * flare.size * (0.45 + 0.55 * fade);
        let c = [0, 1, 2].map(|k| at[k] + dir[k] * 0.01);
        let p = |sx: f32, sy: f32| [0, 1, 2].map(|k| c[k] + (right[k] * sx + up[k] * sy) * h);
        let corners = [p(-1.0, -1.0), p(1.0, -1.0), p(1.0, 1.0), p(-1.0, 1.0)];
        // Drawn twice: the additive passes add up to a brighter flash than the texture alone.
        out.push((dist, sprite("flare star", corners, tex, fade)));
        out.push((dist, sprite("flare star", corners, tex, fade)));
    }
    out
}

/// An image shape's sights: where they are held ([`sight_offset`]) and their nodes (front, rear).
#[derive(Debug, Clone, Copy)]
struct SightNodes {
    place: Option<([f32; 3], f32)>,
    nodes: Option<[usize; 2]>,
}

/// The arena's style of the engine's ghosts (see the module docs).
pub struct ArenaStyle {
    pub kick: Option<Kick>,
    pub sights: Sights,
    pub bolt: BoltPath,
    pub flare: Option<ShotFlare>,
    /// How far the sights' blend has run (0 at the hip, 1 at the sights; not eased).
    aim: f32,
    /// Each image shape's sights ([`sight_offset`]) and its `sightFront` and `sightRear` nodes, by path.
    sight_nodes: HashMap<String, Option<SightNodes>>,
    own_sights: Option<[[f32; 3]; 2]>,
    /// Interiors' textures magnified with nearest-neighbor filtering (the map's sharp texels).
    pub nearest: bool,
}

impl ArenaStyle {
    /// The style with the sights down.
    pub fn new(
        kick: Option<Kick>,
        sights: Sights,
        bolt: BoltPath,
        flare: Option<ShotFlare>,
    ) -> ArenaStyle {
        ArenaStyle {
            kick,
            sights,
            bolt,
            flare,
            aim: 0.0,
            sight_nodes: HashMap::new(),
            own_sights: None,
            nearest: false,
        }
    }

    /// `dt` seconds of the sights' blend toward the sights (`aiming`) or the hip; returns how far up they are (eased).
    pub fn update_aim(&mut self, aiming: bool, dt: f32) -> f32 {
        let step = dt.max(0.0) / self.sights.seconds.max(1e-3);
        self.aim = if aiming {
            (self.aim + step).min(1.0)
        } else {
            (self.aim - step).max(0.0)
        };
        aim_ease(self.aim)
    }

    /// How far the sights are up (eased, 0 to 1).
    pub fn aim(&self) -> f32 {
        aim_ease(self.aim)
    }

    /// Where the own Player's first image's sights (`sightFront`, `sightRear`) were drawn in the last frame.
    pub fn own_sights(&self) -> Option<[[f32; 3]; 2]> {
        self.own_sights
    }

    /// The sights of the image shape at `path`, loaded once.
    fn sight_nodes(
        &mut self,
        assets: &dyn Assets,
        world: &ClientWorld<TgeClientSet>,
        path: &str,
    ) -> Option<SightNodes> {
        let rear = self.sights.rear;
        *self.sight_nodes.entry(path.to_string()).or_insert_with(|| {
            let mut notes = Vec::new();
            let loaded = load(
                &|p| world.constructor_sequences(p),
                assets,
                path,
                &mut notes,
            )?;
            let shape = &loaded.model.shape;
            let nodes = node_named(shape, b"sightFront").zip(node_named(shape, b"sightRear"));
            Some(SightNodes {
                place: sight_offset(shape, rear),
                nodes: nodes.map(|(f, r)| [f, r]),
            })
        })
    }

    /// Where a bolt is drawn: on its flight, from the muzzle curving onto it over [`BoltPath::blend`].
    pub fn drawn(&self, flight: &Flight) -> [f32; 3] {
        let path = &self.bolt;
        let offset = match flight.muzzle {
            Some(m) if path.blend > 0.0 => [0, 1, 2].map(|k| m[k] - flight.start[k]),
            _ => [0.0; 3],
        };
        let k = if path.blend > 0.0 {
            (1.0 - (flight.flown() / path.blend).clamp(0.0, 1.0)).powi(2)
        } else {
            0.0
        };
        [0, 1, 2].map(|i| flight.start[i] + flight.velocity[i] * flight.age + offset[i] * k)
    }

    /// A bolt's size as drawn (a share of its shape's): growing out of the muzzle.
    pub fn size(&self, flight: &Flight) -> f32 {
        let path = &self.bolt;
        if path.grow <= 0.0 {
            return 1.0;
        }
        path.start_size + (1.0 - path.start_size) * (flight.flown() / path.grow).clamp(0.0, 1.0)
    }
}

impl GhostStyle for ArenaStyle {
    /// The kit's floor and wall tiles are already tinted (a sand and a slate); each wall triangle's vertices take the cubes'
    /// tint at its top over the slate, so a wall is drawn as a cube of its height.
    fn interior_draw(&mut self, material: &str, draw: &mut Draw) {
        if let Some((_, sampler)) = draw.texture.as_mut()
            && self.nearest
        {
            sampler.mag = Filter::Nearest;
        }
        if material.eq_ignore_ascii_case(torque_example_assets::interiors::FLOOR) {
            return;
        }
        let slate = torque_example_assets::textures::SLATE;
        for t in draw.vertices.chunks_exact_mut(3) {
            let top = t.iter().map(|v| v.position[2]).fold(f32::MIN, f32::max);
            let tint = crate::scene::cube_tint(top.max(0.06));
            let c =
                [0, 1, 2].map(|k| (u32::from(tint[k]) * 255 / u32::from(slate[k])).min(255) as u8);
            for v in t {
                v.color = [c[0], c[1], c[2], 255];
            }
        }
    }

    fn clear(&mut self) {
        self.aim = 0.0;
        self.sight_nodes.clear();
        self.own_sights = None;
    }

    /// Its kick, and in the first person its rise to the sights.
    fn held_motion(&mut self, image: &HeldImage<'_>) -> HeldMotion {
        let aim = self.aim();
        let kick = self.kick.map_or(0.0, |k| k.at(image.since_fire));
        let mut motion = HeldMotion::default();
        if let Some(k) = self.kick.filter(|k| kick > 0.0 && k.up_hand != 0.0) {
            motion.hand = Some(Mat::euler_x(-k.up_hand * kick));
        }
        if image.own_eye
            && let Some(db) = ImageDb::of(image.world, image.datablock)
            && let Some(eye_offset) = db.eye_offset
            && (aim > 0.0 || kick > 0.0)
        {
            let sights = self.sight_nodes(image.assets, image.world, &db.shape_file);
            let fallback = (self.sights.fallback, 0.0);
            let (place, pitch) = sights.and_then(|s| s.place).unwrap_or(fallback);
            let (back, up) = self.kick.map_or((0.0, 0.0), |k| (k.back, k.up));
            let mut m = Mat::mul(
                &eye_offset,
                &Mat::mul(&Mat::euler_x(-up * kick), &Mat::euler_x(pitch * aim)),
            );
            let from = eye_offset.column(3);
            let [x, y, z] = [0, 1, 2].map(|k| from[k] + (place[k] - from[k]) * aim);
            m.set_column3([x, y - back * kick, z + 0.2 * back * kick]);
            motion.eye = Some(m);
        }
        motion
    }

    /// The own pistol's sights as drawn.
    fn own_image(
        &mut self,
        image: Option<&HeldImage<'_>>,
        node: &mut dyn FnMut(usize) -> Option<[f32; 3]>,
    ) {
        self.own_sights = None;
        let Some(image) = image else { return };
        let nodes = ImageDb::of(image.world, image.datablock)
            .and_then(|db| self.sight_nodes(image.assets, image.world, &db.shape_file))
            .and_then(|s| s.nodes);
        if let Some(nodes) = nodes {
            let mut at = nodes.map(node);
            if let [Some(front), Some(rear)] = [at[0].take(), at[1].take()] {
                self.own_sights = Some([front, rear]);
            }
        }
    }

    /// Turned along its flight and about it, sized, at its drawn place.
    fn projectile(&self, flight: &Flight) -> Mat {
        let spin = Mat::from_axis_angle([0.0, 1.0, 0.0], self.bolt.spin * flight.age);
        let mut m = Mat::mul(&flight.heading(), &spin);
        let size = self.size(flight);
        for r in 0..3 {
            for c in 0..3 {
                m.m[r * 4 + c] *= size;
            }
        }
        m.set_column3(self.drawn(flight));
        m
    }

    /// The shots' flares at the muzzles.
    fn effects(&mut self, frame: &EffectsFrame<'_>, set: &mut TextureSet) -> Vec<(f32, Draw)> {
        let mut out = Vec::new();
        let Some(flare) = &self.flare else {
            return out;
        };
        for m in frame.muzzles {
            if m.since_fire < flare.seconds {
                let t = m.since_fire / flare.seconds;
                out.extend(flare_draws(
                    flare,
                    m.at,
                    m.dir,
                    t,
                    frame.eye,
                    (frame.right, frame.up),
                    set,
                ));
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_kick_rises_quickly_and_settles() {
        let k = Kick {
            rise: 0.04,
            settle: 0.08,
            back: 0.06,
            up: 0.14,
            up_hand: 0.3,
        };
        assert_eq!(k.at(0.0), 0.0);
        assert_eq!(k.at(0.04), 1.0);
        assert!(k.at(0.02) > 0.4);
        assert!(k.at(0.08) > k.at(0.15) && k.at(0.3) < 0.05);
        assert_eq!(k.at(f32::MAX), 0.0);
    }

    #[test]
    fn a_bolt_leaves_the_muzzle_grows_and_curves_onto_its_flight() {
        let path = BoltPath {
            blend: 12.0,
            start_size: 0.3,
            grow: 1.5,
            spin: std::f32::consts::TAU,
        };
        let style = ArenaStyle::new(None, Sights::default(), path, None);
        let mut b = Flight {
            start: [0.0; 3],
            velocity: [0.0, 100.0, 0.0],
            age: 0.0,
            muzzle: Some([0.2, 0.0, -0.1]),
        };
        assert_eq!(style.drawn(&b), [0.2, 0.0, -0.1], "at the muzzle");
        assert!((style.size(&b) - 0.3).abs() < 1e-6);
        b.age = 0.05;
        let mid = style.drawn(&b);
        assert!(mid[0] > 0.0 && mid[0] < 0.2 && mid[1] == 5.0, "{mid:?}");
        assert_eq!(style.size(&b), 1.0);
        b.age = 0.2;
        assert_eq!(
            style.drawn(&b),
            [0.0, 20.0, 0.0],
            "on its flight past the curve"
        );
        // The defaults draw it on its flight, whole.
        let plain = ArenaStyle::new(None, Sights::default(), BoltPath::default(), None);
        b.age = 0.0;
        assert_eq!((plain.drawn(&b), plain.size(&b)), ([0.0; 3], 1.0));
        let m = plain.projectile(&b);
        assert_eq!(m.column(3), [0.0; 3]);
        assert!(
            (m.mul_v([0.0, 1.0, 0.0])[1] - 1.0).abs() < 1e-6,
            "along its flight"
        );
    }

    #[test]
    fn the_sights_come_up_eased_and_go_down() {
        let mut g = ArenaStyle::new(None, Sights::default(), BoltPath::default(), None);
        assert_eq!(g.update_aim(true, 0.09), aim_ease(0.5));
        assert_eq!(g.update_aim(true, 1.0), 1.0);
        assert_eq!(g.update_aim(false, 1.0), 0.0);
    }
}
