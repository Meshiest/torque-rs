//! The offscreen renderer: an HD world with a plain scene (a physical sky at a fixed time of day), the textures of Source
//! materials interned once each, models posed on the CPU into dynamic objects, and pictures read back to PNG files.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use torque_format_valve::model::{StudioModel, pose_at, world_transforms};
use torque_format_valve::vmt::{Blend as VmtBlend, Vmt};
use torque_modelcore::math;
use torque_render_core::{Gpu, Offscreen};
use torque_render_hd::HdWorld;
use torque_render_hd::ext::{NoExtension, PlainScene};
use torque_render_hd::gizmos::{GizmoFrame, GizmoLines, GizmoSegment};
use torque_render_scene::hd::cells::StaticCells;
use torque_render_scene::hd::kit::look_at;
use torque_render_scene::hd::scatter::{ScatterLook, ScatterTable};
use torque_render_scene::hd::{
    DynamicObject, HdCamera, HdDraw, HdMode, HdScene, HdVertex, Quality, material, oct_encode,
};
use torque_render_scene::math::Mat;
use torque_render_scene::{Blend, Cull, Filter, Sampler, Texture};
use torque_scene::time_of_day::{DayFields, TimeOfDay};

use crate::Content;

pub const FOV: u8 = 70;

/// A material as drawn: its texture in the scene's table (or none), blend, sides and lighting.
#[derive(Debug, Clone, Copy)]
pub struct Look {
    pub texture: Option<usize>,
    pub blend: Blend,
    pub two_sided: bool,
    pub unlit: bool,
    pub tint: [u8; 4],
    /// Cut out by the texture's alpha (drawn opaque elsewhere): `$alphatest` materials, and `$translucent` ones of models
    /// (their leaves and fences; a brush's translucent glass stays blended).
    pub cutout: bool,
}

impl Look {
    pub const PLAIN: Look = Look {
        texture: None,
        blend: Blend::Opaque,
        two_sided: false,
        unlit: false,
        tint: [200, 200, 200, 255],
        cutout: false,
    };

    /// The vertex material word of a surface of `kind`.
    pub fn word(&self, kind: u32) -> u32 {
        let mut w = kind << material::KIND_SHIFT;
        if self.texture.is_some() {
            w |= material::tex_env(torque_render_scene::TexEnv::Modulate);
        }
        if self.unlit {
            w |= material::UNLIT;
        }
        if self.cutout {
            w |= material::CUTOUT;
        }
        w
    }

    /// An empty draw of this look.
    pub fn draw(&self, label: String) -> HdDraw {
        HdDraw {
            label,
            vertices: Vec::new(),
            indices: Vec::new(),
            texture: self.texture.map(|t| (t, sampler())),
            blend: self.blend,
            cull: if self.two_sided {
                Cull::None
            } else {
                Cull::BackCw
            },
            depth_test: true,
            depth_write: self.blend == Blend::Opaque,
            pbr: None,
        }
    }
}

pub fn sampler() -> Sampler {
    Sampler {
        repeat: true,
        mag: Filter::Linear,
        min: Filter::Linear,
        mip: Filter::Linear,
        mipmaps: true,
    }
}

/// A unit-length vertex normal's encoding.
pub fn normal(n: [f32; 3]) -> [i16; 2] {
    let l = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
    if l > 1e-6 {
        oct_encode([n[0] / l, n[1] / l, n[2] / l])
    } else {
        oct_encode([0.0, 0.0, 1.0])
    }
}

pub struct Renderer {
    pub gpu: Gpu,
    pub world: HdWorld<NoExtension>,
    pub scene: PlainScene,
    pub target: Offscreen,
    pub size: (u32, u32),
    looks: HashMap<String, Look>,
}

impl Renderer {
    pub fn new(size: (u32, u32)) -> Result<Renderer, String> {
        let gpu = Gpu::new().map_err(|e| e.to_string())?;
        let world: HdWorld<NoExtension> = HdWorld::new(&gpu);
        let mut scene = PlainScene {
            frame: HdScene::new(StaticCells::default()),
            mode: Some(HdMode::new(Quality::High)),
            time: 0.0,
        };
        scene.frame.atmosphere = true;
        scene.frame.visible_distance = 2000.0;
        scene.frame.fog_distance = 1500.0;
        scene.frame.fog.start = 600.0;
        scene.frame.fog.end = 2000.0;
        scene.frame.shadow_distance = 150.0;
        let target = gpu.offscreen(size.0, size.1).map_err(|e| e.to_string())?;
        let mut r = Renderer {
            gpu,
            world,
            scene,
            target,
            size,
            looks: HashMap::new(),
        };
        r.set_time(0.2, 220.0);
        Ok(r)
    }

    /// The sun and sky of `time` (0..1 of a day), the sun at `azimuth` degrees.
    pub fn set_time(&mut self, time: f32, azimuth: f32) {
        let mut day = TimeOfDay::new(DayFields {
            azimuth_override: azimuth.to_radians(),
            play: false,
            ..DayFields::default()
        });
        day.set_time(time);
        self.set_sun(day.towards());
    }

