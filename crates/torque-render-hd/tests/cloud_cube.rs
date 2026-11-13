//! The clouds' cube: the water reflects the volumetric clouds the main view does not see (the camera pitched down at it, the
//! sky it reflects all above the view), and where it does see them the water keeps the main view's own image of them.

use std::sync::Arc;

use torque_render_hd::ext::{NoExtension, PlainScene};
use torque_render_hd::{HdWorld, non_finite};
use torque_render_scene::hd::cells::StaticCells;
use torque_render_scene::hd::kit::look_at;
use torque_render_scene::hd::scatter::{ScatterLook, ScatterTable};
use torque_render_scene::hd::{HdCamera, HdDraw, HdMode, HdScene, Quality, material};
use torque_render_scene::{Blend, Cull, Draw, TexEnv, Texture, Vertex};
use torque_scene::time_of_day::{DayFields, TimeOfDay};

const SIZE: (u32, u32) = (192, 108);
const FOV: u8 = 60;
const EYE: f32 = 20.0;
/// Frames a picture settles over (the cube's five faces drawn in turn, the histories filled).
const FRAMES: usize = 16;

fn water() -> Draw {
    let w = 3000.0;
    let corner = |x: f32, y: f32| Vertex {
        position: [x, y, 0.0],
        normal: [0.0, 0.0, 1.0],
        color: [40, 80, 90, 160],
        uv: [0.0; 2],
    };
    let (a, b, c, d) = (corner(-w, -w), corner(w, -w), corner(w, w), corner(-w, w));
    Draw {
        label: "water".into(),
        vertices: vec![a, b, c, a, c, d],
        lighting: true,
        tex_env: TexEnv::None,
        texture: None,
        blend: Blend::Alpha,
        cull: Cull::None,
        depth_test: true,
        depth_write: false,
        fog: true,
        lights: Vec::new(),
        vertex_key: None,
    }
}

fn scene() -> PlainScene {
    let mut scene = PlainScene {
        frame: HdScene::new(StaticCells::default()),
        mode: Some(HdMode::new(Quality::High)),
        time: 0.0,
    };
    let s = &mut scene.frame;
    s.atmosphere = true;
    let _ = s.textures.intern(Texture {
        format: torque_render_scene::TexelFormat::Rgba8,
        path: "white".into(),
        width: 1,
        height: 1,
        levels: Arc::new(vec![vec![255; 4]]),
    });
    s.water = vec![HdDraw::from_fixed(&water(), material::KIND_PLANE, &[])];
    s.visible_distance = 3000.0;
    s.fog.start = 1500.0;
    s.fog.end = 3000.0;
    s.fog_distance = 1500.0;
    // An afternoon sun ahead and to the side.
    let mut day = TimeOfDay::new(DayFields {
        azimuth_override: 120f32.to_radians(),
        play: false,
        ..DayFields::default()
    });
    day.set_time(0.38);
    let sun = day.towards();
    let light = ScatterTable::earth().light(sun, &ScatterLook::default());
    s.sun = light.sun();
    s.fog = light.fog(s.fog);
    s.clear = light.fog;
    s.atmosphere_sun = Some(sun);
    s.clouds = Some(light.clouds([1.0, 0.4, 0.0]));
    scene
}

/// The HDR frame after [`FRAMES`] frames from the eye pitched `pitch` degrees (down when positive) toward +x, with the clouds'
/// cube on or off.
fn picture(
    world: &mut HdWorld<NoExtension>,
    scene: &mut PlainScene,
    pitch: f32,
    cube: bool,
) -> Vec<[f32; 4]> {
    let p = pitch.to_radians();
    let at = [0.0, 0.0, EYE];
    let eye = look_at(at, [100.0 * p.cos(), 0.0, EYE - 100.0 * p.sin()]);
    scene.frame.eye = Some(eye);
    scene.frame.fov = FOV;
    scene.frame.size = SIZE;
    let camera = HdCamera::new(eye, FOV, scene.frame.visible_distance, SIZE.0, SIZE.1);
    world.features.cloud_cube = cube;
    world.reset_history();
    scene.time = 0.0;
    for _ in 0..FRAMES {
        scene.frame.generation += 1;
        world.upload(scene);
        world.draw(scene, &camera, None).expect("draw");
    }
    world.read_hdr().expect("hdr").2
}

fn luminance(c: &[f32; 4]) -> f32 {
    0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]
}

