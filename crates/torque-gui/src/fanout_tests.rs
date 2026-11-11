use std::cell::RefCell;

use torque_engine::{EngineBuilder, Plugin, PluginId};

use super::*;
use crate::DrawCache;
use crate::draw::draw_canvas;
use crate::test_rig::{id, rig};

type Log = Rc<RefCell<Vec<String>>>;

/// An extension that answers what it was given and records what it was told, under its name.
#[derive(Default)]
struct Probe {
    name: &'static str,
    log: Log,
    canvas: Option<(i32, i32)>,
    rule: bool,
    message: Option<&'static str>,
    missing_bitmap: Option<&'static str>,
    classic_boxes: bool,
    fonts: bool,
    fail: bool,
    outline: Option<[u8; 4]>,
    pad: Option<(i32, i32)>,
    key_press: bool,
    active: bool,
    fill: bool,
    resize: bool,
    unclipped: bool,
    opacity: Option<f32>,
    shift: (i32, i32),
    stamp: Option<u64>,
    rects: bool,
}

impl Probe {
    fn named(name: &'static str, log: &Log) -> Probe {
        Probe {
            name,
            log: Rc::clone(log),
            ..Probe::default()
        }
    }

    fn note(&self, what: &str) {
        self.log.borrow_mut().push(format!("{}:{what}", self.name));
    }

    fn result(&self, what: &str) -> Result<(), VmError> {
        self.note(what);
        if self.fail {
            Err(VmError::Unsupported("probe"))
        } else {
            Ok(())
        }
    }
}

impl GuiExtension for Probe {
    fn into_any(self: Rc<Self>) -> Rc<dyn Any> {
        self
    }

    fn fonts(&self) -> Option<Box<dyn FontSource>> {
        self.fonts.then(crate::ext::default_fonts)
    }

    fn default_canvas_size(&self) -> (i32, i32) {
        self.canvas.unwrap_or(crate::DEFAULT_CANVAS_SIZE)
    }

    fn rule(&self, _rule: GuiRule) -> bool {
        self.rule
    }

    fn message(&self, _message: &GuiMessage<'_>) -> Option<String> {
        self.message.map(str::to_string)
    }

    fn missing_bitmap(&self) -> Option<&'static str> {
        self.missing_bitmap
    }

    fn classic_boxes(&self, _vm: &Vm, _profile: ObjId) -> bool {
        self.classic_boxes
    }

    fn removed(&self, _id: ObjId) {
        self.note("removed");
    }

    fn before_control(
        &self,
        _d: &mut DrawCtx<'_, '_>,
        id: ObjId,
        _offset: (i32, i32),
        _update: [i32; 4],
    ) {
        self.note(&format!("before_control {id}"));
    }

    fn scrolled(&self, _vm: &Vm, _gui: &Gui, _id: ObjId, _moved: (i32, i32)) {
        self.note("scrolled");
    }

    fn thumb_dragged(&self, _gui: &Gui, _id: ObjId) {
        self.note("thumb_dragged");
    }

    fn mouse_pressed(&self, _gui: &Gui) {
        self.note("mouse_pressed");
    }

    fn focused(&self, _vm: &mut Vm, _gui: &Gui, _id: ObjId) -> Result<(), VmError> {
        self.result("focused")
    }

    fn before_canvas(&self, _vm: &mut Vm) -> Result<(), VmError> {
        self.result("before_canvas")
    }

    fn text_outline(&self, _vm: &Vm, _profile: ObjId) -> Option<[u8; 4]> {
        self.outline
    }

    fn list_pad(&self, _vm: &Vm, _profile: &[u8]) -> Option<(i32, i32)> {
        self.pad
    }

    fn key_press(&self, _vm: &Vm, _id: ObjId) -> bool {
        self.key_press
    }

    fn starts_active(&self, _vm: &Vm, _id: ObjId) -> bool {
        self.active
    }

    fn replaces_fill(&self, _vm: &Vm, _profile: ObjId) -> bool {
        self.fill
    }

    fn ml_auto_resize(&self, _vm: &Vm, _id: ObjId) -> bool {
        self.resize
    }

    fn clip_to_parent(&self, _vm: &Vm, _id: ObjId) -> bool {
        !self.unclipped
    }

    fn opacity(&self, _vm: &Vm, _id: ObjId) -> Option<f32> {
        self.opacity
    }

    fn shift(&self, _gui: &Gui, _id: ObjId, _now: u32) -> (i32, i32) {
        self.shift
    }

    fn layer_stamp(&self, _gui: &Gui, _now: u32) -> Option<u64> {
        self.stamp
    }

    fn scroll_rects(&self, s: &mut ScrollState, _ext: (i32, i32), _border: i32) -> bool {
        if self.rects {
            s.thickness = i32::try_from(self.name.len()).unwrap();
        }
        self.rects
    }
}

