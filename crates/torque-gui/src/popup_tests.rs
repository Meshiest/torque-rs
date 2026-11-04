use std::any::Any;
use std::rc::Rc;

use torque_fonts::{Fallback, Font, FontCache, Remap, Tab, Units};

use super::*;
use crate::draw::{DrawCache, draw_canvas};
use crate::ext::{FontSource, GuiExtension};

/// A font of 12 pixel rows with no glyphs, made for any face and size.
struct PlainFonts;

impl Fallback for PlainFonts {
    fn make(&self, _face: &str, _size: i32) -> Option<Font> {
        Some(Font {
            height: 12,
            baseline: 10,
            ascent: 10,
            descent: 2,
            glyphs: Vec::new(),
            sheets: Vec::new(),
            remap: Remap::default(),
            units: Units::Utf16,
            tab: Tab::Spaces,
        })
    }

    fn name(&self) -> &'static str {
        "plain"
    }
}

/// An extension that only supplies the fonts: no game behind the GUI.
struct FontsOnly;

impl GuiExtension for FontsOnly {
    fn into_any(self: Rc<Self>) -> Rc<dyn Any> {
        self
    }

    fn fonts(&self) -> Option<Box<dyn FontSource>> {
        Some(Box::new(FontCache::new("fonts", Box::new(PlainFonts))))
    }
}

#[test]
fn an_open_popup_list_is_awake_visible_and_drawn_on_a_bare_canvas() {
    let mut vm = Vm::new();
    let gui = crate::register(&mut vm);
    gui.set_extension(Rc::new(FontsOnly));
    gui.force_canvas_size(Some((400, 300)));
    assert!(crate::canvas::create_canvas(&mut vm, &gui).unwrap());
    vm.eval(
        b"new GuiControlProfile(PopProfile) { fontType = \"Arial\"; fontSize = 12; opaque = true; \
          fillColor = \"200 200 200\"; border = true; };\
          new GuiControl(Main) { extent = \"400 300\"; \
            new GuiPopUpMenuCtrl(Pop) { profile = \"PopProfile\"; enabled = 1; position = \"20 20\"; extent = \"100 20\"; }; };\
          Canvas.setContent(Main);\
          Pop.add(\"one\", 1); Pop.add(\"two\", 2);",
    )
    .unwrap();
    let pop = vm.objects().find(b"Pop").expect("Pop");
    let mut cache = DrawCache::for_gui(&gui);
    let before = draw_canvas(&vm, &gui, &mut cache).draws.quads();

    open(&mut vm, &gui, pop).unwrap();

    let parts = gui.with(pop, |c| c.popup).flatten().expect("popup is open");
    for (what, id) in [
        ("list", parts.list),
        ("scroll", parts.scroll),
        ("background", parts.background),
    ] {
        assert!(gui.is_awake(id), "{what} is awake");
        assert!(flag(&vm, id, b"visible"), "{what} is visible");
    }
    let after = draw_canvas(&vm, &gui, &mut cache).draws.quads();
    assert!(
        after > before,
        "the open list is drawn ({before} -> {after})"
    );
}
