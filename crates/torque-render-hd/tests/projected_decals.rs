//! The projected decals against the CPU path's clipped triangles: the same decals (multiplied and alpha-blended) laid on flat
//! ground, over a corner and on a ball, drawn at High through the decal pass or as the CPU path's draws, land on the same pixels
//! with the same colors within a few levels. A moving shape standing in a decal's box takes none of it unless it takes decals,
//! a box around the eye still draws, a frame's boxes are drawn one instanced draw a texture, and Low draws none.

#[path = "../examples/decal_kit/mod.rs"]
mod decal_kit;

use decal_kit::{DecalTextures, Surface, decal};
use torque_render_core::{Frame, Gpu};
use torque_render_hd::ext::{NoExtension, PlainScene};
use torque_render_hd::{DECAL_FILL_BUDGET, DecalCounts, DecalProjection, HdWorld};
use torque_render_scene::Blend;
use torque_render_scene::decals::{DecalData, DecalManager, Triangle};
use torque_render_scene::hd::kit::{box_object, look_at};
use torque_render_scene::hd::{DynamicObject, HdCamera, HdMode, Quality};
use torque_render_scene::source::Eye;
use torque_render_scene::textures::TextureSet;

const SIZE: (u32, u32) = (320, 180);
/// A channel this far off is a pixel that differs.
const LEVELS: u8 = 6;

/// One decal laid: its datablock, the hit, the surface's normal there and the turn about it.
struct Hit {
    data: DecalData,
    at: [f32; 3],
    normal: [f32; 3],
    angle: f32,
}

fn hit(data: DecalData, at: [f32; 3], normal: [f32; 3], angle: f32) -> Hit {
    Hit {
        data,
        at,
        normal,
        angle,
    }
}

/// How the decals are drawn: the CPU path's clipped triangles, projected, or not at all (the picture without them).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Way {
    Cpu,
    Projected,
    None,
}

struct Setup<'a> {
    surfaces: &'a [Surface],
    objects: Vec<DynamicObject>,
    hits: &'a [Hit],
    eye: Eye,
    quality: Quality,
}

fn gpu() -> Option<Gpu> {
    match Gpu::new() {
        Ok(g) => Some(g),
        Err(e) => {
            eprintln!("skipped: no GPU ({e})");
            None
        }
    }
}

/// `setup` drawn `way` (four frames, no anti-aliasing: the two ways' pictures are compared texel for texel). None: the device
/// does not project decals.
fn draw(gpu: &Gpu, setup: &Setup<'_>, way: Way) -> Option<(Frame, DecalCounts)> {
    let mut s = PlainScene {
        mode: Some(HdMode::new(setup.quality)),
        ..PlainScene::default()
    };
    s.frame.sun.towards = [-0.35, -0.5, 0.79];
    s.frame.sun.diffuse = [0.75; 3];
    s.frame.sun.ambient = [0.4; 3];
    s.frame.planes = setup.surfaces.iter().map(|f| f.draw.clone()).collect();
    s.frame.planes_generation += 1;
    s.frame.objects = setup.objects.clone();
    s.frame.eye = Some(setup.eye);
    s.frame.fov = 70;
    s.frame.size = SIZE;
    let mut world: HdWorld<NoExtension> = HdWorld::new(gpu);
    world.features.anti_aliasing = false;
    world.features.projected_decals = match way {
        Way::Projected => DecalProjection::On,
        Way::Cpu | Way::None => DecalProjection::Off,
    };
    if way == Way::Projected && !world.projected_decals_at(setup.quality) {
        return None;
    }
    let triangles: Vec<Triangle> = setup
        .surfaces
        .iter()
        .flat_map(|f| f.triangles.iter().copied())
        .collect();
    let mut m = DecalManager::new(256, 25.0, 3.0);
    if way == Way::Projected {
        m.set_projected(Some(4096));
    }
    for h in setup.hits.iter().filter(|_| way != Way::None) {
        match way {
            Way::Projected => m.add_box(&h.data, h.at, h.normal, h.angle),
            _ => m.add_clipped(&h.data, h.at, h.normal, h.angle, &triangles),
        }
    }
    let mut set = TextureSet::new(&DecalTextures);
    if way == Way::Projected {
        let boxes = m.boxes(&mut set);
        s.frame.set_decals(&boxes, set);
    } else {
        let draws = m.draws(setup.eye.position, &mut set);
        s.frame.set_particles_and_decals(&[], &draws, set);
    }
    let camera = HdCamera::new(setup.eye, 70.0f32, 200.0, SIZE.0, SIZE.1);
    s.frame.generation += 1;
    world.upload(&s);
    for _ in 0..4 {
        world.draw(&s, &camera, None).expect("a frame");
    }
    Some((world.read_ldr().expect("the picture"), world.decal_counts()))
}