/// A GUI with `primary` and `added` as its extensions, and a vm to ask them with.
fn guis(primary: Probe, added: Vec<Probe>) -> (Vm, Gui) {
    let (vm, gui) = rig("");
    gui.set_extension(Rc::new(primary));
    for ext in added {
        gui.add_extension(Rc::new(ext));
    }
    (vm, gui)
}

fn taken(log: &Log) -> Vec<String> {
    std::mem::take(&mut *log.borrow_mut())
}

#[test]
fn fanout_ext_is_the_primary_itself_while_nothing_is_added() {
    let (_, gui) = rig("");
    let primary: Rc<dyn GuiExtension> = Rc::new(Probe::default());
    gui.set_extension(Rc::clone(&primary));
    assert!(Rc::ptr_eq(&gui.ext(), &primary));
    gui.add_extension(Rc::new(Probe::default()));
    assert!(!Rc::ptr_eq(&gui.ext(), &primary));
}

#[test]
fn fanout_a_new_primary_keeps_the_added_ones() {
    let log = Log::default();
    let (vm, gui) = guis(Probe::named("a", &log), vec![Probe::named("b", &log)]);
    gui.set_extension(Rc::new(Probe::named("c", &log)));
    gui.ext().removed(id(&vm, "Top"));
    assert_eq!(taken(&log), ["c:removed", "b:removed"]);
}

#[test]
fn fanout_extension_finds_the_primary_or_the_first_added_of_a_type() {
    struct Other;
    impl GuiExtension for Other {
        fn into_any(self: Rc<Self>) -> Rc<dyn Any> {
            self
        }
    }
    let log = Log::default();
    let (_, gui) = guis(Probe::named("a", &log), vec![Probe::named("b", &log)]);
    assert_eq!(gui.extension::<Probe>().unwrap().name, "a");
    assert!(gui.extension::<Other>().is_none());
    gui.add_extension(Rc::new(Other));
    assert!(gui.extension::<Other>().is_some());
    assert_eq!(gui.extension::<Probe>().unwrap().name, "a");
    gui.set_extension(Rc::new(Other));
    assert_eq!(gui.extension::<Probe>().unwrap().name, "b");
}

#[test]
fn fanout_game_wide_methods_are_the_primarys_alone() {
    let log = Log::default();
    let (vm, gui) = guis(
        Probe::named("a", &log),
        vec![Probe {
            canvas: Some((800, 600)),
            rule: true,
            message: Some("added"),
            missing_bitmap: Some("added"),
            classic_boxes: true,
            fonts: true,
            ..Probe::named("b", &log)
        }],
    );
    let ext = gui.ext();
    let top = id(&vm, "Top");
    let message = GuiMessage::InvalidCursor { name: "x" };
    assert_eq!(ext.default_canvas_size(), crate::DEFAULT_CANVAS_SIZE);
    assert!(!ext.rule(GuiRule::TgeEditKeys));
    assert_eq!(ext.message(&message), None);
    assert_eq!(ext.missing_bitmap(), None);
    assert!(!ext.classic_boxes(&vm, top));
    assert!(ext.fonts().is_none());

    let (_, gui) = guis(
        Probe {
            canvas: Some((640, 480)),
            rule: true,
            message: Some("primary"),
            ..Probe::named("a", &log)
        },
        vec![Probe::named("b", &log)],
    );
    let ext = gui.ext();
    assert_eq!(ext.default_canvas_size(), (640, 480));
    assert!(ext.rule(GuiRule::TgeEditKeys));
    assert_eq!(ext.message(&message).as_deref(), Some("primary"));
}

#[test]
fn fanout_an_added_extension_does_not_change_the_fonts_or_canvas() {
    let log = Log::default();
    let (_, gui) = rig("");
    let before = gui.0.borrow().canvas_size;
    gui.add_extension(Rc::new(Probe {
        canvas: Some((1, 1)),
        fonts: true,
        ..Probe::named("b", &log)
    }));
    assert_eq!(gui.0.borrow().canvas_size, before);
}

