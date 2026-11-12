//! One effect on its stage with a world to draw it: what the window's tabs switch between and what the recorder runs. A show
//! steps its effect's emitters at a fixed rate (any number of simulation steps a frame), hands their particles to the GPU pool
//! and draws the frame from the stage's eye or one the caller gives.

use torque_render_core::{Gpu, wgpu};
use torque_render_hd::ext::{NoExtension, PlainScene};
use torque_render_hd::{HdWorld, ParticlePool};
use torque_render_scene::hd::cells::StaticCells;
use torque_render_scene::hd::kit::{box_object, look_at};
use torque_render_scene::hd::particle_light::ParticleLight;
use torque_render_scene::hd::{DynamicObject, HdCamera, HdDraw, HdScene, HdSun, material};
use torque_render_scene::particles::{Rng, SpriteFrame};
use torque_render_scene::source::Eye;
use torque_render_scene::textures::TextureSet;
use torque_render_scene::{Blend, Cull, Draw, TexEnv, Vertex};

use crate::effects::{
    Circle, Effect, Explosion, Fireflies, Fireworks, Floor, Fog, Fountain, Galaxy, Glyphs, Lava,
    Petals, Portal, Railgun, Setting, Smoke, Sparks, Storm,
};
use crate::sprites::{self, Sprites};

/// The shows in tab order: name, tab label, the seconds simulated before a recording's first frame (the particles fill the
/// air, the picture settles), and the seconds recorded.
pub const SHOWS: [(&str, &str, f32, f32); 15] = [
    ("sparks", "Sparks", 1.5, 10.0),
    ("smoke", "Smoke", 12.0, 12.0),
    ("fireflies", "Fireflies", 6.0, 12.0),
    ("circle", "Magic circle", 0.3, 16.0),
    ("portal", "Portal", 2.0, 10.0),
    ("railgun", "Railgun", 0.5, 12.0),
    ("fireworks", "Fireworks", 0.0, 18.0),
    ("explosion", "Explosion", 0.0, 12.0),
    ("storm", "Lightning storm", 2.0, 12.0),
    ("fountain", "Fountain", 4.0, 12.0),
    ("fog", "Volumetric fog", 2.0, 12.0),
    ("lava", "Lava", 5.0, 12.0),
    ("petals", "Petals", 14.0, 12.0),
    ("galaxy", "Galaxy", 5.0, 12.0),
    ("glyphs", "Digital rain", 4.0, 12.0),
];

pub fn effect(name: &str) -> Option<Box<dyn Effect>> {
    Some(match name {
        "sparks" => Box::new(Sparks::new()),
        "smoke" => Box::new(Smoke::new()),
        "fireflies" => Box::new(Fireflies::new()),
        "circle" => Box::new(Circle::new()),
        "portal" => Box::new(Portal::new()),
        "railgun" => Box::new(Railgun::new()),
        "fireworks" => Box::new(Fireworks::new()),
        "explosion" => Box::new(Explosion::new()),
        "storm" => Box::new(Storm::new()),
        "fountain" => Box::new(Fountain::new()),
        "fog" => Box::new(Fog::new()),
        "lava" => Box::new(Lava::new()),
        "petals" => Box::new(Petals::new()),
        "galaxy" => Box::new(Galaxy::new()),
        "glyphs" => Box::new(Glyphs::new()),
        _ => return None,
    })
}

/// What a flash of 1 adds to the ambient light and to the sky's color (linear): a stroke's blue-white over everything.
const FLASH_AMBIENT: [f32; 3] = [0.7, 0.88, 1.0];
const FLASH_SKY: [f32; 3] = [0.32, 0.42, 0.5];

/// The floor: `half` units each way at height 0, its texture every 4 units.
fn floor(half: f32) -> Draw {
    let corner = |x: f32, y: f32| Vertex {
        position: [x, y, 0.0],
        normal: [0.0, 0.0, 1.0],
        color: [255; 4],
        uv: [x / 4.0, y / 4.0],
    };
    let (a, b, c, d) = (
        corner(-half, -half),
        corner(half, -half),
        corner(half, half),
        corner(-half, half),
    );
    Draw {
        label: "floor".into(),
        vertices: vec![a, b, c, a, c, d],
        lighting: true,
        tex_env: TexEnv::Modulate,
        texture: Some((0, crate::scenery::sampler(true))),
        blend: Blend::Opaque,
        cull: Cull::None,
        depth_test: true,
        depth_write: true,
        fog: true,
        lights: Vec::new(),
        vertex_key: None,
    }
}