/// The share of `a`'s pixels a channel of which is more than `levels` from `b`'s, and how many those are.
fn off(a: &Frame, b: &Frame, levels: u8) -> (f64, usize) {
    let n = a
        .rgba
        .chunks(4)
        .zip(b.rgba.chunks(4))
        .filter(|(p, q)| {
            p.iter()
                .zip(*q)
                .take(3)
                .any(|(x, y)| x.abs_diff(*y) > levels)
        })
        .count();
    (n as f64 / (a.rgba.len() / 4) as f64, n)
}

/// The projected picture against the CPU path's: the decals cover at least `least` of the picture, and of the pixels they
/// cover at most `most` differ by more than [`LEVELS`].
fn matches_the_cpu_path(setup: &Setup<'_>, least: f64, most: f64, what: &str) {
    let Some(gpu) = gpu() else { return };
    let Some((projected, _)) = draw(&gpu, setup, Way::Projected) else {
        eprintln!("skipped: the device does not project decals");
        return;
    };
    let (cpu, _) = draw(&gpu, setup, Way::Cpu).expect("the CPU path");
    let (blank, _) = draw(&gpu, setup, Way::None).expect("no decals");
    let (covered, covered_n) = off(&cpu, &blank, 4);
    let (_, differ_n) = off(&projected, &cpu, LEVELS);
    let share = differ_n as f64 / covered_n.max(1) as f64;
    eprintln!(
        "{what}: the decals cover {:.2} % of the picture; {:.2} % of those pixels differ by more than {LEVELS} levels",
        covered * 100.0,
        share * 100.0
    );
    assert!(
        covered > least,
        "{what}: the decals cover {:.3} %",
        covered * 100.0
    );
    assert!(
        share < most,
        "{what}: {:.2} % of the decals' pixels differ by more than {LEVELS} levels",
        share * 100.0
    );
}

fn floor() -> Surface {
    Surface::block([0.0, 4.0, -0.5], [12.0, 12.0, 0.5], [150, 140, 125, 255])
}

#[test]
fn a_decal_on_flat_ground_matches_the_cpu_paths_picture() {
    let up = [0.0, 0.0, 1.0];
    let hits = [
        hit(
            decal("hole", [0.9, 0.9], Blend::Modulate2x),
            [-1.0, 3.0, 0.0],
            up,
            0.4,
        ),
        hit(
            decal("scorch", [1.6, 1.2], Blend::Modulate2x),
            [0.8, 4.0, 0.0],
            up,
            1.9,
        ),
        hit(
            decal("splat", [1.2, 1.2], Blend::Alpha),
            [-0.4, 5.5, 0.0],
            up,
            2.6,
        ),
        hit(
            decal("hole", [0.5, 0.5], Blend::Alpha),
            [1.8, 2.4, 0.0],
            up,
            0.0,
        ),
    ];
    let surfaces = [floor()];
    let setup = Setup {
        surfaces: &surfaces,
        objects: Vec::new(),
        hits: &hits,
        eye: look_at([0.0, -0.5, 3.0], [0.0, 4.0, 0.0]),
        quality: Quality::High,
    };
    matches_the_cpu_path(&setup, 0.03, 0.02, "flat ground");
}

#[test]
fn a_decal_over_a_corner_matches_the_cpu_paths_picture() {
    // A wall rising from the floor at y = 5: one hole laid on the floor at its foot (the wall faces away from it: the CPU path
    // cuts the wall off), one on the wall at the floor, one straddling the edge at 45 degrees.
    let wall = Surface::block([0.0, 5.5, 1.5], [6.0, 0.5, 1.5], [170, 165, 150, 255]);
    let surfaces = [floor(), wall];
    let s = std::f32::consts::FRAC_1_SQRT_2;
    let hits = [
        hit(
            decal("scorch", [1.6, 1.6], Blend::Modulate2x),
            [-1.5, 4.8, 0.0],
            [0.0, 0.0, 1.0],
            0.3,
        ),
        hit(
            decal("hole", [1.0, 1.0], Blend::Modulate2x),
            [0.0, 5.0, 0.3],
            [0.0, -1.0, 0.0],
            1.1,
        ),
        hit(
            decal("splat", [1.4, 1.4], Blend::Alpha),
            [1.6, 5.0, 0.0],
            [0.0, -s, s],
            0.7,
        ),
    ];
    let setup = Setup {
        surfaces: &surfaces,
        objects: Vec::new(),
        hits: &hits,
        eye: look_at([0.8, 1.0, 2.2], [0.0, 5.0, 0.4]),
        quality: Quality::High,
    };
    matches_the_cpu_path(&setup, 0.03, 0.03, "a corner");
}

