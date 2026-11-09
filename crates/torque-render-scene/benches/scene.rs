//! The frame's vector math: particles, decals, the sky's SH shape, the interiors' merged meshes and a frame of moving ghosts.

use std::collections::HashMap;
use std::hint::black_box;
use std::sync::atomic::Ordering;

use criterion::{Criterion, criterion_group, criterion_main};
use ghost_world::tge::{TgeClientSet, TgeDatablock};
use ghost_world::world::{ClientWorld, Ghost};
use ghosts::shapes::StaticShape;
use ghosts::{F32, TgeClass, TgeRecord, TgeShapeBase, Xf};
use torque_interior_builder::{Brush, Options, TexGen, build};
use torque_render_scene::decals::{DecalData, DecalManager, Triangle};
use torque_render_scene::hd::objects::{KeptObject, objects_of};
use torque_render_scene::hd::{DynamicObject, HdScene, TextureTable, sh};
use torque_render_scene::map::{
    self, MapClass, MapInput, MapProfile, MapRecords, MapValue, Object,
};
use torque_render_scene::moving::{self as moving, ShapeSelect, draws_culled};
use torque_render_scene::particles::{Emitter, EmitterData, ParticleData, Rng};
use torque_render_scene::shapecache::with_cache;
use torque_render_scene::shapes::DetailPolicy;
use torque_render_scene::textures::TextureSet;
use torque_render_scene::world::Value;
use torque_render_scene::{Assets, Blend, Cull, Draw, Sun, TexEnv, Vertex};
use torque_shape_builder::{FULL_UV, MeshData, ShapeBuilder};

fn emitter() -> Emitter {
    let particle = ParticleData {
        drag: 0.5,
        wind: 0.0,
        gravity: 0.3,
        inherited_vel: 0.0,
        constant_accel: 0.1,
        lifetime_ms: 60_000,
        lifetime_variance_ms: 0,
        spin_speed: 10.0,
        spin_min: -20.0,
        spin_max: 20.0,
        inv_alpha: false,
        colors: [[1.0; 4]; 4],
        sizes: [1.0, 2.0, 3.0, 4.0],
        times: [0.0, 0.3, 0.6, 1.0],
        textures: Vec::new(),
        animate: false,
        fps: 0,
    };
    let data = EmitterData {
        period_ms: 1,
        period_variance_ms: 0,
        velocity: 4.0,
        velocity_variance: 1.0,
        offset: 0.2,
        theta_min: 0.0,
        theta_max: 90.0,
        phi_ref_vel: 0.0,
        phi_variance: 360.0,
        override_advance: false,
        lifetime_ms: 0,
        use_emitter_sizes: false,
        use_emitter_colors: false,
        particles: vec![1],
    };
    Emitter::of_data(data, vec![particle]).expect("a particle datablock")
}

/// 10,000 particles: five bursts of 2,000.
fn burst_10k(e: &mut Emitter, live: &mut usize) {
    let mut rng = Rng::new(7);
    for i in 0..5 {
        let normal = [0.3 * i as f32, 0.2, 1.0];
        e.emit_burst([i as f32, 0.0, 2.0], normal, 1.5, 2_000, &mut rng, live);
    }
}

fn particles(c: &mut Criterion) {
    c.bench_function("particles emit 10k", |b| {
        b.iter(|| {
            let (mut e, mut live) = (emitter(), 0);
            burst_10k(&mut e, &mut live);
            black_box(e.particles.len())
        })
    });
    let (mut full, mut live) = (emitter(), 0);
    burst_10k(&mut full, &mut live);
    c.bench_function("particles advance 10k", |b| {
        b.iter_batched_ref(
            || (full.clone(), live),
            |(e, live)| {
                e.advance(32, live);
                black_box(e.centre())
            },
            criterion::BatchSize::LargeInput,
        )
    });
}

