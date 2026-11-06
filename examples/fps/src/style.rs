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

/// Aiming down the sights: how long an image takes to come up to its sights (seconds: [`SightsBlend`] is within 1% of them by
/// then), how far ahead of the eye its rear sight is held ([`sight_offset`]), and where an image without sights is held there
/// (its place in the eye's frame).
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

/// `1 - (1 + x) e^-x` at this `x` is 0.99: a critically damped spring of rate `x / seconds` is within 1% of its target then.
const SETTLED_RATE: f32 = 6.638;

/// How far the sights are up (0 at the hip, 1 at the sights) and how fast that changes (a second): a critically damped spring
/// toward the hip or the sights, stepped exactly ([`SightsBlend::step`]), so it moves the same at any frame rate, never overshoots
/// from rest, and a press let go mid-way turns back from where it is at the speed it had.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct SightsBlend {
    pub at: f32,
    pub speed: f32,
}

impl SightsBlend {
    /// `dt` seconds toward `target` (0 or 1), settled within 1% after `seconds`.
    pub fn step(&mut self, target: f32, seconds: f32, dt: f32) {
        let dt = dt.max(0.0);
        let w = SETTLED_RATE / seconds.max(1e-3);
        let off = self.at - target;
        let e = (-w * dt).exp();
        let k = (self.speed + w * off) * dt;
        self.speed = (self.speed - w * k) * e;
        self.at = target + (off + k) * e;
        // Within a thousandth and slow: settled there (a step no one sees), not a tail that never ends.
        if (self.at - target).abs() < 1e-3 && self.speed.abs() < 0.05 {
            *self = SightsBlend {
                at: target,
                speed: 0.0,
            };
        }
        self.at = self.at.clamp(0.0, 1.0);
    }
}

/// The image's way from the hip to the sights at `aim` (0 to 1): its turn and its height and distance follow `aim`, and it
/// comes in toward the eye's line ahead of them (the side's share of the way), so it arcs in and up instead of sliding along the
/// straight line.
pub fn sights_path(aim: f32) -> [f32; 3] {
    let a = aim.clamp(0.0, 1.0);
    let side = 1.0 - (1.0 - a) * (1.0 - a);
    [side, a, a]
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
    /// How far the sights are up and how fast they move.
    aim: SightsBlend,
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
            aim: SightsBlend::default(),
            sight_nodes: HashMap::new(),
            own_sights: None,
            nearest: false,
        }
    }

    /// `dt` seconds of the sights' blend toward the sights (`aiming`) or the hip; returns how far up they are.
    pub fn update_aim(&mut self, aiming: bool, dt: f32) -> f32 {
        let target = if aiming { 1.0 } else { 0.0 };
        self.aim.step(target, self.sights.seconds, dt);
        self.aim.at
    }

    /// How far the sights are up (0 to 1).
    pub fn aim(&self) -> f32 {
        self.aim.at
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
        for t in draw.vertices.as_chunks_mut::<3>().0 {
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
        self.aim = SightsBlend::default();
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
            let way = sights_path(aim);
            let [x, y, z] = [0, 1, 2].map(|k| from[k] + (place[k] - from[k]) * way[k]);
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
        let half = g.update_aim(true, 0.09);
        assert!(half > 0.5 && half < 0.95, "{half}");
        assert_eq!(g.update_aim(true, 1.0), 1.0);
        assert_eq!(g.update_aim(false, 1.0), 0.0);
    }

    /// The blend at `fps` with the press down for `down` seconds, then up, sampled each frame for `seconds`: (time, place).
    fn blend_at(fps: f32, down: f32, seconds: f32) -> Vec<(f32, SightsBlend)> {
        let (mut b, dt) = (SightsBlend::default(), 1.0 / fps);
        let mut out = Vec::new();
        let mut t = 0.0;
        while t < seconds {
            let target = if t < down { 1.0 } else { 0.0 };
            b.step(target, Sights::default().seconds, dt);
            t += dt;
            out.push((t, b));
        }
        out
    }

    #[test]
    fn the_sights_blend_is_the_same_at_60_and_144_frames_a_second() {
        let seconds = Sights::default().seconds;
        for down in [1.0, 0.08] {
            let (a, b) = (blend_at(60.0, down, 0.6), blend_at(144.0, down, 0.6));
            // Every 12th of a second both have a frame: the same place within float noise.
            for k in 1..7 {
                let t = k as f32 / 12.0;
                let near = |v: &[(f32, SightsBlend)]| {
                    v.iter()
                        .min_by(|x, y| (x.0 - t).abs().total_cmp(&(y.0 - t).abs()))
                        .unwrap()
                        .1
                };
                let (pa, pb) = (near(&a), near(&b));
                assert!(
                    (pa.at - pb.at).abs() < 2e-3,
                    "down {down} at {t}: {pa:?} {pb:?}"
                );
            }
            // Within 1% of the sights by their time (the long press).
            if down == 1.0 {
                let at = a.iter().find(|(t, _)| *t >= seconds).unwrap().1.at;
                assert!(at > 0.985, "{at}");
            }
        }
    }

    #[test]
    fn the_sights_move_smoothly_never_past_their_ends_and_turn_back_without_a_jump() {
        for fps in [60.0, 144.0] {
            let dt = 1.0 / fps;
            for down in [0.3, 0.08, 0.03] {
                let v = blend_at(fps, down, 1.0);
                // The fastest the spring moves (from rest to the far end): at most `w / e` a second.
                let most = SETTLED_RATE / Sights::default().seconds / std::f32::consts::E;
                let mut last = SightsBlend::default();
                for (t, b) in &v {
                    assert!((0.0..=1.0).contains(&b.at), "{fps} {down} {t}: {b:?}");
                    assert!(
                        (b.at - last.at).abs() <= most * dt * 1.05,
                        "a jump at {t}: {last:?} -> {b:?}"
                    );
                    // The speed changes by at most the spring's pull a frame (no step in it).
                    let w = SETTLED_RATE / Sights::default().seconds;
                    assert!(
                        (b.speed - last.speed).abs() <= w * w * dt * 1.05,
                        "a kick at {t}"
                    );
                    last = *b;
                }
                // Up then down: never past the sights, and back at the hip.
                let peak = v.iter().map(|(_, b)| b.at).fold(0.0, f32::max);
                assert!(peak <= 1.0);
                assert_eq!(v.last().unwrap().1.at, 0.0);
            }
        }
    }

    #[test]
    fn the_pistol_arcs_in_then_up() {
        assert_eq!(sights_path(0.0), [0.0; 3]);
        assert_eq!(sights_path(1.0), [1.0; 3]);
        let mid = sights_path(0.5);
        assert!(
            mid[0] > mid[2],
            "in toward the eye's line ahead of the rise: {mid:?}"
        );
        let mut last = sights_path(0.0);
        for k in 1..=100 {
            let p = sights_path(k as f32 / 100.0);
            assert!((0..3).all(|i| p[i] >= last[i]), "monotone");
            last = p;
        }
    }
}