#[test]
fn a_decal_on_a_curved_surface_matches_the_cpu_paths_picture() {
    let ball = Surface::ball([0.0, 4.0, 1.0], 1.0, [140, 150, 165, 255]);
    let surfaces = [floor(), ball];
    let n = |v: [f32; 3]| {
        let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
        v.map(|c| c / l)
    };
    let on = |d: [f32; 3]| {
        let d = n(d);
        ([d[0], 4.0 + d[1], 1.0 + d[2]], d)
    };
    let (a, na) = on([0.0, -1.0, 0.4]);
    let (b, nb) = on([-0.7, -0.6, -0.1]);
    let hits = [
        hit(decal("scorch", [1.1, 1.1], Blend::Modulate2x), a, na, 0.2),
        hit(decal("splat", [0.9, 0.9], Blend::Alpha), b, nb, 1.4),
    ];
    let setup = Setup {
        surfaces: &surfaces,
        objects: Vec::new(),
        hits: &hits,
        eye: look_at([0.0, 0.6, 1.8], [0.0, 4.0, 1.0]),
        quality: Quality::High,
    };
    matches_the_cpu_path(&setup, 0.02, 0.04, "a ball");
}

/// A box shape standing in a scorch's box over the floor (the scorch turned toward the eye, so the shape's top and front face
/// within the clip angle): it keeps its own colors where it takes no decals (the floor round it takes the scorch), and takes
/// the scorch where it does.
#[test]
fn a_moving_shape_in_a_decals_box_takes_none_unless_it_takes_decals() {
    let Some(gpu) = gpu() else { return };
    let hits = [hit(
        decal("scorch", [3.0, 3.0], Blend::Modulate2x),
        [0.0, 4.0, 0.0],
        [0.0, -0.6, 0.8],
        0.0,
    )];
    let surfaces = [floor()];
    let figure = |takes| DynamicObject {
        takes_decals: takes,
        ..box_object(7, [0.0, 4.0, 0.45], [0.25, 0.25, 0.45])
    };
    let setup = |takes| Setup {
        surfaces: &surfaces,
        objects: vec![figure(takes)],
        hits: &hits,
        eye: look_at([0.0, 1.0, 1.4], [0.0, 4.0, 0.4]),
        quality: Quality::High,
    };
    let Some((bare, _)) = draw(&gpu, &setup(false), Way::Projected) else {
        eprintln!("skipped: the device does not project decals");
        return;
    };
    let (blank, _) = draw(&gpu, &setup(false), Way::None).expect("no decals");
    let (taken, _) = draw(&gpu, &setup(true), Way::Projected).expect("projected");
    // The figure's pixels: those of its own color in the picture without decals.
    let figure_px: Vec<usize> = blank
        .rgba
        .chunks(4)
        .enumerate()
        .filter(|(_, p)| p[0] > p[2].saturating_add(40))
        .map(|(i, _)| i)
        .collect();
    assert!(figure_px.len() > 500, "{} figure pixels", figure_px.len());
    let changed = |f: &Frame| {
        figure_px
            .iter()
            .filter(|&&i| (0..3).any(|k| f.rgba[4 * i + k].abs_diff(blank.rgba[4 * i + k]) > 4))
            .count() as f64
            / figure_px.len() as f64
    };
    let (floor_off, _) = off(&bare, &blank, 4);
    assert!(floor_off > 0.03, "the floor round it takes the scorch");
    assert!(
        changed(&bare) < 0.005,
        "{:.2} % of the figure changed",
        changed(&bare) * 100.0
    );
    assert!(
        changed(&taken) > 0.3,
        "{:.2} % of the figure changed",
        changed(&taken) * 100.0
    );
}

/// A decal whose box holds the eye still draws (its inside faces), and a frame's decals are one instanced draw a run of one
/// texture and blend.
#[test]
fn a_box_around_the_eye_draws_and_the_boxes_are_one_draw_a_texture() {
    let Some(gpu) = gpu() else { return };
    let up = [0.0, 0.0, 1.0];
    let mut hits = vec![hit(
        decal("scorch", [8.0, 8.0], Blend::Modulate2x),
        [0.0, 2.0, 0.0],
        up,
        0.0,
    )];
    for i in 0..60 {
        let name = ["hole", "splat"][i % 2];
        let blend = [Blend::Modulate2x, Blend::Alpha][(i / 2) % 2];
        let at = [
            (i % 10) as f32 * 0.6 - 2.7,
            3.0 + (i / 10) as f32 * 0.6,
            0.0,
        ];
        hits.push(hit(decal(name, [0.4, 0.4], blend), at, up, i as f32));
    }
    let surfaces = [floor()];
    let setup = Setup {
        surfaces: &surfaces,
        objects: Vec::new(),
        hits: &hits,
        eye: look_at([0.0, 0.5, 1.6], [0.0, 4.0, 0.0]),
        quality: Quality::High,
    };
    let Some((projected, DecalCounts { boxes, draws, .. })) = draw(&gpu, &setup, Way::Projected)
    else {
        eprintln!("skipped: the device does not project decals");
        return;
    };
    // A few at the sides are out of the view.
    assert!((50..=61).contains(&boxes), "{boxes} boxes drawn");
    // The scorch's run, then a run each of hole and splat multiplied and alpha-blended.
    assert_eq!(draws, 5);
    let (blank, _) = draw(&gpu, &setup, Way::None).expect("no decals");
    let (changed, _) = off(&projected, &blank, 4);
    assert!(
        changed > 0.3,
        "the big scorch around the eye covers {:.1} %",
        changed * 100.0
    );
}