/// A wavy surface of `n` triangles under the decal's box.
fn surface(n: usize) -> Vec<Triangle> {
    let side = ((n / 2) as f32).sqrt().ceil() as usize;
    let step = 2.0 / side as f32;
    let z = |x: f32, y: f32| 0.1 * (x * 3.0).sin() * (y * 2.0).cos();
    let p = |i: usize, j: usize| {
        let (x, y) = (-1.0 + i as f32 * step, -1.0 + j as f32 * step);
        [x, y, z(x, y)]
    };
    let mut out = Vec::with_capacity(n);
    'grid: for i in 0..side {
        for j in 0..side {
            for t in [
                [p(i, j), p(i + 1, j), p(i + 1, j + 1)],
                [p(i, j), p(i + 1, j + 1), p(i, j + 1)],
            ] {
                if out.len() == n {
                    break 'grid;
                }
                out.push(t);
            }
        }
    }
    out
}

fn decals(c: &mut Criterion) {
    let tris = surface(1_000);
    let data = DecalData {
        size: [1.5, 1.5],
        texture: None,
        blend: Blend::Alpha,
    };
    c.bench_function("decal clip 1k triangles", |b| {
        b.iter(|| {
            let mut m = DecalManager::new(4, 10.0, 1.0);
            m.add_clipped(&data, [0.1, 0.2, 0.0], [0.1, 0.0, 1.0], 0.4, &tris);
            black_box(m.decals().map(|d| d.triangles.len()).sum::<usize>())
        })
    });
}

/// A sky dome of 64 wedges, white above the horizon.
fn dome() -> Draw {
    let mut v = Vec::new();
    let r = 100.0;
    let wedges = 64;
    for k in 0..wedges {
        let a0 = k as f32 * std::f32::consts::TAU / wedges as f32;
        let a1 = (k + 1) as f32 * std::f32::consts::TAU / wedges as f32;
        let p = |a: f32, z: f32| Vertex {
            position: [r * a.cos(), r * a.sin(), z],
            normal: [0.0, 0.0, 1.0],
            color: [255; 4],
            uv: [0.0; 2],
        };
        let mut apex = p(0.0, r);
        apex.position = [0.0, 0.0, r];
        v.extend([p(a0, 0.0), p(a1, 0.0), apex]);
    }
    Draw {
        label: "sky".into(),
        vertices: v,
        lighting: false,
        tex_env: TexEnv::None,
        texture: None,
        blend: Blend::Opaque,
        cull: Cull::None,
        depth_test: false,
        depth_write: false,
        fog: false,
        lights: Vec::new(),
        vertex_key: None,
    }
}

fn sh_lighting(c: &mut Criterion) {
    let sky = [dome()];
    let table = TextureTable::default();
    c.bench_function("sh sky shape", |b| {
        b.iter(|| black_box(sh::sky_shape(&sky, &table, &[], [0.0; 3], [0.1; 3])))
    });
    let shape = sh::sky_shape(&sky, &table, &[], [0.0; 3], [0.1; 3]);
    c.bench_function("sh eval 10k", |b| {
        b.iter(|| {
            let mut sum = 0.0;
            for n in 0..10_000 {
                sum += sh::eval(&shape, sh::direction(n))[1];
            }
            black_box(sum)
        })
    });
}

fn cube(center: [f64; 3], size: [f64; 3]) -> Brush {
    let min = [0, 1, 2].map(|k| center[k] - size[k] * 0.5);
    let max = [0, 1, 2].map(|k| center[k] + size[k] * 0.5);
    Brush::cuboid(min.into(), max.into(), "wall", |n| TexGen::world(n, 4.0))
}

/// A walled floor with a ring of blocks.
fn yard_dif() -> Vec<u8> {
    let mut brushes = vec![
        cube([0.0, 0.0, -0.5], [40.0, 40.0, 1.0]),
        cube([0.0, 20.5, 1.5], [42.0, 1.0, 3.0]),
        cube([0.0, -20.5, 1.5], [42.0, 1.0, 3.0]),
        cube([20.5, 0.0, 1.5], [1.0, 40.0, 3.0]),
        cube([-20.5, 0.0, 1.5], [1.0, 40.0, 3.0]),
    ];
    for i in 0..16 {
        let a = f64::from(i) * std::f64::consts::TAU / 16.0;
        brushes.push(cube([a.cos() * 9.0, a.sin() * 9.0, 1.2], [1.5, 2.5, 2.4]));
    }
    build(&brushes, &Options::default())
        .and_then(|b| b.bytes())
        .expect("the yard builds")
}

