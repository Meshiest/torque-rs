// ANCHOR: plugin
use torque_app::render::{EngineRender as _, Stage};
use torque_engine::{ClientTick, EngineBuilder, Plugin, PluginId, natives};

use crate::data::RopeData;
use crate::natives::{rope_count, rope_create};
use crate::pass::LinesPass;
use crate::sim::Ropes;

/// The seconds one client tick runs the ropes on by (the tick is 32 ms).
const TICK: f32 = 0.032;

/// Ropes for a game's clients. Add it beside `DefaultPlugins`.
#[derive(Clone, Copy, Default, PluginId)]
#[plugin(id = "example.rope")]
pub struct RopePlugin;

impl Plugin for RopePlugin {
    fn build(&self, engine: &mut EngineBuilder) {
        // The ropes are a resource: every native, the tick and the pass reach the same ones through it.
        let ropes = Ropes::default();
        engine.insert_resource(ropes.clone());
        // The class and the functions are for clients only: a dedicated server draws nothing.
        engine
            .client_script()
            .class::<RopeData>()
            .natives(natives![rope_create, rope_count]);
        // Once per client tick, every rope runs on.
        let ticking = ropes.clone();
        engine.push(ClientTick::new(move |_vm| {
            for rope in ticking.lock().iter_mut() {
                rope.step(TICK);
            }
        }));
        // And each client's world view draws them, at the opaque stage.
        engine
            .render()
            .pass(Stage::Opaque, move || LinesPass::new(ropes.clone()));
    }
}
// ANCHOR_END: plugin