/// A box of `color`.
fn prop(ghost: u32, (center, half, color): ([f32; 3], [f32; 3], [u8; 3])) -> DynamicObject {
    let mut o = box_object(ghost, center, half);
    for d in &mut o.draws {
        for v in &mut d.vertices {
            v.color = [color[0], color[1], color[2], 255];
        }
    }
    o
}

/// The stage of `setting` at `size`: the floor, the props and scenery, the light and the air, the pool's particles lit where
/// blended; with the objects that stay through the show.
fn stage(
    setting: Setting,
    floor_texture: usize,
    size: (u32, u32),
    frame: &mut HdScene<StaticCells>,
) -> Vec<DynamicObject> {
    frame.planes = vec![HdDraw::from_fixed(
        &floor(80.0),
        material::KIND_PLANE,
        &[floor_texture],
    )];
    frame.planes_generation += 1;
    let mut objects: Vec<DynamicObject> = setting
        .props
        .iter()
        .enumerate()
        .map(|(i, &p)| prop(i as u32 + 1, p))
        .collect();
    objects.extend(setting.scenery);
    let (towards, diffuse, ambient) = setting.sun;
    frame.sun = HdSun {
        towards,
        diffuse,
        ambient,
        shadow: ambient,
    };
    frame.clear = setting.clear;
    frame.atmosphere = setting.atmosphere;
    let (color, start, end) = setting.fog.unwrap_or((setting.clear, 40.0, 90.0));
    frame.fog.color = color;
    // A hazy stage's fog is even through the room (no height fog); the usual stage keeps the enhanced fog.
    frame.fog.linear = setting.fog.is_some();
    frame.fog.start = start;
    frame.fog.end = end;
    frame.visible_distance = end.max(100.0);
    frame.fog_distance = start;
    frame.ao_radius = 0.8;
    frame.particle_light = Some(ParticleLight::BLENDED);
    frame.size = size;
    frame.fov = setting.fov;
    let mut eye = look_at(setting.eye, setting.target);
    eye.fov = Some(setting.fov);
    frame.eye = Some(eye);
    objects
}

/// Where a show's camera starts: the stage's eye, what it looks at and its field of view (degrees).
#[derive(Clone, Copy)]
pub struct View {
    pub eye: [f32; 3],
    pub target: [f32; 3],
    pub fov: u8,
}

pub struct Show {
    pub view: View,
    pub world: HdWorld<NoExtension>,
    /// The stage alone: the effect is stepped but nothing of it is drawn (a recording to diff an effect against).
    pub bare: bool,
    effect: Box<dyn Effect>,
    scene: PlainScene,
    still: Vec<DynamicObject>,
    /// The air's velocity the pool's steps carry the particles by: the setting's, or the effect's at the time (a gust).
    wind: [f32; 3],
    /// The setting's sun and sky, which a flash lifts for a frame.
    sun: HdSun,
    clear: [f32; 3],
    assets: Sprites,
    rng: Rng,
    ms_total: u64,
    frames: u32,
}

impl Show {
    /// The show `name` on `gpu`, drawn at `size`.
    pub fn new(gpu: &Gpu, name: &str, size: (u32, u32)) -> Result<Show, String> {
        let (name, ..) = SHOWS
            .iter()
            .copied()
            .find(|s| s.0 == name)
            .ok_or(format!("no show {name}"))?;
        let effect = effect(name).ok_or(format!("no show {name}"))?;
        let mut frame = HdScene::new(StaticCells::default());
        let setting = effect.setting();
        let floor_texture = frame.textures.intern(match setting.floor {
            Floor::Concrete => sprites::concrete(),
            Floor::Grass => sprites::grass(),
            Floor::Planks => sprites::planks(),
            Floor::Basalt => sprites::basalt(),
        });
        let view = View {
            eye: setting.eye,
            target: setting.target,
            fov: setting.fov,
        };
        let wind = setting.wind;
        let still = stage(setting, floor_texture, size, &mut frame);
        let (sun, clear) = (frame.sun, frame.clear);
        let scene = PlainScene {
            frame,
            mode: Some(effect.mode()),
            time: 0.0,
        };
        let mut world: HdWorld<NoExtension> = HdWorld::new(gpu);
        world.features.gpu_particles = ParticlePool::On;
        // The pool at its quality's full capacity: the portal's rim alone wants more than the CPU path's cap.
        world.set_gpu_particle_cap(u32::MAX);
        if !world.gpu_particles_on() {
            return Err("this device does not run the GPU particle pool".into());
        }
        Ok(Show {
            view,
            world,
            bare: false,
            effect,
            scene,
            still,
            wind,
            sun,
            clear,
            assets: Sprites::new(),
            rng: Rng::new(0x5eed),
            ms_total: 0,
            frames: 0,
        })
    }

