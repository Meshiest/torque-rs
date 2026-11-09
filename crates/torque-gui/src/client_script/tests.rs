//! The client-script GUI: the classes, the applier's checks, the host layers, the chip, the Escape filter, user activation and
//! the eval-path canary.

mod fuzz;
mod layout;
mod prompt;

use std::any::Any;
use std::rc::Rc;
use std::sync::Arc;

use torque_client_script_format::ui::{Kind, MAX_CONTROLS, MAX_DEPTH, Prop, PropValue};
use torque_client_script_format::{Grants, Tier, Writer, event, op};
use torque_engine::{Engine, EngineBuilder, Plugin, PluginId};
use torque_fonts::{Fallback, Font, FontCache, Glyph, Remap, Tab, Units};
use torque_ts_vm::sim::ObjId;
use torque_ts_vm::vm::Vm;

use super::{
    Applied, Changed, ContentCheck, DrawPlan, Hold, LayerAction, LayerMark, Realm,
    RealmLayoutEntry, RealmView, TierToggle, UiEvent, Violation, add_controls, class_names,
    encode_events,
};
use crate::Gui;
use crate::canvas::create_canvas;
use crate::control::bounds;
use crate::custom::ControlEntry;
use crate::draw::{DrawCache, draw_canvas};
use crate::input::InputEvent;
use crate::keys::{
    KEY_BUTTON0, KEY_ESCAPE, KEY_RETURN, KEYBOARD_DEVICE, MOUSE_DEVICE, SI_BREAK, SI_BUTTON,
    SI_KEY, SI_MAKE, SI_SHIFT,
};

#[derive(PluginId)]
#[plugin(id = "test.client-script")]
struct TestPlugin;

impl Plugin for TestPlugin {
    fn build(&self, engine: &mut EngineBuilder) {
        add_controls(engine);
    }
}

/// Fonts whose printable characters are 6 pixels apart, each glyph a 5 x 8 block of one sheet.
struct BlockFonts;

