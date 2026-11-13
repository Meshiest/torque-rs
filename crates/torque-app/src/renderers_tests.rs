use std::rc::Rc;
use torque_ts_vm::testing::eval;

use torque_engine::{Engine, EngineBuilder, Plugin, PluginGroup, PluginType};
use torque_render_core::{Gpu, wgpu};
use torque_render_hd::HdFeatures;
use torque_render_scene::hd::{HdMode, Quality};
use torque_render_scene::source::Eye;

use crate::client::{ClientBuilder, ClientNet};
use crate::plugins::*;
use crate::renderers::{
    DEFAULT_RENDERER, EngineRenderers, NamedRenderer, RenderFeatures, RenderFrame, RendererSlot,
    SwitchError, WorldRenderer,
};
use crate::view::WorldView;

/// A renderer for the tests: it clears its target to one color (the tests draw the world over the whole target).
struct FlatRenderer {
    gpu: Gpu,
}

impl WorldRenderer for FlatRenderer {
    fn draw(&mut self, frame: RenderFrame<'_>) -> Result<(), String> {
        let mut encoder =
            self.gpu
                .device()
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("flat"),
                });
        {
            let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("flat"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: frame.target,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 1.0,
                            g: 0.0,
                            b: 1.0,
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
        }
        self.gpu.queue().submit([encoder.finish()]);
        Ok(())
    }
}

impl NamedRenderer for FlatRenderer {
    const ID: &'static str = "test.flat";

    fn new(gpu: &Gpu) -> Result<FlatRenderer, String> {
        Ok(FlatRenderer { gpu: gpu.clone() })
    }
}

/// Registers the flat renderer on the slot.
#[derive(Default, torque_engine::PluginId)]
#[plugin(id = "test.flat-renderer")]
struct FlatRendererPlugin;

impl Plugin for FlatRendererPlugin {
    fn requires(&self, requires: &mut torque_engine::Requires) {
        requires.plugin::<RendererSlotPlugin>();
    }

    fn build(&self, engine: &mut EngineBuilder) {
        engine.add_renderer::<FlatRenderer>();
    }
}

fn engine(group: impl PluginGroup) -> Engine {
    EngineBuilder::new()
        .add_plugins(group)
        .build()
        .expect("the engine builds")
}

fn with_flat(group: impl PluginGroup) -> Engine {
    EngineBuilder::new()
        .add_plugins((group, FlatRendererPlugin))
        .build()
        .expect("the engine builds")
}

fn client(engine: &Engine) -> ClientBuilder {
    ClientBuilder::from_engine(engine, None, ClientNet::None).expect("the client is made")
}

fn slot(builder: &ClientBuilder) -> Rc<RendererSlot> {
    RendererSlot::of(&builder.vm).expect("the client has a renderer slot")
}

#[test]
fn every_feature_plugin_on_is_the_renderer_as_it_always_drew() {
    assert_eq!(RenderFeatures::ALL.features, HdFeatures::default());
    assert_eq!(RenderFeatures::ALL.mode(Quality::High), HdMode::default());
    let engine = engine(DefaultPlugins);
    assert_eq!(
        engine.resource::<RenderFeatures>().as_deref(),
        Some(&RenderFeatures::ALL)
    );
    let slot = slot(&client(&engine));
    assert_eq!(slot.current(), Some(DEFAULT_RENDERER));
    assert_eq!(slot.ids().collect::<Vec<_>>(), [DEFAULT_RENDERER]);
    assert_eq!(slot.features(), RenderFeatures::ALL);
}

#[test]
fn the_default_group_has_the_slot_the_renderer_and_its_features_in_order() {
    let ids: Vec<&str> = engine(DefaultPlugins)
        .plugins()
        .iter()
        .map(|id| id.as_str())
        .skip_while(|id| *id != "torque.renderer-slot")
        .take(15)
        .collect();
    assert_eq!(
        ids,
        [
            "torque.renderer-slot",
            "torque.renderer",
            "torque.render-shadows",
            "torque.render-anti-aliasing",
            "torque.render-ambient-occlusion",
            "torque.render-reflections",
            "torque.render-clear-water",
            "torque.render-bloom",
            "torque.render-volumetrics",
            "torque.render-clouds",
            "torque.render-transparency",
            "torque.render-grading",
            "torque.render-glow-lights",
            "torque.render-surface-detail",
            "torque.render-particle-light",
        ]
    );
}

/// The engine's renderer features with the plugin `P` off.
fn without<P: PluginType>() -> RenderFeatures {
    let engine = engine(DefaultPlugins.disable::<P>());
    assert!(engine.warnings().is_empty(), "{:?}", engine.warnings());
    slot(&client(&engine)).features()
}