#[test]
fn the_water_reflects_the_clouds_off_screen_from_the_cube_and_keeps_those_on_screen() {
    let gpu = match torque_render_core::Gpu::new() {
        Ok(g) => g,
        Err(e) => {
            eprintln!("skipped: no GPU ({e})");
            return;
        }
    };
    let mut world: HdWorld<NoExtension> = HdWorld::new(&gpu);
    let mut scene = scene();
    // Pitched 50 degrees down: the view's top edge is 30 degrees under the horizon, and the water reflects the sky from 30
    // degrees up to past the zenith, none of it on screen. With the cube its clouds change the water; without it nothing
    // stands in for them.
    let on = picture(&mut world, &mut scene, 50.0, true);
    let off = picture(&mut world, &mut scene, 50.0, false);
    assert_eq!(non_finite(&on), 0);
    let mean = |v: &[[f32; 4]]| v.iter().map(luminance).sum::<f32>() / v.len() as f32;
    let changed = on
        .iter()
        .zip(&off)
        .map(|(a, b)| (luminance(a) - luminance(b)).abs())
        .sum::<f32>()
        / on.len() as f32;
    assert!(
        changed > 0.05 * mean(&off),
        "the clouds off screen change the water by {changed} (its mean {})",
        mean(&off)
    );
    // Pitched 10 degrees up: the water just under the horizon reflects the sky just over it, well inside the view. There the
    // water keeps the main view's own image of the clouds: the cube changes nothing.
    let on = picture(&mut world, &mut scene, -10.0, true);
    let off = picture(&mut world, &mut scene, -10.0, false);
    let (w, h) = SIZE;
    // The horizon's row: the view's center is 10 degrees up, the vertical half-angle from the horizontal one.
    let half = (f32::from(FOV).to_radians() / 2.0).tan() * h as f32 / w as f32;
    let horizon = h as f32 / 2.0 * (1.0 + 10f32.to_radians().tan() / half);
    let rows = (horizon as u32 + 3)..(horizon as u32 + 8);
    let mut worst = 0.0f32;
    for y in rows {
        for x in w / 4..3 * w / 4 {
            let i = (y * w + x) as usize;
            worst = worst
                .max((luminance(&on[i]) - luminance(&off[i])).abs() / luminance(&off[i]).max(1e-3));
        }
    }
    assert!(
        worst < 1e-3,
        "the cube changed the on-screen clouds' reflection by {worst}"
    );
}

#[test]
fn the_cube_draws_no_face_while_the_eye_is_under_the_water_and_resumes_after() {
    let gpu = match torque_render_core::Gpu::new() {
        Ok(g) => g,
        Err(e) => {
            eprintln!("skipped: no GPU ({e})");
            return;
        }
    };
    let mut world: HdWorld<NoExtension> = HdWorld::new(&gpu);
    let mut scene = scene();
    world.features.cloud_cube = true;
    let mut frame = |world: &mut HdWorld<NoExtension>, height: f32| {
        let eye = look_at([0.0, 0.0, height], [100.0, 0.0, height]);
        scene.frame.eye = Some(eye);
        scene.frame.fov = FOV;
        scene.frame.size = SIZE;
        let camera = HdCamera::new(eye, FOV, scene.frame.visible_distance, SIZE.0, SIZE.1);
        scene.frame.generation += 1;
        world.upload(&scene);
        world.draw(&scene, &camera, None).expect("draw");
        world.cloud_cube_state()
    };
    // Above the water the faces are drawn in turn until the cube holds them all.
    let mut faces = Vec::new();
    for _ in 0..FRAMES {
        let (face, _) = frame(&mut world, EYE);
        faces.extend(face);
    }
    assert!(faces.len() >= 5, "faces drawn above the water: {faces:?}");
    assert!(
        frame(&mut world, EYE).1,
        "the cube is ready above the water"
    );
    // Under it none is drawn, and the cube keeps what it holds. (The eye dives in steps: a jump would be a cut that starts the
    // histories again.)
    let step = 1.0;
    let mut height = EYE;
    while height > -EYE {
        height -= step;
        let (face, ready) = frame(&mut world, height);
        if height < 0.0 {
            assert_eq!(face, None, "a face drawn with the eye under the water");
        }
        assert!(ready, "the cube was dropped at height {height}");
    }
    // Back above, the faces come again.
    while height < EYE - step {
        height += step;
        frame(&mut world, height);
    }
    let (face, ready) = frame(&mut world, EYE);
    assert!(face.is_some(), "no face drawn after the eye came up");
    assert!(ready, "the cube was not ready when the eye came up");
}
