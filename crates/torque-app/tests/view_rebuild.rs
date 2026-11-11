//! A world view's GPU world emptied and made again, drawn offscreen over a small scene (a floor, a static cube, a moving cube
//! and a light): the picture after a restore is the one before the release, and what the scene holds survives.

use std::rc::Rc;

use torque_app::renderers::{RenderFeatures, RendererEntry, RendererPlugin, RendererSlot};
use torque_app::view::{Daylight, WorldView, static_mesh_key};
use torque_engine::{Engine, EngineBuilder};
use torque_render_core::{Frame, Gpu};
use torque_render_hd::HdWorld;
use torque_render_scene::hd::HdLight;
use torque_render_scene::hd::kit::look_at;
use torque_render_scene::source::Eye;
use torque_render_scene::textures::TextureSet;
use torque_render_scene::{Assets, Blend, Cull, Draw, TexEnv, Vertex};

const SIZE: (u32, u32) = (160, 120);
/// Frames each picture settles for (the anti-aliasing's history).
const FRAMES: u32 = 6;

fn gpu() -> Option<Gpu> {
    match torque_app::window::offscreen_gpu() {
        Ok(gpu) => Some(gpu),
        Err(e) => {
            eprintln!("skipped: {e}");
            None
        }
    }
}

/// No texture files: the cubes are colored.
struct NoFiles;

impl Assets for NoFiles {
    fn read(&self, _path: &str) -> Option<Vec<u8>> {
        None
    }
}

fn eye() -> Eye {
    look_at([0.0, -9.0, 4.0], [0.0, 0.0, 1.0])
}

/// A cube of side `2 * half` about `at`, its sides colored `color`.
fn cube(at: [f32; 3], half: f32, color: [u8; 3]) -> Draw {
    let mut vertices = Vec::new();
    for axis in 0..3 {
        for sign in [-1.0_f32, 1.0] {
            let (u, v) = ((axis + 1) % 3, (axis + 2) % 3);
            let corner = |a: f32, b: f32| {
                let mut p = at;
                p[axis] += sign * half;
                p[u] += a * half;
                p[v] += b * half;
                let mut normal = [0.0; 3];
                normal[axis] = sign;
                Vertex {
                    position: p,
                    normal,
                    color: [color[0], color[1], color[2], 255],
                    uv: [0.0, 0.0],
                }
            };
            let quad = [
                corner(-1.0, -1.0),
                corner(1.0, -1.0),
                corner(1.0, 1.0),
                corner(-1.0, 1.0),
            ];
            vertices.extend([quad[0], quad[1], quad[2], quad[0], quad[2], quad[3]]);
        }
    }
    Draw {
        label: "cube".to_string(),
        vertices,
        lighting: true,
        tex_env: TexEnv::None,
        texture: None,
        blend: Blend::Opaque,
        cull: Cull::None,
        depth_test: true,
        depth_write: true,
        fog: true,
        lights: Vec::new(),
        vertex_key: None,
    }
}

/// A floor, a static cube, a moving cube (the object `7`, at `x`) and a red light by them.
fn view(gpu: &Gpu, x: f32) -> WorldView {
    let mut view = WorldView::new(gpu);
    view.set_plain_floor(50.0, [120, 160, 90]);
    view.set_daylight(&Daylight::default());
    view.set_effect_frame_seconds(Some(1.0 / 60.0));
    view.renderer().set_unbudgeted();
    view.set_static_mesh(
        static_mesh_key(0),
        TextureSet::new(&NoFiles),
        &[cube([-3.0, 0.0, 1.0], 1.0, [200, 60, 60])],
    );
    moving(&mut view, x);
    view.scene_mut().lights.push(HdLight {
        position: [0.0, -2.0, 3.0],
        color: [1.0, 0.3, 0.2],
        radius: 14.0,
        carrier: None,
    });
    view.scene_mut().lights_generation += 1;
    view
}

/// The moving cube at `x`.
fn moving(view: &mut WorldView, x: f32) {
    let object = view.mesh_object(
        TextureSet::new(&NoFiles),
        &[cube([x, 0.0, 1.0], 1.0, [60, 80, 220])],
        7,
    );
    view.set_objects(vec![object]);
}

