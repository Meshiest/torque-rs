use torque_engine::{EngineBuilder, Plugin, PluginId};

#[derive(PluginId)]
#[plugin(id = "torque gamepad")]
struct Gamepad;

impl Plugin for Gamepad {
    fn build(&self, _engine: &mut EngineBuilder) {}
}

#[derive(PluginId)]
struct Wrapper<T>(T);

fn main() {}