#[test]
fn turning_a_feature_plugin_off_turns_its_switches_off_and_no_other() {
    let all = RenderFeatures::ALL;
    let f = all.features;
    let off = |features: HdFeatures| RenderFeatures { features, ..all };
    let cases: [(&str, RenderFeatures, RenderFeatures); 13] = [
        (
            "shadows",
            without::<ShadowsPlugin>(),
            off(HdFeatures {
                sun_shadows: false,
                point_shadows: false,
                contact_shadows: false,
                ..f
            }),
        ),
        (
            "anti-aliasing",
            without::<AntiAliasingPlugin>(),
            off(HdFeatures {
                anti_aliasing: false,
                ..f
            }),
        ),
        (
            "ambient occlusion",
            without::<AmbientOcclusionPlugin>(),
            off(HdFeatures {
                ambient_occlusion: false,
                ..f
            }),
        ),
        (
            "reflections",
            without::<ReflectionsPlugin>(),
            off(HdFeatures {
                reflections: false,
                ..f
            }),
        ),
        (
            "clear water",
            without::<ClearWaterPlugin>(),
            RenderFeatures {
                clear_water: false,
                ..all
            },
        ),
        (
            "bloom",
            without::<BloomPlugin>(),
            off(HdFeatures { bloom: false, ..f }),
        ),
        (
            "volumetrics",
            without::<VolumetricsPlugin>(),
            off(HdFeatures {
                volumetrics: false,
                ..f
            }),
        ),
        (
            "clouds",
            without::<CloudsPlugin>(),
            off(HdFeatures { clouds: false, ..f }),
        ),
        (
            "transparency",
            without::<TransparencyPlugin>(),
            off(HdFeatures { wboit: false, ..f }),
        ),
        (
            "grading",
            without::<GradingPlugin>(),
            off(HdFeatures {
                grading_lut: false,
                ..f
            }),
        ),
        (
            "glow lights",
            without::<GlowLightsPlugin>(),
            off(HdFeatures {
                glow_lights: false,
                ..f
            }),
        ),
        (
            "surface detail",
            without::<SurfaceDetailPlugin>(),
            off(HdFeatures {
                ext_detail: false,
                ..f
            }),
        ),
        (
            "particle light",
            without::<ParticleLightPlugin>(),
            RenderFeatures {
                particle_light: false,
                ..all
            },
        ),
    ];
    for (name, got, want) in cases {
        assert_eq!(got, want, "{name} off");
        assert_ne!(got, all, "{name} off changes something");
    }
    // Every one off: the bare renderer.
    let engine = engine(
        DefaultPlugins
            .disable::<(
                ShadowsPlugin,
                AntiAliasingPlugin,
                AmbientOcclusionPlugin,
                ReflectionsPlugin,
                ClearWaterPlugin,
                BloomPlugin,
                VolumetricsPlugin,
                CloudsPlugin,
                TransparencyPlugin,
                GradingPlugin,
                GlowLightsPlugin,
                SurfaceDetailPlugin,
            )>()
            .disable::<ParticleLightPlugin>(),
    );
    assert_eq!(slot(&client(&engine)).features(), RenderFeatures::NONE);
}

#[test]
fn set_renderer_swaps_among_the_registered_ids_and_refuses_others_naming_them() {
    let mut builder = client(&with_flat(DefaultPlugins));
    let slot = slot(&builder);
    assert_eq!(
        slot.ids().collect::<Vec<_>>(),
        [DEFAULT_RENDERER, FlatRenderer::ID]
    );
    let vm = &mut builder.vm;
    assert_eq!(eval(vm, "return getRenderer();"), "default");
    assert_eq!(eval(vm, "return setRenderer(\"test.flat\");"), "1");
    assert_eq!(slot.current(), Some(FlatRenderer::ID));
    assert_eq!(eval(vm, "return getRenderer();"), "test.flat");
    let swaps = slot.generation();
    assert_eq!(eval(vm, "return setRenderer(\"test.flat\");"), "1");
    assert_eq!(
        slot.generation(),
        swaps,
        "the same renderer again is no swap"
    );

    assert_eq!(eval(vm, "return setRenderer(\"vulkan\");"), "0");
    assert_eq!(slot.current(), Some(FlatRenderer::ID));
    let refused = slot.switch("vulkan").unwrap_err();
    assert!(
        matches!(&refused, SwitchError::Unknown { registered, .. } if registered == &["default", "test.flat"]),
        "{refused:?}"
    );
    assert_eq!(
        refused.to_string(),
        "no renderer \"vulkan\" (registered: default, test.flat)"
    );
    slot.switch(DEFAULT_RENDERER).unwrap();
    assert_eq!(eval(vm, "return getRenderer();"), "default");
}

