//! `#[derive(Prefs)]` and `#[derive(EngineClass)]` as a plugin writes them, registered and used.

use torque_engine::{
    EngineBuilder, EngineClass, Filename, Plugin, PluginId, Prefs, ScriptClass, ServerStage, VmPrefs,
};
use torque_ts_vm::vm::Vm;

#[derive(Prefs)]
#[prefs(prefix = "$Pref::Pad")]
struct PadPrefs {
    /// How far the stick must move before it counts.
    #[pref(default = 0.15, range = 0.0..=0.9)]
    dead_zone: f32,
    #[pref(default = true)]
    invert_y: bool,
    #[pref(default = "default", name = "Profile")]
    active_profile: String,
    #[pref(default = -1)]
    slot: i32,
    #[pref(default = 4, range = 1..=8)]
    rumble: u32,
}

#[derive(EngineClass)]
#[class(name = "RopeData", parent = "SimDataBlock", preload = RopeData::check)]
struct RopeData {
    /// How many pieces the rope is cut into.
    #[field(range = 2..=64, default = 8)]
    segments: i32,
    length: f32,
    material: Filename,
    anchor: [f32; 3],
}

impl RopeData {
    fn check(
        &self,
        _vm: &mut torque_engine::class::Vm,
        _id: torque_engine::class::ObjId,
    ) -> Result<(), String> {
        Ok(())
    }
}

#[derive(PluginId)]
struct RopePlugin;

impl Plugin for RopePlugin {
    fn build(&self, engine: &mut EngineBuilder) {
        engine.script().prefs::<PadPrefs>().class::<RopeData>();
    }
}

fn main() {
    let engine = EngineBuilder::new().add_plugins(RopePlugin).build().expect("builds");
    let mut vm = Vm::new();
    for &stage in ServerStage::ALL {
        engine.replay(stage, &mut vm).expect("replays");
    }
    assert_eq!(vm.prefs::<PadPrefs>().dead_zone, 0.15);
    vm.eval(b"datablock RopeData(Cable) { segments = 4; };").expect("runs");
    let id = vm.objects().find(b"Cable").expect("the datablock");
    assert_eq!(RopeData::read(&vm, id).expect("reads").segments, 4);
}