    pub fn resize(&mut self, size: (u32, u32)) {
        self.scene.frame.size = size;
    }

    /// Seconds since the show started.
    pub fn time(&self) -> f32 {
        self.ms_total as f32 / 1000.0
    }

    /// The effect's key: the circle traces again, the railgun fires, a shell goes up.
    pub fn trigger(&mut self) {
        let t = self.time();
        self.effect.trigger(t, &mut self.rng);
    }

    /// The simulation steps of frame `k` at `fps` (ms, two a frame, the frames' times rounded to whole ms).
    pub fn steps_at(k: u64, fps: u32) -> [u32; 2] {
        let at = |k: u64| (k * 1000 / u64::from(fps)) as u32;
        let ms = at(k + 1) - at(k);
        [ms / 2, ms - ms / 2]
    }

    /// One frame: the emitters stepped by each of `steps` (ms), their particles handed to the pool, the stage drawn into
    /// `target` (its format and offset) from `eye` (None: the stage's own).
    pub fn frame(
        &mut self,
        steps: &[u32],
        eye: Option<Eye>,
        target: (&wgpu::TextureView, wgpu::TextureFormat),
    ) -> Result<(), String> {
        for &ms in steps {
            self.ms_total += u64::from(ms);
            let t = self.time();
            for n in self.effect.nodes() {
                n.emitter.advance(ms, &mut n.live);
            }
            self.effect.step(t, ms, &mut self.rng);
        }
        let t = self.time();
        if let Some(wind) = self.effect.wind(t) {
            self.wind = wind;
        }
        let mut sprites = SpriteFrame {
            steps: steps.to_vec(),
            wind: self.wind,
            ..SpriteFrame::default()
        };
        let mut set = TextureSet::new(&self.assets);
        // Bare, the emitters' particles go to a frame that is never drawn.
        let mut bin = SpriteFrame::default();
        let into = if self.bare { &mut bin } else { &mut sprites };
        for n in self.effect.nodes() {
            n.emitter.hand_over(into, &mut set, "showcase", &mut n.live);
        }
        let decals = if self.bare {
            Vec::new()
        } else {
            self.effect.decals(t, &mut set)
        };
        let s = &mut self.scene.frame;
        s.set_particles_decals_and_sprites(&[], &decals, Some(&sprites), set);
        s.objects.clone_from(&self.still);
        if !self.bare {
            s.objects.extend(self.effect.meshes(t));
        }
        s.distorters = if self.bare {
            Vec::new()
        } else {
            self.effect.distorters(t)
        };
        s.lights = if self.bare {
            Vec::new()
        } else {
            self.effect.lights(t, self.frames)
        };
        s.spots = if self.bare {
            Vec::new()
        } else {
            self.effect.spots(t)
        };
        let flash = if self.bare { 0.0 } else { self.effect.flash(t) };
        let lifted = |base: [f32; 3], by: [f32; 3]| [0, 1, 2].map(|k| base[k] + by[k] * flash);
        s.sun = HdSun {
            ambient: lifted(self.sun.ambient, FLASH_AMBIENT),
            shadow: lifted(self.sun.shadow, FLASH_AMBIENT),
            ..self.sun
        };
        s.clear = lifted(self.clear, FLASH_SKY);
        s.fog.color = s.clear;
        s.lights_generation += 1;
        s.generation += 1;
        s.control_generation += 1;
        if let Some(eye) = eye.or_else(|| self.effect.eye(t)) {
            s.eye = Some(eye);
        }
        let eye = s.eye.expect("the stage's eye");
        let camera = HdCamera::new(
            eye,
            f32::from(eye.fov.unwrap_or(s.fov)),
            s.visible_distance,
            s.size.0,
            s.size.1,
        );
        self.scene.time = t;
        self.frames += 1;
        self.world.upload(&self.scene);
        self.world
            .draw(&self.scene, &camera, Some((target.0, target.1, [0, 0])))
    }

    /// The particles alive in the pool after the last frame (waits for the GPU).
    pub fn live(&self) -> Result<u32, String> {
        Ok(self.world.gpu_particle_counts()?.map_or(0, |c| c.live()))
    }
}