#[test]
fn a_client_starts_on_the_default_renderer_else_the_first_and_without_any_on_none() {
    let builder = client(&with_flat(DefaultPlugins.disable::<RendererPlugin>()));
    assert_eq!(slot(&builder).current(), Some(FlatRenderer::ID));
    let mut builder = client(&engine(DefaultPlugins.disable::<RendererPlugin>()));
    assert_eq!(slot(&builder).current(), None);
    assert_eq!(eval(&mut builder.vm, "return getRenderer();"), "");
    assert_eq!(
        eval(&mut builder.vm, "return setRenderer(\"default\");"),
        "0"
    );
    assert_eq!(
        slot(&builder).switch("default").unwrap_err().to_string(),
        "no renderer \"default\" (registered: none)"
    );
}

#[test]
fn a_second_renderer_under_an_id_is_left_out() {
    /// Registers a renderer under the default id and one under an id that is not valid, after the engine's renderer.
    #[derive(torque_engine::PluginId)]
    #[plugin(id = "test.clashing-renderers")]
    struct Clashing;

    impl Plugin for Clashing {
        fn build(&self, engine: &mut EngineBuilder) {
            for id in [DEFAULT_RENDERER, "not an id"] {
                engine.add_renderer_id(id, |gpu| {
                    Ok(Box::new(FlatRenderer::new(gpu)?) as Box<dyn WorldRenderer>)
                });
            }
        }
    }

    let engine = EngineBuilder::new()
        .add_plugins((DefaultPlugins, FlatRendererPlugin, Clashing))
        .build()
        .unwrap();
    let slot = slot(&client(&engine));
    assert_eq!(
        slot.ids().collect::<Vec<_>>(),
        [DEFAULT_RENDERER, FlatRenderer::ID]
    );
    assert!(
        slot.current_entry().unwrap().make().is_none(),
        "the first registered under the id is kept"
    );
}

/// A frame of a plain floor under the default daylight, looking down at it from 4 units up.
fn picture(view: &mut WorldView, gpu: &Gpu) -> Vec<u8> {
    let eye = Eye {
        position: [0.0, 0.0, 4.0],
        pitch: 0.6,
        yaw: 0.0,
        fov: Some(75),
    };
    view.render_world(gpu, (64, 48), eye)
        .expect("the world is drawn")
        .rgb()
}

#[test]
fn the_view_hot_swaps_with_its_client_s_slot_and_keeps_the_feature_plugins_offscreen() {
    let gpu = match crate::window::offscreen_gpu() {
        Ok(gpu) => gpu,
        Err(e) => {
            eprintln!("skipped: {e}");
            return;
        }
    };
    let mut builder = client(&with_flat(DefaultPlugins.disable::<CloudsPlugin>()));
    let mut view = WorldView::new(&gpu);
    assert_eq!(view.renderer().features, HdFeatures::default());
    view.set_plain_floor(50.0, [120, 160, 90]);
    view.attach_slot(slot(&builder)).unwrap();
    assert!(
        !view.renderer().features.clouds,
        "the clouds' plugin is off"
    );
    assert!(view.renderer().features.sun_shadows);
    assert_eq!(view.current_renderer(), Some(DEFAULT_RENDERER));
    let first = picture(&mut view, &gpu);

    assert_eq!(
        eval(&mut builder.vm, "return setRenderer(\"test.flat\");"),
        "1"
    );
    let flat = picture(&mut view, &gpu);
    assert_eq!(view.current_renderer(), Some(FlatRenderer::ID));
    assert!(
        flat.chunks(3).all(|p| p == [255, 0, 255]),
        "the flat renderer fills the world"
    );
    assert_ne!(flat, first);

    // The view's own swap goes through the slot: the scripts see it.
    view.set_renderer(DEFAULT_RENDERER).unwrap();
    assert_eq!(eval(&mut builder.vm, "return getRenderer();"), "default");
    let back = picture(&mut view, &gpu);
    assert_eq!(view.current_renderer(), Some(DEFAULT_RENDERER));
    assert!(!view.renderer().features.clouds);
    assert_ne!(back, flat);

    let refused = view.set_renderer("vulkan").unwrap_err();
    assert!(
        refused.contains("registered: default, test.flat"),
        "{refused}"
    );
    assert_eq!(view.current_renderer(), Some(DEFAULT_RENDERER));
}

