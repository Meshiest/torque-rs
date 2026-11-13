//! The layout containers driven from Lua through the object API: a client's Lua sets a container's fields and its
//! children's layout fields, and the next GUI frame lays them out.

use crate::common;

use common::{files, run};
use torque_engine::{ClientStage, EngineBuilder};
use torque_gui::custom::ControlEntry;
use torque_mods::ModsPlugin;
use torque_plugin_gui_layout::GuiLayoutPlugin;
use torque_ts_vm::vm::Vm;

fn rect(vm: &Vm, name: &str) -> [i32; 4] {
    let id = vm.objects().find(name.as_bytes()).expect("the control");
    let ((x, y), (w, h)) = torque_gui::bounds(vm, id);
    [x, y, w, h]
}

#[test]
fn lua_sets_layout_fields_and_the_next_frame_lays_them_out() {
    let engine = EngineBuilder::new()
        .add_plugins((ModsPlugin::new(), common::lua(), GuiLayoutPlugin))
        .build()
        .expect("the engine builds");
    let files = files(&[]);
    // A client's script engine with the GUI, as the GUI plugin makes it: the GUI's natives, then the control classes.
    let mut vm = Vm::new();
    let gui = torque_gui::register(&mut vm);
    gui.add_controls(engine.list::<ControlEntry>());
    gui.0.borrow_mut().forced_size = Some((640, 480));
    for &stage in ClientStage::ALL {
        if stage == ClientStage::Game {
            vm.set_file_host(files.clone());
        }
        engine.replay(stage, &mut vm).expect("the stage replays");
    }
    vm.eval(
        b"createCanvas(\"test\");\
          new GuiControlProfile(GuiDefaultProfile) { };\
          new GuiControl(Content) { extent = \"640 480\"; };\
          Canvas.setContent(Content);\
          Content.add(new GuiFlexCtrl(Row) { extent = \"300 200\";\
             new GuiControl(A) { extent = \"50 20\"; };\
             new GuiControl(B) { extent = \"50 20\"; };\
          });",
    )
    .expect("the controls");
    gui.frame(&mut vm);
    assert_eq!(rect(&vm, "B"), [50, 0, 50, 200], "a row, stretched");
    let lua = "local row = ts.object('Row') \
               row.direction = 'column' \
               row.gap = 10 \
               row.alignItems = 'center' \
               ts.object('B').flexGrow = 1 \
               ts.setGlobal('$Test::Direction', row.direction)";
    assert_eq!(run(&mut vm, &files, "layout.lua", lua), Ok(true));
    assert_eq!(
        torque_ts_vm::testing::global(&vm, "$Test::Direction"),
        "column"
    );
    gui.frame(&mut vm);
    assert_eq!(rect(&vm, "A"), [125, 0, 50, 20]);
    assert_eq!(
        rect(&vm, "B"),
        [125, 30, 50, 170],
        "the rest of the column, centered"
    );
}