#[test]
fn fanout_notices_reach_every_member_in_add_order() {
    let log = Log::default();
    let (vm, gui) = guis(
        Probe::named("a", &log),
        vec![Probe::named("b", &log), Probe::named("c", &log)],
    );
    let ext = gui.ext();
    let top = id(&vm, "Top");
    ext.removed(top);
    ext.scrolled(&vm, &gui, top, (0, 1));
    ext.thumb_dragged(&gui, top);
    ext.mouse_pressed(&gui);
    let want: Vec<String> = ["removed", "scrolled", "thumb_dragged", "mouse_pressed"]
        .iter()
        .flat_map(|what| ["a", "b", "c"].map(|who| format!("{who}:{what}")))
        .collect();
    assert_eq!(taken(&log), want);
}

#[test]
fn fanout_before_control_runs_in_add_order_for_every_control() {
    let log = Log::default();
    let (vm, gui) = guis(
        Probe::named("a", &log),
        vec![Probe::named("b", &log), Probe::named("c", &log)],
    );
    let mut cache = DrawCache::for_gui(&gui);
    draw_canvas(&vm, &gui, &mut cache);
    let top = id(&vm, "Top");
    let drawn = taken(&log);
    assert_eq!(
        drawn[..3],
        [
            format!("a:before_control {top}"),
            format!("b:before_control {top}"),
            format!("c:before_control {top}"),
        ]
    );
    assert_eq!(drawn.len() % 3, 0);
}

#[test]
fn fanout_a_failing_member_stops_the_ones_after_it() {
    let log = Log::default();
    let (mut vm, gui) = guis(
        Probe::named("a", &log),
        vec![
            Probe {
                fail: true,
                ..Probe::named("b", &log)
            },
            Probe::named("c", &log),
        ],
    );
    let ext = gui.ext();
    let top = id(&vm, "Top");
    assert!(ext.focused(&mut vm, &gui, top).is_err());
    assert!(ext.before_canvas(&mut vm).is_err());
    assert_eq!(
        taken(&log),
        [
            "a:focused",
            "b:focused",
            "a:before_canvas",
            "b:before_canvas"
        ]
    );
}

#[test]
fn fanout_the_first_answer_wins() {
    let log = Log::default();
    let (vm, gui) = guis(
        Probe::named("a", &log),
        vec![
            Probe {
                outline: Some([1, 2, 3, 4]),
                pad: Some((5, 6)),
                key_press: true,
                rects: true,
                ..Probe::named("bb", &log)
            },
            Probe {
                outline: Some([9, 9, 9, 9]),
                pad: Some((7, 7)),
                rects: true,
                ..Probe::named("ccc", &log)
            },
        ],
    );
    let ext = gui.ext();
    let top = id(&vm, "Top");
    assert_eq!(ext.text_outline(&vm, top), Some([1, 2, 3, 4]));
    assert_eq!(ext.list_pad(&vm, b"p"), Some((5, 6)));
    assert!(ext.key_press(&vm, top));
    let mut scroll = ScrollState::default();
    assert!(ext.scroll_rects(&mut scroll, (10, 10), 0));
    assert_eq!(
        scroll.thickness, 2,
        "the member after the first answer did not run"
    );
    assert_eq!(ext.bitmap_fit(&vm, top), None);
    assert_eq!(ext.ml_max_bitmap_height(&vm, top), None);
}

#[test]
fn fanout_the_primary_answers_before_the_added_ones() {
    let log = Log::default();
    let (vm, gui) = guis(
        Probe {
            outline: Some([1, 1, 1, 1]),
            ..Probe::named("a", &log)
        },
        vec![Probe {
            outline: Some([2, 2, 2, 2]),
            ..Probe::named("b", &log)
        }],
    );
    assert_eq!(
        gui.ext().text_outline(&vm, id(&vm, "Top")),
        Some([1, 1, 1, 1])
    );
}

#[test]
fn fanout_any_true_wins_for_the_flags() {
    let log = Log::default();
    let (vm, gui) = guis(
        Probe::named("a", &log),
        vec![Probe::named("b", &log), Probe::named("c", &log)],
    );
    let top = id(&vm, "Top");
    let ext = gui.ext();
    assert!(!ext.starts_active(&vm, top));
    assert!(!ext.replaces_fill(&vm, top));
    assert!(!ext.ml_auto_resize(&vm, top));

    let (vm, gui) = guis(
        Probe::named("a", &log),
        vec![
            Probe::named("b", &log),
            Probe {
                active: true,
                fill: true,
                resize: true,
                ..Probe::named("c", &log)
            },
        ],
    );
    let ext = gui.ext();
    let top = id(&vm, "Top");
    assert!(ext.starts_active(&vm, top));
    assert!(ext.replaces_fill(&vm, top));
    assert!(ext.ml_auto_resize(&vm, top));
}