fn shot(view: &mut WorldView, gpu: &Gpu) -> Frame {
    view.reset_history();
    let mut frame = None;
    for _ in 0..FRAMES {
        frame = Some(
            view.render_world(gpu, SIZE, eye())
                .expect("the world is drawn"),
        );
    }
    frame.expect("a frame")
}

/// The GPU memory the device's allocator holds (None: the backend gives no report).
fn allocated(gpu: &Gpu) -> Option<u64> {
    gpu.device()
        .generate_allocator_report()
        .map(|r| r.total_allocated_bytes)
}

fn engine() -> Engine {
    EngineBuilder::new()
        .add_plugins((RendererPlugin, torque_app::renderers::RenderFeaturePlugins))
        .build()
        .expect("the engine is built")
}

#[test]
fn a_picture_after_a_restore_is_the_one_before_the_release_and_the_release_frees_memory() {
    let Some(gpu) = gpu() else {
        return;
    };
    let mut view = view(&gpu, 3.0);
    let before = shot(&mut view, &gpu);
    let (bytes, allocator) = (view.gpu_geometry_bytes(), allocated(&gpu));
    assert!(bytes > 0, "the world holds geometry");

    view.release_gpu();
    assert!(!view.has_gpu());
    assert_eq!(view.gpu_geometry_bytes(), 0, "the buffers are dropped");
    if let (Some(held), Some(freed)) = (allocator, allocated(&gpu)) {
        assert!(freed < held, "the device frees memory: {held} to {freed}");
    }
    let released = view.render_world(&gpu, SIZE, eye());
    assert!(released.is_err(), "a released view draws no world");

    assert_eq!(view.restore_gpu(), 0, "no cell dropped its mesh");
    assert!(view.has_gpu());
    let after = shot(&mut view, &gpu);
    assert_eq!(
        view.gpu_geometry_bytes(),
        bytes,
        "the same geometry is back"
    );
    assert!(
        before.rgba == after.rgba,
        "the picture is the same, pixel for pixel"
    );
}

#[test]
fn the_renderer_s_settings_survive_a_release_and_restore() {
    let Some(gpu) = gpu() else {
        return;
    };
    let mut view = view(&gpu, 3.0);
    let world = view.renderer();
    world.features.clouds = false;
    world.lod = false;
    world.occlusion = false;
    world.grain = 0.25;
    world.mirror_reflections = !world.mirror_reflections;
    world.planar_override = Some(0.5);
    world.set_buffer_cap(Some(1 << 24));
    world.set_static_budget(1 << 26);
    world.set_arena_pages(1 << 16, 1 << 18);
    let before = world.settings();
    view.release_gpu();
    view.restore_gpu();
    let after = view.renderer().settings();
    assert_eq!(format!("{before:?}"), format!("{after:?}"));
    assert!(!after.features.clouds && !after.lod && after.grain == 0.25);
}

#[test]
fn a_view_attached_to_a_slot_draws_the_same_picture_after_a_restore() {
    let Some(gpu) = gpu() else {
        return;
    };
    let engine = engine();
    let (slot, warnings) = RendererSlot::new(engine.list::<RendererEntry>(), RenderFeatures::ALL);
    assert!(warnings.is_empty(), "{warnings:?}");
    let mut view = view(&gpu, 3.0);
    view.attach_slot(Rc::new(slot))
        .expect("the engine's renderer is made");
    view.renderer().features.clouds = false;
    let before = shot(&mut view, &gpu);
    view.release_gpu();
    view.restore_gpu();
    let after = shot(&mut view, &gpu);
    assert!(
        before.rgba == after.rgba,
        "the picture is the same, pixel for pixel"
    );
}

#[test]
fn a_release_and_restore_during_play_keeps_the_moving_objects_and_lights() {
    let Some(gpu) = gpu() else {
        return;
    };
    let mut played = view(&gpu, -3.0);
    let mut steady = view(&gpu, -3.0);
    for frame in 0..8 {
        let x = -3.0 + frame as f32 * 0.5;
        for v in [&mut played, &mut steady] {
            moving(v, x);
            v.render_world(&gpu, SIZE, eye()).expect("a frame");
        }
        if frame == 3 {
            played.release_gpu();
            // The app draws the world its own way here.
            assert_eq!(played.scene().objects.len(), 1);
            assert_eq!(played.scene().lights.len(), 1);
            played.restore_gpu();
        }
    }
    assert_eq!(played.scene().objects.len(), 1);
    assert_eq!(played.scene().lights.len(), 1);
    let (a, b) = (shot(&mut played, &gpu), shot(&mut steady, &gpu));
    assert!(
        a.rgba == b.rgba,
        "the view that was released draws what the other does"
    );
    moving(&mut played, 30.0);
    let off_screen = shot(&mut played, &gpu);
    assert!(
        a.rgba != off_screen.rgba,
        "the moving cube is in the picture"
    );
}