impl Fallback for BlockFonts {
    fn make(&self, _face: &str, _size: i32) -> Option<Font> {
        let glyph = Glyph {
            width: 5,
            height: 8,
            y_origin: 8,
            x_increment: 6,
            ..Glyph::default()
        };
        Some(Font {
            height: 10,
            baseline: 8,
            ascent: 8,
            descent: 2,
            glyphs: vec![glyph; 95],
            sheets: vec![torque_images::Bitmap {
                width: 8,
                height: 8,
                format: torque_images::Format::Alpha,
                bits: vec![255; 64],
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

/// The host's side of the GUI in these tests: the block fonts.
struct Host;

impl crate::GuiExtension for Host {
    fn into_any(self: Rc<Self>) -> Rc<dyn Any> {
        self
    }

    fn fonts(&self) -> Option<Box<dyn crate::FontSource>> {
        Some(Box::new(FontCache::new("fonts", Box::new(BlockFonts))))
    }
}

/// A PNG of `w` x `h` pixels of one color.
pub(super) fn png(w: u32, h: u32, rgb: [u8; 3]) -> Vec<u8> {
    let mut out = Vec::new();
    {
        let mut e = ::png::Encoder::new(&mut out, w, h);
        e.set_color(::png::ColorType::Rgb);
        e.set_depth(::png::BitDepth::Eight);
        let mut wr = e.write_header().expect("the header");
        let pixels: Vec<u8> = (0..w * h).flat_map(|_| rgb).collect();
        wr.write_image_data(&pixels).expect("the pixels");
    }
    out
}

/// A server's store holding a 2 x 2 PNG at every path under `prefix`.
pub(super) fn store(prefix: &'static str) -> ContentCheck {
    let bytes: Arc<[u8]> = png(2, 2, [10, 200, 30]).into();
    Box::new(move |p| p.starts_with(prefix).then(|| Arc::clone(&bytes)))
}

fn engine() -> Engine {
    EngineBuilder::new()
        .add_plugins(TestPlugin)
        .build()
        .expect("the engine builds")
}

/// Builds the ops of a flush.
#[derive(Default)]
pub(super) struct Ops(pub(super) Writer);

impl Ops {
    pub(super) fn record(&mut self, kind: u8, body: impl FnOnce(&mut Writer)) -> &mut Ops {
        let mark = self.0.begin(Tier::Ui.id(), kind);
        body(&mut self.0);
        self.0.end(mark).expect("the record fits");
        self
    }

    pub(super) fn root(&mut self, handle: u32, name: &str) -> &mut Ops {
        self.record(op::ui::ROOT, |w| {
            w.u32(handle);
            w.text(name);
        })
    }

    pub(super) fn create(
        &mut self,
        handle: u32,
        parent: u32,
        kind: Kind,
        props: &[(Prop, PropValue)],
    ) -> &mut Ops {
        self.record(op::ui::CREATE, |w| {
            w.u32(handle);
            w.u32(parent);
            w.u8(kind as u8);
            write_sorted(w, props);
        })
    }

    pub(super) fn set(&mut self, handle: u32, props: &[(Prop, PropValue)]) -> &mut Ops {
        self.record(op::ui::SET, |w| {
            w.u32(handle);
            write_sorted(w, props);
        })
    }

    pub(super) fn destroy(&mut self, handle: u32) -> &mut Ops {
        self.record(op::ui::DESTROY, |w| w.u32(handle))
    }

    pub(super) fn parent(&mut self, handle: u32, parent: u32) -> &mut Ops {
        self.record(op::ui::PARENT, |w| {
            w.u32(handle);
            w.u32(parent);
        })
    }

    pub(super) fn order(&mut self, handle: u32, front: bool) -> &mut Ops {
        self.record(op::ui::ORDER, |w| {
            w.u32(handle);
            w.bool(front);
        })
    }

    pub(super) fn interactive(&mut self, on: bool) -> &mut Ops {
        self.record(op::ui::INTERACTIVE, |w| w.bool(on))
    }

    pub(super) fn focus(&mut self, handle: u32) -> &mut Ops {
        self.record(op::ui::FOCUS, |w| w.u32(handle))
    }

    pub(super) fn bytes(&self) -> &[u8] {
        self.0.bytes()
    }
}

fn write_sorted(w: &mut Writer, props: &[(Prop, PropValue)]) {
    let mut sorted: Vec<&(Prop, PropValue)> = props.iter().collect();
    sorted.sort_by_key(|(p, _)| *p as u8);
    torque_client_script_format::ui::write_props(w, sorted.into_iter().map(|(p, v)| (*p, v)));
}

pub(super) fn text(s: &str) -> PropValue {
    PropValue::Text(s.to_string())
}

pub(super) fn vec2(x: f64, y: f64) -> PropValue {
    PropValue::Vec2(x, y)
}

/// A client's GUI with a 640 x 480 canvas, a content `PlayGui` (a marked screen), another `MenuGui`, two dialogs, and a realm.
pub(super) struct Rig {
    pub(super) vm: Vm,
    pub(super) gui: Gui,
    pub(super) realm: Realm,
    cache: DrawCache,
    clock: u64,
}

pub(super) fn view(tiers: &[Tier]) -> RealmView {
    RealmView {
        address: "10.0.0.1:28000".into(),
        name: "A Server".into(),
        tiers: tiers
            .iter()
            .map(|&tier| TierToggle { tier, on: true })
            .collect(),
    }
}

impl Rig {
    pub(super) fn new(tiers: &[Tier]) -> Rig {
        Rig::of(&engine(), tiers)
    }

    /// A rig of the engine `engine`, whose plugins may have added a layout.
    pub(super) fn of(engine: &Engine, tiers: &[Tier]) -> Rig {
        let mut vm = Vm::new();
        let gui = crate::register(&mut vm);
        gui.set_extension(Rc::new(Host));
        gui.add_controls(engine.list::<ControlEntry>());
        gui.add_realm_layouts(engine.list::<RealmLayoutEntry>());
        engine
            .replay(torque_engine::script::CLIENT_SCRIPT_STAGE, &mut vm)
            .expect("the classes register");
        gui.force_canvas_size(Some((640, 480)));
        assert!(create_canvas(&mut vm, &gui).expect("the canvas"));
        vm.eval(
            b"new GuiControl(PlayGui) { position = \"0 0\"; extent = \"640 480\"; };\
              new GuiControl(MenuGui) { position = \"0 0\"; extent = \"640 480\"; };\
              new GuiControl(DialogOne) { position = \"0 0\"; extent = \"640 480\"; };\
              new GuiControl(DialogTwo) { position = \"0 0\"; extent = \"640 480\"; };\
              Canvas.setContent(PlayGui);",
        )
        .expect("the game's screens");
        gui.set_client_script_screens(&["PlayGui"]);
        let mut grants = Grants::of(tiers);
        grants = grants.with(Tier::Ui);
        let realm = Realm::new(&mut vm, &gui, grants, view(tiers)).expect("the realm starts");
        let cache = DrawCache::for_gui(&gui);
        let mut rig = Rig {
            vm,
            gui,
            realm,
            cache,
            clock: 0,
        };
        rig.frame();
        rig
    }

    /// Applies a flush and returns what it did.
    pub(super) fn apply(&mut self, ops: &Ops) -> Result<Applied, Violation> {
        self.clock += 1000;
        let result = self
            .realm
            .apply_flush(&mut self.vm, &self.gui, ops.bytes(), self.clock);
        self.frame();
        result
    }

    pub(super) fn apply_ok(&mut self, ops: &Ops) {
        self.apply(ops).expect("the flush applies");
    }

    pub(super) fn frame(&mut self) {
        self.gui.frame(&mut self.vm);
    }

    pub(super) fn game(&mut self, script: &str) {
        self.vm.eval(script.as_bytes()).expect("the game's script");
        self.frame();
    }

    pub(super) fn draw(&mut self) -> crate::draw::DrawReport {
        draw_canvas(&self.vm, &self.gui, &mut self.cache)
            .draws
            .report
    }

    pub(super) fn events(&self) -> Vec<UiEvent> {
        self.gui.take_client_script_events()
    }

    pub(super) fn move_to(&mut self, x: i32, y: i32) {
        assert!(self.gui.mouse_moved(&mut self.vm, (x, y), 0).is_none());
    }

    pub(super) fn button(&mut self, action: u8) -> bool {
        let ev = InputEvent {
            device: MOUSE_DEVICE,
            obj_type: SI_BUTTON,
            obj_inst: KEY_BUTTON0,
            action,
            ..InputEvent::default()
        };
        let (consumed, error) = self.gui.input_event(&mut self.vm, &ev);
        assert!(error.is_none());
        consumed
    }

    pub(super) fn click_at(&mut self, x: i32, y: i32) -> bool {
        self.move_to(x, y);
        let down = self.button(SI_MAKE);
        let up = self.button(SI_BREAK);
        down && up
    }

    pub(super) fn key(&mut self, key: u16, ascii: u16, action: u8, modifier: u8) -> bool {
        let ev = InputEvent {
            device: KEYBOARD_DEVICE,
            obj_type: SI_KEY,
            obj_inst: key,
            action,
            ascii,
            modifier,
            ..InputEvent::default()
        };
        let (consumed, error) = self.gui.input_event(&mut self.vm, &ev);
        assert!(error.is_none());
        consumed
    }

    pub(super) fn object(&self, handle: u32) -> ObjId {
        self.realm.object(handle).expect("a live handle")
    }

    /// The number of objects of the `ClientScript*` classes alive.
    pub(super) fn realm_objects(&self) -> usize {
        let objects = self.vm.objects();
        objects
            .iter()
            .filter(|(_, o)| objects.classes.name(o.class()).starts_with(b"ClientScript"))
            .count()
    }
}

/// A root `ui` with a button at (10, 10) of 100 x 30.
fn button_ops() -> Ops {
    let mut ops = Ops::default();
    ops.root(1, "mod").create(
        2,
        1,
        Kind::Button,
        &[
            (Prop::Pos, vec2(10.0, 10.0)),
            (Prop::Size, vec2(100.0, 30.0)),
            (Prop::Text, text("Go")),
        ],
    );
    ops
}

#[test]
fn client_script_classes_are_registered_below_gui_control() {
    let rig = Rig::new(&[Tier::Ui]);
    let objects = rig.vm.objects();
    let parent = objects.classes.find(b"GuiControl").expect("GuiControl");
    let names = class_names();
    assert_eq!(
        names.len(),
        15,
        "a root, eleven kinds, the container group, the chip and the prompt"
    );
    for name in names {
        assert!(name.starts_with("ClientScript"));
        let class = objects.classes.find(name.as_bytes()).expect(name);
        assert!(objects.classes.is_a(class, parent), "{name}");
    }
}

#[test]
fn client_script_applier_builds_a_tree_with_the_sizing_rules() {
    let mut rig = Rig::new(&[Tier::Ui]);
    rig.apply_ok(&button_ops());
    assert_eq!(rig.realm.control_count(), 1);
    let (pos, ext) = bounds(&rig.vm, rig.object(2));
    assert_eq!((pos, ext), ((10, 10), (100, 30)));
    // A root follows the server area; a control sized `width` follows its root.
    let mut ops = Ops::default();
    ops.create(
        3,
        1,
        Kind::Panel,
        &[
            (Prop::Size, vec2(100.0, 100.0)),
            (Prop::HSize, PropValue::Enum(1)),
            (Prop::VSize, PropValue::Enum(1)),
        ],
    );
    rig.apply_ok(&ops);
    rig.gui.set_canvas_size((800, 600));
    rig.frame();
    assert_eq!(bounds(&rig.vm, rig.object(1)).1, (800, 600));
    assert_eq!(
        bounds(&rig.vm, rig.object(3)).1,
        (260, 220),
        "grown by the root's 160 x 120"
    );
    rig.realm
        .check(&rig.vm, &rig.gui)
        .expect("the invariants hold");
}

#[test]
fn client_script_destroy_removes_the_subtree_and_never_reuses_a_number() {
    let mut rig = Rig::new(&[Tier::Ui]);
    let mut ops = button_ops();
    ops.create(3, 2, Kind::Panel, &[]);
    rig.apply_ok(&ops);
    let (child, parent) = (rig.object(3), rig.object(2));
    let mut ops = Ops::default();
    ops.destroy(2);
    rig.apply_ok(&ops);
    assert!(!rig.vm.objects().contains(parent) && !rig.vm.objects().contains(child));
    assert_eq!(rig.realm.control_count(), 0);
    let mut again = Ops::default();
    again.create(2, 1, Kind::Panel, &[]);
    assert!(
        rig.apply(&again).is_err(),
        "a destroyed number never comes back"
    );
    let mut next = Ops::default();
    next.create(4, 1, Kind::Panel, &[]);
    rig.apply_ok(&next);
}

#[test]
fn client_script_order_and_parent_move_controls() {
    let mut rig = Rig::new(&[Tier::Ui]);
    let mut ops = Ops::default();
    ops.root(1, "mod")
        .create(2, 1, Kind::Panel, &[])
        .create(3, 1, Kind::Panel, &[])
        .create(4, 2, Kind::Panel, &[]);
    rig.apply_ok(&ops);
    let kids = |rig: &Rig, h: u32| crate::control::children(&rig.vm, rig.object(h));
    assert_eq!(kids(&rig, 1), [rig.object(2), rig.object(3)]);
    let mut ops = Ops::default();
    ops.order(2, true);
    rig.apply_ok(&ops);
    assert_eq!(
        kids(&rig, 1),
        [rig.object(3), rig.object(2)],
        "front is drawn last"
    );
    let mut ops = Ops::default();
    ops.order(2, false).parent(4, 3);
    rig.apply_ok(&ops);
    assert_eq!(kids(&rig, 1), [rig.object(2), rig.object(3)]);
    assert_eq!(kids(&rig, 3), [rig.object(4)]);
    rig.realm
        .check(&rig.vm, &rig.gui)
        .expect("the invariants hold");
}

/// Each op is a violation that changes nothing.
#[test]
fn client_script_applier_refuses_what_a_checked_guest_never_sends() {
    let mut rig = Rig::new(&[Tier::Ui, Tier::Net]);
    let mut base = button_ops();
    base.create(3, 1, Kind::TextField, &[])
        .create(
            4,
            1,
            Kind::List,
            &[(Prop::Rows, PropValue::Rows(vec!["a".into(), "b".into()]))],
        )
        .create(5, 1, Kind::Slider, &[])
        .create(6, 2, Kind::Panel, &[]);
    rig.apply_ok(&base);
    let cases: Vec<(&str, Ops)> = vec![
        ("a wrong next handle", {
            let mut o = Ops::default();
            o.create(9, 1, Kind::Panel, &[]);
            o
        }),
        ("an unknown parent", {
            let mut o = Ops::default();
            o.create(7, 77, Kind::Panel, &[]);
            o
        }),
        ("a second root for a mod", {
            let mut o = Ops::default();
            o.root(7, "mod");
            o
        }),
        ("a bad mod id", {
            let mut o = Ops::default();
            o.root(7, "../x");
            o
        }),
        ("a prop the kind lacks", {
            let mut o = Ops::default();
            o.create(
                7,
                1,
                Kind::Text,
                &[(Prop::Fill, PropValue::Color([1, 2, 3, 4]))],
            );
            o
        }),
        ("a number out of range", {
            let mut o = Ops::default();
            o.create(
                7,
                1,
                Kind::Panel,
                &[(Prop::Opacity, PropValue::Number(2.0))],
            );
            o
        }),
        ("a font size below 8", {
            let mut o = Ops::default();
            o.set(2, &[(Prop::FontSize, PropValue::Number(7.0))]);
            o
        }),
        ("a text with a bidirectional override", {
            let mut o = Ops::default();
            o.create(7, 1, Kind::Text, &[(Prop::Text, text("a\u{202e}b"))]);
            o
        }),
        ("a text with a line separator", {
            let mut o = Ops::default();
            o.set(2, &[(Prop::Tooltip, text("a\u{2028}b"))]);
            o
        }),
        ("an image named like a device", {
            let mut o = Ops::default();
            o.create(
                7,
                1,
                Kind::Image,
                &[(Prop::Image, text("mods/mod/con.png"))],
            );
            o
        }),
        ("markup with a link", {
            let mut o = Ops::default();
            o.create(
                7,
                1,
                Kind::RichText,
                &[(Prop::RichText, text("<a:http://x>x</a>"))],
            );
            o
        }),
        ("an image outside the content", {
            let mut o = Ops::default();
            o.create(7, 1, Kind::Image, &[(Prop::Image, text("../prefs.cs"))]);
            o
        }),
        ("control text past the cap", {
            let mut o = Ops::default();
            o.set(2, &[(Prop::Text, text(&"y".repeat(4097)))]);
            o
        }),
        ("list rows past the list cap", {
            let mut o = Ops::default();
            o.set(
                4,
                &[(Prop::Rows, PropValue::Rows(vec!["r".repeat(4000); 5]))],
            );
            o
        }),
        ("one row past the row cap", {
            let mut o = Ops::default();
            o.set(4, &[(Prop::Rows, PropValue::Rows(vec!["r".repeat(4097)]))]);
            o
        }),
        ("a selected row past the rows", {
            let mut o = Ops::default();
            o.set(4, &[(Prop::Selected, PropValue::Int(3))]);
            o
        }),
        ("a slider value outside its range", {
            let mut o = Ops::default();
            o.set(5, &[(Prop::SliderValue, PropValue::Number(5.0))]);
            o
        }),
        ("a range with x not below y", {
            let mut o = Ops::default();
            o.set(5, &[(Prop::Range, vec2(3.0, 3.0))]);
            o
        }),
        ("a set of a mod root", {
            let mut o = Ops::default();
            o.set(1, &[]);
            o
        }),
        ("a destroy of a mod root", {
            let mut o = Ops::default();
            o.destroy(1);
            o
        }),
        ("a control under itself", {
            let mut o = Ops::default();
            o.parent(2, 6);
            o
        }),
        ("a control that is its own parent", {
            let mut o = Ops::default();
            o.parent(2, 2);
            o
        }),
        ("a move of a mod root", {
            let mut o = Ops::default();
            o.parent(1, 2);
            o
        }),
        ("a focus on a button", {
            let mut o = Ops::default();
            o.focus(2);
            o
        }),
        ("a set of a handle never made", {
            let mut o = Ops::default();
            o.set(42, &[]);
            o
        }),
        ("an unknown ui op", {
            let mut o = Ops::default();
            o.record(200, |_| {});
            o
        }),
        ("a body with bytes left over", {
            let mut o = Ops::default();
            o.record(op::ui::DESTROY, |w| {
                w.u32(2);
                w.u8(0);
            });
            o
        }),
        (
            "a net record the grants allow but a draw one they do not",
            {
                let mut w = Ops::default();
                let mark = w.0.begin(Tier::Draw.id(), 0);
                w.0.end(mark).unwrap();
                w
            },
        ),
    ];
    let before = rig.realm.handles();
    for (what, ops) in cases {
        assert!(rig.apply(&ops).is_err(), "{what} was applied");
        rig.realm
            .check(&rig.vm, &rig.gui)
            .unwrap_or_else(|e| panic!("{what}: {e}"));
        assert_eq!(rig.realm.handles(), before, "{what} changed the tree");
    }
}

#[test]
fn client_script_an_op_of_an_ungranted_tier_is_refused() {
    let mut rig = Rig::new(&[Tier::Ui]);
    let mut ops = Ops::default();
    let mark = ops.0.begin(Tier::Net.id(), event::net::MESSAGE);
    ops.0.end(mark).unwrap();
    assert_eq!(
        rig.apply(&ops),
        Err(Violation("an op of a tier the realm does not have"))
    );
    let granted = Rig::new(&[Tier::Ui, Tier::Net]);
    assert!(granted.realm.grants().has(Tier::Net) && !granted.realm.grants().has(Tier::Draw));
}

#[test]
fn client_script_the_caps_hold() {
    let mut rig = Rig::new(&[Tier::Ui]);
    let mut ops = Ops::default();
    ops.root(1, "mod");
    for h in 2..=1 + MAX_CONTROLS as u32 {
        ops.create(h, 1, Kind::Panel, &[]);
    }
    // One flush holds at most 4096 ops, so all of these fit.
    rig.apply_ok(&ops);
    assert_eq!(rig.realm.control_count(), MAX_CONTROLS);
    let mut one_more = Ops::default();
    one_more.create(1 + MAX_CONTROLS as u32 + 1, 1, Kind::Panel, &[]);
    assert!(rig.apply(&one_more).is_err());

    let mut deep = Rig::new(&[Tier::Ui]);
    let mut ops = Ops::default();
    ops.root(1, "mod");
    for level in 1..=MAX_DEPTH as u32 {
        ops.create(level + 1, level, Kind::Panel, &[]);
    }
    deep.apply_ok(&ops);
    let mut past = Ops::default();
    past.create(MAX_DEPTH as u32 + 2, MAX_DEPTH as u32 + 1, Kind::Panel, &[]);
    assert_eq!(
        deep.apply(&past),
        Err(Violation("a control past the depth or count cap"))
    );
    deep.realm
        .check(&deep.vm, &deep.gui)
        .expect("the invariants hold");
}

#[test]
fn client_script_images_and_text_are_counted_across_the_realm() {
    let mut rig = Rig::new(&[Tier::Ui]);
    rig.realm.set_content_check(store("mods/mod/"));
    let mut ops = Ops::default();
    ops.root(1, "mod");
    for h in 0..256u32 {
        ops.create(
            h + 2,
            1,
            Kind::Image,
            &[(Prop::Image, text(&format!("mods/mod/{h}.png")))],
        );
    }
    rig.apply_ok(&ops);
    let mut more = Ops::default();
    more.create(
        258,
        1,
        Kind::Image,
        &[(Prop::Image, text("mods/mod/extra.png"))],
    );
    assert!(rig.apply(&more).is_err(), "257 distinct images");
    let mut same = Ops::default();
    same.create(
        258,
        1,
        Kind::Image,
        &[(Prop::Image, text("mods/mod/0.png"))],
    );
    rig.apply_ok(&same);
    let mut other = Ops::default();
    other.create(
        259,
        1,
        Kind::Image,
        &[(Prop::Image, text("mods/other/0.png"))],
    );
    assert!(rig.apply(&other).is_err(), "another server's content");
}

#[test]
fn client_script_revoking_ui_removes_every_control_at_once() {
    let mut rig = Rig::new(&[Tier::Ui, Tier::Net]);
    let mut ops = button_ops();
    ops.interactive(true);
    rig.apply_ok(&ops);
    assert!(rig.gui.client_script_holds().cursor);
    rig.realm.revoke(&mut rig.vm, &rig.gui, Tier::Ui).unwrap();
    assert_eq!(rig.realm.control_count(), 0);
    assert_eq!(rig.realm_objects(), 2, "the layer's container and the chip");
    assert!(!rig.gui.client_script_holds().cursor);
    let mut again = Ops::default();
    again.create(3, 1, Kind::Panel, &[]);
    assert!(rig.apply(&again).is_err(), "the tier is gone");
    rig.realm
        .check(&rig.vm, &rig.gui)
        .expect("the invariants hold");
}

#[test]
fn client_script_teardown_leaves_no_objects_and_restores_the_cursor() {
    let mut rig = Rig::new(&[Tier::Ui]);
    rig.gui.0.borrow_mut().cursor_on = false;
    let mut ops = button_ops();
    ops.interactive(true);
    rig.apply_ok(&ops);
    assert!(
        rig.gui.0.borrow().cursor_on,
        "the realm turned the cursor on"
    );
    assert!(rig.realm_objects() > 0);
    let Rig {
        mut vm, gui, realm, ..
    } = rig;
    realm.teardown(&mut vm, &gui);
    assert!(!gui.0.borrow().cursor_on, "the cursor is as it was");
    let objects = vm.objects();
    let left = objects
        .iter()
        .filter(|(_, o)| objects.classes.name(o.class()).starts_with(b"ClientScript"))
        .count();
    assert_eq!(left, 0);
    assert!(!gui.client_script_active());
    assert!(gui.take_client_script_events().is_empty());
}

// ---- layers

fn marks(report: &crate::draw::DrawReport) -> Vec<LayerMark> {
    report.layers.clone()
}

#[test]
fn client_script_the_server_layer_is_between_content_and_dialogs_and_the_chip_is_last() {
    let mut rig = Rig::new(&[Tier::Ui]);
    rig.apply_ok(&button_ops());
    let report = rig.draw();
    assert_eq!(
        marks(&report),
        [
            LayerMark::Content,
            LayerMark::Server,
            LayerMark::Engine,
            LayerMark::Chip
        ]
    );
    assert_eq!(report.realm_controls, 1);
    rig.game("Canvas.pushDialog(DialogOne); Canvas.pushDialog(DialogTwo);");
    let report = rig.draw();
    assert_eq!(
        marks(&report),
        [
            LayerMark::Content,
            LayerMark::Server,
            LayerMark::Dialog,
            LayerMark::Dialog,
            LayerMark::Engine,
            LayerMark::Chip
        ]
    );
    assert!(report.chip && report.realm_controls == 1);
}

#[test]
fn client_script_game_screen_changes_never_move_the_server_layer() {
    let mut rig = Rig::new(&[Tier::Ui]);
    rig.apply_ok(&button_ops());
    let layer = DrawPlan::of(&rig.vm, &rig.gui).server.expect("shown");
    let steps = [
        "Canvas.pushDialog(DialogOne);",
        "Canvas.pushDialog(DialogTwo);",
        "Canvas.popLayer(0);",
        "Canvas.popDialog(DialogOne);",
        "Canvas.setContent(PlayGui);",
        "Canvas.pushDialog(DialogOne); Canvas.popLayer(0);",
    ];
    for step in steps {
        rig.game(step);
        let (content, dialogs) = rig.gui.stack(&rig.vm);
        let canvas = rig.gui.canvas().unwrap();
        let members = crate::control::children(&rig.vm, canvas);
        assert!(!members.contains(&layer), "{step}: not a canvas child");
        assert!(rig.vm.objects().contains(layer), "{step}: still there");
        assert_eq!(rig.gui.0.borrow().client_script.server, Some(layer));
        let marks = marks(&rig.draw());
        let server = marks
            .iter()
            .position(|m| *m == LayerMark::Server)
            .expect(step);
        let first_dialog = marks.iter().position(|m| *m == LayerMark::Dialog);
        assert_eq!(marks[0], LayerMark::Content, "{step}");
        assert!(
            server == 1 && first_dialog.is_none_or(|d| d > server),
            "{step}: {marks:?}"
        );
        assert_eq!(
            dialogs.len() + usize::from(content.is_some()),
            members.len()
        );
    }
    // `setContent` of the game's own other screen: the layer stays, hidden, and comes back.
    rig.game("Canvas.setContent(MenuGui);");
    assert!(DrawPlan::of(&rig.vm, &rig.gui).server.is_none());
    assert_eq!(rig.draw().realm_controls, 0);
    assert!(
        !rig.draw().chip,
        "nothing of the realm is drawn, so no chip"
    );
    rig.game("Canvas.setContent(PlayGui);");
    assert_eq!(DrawPlan::of(&rig.vm, &rig.gui).server, Some(layer));
}

#[test]
fn client_script_a_realm_control_is_not_the_games() {
    let mut rig = Rig::new(&[Tier::Ui]);
    rig.game(
        "$wakes = 0; function GuiControl::onWake(%this) { $wakes++; }\
         function GuiControl::onSleep(%this) { $wakes--; }\
         function GuiControl::onRemove(%this) { $wakes += 100; }\
         function GuiControl::onRender(%this) { $wakes += 1000; }",
    );
    let evals = rig.vm.eval_count();
    let mut ops = button_ops();
    ops.create(3, 1, Kind::Text, &[(Prop::Text, text("hi"))]);
    rig.apply_ok(&ops);
    let report = rig.draw();
    assert!(report.realm_controls >= 2);
    let frame = draw_canvas(&rig.vm, &rig.gui, &mut DrawCache::default());
    crate::draw::run_callbacks(&mut rig.vm, &frame.callbacks).unwrap();
    let mut destroy = Ops::default();
    destroy.destroy(3);
    rig.apply_ok(&destroy);
    assert_eq!(
        rig.vm.global("$wakes").as_deref(),
        Some(&b"0"[..]),
        "no callback ran"
    );
    assert_eq!(rig.vm.eval_count(), evals, "no eval");
    // Scripts read nothing of the control and their writes are dropped.
    let obj = rig.object(2);
    assert!(rig.vm.script_field(obj, b"position", b"").is_empty());
    rig.vm.script_set_field(obj, b"command", b"", b"$x = 1;");
    rig.vm.script_set_field(obj, b"altCommand", b"", b"$x = 1;");
    rig.vm.script_set_field(obj, b"variable", b"", b"$x");
    assert!(super::apply::code_field(&rig.vm, obj).is_none());
    // They are not in GuiGroup, so a script cannot walk to them.
    let group = rig.vm.objects().find(b"GuiGroup");
    if let Some(group) = group {
        let members = crate::control::children(&rig.vm, group);
        assert!(members.iter().all(|m| !rig.gui.is_realm(*m)));
    }
}

#[test]
fn client_script_string_props_set_to_code_never_run() {
    let mut rig = Rig::new(&[Tier::Ui]);
    let canary = "$ClientScript::Canary = 1;";
    let evals = rig.vm.eval_count();
    let mut ops = Ops::default();
    ops.root(1, "mod");
    let strings: &[(Kind, &[Prop])] = &[
        (Kind::Panel, &[Prop::Tooltip]),
        (Kind::Text, &[Prop::Text, Prop::Tooltip]),
        (Kind::RichText, &[Prop::RichText]),
        (Kind::Button, &[Prop::Text, Prop::Tooltip]),
        (Kind::Toggle, &[Prop::Text]),
        (Kind::TextField, &[Prop::Text, Prop::Placeholder]),
    ];
    let mut handle = 2;
    for (kind, props) in strings {
        let given: Vec<(Prop, PropValue)> = props
            .iter()
            .map(|&p| (p, text(canary)))
            .chain([
                (Prop::Size, vec2(60.0, 20.0)),
                (Prop::Pos, vec2(0.0, f64::from(handle) * 22.0)),
            ])
            .collect();
        ops.create(handle, 1, *kind, &given);
        handle += 1;
    }
    ops.create(
        handle,
        1,
        Kind::List,
        &[
            (Prop::Rows, PropValue::Rows(vec![canary.into(); 3])),
            (Prop::Size, vec2(60.0, 60.0)),
        ],
    );
    rig.apply_ok(&ops);
    rig.draw();
    for h in 2..=handle {
        let (pos, ext) = bounds(&rig.vm, rig.object(h));
        rig.click_at(pos.0 + ext.0 / 2, pos.1 + ext.1 / 2);
        rig.key(0x30, u16::from(b'a'), SI_MAKE, 0);
        rig.key(KEY_RETURN, 13, SI_MAKE, 0);
    }
    rig.frame();
    assert!(
        rig.vm
            .global("$ClientScript::Canary")
            .is_none_or(|v| v.is_empty())
    );
    assert_eq!(rig.vm.eval_count(), evals);
    rig.realm.check(&rig.vm, &rig.gui).expect("no code field");
}

// ---- input

#[test]
fn client_script_clicks_reach_controls_as_events() {
    let mut rig = Rig::new(&[Tier::Ui]);
    let mut ops = button_ops();
    ops.create(
        3,
        1,
        Kind::Toggle,
        &[
            (Prop::Pos, vec2(10.0, 50.0)),
            (Prop::Size, vec2(100.0, 20.0)),
        ],
    )
    .create(
        4,
        1,
        Kind::Panel,
        &[
            (Prop::Pos, vec2(10.0, 80.0)),
            (Prop::Size, vec2(50.0, 20.0)),
        ],
    )
    .create(
        5,
        1,
        Kind::Slider,
        &[
            (Prop::Pos, vec2(10.0, 110.0)),
            (Prop::Size, vec2(100.0, 20.0)),
            (Prop::Range, vec2(0.0, 10.0)),
            (Prop::Step, PropValue::Number(1.0)),
        ],
    )
    .create(
        6,
        1,
        Kind::List,
        &[
            (Prop::Pos, vec2(10.0, 140.0)),
            (Prop::Size, vec2(100.0, 60.0)),
            (
                Prop::Rows,
                PropValue::Rows(vec!["a".into(), "b".into(), "c".into()]),
            ),
        ],
    );
    rig.apply_ok(&ops);
    rig.events();
    assert!(rig.click_at(20, 20), "the realm takes the click");
    assert!(rig.click_at(20, 60));
    assert!(rig.click_at(20, 90));
    assert!(rig.click_at(60, 120));
    assert!(rig.click_at(20, 140 + 14 + 4 + 2));
    let events = rig.events();
    assert_eq!(
        events,
        [
            UiEvent::Press(2),
            UiEvent::Change(3, Changed::Bool(true)),
            UiEvent::Hover(4, true),
            UiEvent::Click(4),
            UiEvent::Hover(4, false),
            UiEvent::Change(5, Changed::Number(5.0)),
            UiEvent::Select(6, 2),
        ]
    );
    rig.click_at(500, 400);
    assert!(rig.events().is_empty(), "a click on nothing is the game's");
}

#[test]
fn client_script_events_encode_as_the_formats_records() {
    let events = [
        UiEvent::Click(2),
        UiEvent::Hover(3, true),
        UiEvent::Change(4, Changed::Text("hi".into())),
        UiEvent::Screen(640.0, 480.0, 1.0),
        UiEvent::Closed,
    ];
    let bytes = encode_events(&events);
    let records = torque_client_script_format::Records::new(&bytes, 16, 1 << 16)
        .unwrap()
        .collect_all()
        .unwrap();
    let kinds: Vec<u8> = records.iter().map(|r| r.kind).collect();
    assert_eq!(
        kinds,
        [
            event::ui::CLICK,
            event::ui::HOVER,
            event::ui::CHANGE,
            event::ui::SCREEN,
            event::ui::CLOSED
        ]
    );
    assert!(records.iter().all(|r| r.tier == Tier::Ui.id()));
}

#[test]
fn client_script_a_text_field_takes_text_and_navigation_only_when_focused() {
    let mut rig = Rig::new(&[Tier::Ui]);
    let mut ops = Ops::default();
    ops.root(1, "mod").create(
        2,
        1,
        Kind::TextField,
        &[
            (Prop::Pos, vec2(10.0, 10.0)),
            (Prop::Size, vec2(100.0, 20.0)),
            (Prop::MaxLength, PropValue::Int(4)),
        ],
    );
    rig.apply_ok(&ops);
    rig.events();
    assert!(
        !rig.key(0x30, u16::from(b'a'), SI_MAKE, 0),
        "no focus: the game gets the key"
    );
    assert!(rig.click_at(20, 20), "a click focuses it");
    for c in "abcde".bytes() {
        assert!(
            rig.key(0x30, u16::from(c), SI_MAKE, 0),
            "typed keys are taken"
        );
        rig.key(0x30, u16::from(c), SI_BREAK, 0);
    }
    assert!(rig.key(KEY_RETURN, 13, SI_MAKE, 0));
    let events = rig.events();
    assert_eq!(
        events.last(),
        Some(&UiEvent::Submit(2, "abcd".into())),
        "at most maxLength characters"
    );
    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(e, UiEvent::Change(..)))
            .count(),
        4
    );
    assert!(
        !rig.key(crate::keys::KEY_F1, 0, SI_MAKE, 0),
        "other keys go to the game"
    );
    assert!(
        !rig.key(0x30, u16::from(b'z'), SI_MAKE, crate::keys::SI_CTRL),
        "no shortcuts"
    );
    assert_eq!(rig.gui.client_script_stats().escape_keys, 0);
}

#[test]
fn client_script_no_input_while_a_dialog_or_prompt_is_up_or_off_a_marked_screen() {
    let mut rig = Rig::new(&[Tier::Ui]);
    let mut ops = button_ops();
    ops.create(
        3,
        1,
        Kind::TextField,
        &[
            (Prop::Pos, vec2(10.0, 50.0)),
            (Prop::Size, vec2(100.0, 20.0)),
        ],
    );
    rig.apply_ok(&ops);
    rig.click_at(20, 60);
    rig.events();
    let before = rig.gui.client_script_stats().input_events;
    rig.game("Canvas.pushDialog(DialogOne);");
    assert!(
        rig.gui.0.borrow().client_script.focus.is_none(),
        "a dialog drops the focus"
    );
    rig.click_at(20, 20);
    rig.key(0x30, u16::from(b'a'), SI_MAKE, 0);
    rig.game("Canvas.popLayer(0);");
    // An engine prompt over the server layer.
    rig.game("new GuiControl(Prompt) { position = \"0 0\"; extent = \"640 480\"; };");
    let prompt = rig.vm.objects().find(b"Prompt").unwrap();
    rig.vm
        .objects_mut()
        .remove(rig.gui.canvas().unwrap(), prompt);
    assert!(rig.gui.push_engine_layer(&mut rig.vm, prompt));
    rig.click_at(20, 20);
    rig.gui.pop_engine_layer(&mut rig.vm, prompt);
    rig.game("Canvas.setContent(MenuGui);");
    rig.click_at(20, 20);
    assert_eq!(rig.gui.client_script_stats().input_events, before);
    assert!(rig.events().iter().all(|e| !e.is_input()));
}

// ---- the chip

/// What the chip shows the realm holds.
fn chip_holds(rig: &Rig) -> Vec<&'static str> {
    let (_, chip) = chip_rect(rig);
    rig.gui
        .with_custom_state::<super::chip::ChipState, _>(chip, |s| s.holds.clone())
        .expect("chip state")
}

fn chip_rect(rig: &Rig) -> ([i32; 4], ObjId) {
    let chip = rig.gui.0.borrow().client_script.chip.expect("a chip");
    let (pos, ext) = bounds(&rig.vm, chip);
    ([pos.0, pos.1, ext.0, ext.1], chip)
}

#[test]
fn client_script_the_chip_names_the_server_and_opens_per_tier_toggles() {
    let mut rig = Rig::new(&[Tier::Ui, Tier::Net]);
    assert!(!rig.draw().chip, "nothing on screen yet");
    rig.apply_ok(&button_ops());
    assert!(rig.draw().chip);
    let (rect, chip) = chip_rect(&rig);
    assert!(
        rect[0] + rect[2] <= 640 && rect[1] >= 0,
        "in the corner, on screen"
    );
    let (label, name) = rig
        .gui
        .with_custom_state::<super::chip::ChipState, _>(chip, |s| (s.label.clone(), s.name.clone()))
        .expect("chip state");
    assert_eq!(label, "Server scripts: 10.0.0.1:28000");
    assert_eq!(name, "A Server");
    // Clicking the header opens the menu; the first row is `ui`.
    assert!(rig.click_at(rect[0] + 5, rect[1] + 5));
    rig.frame();
    let (open, _) = chip_rect(&rig);
    assert_eq!(open[3], 22 + 4 * 20, "two tiers, hide and turn off");
    assert!(rig.click_at(open[0] + 5, open[1] + 22 + 5));
    assert_eq!(
        rig.gui.take_client_script_actions(),
        [LayerAction::Revoke(Tier::Ui)]
    );
    // The server layer takes no input inside the chip's rectangle: a control under it is not clicked.
    rig.events();
    let mut ops = Ops::default();
    ops.create(
        3,
        1,
        Kind::Button,
        &[
            (Prop::Pos, vec2(f64::from(open[0]), f64::from(open[1]))),
            (Prop::Size, vec2(100.0, 30.0)),
        ],
    );
    rig.apply_ok(&ops);
    rig.click_at(open[0] + 5, open[1] + 5);
    assert!(rig.events().iter().all(|e| *e != UiEvent::Press(3)));
}

#[test]
fn client_script_hide_for_now_and_turn_off() {
    let mut rig = Rig::new(&[Tier::Ui]);
    rig.apply_ok(&button_ops());
    let (rect, _) = chip_rect(&rig);
    rig.click_at(rect[0] + 5, rect[1] + 5);
    rig.frame();
    let (open, _) = chip_rect(&rig);
    // Rows: ui, hide, turn off.
    rig.click_at(open[0] + 5, open[1] + 22 + 20 + 5);
    assert_eq!(
        rig.gui.take_client_script_actions(),
        [LayerAction::Hide(true)]
    );
    rig.frame();
    let report = rig.draw();
    assert_eq!(report.realm_controls, 0, "hidden");
    assert!(
        report.chip,
        "the chip stays so the player can show it again"
    );
    rig.click_at(open[0] + 5, open[1] + 22 + 20 + 5);
    assert_eq!(
        rig.gui.take_client_script_actions(),
        [LayerAction::Hide(false)]
    );
    assert_eq!(rig.draw().realm_controls, 1);
    rig.click_at(open[0] + 5, open[1] + 22 + 40 + 5);
    assert_eq!(rig.gui.take_client_script_actions(), [LayerAction::TurnOff]);
    assert!(!rig.gui.client_script_active());
    let report = rig.draw();
    assert!(report.realm_controls == 0 && !report.chip);
}

#[test]
fn client_script_the_chip_shows_holds() {
    let mut rig = Rig::new(&[Tier::Ui]);
    rig.apply_ok(&button_ops());
    rig.gui.client_script_hold(Hold::Camera, true);
    rig.frame();
    assert_eq!(chip_holds(&rig), ["camera"]);
    assert!(rig.gui.client_script_holds().camera);
}

// ---- Escape and focus

fn escape(rig: &mut Rig, action: u8, modifier: u8) -> bool {
    rig.key(KEY_ESCAPE, 0, action, modifier)
}

#[test]
fn client_script_escape_drops_the_holds_and_tells_the_realm() {
    let mut rig = Rig::new(&[Tier::Ui]);
    rig.gui.0.borrow_mut().cursor_on = false;
    let mut ops = button_ops();
    ops.create(
        3,
        1,
        Kind::TextField,
        &[
            (Prop::Pos, vec2(10.0, 50.0)),
            (Prop::Size, vec2(100.0, 20.0)),
        ],
    )
    .interactive(true);
    rig.apply_ok(&ops);
    assert!(rig.gui.client_script_holds().cursor && rig.gui.0.borrow().cursor_on);
    rig.click_at(20, 60);
    assert!(rig.gui.0.borrow().client_script.focus.is_some());
    rig.events();
    assert!(
        escape(&mut rig, SI_MAKE, 0),
        "the first press is the host's"
    );
    assert!(escape(&mut rig, SI_BREAK, 0), "and its release");
    assert_eq!(rig.events(), [UiEvent::Closed]);
    assert!(
        !rig.gui.client_script_holds().cursor && rig.gui.0.borrow().client_script.focus.is_none()
    );
    assert!(!rig.gui.0.borrow().cursor_on, "the cursor is as it was");
    assert!(
        !escape(&mut rig, SI_MAKE, 0),
        "with nothing held, Escape goes to the game"
    );
    assert_eq!(rig.gui.client_script_stats().escape_keys, 0);
}

#[test]
fn client_script_shift_escape_turns_server_scripts_off() {
    let mut rig = Rig::new(&[Tier::Ui]);
    rig.apply_ok(&button_ops());
    assert!(escape(&mut rig, SI_MAKE, SI_SHIFT));
    assert_eq!(rig.gui.take_client_script_actions(), [LayerAction::TurnOff]);
    assert!(!rig.gui.client_script_active());
    let report = rig.draw();
    assert!(report.realm_controls == 0 && !report.chip);
    assert!(
        escape(&mut rig, SI_BREAK, 0),
        "the release of a taken press is taken too"
    );
}

#[test]
fn client_script_focus_needs_a_recent_action_of_the_player() {
    let mut rig = Rig::new(&[Tier::Ui]);
    let mut ops = button_ops();
    ops.create(
        3,
        1,
        Kind::TextField,
        &[
            (Prop::Pos, vec2(10.0, 50.0)),
            (Prop::Size, vec2(100.0, 20.0)),
        ],
    );
    rig.apply_ok(&ops);
    let mut focus = Ops::default();
    focus.focus(3);
    let applied = rig.apply(&focus).unwrap();
    assert_eq!(applied.focus_refused, 1, "no player action yet");
    assert!(rig.gui.0.borrow().client_script.focus.is_none());
    // A press on the realm's own button counts for a second.
    rig.gui.set_client_script_clock(5_000_000);
    rig.click_at(20, 20);
    assert_eq!(
        rig.realm
            .apply_flush(&mut rig.vm, &rig.gui, focus.bytes(), 5_900_000)
            .unwrap()
            .focus_refused,
        0
    );
    assert!(rig.gui.0.borrow().client_script.focus.is_some());
    rig.gui.0.borrow_mut().client_script.focus = None;
    let late = rig
        .realm
        .apply_flush(&mut rig.vm, &rig.gui, focus.bytes(), 6_100_000)
        .unwrap();
    assert_eq!(late.focus_refused, 1, "more than a second later");
    assert!(rig.gui.0.borrow().client_script.focus.is_none());
}

#[test]
fn client_script_a_dialog_drops_the_cursor_request() {
    let mut rig = Rig::new(&[Tier::Ui]);
    rig.gui.0.borrow_mut().cursor_on = false;
    let mut ops = button_ops();
    ops.interactive(true);
    rig.apply_ok(&ops);
    assert!(rig.gui.client_script_holds().cursor);
    rig.events();
    rig.game("Canvas.pushDialog(DialogOne);");
    assert!(!rig.gui.client_script_holds().cursor);
    assert_eq!(rig.events(), [UiEvent::Closed]);
    rig.game("Canvas.popLayer(0);");
    assert!(
        !rig.gui.client_script_holds().cursor,
        "the realm has to ask again"
    );
}

#[test]
fn client_script_the_realm_is_told_the_screen_size() {
    let mut rig = Rig::new(&[Tier::Ui]);
    rig.events();
    rig.gui.set_canvas_size((800, 600));
    rig.frame();
    assert_eq!(rig.events(), [UiEvent::Screen(800.0, 600.0, 1.0)]);
}

#[test]
fn client_script_the_engine_layer_gets_input_first_and_the_chip_is_above_it() {
    let mut rig = Rig::new(&[Tier::Ui]);
    let mut ops = button_ops();
    ops.create(
        3,
        1,
        Kind::Panel,
        &[
            (Prop::Pos, vec2(300.0, 300.0)),
            (Prop::Size, vec2(50.0, 50.0)),
        ],
    );
    rig.apply_ok(&ops);
    rig.game(
        "new GuiControlProfile(GuiDefaultProfile) { modal = 1; };         new GuiControl(Prompt) { position = \"0 0\"; extent = \"640 480\";            new GuiButtonCtrl(PromptButton) { position = \"100 100\"; extent = \"100 40\"; command = \"$prompt = 1;\"; enabled = 1; visible = 1; }; };",
    );
    let prompt = rig.vm.objects().find(b"Prompt").unwrap();
    rig.vm
        .objects_mut()
        .remove(rig.gui.canvas().unwrap(), prompt);
    assert!(rig.gui.push_engine_layer(&mut rig.vm, prompt));
    rig.frame();
    assert_eq!(
        marks(&rig.draw()),
        [
            LayerMark::Content,
            LayerMark::Server,
            LayerMark::Engine,
            LayerMark::Chip
        ],
        "the prompt is drawn in the engine layer, the chip after it"
    );
    rig.events();
    assert!(rig.click_at(120, 120), "the prompt takes the click");
    assert_eq!(
        rig.vm.global("$prompt").as_deref(),
        Some(&b"1"[..]),
        "its own button ran"
    );
    assert!(
        rig.click_at(320, 320),
        "a prompt is modal over the server layer"
    );
    assert!(
        rig.events().iter().all(|e| !e.is_input()),
        "the realm saw nothing"
    );
    // The chip is above the prompt and takes its click first.
    let (chip, _) = chip_rect(&rig);
    assert!(rig.click_at(chip[0] + 5, chip[1] + 5));
    rig.frame();
    assert!(chip_rect(&rig).0[3] > 22, "the chip opened over the prompt");
    rig.gui.pop_engine_layer(&mut rig.vm, prompt);
    assert!(rig.click_at(320, 320));
    assert_eq!(rig.events(), [UiEvent::Hover(3, true), UiEvent::Click(3)]);
}

#[test]
fn client_script_a_tooltip_shows_for_the_control_under_the_pointer() {
    let mut rig = Rig::new(&[Tier::Ui]);
    let mut ops = Ops::default();
    ops.root(1, "mod").create(
        2,
        1,
        Kind::Panel,
        &[
            (Prop::Pos, vec2(10.0, 10.0)),
            (Prop::Size, vec2(50.0, 50.0)),
            (Prop::Tooltip, text("Hello")),
        ],
    );
    rig.apply_ok(&ops);
    assert_eq!(super::paint::tooltip_text(&rig.gui), None);
    rig.move_to(20, 20);
    assert_eq!(
        super::paint::tooltip_text(&rig.gui).as_deref(),
        Some("Hello")
    );
    rig.move_to(300, 300);
    assert_eq!(super::paint::tooltip_text(&rig.gui), None);
}

#[test]
fn client_script_images_are_refused_until_the_host_sets_a_content_check() {
    let mut rig = Rig::new(&[Tier::Ui]);
    let mut root = Ops::default();
    root.root(1, "mod");
    rig.apply_ok(&root);
    let mut image = Ops::default();
    image.create(2, 1, Kind::Image, &[(Prop::Image, text("mods/mod/a.png"))]);
    assert_eq!(
        rig.apply(&image),
        Err(Violation("an image that is not this server's content"))
    );
    let mut none = Ops::default();
    none.create(2, 1, Kind::Image, &[(Prop::Image, text(""))]);
    rig.apply_ok(&none);
    rig.realm.set_content_check(store("mods/mod/"));
    let mut set = Ops::default();
    set.set(2, &[(Prop::Image, text("mods/mod/a.png"))]);
    rig.apply_ok(&set);
}

/// A visible button at (10, 10) to act on, then `field_ops`' controls, with the text field of handle `field`; the player acts
/// on the button and the realm asks for focus.
fn ask_focus(field_ops: &Ops, field: u32) -> (Rig, Applied) {
    let mut rig = Rig::new(&[Tier::Ui]);
    rig.apply_ok(&button_ops());
    rig.apply_ok(field_ops);
    rig.click_at(20, 20);
    let mut focus = Ops::default();
    focus.focus(field);
    let applied = rig
        .apply(&focus)
        .expect("a refused focus is not a violation");
    (rig, applied)
}

fn field(handle: u32, parent: u32, props: &[(Prop, PropValue)]) -> Ops {
    let mut ops = Ops::default();
    ops.create(handle, parent, Kind::TextField, props);
    ops
}

#[test]
fn client_script_a_text_field_that_cannot_be_seen_takes_no_focus() {
    let seen = [
        (Prop::Pos, vec2(10.0, 60.0)),
        (Prop::Size, vec2(100.0, 20.0)),
    ];
    let with = |extra: (Prop, PropValue)| -> Vec<(Prop, PropValue)> {
        let mut props: Vec<_> = seen.to_vec();
        props.retain(|(p, _)| *p != extra.0);
        props.push(extra);
        props
    };
    let (rig, applied) = ask_focus(&field(3, 1, &seen), 3);
    assert_eq!(applied.focus_refused, 0, "a visible field takes the focus");
    assert!(rig.gui.0.borrow().client_script.focus.is_some());
    let cases = [
        ("opacity 0", with((Prop::Opacity, PropValue::Number(0.0)))),
        (
            "opacity under half",
            with((Prop::Opacity, PropValue::Number(0.4))),
        ),
        ("1 x 1", with((Prop::Size, vec2(1.0, 1.0)))),
        ("20 x 7", with((Prop::Size, vec2(20.0, 7.0)))),
        ("off screen", with((Prop::Pos, vec2(700.0, 60.0)))),
        ("half off screen", with((Prop::Pos, vec2(600.0, 60.0)))),
        ("above the screen", with((Prop::Pos, vec2(10.0, -10.0)))),
        ("not visible", with((Prop::Visible, PropValue::Bool(false)))),
    ];
    for (what, props) in cases {
        let (rig, applied) = ask_focus(&field(3, 1, &props), 3);
        assert_eq!(applied.focus_refused, 1, "{what}");
        assert!(rig.gui.0.borrow().client_script.focus.is_none(), "{what}");
    }
}

#[test]
fn client_script_a_text_field_clipped_by_its_parent_or_faint_with_it_takes_no_focus() {
    let mut parent = Ops::default();
    parent.create(
        3,
        1,
        Kind::Panel,
        &[
            (Prop::Pos, vec2(10.0, 60.0)),
            (Prop::Size, vec2(50.0, 50.0)),
        ],
    );
    let place = |pos: (f64, f64), size: (f64, f64)| {
        field(
            4,
            3,
            &[
                (Prop::Pos, vec2(pos.0, pos.1)),
                (Prop::Size, vec2(size.0, size.1)),
            ],
        )
    };
    let build = |field_ops: Ops, parent_opacity: f64| {
        let mut rig = Rig::new(&[Tier::Ui]);
        rig.apply_ok(&button_ops());
        rig.apply_ok(&parent);
        let mut dim = Ops::default();
        dim.set(3, &[(Prop::Opacity, PropValue::Number(parent_opacity))]);
        rig.apply_ok(&dim);
        rig.apply_ok(&field_ops);
        rig.click_at(20, 20);
        let mut focus = Ops::default();
        focus.focus(4);
        let applied = rig.apply(&focus).unwrap();
        (rig, applied)
    };
    let (_, applied) = build(place((5.0, 5.0), (40.0, 40.0)), 1.0);
    assert_eq!(applied.focus_refused, 0, "inside its parent");
    let (_, applied) = build(place((30.0, 30.0), (40.0, 40.0)), 1.0);
    assert_eq!(applied.focus_refused, 1, "past its parent's edge");
    let (_, applied) = build(place((-5.0, 5.0), (40.0, 40.0)), 1.0);
    assert_eq!(applied.focus_refused, 1, "past its parent's other edge");
    let (_, applied) = build(place((5.0, 5.0), (40.0, 40.0)), 0.0);
    assert_eq!(applied.focus_refused, 1, "its parent is invisible");
    let (_, applied) = build(place((5.0, 5.0), (40.0, 40.0)), 0.6);
    assert_eq!(
        applied.focus_refused, 0,
        "0.6 of the parent's opacity is enough"
    );
    let half = field(
        4,
        3,
        &[
            (Prop::Pos, vec2(5.0, 5.0)),
            (Prop::Size, vec2(40.0, 40.0)),
            (Prop::Opacity, PropValue::Number(0.6)),
        ],
    );
    let (_, applied) = build(half, 0.6);
    assert_eq!(applied.focus_refused, 1, "0.6 of 0.6 is under half");
}

#[test]
fn client_script_clicking_something_invisible_does_not_earn_the_focus() {
    let mut rig = Rig::new(&[Tier::Ui]);
    let mut ops = button_ops();
    ops.create(
        3,
        1,
        Kind::Button,
        &[
            (Prop::Pos, vec2(200.0, 150.0)),
            (Prop::Size, vec2(50.0, 30.0)),
            (Prop::Opacity, PropValue::Number(0.0)),
        ],
    )
    .create(
        4,
        1,
        Kind::TextField,
        &[
            (Prop::Pos, vec2(10.0, 60.0)),
            (Prop::Size, vec2(100.0, 20.0)),
        ],
    )
    .create(
        5,
        1,
        Kind::TextField,
        &[
            (Prop::Pos, vec2(10.0, 100.0)),
            (Prop::Size, vec2(100.0, 20.0)),
            (Prop::Opacity, PropValue::Number(0.0)),
        ],
    );
    rig.apply_ok(&ops);
    rig.events();
    rig.click_at(210, 160);
    assert_eq!(
        rig.events(),
        [UiEvent::Press(3)],
        "the invisible button still works"
    );
    let mut focus = Ops::default();
    focus.focus(4);
    assert_eq!(
        rig.apply(&focus).unwrap().focus_refused,
        1,
        "but it is no action of the player's"
    );
    // Nor does clicking an invisible text field focus it.
    rig.click_at(20, 110);
    assert!(rig.gui.0.borrow().client_script.focus.is_none());
}

#[test]
fn client_script_a_field_that_stops_being_seen_loses_the_focus_that_frame() {
    let changes: Vec<(&str, Vec<(Prop, PropValue)>)> = vec![
        ("opacity", vec![(Prop::Opacity, PropValue::Number(0.0))]),
        ("size", vec![(Prop::Size, vec2(1.0, 1.0))]),
        ("place", vec![(Prop::Pos, vec2(900.0, 60.0))]),
        ("visible", vec![(Prop::Visible, PropValue::Bool(false))]),
    ];
    for (what, props) in changes {
        let mut rig = Rig::new(&[Tier::Ui]);
        let mut ops = button_ops();
        ops.create(
            3,
            1,
            Kind::TextField,
            &[
                (Prop::Pos, vec2(10.0, 60.0)),
                (Prop::Size, vec2(100.0, 20.0)),
            ],
        );
        rig.apply_ok(&ops);
        rig.click_at(20, 70);
        assert!(
            rig.gui.0.borrow().client_script.focus.is_some(),
            "{what}: focused"
        );
        let mut change = Ops::default();
        change.set(3, &props);
        // `apply` runs one frame after the flush.
        rig.apply_ok(&change);
        assert!(
            rig.gui.0.borrow().client_script.focus.is_none(),
            "{what}: still focused"
        );
        rig.events();
        assert!(
            !rig.key(0x30, u16::from(b'a'), SI_MAKE, 0),
            "{what}: it takes no keys"
        );
        assert!(rig.events().is_empty());
    }
}

#[test]
fn client_script_the_chip_shows_a_typing_marker_while_a_field_has_focus() {
    let mut rig = Rig::new(&[Tier::Ui]);
    let mut ops = button_ops();
    ops.create(
        3,
        1,
        Kind::TextField,
        &[
            (Prop::Pos, vec2(10.0, 60.0)),
            (Prop::Size, vec2(100.0, 20.0)),
        ],
    );
    rig.apply_ok(&ops);
    assert!(chip_holds(&rig).is_empty());
    rig.click_at(20, 70);
    rig.frame();
    assert_eq!(chip_holds(&rig), ["typing"]);
    assert!(rig.draw().chip);
    let mut hide = Ops::default();
    hide.set(3, &[(Prop::Opacity, PropValue::Number(0.0))]);
    rig.apply_ok(&hide);
    assert!(chip_holds(&rig).is_empty());
}

// ---- occlusion

const FIELD_COLOR: [u8; 4] = [20, 22, 26, 255];
const COVER_COLOR: [u8; 4] = [200, 0, 0, 255];

impl Rig {
    /// The quads of one frame in drawing order.
    fn quads(&mut self) -> Vec<crate::draw::GuiQuad> {
        draw_canvas(&self.vm, &self.gui, &mut self.cache)
            .draws
            .batches
            .iter()
            .flat_map(|b| b.quads.iter().copied())
            .collect()
    }
}

/// The index of the last quad of `color` in `quads`.
fn last_quad(quads: &[crate::draw::GuiQuad], color: [u8; 4]) -> usize {
    quads
        .iter()
        .rposition(|q| q.color == color)
        .expect("a quad of the color")
}

/// A text field (handle 5 when `nested`, else 3) and, created after it, an opaque panel over it: a sibling of the field, or of
/// the container it is in.
fn covered_field(nested: bool) -> (Rig, u32, u32) {
    let mut rig = Rig::new(&[Tier::Ui]);
    let mut ops = button_ops();
    let (field, cover) = if nested {
        ops.create(
            3,
            1,
            Kind::Panel,
            &[
                (Prop::Pos, vec2(0.0, 50.0)),
                (Prop::Size, vec2(200.0, 60.0)),
            ],
        )
        .create(
            4,
            3,
            Kind::TextField,
            &[
                (Prop::Pos, vec2(10.0, 10.0)),
                (Prop::Size, vec2(100.0, 20.0)),
            ],
        );
        (4, 5)
    } else {
        ops.create(
            3,
            1,
            Kind::TextField,
            &[
                (Prop::Pos, vec2(10.0, 60.0)),
                (Prop::Size, vec2(100.0, 20.0)),
            ],
        );
        (3, 4)
    };
    ops.create(
        cover,
        1,
        Kind::Panel,
        &[
            (Prop::Pos, vec2(0.0, 50.0)),
            (Prop::Size, vec2(200.0, 60.0)),
            (Prop::Fill, PropValue::Color(COVER_COLOR)),
        ],
    );
    rig.apply_ok(&ops);
    (rig, field, cover)
}

#[test]
fn client_script_a_focused_field_is_drawn_and_hit_above_what_covers_it() {
    for nested in [false, true] {
        let (mut rig, field, cover) = covered_field(nested);
        let quads = rig.quads();
        assert!(
            last_quad(&quads, FIELD_COLOR) < last_quad(&quads, COVER_COLOR),
            "nested {nested}: not focused, the later panel covers the field"
        );
        // The click lands on the cover, which is the realm's right while nothing is focused.
        rig.events();
        rig.click_at(20, 70);
        assert!(
            rig.events().contains(&UiEvent::Click(cover)),
            "nested {nested}"
        );
        assert!(rig.gui.0.borrow().client_script.focus.is_none());
        // Focus the field the way the realm would, through a visible control's click.
        let mut focus = Ops::default();
        focus.focus(field);
        rig.click_at(20, 20);
        rig.apply_ok(&focus);
        assert_eq!(
            rig.gui.0.borrow().client_script.focus,
            Some(rig.object(field)),
            "nested {nested}"
        );
        let quads = rig.quads();
        assert!(
            last_quad(&quads, FIELD_COLOR) > last_quad(&quads, COVER_COLOR),
            "nested {nested}: focused, the field is drawn over the panel"
        );
        rig.events();
        rig.click_at(20, 70);
        assert!(
            rig.events().iter().all(|e| *e != UiEvent::Click(cover)),
            "nested {nested}: the field is hit first"
        );
        assert_eq!(
            rig.gui.0.borrow().client_script.focus,
            Some(rig.object(field))
        );
        // Without the focus the order is the realm's again.
        assert!(escape(&mut rig, SI_MAKE, 0));
        assert!(rig.gui.0.borrow().client_script.focus.is_none());
        let quads = rig.quads();
        assert!(
            last_quad(&quads, FIELD_COLOR) < last_quad(&quads, COVER_COLOR),
            "nested {nested}: back to normal"
        );
        rig.events();
        rig.click_at(20, 70);
        assert!(
            rig.events().contains(&UiEvent::Click(cover)),
            "nested {nested}: hit-testing is back to normal"
        );
    }
}

#[test]
fn client_script_a_text_field_takes_no_hidden_or_direction_characters() {
    let mut rig = Rig::new(&[Tier::Ui]);
    let mut ops = Ops::default();
    ops.root(1, "mod").create(
        2,
        1,
        Kind::TextField,
        &[
            (Prop::Pos, vec2(10.0, 10.0)),
            (Prop::Size, vec2(100.0, 20.0)),
        ],
    );
    rig.apply_ok(&ops);
    rig.click_at(20, 20);
    rig.events();
    for c in [
        '\u{202e}', '\u{200f}', '\u{2028}', '\u{2066}', '\u{85}', '\u{200b}', '\u{feff}',
    ] {
        assert!(
            rig.key(0x30, c as u16, SI_MAKE, 0),
            "{c:?} is taken, not typed"
        );
    }
    assert!(rig.key(0x30, u16::from(b'a'), SI_MAKE, 0));
    assert_eq!(
        rig.events(),
        [UiEvent::Change(2, Changed::Text("a".into()))]
    );
}

#[test]
fn client_script_the_chip_shows_a_servers_name_without_hidden_characters() {
    let mut rig = Rig::new(&[Tier::Ui]);
    rig.apply_ok(&button_ops());
    rig.gui
        .0
        .borrow_mut()
        .client_script
        .view
        .as_mut()
        .unwrap()
        .name = "Safe\u{202e}tab\nline\u{2028}x\u{2067}".to_string();
    rig.frame();
    let (_, chip) = chip_rect(&rig);
    let text = rig
        .gui
        .with_custom_state::<super::chip::ChipState, _>(chip, |s| format!("{} {}", s.label, s.name))
        .unwrap();
    assert!(
        text.chars()
            .all(|c| !torque_client_script_format::is_hidden(c)),
        "{text:?}"
    );
    assert!(text.contains("Safe tab line x"), "{text:?}");
}

// ---- holds, the chip's column, images and layout

/// A root, the button of [`button_ops`] and a text field (handle 3) at (10, 60) of 100 x 20.
fn field_ops() -> Ops {
    let mut ops = button_ops();
    ops.create(
        3,
        1,
        Kind::TextField,
        &[
            (Prop::Pos, vec2(10.0, 60.0)),
            (Prop::Size, vec2(100.0, 20.0)),
        ],
    );
    ops
}

fn changes(events: &[UiEvent]) -> usize {
    events
        .iter()
        .filter(|e| matches!(e, UiEvent::Change(..)))
        .count()
}

impl Rig {
    fn focus(&self) -> Option<ObjId> {
        self.gui.0.borrow().client_script.focus
    }

    /// Types `text` as presses and releases of one key.
    fn type_text(&mut self, text: &str) {
        for c in text.bytes() {
            self.key(0x30, u16::from(c), SI_MAKE, 0);
            self.key(0x30, u16::from(c), SI_BREAK, 0);
        }
    }

    /// A game script's value.
    fn value(&mut self, expr: &str) -> String {
        self.vm
            .eval(format!("$test::value = {expr};").as_bytes())
            .expect("the expression");
        String::from_utf8(self.vm.global("$test::value").unwrap_or_default()).unwrap()
    }

    /// The batches of one frame.
    fn batches(&mut self) -> Vec<crate::draw::GuiBatch> {
        draw_canvas(&self.vm, &self.gui, &mut self.cache)
            .draws
            .batches
    }
}

const GAME_EDIT: &str = "new GuiControlProfile(EditProfile) { canKeyFocus = 1; };";

#[test]
fn client_script_a_press_off_the_field_or_the_cursor_going_off_drops_the_focus() {
    let mut rig = Rig::new(&[Tier::Ui]);
    rig.game(&format!(
        "{GAME_EDIT} new GuiTextEditCtrl(GameEdit) {{ profile = \"EditProfile\"; position = \"300 300\"; \
         extent = \"200 20\"; enabled = 1; }}; PlayGui.add(GameEdit);"
    ));
    rig.apply_ok(&field_ops());
    rig.click_at(20, 70);
    assert!(rig.focus().is_some());
    rig.events();
    rig.click_at(310, 310);
    assert!(rig.focus().is_none(), "the press was not on the field");
    rig.type_text("hi");
    assert_eq!(changes(&rig.events()), 0, "the realm saw none of it");
    assert_eq!(rig.value("GameEdit.getValue()"), "hi");
    // Mouselook: the game turns the cursor off while the field has the focus.
    rig.click_at(20, 70);
    assert!(rig.focus().is_some());
    rig.events();
    rig.vm.eval(b"Canvas.cursorOff();").unwrap();
    rig.type_text("w");
    assert!(rig.focus().is_none());
    assert_eq!(changes(&rig.events()), 0);
    assert_eq!(
        rig.value("GameEdit.getValue()"),
        "hiw",
        "with the cursor off the game gets the key"
    );
    // And the frame drops it too.
    rig.vm.eval(b"Canvas.cursorOn();").unwrap();
    rig.click_at(20, 70);
    assert!(rig.focus().is_some());
    rig.game("Canvas.cursorOff();");
    assert!(rig.focus().is_none());
}

#[test]
fn client_script_a_text_field_is_a_leaf() {
    let mut rig = Rig::new(&[Tier::Ui]);
    rig.apply_ok(&field_ops());
    let mut under = Ops::default();
    under.create(4, 3, Kind::Panel, &[]);
    assert_eq!(
        rig.apply(&under),
        Err(Violation("a control under a text field"))
    );
    let mut panel = Ops::default();
    panel.create(4, 1, Kind::Panel, &[]);
    rig.apply_ok(&panel);
    let mut moved = Ops::default();
    moved.parent(4, 3);
    assert_eq!(
        rig.apply(&moved),
        Err(Violation("a control moved under a text field"))
    );
}

#[test]
fn client_script_a_field_in_the_chips_column_takes_no_focus() {
    let under_chip = [
        (Prop::Pos, vec2(500.0, 12.0)),
        (Prop::Size, vec2(100.0, 20.0)),
    ];
    let (mut rig, applied) = ask_focus(&field(3, 1, &under_chip), 3);
    assert_eq!(applied.focus_refused, 1);
    rig.click_at(590, 20);
    assert!(rig.focus().is_none(), "nor does a click there focus it");
    let edge = [
        (Prop::Pos, vec2(100.0, 60.0)),
        (Prop::Size, vec2(100.0, 20.0)),
    ];
    let (_, applied) = ask_focus(&field(3, 1, &edge), 3);
    assert_eq!(applied.focus_refused, 1, "partly in the column");
}

/// The quads of the batches whose texture's name starts with `prefix`.
fn textured(batches: &[crate::draw::GuiBatch], prefix: &str) -> Vec<crate::draw::GuiQuad> {
    batches
        .iter()
        .filter(|b| {
            b.texture
                .as_ref()
                .is_some_and(|t| t.name.starts_with(prefix))
        })
        .flat_map(|b| b.quads.iter().copied())
        .collect()
}

fn inside(q: &crate::draw::GuiQuad, r: [i32; 4]) -> bool {
    let [x0, y0, x1, y1] = q.rect;
    x0 >= r[0] as f32
        && y0 >= r[1] as f32
        && x1 <= (r[0] + r[2]) as f32
        && y1 <= (r[1] + r[3]) as f32
}

#[test]
fn client_script_the_typing_marker_stays_inside_the_chip() {
    let mut rig = Rig::new(&[Tier::Ui]);
    rig.gui
        .0
        .borrow_mut()
        .client_script
        .view
        .as_mut()
        .unwrap()
        .name = "W".repeat(64);
    rig.gui.client_script_hold(Hold::Camera, true);
    rig.gui.client_script_hold(Hold::Key, true);
    rig.apply_ok(&field_ops());
    rig.click_at(20, 70);
    rig.frame();
    assert_eq!(chip_holds(&rig), ["camera", "keys", "typing"]);
    let (chip, _) = chip_rect(&rig);
    let glyphs = textured(&rig.batches(), "font:");
    let marker: Vec<_> = glyphs
        .iter()
        .filter(|q| q.color == super::chip::HOLD)
        .collect();
    assert_eq!(
        marker.len(),
        "[camera, keys, typing]".len(),
        "every glyph of the marker is drawn"
    );
    assert!(marker.iter().all(|q| inside(q, chip)), "{chip:?}");
    let name: Vec<_> = glyphs
        .iter()
        .filter(|q| q.color == [140, 146, 158, 255])
        .collect();
    assert!(
        !name.is_empty() && name.len() < 64,
        "the name is cut to the room left: {}",
        name.len()
    );
    assert!(name.iter().all(|q| inside(q, chip)));
}

#[test]
fn client_script_a_fields_text_is_drawn_opaque_whatever_its_color() {
    let mut rig = Rig::new(&[Tier::Ui]);
    let mut ops = Ops::default();
    ops.root(1, "mod").create(
        2,
        1,
        Kind::TextField,
        &[
            (Prop::Pos, vec2(10.0, 60.0)),
            (Prop::Size, vec2(100.0, 20.0)),
            (Prop::Color, PropValue::Color([0, 0, 0, 0])),
            (Prop::Text, text("abc")),
        ],
    );
    rig.apply_ok(&ops);
    let glyphs: Vec<_> = textured(&rig.batches(), "font:")
        .into_iter()
        .filter(|q| inside(q, [10, 60, 100, 20]))
        .collect();
    assert_eq!(glyphs.len(), 3);
    assert!(glyphs.iter().all(|q| q.color == [235, 238, 242, 255]));
}

#[test]
fn client_script_after_escape_the_realm_waits_for_the_player_to_take_the_cursor() {
    let mut rig = Rig::new(&[Tier::Ui]);
    rig.gui.0.borrow_mut().cursor_on = false;
    let mut ops = button_ops();
    ops.interactive(true);
    rig.apply_ok(&ops);
    assert!(rig.gui.client_script_holds().cursor);
    assert!(escape(&mut rig, SI_MAKE, 0));
    assert!(escape(&mut rig, SI_BREAK, 0));
    let mut again = Ops::default();
    again.interactive(true);
    assert_eq!(rig.apply(&again).unwrap().interactive_refused, 1);
    assert!(!rig.gui.client_script_holds().cursor);
    assert!(!rig.gui.0.borrow().cursor_on);
    assert!(!escape(&mut rig, SI_MAKE, 0), "the game gets the key");
    escape(&mut rig, SI_BREAK, 0);
    // The player's own click lets the realm ask again; a dialog drop is like Escape.
    rig.vm.eval(b"Canvas.cursorOn();").unwrap();
    rig.click_at(20, 20);
    assert_eq!(rig.apply(&again).unwrap().interactive_refused, 0);
    rig.game("Canvas.pushDialog(DialogOne);");
    rig.game("Canvas.popLayer(0);");
    assert_eq!(rig.apply(&again).unwrap().interactive_refused, 1);
}

#[test]
fn client_script_giving_the_cursor_back_never_undoes_the_games_call() {
    let mut rig = Rig::new(&[Tier::Ui]);
    rig.gui.0.borrow_mut().cursor_on = false;
    let mut ops = button_ops();
    ops.interactive(true);
    rig.apply_ok(&ops);
    assert!(rig.gui.client_script_holds().cursor);
    rig.game("Canvas.cursorOn(); Canvas.pushDialog(DialogOne);");
    rig.frame();
    assert!(!rig.gui.client_script_holds().cursor);
    assert!(
        rig.gui.0.borrow().cursor_on,
        "the game asked for the cursor"
    );
}

#[test]
fn client_script_the_server_layer_is_neither_drawn_nor_hit_in_the_chips_column() {
    let mut rig = Rig::new(&[Tier::Ui]);
    let mut ops = Ops::default();
    ops.root(1, "mod").create(
        2,
        1,
        Kind::Panel,
        &[
            (Prop::Pos, vec2(0.0, 0.0)),
            (Prop::Size, vec2(640.0, 200.0)),
            (Prop::Fill, PropValue::Color(COVER_COLOR)),
        ],
    );
    rig.apply_ok(&ops);
    let column = rig.gui.0.borrow().client_script.column.expect("a column");
    let (chip, _) = chip_rect(&rig);
    assert_eq!(
        crate::draw::intersect_rect(column, chip),
        chip,
        "the column holds the chip"
    );
    assert!(column[3] >= 22 + 3 * 20, "and its open menu");
    let meets = |q: &crate::draw::GuiQuad| {
        let [x0, y0, x1, y1] = q.rect;
        x0 < (column[0] + column[2]) as f32
            && x1 > column[0] as f32
            && y0 < (column[1] + column[3]) as f32
            && y1 > column[1] as f32
    };
    let quads = rig.quads();
    let cover: Vec<_> = quads.iter().filter(|q| q.color == COVER_COLOR).collect();
    assert!(!cover.is_empty(), "the panel is drawn outside the column");
    assert!(cover.iter().all(|q| !meets(q)), "{cover:?}");
    rig.events();
    rig.click_at(column[0] + 4, column[1] + 4);
    assert!(rig.events().is_empty(), "no click in the column");
    rig.click_at(20, 150);
    assert!(rig.events().contains(&UiEvent::Click(2)));
}

#[test]
fn client_script_images_come_from_the_servers_store_alone() {
    let mut rig = Rig::new(&[Tier::Ui]);
    let files = torque_ts_vm::host::MemFiles::new();
    for path in ["mods/mod/a.png", "mods/mod/b.png", "mods/mod/c.png"] {
        files.insert(path, &png(2, 2, [255, 0, 0]));
    }
    rig.vm.set_file_host(files);
    let green: Arc<[u8]> = png(2, 2, [10, 200, 30]).into();
    rig.realm.set_content_check(Box::new(move |p| match p {
        "mods/mod/b.png" => Some(Arc::clone(&green)),
        "mods/mod/c.png" => Some(Arc::from(&b"not an image"[..])),
        _ => None,
    }));
    let mut root = Ops::default();
    root.root(1, "mod");
    rig.apply_ok(&root);
    let image = |handle: u32, path: &str, y: f64| {
        let mut ops = Ops::default();
        ops.create(
            handle,
            1,
            Kind::Image,
            &[
                (Prop::Image, text(path)),
                (Prop::Pos, vec2(10.0, y)),
                (Prop::Size, vec2(40.0, 40.0)),
            ],
        );
        ops
    };
    assert_eq!(
        rig.apply(&image(2, "mods/mod/a.png", 100.0)),
        Err(Violation("an image that is not this server's content")),
        "the game has the file, the store does not"
    );
    rig.apply_ok(&image(2, "mods/mod/b.png", 100.0));
    rig.apply_ok(&image(3, "mods/mod/c.png", 200.0));
    let batches = rig.batches();
    assert!(
        batches
            .iter()
            .filter_map(|b| b.texture.as_ref())
            .all(|t| !t.name.starts_with("mods/")),
        "no texture is the game's file"
    );
    let realm: Vec<_> = batches
        .iter()
        .filter_map(|b| b.texture.as_ref())
        .filter(|t| t.name.starts_with("realm:"))
        .collect();
    assert_eq!(
        realm.len(),
        1,
        "the store's image; the bytes that are no image draw nothing"
    );
    assert_eq!(&realm[0].rgba[..3], &[10, 200, 30], "the store's pixels");
    assert!(
        textured(&batches, "realm:")
            .iter()
            .all(|q| inside(q, [10, 100, 40, 40]))
    );
}

#[test]
fn client_script_wrapped_text_is_measured_once_a_word_and_kept() {
    let words = 300;
    let mut calls = 0;
    let lines = super::paint::wrap_plain(&"ab ".repeat(words), Some(40), |w| {
        calls += 1;
        w.len() as i32 * 6
    });
    assert_eq!(calls, words);
    assert_eq!(lines.len(), words / 2);
    assert!(lines.iter().all(|(_, w)| *w <= 40));
    let mut rig = Rig::new(&[Tier::Ui]);
    let mut ops = Ops::default();
    ops.root(1, "mod").create(
        2,
        1,
        Kind::Text,
        &[
            (Prop::Pos, vec2(10.0, 100.0)),
            (Prop::Size, vec2(40.0, 300.0)),
            (Prop::Wrap, PropValue::Bool(true)),
            (Prop::Text, text(&"ab ".repeat(words))),
        ],
    );
    rig.apply_ok(&ops);
    let first = rig.draw().text_measures as usize;
    let again = rig.draw().text_measures as usize;
    assert!(
        first - again <= words + lines.len(),
        "{first} then {again} measures"
    );
    assert!(again < 10, "the layout is kept: {again} measures");
    let mut set = Ops::default();
    set.set(2, &[(Prop::Text, text("ab ab"))]);
    rig.apply_ok(&set);
    assert!(
        rig.draw().text_measures as usize > again,
        "a new text is laid out again"
    );
}

#[test]
fn client_script_a_click_on_a_clear_panel_earns_no_focus() {
    for (fill, refused) in [([0, 0, 0, 0], 1), ([30, 30, 30, 255], 0)] {
        let mut rig = Rig::new(&[Tier::Ui]);
        let mut ops = Ops::default();
        ops.root(1, "mod")
            .create(
                2,
                1,
                Kind::Panel,
                &[
                    (Prop::Pos, vec2(10.0, 10.0)),
                    (Prop::Size, vec2(100.0, 30.0)),
                    (Prop::Fill, PropValue::Color(fill)),
                ],
            )
            .create(
                3,
                1,
                Kind::TextField,
                &[
                    (Prop::Pos, vec2(10.0, 60.0)),
                    (Prop::Size, vec2(100.0, 20.0)),
                ],
            );
        rig.apply_ok(&ops);
        rig.events();
        rig.click_at(20, 20);
        assert!(rig.events().contains(&UiEvent::Click(2)), "{fill:?}");
        let mut focus = Ops::default();
        focus.focus(3);
        assert_eq!(
            rig.apply(&focus).unwrap().focus_refused,
            refused,
            "{fill:?}"
        );
    }
}

#[test]
fn client_script_a_game_keys_release_reaches_the_game_when_the_focus_came_after_its_press() {
    let mut rig = Rig::new(&[Tier::Ui]);
    rig.apply_ok(&field_ops());
    let w = crate::keys::KEY_A + 22;
    assert!(!rig.key(w, u16::from(b'w'), SI_MAKE, 0), "the game's press");
    rig.click_at(20, 70);
    assert!(rig.focus().is_some());
    rig.events();
    rig.key(w, u16::from(b'w'), crate::keys::SI_REPEAT, 0);
    assert_eq!(changes(&rig.events()), 0, "nor its repeats");
    assert!(
        !rig.key(w, u16::from(b'w'), SI_BREAK, 0),
        "the release reaches the canvas"
    );
    // A press the field took has its release kept, even once the focus is gone.
    assert!(rig.key(w, u16::from(b'w'), SI_MAKE, 0));
    rig.click_at(500, 400);
    assert!(rig.focus().is_none());
    assert!(rig.key(w, u16::from(b'w'), SI_BREAK, 0));
}

#[test]
fn client_script_a_prompt_takes_the_keys_and_escape_is_the_hosts() {
    let mut rig = Rig::new(&[Tier::Ui]);
    rig.apply_ok(&field_ops());
    rig.game(&format!(
        "{GAME_EDIT} new GuiControl(Prompt) {{ position = \"0 0\"; extent = \"640 480\"; \
         new GuiTextEditCtrl(PromptEdit) {{ profile = \"EditProfile\"; position = \"100 300\"; extent = \"200 20\"; \
         enabled = 1; }}; }};"
    ));
    let prompt = rig.vm.objects().find(b"Prompt").unwrap();
    rig.vm
        .objects_mut()
        .remove(rig.gui.canvas().unwrap(), prompt);
    let held = crate::keys::KEY_A + 3;
    assert!(
        !rig.key(held, u16::from(b'd'), SI_MAKE, 0),
        "the game's press"
    );
    assert!(rig.gui.push_engine_layer(&mut rig.vm, prompt));
    rig.frame();
    assert!(rig.click_at(110, 310), "the prompt's field");
    rig.events();
    assert!(
        rig.key(0x30, u16::from(b'o'), SI_MAKE, 0),
        "the prompt takes the key"
    );
    assert!(rig.key(0x30, u16::from(b'o'), SI_BREAK, 0));
    assert_eq!(rig.value("PromptEdit.getValue()"), "o");
    assert!(
        !rig.key(held, u16::from(b'd'), SI_BREAK, 0),
        "the release of the game's press is the game's"
    );
    assert!(escape(&mut rig, SI_MAKE, 0), "Escape is the host's");
    assert!(escape(&mut rig, SI_BREAK, 0));
    assert_eq!(rig.value("PromptEdit.getValue()"), "o");
    assert!(rig.events().iter().all(|e| !e.is_input()));
    rig.gui.pop_engine_layer(&mut rig.vm, prompt);
    assert!(
        !escape(&mut rig, SI_MAKE, 0),
        "with no prompt and nothing held, the game's"
    );
}

#[test]
fn client_script_the_server_layer_fonts_are_markup_and_never_the_games() {
    let mut rig = Rig::new(&[Tier::Ui]);
    let mut ops = Ops::default();
    ops.root(1, "mod");
    for size in 8..=72u32 {
        ops.create(
            size - 6,
            1,
            Kind::Text,
            &[
                (Prop::Text, text("hi")),
                (Prop::FontSize, PropValue::Number(f64::from(size))),
            ],
        );
    }
    rig.apply_ok(&ops);
    // A frame loads at most its share of new markup fonts; the rest come in the next frames.
    let mut keys = Vec::new();
    for _ in 0..12 {
        rig.draw();
        keys.push(rig.cache.fonts.cache().server_usage().1);
    }
    let steps: Vec<usize> = keys.windows(2).map(|w| w[1] - w[0]).collect();
    assert!(
        keys[0] <= crate::draw::MAX_FRAME_MARKUP_LOADS as usize,
        "{keys:?}"
    );
    assert!(
        steps
            .iter()
            .all(|&s| s <= crate::draw::MAX_FRAME_MARKUP_LOADS as usize),
        "{keys:?}"
    );
    // The sizes the realm named are kept under the server's share of the markup quota (a size the engine also uses is the
    // engine's), so the game's fonts are never the ones it fills.
    let (_, keys) = rig.cache.fonts.cache().server_usage();
    assert!(keys >= 60, "{keys} server fonts");
    assert!(keys <= torque_fonts::MAX_SERVER_FONTS);
}
