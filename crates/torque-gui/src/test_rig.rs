//! What the tests of the scroll, key and draw features share: a canvas whose content holds a scroll control over a tall page of
//! rows, and the input events a host sends.

use torque_ts_vm::sim::ObjId;
use torque_ts_vm::vm::Vm;

use crate::input::InputEvent;
use crate::keys::{
    KEY_BUTTON0, KEYBOARD_DEVICE, MOUSE_DEVICE, SI_BREAK, SI_BUTTON, SI_KEY, SI_MAKE, SI_ZAXIS,
};
use crate::{Gui, canvas};

pub(crate) const PROFILES: &str = "\
    new GuiControlProfile(BackProfile) { opaque = 1; fillColor = \"0 0 0 255\"; }; \
    new GuiControlProfile(ScrollProfile) { opaque = 1; fillColor = \"20 20 20 255\"; border = 0; borderThickness = 0; }; \
    new GuiControlProfile(PageProfile) { }; \
    new GuiControlProfile(RowProfile) { opaque = 1; fillColor = \"200 40 40 255\"; canKeyFocus = 1; tab = 1; }; \
    new GuiControlProfile(MarkedRowProfile) { opaque = 1; fillColor = \"255 0 255 255\"; canKeyFocus = 1; tab = 1; }; \
    new GuiControlProfile(KeyProfile) { opaque = 1; fillColor = \"10 90 160 255\"; canKeyFocus = 1; tab = 1; keyActivate = 1; }; \
    new GuiControlProfile(StockProfile) { opaque = 1; fillColor = \"10 90 160 255\"; canKeyFocus = 1; tab = 1; };";

/// Rows `Row0` to `Row9`, 30 high at every 40 pixels of the page `Page` (400 high), and a button `Btn` over the gap between the rows 8
/// and 9 (345 to 375), in the scroll control `Scroll` (100 x 100 at 10 10, no border, no bar thickness), which shows 100 pixels of it.
pub(crate) fn scroll_body() -> String {
    let rows: String = (0..10)
        .map(|i| {
            let profile = if i == 2 { "MarkedRowProfile" } else { "RowProfile" };
            format!(
                "new GuiControl(Row{i}) {{ profile = {profile}; position = \"0 {}\"; extent = \"100 30\"; visible = 1; }};",
                i * 40
            )
        })
        .collect();
    format!(
        "new GuiScrollCtrl(Scroll) {{ profile = ScrollProfile; position = \"10 10\"; extent = \"100 100\"; \
           hScrollBar = \"alwaysOff\"; vScrollBar = \"alwaysOn\"; visible = 1; \
           new GuiControl(Page) {{ profile = PageProfile; position = \"0 0\"; extent = \"100 400\"; visible = 1; {rows}              new GuiButtonCtrl(Btn) {{ profile = RowProfile; position = \"0 345\"; extent = \"100 30\"; visible = 1; enabled = 1; }}; }}; }};"
    )
}

/// A canvas of 200 x 200 whose content holds `body`.
pub(crate) fn rig(body: &str) -> (Vm, Gui) {
    let mut vm = Vm::new();
    let gui = crate::register(&mut vm);
    gui.force_canvas_size(Some((200, 200)));
    vm.eval(PROFILES.as_bytes()).unwrap();
    assert!(canvas::create_canvas(&mut vm, &gui).unwrap());
    vm.eval(
        format!(
            "new GuiControl(Top) {{ profile = BackProfile; extent = \"200 200\"; visible = 1; {body} }};"
        )
        .as_bytes(),
    )
    .unwrap();
    let top = vm.objects().find(b"Top").unwrap();
    canvas::set_content(&mut vm, &gui, top).unwrap();
    (vm, gui)
}

/// The object named `name`.
pub(crate) fn id(vm: &Vm, name: &str) -> ObjId {
    vm.objects().find(name.as_bytes()).expect(name)
}

/// The `y` of the control `name`'s position.
pub(crate) fn y_of(vm: &Vm, name: &str) -> i32 {
    crate::point(vm, id(vm, name), b"position").1
}

/// A wheel notch (up or down) over the pointer.
pub(crate) fn wheel(vm: &mut Vm, gui: &Gui, up: bool) {
    let ev = InputEvent {
        device: MOUSE_DEVICE,
        obj_type: SI_ZAXIS,
        value: if up { 1.0 } else { -1.0 },
        ..InputEvent::default()
    };
    let (_, error) = gui.input_event(vm, &ev);
    assert!(error.is_none(), "{error:?}");
}

/// The pointer moved to `pt`.
pub(crate) fn point_at(vm: &mut Vm, gui: &Gui, pt: (i32, i32)) {
    assert!(gui.mouse_moved(vm, pt, 0).is_none());
}

/// The left button going down.
pub(crate) fn press(vm: &mut Vm, gui: &Gui) {
    let ev = InputEvent {
        device: MOUSE_DEVICE,
        obj_type: SI_BUTTON,
        obj_inst: KEY_BUTTON0,
        action: SI_MAKE,
        ..InputEvent::default()
    };
    let (_, error) = gui.input_event(vm, &ev);
    assert!(error.is_none(), "{error:?}");
}

/// A key going down and up.
pub(crate) fn key(vm: &mut Vm, gui: &Gui, code: u16) {
    for action in [SI_MAKE, SI_BREAK] {
        let ev = InputEvent {
            device: KEYBOARD_DEVICE,
            obj_type: SI_KEY,
            obj_inst: code,
            action,
            ..InputEvent::default()
        };
        let (_, error) = gui.input_event(vm, &ev);
        assert!(error.is_none(), "{error:?}");
    }
}
