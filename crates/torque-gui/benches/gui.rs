//! A frame of a busy menu: the canvas walked and its quads laid out (`draw_canvas`), with nothing changed since the last frame.

use std::any::Any;
use std::fmt::Write as _;
use std::hint::black_box;
use std::rc::Rc;

use criterion::{Criterion, criterion_group, criterion_main};
use torque_fonts::{Fallback, Font, FontCache, Glyph, Remap, Tab, Units};
use torque_gui::{DrawCache, FontSource, Gui, GuiExtension, canvas, draw_canvas};
use torque_ts_vm::vm::Vm;

/// Fonts whose printable characters are 7 pixels apart, each glyph a 6 x 10 block of one sheet.
struct BlockFonts;

impl Fallback for BlockFonts {
    // Makes one generic face for any name: none is a system face a registration could replace.
    fn resolves(&self, _face: &str) -> bool {
        false
    }

    fn make(&self, _face: &str, _size: i32) -> Option<Font> {
        let glyph = Glyph {
            width: 6,
            height: 10,
            y_origin: 10,
            x_increment: 7,
            ..Glyph::default()
        };
        Some(Font {
            height: 12,
            baseline: 10,
            ascent: 10,
            descent: 2,
            glyphs: vec![glyph; 95],
            sheets: vec![torque_images::Bitmap {
                width: 8,
                height: 16,
                format: torque_images::Format::Alpha,
                bits: vec![255; 128],
            }],
            remap: Remap::new(32, (0..95).collect()),
            units: Units::Utf16,
            tab: Tab::Spaces,
        })
    }

    fn name(&self) -> &'static str {
        "block"
    }
}

struct Host;

impl GuiExtension for Host {
    fn into_any(self: Rc<Self>) -> Rc<dyn Any> {
        self
    }

    fn fonts(&self) -> Option<Box<dyn FontSource>> {
        Some(Box::new(FontCache::new("fonts", Box::new(BlockFonts))))
    }
}

/// A 1280 x 720 canvas of ten panels, each with fifteen labels of eight characters and fourteen buttons of six: 300 controls,
/// about 2,000 glyphs.
fn menu() -> (Vm, Gui) {
    let mut vm = Vm::new();
    let gui = torque_gui::register(&mut vm);
    gui.set_extension(Rc::new(Host));
    gui.force_canvas_size(Some((1280, 720)));
    assert!(canvas::create_canvas(&mut vm, &gui).expect("a canvas"));
    let mut script = String::from(
        "new GuiControlProfile(PanelProfile) { opaque = 1; fillColor = \"30 30 40 255\"; border = 1; \
           borderColor = \"90 90 110 255\"; }; \
         new GuiControlProfile(LabelProfile) { fontType = \"Arial\"; fontSize = 14; fontColor = \"230 230 230 255\"; }; \
         new GuiControlProfile(ButtonProfile) { fontType = \"Arial\"; fontSize = 14; fontColor = \"255 255 255 255\"; \
           opaque = 1; fillColor = \"60 70 90 255\"; border = 1; borderColor = \"120 130 150 255\"; }; \
         new GuiControl(Menu) { profile = PanelProfile; extent = \"1280 720\"; visible = 1; ",
    );
    for p in 0..10 {
        let (x, y) = ((p % 5) * 256, (p / 5) * 360);
        let _ = write!(
            script,
            "new GuiControl() {{ profile = PanelProfile; position = \"{x} {y}\"; extent = \"250 350\"; visible = 1; "
        );
        for k in 0..15 {
            let _ = write!(
                script,
                "new GuiTextCtrl() {{ profile = LabelProfile; position = \"4 {}\"; extent = \"120 20\"; visible = 1; \
                 text = \"Name {p}{k:03}\"; }}; ",
                4 + k * 22
            );
        }
        for k in 0..14 {
            let _ = write!(
                script,
                "new GuiButtonCtrl() {{ profile = ButtonProfile; position = \"130 {}\"; extent = \"110 20\"; visible = 1; \
                 enabled = 1; text = \"Go {k:03}\"; }}; ",
                4 + k * 24
            );
        }
        script.push_str("}; ");
    }
    script.push_str("};");
    vm.eval(script.as_bytes()).expect("the menu");
    let top = vm.objects().find(b"Menu").expect("the menu");
    canvas::set_content(&mut vm, &gui, top).expect("the menu shown");
    (vm, gui)
}

fn gui_draw(c: &mut Criterion) {
    let (vm, gui) = menu();
    let mut cache = DrawCache::for_gui(&gui);
    let frame = draw_canvas(&vm, &gui, &mut cache);
    assert!(
        frame.draws.quads() > 2_000,
        "the menu draws its glyphs: {}",
        frame.draws.quads()
    );
    c.bench_function("gui draw 300 controls", |b| {
        b.iter(|| black_box(draw_canvas(&vm, &gui, &mut cache).draws.quads()))
    });
}

criterion_group!(benches, gui_draw);
criterion_main!(benches);
