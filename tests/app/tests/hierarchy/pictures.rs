//! A three-level hierarchy drawn offscreen by the client it was sent to: a cube, a turned cube on it, a scaled pistol on
//! that, a small cube on the pistol's muzzle node, before and after the root moves and turns. Needs a GPU (skipped
//! without one). `TORQUE_HIERARCHY_PICTURES=<folder>` also writes the pictures there as PNGs.

#[path = "net/rig.rs"]
mod rig;

use std::path::PathBuf;
use std::rc::Rc;

use rig::{Kit, Rig, files, linked, three_levels};
use torque_app::ghosts::{GhostOptions, GhostScene};
use torque_app::renderers::{RenderFeatures, RendererEntry, RendererSlot};
use torque_app::shots::write_png;
use torque_app::view::{Daylight, WorldView};
use torque_engine::Engine;
use torque_render_core::{Frame, Gpu};
use torque_render_scene::hd::DynamicObject;
use torque_render_scene::hd::kit::look_at;
use torque_render_scene::moving::ShapeSelect;
use torque_render_scene::source::Eye;

const SIZE: (u32, u32) = (960, 540);
/// Frames each picture settles for (the anti-aliasing's history).
const FRAMES: u32 = 10;

fn gpu() -> Option<Gpu> {
    match torque_app::window::offscreen_gpu() {
        Ok(gpu) => Some(gpu),
        Err(e) => {
            eprintln!("skipped: {e}");
            None
        }
    }
}

fn save(frame: &Frame, name: &str) {
    if let Some(dir) = std::env::var_os("TORQUE_HIERARCHY_PICTURES").map(PathBuf::from) {
        std::fs::create_dir_all(&dir).expect("the pictures' folder");
        write_png(frame, &dir.join(format!("{name}.png"))).expect("the picture is written");
    }
}

fn view(gpu: &Gpu, engine: &Engine) -> WorldView {
    let mut view = WorldView::new(gpu);
    view.set_plain_floor(120.0, [128, 150, 110]);
    view.set_daylight(&Daylight::default());
    let features = engine
        .resource::<RenderFeatures>()
        .as_deref()
        .copied()
        .unwrap_or(RenderFeatures::ALL);
    let (slot, warnings) = RendererSlot::new(engine.list::<RendererEntry>(), features);
    assert!(warnings.is_empty(), "{warnings:?}");
    let adds = torque_app::render::RenderAdds::of_engine(engine).kept(|w| panic!("{w}"));
    view.attach_slot(Rc::new(slot.with_adds(adds)))
        .expect("the engine's renderer is made");
    view
}

fn eye() -> Eye {
    look_at([1.0, -6.5, 3.6], [1.0, 0.5, 2.0])
}

fn objects(rig: &Rig, scene: &mut GhostScene, view: &mut WorldView) -> Vec<DynamicObject> {
    let kit = Kit(files());
    rig.client
        .with_world(|w| scene.objects(view, &kit, &w.records, None))
        .expect("connected")
}

fn shot(view: &mut WorldView, gpu: &Gpu, objects: &[DynamicObject]) -> Frame {
    view.reset_history();
    let mut frame = None;
    for _ in 0..FRAMES {
        view.set_objects(objects.to_vec());
        frame = Some(
            view.render_world(gpu, SIZE, eye())
                .expect("the world is drawn"),
        );
    }
    frame.expect("a frame")
}

/// The pixels the two pictures differ in, by the largest channel's difference over `threshold`.
fn differing(a: &Frame, b: &Frame, threshold: i32) -> usize {
    a.rgba
        .chunks(4)
        .zip(b.rgba.chunks(4))
        .filter(|(p, q)| {
            (0..3)
                .map(|c| (i32::from(p[c]) - i32::from(q[c])).abs())
                .max()
                .is_some_and(|d| d > threshold)
        })
        .count()
}

#[test]
fn a_three_level_hierarchy_is_drawn_on_its_moving_root() {
    let Some(gpu) = gpu() else { return };
    let mut rig = Rig::new(0.0);
    let mut view = view(&gpu, &rig.engine);
    let mut scene = GhostScene::new(GhostOptions {
        select: ShapeSelect::All,
        ..GhostOptions::default()
    });
    let none = objects(&rig, &mut scene, &mut view);
    let empty = shot(&mut view, &gpu, &none);
    let (root, pairs) = three_levels(&mut rig);
    let ghosts = linked(&mut rig, &pairs);
    rig.run(4);
    let drawn = objects(&rig, &mut scene, &mut view);
    for ghost in &ghosts {
        assert!(
            drawn.iter().any(|o| o.ghost == *ghost),
            "child {ghost} is drawn"
        );
    }
    let still = shot(&mut view, &gpu, &drawn);
    save(&still, "three-levels");
    // The root lifts and turns a quarter: the whole tree goes with it.
    rig.move_to(root, "0 0.5 1.8 0 0 1 0.8");
    rig.run(4);
    let moved = objects(&rig, &mut scene, &mut view);
    let turned = shot(&mut view, &gpu, &moved);
    save(&turned, "three-levels-turned");
    assert!(differing(&empty, &still, 40) > 4_000, "the tree is drawn");
    assert!(
        differing(&still, &turned, 40) > 2_000,
        "the tree moved with its root"
    );
}