#[test]
fn releasing_or_restoring_twice_is_harmless() {
    let Some(gpu) = gpu() else {
        return;
    };
    let mut view = view(&gpu, 3.0);
    let before = shot(&mut view, &gpu);
    assert_eq!(
        view.restore_gpu(),
        0,
        "a view that holds its world is left alone"
    );
    view.release_gpu();
    view.release_gpu();
    assert!(!view.has_gpu());
    view.restore_gpu();
    view.restore_gpu();
    assert!(view.has_gpu());
    let after = shot(&mut view, &gpu);
    assert!(
        before.rgba == after.rgba,
        "the picture is the same, pixel for pixel"
    );
}

#[test]
fn a_cell_that_dropped_its_mesh_is_counted_for_the_game_to_build_again() {
    let Some(gpu) = gpu() else {
        return;
    };
    let mut view = view(&gpu, 3.0);
    shot(&mut view, &gpu);
    view.release_gpu();
    let cell = view
        .scene_mut()
        .frame
        .cells
        .cells
        .get_mut(&static_mesh_key(0))
        .expect("the static cube's cell");
    cell.resident = true;
    cell.vertices.clear();
    cell.indices.clear();
    cell.batches.clear();
    cell.clusters.clear();
    assert_eq!(view.restore_gpu(), 1);
    let cell = &view.scene().frame.cells.cells[&static_mesh_key(0)];
    assert!(!cell.resident, "the flag is cleared");
    shot(&mut view, &gpu);
}

#[test]
fn a_world_built_elsewhere_draws_the_picture_a_restore_does() {
    let Some(gpu) = gpu() else {
        return;
    };
    let mut restored = view(&gpu, 3.0);
    let mut adopted = view(&gpu, 3.0);
    restored.renderer().features.clouds = false;
    adopted.renderer().features.clouds = false;
    let before = shot(&mut adopted, &gpu);
    restored.release_gpu();
    adopted.release_gpu();
    restored.restore_gpu();
    let world = std::thread::scope(|scope| {
        scope
            .spawn(|| {
                let mut world = HdWorld::new(&gpu);
                world.warm_effects();
                world
            })
            .join()
            .expect("the world is built")
    });
    assert_eq!(adopted.adopt_gpu(world), 0, "no cell dropped its mesh");
    assert!(adopted.has_gpu());
    assert!(
        !adopted.renderer().features.clouds,
        "the carried settings are applied to it"
    );
    let (a, b) = (shot(&mut adopted, &gpu), shot(&mut restored, &gpu));
    assert!(
        a.rgba == b.rgba,
        "the adopted world draws what the restored one does"
    );
    assert!(
        a.rgba == before.rgba,
        "and what the view drew before the release"
    );
}

#[test]
fn adopting_a_world_replaces_the_one_a_view_holds_and_counts_the_cells_without_meshes() {
    let Some(gpu) = gpu() else {
        return;
    };
    let mut view = view(&gpu, 3.0);
    view.renderer().grain = 0.25;
    let before = shot(&mut view, &gpu);
    let held = view.gpu_geometry_bytes();
    assert_eq!(view.adopt_gpu(HdWorld::new(&gpu)), 0);
    assert!(view.has_gpu());
    assert_eq!(
        view.renderer().grain,
        0.25,
        "the settings are the old world's"
    );
    let after = shot(&mut view, &gpu);
    assert_eq!(view.gpu_geometry_bytes(), held, "the scene is sent again");
    assert!(before.rgba == after.rgba, "the picture is the same");

    let cell = view
        .scene_mut()
        .frame
        .cells
        .cells
        .get_mut(&static_mesh_key(0))
        .expect("the static cube's cell");
    cell.resident = true;
    cell.vertices.clear();
    cell.indices.clear();
    cell.batches.clear();
    cell.clusters.clear();
    assert_eq!(view.adopt_gpu(HdWorld::new(&gpu)), 1);
}