#[test]
fn fanout_a_control_is_clipped_unless_a_member_says_no() {
    let log = Log::default();
    let (vm, gui) = guis(
        Probe {
            unclipped: true,
            ..Probe::named("a", &log)
        },
        vec![Probe::named("b", &log)],
    );
    let top = id(&vm, "Top");
    assert!(
        !gui.ext().clip_to_parent(&vm, top),
        "the default of b does not undo the no of a"
    );
    let (vm, gui) = guis(Probe::named("a", &log), vec![Probe::named("b", &log)]);
    assert!(gui.ext().clip_to_parent(&vm, id(&vm, "Top")));
}

#[test]
fn fanout_opacities_multiply_and_shifts_add() {
    let log = Log::default();
    let (vm, gui) = guis(
        Probe {
            opacity: Some(0.5),
            shift: (1, 2),
            ..Probe::named("a", &log)
        },
        vec![
            Probe::named("b", &log),
            Probe {
                opacity: Some(0.5),
                shift: (10, 20),
                ..Probe::named("c", &log)
            },
        ],
    );
    let top = id(&vm, "Top");
    let ext = gui.ext();
    assert_eq!(ext.opacity(&vm, top), Some(0.25));
    assert_eq!(ext.shift(&gui, top, 0), (11, 22));

    let (vm, gui) = guis(Probe::named("a", &log), vec![Probe::named("b", &log)]);
    let top = id(&vm, "Top");
    assert_eq!(gui.ext().opacity(&vm, top), None);
    assert_eq!(gui.ext().shift(&gui, top, 0), (0, 0));
}

#[test]
fn fanout_the_layer_stamp_follows_every_answer_in_order() {
    let log = Log::default();
    let stamped = |a, b| {
        let (_, gui) = guis(
            Probe {
                stamp: a,
                ..Probe::named("a", &log)
            },
            vec![Probe {
                stamp: b,
                ..Probe::named("b", &log)
            }],
        );
        gui.ext().layer_stamp(&gui, 0)
    };
    assert_eq!(stamped(None, None), None);
    assert_eq!(
        stamped(Some(1), None),
        None,
        "a member that cannot say leaves the layer unsaid"
    );
    assert_eq!(stamped(None, Some(1)), None);
    let one = stamped(Some(1), Some(2)).unwrap();
    assert_eq!(
        stamped(Some(1), Some(2)),
        Some(one),
        "the same answers, the same stamp"
    );
    assert_ne!(stamped(Some(1), Some(3)), Some(one), "an answer changed");
    assert_ne!(
        stamped(Some(2), Some(1)),
        Some(one),
        "the order is part of it"
    );
}

#[derive(Clone, Copy, Debug, Default, PluginId)]
#[plugin(id = "test.primary-one")]
struct PrimaryOne;

#[derive(Clone, Copy, Debug, Default, PluginId)]
#[plugin(id = "test.primary-two")]
struct PrimaryTwo;

#[derive(Clone, Copy, Debug, Default, PluginId)]
#[plugin(id = "test.added")]
struct Added;

impl Plugin for PrimaryOne {
    fn build(&self, engine: &mut EngineBuilder) {
        engine.gui_primary(|_, _| Rc::new(Probe::named("one", &Log::default())));
    }
}

impl Plugin for PrimaryTwo {
    fn build(&self, engine: &mut EngineBuilder) {
        engine.gui_primary(|_, _| Rc::new(Probe::named("two", &Log::default())));
    }
}

impl Plugin for Added {
    fn build(&self, engine: &mut EngineBuilder) {
        engine.gui_extension(|_, _| Rc::new(Probe::named("x", &Log::default())));
    }
}

#[test]
fn fanout_a_second_primary_fails_the_build_naming_both_plugins() {
    let error = EngineBuilder::new()
        .add_plugins(PrimaryOne)
        .add_plugins(PrimaryTwo)
        .build()
        .unwrap_err();
    let text = error.to_string();
    assert!(text.contains("test.primary-one"), "{text}");
    assert!(text.contains("test.primary-two"), "{text}");
}

#[test]
fn fanout_installed_extensions_are_made_for_each_gui_in_order() {
    let engine = EngineBuilder::new()
        .add_plugins(PrimaryOne)
        .add_plugins(Added)
        .build()
        .unwrap();
    for _ in 0..2 {
        let (mut vm, gui) = rig("");
        gui.install_extensions(&mut vm, &engine);
        let names: Vec<_> = gui
            .0
            .borrow()
            .ext
            .members()
            .filter_map(|m| Rc::clone(m).into_any().downcast::<Probe>().ok())
            .map(|p| p.name)
            .collect();
        assert_eq!(names, ["one", "x"]);
    }
}