struct Files(Vec<u8>);

impl Assets for Files {
    fn read(&self, path: &str) -> Option<Vec<u8>> {
        path.eq_ignore_ascii_case("bench/yard.dif")
            .then(|| self.0.clone())
    }
}

fn interiors(c: &mut Criterion) {
    map::light::ENABLED.store(false, Ordering::Relaxed);
    let files = Files(yard_dif());
    let mut objects = Vec::new();
    for i in 0..4 {
        for j in 0..4 {
            objects.push(Object::new(
                MapClass::InteriorInstance,
                [i as f32 * 50.0, j as f32 * 50.0, 0.0],
                [1.0, 1.0 + 0.1 * j as f32, 1.0],
                [("interiorFile", MapValue::Text(b"bench/yard.dif".to_vec()))],
            ));
        }
    }
    let records = MapRecords {
        objects,
        statics: Vec::new(),
    };
    let profile = MapProfile::TGE;
    let input = MapInput {
        records: &records,
        profile: &profile,
        shapes: &(),
    };
    let sun = Sun {
        towards: [0.3, 0.2, 0.9],
        diffuse: [0.8; 3],
        ambient: [0.3; 3],
    };
    c.bench_function("interior draws 16 instances", |b| {
        b.iter(|| {
            map::forget_built();
            let mut textures = TextureSet::new(&files);
            let mut notes = Vec::new();
            let d = map::draws(
                input,
                &files,
                [0.0; 3],
                1_000.0,
                sun,
                &mut textures,
                &mut notes,
                None,
            );
            black_box(d.opaque.len() + d.translucent.len())
        })
    });
}

/// 10,000 particles drawn (one draw facing the camera) and handed to the enhanced renderer's frame with a decal: what a frame
/// with a burst alive converts.
fn particles_draw(c: &mut Criterion) {
    let (mut e, mut live) = (emitter(), 0);
    burst_10k(&mut e, &mut live);
    let files = Shapes(HashMap::new());
    let mut scene = HdScene::new(());
    c.bench_function("particles draw+upload 10k", |b| {
        b.iter(|| {
            let mut textures = TextureSet::new(&files);
            let Some(d) = e.draw([1.0, 0.0, 0.0], [0.0, 0.0, 1.0], &mut textures, "burst") else {
                return 0;
            };
            scene.set_particles_and_decals(&[(5.0, d)], &[], textures);
            black_box(scene.particles.len())
        })
    });
}

/// The shape files of the ghost benches, by lowercase path.
struct Shapes(HashMap<String, Vec<u8>>);

impl Assets for Shapes {
    fn read(&self, path: &str) -> Option<Vec<u8>> {
        self.0.get(&path.to_ascii_lowercase()).cloned()
    }
}

const FIGURE: &str = "bench/figure.dts";

/// A figure of ten nodes, twenty boxes each (4,800 vertices).
fn figure_dts() -> Vec<u8> {
    let mut b = ShapeBuilder::new();
    let root = b.node("root", None, [0.0; 3]);
    let detail = b.detail("detail1", 1.0);
    let m = b.material("figure", 0);
    for n in 0..10 {
        let node = b.node(&format!("part{n}"), Some(root), [0.0, 0.0, n as f32 * 0.2]);
        let mut mesh = MeshData::new(Some(m));
        for k in 0..20 {
            let x = k as f32 * 0.05 - 0.5;
            mesh.add_box([x, -0.1, 0.0], [x + 0.04, 0.1, 0.18], |_| FULL_UV);
        }
        b.object(&format!("part{n}"), node, detail, mesh);
    }
    shapes::write::write_dts(&b.build())
}