    /// The sun and sky with the sun towards `sun` (a unit vector).
    pub fn set_sun(&mut self, sun: [f32; 3]) {
        let light = ScatterTable::earth().light(sun, &ScatterLook::default());
        let s = &mut self.scene.frame;
        s.sun = light.sun();
        s.fog = light.fog(s.fog);
        s.clear = light.fog;
        s.atmosphere_sun = Some(sun);
        s.clouds = Some(light.clouds([1.0, 0.4, 0.0]));
        s.lights_generation += 1;
    }

    /// The scene's index of a texture file (decoded once).
    pub fn texture(&mut self, c: &Content, path: &str) -> Option<usize> {
        let t = c.texture(path)?;
        let levels: Vec<Vec<u8>> = t.levels.iter().map(|l| l.rgba.clone()).collect();
        Some(self.scene.frame.textures.intern(Texture {
            path: path.to_string(),
            width: t.levels[0].width,
            height: t.levels[0].height,
            levels: Arc::new(levels),
        }))
    }

    /// How a material draws (its base texture, blend, sides, lighting); `second` takes its second base texture (a blend's).
    pub fn look(&mut self, c: &Content, name_or_paths: &[String], second: bool) -> Look {
        let key = format!("{}#{second}", name_or_paths.join("|"));
        if let Some(l) = self.looks.get(&key) {
            return *l;
        }
        let m: Option<std::rc::Rc<Vmt>> = name_or_paths.iter().find_map(|p| c.material(p));
        let look = match m {
            None => Look::PLAIN,
            Some(m) => {
                let tex = if second {
                    m.base_texture2()
                } else {
                    m.base_texture()
                };
                let texture = tex.and_then(|p| self.texture(c, &p));
                let blend = match m.blend() {
                    VmtBlend::Opaque => Blend::Opaque,
                    VmtBlend::AlphaTest | VmtBlend::Translucent => Blend::Alpha,
                    VmtBlend::Additive => Blend::Additive,
                };
                let color = m.color("$color").or(m.color("$color2")).unwrap_or([1.0; 3]);
                let alpha = m.float("$alpha").unwrap_or(1.0).clamp(0.0, 1.0);
                let tint = [
                    (color[0].clamp(0.0, 1.0) * 255.0) as u8,
                    (color[1].clamp(0.0, 1.0) * 255.0) as u8,
                    (color[2].clamp(0.0, 1.0) * 255.0) as u8,
                    (alpha * 255.0) as u8,
                ];
                let model = name_or_paths.first().is_some_and(|p| {
                    p.to_ascii_lowercase()
                        .replace('\\', "/")
                        .starts_with("materials/models/")
                });
                let cutout = texture.is_some()
                    && (m.blend() == VmtBlend::AlphaTest
                        || (model && m.blend() == VmtBlend::Translucent));
                Look {
                    texture,
                    blend: if cutout {
                        Blend::Opaque
                    } else if alpha < 1.0 && blend == Blend::Opaque {
                        Blend::Alpha
                    } else {
                        blend
                    },
                    two_sided: m.two_sided() || cutout,
                    unlit: m.unlit(),
                    tint,
                    cutout,
                }
            }
        };
        self.looks.insert(key, look);
        look
    }

    /// The draws of a model posed at time `t` of its animation `anim`, placed by `place` (in the world's meters).
    pub fn model_draws(
        &mut self,
        c: &Content,
        m: &StudioModel,
        anim: Option<usize>,
        t: f32,
        place: &math::Mat4,
        label: &str,
    ) -> Vec<HdDraw> {
        let model = &m.model;
        let local = pose_at(model, anim.and_then(|a| model.animations.get(a)), t);
        let world = world_transforms(model, &local);
        let mut out = Vec::new();
        for mesh in &model.meshes {
            let joints: Vec<math::Mat4> = match &mesh.skin {
                Some(s) => s
                    .joints
                    .iter()
                    .zip(&s.inverse_binds)
                    .map(|(&j, ib)| math::mul(place, &math::mul(&world[j], ib)))
                    .collect(),
                None => vec![math::mul(place, &world[mesh.node])],
            };
            for prim in &mesh.primitives {
                let paths = prim
                    .material
                    .and_then(|i| m.material_paths.get(i))
                    .cloned()
                    .unwrap_or_default();
                let look = self.look(c, &paths, false);
                let mut d = look.draw(format!("{label} {}", mesh.name));
                let word = look.word(material::KIND_SHAPE);
                let mut map: HashMap<u32, u32> = HashMap::new();
                for &i in &prim.indices {
                    let n = *map.entry(i).or_insert_with(|| {
                        let k = i as usize;
                        let p = mesh.positions[k];
                        let nn = mesh.normals.get(k).copied().unwrap_or([0.0, 0.0, 1.0]);
                        let (mut wp, mut wn) = ([0f32; 3], [0f32; 3]);
                        if mesh.skin.is_some() {
                            let js = mesh.joints[k];
                            let ws = mesh.weights[k];
                            for s in 0..4 {
                                if ws[s] <= 0.0 {
                                    continue;
                                }
                                let Some(jm) = joints.get(usize::from(js[s])) else {
                                    continue;
                                };
                                let a = math::point(jm, p);
                                let b = math::vector(jm, nn);
                                for q in 0..3 {
                                    wp[q] += a[q] * ws[s];
                                    wn[q] += b[q] * ws[s];
                                }
                            }
                        } else {
                            wp = math::point(&joints[0], p);
                            wn = math::vector(&joints[0], nn);
                        }
                        d.vertices.push(HdVertex {
                            position: wp,
                            normal: normal(wn),
                            uv: mesh.uv0.get(k).copied().unwrap_or([0.0; 2]),
                            color: look.tint,
                            material: word,
                            extra: 0,
                        });
                        (d.vertices.len() - 1) as u32
                    });
                    d.indices.push(n);
                }
                if !d.indices.is_empty() {
                    out.push(d);
                }
            }
        }
        out
    }

