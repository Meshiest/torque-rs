//! The sample extension's render passes, drawn offscreen over a plain floor: the picture of the library loaded from a
//! plugins folder is the same plugin's compiled in (within the shots' tolerance), and both differ from the floor alone
//! where the passes draw. A pass that panics disables its extension and the frame is still drawn.
//!
//! `TORQUE_NATIVE_EXT_SHOTS=<folder>` (relative: the workspace's) also writes `compiled/sample.png` and `loaded/sample.png` there, for
//! `just shots-diff <folder>/compiled <folder>/loaded` (`just native-ext-shots` does both).

// The panicking pass is linked in through its entry point.
#![allow(unsafe_code)]

mod common;

use std::path::PathBuf;
use std::rc::Rc;

use common::{CompiledSample, sample_folder};
use torque_app::render::RenderAdds;
use torque_app::renderers::{RenderFeatures, RendererEntry, RendererPlugin, RendererSlot};
use torque_app::shots::write_png;
use torque_app::view::{Daylight, WorldView};
use torque_engine::{Engine, EngineBuilder, Plugins};
use torque_plugin_loader::abi::TORQUE_ABI_VERSION;
use torque_plugin_loader::{ApprovalRequest, ExtensionPlugin, NativeExtensions};
use torque_plugin_sdk::{Extension, PassFrame, Registrar, Triangles};
use torque_render_core::{Frame, Gpu};
use torque_render_scene::hd::kit::look_at;

const SIZE: (u32, u32) = (640, 360);
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

/// An engine of the renderer and `plugins`.
fn engine<M>(plugins: impl Plugins<M>) -> Engine {
    EngineBuilder::new()
        .add_plugins((RendererPlugin, torque_app::renderers::RenderFeaturePlugins))
        .add_plugins(plugins)
        .build()
        .expect("the engine is built")
}

/// The floor under the default day, seen from in front of the sample's square, with what `engine`'s plugins draw.
fn shot(gpu: &Gpu, engine: &Engine) -> Frame {
    let mut view = WorldView::new(gpu);
    view.set_plain_floor(120.0, [128, 150, 110]);
    view.set_daylight(&Daylight::default());
    view.set_effect_frame_seconds(Some(1.0 / 60.0));
    let features = engine
        .resource::<RenderFeatures>()
        .as_deref()
        .copied()
        .unwrap_or(RenderFeatures::ALL);
    let (slot, warnings) = RendererSlot::new(engine.list::<RendererEntry>(), features);
    assert!(warnings.is_empty(), "{warnings:?}");
    let adds = RenderAdds::of_engine(engine).kept(|w| panic!("{w}"));
    view.attach_slot(Rc::new(slot.with_adds(adds)))
        .expect("the engine's renderer is made");
    view.reset_history();
    let eye = look_at([0.0, -16.0, 4.5], [0.0, 0.0, 2.0]);
    let mut frame = None;
    for _ in 0..FRAMES {
        frame = Some(
            view.render_world(gpu, SIZE, eye)
                .expect("the world is drawn"),
        );
    }
    frame.unwrap()
}

fn save(frame: &Frame, side: &str) {
    if let Some(dir) = std::env::var_os("TORQUE_NATIVE_EXT_SHOTS").map(PathBuf::from) {
        // A relative folder is the workspace's (`just native-ext-shots` passes one).
        let dir = common::workspace().join(dir).join(side);
        std::fs::create_dir_all(&dir).unwrap();
        write_png(frame, &dir.join("sample.png")).unwrap();
    }
}

/// The pixels whose largest channel difference is over `over`, and the largest difference.
fn differing(a: &Frame, b: &Frame, over: u8) -> (usize, u8) {
    assert_eq!((a.width, a.height), (b.width, b.height));
    let mut n = 0;
    let mut most = 0;
    for (p, q) in a
        .rgba
        .as_chunks::<4>()
        .0
        .iter()
        .zip(b.rgba.as_chunks::<4>().0)
    {
        let d = (0..3).map(|c| p[c].abs_diff(q[c])).max().unwrap();
        most = most.max(d);
        if d > over {
            n += 1;
        }
    }
    (n, most)
}

#[test]
fn compiled_in_and_loaded_draw_the_same_picture() {
    let Some(gpu) = gpu() else {
        return;
    };
    let folder = sample_folder("pictures", &[]);
    let loaded = EngineBuilder::new()
        .add_plugins((RendererPlugin, torque_app::renderers::RenderFeaturePlugins))
        .add_plugins(NativeExtensions(
            common::plugin(&folder).approver(|_: &ApprovalRequest| true),
        ))
        .enable_id("torque.native-extensions")
        .build()
        .unwrap();
    assert!(loaded.is_plugin_added_id("example.native-sample"));
    let compiled = engine(CompiledSample);
    let plain = engine(torque_engine::PluginGroupBuilder::new());

    let loaded_frame = shot(&gpu, &loaded);
    let compiled_frame = shot(&gpu, &compiled);
    let plain_frame = shot(&gpu, &plain);
    save(&compiled_frame, "compiled");
    save(&loaded_frame, "loaded");

    // The shots' tolerance (`shots_tolerance` in the justfile): no pixel off by more than 1, at most 32 off at all.
    let (off, most) = differing(&compiled_frame, &loaded_frame, 0);
    assert!(
        most <= 1 && off <= 32,
        "{off} pixels differ, by up to {most}"
    );
    // The passes drew: the square and the badge changed many pixels from the floor alone.
    let (changed, _) = differing(&plain_frame, &loaded_frame, 40);
    assert!(changed > 2_000, "only {changed} pixels changed");
}

#[derive(Default)]
struct PanickingPass;

impl Extension for PanickingPass {
    const ID: &'static str = "test.panicking-pass";
    const VERSION: &'static str = "1";

    fn build(&self, ext: &mut Registrar) {
        ext.render().pass(torque_plugin_sdk::Stage::Overlay, || {
            |_: &PassFrame, _: &mut Triangles| panic!("no pass today")
        });
    }
}

torque_plugin_sdk::init_fn!(panicking_init, PanickingPass);

#[test]
fn a_panicking_pass_disables_its_extension_and_the_frame_is_drawn() {
    let Some(gpu) = gpu() else {
        return;
    };
    let plugin = ExtensionPlugin::from_init(TORQUE_ABI_VERSION, panicking_init).unwrap();
    let extension = plugin.extension();
    let engine = engine(plugin);
    let frame = shot(&gpu, &engine);
    assert_eq!((frame.width, frame.height), SIZE);
    assert!(extension.is_disabled());
    assert!(extension.disabled_reason().unwrap().contains("pass"));
    assert!(
        extension.log().iter().any(|l| l.contains("no pass today")),
        "{:?}",
        extension.log()
    );
}
