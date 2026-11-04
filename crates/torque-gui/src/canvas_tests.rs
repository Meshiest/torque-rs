use std::any::Any;
use std::rc::Rc;

use super::*;
use crate::ext::GuiExtension;

/// An extension that names a 4:3 default canvas.
struct Square;

impl GuiExtension for Square {
    fn into_any(self: Rc<Self>) -> Rc<dyn Any> {
        self
    }

    fn default_canvas_size(&self) -> (i32, i32) {
        (800, 600)
    }
}

fn canvas_of(vm: &mut Vm, gui: &Gui) -> (i32, i32) {
    assert!(create_canvas(vm, gui).unwrap());
    gui.0.borrow().canvas_size
}

#[test]
fn the_default_canvas_is_16_by_9() {
    assert_eq!(DEFAULT_CANVAS_SIZE, (1280, 720));
    let mut vm = Vm::new();
    let gui = crate::register(&mut vm);
    assert_eq!(gui.0.borrow().canvas_size, (1280, 720), "before the canvas");
    assert_eq!(canvas_of(&mut vm, &gui), (1280, 720));
}

#[test]
fn a_saved_resolution_wins_over_the_default() {
    let mut vm = Vm::new();
    let gui = crate::register(&mut vm);
    vm.set_global("$pref::Video::resolution", "1024 768 32");
    assert_eq!(canvas_of(&mut vm, &gui), (1024, 768));
}

#[test]
fn the_resolution_preference_falls_back_to_the_given_default() {
    let mut vm = Vm::new();
    for (pref, size) in [
        ("1680 1050 32 60", (1680, 1050)),
        ("640x480", (640, 480)),
        ("", (1280, 720)),
        ("1024", (1024, 720)),
        ("99999 5", (1280, 720)),
        ("-5 600", (1280, 720)),
    ] {
        vm.set_global("$pref::Video::resolution", pref);
        assert_eq!(resolution(&vm, DEFAULT_CANVAS_SIZE), size, "{pref:?}");
    }
    vm.set_global("$pref::Video::resolution", "");
    assert_eq!(resolution(&vm, (800, 600)), (800, 600));
}

#[test]
fn a_games_extension_names_its_own_default() {
    let mut vm = Vm::new();
    let gui = crate::register(&mut vm);
    gui.set_extension(Rc::new(Square));
    assert_eq!(gui.0.borrow().canvas_size, (800, 600), "before the canvas");
    assert_eq!(canvas_of(&mut vm, &gui), (800, 600));
}