    /// Draws `frames` frames from `eye` towards `target` and writes the last to `out`.
    pub fn shot(
        &mut self,
        eye: [f32; 3],
        target: [f32; 3],
        frames: usize,
        out: &Path,
    ) -> Result<(), String> {
        self.shot_lines(eye, target, frames, out, &[])
    }

    /// [`Renderer::shot`] with `lines` (world-space segments, drawn over the picture, `width` pixels wide) on the last frame.
    pub fn shot_lines(
        &mut self,
        eye: [f32; 3],
        target: [f32; 3],
        frames: usize,
        out: &Path,
        lines: &[GizmoSegment],
    ) -> Result<(), String> {
        let e = look_at(eye, target);
        let s = &mut self.scene.frame;
        s.eye = Some(e);
        s.fov = FOV;
        s.size = self.size;
        let camera = HdCamera::new(e, FOV, s.visible_distance, self.size.0, self.size.1);
        let mut frame = None;
        let mut gizmos = GizmoLines::new(&self.gpu);
        for i in 0..frames.max(1) {
            self.scene.frame.generation += 1;
            self.scene.time += 1.0 / 30.0;
            self.world.upload(&self.scene);
            self.world.draw(
                &self.scene,
                &camera,
                Some((
                    &self.target.color,
                    self.target.color.texture().format(),
                    [0, 0],
                )),
            )?;
            if i + 1 == frames.max(1) {
                gizmos.draw(
                    &self.gpu,
                    &GizmoFrame {
                        target: &self.target.color,
                        rect: [0, 0, self.size.0, self.size.1],
                        camera: &camera,
                        depth: None,
                        width: 2.0,
                    },
                    lines,
                );
            }
            frame = Some(
                self.gpu
                    .read_back(&self.target)
                    .map_err(|e| e.to_string())?,
            );
        }
        let f = frame.ok_or("no frame")?;
        crate::write_png(out, f.width, f.height, false, &f.rgb())
    }
}

/// An object of `draws` (in the world already).
pub fn object(id: u32, draws: Vec<HdDraw>) -> DynamicObject {
    let (center, radius) = torque_render_scene::hd::bounding_sphere(&draws);
    DynamicObject {
        ghost: id,
        transform: Mat::IDENTITY,
        prev_transform: Mat::IDENTITY,
        translucent: draws.iter().any(|d| d.blend != Blend::Opaque),
        centre: center,
        radius,
        draws,
        previous: None,
        stamp: None,
        nodes: Vec::new(),
    }
}

/// The box of every vertex of `draws`.
pub fn bounds(draws: &[HdDraw]) -> ([f32; 3], [f32; 3]) {
    let mut lo = [f32::INFINITY; 3];
    let mut hi = [f32::NEG_INFINITY; 3];
    for d in draws {
        for v in &d.vertices {
            for k in 0..3 {
                lo[k] = lo[k].min(v.position[k]);
                hi[k] = hi[k].max(v.position[k]);
            }
        }
    }
    (lo, hi)
}

/// A checkered ground square of `half` meters each way at height `z`.
pub fn ground(r: &mut Renderer, half: f32, z: f32) -> HdDraw {
    let mut rgba = Vec::with_capacity(64 * 64 * 4);
    for y in 0..64 {
        for x in 0..64 {
            let v = if (x / 32 + y / 32) % 2 == 0 { 150 } else { 110 };
            rgba.extend_from_slice(&[v, v, v, 255]);
        }
    }
    let t = r.scene.frame.textures.intern(Texture {
        path: "viewer-ground".into(),
        width: 64,
        height: 64,
        levels: Arc::new(vec![rgba]),
    });
    let look = Look {
        texture: Some(t),
        ..Look::PLAIN
    };
    let mut d = look.draw("ground".into());
    d.cull = Cull::None;
    let word = look.word(material::KIND_PLANE);
    for (x, y) in [(-half, -half), (half, -half), (half, half), (-half, half)] {
        d.vertices.push(HdVertex {
            position: [x, y, z],
            normal: oct_encode([0.0, 0.0, 1.0]),
            uv: [x / 2.0, y / 2.0],
            color: [255; 4],
            material: word,
            extra: 0,
        });
    }
    d.indices = vec![0, 1, 2, 0, 2, 3];
    d
}
