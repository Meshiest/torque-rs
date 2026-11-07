use torque_engine::{EngineBuilder, Plugin, PluginId, PluginType};

/// The id comes from the package and the type.
#[derive(PluginId)]
struct AudioPlugin;

impl Plugin for AudioPlugin {
    fn build(&self, _engine: &mut EngineBuilder) {}
}

/// The id is given, and the engine crate is named by a path.
#[derive(PluginId)]
#[plugin(id = "torque.gamepad", crate = torque_engine)]
struct Gamepad {
    _dead_zone: f32,
}

impl Plugin for Gamepad {
    fn build(&self, _engine: &mut EngineBuilder) {}
}

fn main() {
    // trybuild compiles the cases in a package of its own, `torque-macros-tests`.
    assert_eq!(AudioPlugin::ID.as_str(), "torque_macros_tests::AudioPlugin");
    assert_eq!(Gamepad::ID.as_str(), "torque.gamepad");
    let engine = EngineBuilder::new()
        .add_plugins((AudioPlugin, Gamepad { _dead_zone: 0.1 }))
        .build()
        .unwrap();
    assert_eq!(engine.plugins(), [AudioPlugin::ID, Gamepad::ID]);
}
