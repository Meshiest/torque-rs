use torque_ts_vm::vm::Vm;

use crate::Gui;
use crate::keys::{KEY_RETURN, KEY_SPACE};
use crate::test_rig::{id, key, rig};

const BUTTONS: &str = "\
    new GuiButtonCtrl(KeyBtn) { profile = KeyProfile; position = \"0 0\"; extent = \"50 20\"; visible = 1; enabled = 1; \
        command = \"$keyed = $keyed + 1;\"; }; \
    new GuiButtonCtrl(StockBtn) { profile = StockProfile; position = \"60 0\"; extent = \"50 20\"; visible = 1; enabled = 1; \
        command = \"$stock = $stock + 1;\"; };";

fn buttons() -> (Vm, Gui) {
    let (mut vm, gui) = rig(BUTTONS);
    vm.eval(b"$keyed = 0; $stock = 0;").unwrap();
    (vm, gui)
}

fn count(vm: &Vm, name: &str) -> String {
    String::from_utf8(vm.global(name).unwrap_or_default()).unwrap()
}

#[test]
fn return_and_space_press_the_focused_button_when_its_profile_asks() {
    let (mut vm, gui) = buttons();
    vm.eval(b"KeyBtn.makeFirstResponder(1);").unwrap();
    assert_eq!(gui.first_responder(), Some(id(&vm, "KeyBtn")));
    key(&mut vm, &gui, KEY_RETURN);
    assert_eq!(count(&vm, "$keyed"), "1");
    key(&mut vm, &gui, KEY_SPACE);
    assert_eq!(count(&vm, "$keyed"), "2");
    // A key down alone presses it down; the action runs on the key up, and it is up again after.
    assert!(!gui.control(id(&vm, "KeyBtn")).unwrap().depressed);
}

#[test]
fn a_button_whose_profile_does_not_ask_never_answers_the_keys() {
    let (mut vm, gui) = buttons();
    vm.eval(b"StockBtn.makeFirstResponder(1);").unwrap();
    assert_eq!(gui.first_responder(), Some(id(&vm, "StockBtn")));
    key(&mut vm, &gui, KEY_RETURN);
    key(&mut vm, &gui, KEY_SPACE);
    assert_eq!(count(&vm, "$stock"), "0");
}

#[test]
fn a_disabled_button_stays_still_even_with_the_field() {
    let (mut vm, gui) = buttons();
    vm.eval(b"KeyBtn.makeFirstResponder(1); KeyBtn.setActive(false);")
        .unwrap();
    key(&mut vm, &gui, KEY_RETURN);
    assert_eq!(count(&vm, "$keyed"), "0");
}