/// The record of a StaticShape at `at`.
fn static_shape(at: [f32; 3]) -> TgeRecord {
    TgeRecord::StaticShape(StaticShape {
        shape: TgeShapeBase {
            game: ghosts::parts::GameBase {
                scale: None,
                datablock: Some(3),
            },
            body: None,
        },
        transform: Some((
            Xf {
                position: at.map(F32::new),
                quat: [F32::new(0.0); 3],
                negative_w: false,
            },
            [F32::new(1.0); 3],
        )),
        powered: false,
    })
}

/// 128 figures in a row along x, ten units apart; the frame `frame` moves every fourth one a little (a quarter of them change).
fn ghost_world(frame: u32) -> ClientWorld<TgeClientSet> {
    let mut w = ClientWorld::<TgeClientSet>::default();
    w.datablocks.insert(
        3,
        TgeDatablock {
            class: "StaticShapeData".to_string(),
            fields: vec![("shapeFile".to_string(), 0, Value::str(FIGURE))],
        },
    );
    for i in 0..128u32 {
        let step = if i % 4 == 0 { frame as f32 * 0.01 } else { 0.0 };
        let ghost = Ghost {
            class: Some(TgeClass::StaticShape),
            serial: i as usize + 1,
            always: false,
            record: Some(static_shape([i as f32 * 10.0, step, 0.0])),
            part: (),
            position: None,
            updates: 0,
        };
        w.ghosts.insert(i + 1, ghost);
    }
    w
}

/// The first tenth of the row (x below 128) is in view.
struct FirstTenth;

impl moving::Cull for FirstTenth {
    fn keeps(&self, centre: [f32; 3], radius: f32) -> bool {
        centre[0] - radius < 128.0
    }
}

/// One frame of the ghosts' objects: built (kept builds and kept objects with `kept`), converted, and each given its motion from
/// the last frame's object (`drawn`, replaced).
fn ghost_frame(
    w: &ClientWorld<TgeClientSet>,
    files: &Shapes,
    drawn: &mut Vec<DynamicObject>,
    kept: Option<&mut HashMap<u32, KeptObject>>,
    cull: Option<&dyn moving::Cull>,
) -> usize {
    let mut textures = TextureSet::new(files);
    let mut notes = Vec::new();
    let shaped = draws_culled(
        w,
        &(),
        files,
        &mut textures,
        &mut notes,
        DetailPolicy::Highest,
        ShapeSelect::All,
        cull,
    );
    let local: Vec<usize> = (0..textures.into_textures().len()).collect();
    let mut out = objects_of(w, &shaped, &local, drawn, kept);
    torque_render_scene::hd::objects::carry_motion(&mut out, drawn);
    let n = out.len();
    *drawn = out;
    n
}

fn ghosts(c: &mut Criterion) {
    let files = Shapes([(FIGURE.to_string(), figure_dts())].into());
    let worlds = [ghost_world(0), ghost_world(1)];
    let mut drawn = Vec::new();
    let n = ghost_frame(&worlds[0], &files, &mut drawn, None, None);
    assert_eq!(n, 128, "every figure is drawn");
    let mut frame = 0usize;
    c.bench_function("ghost frame 128 rebuilt", |b| {
        b.iter(|| {
            frame += 1;
            black_box(ghost_frame(
                &worlds[frame % 2],
                &files,
                &mut drawn,
                None,
                None,
            ))
        })
    });
    let mut drawn = Vec::new();
    let mut kept = HashMap::new();
    c.bench_function("ghost frame 128 kept and culled", |b| {
        b.iter(|| {
            frame += 1;
            with_cache(|| {
                black_box(ghost_frame(
                    &worlds[frame % 2],
                    &files,
                    &mut drawn,
                    Some(&mut kept),
                    Some(&FirstTenth),
                ))
            })
        })
    });
}

criterion_group!(
    benches,
    particles,
    particles_draw,
    decals,
    sh_lighting,
    interiors,
    ghosts
);
criterion_main!(benches);