#[test]
fn a_view_without_a_renderer_draws_the_gui_alone() {
    let gpu = match crate::window::offscreen_gpu() {
        Ok(gpu) => gpu,
        Err(e) => {
            eprintln!("skipped: {e}");
            return;
        }
    };
    let builder = client(&engine(DefaultPlugins.disable::<RendererPlugin>()));
    let mut view = WorldView::new(&gpu);
    view.attach_slot(slot(&builder)).unwrap();
    assert_eq!(view.current_renderer(), None);
    let eye = Eye {
        position: [0.0, 0.0, 4.0],
        pitch: 0.6,
        yaw: 0.0,
        fov: Some(75),
    };
    assert!(view.render_world(&gpu, (32, 32), eye).is_err());
    assert!(view.set_renderer(DEFAULT_RENDERER).is_err());
}

/// Registers `test.broken`, a renderer that can never be made, and counts the tries.
#[derive(Default, torque_engine::PluginId)]
#[plugin(id = "test.broken-renderer")]
struct BrokenRendererPlugin {
    tries: Rc<std::cell::Cell<u32>>,
}

impl Plugin for BrokenRendererPlugin {
    fn build(&self, engine: &mut EngineBuilder) {
        let tries = Rc::clone(&self.tries);
        engine.add_renderer_id("test.broken", move |_gpu| {
            tries.set(tries.get() + 1);
            Err("the device has no such feature".to_string())
        });
    }
}

#[test]
fn a_renderer_that_cannot_be_made_is_refused_and_the_one_drawing_stays() {
    let gpu = match crate::window::offscreen_gpu() {
        Ok(gpu) => gpu,
        Err(e) => {
            eprintln!("skipped: {e}");
            return;
        }
    };
    let broken = BrokenRendererPlugin::default();
    let tries = Rc::clone(&broken.tries);
    let engine = EngineBuilder::new()
        .add_plugins((DefaultPlugins, FlatRendererPlugin, broken))
        .build()
        .unwrap();

    // Chosen before a view attaches (a script at start-up): made at the attach, which falls back to the default.
    let mut builder = client(&engine);
    assert_eq!(
        eval(&mut builder.vm, "return setRenderer(\"test.broken\");"),
        "1"
    );
    let mut view = WorldView::new(&gpu);
    view.set_plain_floor(50.0, [120, 160, 90]);
    let refused = view.attach_slot(slot(&builder)).unwrap_err();
    assert!(
        refused.contains("the device has no such feature"),
        "{refused}"
    );
    assert_eq!(view.current_renderer(), Some(DEFAULT_RENDERER));
    assert_eq!(eval(&mut builder.vm, "return getRenderer();"), "default");
    assert_eq!(tries.get(), 1);

    // Chosen while drawing with another: refused, false to the script, the flat renderer still draws.
    assert_eq!(
        eval(&mut builder.vm, "return setRenderer(\"test.flat\");"),
        "1"
    );
    let flat = picture(&mut view, &gpu);
    assert_eq!(
        eval(&mut builder.vm, "return setRenderer(\"test.broken\");"),
        "0"
    );
    assert_eq!(tries.get(), 2);
    assert_eq!(eval(&mut builder.vm, "return getRenderer();"), "test.flat");
    assert_eq!(view.current_renderer(), Some(FlatRenderer::ID));
    assert_eq!(picture(&mut view, &gpu), flat, "the world still draws");
    let refused = view.set_renderer("test.broken").unwrap_err();
    assert!(refused.contains("could not be made"), "{refused}");
    assert_eq!(tries.get(), 3, "choosing the id again tries again");
    assert!(matches!(
        slot(&builder).switch("test.broken"),
        Err(SwitchError::Failed { .. })
    ));
    assert_eq!(view.current_renderer(), Some(FlatRenderer::ID));
}

/// The upscaler's prefs: off by default (a fresh script engine has no such global), the temporal upscaler by its name at the
/// quality's share or a set one, and any other word off.
#[test]
fn the_render_prefs_choose_the_upscaler() {
    use torque_engine::VmPrefs;
    use torque_render_scene::hd::post::{Upscaler, Upscaling};

    use crate::renderers::RenderPrefs;

    let mut vm = torque_ts_vm::vm::Vm::new();
    assert_eq!(vm.prefs::<RenderPrefs>().upscaling(), Upscaling::OFF);
    for (upscaler, scale, read) in [
        ("temporal", "0", Upscaling::TEMPORAL),
        (
            "Temporal",
            "0.9",
            Upscaling {
                upscaler: Upscaler::Temporal,
                scale: Some(0.9),
            },
        ),
        (
            "fsr",
            "0.9",
            Upscaling {
                upscaler: Upscaler::Off,
                scale: Some(0.9),
            },
        ),
        ("off", "-1", Upscaling::OFF),
    ] {
        vm.set_global("$Pref::Render::Upscaler", upscaler);
        vm.set_global("$Pref::Render::UpscaleScale", scale);
        assert_eq!(
            vm.prefs::<RenderPrefs>().upscaling(),
            read,
            "{upscaler} {scale}"
        );
    }
}