/// At Low the decals are the CPU path's: boxes in the scene draw nothing there.
#[test]
fn low_projects_no_decal() {
    let Some(gpu) = gpu() else { return };
    let hits = [hit(
        decal("scorch", [2.0, 2.0], Blend::Modulate2x),
        [0.0, 4.0, 0.0],
        [0.0, 0.0, 1.0],
        0.0,
    )];
    let surfaces = [floor()];
    let setup = Setup {
        surfaces: &surfaces,
        objects: Vec::new(),
        hits: &hits,
        eye: look_at([0.0, 0.5, 2.0], [0.0, 4.0, 0.0]),
        quality: Quality::Low,
    };
    let world: HdWorld<NoExtension> = HdWorld::new(&gpu);
    assert!(!world.projected_decals_at(Quality::Low));
    assert!(
        world.projected_decals_at(Quality::Medium) || !world.projected_decals_at(Quality::Ultra)
    );
    // Boxes handed to a Low frame (the feature at its default) leave the picture as it is without them.
    let mut s = PlainScene {
        mode: Some(HdMode::new(Quality::Low)),
        ..PlainScene::default()
    };
    s.frame.planes = surfaces.iter().map(|f| f.draw.clone()).collect();
    s.frame.planes_generation += 1;
    s.frame.eye = Some(setup.eye);
    s.frame.fov = 70;
    s.frame.size = SIZE;
    let camera = HdCamera::new(setup.eye, 70.0f32, 200.0, SIZE.0, SIZE.1);
    let picture = |boxes: bool| {
        let mut s = s.clone();
        let mut world: HdWorld<NoExtension> = HdWorld::new(&gpu);
        if boxes {
            let mut m = DecalManager::new(256, 25.0, 3.0);
            m.set_projected(Some(16));
            for h in &hits {
                m.add_box(&h.data, h.at, h.normal, h.angle);
            }
            let mut set = TextureSet::new(&DecalTextures);
            let b = m.boxes(&mut set);
            assert_eq!(b.len(), 1);
            s.frame.set_decals(&b, set);
        }
        s.frame.generation += 1;
        world.upload(&s);
        world.draw(&s, &camera, None).expect("a frame");
        (world.read_ldr().expect("the picture"), world.decal_counts())
    };
    let (with, counts) = picture(true);
    let (without, _) = picture(false);
    assert_eq!(counts, DecalCounts::default());
    assert!(
        with.rgba == without.rgba,
        "a Low frame with boxes is the frame without them"
    );
}

/// Thousands of the largest decals a server can send (64 units across), all around the eye: the boxes drawn cover at most the
/// fill budget's screens (each holds the eye: a whole screen), the rest are left out and counted, and the frame is drawn.
#[test]
fn decals_around_the_eye_are_held_to_the_fill_budget() {
    let Some(gpu) = gpu() else { return };
    let mut hits = Vec::new();
    for i in 0..4000 {
        let a = i as f32 * 2.399;
        let r = (i % 50) as f32 * 0.1;
        let at = [r * a.cos(), 2.0 + r * a.sin(), 0.0];
        hits.push(hit(
            decal("scorch", [64.0, 64.0], Blend::Modulate2x),
            at,
            [0.0, 0.0, 1.0],
            a,
        ));
    }
    let surfaces = [floor()];
    let setup = Setup {
        surfaces: &surfaces,
        objects: Vec::new(),
        hits: &hits,
        eye: look_at([0.0, 0.5, 1.6], [0.0, 4.0, 0.0]),
        quality: Quality::High,
    };
    let Some((_, counts)) = draw(&gpu, &setup, Way::Projected) else {
        eprintln!("skipped: the device does not project decals");
        return;
    };
    assert!(counts.boxes >= 1, "{counts:?}");
    assert!(counts.boxes as f32 <= DECAL_FILL_BUDGET, "{counts:?}");
    assert_eq!(counts.boxes + counts.dropped, 4000, "{counts:?}");
}
